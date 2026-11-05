//! Readable, bounded Git diffs for the current workspace.
//!
//! This module deliberately invokes `git` directly instead of going through a
//! shell. Both stdout and stderr are drained while retaining only bounded
//! prefixes, so a very large diff cannot grow the CLI's memory use without
//! limit.

use std::env;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Maximum UTF-8 byte length returned by [`workspace_diff`].
pub const MAX_WORKSPACE_DIFF_BYTES: usize = 200_000;

const MAX_GIT_STDERR_BYTES: usize = 16_000;
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
const TRUNCATION_MARKER: &str = "\n… [workspace diff truncated]\n";

/// Return a readable staged and unstaged diff for the current working directory.
pub fn workspace_diff() -> String {
    match env::current_dir() {
        Ok(cwd) => workspace_diff_at(&cwd),
        Err(error) => format!("Unable to show workspace diff: {error}"),
    }
}

/// Return a readable staged and unstaged diff for `cwd`.
///
/// This path-explicit variant is useful to callers that already track a
/// workspace root and keeps the current-directory wrapper straightforward.
pub fn workspace_diff_at(cwd: &Path) -> String {
    workspace_diff_at_with_limit(cwd, MAX_WORKSPACE_DIFF_BYTES)
}

fn workspace_diff_at_with_limit(cwd: &Path, limit: usize) -> String {
    let inside = match run_git(cwd, &["rev-parse", "--is-inside-work-tree"], 64) {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim() == "true"
        }
        Ok(output) => {
            let detail = error_detail(&output.stderr);
            if detail.to_ascii_lowercase().contains("not a git repository") {
                return "No workspace diff: current directory is not inside a Git repository."
                    .to_string();
            }
            return format!(
                "Unable to show workspace diff: Git could not inspect this directory{detail}."
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return "Unable to show workspace diff: git is not installed or is not available on PATH."
                .to_string();
        }
        Err(error) => return format!("Unable to show workspace diff: could not run git: {error}"),
    };

    if !inside {
        return "No workspace diff: current directory is not inside a Git work tree.".to_string();
    }

    let section_limit = limit.saturating_sub(1_024).max(3) / 3;
    let unstaged = match run_git(
        cwd,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--",
            ".",
        ],
        section_limit,
    ) {
        Ok(output) if output.status.success() => output,
        Ok(output) => return git_diff_error("unstaged", &output.stderr),
        Err(error) => return format!("Unable to show workspace diff: could not run git: {error}"),
    };
    let staged = match run_git(
        cwd,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--",
            ".",
        ],
        section_limit,
    ) {
        Ok(output) if output.status.success() => output,
        Ok(output) => return git_diff_error("staged", &output.stderr),
        Err(error) => return format!("Unable to show workspace diff: could not run git: {error}"),
    };

    let untracked = match run_git(
        cwd,
        &["ls-files", "--others", "--exclude-standard", "--", "."],
        section_limit,
    ) {
        Ok(output) if output.status.success() => output,
        Ok(output) => return git_diff_error("untracked-file", &output.stderr),
        Err(error) => return format!("Unable to show workspace diff: could not run git: {error}"),
    };

    let unstaged_text = bounded_section_text(
        &unstaged.stdout,
        unstaged.truncated,
        section_limit,
        "unstaged diff",
    );
    let staged_text = bounded_section_text(
        &staged.stdout,
        staged.truncated,
        section_limit,
        "staged diff",
    );
    let untracked_text = bounded_section_text(
        &untracked.stdout,
        untracked.truncated,
        section_limit,
        "untracked file list",
    );
    if unstaged_text.trim().is_empty()
        && staged_text.trim().is_empty()
        && untracked_text.trim().is_empty()
    {
        return "No workspace changes found (staged, unstaged, or untracked).".to_string();
    }

    let mut rendered = String::new();
    if !unstaged_text.trim().is_empty() {
        rendered.push_str("=== Unstaged changes ===\n");
        rendered.push_str(&unstaged_text);
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
    }
    if !staged_text.trim().is_empty() {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str("=== Staged changes ===\n");
        rendered.push_str(&staged_text);
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
    }

    if !untracked_text.trim().is_empty() {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str("=== Untracked files ===\n");
        rendered.push_str(&untracked_text);
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
    }

    if rendered.len() > limit {
        truncate_with_marker(rendered, limit)
    } else {
        rendered
    }
}

fn git_diff_error(kind: &str, stderr: &[u8]) -> String {
    let detail = error_detail(stderr);
    format!("Unable to show workspace diff: git {kind} diff failed{detail}.")
}

fn error_detail(stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    }
}

struct GitOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}

fn run_git(cwd: &Path, args: &[&str], stdout_limit: usize) -> io::Result<GitOutput> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("git stdout was not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("git stderr was not captured"))?;

    // Drain both pipes concurrently. Each reader discards bytes beyond its
    // retention limit instead of stopping, which avoids child-process pipe
    // deadlocks while keeping memory bounded.
    let stdout_reader = thread::spawn(move || read_capped(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_GIT_STDERR_BYTES));

    let status = wait_with_timeout(&mut child, GIT_TIMEOUT);
    let stdout = join_reader(stdout_reader, "stdout")?;
    let stderr = join_reader(stderr_reader, "stderr")?;
    let status = status?;

    Ok(GitOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        truncated: stdout.truncated,
    })
}

fn bounded_section_text(bytes: &[u8], raw_truncated: bool, limit: usize, label: &str) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let marker = format!("\n… [{label} truncated]");
    if !raw_truncated && text.len() <= limit {
        return text;
    }
    if limit <= marker.len() {
        let mut end = limit.min(marker.len());
        while !marker.is_char_boundary(end) {
            end -= 1;
        }
        return marker[..end].to_string();
    }
    let mut end = (limit - marker.len()).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(&marker);
    text
}

fn terminate_process_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let process_group = -(child.id() as i32);
        // SAFETY: the Git child was launched as leader of its own process group.
        unsafe {
            libc::kill(process_group, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child.kill();
    }
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            terminate_process_tree(child);
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("git timed out after {}s", timeout.as_secs()),
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

struct CappedRead {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_capped(mut reader: impl Read, limit: usize) -> io::Result<CappedRead> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut truncated = false;
    let mut chunk = [0_u8; 8192];

    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let keep = remaining.min(count);
        bytes.extend_from_slice(&chunk[..keep]);
        truncated |= keep < count;
    }

    Ok(CappedRead { bytes, truncated })
}

fn join_reader(
    reader: thread::JoinHandle<io::Result<CappedRead>>,
    stream: &str,
) -> io::Result<CappedRead> {
    reader
        .join()
        .map_err(|_| io::Error::other(format!("git {stream} reader thread panicked")))?
}

fn truncate_with_marker(mut text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    if limit == 0 {
        return String::new();
    }

    if limit <= TRUNCATION_MARKER.len() {
        let mut end = limit;
        while !TRUNCATION_MARKER.is_char_boundary(end) {
            end -= 1;
        }
        return TRUNCATION_MARKER[..end].to_string();
    }

    let mut end = limit - TRUNCATION_MARKER.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(TRUNCATION_MARKER);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn git(cwd: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn capped_reader_discards_excess_without_retaining_it() {
        let input = io::Cursor::new(vec![b'x'; 64]);
        let captured = read_capped(input, 10).unwrap();
        assert_eq!(captured.bytes, vec![b'x'; 10]);
        assert!(captured.truncated);
    }

    #[test]
    fn truncation_is_utf8_safe_and_bounded() {
        let text = "é".repeat(100);
        let rendered = truncate_with_marker(text, 51);
        assert!(rendered.len() <= 51);
        assert!(rendered.contains("truncated"));
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
    }

    #[test]
    fn lossy_utf8_expansion_stays_within_section_budget() {
        let text = bounded_section_text(&vec![0xff; 1_000], false, 128, "test section");
        assert!(text.len() <= 128);
        assert!(text.contains("test section truncated"));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_descendants_that_keep_pipes_open() {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.process_group(0);
        let mut child = command.spawn().unwrap();
        let started = Instant::now();
        let error = wait_with_timeout(&mut child, Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn outside_repository_has_honest_message() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let rendered = workspace_diff_at(tmp.path());
        assert!(rendered.contains("not inside a Git"), "{rendered}");
    }

    #[test]
    fn clean_repository_reports_no_changes() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        if !git(tmp.path(), &["init", "--quiet"]) {
            return;
        }

        assert_eq!(
            workspace_diff_at(tmp.path()),
            "No workspace changes found (staged, unstaged, or untracked)."
        );
    }

    #[test]
    fn untracked_files_are_reported() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        if !git(tmp.path(), &["init", "--quiet"]) {
            return;
        }
        fs::write(tmp.path().join("new.txt"), "new").unwrap();
        let rendered = workspace_diff_at(tmp.path());
        assert!(rendered.contains("=== Untracked files ==="), "{rendered}");
        assert!(rendered.contains("new.txt"), "{rendered}");
    }

    #[test]
    fn bounded_output_preserves_every_nonempty_section_heading() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        if !git(tmp.path(), &["init", "--quiet"]) {
            return;
        }
        let path = tmp.path().join("large.txt");
        fs::write(&path, "staged\n".repeat(10_000)).unwrap();
        assert!(git(tmp.path(), &["add", "large.txt"]));
        fs::write(
            &path,
            format!(
                "{}{}",
                "staged\n".repeat(10_000),
                "unstaged\n".repeat(10_000)
            ),
        )
        .unwrap();
        fs::write(tmp.path().join("untracked.txt"), "new").unwrap();

        let rendered = workspace_diff_at_with_limit(tmp.path(), 3_000);
        assert!(rendered.contains("=== Unstaged changes ==="), "{rendered}");
        assert!(rendered.contains("=== Staged changes ==="), "{rendered}");
        assert!(rendered.contains("=== Untracked files ==="), "{rendered}");
        assert!(rendered.len() <= 3_000);
    }

    #[test]
    fn combines_unstaged_and_staged_sections() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        if !git(tmp.path(), &["init", "--quiet"]) {
            return;
        }

        let path = tmp.path().join("example.txt");
        fs::write(&path, "staged\n").unwrap();
        assert!(git(tmp.path(), &["add", "example.txt"]));
        fs::write(&path, "staged\nunstaged\n").unwrap();

        let rendered = workspace_diff_at(tmp.path());
        assert!(rendered.contains("=== Unstaged changes ==="), "{rendered}");
        assert!(rendered.contains("=== Staged changes ==="), "{rendered}");
        assert!(rendered.contains("+unstaged"), "{rendered}");
        assert!(rendered.contains("+staged"), "{rendered}");
    }
}
