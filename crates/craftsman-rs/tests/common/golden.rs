//! Transcripts, normalization and golden-file comparison.
//!
//! A scenario records every frame it sends and receives as a [`Transcript`].
//! [`check`] normalizes the volatile parts (ids, ports, temp paths, timings)
//! into stable placeholders, renders the result as pretty-printed JSON with
//! sorted keys, and compares it with `tests/golden/<scenario>.json`.
//!
//! `UPDATE_GOLDEN=1` rewrites the file instead of comparing; any other value
//! of `UPDATE_GOLDEN`, and `UPDATE_GOLDEN` on CI, is an error ([`update_mode`]).
//! A mismatch fails with the frame sequence, in which every frame that
//! changed, was added or was removed is marked and shown as a diff of its JSON.

use serde_json::{Value, json};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The ordered record of one scenario. Clones share the same record, so a
/// WebSocket client and an HTTP client can write into one transcript.
#[derive(Clone, Default)]
pub struct Transcript(Arc<Record>);

#[derive(Default)]
struct Record {
    steps: Mutex<Vec<Value>>,
    /// The golden this recording is meant to reproduce, once the scenario has
    /// said which; see [`Transcript::expect_golden`].
    golden: Mutex<Option<(&'static str, Normalizer)>>,
}

impl Transcript {
    /// Name the golden this recording is meant to reproduce, and how its
    /// volatile values are normalized. A scenario that fails part-way (a
    /// timeout, a frame the driver cannot use) then shows how far the
    /// recording got against the golden ([`Transcript::progress_report`]),
    /// instead of only the labels of the frames.
    pub fn expect_golden(&self, name: &'static str, normalizer: Normalizer) {
        *self.0.golden.lock().unwrap() = Some((name, normalizer));
    }

    /// Where the recording so far departs from its golden: the frames that
    /// differ, with what the golden expected, and what the golden had next.
    /// `None` when no golden was named.
    pub fn progress_report(&self) -> Option<String> {
        let (name, normalizer) = self.0.golden.lock().unwrap().clone()?;
        let path = golden_path(name);
        let recorded = match std::fs::read_to_string(&path) {
            Ok(text) => steps_of(&text.replace("\r\n", "\n")),
            Err(error) => {
                return Some(format!(
                    "golden `{name}` is unreadable ({}): {error}\n",
                    path.display()
                ));
            }
        };
        let Some(recorded) = recorded else {
            return Some(format!(
                "golden `{name}` ({}) has no readable steps\n",
                path.display()
            ));
        };
        let now = normalizer.apply_partial(self.steps());
        Some(format!(
            "golden `{name}` ({}); `-` is the golden, `+` is this run\n{}",
            path.display(),
            progress_report(&recorded, &now)
        ))
    }

    /// A frame the test sent to the program under test.
    pub fn send(&self, frame: &Value) {
        self.push(json!({ "send": frame }));
    }

    /// A frame the program under test sent back.
    pub fn recv(&self, frame: &Value) {
        self.push(json!({ "recv": frame }));
    }

    /// One HTTP exchange (`request`, `status`, `body`, ...).
    pub fn http(&self, exchange: Value) {
        self.push(json!({ "http": exchange }));
    }

    /// A position in the record, to hand back to [`Transcript::rewind_to`].
    pub fn mark(&self) -> usize {
        self.0.steps.lock().unwrap().len()
    }

    /// Forget every step recorded after `mark`: for an attempt that is being
    /// abandoned and retried, so that it leaves no trace in the golden.
    pub fn rewind_to(&self, mark: usize) {
        self.0.steps.lock().unwrap().truncate(mark);
    }

    /// Panics unless some recorded step satisfies `matches`.
    ///
    /// The goldens are recorded by the same code that replays them, so a
    /// recording that silently lost frames would match its own golden for
    /// ever. Each scenario uses this to insist on the frames it exists to pin.
    pub fn require(&self, what: &str, matches: impl Fn(&Value) -> bool) {
        assert!(
            self.steps().iter().any(matches),
            "the recording has no step for {what}; frames were lost\n{}",
            self.outline()
        );
    }

    fn push(&self, step: Value) {
        self.0.steps.lock().unwrap().push(step);
    }

    pub fn steps(&self) -> Vec<Value> {
        self.0.steps.lock().unwrap().clone()
    }

    /// One line per step, for failure messages.
    pub fn outline(&self) -> String {
        let steps = self.steps();
        if steps.is_empty() {
            return "    (nothing recorded yet)".to_string();
        }
        outline_of(&steps)
            .iter()
            .enumerate()
            .map(|(index, label)| format!("    {:>3}. {label}", index + 1))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn outline_of(steps: &[Value]) -> Vec<String> {
    steps.iter().map(label).collect()
}

/// A short, human-readable name for a step, used in sequence diffs.
fn label(step: &Value) -> String {
    if let Some(frame) = step.get("send") {
        return format!("send  {}", frame_label(frame));
    }
    if let Some(frame) = step.get("recv") {
        return format!("recv  {}", frame_label(frame));
    }
    if let Some(exchange) = step.get("http") {
        return format!(
            "http  {} -> {}",
            exchange["request"].as_str().unwrap_or("?"),
            exchange["status"]
        );
    }
    "?".to_string()
}

fn frame_label(frame: &Value) -> String {
    let id = frame
        .get("id")
        .map(|id| format!(" id={id}"))
        .unwrap_or_default();
    if let Some(method) = frame["method"].as_str() {
        let kind = frame["params"]["update"]["sessionUpdate"]
            .as_str()
            .map(|kind| format!(" ({kind})"))
            .unwrap_or_default();
        return format!("{method}{kind}{id}");
    }
    if frame.get("result").is_some() {
        let stop = frame["result"]["stopReason"]
            .as_str()
            .map(|reason| format!(" stopReason={reason}"))
            .unwrap_or_default();
        return format!("result{id}{stop}");
    }
    if frame.get("error").is_some() {
        return format!("error{id}");
    }
    match frame["type"].as_str() {
        Some(kind) => kind.to_string(),
        None => "frame".to_string(),
    }
}

// ── Normalization ───────────────────────────────────────────────────────────

/// The JSON type a masked value must have; anything else is left visible so
/// that a type change shows up in the diff instead of being hidden.
#[derive(Clone, Copy)]
pub enum Kind {
    UInt,
    Float,
}

impl Kind {
    fn matches(self, value: &Value) -> bool {
        match self {
            Kind::UInt => value.is_u64(),
            Kind::Float => value.is_f64(),
        }
    }
}

#[derive(Clone)]
enum Action {
    Mask {
        kind: Kind,
        placeholder: &'static str,
    },
    MaskExact {
        expected: String,
        placeholder: &'static str,
    },
    Shape,
}

#[derive(Clone)]
struct Rule {
    pointer: &'static str,
    action: Action,
    hits: usize,
}

/// Turns raw transcripts into stable ones.
///
/// Strings are scrubbed everywhere: registered directories become
/// placeholders, loopback ports become `<PORT>` and conversation ids become
/// `<thread-id-N>` (numbered by first appearance, so the identity of an id
/// across frames survives). Individual values are handled by pointer rules,
/// which address a step as `/recv/...`, `/send/...` or `/http/...`; `*`
/// matches every key or index at that level. Every rule must match at least
/// once, so a rule that stops applying is a failure, not a silent no-op.
#[derive(Clone)]
pub struct Normalizer {
    directories: Vec<(String, &'static str)>,
    forbidden: Vec<String>,
    rules: Vec<Rule>,
    thread_ids: Vec<String>,
}

impl Normalizer {
    pub fn new() -> Normalizer {
        Normalizer {
            directories: Vec::new(),
            forbidden: Vec::new(),
            rules: Vec::new(),
            thread_ids: Vec::new(),
        }
    }

    /// A directory with a volatile absolute path (a temp dir). Its path as
    /// given and as canonicalized (`/var` is `/private/var` on macOS) are both
    /// replaced.
    pub fn directory(mut self, path: &Path, placeholder: &'static str) -> Normalizer {
        let mut spellings = vec![path.to_string_lossy().into_owned()];
        if let Ok(canonical) = path.canonicalize() {
            spellings.push(canonical.to_string_lossy().into_owned());
        }
        spellings.sort();
        spellings.dedup();
        for spelling in spellings {
            self.forbidden.push(spelling.clone());
            self.directories.push((spelling, placeholder));
        }
        // Longest first, so a directory nested in another is replaced whole.
        self.directories
            .sort_by_key(|(spelling, _)| std::cmp::Reverse(spelling.len()));
        self
    }

    /// Text that must not survive normalization anywhere in the transcript.
    pub fn forbid(mut self, text: impl Into<String>) -> Normalizer {
        self.forbidden.push(text.into());
        self
    }

    /// Forbid the paths of the machine that records the golden: the checkout,
    /// the directory of the binary under test, the temp directory and the
    /// user's home. Goldens are re-recorded by people on their own machines,
    /// so a frame that starts to echo one of these must fail the recording
    /// instead of putting a developer's path into the repository.
    pub fn forbid_machine_paths(mut self) -> Normalizer {
        self.forbidden.extend(machine_paths());
        self
    }

    /// Replace the value at `pointer` with `placeholder` if it has type `kind`.
    pub fn mask(mut self, pointer: &'static str, kind: Kind, placeholder: &'static str) -> Self {
        self.rules.push(Rule {
            pointer,
            action: Action::Mask { kind, placeholder },
            hits: 0,
        });
        self
    }

    /// Replace the string at `pointer` with `placeholder` if it equals
    /// `expected`. The test asserts the equality that the golden cannot see.
    pub fn mask_exact(
        mut self,
        pointer: &'static str,
        expected: &str,
        placeholder: &'static str,
    ) -> Self {
        self.rules.push(Rule {
            pointer,
            action: Action::MaskExact {
                expected: expected.to_string(),
                placeholder,
            },
            hits: 0,
        });
        self
    }

    /// Keep the key structure at `pointer` but replace every scalar with its
    /// type name and every array with `"<array>"`. For values that depend on
    /// the build or the machine but whose schema is part of the contract.
    pub fn shape(mut self, pointer: &'static str) -> Normalizer {
        self.rules.push(Rule {
            pointer,
            action: Action::Shape,
            hits: 0,
        });
        self
    }

    /// Normalize `steps`. Panics when a rule never matched or when something
    /// machine-specific is still visible afterwards.
    pub fn apply(mut self, steps: Vec<Value>) -> Vec<Value> {
        let steps = self.rewrite(steps);
        for rule in &self.rules {
            assert!(
                rule.hits > 0,
                "normalization rule {} matched nothing; the frame it targets changed shape \
                 or its value no longer has the expected form",
                rule.pointer
            );
        }
        for step in &steps {
            self.assert_clean(step);
        }
        steps
    }

    /// Normalize the steps of a recording that is not finished, to compare it
    /// with a golden in a failure message. A rule whose frame has not arrived
    /// yet is not an error, and nothing is asserted: a message that explains a
    /// failure must not fail itself.
    pub fn apply_partial(mut self, steps: Vec<Value>) -> Vec<Value> {
        self.rewrite(steps)
    }

    fn rewrite(&mut self, mut steps: Vec<Value>) -> Vec<Value> {
        for step in &mut steps {
            for rule in &mut self.rules {
                let segments: Vec<&str> = rule.pointer.split('/').skip(1).collect();
                let action = &rule.action;
                rule.hits += walk(step, &segments, &mut |value| apply_action(action, value));
            }
        }
        for step in &mut steps {
            self.scrub_strings(step);
        }
        steps
    }

    fn scrub_strings(&mut self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.scrub(text),
            Value::Array(items) => items.iter_mut().for_each(|item| self.scrub_strings(item)),
            Value::Object(map) => map
                .iter_mut()
                .for_each(|(_, item)| self.scrub_strings(item)),
            _ => {}
        }
    }

    fn scrub(&mut self, text: &str) -> String {
        let mut text = text.to_string();
        for (directory, placeholder) in &self.directories {
            text = text.replace(directory.as_str(), placeholder);
        }
        let text = scrub_ports(&text);
        self.scrub_thread_ids(&text)
    }

    fn scrub_thread_ids(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut word_start: Option<usize> = None;
        // The trailing sentinel flushes a word that ends the string.
        let sentinel = (text.len(), ' ');
        for (index, ch) in text.char_indices().chain(std::iter::once(sentinel)) {
            let in_word = ch.is_ascii_alphanumeric() || ch == '-';
            if in_word {
                word_start.get_or_insert(index);
                continue;
            }
            if let Some(start) = word_start.take() {
                let word = &text[start..index];
                if is_thread_id(word) {
                    out.push_str(&self.thread_placeholder(word));
                } else {
                    out.push_str(word);
                }
            }
            if index < text.len() {
                out.push(ch);
            }
        }
        out
    }

    fn thread_placeholder(&mut self, id: &str) -> String {
        let position = match self.thread_ids.iter().position(|seen| seen == id) {
            Some(position) => position,
            None => {
                self.thread_ids.push(id.to_string());
                self.thread_ids.len() - 1
            }
        };
        format!("<thread-id-{}>", position + 1)
    }

    fn assert_clean(&self, value: &Value) {
        match value {
            Value::String(text) => self.assert_clean_text(text),
            Value::Array(items) => items.iter().for_each(|item| self.assert_clean(item)),
            Value::Object(map) => {
                for (key, item) in map {
                    self.assert_clean_text(key);
                    self.assert_clean(item);
                }
            }
            _ => {}
        }
    }

    fn assert_clean_text(&self, text: &str) {
        for needle in &self.forbidden {
            assert!(
                !text.contains(needle.as_str()),
                "machine-specific text {needle:?} survived normalization in {text:?}"
            );
        }
        for host in ["127.0.0.1:", "localhost:"] {
            let mut rest = text;
            while let Some(at) = rest.find(host) {
                rest = &rest[at + host.len()..];
                assert!(
                    !rest.starts_with(|ch: char| ch.is_ascii_digit()),
                    "a port survived normalization in {text:?}"
                );
            }
        }
    }
}

/// Apply `f` to every value `segments` addresses below `value`; returns how
/// many it changed.
fn walk(value: &mut Value, segments: &[&str], f: &mut dyn FnMut(&mut Value) -> bool) -> usize {
    let Some((segment, rest)) = segments.split_first() else {
        return usize::from(f(value));
    };
    match value {
        Value::Object(map) if *segment == "*" => {
            map.iter_mut().map(|(_, child)| walk(child, rest, f)).sum()
        }
        Value::Object(map) => map
            .get_mut(*segment)
            .map_or(0, |child| walk(child, rest, f)),
        Value::Array(items) if *segment == "*" => {
            items.iter_mut().map(|child| walk(child, rest, f)).sum()
        }
        Value::Array(items) => segment
            .parse::<usize>()
            .ok()
            .and_then(|index| items.get_mut(index))
            .map_or(0, |child| walk(child, rest, f)),
        _ => 0,
    }
}

fn apply_action(action: &Action, value: &mut Value) -> bool {
    match action {
        Action::Mask { kind, placeholder } if kind.matches(value) => {
            *value = Value::String((*placeholder).to_string());
            true
        }
        Action::MaskExact {
            expected,
            placeholder,
        } if value.as_str() == Some(expected.as_str()) => {
            *value = Value::String((*placeholder).to_string());
            true
        }
        Action::Shape => {
            *value = shape_of(value);
            true
        }
        _ => false,
    }
}

fn shape_of(value: &Value) -> Value {
    let name = match value {
        Value::Null => "<null>",
        Value::Bool(_) => "<bool>",
        Value::Number(number) if number.is_u64() => "<uint>",
        Value::Number(number) if number.is_i64() => "<int>",
        Value::Number(_) => "<float>",
        Value::String(_) => "<string>",
        Value::Array(_) => "<array>",
        Value::Object(map) => {
            return Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), shape_of(item)))
                    .collect(),
            );
        }
    };
    Value::String(name.to_string())
}

fn scrub_ports(text: &str) -> String {
    let mut out = text.to_string();
    for host in ["127.0.0.1:", "localhost:", "[::1]:", "0.0.0.0:"] {
        let mut scrubbed = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(at) = rest.find(host) {
            let (head, tail) = rest.split_at(at + host.len());
            scrubbed.push_str(head);
            let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 {
                scrubbed.push_str("<PORT>");
            }
            rest = &tail[digits..];
        }
        scrubbed.push_str(rest);
        out = scrubbed;
    }
    out
}

/// `LibsqlCheckpointer::new_thread_id` mints `<unix seconds>-<pid in hex>-<4 hex digits>`.
fn is_thread_id(word: &str) -> bool {
    let mut parts = word.split('-');
    let (Some(seconds), Some(pid), Some(counter), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    seconds.len() >= 9
        && seconds.bytes().all(|byte| byte.is_ascii_digit())
        && !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_hexdigit())
        && counter.len() == 4
        && counter.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Where this test run lives, as text: each path as given and canonicalized.
/// A path of a single component (`/`, `C:\`) is left out, since forbidding it
/// would forbid almost every string.
fn machine_paths() -> Vec<String> {
    let mut paths = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        std::env::temp_dir(),
    ];
    paths.extend(
        Path::new(env!("CARGO_BIN_EXE_craftsman"))
            .parent()
            .map(Path::to_path_buf),
    );
    paths.extend(dirs::home_dir());
    let mut spellings = Vec::new();
    for path in paths {
        spellings.extend(path.canonicalize().ok());
        spellings.push(path);
    }
    let mut texts: Vec<String> = spellings
        .into_iter()
        .filter(|path| path.components().count() >= 2)
        // `components` drops a trailing separator (`$TMPDIR` ends in one).
        .map(|path| path.components().collect::<PathBuf>())
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    texts.sort();
    texts.dedup();
    texts
}

// ── Golden files ────────────────────────────────────────────────────────────

pub struct Case {
    /// The golden file stem under `tests/golden/`.
    pub name: &'static str,
    /// What the scenario covers, stored at the top of the golden file.
    pub description: &'static str,
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.json"))
}

const UPDATE_ON_CI: &str = "UPDATE_GOLDEN is set while CI is set. Goldens are never rewritten \
    on CI, where a run must compare; unset UPDATE_GOLDEN (on a developer machine that has CI \
    set for another reason, unset CI for the command).";

/// Whether this run is to rewrite the goldens, from the values of the
/// `UPDATE_GOLDEN` and `CI` environment variables (`None` when unset).
///
/// A golden is recorded by the code that replays it, so a rewrite passes
/// whatever the program did; it has to be a deliberate act of a person. Only
/// `UPDATE_GOLDEN=1` asks for it, and only where `CI` is unset. Any other
/// setting is an error rather than a quiet comparison or a quiet rewrite: a
/// developer who typed `UPDATE_GOLDEN=true` would otherwise wait for a mismatch
/// to find out nothing was written, and a CI environment that gained the
/// variable would turn every protocol change green.
fn update_mode(update: Option<&OsStr>, ci: Option<&OsStr>) -> Result<bool, String> {
    let Some(update) = update else {
        return Ok(false);
    };
    if ci.is_some() {
        return Err(UPDATE_ON_CI.to_string());
    }
    if update != OsStr::new("1") {
        return Err(format!(
            "UPDATE_GOLDEN={update:?} is not understood: only the value 1 rewrites the goldens. \
             Unset UPDATE_GOLDEN to compare, or use UPDATE_GOLDEN=1 to record."
        ));
    }
    Ok(true)
}

fn update_requested() -> bool {
    let update = std::env::var_os("UPDATE_GOLDEN");
    let ci = std::env::var_os("CI");
    update_mode(update.as_deref(), ci.as_deref()).unwrap_or_else(|reason| panic!("{reason}"))
}

/// Compare the normalized transcript with its golden file (or rewrite it when
/// `UPDATE_GOLDEN` is set).
pub fn check(case: Case, transcript: &Transcript, normalizer: Normalizer) {
    let steps = normalizer.forbid_machine_paths().apply(transcript.steps());
    let document = json!({
        "scenario": case.name,
        "description": case.description,
        "steps": steps,
    });
    let actual = render(&document);
    let path = golden_path(case.name);

    if update_requested() {
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current.replace("\r\n", "\n") != actual {
            std::fs::create_dir_all(path.parent().expect("golden dir")).expect("create golden dir");
            std::fs::write(&path, &actual).expect("write golden file");
            eprintln!("golden updated: {}", path.display());
        }
        return;
    }

    let expected = match std::fs::read_to_string(&path) {
        Ok(text) => text.replace("\r\n", "\n"),
        Err(error) => panic!(
            "golden file {} is unreadable ({error}); record it with {UPDATE_HINT}",
            path.display()
        ),
    };
    if expected != actual {
        panic!("{}", mismatch_report(case.name, &path, &expected, &actual));
    }
}

const UPDATE_HINT: &str = "UPDATE_GOLDEN=1 cargo test --locked --no-default-features -p craftsman-rs --test golden_frames";

/// Pretty JSON with sorted keys, so the file does not depend on whether
/// `serde_json` was built with `preserve_order` in this feature set.
fn render(document: &Value) -> String {
    let mut text = serde_json::to_string_pretty(&canonical(document)).expect("render golden");
    text.push('\n');
    text
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, item)| (key.clone(), canonical(item)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The failure message of a mismatch: which frames differ, and how.
fn mismatch_report(name: &str, path: &Path, expected: &str, actual: &str) -> String {
    let mut report = format!(
        "golden frames changed for `{name}`\n  golden file: {}\n  \
         `-` is the recorded golden, `+` is what this run produced\n",
        path.display()
    );
    match (steps_of(expected), steps_of(actual)) {
        (Some(recorded), Some(now)) if recorded != now => {
            report.push_str(&sequence_report(&recorded, &now));
        }
        // The frames agree and only the text around them differs, or the
        // golden is not a readable recording: show the files themselves.
        _ => report.push_str(&format!(
            "\nfile contents:\n{}\n",
            render_diff(&diff(&text_lines(expected), &text_lines(actual)), 3)
        )),
    }
    report.push_str(&format!(
        "\nThe frozen protocols (protocol v4 and ACP) must not change silently. If this change is \
         intended, regenerate with\n  {UPDATE_HINT}\nand have the desktop and workspace owners \
         sign off on the new goldens in the PR.\n"
    ));
    report
}

fn steps_of(text: &str) -> Option<Vec<Value>> {
    let document: Value = serde_json::from_str(text).ok()?;
    document["steps"].as_array().cloned()
}

fn text_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn pretty_lines(step: &Value) -> Vec<String> {
    let text = serde_json::to_string_pretty(&canonical(step)).expect("render step");
    text_lines(&text)
}

// ── Frame sequence diff ─────────────────────────────────────────────────────

/// One place in the comparison of the golden with this run.
enum Row<'a> {
    Same {
        now: usize,
        step: &'a Value,
    },
    /// The same kind of frame at the same place, with other contents.
    Changed {
        now: usize,
        recorded: &'a Value,
        step: &'a Value,
    },
    Added {
        now: usize,
        step: &'a Value,
    },
    Removed {
        recorded: usize,
        step: &'a Value,
    },
}

/// A frame sequence diff: every step of both recordings in order, with the
/// steps that differ marked and unfolded.
fn sequence_report(recorded: &[Value], now: &[Value]) -> String {
    let rows = align(recorded, now);
    let count = |wanted: fn(&Row) -> bool| rows.iter().filter(|row| wanted(row)).count();
    let mut out = format!(
        "\nframe sequence ({} recorded, {} now; {} changed, {} added, {} removed):\n",
        recorded.len(),
        now.len(),
        count(|row| matches!(row, Row::Changed { .. })),
        count(|row| matches!(row, Row::Added { .. })),
        count(|row| matches!(row, Row::Removed { .. })),
    );
    let changes: Vec<bool> = rows
        .iter()
        .map(|row| !matches!(row, Row::Same { .. }))
        .collect();
    let mut skipping = false;
    for (row, shown) in rows.iter().zip(with_context(&changes, 2)) {
        if !shown {
            if !skipping {
                out.push_str("        ...\n");
            }
            skipping = true;
            continue;
        }
        skipping = false;
        write_row(&mut out, row);
    }
    out
}

/// How a recording that is not finished compares with its golden: the steps
/// that differ so far, and what the golden had next. After the last step that
/// matches, the golden's remaining steps are not removals, the run just has not
/// got there (or got something else instead, which is shown as added); they
/// are summarised by the one that was due.
fn progress_report(recorded: &[Value], now: &[Value]) -> String {
    let rows = align(recorded, now);
    let tail = rows
        .iter()
        .rposition(|row| matches!(row, Row::Same { .. } | Row::Changed { .. }))
        .map_or(0, |last| last + 1);
    let (mut reached, mut unreached) = (Vec::new(), Vec::new());
    for (index, row) in rows.iter().enumerate() {
        if index >= tail && matches!(row, Row::Removed { .. }) {
            unreached.push(row);
        } else {
            reached.push(row);
        }
    }
    let count = |wanted: fn(&Row) -> bool| reached.iter().filter(|row| wanted(row)).count();
    let mut out = format!(
        "frame sequence so far ({} of the golden's {} steps reached, {} recorded; {} changed, \
         {} added, {} removed):\n",
        recorded.len() - unreached.len(),
        recorded.len(),
        now.len(),
        count(|row| matches!(row, Row::Changed { .. })),
        count(|row| matches!(row, Row::Added { .. })),
        count(|row| matches!(row, Row::Removed { .. })),
    );
    let changes: Vec<bool> = reached
        .iter()
        .map(|row| !matches!(row, Row::Same { .. }))
        .collect();
    if !changes.contains(&true) {
        out.push_str("        every step so far matches the golden\n");
    }
    let mut skipping = false;
    for (row, shown) in reached.iter().zip(with_context(&changes, 2)) {
        if !shown {
            if !skipping {
                out.push_str("        ...\n");
            }
            skipping = true;
            continue;
        }
        skipping = false;
        write_row(&mut out, row);
    }
    match unreached.first() {
        Some(Row::Removed { recorded, step }) => out.push_str(&format!(
            "        the golden goes on with step {}: {} ({} steps not reached)\n",
            recorded + 1,
            label(step),
            unreached.len()
        )),
        _ => out.push_str("        the golden has no further steps\n"),
    }
    out
}

/// Match the steps of the golden with the steps of this run.
fn align<'a>(recorded: &'a [Value], now: &'a [Value]) -> Vec<Row<'a>> {
    let compact = |steps: &[Value]| -> Vec<String> {
        steps
            .iter()
            .map(|step| canonical(step).to_string())
            .collect()
    };
    let (mut next_recorded, mut next_now) = (0, 0);
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    let mut rows = Vec::new();
    for line in diff(&compact(recorded), &compact(now)) {
        match line {
            Line::Removed(_) => {
                removed.push(next_recorded);
                next_recorded += 1;
            }
            Line::Added(_) => {
                added.push(next_now);
                next_now += 1;
            }
            Line::Same(_) => {
                settle(&mut rows, recorded, now, &mut removed, &mut added);
                rows.push(Row::Same {
                    now: next_now,
                    step: &now[next_now],
                });
                next_recorded += 1;
                next_now += 1;
            }
        }
    }
    settle(&mut rows, recorded, now, &mut removed, &mut added);
    rows
}

/// Turn a run of removed and added steps into rows. A removed step and an
/// added step of the same kind (the same label, in the same order) are one
/// step whose contents changed; the others are plain removals and additions.
fn settle<'a>(
    rows: &mut Vec<Row<'a>>,
    recorded: &'a [Value],
    now: &'a [Value],
    removed: &mut Vec<usize>,
    added: &mut Vec<usize>,
) {
    let mut next = 0;
    for &old in removed.iter() {
        let wanted = label(&recorded[old]);
        let found = added[next..]
            .iter()
            .position(|&new| label(&now[new]) == wanted);
        let Some(offset) = found else {
            rows.push(Row::Removed {
                recorded: old,
                step: &recorded[old],
            });
            continue;
        };
        for &new in &added[next..next + offset] {
            rows.push(Row::Added {
                now: new,
                step: &now[new],
            });
        }
        let new = added[next + offset];
        rows.push(Row::Changed {
            now: new,
            recorded: &recorded[old],
            step: &now[new],
        });
        next += offset + 1;
    }
    for &new in &added[next..] {
        rows.push(Row::Added {
            now: new,
            step: &now[new],
        });
    }
    removed.clear();
    added.clear();
}

/// The longest block of JSON shown for a step that was added or removed.
const MAX_STEP_LINES: usize = 40;

fn write_row(out: &mut String, row: &Row) {
    match row {
        Row::Same { now, step } => {
            out.push_str(&format!("      {:>3}  {}\n", now + 1, label(step)));
        }
        Row::Changed {
            now,
            recorded,
            step,
        } => {
            out.push_str(&format!("   ~  {:>3}  {}\n", now + 1, label(step)));
            let lines = diff(&pretty_lines(recorded), &pretty_lines(step));
            push_block(out, &render_diff(&lines, 2));
        }
        Row::Added { now, step } => {
            out.push_str(&format!("   +  {:>3}  {}\n", now + 1, label(step)));
            push_block(out, &step_block("+", step));
        }
        Row::Removed { recorded, step } => {
            out.push_str(&format!(
                "   -       {}  (step {} of the golden)\n",
                label(step),
                recorded + 1
            ));
            push_block(out, &step_block("-", step));
        }
    }
}

/// `text` below a row, indented to sit under its label.
fn push_block(out: &mut String, text: &str) {
    for line in text.lines() {
        out.push_str(&format!("          {line}\n"));
    }
}

/// A whole step as signed JSON lines, cut off after [`MAX_STEP_LINES`].
fn step_block(sign: &str, step: &Value) -> String {
    let lines = pretty_lines(step);
    let mut shown: Vec<String> = lines
        .iter()
        .take(MAX_STEP_LINES)
        .map(|line| format!("{sign} {line}"))
        .collect();
    if lines.len() > MAX_STEP_LINES {
        shown.push(format!("  ... {} more lines", lines.len() - MAX_STEP_LINES));
    }
    shown.join("\n")
}

// ── Line diff ───────────────────────────────────────────────────────────────

enum Line {
    Same(String),
    Removed(String),
    Added(String),
}

/// Longest-common-subsequence diff of two line lists.
fn diff(expected: &[String], actual: &[String]) -> Vec<Line> {
    let (n, m) = (expected.len(), actual.len());
    if n.saturating_mul(m) > 16_000_000 {
        // Too large for the quadratic table: show everything as replaced.
        return expected
            .iter()
            .cloned()
            .map(Line::Removed)
            .chain(actual.iter().cloned().map(Line::Added))
            .collect();
    }
    let mut common = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[i][j] = if expected[i] == actual[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut lines = Vec::new();
    while i < n && j < m {
        if expected[i] == actual[j] {
            lines.push(Line::Same(expected[i].clone()));
            i += 1;
            j += 1;
        } else if common[i + 1][j] >= common[i][j + 1] {
            lines.push(Line::Removed(expected[i].clone()));
            i += 1;
        } else {
            lines.push(Line::Added(actual[j].clone()));
            j += 1;
        }
    }
    lines.extend(expected[i..].iter().cloned().map(Line::Removed));
    lines.extend(actual[j..].iter().cloned().map(Line::Added));
    lines
}

/// Which of the items stay visible when only the changed ones and `context`
/// neighbours on each side are shown.
fn with_context(changed: &[bool], context: usize) -> Vec<bool> {
    let mut keep = vec![false; changed.len()];
    for (index, _) in changed.iter().enumerate().filter(|(_, changed)| **changed) {
        let from = index.saturating_sub(context);
        let to = (index + context).min(changed.len() - 1);
        keep[from..=to].fill(true);
    }
    keep
}

/// Changed lines with `context` lines around them; unchanged runs collapse to `...`.
fn render_diff(lines: &[Line], context: usize) -> String {
    let changes: Vec<bool> = lines
        .iter()
        .map(|line| !matches!(line, Line::Same(_)))
        .collect();
    if !changes.contains(&true) {
        return "  (no differences)".to_string();
    }
    let mut out = Vec::new();
    let mut skipping = false;
    for (line, shown) in lines.iter().zip(with_context(&changes, context)) {
        if !shown {
            if !skipping {
                out.push("  ...".to_string());
            }
            skipping = true;
            continue;
        }
        skipping = false;
        out.push(match line {
            Line::Same(text) => format!("  {text}"),
            Line::Removed(text) => format!("- {text}"),
            Line::Added(text) => format!("+ {text}"),
        });
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(normalizer: Normalizer, steps: Vec<Value>) -> Vec<Value> {
        normalizer.apply(steps)
    }

    #[test]
    fn ports_ids_and_directories_become_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt").to_string_lossy().into_owned();
        let steps = vec![json!({"recv": {
            "url": "http://127.0.0.1:43211/chat/completions",
            "path": path,
            "first": "1760000000-1a2b-00ff",
            "again": "id 1760000000-1a2b-00ff!",
            "other": "1760000000-1a2b-0100",
        }})];
        let out = normalized(Normalizer::new().directory(dir.path(), "<DIR>"), steps);
        assert_eq!(
            out[0]["recv"],
            json!({
                "url": "http://127.0.0.1:<PORT>/chat/completions",
                "path": format!("<DIR>{}file.txt", std::path::MAIN_SEPARATOR),
                "first": "<thread-id-1>",
                "again": "id <thread-id-1>!",
                "other": "<thread-id-2>",
            })
        );
    }

    #[test]
    fn words_that_only_resemble_ids_are_left_alone() {
        assert!(is_thread_id("1760000000-1a2b-00ff"));
        assert!(!is_thread_id("2026-10-16"));
        assert!(!is_thread_id("1760000000-1a2b"));
        assert!(!is_thread_id("1760000000-zz-00ff"));
        assert!(!is_thread_id("1760000000-1a2b-00ff-0"));
    }

    #[test]
    fn masks_only_replace_values_of_the_expected_type() {
        let steps = vec![
            json!({"http": {"body": {"uptime_s": 0.25, "used": 7, "name": "x"}}}),
            json!({"http": {"body": {"uptime_s": "not a number"}}}),
        ];
        let out = normalized(
            Normalizer::new()
                .mask("/http/body/uptime_s", Kind::Float, "<float>")
                .mask("/http/body/used", Kind::UInt, "<uint>"),
            steps,
        );
        assert_eq!(out[0]["http"]["body"]["uptime_s"], "<float>");
        assert_eq!(out[0]["http"]["body"]["used"], "<uint>");
        assert_eq!(out[0]["http"]["body"]["name"], "x");
        // The wrong type stays visible, so the golden diff reports it.
        assert_eq!(out[1]["http"]["body"]["uptime_s"], "not a number");
    }

    #[test]
    fn shape_keeps_keys_and_replaces_scalars_with_type_names() {
        let steps = vec![json!({"recv": {"state": {"voice": {
            "available": true, "bytes": 5, "language": null, "models": [1, 2],
            "nested": {"name": "n"},
        }}}})];
        let out = normalized(Normalizer::new().shape("/recv/state/voice"), steps);
        assert_eq!(
            out[0]["recv"]["state"]["voice"],
            json!({
                "available": "<bool>", "bytes": "<uint>", "language": "<null>",
                "models": "<array>", "nested": {"name": "<string>"},
            })
        );
    }

    #[test]
    #[should_panic(expected = "matched nothing")]
    fn a_rule_that_matches_nothing_fails() {
        let steps = vec![json!({"recv": {"a": 1}})];
        Normalizer::new()
            .mask("/recv/missing", Kind::UInt, "<uint>")
            .apply(steps);
    }

    #[test]
    #[should_panic(expected = "survived normalization")]
    fn forbidden_text_that_survives_fails() {
        let steps = vec![json!({"recv": {"a": "contains secret-value here"}})];
        Normalizer::new().forbid("secret-value").apply(steps);
    }

    #[test]
    fn diff_marks_removed_and_added_lines_with_context() {
        let a: Vec<String> = ["a", "b", "c", "d", "e", "f", "g", "h", "i"]
            .map(String::from)
            .to_vec();
        let mut b = a.clone();
        b[4] = "E".to_string();
        let text = render_diff(&diff(&a, &b), 3);
        assert!(text.contains("- e"), "{text}");
        assert!(text.contains("+ E"), "{text}");
        assert!(text.contains("  d"), "{text}");
        assert!(text.contains("  ..."), "{text}");
        assert!(!text.contains("  a"), "{text}");
    }

    #[test]
    fn rewinding_forgets_an_abandoned_attempt() {
        let transcript = Transcript::default();
        transcript.send(&json!({"type": "kept"}));
        let mark = transcript.mark();
        transcript.recv(&json!({"type": "abandoned"}));
        transcript.rewind_to(mark);
        transcript.recv(&json!({"type": "retried"}));
        let steps = transcript.steps();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["send"]["type"], "kept");
        assert_eq!(steps[1]["recv"]["type"], "retried");
    }

    #[test]
    fn require_accepts_a_recorded_step() {
        let transcript = Transcript::default();
        transcript.recv(&json!({"type": "done"}));
        transcript.require("done", |step| step["recv"]["type"] == "done");
    }

    #[test]
    #[should_panic(expected = "frames were lost")]
    fn require_fails_when_a_step_was_never_recorded() {
        let transcript = Transcript::default();
        transcript.send(&json!({"type": "message"}));
        transcript.require("done", |step| step["recv"]["type"] == "done");
    }

    #[test]
    #[should_panic(expected = "survived normalization")]
    fn a_path_of_the_recording_machine_that_survives_fails() {
        let leaked = std::env::temp_dir().join("leaked-file");
        let steps = vec![json!({"recv": {"path": leaked.to_string_lossy()}})];
        Normalizer::new().forbid_machine_paths().apply(steps);
    }

    #[test]
    fn machine_paths_name_the_checkout_and_leave_out_a_bare_root() {
        let paths = machine_paths();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(
            paths.iter().any(|path| Path::new(path) == manifest),
            "{paths:?}"
        );
        assert!(
            paths
                .iter()
                .all(|path| Path::new(path).components().count() >= 2),
            "{paths:?}"
        );
    }

    /// Whether `report` has a line that is `sign` and then `text`, whatever
    /// the indentation in between.
    fn has_line(report: &str, sign: char, text: &str) -> bool {
        report
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix(sign))
            .any(|rest| rest.trim() == text)
    }

    fn tool_call(status: &str) -> Value {
        json!({"recv": {"method": "session/update", "params": {"update": {
            "sessionUpdate": "tool_call", "status": status, "toolCallId": "call_1",
        }}}})
    }

    fn initialize() -> Value {
        json!({"send": {"id": 1, "method": "initialize"}})
    }

    fn end_turn() -> Value {
        json!({"recv": {"id": 3, "result": {"stopReason": "end_turn"}}})
    }

    #[test]
    fn the_report_points_at_the_frame_that_changed() {
        let recorded = vec![initialize(), tool_call("in_progress"), end_turn()];
        let now = vec![initialize(), tool_call("running"), end_turn()];
        let report = sequence_report(&recorded, &now);
        assert!(
            report.contains("(3 recorded, 3 now; 1 changed, 0 added, 0 removed)"),
            "{report}"
        );
        assert!(
            report.contains("   ~    2  recv  session/update (tool_call)\n"),
            "{report}"
        );
        assert!(
            has_line(&report, '-', "\"status\": \"in_progress\","),
            "{report}"
        );
        assert!(
            has_line(&report, '+', "\"status\": \"running\","),
            "{report}"
        );
        // The frames that did not change are listed, without their contents.
        assert!(
            report.contains("      1  send  initialize id=1\n"),
            "{report}"
        );
        assert!(!report.contains("\"stopReason\""), "{report}");
    }

    #[test]
    fn the_report_lists_added_and_removed_frames() {
        let token = |text: &str| json!({"recv": {"type": "token", "text": text}});
        let done = json!({"recv": {"type": "done"}});
        let recorded = vec![initialize(), token("a"), done.clone()];
        let now = vec![initialize(), done, json!({"recv": {"type": "extra"}})];
        let report = sequence_report(&recorded, &now);
        assert!(
            report.contains("(3 recorded, 3 now; 0 changed, 1 added, 1 removed)"),
            "{report}"
        );
        assert!(
            report.contains("   -       recv  token  (step 2 of the golden)\n"),
            "{report}"
        );
        assert!(has_line(&report, '-', "\"text\": \"a\","), "{report}");
        assert!(report.contains("   +    3  recv  extra\n"), "{report}");
        assert!(has_line(&report, '+', "\"type\": \"extra\""), "{report}");
    }

    #[test]
    fn frames_of_other_kinds_are_not_shown_as_one_changed_frame() {
        let recorded = vec![json!({"recv": {"type": "token"}})];
        let now = vec![json!({"recv": {"type": "done"}})];
        let report = sequence_report(&recorded, &now);
        assert!(report.contains("0 changed, 1 added, 1 removed"), "{report}");
    }

    #[test]
    fn long_runs_of_unchanged_frames_are_folded() {
        let recorded: Vec<Value> = (0..12)
            .map(|n| json!({"recv": {"type": format!("frame-{n}")}}))
            .collect();
        let mut now = recorded.clone();
        now[6] = json!({"recv": {"type": "frame-6", "new": true}});
        let report = sequence_report(&recorded, &now);
        assert!(report.contains("        ...\n"), "{report}");
        assert!(report.contains("recv  frame-6\n"), "{report}");
        assert!(!report.contains("recv  frame-0\n"), "{report}");
        assert!(!report.contains("recv  frame-11\n"), "{report}");
    }

    #[test]
    fn the_report_falls_back_to_the_files_when_only_the_description_differs() {
        let steps = json!([{"recv": {"type": "done"}}]);
        let recorded = render(&json!({"description": "old", "scenario": "s", "steps": steps}));
        let now = render(&json!({"description": "new", "scenario": "s", "steps": steps}));
        let report = mismatch_report("s", Path::new("s.json"), &recorded, &now);
        assert!(report.contains("file contents:"), "{report}");
        assert!(
            has_line(&report, '-', "\"description\": \"old\","),
            "{report}"
        );
        assert!(
            has_line(&report, '+', "\"description\": \"new\","),
            "{report}"
        );
        assert!(report.contains("UPDATE_GOLDEN=1"), "{report}");
    }

    #[test]
    fn the_report_survives_a_golden_that_is_not_json() {
        let now = render(&json!({"scenario": "s", "steps": []}));
        let report = mismatch_report("s", Path::new("s.json"), "{ not json", &now);
        assert!(report.contains("file contents:"), "{report}");
    }

    fn token(text: &str) -> Value {
        json!({"recv": {"type": "token", "text": text}})
    }

    #[test]
    fn progress_of_a_recording_that_matches_so_far_names_the_next_step() {
        let recorded = vec![initialize(), token("a"), end_turn()];
        let report = progress_report(&recorded, &recorded[..2]);
        assert!(report.contains("every step so far matches"), "{report}");
        assert!(report.contains("2 of the golden's 3 steps"), "{report}");
        assert!(
            report.contains("the golden goes on with step 3: recv  result id=3"),
            "{report}"
        );
        // Steps the run has not got to yet are not shown as removed.
        assert!(!report.contains("   -  "), "{report}");
        assert!(
            !report.contains("0 changed, 0 added, 1 removed"),
            "{report}"
        );
    }

    #[test]
    fn progress_shows_a_renamed_field_and_what_comes_next() {
        let permission = |key: &str| json!({"recv": {"type": "permission_request", key: "p-1"}});
        let recorded = vec![initialize(), permission("request_id"), end_turn()];
        let now = vec![initialize(), permission("requestId")];
        let report = progress_report(&recorded, &now);
        assert!(report.contains("1 changed, 0 added, 0 removed"), "{report}");
        assert!(
            report.contains("   ~    2  recv  permission_request\n"),
            "{report}"
        );
        assert!(
            has_line(&report, '-', "\"request_id\": \"p-1\","),
            "{report}"
        );
        assert!(
            has_line(&report, '+', "\"requestId\": \"p-1\","),
            "{report}"
        );
        assert!(
            report.contains("the golden goes on with step 3"),
            "{report}"
        );
    }

    #[test]
    fn progress_shows_a_frame_the_golden_does_not_have() {
        let recorded = vec![initialize(), end_turn()];
        let now = vec![
            initialize(),
            json!({"recv": {"type": "operation_error", "code": "invalid_permission_response"}}),
        ];
        let report = progress_report(&recorded, &now);
        // The golden's step 2 was not "removed": the run has not got there.
        assert!(report.contains("0 changed, 1 added, 0 removed"), "{report}");
        assert!(
            report.contains("the golden goes on with step 2: recv  result id=3"),
            "{report}"
        );
        assert!(
            report.contains("   +    2  recv  operation_error\n"),
            "{report}"
        );
        assert!(
            has_line(&report, '+', "\"code\": \"invalid_permission_response\","),
            "{report}"
        );
    }

    #[test]
    fn progress_of_a_complete_recording_has_nothing_next() {
        let recorded = vec![initialize(), end_turn()];
        let report = progress_report(&recorded, &recorded);
        assert!(
            report.contains("the golden has no further steps"),
            "{report}"
        );
    }

    #[test]
    fn a_partial_recording_is_normalized_without_asserting_anything() {
        // The rule's frame has not arrived and the key is "forbidden": neither
        // may make the failure message fail.
        let steps = vec![json!({"recv": {"a": "secret-value 127.0.0.1:4321"}})];
        let out = Normalizer::new()
            .mask("/recv/missing", Kind::UInt, "<uint>")
            .forbid("secret-value")
            .apply_partial(steps);
        assert_eq!(out[0]["recv"]["a"], "secret-value 127.0.0.1:<PORT>");
    }

    fn mode(update: Option<&str>, ci: Option<&str>) -> Result<bool, String> {
        update_mode(update.map(OsStr::new), ci.map(OsStr::new))
    }

    #[test]
    fn goldens_are_compared_unless_a_rewrite_is_asked_for() {
        assert_eq!(mode(None, None), Ok(false));
        // CI alone is the normal case on a runner: compare.
        assert_eq!(mode(None, Some("true")), Ok(false));
    }

    #[test]
    fn only_update_golden_1_rewrites_and_only_off_ci() {
        assert_eq!(mode(Some("1"), None), Ok(true));
    }

    #[test]
    fn update_golden_on_ci_is_refused_whatever_the_values() {
        for ci in ["true", "1", "false", ""] {
            let reason = mode(Some("1"), Some(ci)).unwrap_err();
            assert!(reason.contains("CI"), "{reason}");
        }
        assert!(mode(Some("0"), Some("true")).is_err());
    }

    #[test]
    fn update_golden_with_any_other_value_is_refused_not_ignored() {
        for value in ["0", "", "true", "false", "yes", "2", " 1", "1 ", "01"] {
            let reason = mode(Some(value), None).unwrap_err();
            assert!(reason.contains("only the value 1"), "{value:?}: {reason}");
        }
    }

    #[test]
    fn rendering_sorts_keys_and_ends_with_a_newline() {
        let text = render(&json!({"b": 1, "a": {"d": 1, "c": 2}}));
        assert_eq!(
            text,
            "{\n  \"a\": {\n    \"c\": 2,\n    \"d\": 1\n  },\n  \"b\": 1\n}\n"
        );
    }
}
