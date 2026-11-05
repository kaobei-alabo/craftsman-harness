//! What the `execute` tool captures from a command and how a large result is
//! cut for the model.
//!
//! Two formats (`shell.output_format`, see `shell.rs`):
//!
//! - **legacy** (default): each pipe is read up to 2x the output budget and
//!   the formatted text is cut at the budget from the front
//!   (`tools::clamp`), which loses the end of stdout, all of stderr and the
//!   exit code of a long result. Byte-identical to the behaviour before P2b.
//! - **head_tail**: each pipe is read up to [`SPILL_LIMIT`]; the model sees
//!   the head and the tail of stdout, the head and tail of stderr and the
//!   exit code, and the full output of every cut stream is kept in a spill
//!   file under `$TMPDIR/craftsman-<run>/` (directory 0700, files 0600 on Unix)
//!   whose path the result names. A result that fits the budget is the same
//!   text in both formats.
//!
//! Frontend spill files are left in place when craftsman exits. The private headless
//! runner uses bounded memory-only capture, including formatter/timeout paths;
//! no raw spill authority is granted before seeded redaction. No `exec` CLI or
//! `--ephemeral` option exists in this build.

use std::collections::VecDeque;
use std::path::PathBuf;

/// The model-facing budget of one result, in bytes (the legacy clamp limit).
pub(crate) const OUTPUT_BUDGET: usize = 50_000;
/// head_tail: the most read from one pipe; past it the pipe is closed (the
/// command gets EPIPE) so a firehose cannot fill the disk.
pub(crate) const SPILL_LIMIT: u64 = 64 * 1024 * 1024;
/// legacy: the most read from one pipe (today's 2x-budget cap).
pub(crate) const LEGACY_READ_LIMIT: u64 = (OUTPUT_BUDGET * 2) as u64;

/// One pipe's capture while the command runs.
#[derive(Debug)]
pub(crate) struct Capture {
    stream: &'static str,
    call: u64,
    head_cap: usize,
    tail_cap: usize,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: u64,
    spill: Option<Result<(PathBuf, std::fs::File), String>>,
    spill_enabled: bool,
}

impl Capture {
    /// Today's capture: the first [`LEGACY_READ_LIMIT`] bytes, nothing else.
    pub(crate) fn legacy(stream: &'static str) -> Self {
        Self {
            stream,
            call: 0,
            head_cap: LEGACY_READ_LIMIT as usize,
            tail_cap: 0,
            head: Vec::new(),
            tail: VecDeque::new(),
            total: 0,
            spill: None,
            spill_enabled: false,
        }
    }

    /// head_tail: a budget-sized head and tail in memory, the rest streamed
    /// to a spill file once the head is full.
    pub(crate) fn head_tail(stream: &'static str, call: u64) -> Self {
        Self {
            stream,
            call,
            head_cap: OUTPUT_BUDGET,
            tail_cap: OUTPUT_BUDGET,
            head: Vec::new(),
            tail: VecDeque::new(),
            total: 0,
            spill: None,
            spill_enabled: true,
        }
    }

    /// Test-only legacy in-memory head/tail, without durable spill authority.
    /// Production private shell capture now lives in `exec::shell::capture`.
    #[cfg(test)]
    pub(crate) fn head_tail_memory(stream: &'static str) -> Self {
        let mut capture = Self::head_tail(stream, 0);
        capture.spill_enabled = false;
        capture
    }

    pub(crate) fn push(&mut self, mut bytes: &[u8]) {
        use std::io::Write;
        self.total += bytes.len() as u64;
        if self.head.len() < self.head_cap {
            let take = bytes.len().min(self.head_cap - self.head.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        if bytes.is_empty() {
            return;
        }
        if self.spill_enabled {
            if self.spill.is_none() {
                self.spill = Some(open_spill(self.call, self.stream).and_then(
                    |(path, mut file)| {
                        file.write_all(&self.head)
                            .map_err(|e| e.to_string())
                            .map(|()| (path, file))
                    },
                ));
            }
            if let Some(Ok((_, file))) = &mut self.spill
                && let Err(e) = file.write_all(bytes)
            {
                self.spill = Some(Err(e.to_string()));
            }
        }
        if self.tail_cap > 0 {
            self.tail.extend(bytes);
            let excess = self.tail.len().saturating_sub(self.tail_cap);
            self.tail.drain(..excess);
        }
    }

    /// The capture so far (a timeout takes it while a reader may still hold
    /// the original).
    pub(crate) fn snapshot(&self) -> StreamOutput {
        StreamOutput {
            stream: self.stream,
            call: self.call,
            head: self.head.clone(),
            tail: self.tail.iter().copied().collect(),
            total: self.total,
            spill_enabled: self.spill_enabled,
            spill: match &self.spill {
                None => None,
                Some(Ok((path, _))) => Some(Ok(path.clone())),
                Some(Err(e)) => Some(Err(e.clone())),
            },
        }
    }
}

/// One pipe's output once the command is done (or timed out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamOutput {
    stream: &'static str,
    call: u64,
    head: Vec<u8>,
    tail: Vec<u8>,
    total: u64,
    spill_enabled: bool,
    spill: Option<Result<PathBuf, String>>,
}

impl StreamOutput {
    /// The captured head as text (all of it for a stream that fit).
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.head).into_owned()
    }

    /// [`Self::text`], with terminal control sequences resolved when `clean`.
    pub(crate) fn text_for(&self, clean: bool) -> String {
        cleaned(self.text(), clean)
    }

    /// Bytes the command wrote to this pipe (as far as it was read).
    pub(crate) fn total(&self) -> u64 {
        self.total
    }

    fn complete(&self) -> bool {
        self.total == self.head.len() as u64
    }

    /// The spill file holding this stream's full output, writing it now if
    /// the stream fit in memory and was not spilled while it ran.
    fn ensure_spill(&mut self) -> Result<PathBuf, String> {
        use std::io::Write;
        if !self.spill_enabled {
            return Err("memory-only capture does not retain full output".into());
        }
        if self.spill.is_none() && self.complete() {
            self.spill = Some(
                open_spill(self.call, self.stream).and_then(|(path, mut file)| {
                    file.write_all(&self.head)
                        .map_err(|e| e.to_string())
                        .map(|()| path)
                }),
            );
        }
        match &self.spill {
            Some(result) => result.clone(),
            None => Err("the output was not kept".into()),
        }
    }

    /// At most `budget` bytes of this stream: all of it when it fits,
    /// otherwise its head and its tail around a marker that names the spill
    /// file.
    fn cut(&mut self, budget: usize, clean: bool) -> String {
        if self.complete() {
            let text = self.text_for(clean);
            if text.len() <= budget {
                return text;
            }
        }
        let half = budget / 2;
        let head = utf8_prefix(&self.head, half);
        // The end of the stream, as far as memory holds it: the head itself
        // for a stream that fit, head and tail when nothing was dropped
        // between them, the tail alone otherwise.
        let dropped = self.total - self.head.len() as u64 - self.tail.len() as u64;
        let mut source = Vec::new();
        if dropped == 0 {
            source.extend_from_slice(&self.head);
        }
        source.extend_from_slice(&self.tail);
        // The tail must not overlap the head.
        let room = (self.total as usize).saturating_sub(head.len());
        let tail = utf8_suffix(&source, half.min(room));
        let omitted = self.total - head.len() as u64 - tail.len() as u64;
        let head = cleaned(String::from_utf8_lossy(head).into_owned(), clean);
        let tail = cleaned(String::from_utf8_lossy(tail).into_owned(), clean);
        let stream = self.stream;
        let saved = if self.spill_enabled {
            match self.ensure_spill() {
                Ok(path) => format!("the full {stream} is in {}", path.display()),
                Err(e) => format!("the full {stream} could not be saved: {e}"),
            }
        } else {
            format!("the full {stream} was not retained (memory-only capture)")
        };
        let stopped = if self.total >= SPILL_LIMIT {
            format!("; reading stopped after {} bytes", self.total)
        } else {
            String::new()
        };
        format!("{head}\n… [{omitted} bytes of {stream} omitted; {saved}{stopped}] …\n{tail}")
    }
}

/// The head_tail result: stdout, a framed stderr section and the exit code,
/// within [`OUTPUT_BUDGET`] plus the markers. A result that fits is exactly
/// the legacy text.
///
/// Returns the text and whether it is a cut of a larger output. `clean`
/// (`shell.clean_output`) resolves terminal control sequences first.
pub(crate) fn format_head_tail(
    stdout: &mut StreamOutput,
    stderr: &mut StreamOutput,
    exit_code: i32,
    clean: bool,
) -> (String, bool) {
    let has_stderr = !stderr.text_for(clean).trim().is_empty();
    let fits = stdout.complete()
        && stderr.complete()
        && legacy_len(stdout, stderr, has_stderr, exit_code, clean) <= OUTPUT_BUDGET;
    let (out_budget, err_budget) = if fits {
        (usize::MAX, usize::MAX)
    } else {
        // stderr keeps at least a fifth of the budget when stdout is large,
        // and whatever stdout leaves over otherwise.
        let err_len = if has_stderr {
            stderr.total() as usize
        } else {
            0
        };
        let err_budget = err_len
            .min((OUTPUT_BUDGET / 5).max(OUTPUT_BUDGET.saturating_sub(stdout.total() as usize)));
        (OUTPUT_BUDGET - err_budget, err_budget)
    };
    let mut text = stdout.cut(out_budget, clean);
    if has_stderr {
        text.push_str("\n--- stderr ---\n");
        text.push_str(&stderr.cut(err_budget, clean));
    }
    if exit_code != 0 {
        text.push_str(&format!("\n[exit code {exit_code}]"));
    }
    (text, !fits)
}

fn legacy_len(
    stdout: &StreamOutput,
    stderr: &StreamOutput,
    has_stderr: bool,
    code: i32,
    clean: bool,
) -> usize {
    let mut len = stdout.text_for(clean).len();
    if has_stderr {
        len += "\n--- stderr ---\n".len() + stderr.text_for(clean).len();
    }
    if code != 0 {
        len += format!("\n[exit code {code}]").len();
    }
    len
}

fn cleaned(text: String, clean: bool) -> String {
    if clean {
        clean_terminal_text(&text)
    } else {
        text
    }
}

/// `shell.clean_output`: what a terminal would show of `text`. Escape
/// sequences (CSI such as colours and cursor moves, OSC such as titles and
/// hyperlinks, DCS/SOS/PM/APC strings and two-byte escapes) are removed, and
/// within each line a carriage return moves back to its start so later text
/// overwrites earlier text (progress bars collapse to their last state); a
/// backspace moves back one character. A `\r` right before `\n` is a CRLF
/// line ending and is kept.
pub(crate) fn clean_terminal_text(text: &str) -> String {
    let stripped = strip_escapes(text);
    let mut out = String::with_capacity(stripped.len());
    for (i, line) in stripped.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (body, crlf) = match line.strip_suffix('\r') {
            Some(body) => (body, true),
            None => (line, false),
        };
        if !body.contains(['\r', '\u{8}']) {
            out.push_str(body);
        } else {
            let mut cells: Vec<char> = Vec::new();
            let mut cursor = 0usize;
            for c in body.chars() {
                match c {
                    '\r' => cursor = 0,
                    '\u{8}' => cursor = cursor.saturating_sub(1),
                    c => {
                        if cursor < cells.len() {
                            cells[cursor] = c;
                        } else {
                            cells.push(c);
                        }
                        cursor += 1;
                    }
                }
            }
            out.extend(cells);
        }
        if crlf {
            out.push('\r');
        }
    }
    out
}

fn strip_escapes(text: &str) -> String {
    if !text.contains('\u{1b}') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters and intermediates, then one final byte.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC, DCS, SOS, PM, APC: a string ended by BEL or ESC \.
            Some(']' | 'P' | 'X' | '^' | '_') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Two-byte escapes (charset selection takes one more byte).
            Some('(' | ')' | '*' | '+') => {
                chars.next();
            }
            Some(_) | None => {}
        }
    }
    out
}

/// The longest prefix of `bytes` of at most `n` bytes that does not end in
/// the middle of a UTF-8 sequence.
fn utf8_prefix(bytes: &[u8], n: usize) -> &[u8] {
    let mut end = n.min(bytes.len());
    while end > 0 && end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    &bytes[..end]
}

/// The longest suffix of `bytes` of at most `n` bytes that does not start in
/// the middle of a UTF-8 sequence.
fn utf8_suffix(bytes: &[u8], n: usize) -> &[u8] {
    let mut start = bytes.len().saturating_sub(n);
    while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
        start += 1;
    }
    &bytes[start..]
}

/// A number for each `execute` call of this process, naming its spill files.
pub(crate) fn next_call_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// `$TMPDIR/craftsman-<pid>-<random>/`, created once per run, private to the
/// user (0700 on Unix). Created with `create_dir`, never reusing an
/// existing path, so another user cannot plant it in a shared `/tmp`.
pub(crate) fn spill_dir() -> Result<PathBuf, String> {
    use std::sync::OnceLock;
    static DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = std::env::temp_dir();
        let mut last = String::new();
        for attempt in 0..16u32 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = base.join(format!(
                "craftsman-{}-{:x}",
                std::process::id(),
                (nanos as u64) ^ (u64::from(attempt) << 56)
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) => last = e.to_string(),
            }
        }
        Err(format!(
            "could not create a spill directory in {}: {last}",
            base.display()
        ))
    })
    .clone()
}

/// A new spill file (0600 on Unix, never an existing path).
fn open_spill(call: u64, stream: &str) -> Result<(PathBuf, std::fs::File), String> {
    let path = spill_dir()?.join(format!("execute-{call}.{stream}.txt"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path, file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(stream: &'static str, bytes: &[u8]) -> StreamOutput {
        let mut c = Capture::head_tail(stream, next_call_id());
        for chunk in bytes.chunks(8192) {
            c.push(chunk);
        }
        c.snapshot()
    }

    #[test]
    fn memory_only_capture_keeps_bounded_head_tail_without_any_spill() {
        let mut capture = Capture::head_tail_memory("stdout");
        capture.push(b"FIRST\n");
        for _ in 0..100 {
            capture.push(&vec![b'x'; 8192]);
        }
        capture.push(b"\nLAST\n");
        assert!(capture.spill.is_none());
        assert!(capture.head.len() <= OUTPUT_BUDGET);
        assert!(capture.tail.len() <= OUTPUT_BUDGET);
        let mut out = capture.snapshot();
        let mut err = Capture::head_tail_memory("stderr").snapshot();
        let (text, truncated) = format_head_tail(&mut out, &mut err, 3, false);
        assert!(truncated);
        assert!(text.starts_with("FIRST\n"));
        assert!(text.contains("LAST\n\n[exit code 3]"));
        assert!(text.contains("was not retained (memory-only capture)"));
        assert!(!text.contains(" is in "));
        assert!(out.spill.is_none());
        assert!(out.ensure_spill().is_err());
        assert!(out.spill.is_none());
    }

    #[test]
    fn combined_memory_only_streams_never_publish_late_spills_while_cutting() {
        // Each fits in memory, but together they exceed the formatted budget.
        // The legacy head_tail formatter writes late spill files in this case.
        let mut out = Capture::head_tail_memory("stdout");
        out.push(&vec![b'o'; 45_000]);
        let mut err = Capture::head_tail_memory("stderr");
        err.push(&vec![b'e'; 20_000]);
        let mut out = out.snapshot();
        let mut err = err.snapshot();
        let (text, truncated) = format_head_tail(&mut out, &mut err, 1, false);
        assert!(truncated);
        assert!(text.ends_with("[exit code 1]"));
        assert!(text.contains("full stdout was not retained"));
        assert!(text.contains("full stderr was not retained"));
        assert!(out.spill.is_none() && err.spill.is_none());
    }

    #[test]
    fn memory_only_timeout_and_partial_error_formatting_never_spill() {
        let mut out = Capture::head_tail_memory("stdout");
        out.push(&vec![b'x'; 200_000]);
        let err = Capture::head_tail_memory("stderr");
        let settings = crate::shell::ShellSettings {
            output_format: crate::shell::OutputFormat::HeadTail,
            ..Default::default()
        };
        let mut failure = crate::tools::ExecFailure::TimedOut {
            limit_secs: 1,
            partial: Box::new(crate::tools::ExecRaw {
                stdout: out.snapshot(),
                stderr: err.snapshot(),
                exit_code: -1,
                duration: std::time::Duration::from_secs(1),
            }),
        };
        let text = failure.text(&settings);
        assert!(text.contains("timed out after 1s"));
        assert!(text.contains("was not retained (memory-only capture)"));
        let crate::tools::ExecFailure::TimedOut { partial, .. } = failure else {
            panic!("timeout")
        };
        assert!(partial.stdout.spill.is_none());
        assert!(partial.stderr.spill.is_none());
    }

    #[test]
    fn a_result_that_fits_is_the_legacy_text() {
        let mut out = capture("stdout", b"hi\n");
        let mut err = capture("stderr", b"err\n");
        assert_eq!(
            format_head_tail(&mut out, &mut err, 3, false).0,
            crate::tools::format_exec("hi\n".into(), "err\n".into(), 3)
        );
        let mut out = capture("stdout", b"");
        let mut err = capture("stderr", b"  \n");
        assert_eq!(
            format_head_tail(&mut out, &mut err, 0, false).0,
            crate::tools::format_exec(String::new(), "  \n".into(), 0)
        );
    }

    #[test]
    fn a_large_stdout_keeps_head_tail_stderr_and_the_exit_code() {
        let body: Vec<u8> = (0..200_000u32)
            .map(|i| b"abcdefghij"[(i % 10) as usize])
            .collect();
        let mut data = b"FIRST-LINE\n".to_vec();
        data.extend_from_slice(&body);
        data.extend_from_slice(b"\nLAST-LINE\n");
        let mut out = capture("stdout", &data);
        let mut err = capture("stderr", b"the error\n");
        let (text, cut) = format_head_tail(&mut out, &mut err, 2, false);
        assert!(cut);
        assert!(text.starts_with("FIRST-LINE\n"), "head kept");
        assert!(text.contains("LAST-LINE\n\n--- stderr ---\nthe error\n"));
        assert!(text.ends_with("\n[exit code 2]"));
        assert!(text.len() < OUTPUT_BUDGET + 500, "len {}", text.len());
        let path = text
            .split("the full stdout is in ")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("the spill file is named");
        let spilled = std::fs::read(path).unwrap();
        assert_eq!(spilled, data, "the spill file holds the full stdout");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(file_mode, 0o600);
            let dir = std::path::Path::new(path).parent().unwrap();
            let dir_mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
    }

    #[test]
    fn a_large_stderr_is_cut_too_and_stdout_keeps_most_of_the_budget() {
        let out_data = vec![b'o'; 100_000];
        let err_data = vec![b'e'; 100_000];
        let mut out = capture("stdout", &out_data);
        let mut err = capture("stderr", &err_data);
        let (text, _) = format_head_tail(&mut out, &mut err, 1, false);
        assert!(text.contains("bytes of stdout omitted; the full stdout is in "));
        assert!(text.contains("bytes of stderr omitted; the full stderr is in "));
        assert!(text.ends_with("\n[exit code 1]"));
        assert!(text.len() < OUTPUT_BUDGET + 1_000, "len {}", text.len());
    }

    #[test]
    fn a_stream_that_fits_in_memory_is_spilled_when_it_is_cut() {
        // 45 kB of stdout plus 20 kB of stderr: both in memory, both cut.
        let mut out = capture("stdout", &vec![b'x'; 45_000]);
        let mut err = capture("stderr", &vec![b'y'; 20_000]);
        let (text, _) = format_head_tail(&mut out, &mut err, 0, false);
        assert!(text.contains("the full stdout is in "), "{}", &text[..200]);
        assert!(!text.contains("[exit code"));
    }

    #[test]
    fn cuts_never_split_a_utf8_character() {
        let data = "é".repeat(60_000);
        let mut out = capture("stdout", data.as_bytes());
        let mut err = capture("stderr", b"");
        let (text, _) = format_head_tail(&mut out, &mut err, 0, false);
        assert!(!text.contains('\u{FFFD}'));
    }

    #[test]
    fn clean_output_strips_escapes_and_resolves_carriage_returns() {
        assert_eq!(
            clean_terminal_text("\u{1b}[1;31mred\u{1b}[0m plain"),
            "red plain"
        );
        assert_eq!(
            clean_terminal_text("\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{1b}\\ done"),
            "link done"
        );
        assert_eq!(clean_terminal_text("\u{1b}(Bok"), "ok");
        assert_eq!(
            clean_terminal_text("progress 10%\rprogress 55%\rprogress 100%\nnext"),
            "progress 100%\nnext"
        );
        assert_eq!(clean_terminal_text("abcdef\rxy"), "xycdef");
        assert_eq!(clean_terminal_text("ab\u{8}c"), "ac");
        assert_eq!(
            clean_terminal_text("windows\r\nlines\r\n"),
            "windows\r\nlines\r\n"
        );
        assert_eq!(clean_terminal_text("plain é 中"), "plain é 中");
    }

    #[test]
    fn clean_output_applies_inside_the_head_tail_format() {
        let mut out = capture("stdout", b"\x1b[32mok\x1b[0m\n");
        let mut err = capture("stderr", b"50%\r100%\n");
        let (text, cut) = format_head_tail(&mut out, &mut err, 0, true);
        assert_eq!(text, "ok\n\n--- stderr ---\n100%\n");
        assert!(!cut);
    }

    #[test]
    fn legacy_capture_keeps_only_todays_cap() {
        let mut c = Capture::legacy("stdout");
        c.push(&vec![b'z'; 150_000]);
        let s = c.snapshot();
        assert_eq!(s.text().len(), LEGACY_READ_LIMIT as usize);
        assert!(s.spill.is_none());
    }
}
