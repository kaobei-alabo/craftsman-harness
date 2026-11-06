//! Host tools: the filesystem and shell operations the agent calls on the host
//! (plus the gating lists and call summaries shared with `web_tools.rs`).
//!
//! `execute`, `write_file`, `edit_file` are the host-mutating set and MUST
//! stay behind the HITL gate (craftsman-harness rule #6); `read_file` / `ls` are
//! read-only and ungated.

use crate::shell::{OutputFormat, ShellSettings};
use crate::shell_output::{
    Capture, LEGACY_READ_LIMIT, SPILL_LIMIT, StreamOutput, format_head_tail, next_call_id,
};
use craftsman_harness::backend::{EMPTY_FILE_REMINDER, cat_n};
use craftsman_harness::{FileInfo, ToolOutcome};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;

#[cfg(test)]
mod observations;
#[cfg(test)]
pub(crate) mod owned_cancellation_tests;

pub const GATED_TOOLS: &[&str] = &["execute", "write_file", "edit_file"];

/// Network-egress tools. These reach out to the public internet, so they go
/// through the HITL gate too (craftsman-harness rule #6) — but under a SEPARATE
/// `auto_approve_network` switch (config `extra["auto_approve_web"]`) rather
/// than the host-mutation `auto_approve`, so a user can pre-approve reads
/// without pre-approving writes/exec (or vice versa).
pub const NETWORK_TOOLS: &[&str] = &["web_fetch", "web_search"];
const MAX_OUTPUT_BYTES: usize = crate::shell_output::OUTPUT_BUDGET;

/// Execute one tool call. Errors are returned as the tool RESULT string
/// (the agent sees them and adjusts) — this function never fails the turn.
///
/// The framework agent loop drives tools via the `HostTool` bridge in
/// `agent.rs` (schemas come from `Tool::def`); this direct dispatch remains
/// for the unit tests below (the REPL's `!cmd` escape calls
/// [`run_shell_escape`] with the configured shell settings).
#[cfg(test)]
pub async fn run_tool(name: &str, args: &Value) -> String {
    match name {
        "execute" => execute(args).await,
        "read_file" => read_file(args),
        "write_file" => write_file(args),
        "edit_file" => edit_file(args),
        "ls" => ls(args),
        other => format!("Error: unknown tool '{other}'"),
    }
}

/// Raw process capture — the pure operation behind the `execute` tool.
/// Formatting (stderr framing, exit-code marker, cutting) is the caller's
/// (dispatch / Backend adapter) responsibility: [`ExecRaw::format`].
#[derive(Debug)]
pub(crate) struct ExecRaw {
    pub stdout: StreamOutput,
    pub stderr: StreamOutput,
    pub exit_code: i32,
    /// Wall-clock time from spawn to the end of the call.
    pub duration: Duration,
}

impl ExecRaw {
    /// The tool text for a finished command in the configured format, and
    /// whether it is a cut of a larger output.
    pub(crate) fn format(&mut self, settings: &ShellSettings) -> (String, bool) {
        let clean = settings.clean_output;
        match settings.output_format {
            OutputFormat::Legacy => {
                let full = format_exec_unclamped(
                    self.stdout.text_for(clean),
                    self.stderr.text_for(clean),
                    self.exit_code,
                );
                let cut = full.len() > MAX_OUTPUT_BYTES;
                (clamp(full), cut)
            }
            OutputFormat::HeadTail => {
                format_head_tail(&mut self.stdout, &mut self.stderr, self.exit_code, clean)
            }
        }
    }

    /// Facts about the call for [`craftsman_harness::ToolMeta`].
    fn meta(&self, truncated: bool) -> craftsman_harness::ToolMeta {
        let mut meta = craftsman_harness::ToolMeta::new();
        meta.exit_code = Some(self.exit_code);
        meta.duration = Some(self.duration);
        meta.bytes = Some(self.stdout.total() + self.stderr.total());
        meta.truncated = truncated;
        meta
    }
}

/// Why `execute_impl` produced no normal result.
#[derive(Debug)]
pub(crate) enum ExecFailure {
    /// An actual turn/session stop; captured bytes exist only after a spawn.
    Cancelled { partial: Option<Box<ExecRaw>> },
    /// The shell could not be started.
    Spawn(std::io::Error),
    /// The call hit its limit; the tree was killed. `partial` is what the
    /// command printed before (empty under the legacy output format, which
    /// does not read it).
    TimedOut {
        limit_secs: u64,
        partial: Box<ExecRaw>,
    },
}

impl ExecFailure {
    /// The tool text for this failure. Legacy: today's two messages. Head and
    /// tail: the timeout error names the limit and carries the partial output.
    pub(crate) fn text(&mut self, settings: &ShellSettings) -> String {
        match self {
            ExecFailure::Cancelled { .. } => "Error: command cancelled by the user.".into(),
            ExecFailure::Spawn(e) => format!("Error spawning command: {e}"),
            ExecFailure::TimedOut {
                limit_secs,
                partial,
            } => match settings.output_format {
                OutputFormat::Legacy => format!("Error: command timed out after {limit_secs}s"),
                OutputFormat::HeadTail => timeout_text(*limit_secs, partial, settings),
            },
        }
    }
}

/// `shell.output_format = "head_tail"`: the timeout error, with the output
/// the command printed before it (cut like any other result).
fn timeout_text(limit_secs: u64, partial: &mut ExecRaw, settings: &ShellSettings) -> String {
    let mut text = format!("Error: command timed out after {limit_secs}s and was stopped.");
    if settings.timeout_param {
        text.push_str(&format!(
            " If it needs longer, pass a larger `timeout` (at most {}).",
            settings.max_call_timeout_secs()
        ));
    }
    let clean = settings.clean_output;
    if partial.stdout.text_for(clean).trim().is_empty()
        && partial.stderr.text_for(clean).trim().is_empty()
    {
        text.push_str(" It printed nothing before the timeout.");
        return text;
    }
    text.push_str(" Output printed before the timeout:\n");
    text.push_str(&format_head_tail(&mut partial.stdout, &mut partial.stderr, 0, clean).0);
    text
}

#[cfg(test)]
async fn execute(args: &Value) -> String {
    run_shell_escape(
        args["command"].as_str().unwrap_or_default(),
        &ShellSettings::default(),
    )
    .await
}

/// The `!cmd` escape (and `run_tool("execute")`): run `cmd` in the current
/// directory with the given shell settings and format it like the tool.
pub async fn run_shell_escape(cmd: &str, settings: &ShellSettings) -> String {
    if cmd.is_empty() {
        return "Error: empty command".into();
    }
    let limit = Duration::from_secs(settings.timeout_secs);
    match execute_impl(cmd, Path::new("."), settings, limit).await {
        Ok(mut raw) => raw.format(settings).0,
        Err(mut failure) => failure.text(settings),
    }
}

/// The `execute` tool bound to a working directory and the `shell` settings:
/// what the host `execute` tool calls, so the per-call `timeout` argument
/// and the output format reach the process.
pub struct ShellRunner {
    root: PathBuf,
    settings: ShellSettings,
    headless: Option<crate::exec::shell::Shell>,
    #[cfg(test)]
    legacy_observations: Option<observations::Owner>,
}

impl ShellRunner {
    pub fn new(root: PathBuf, settings: ShellSettings) -> Self {
        Self {
            root,
            settings,
            headless: None,
            #[cfg(test)]
            legacy_observations: Some(observations::Owner::new()),
        }
    }

    /// Private headless adapter: retain captured settings/credential sources,
    /// but never create an unredacted spill file, including on timeout/cutting.
    pub(crate) fn headless(scope: crate::agent::ShellScope<'_>) -> Self {
        Self {
            root: scope.prepared().workspace().to_path_buf(),
            settings: scope.prepared().shell().clone(),
            headless: Some(crate::exec::shell::Shell::from_scope(scope)),
            #[cfg(test)]
            legacy_observations: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn observation_view_for_tests(&self) -> Option<crate::exec::shell::ObservationView> {
        self.headless
            .as_ref()
            .map(|shell| shell.observation_view_for_tests())
    }

    pub fn settings(&self) -> &ShellSettings {
        &self.settings
    }

    /// Query an already-installed builder lease, never create authority. Host
    /// dispatch must route even argument-validation errors through this scope.
    pub(crate) fn has_headless_scope(&self) -> bool {
        self.headless.is_some()
    }

    /// Run one `execute` call as the host tool's outcome, with its
    /// [`craftsman_harness::ToolMeta`] (exit code, duration, bytes, truncated,
    /// error kind; never sent to the model). Error texts are final: legacy
    /// errors keep today's `tool 'execute' error: ` prefix.
    pub async fn run_outcome(&self, cmd: &str, timeout_arg: Option<&Value>) -> ToolOutcome {
        if let Some(shell) = &self.headless {
            return shell.run(cmd, timeout_arg).await;
        }
        let limit = Duration::from_secs(self.settings.call_timeout_secs(timeout_arg));
        let started = std::time::Instant::now();
        let cancellation = craftsman_harness::TurnCancellation::current();
        #[cfg(not(test))]
        let result = execute_impl_core(
            cmd,
            &self.root,
            &self.settings,
            limit,
            cancellation.as_ref(),
        )
        .await;
        #[cfg(test)]
        let result = execute_impl_core(
            cmd,
            &self.root,
            &self.settings,
            limit,
            self.legacy_observations.as_ref(),
            cancellation.as_ref(),
        )
        .await;
        match result {
            Ok(mut raw) => {
                let (text, truncated) = raw.format(&self.settings);
                ToolOutcome::ok(text).with_meta(raw.meta(truncated))
            }
            Err(mut failure) => {
                let text = failure.text(&self.settings);
                let mut meta = craftsman_harness::ToolMeta::new();
                meta.duration = Some(started.elapsed());
                let text = match &failure {
                    ExecFailure::Cancelled { partial } => {
                        meta.error_kind = Some(craftsman_harness::ToolErrorKind::Cancelled);
                        if let Some(partial) = partial {
                            meta.bytes = Some(partial.stdout.total() + partial.stderr.total());
                        }
                        tool_error(text)
                    }
                    ExecFailure::TimedOut { partial, .. } => {
                        meta.error_kind = Some(craftsman_harness::ToolErrorKind::Timeout);
                        meta.bytes = Some(partial.stdout.total() + partial.stderr.total());
                        meta.truncated = self.settings.output_format == OutputFormat::Legacy
                            && meta.bytes != Some(0);
                        if self.settings.output_format == OutputFormat::HeadTail {
                            text
                        } else {
                            tool_error(text)
                        }
                    }
                    ExecFailure::Spawn(_) => {
                        meta.error_kind = Some(craftsman_harness::ToolErrorKind::Spawn);
                        tool_error(text)
                    }
                };
                ToolOutcome::err(text).with_meta(meta)
            }
        }
    }

    /// [`Self::run_outcome`] as text: `Ok` for a normal result, `Err` for a
    /// tool error.
    #[cfg(test)]
    pub async fn run(&self, cmd: &str, timeout_arg: Option<&Value>) -> Result<String, String> {
        let outcome = self.run_outcome(cmd, timeout_arg).await;
        if outcome.is_error {
            Err(outcome.content)
        } else {
            Ok(outcome.content)
        }
    }
}

// Only the original assertion-panic teardown emits this owner's fixed report.
// No global panic hook, normal-frontend sink, native lookup or new IO work.
#[cfg(test)]
impl Drop for ShellRunner {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(owner) = &self.legacy_observations
        {
            eprintln!("{}", owner.report());
        }
    }
}

/// Today's wording of an execute error through the host tool.
fn tool_error(message: String) -> String {
    craftsman_harness::AgentError::Tool {
        name: "execute".into(),
        message,
    }
    .to_string()
}

/// The execute tool's parameter schema. `None` (no runner) and the defaults
/// give today's schema byte for byte.
pub(crate) fn execute_parameters(settings: Option<&ShellSettings>) -> Value {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "command".into(),
        serde_json::json!({"type": "string", "description": "Shell command to run"}),
    );
    if let Some(settings) = settings.filter(|s| s.timeout_param) {
        properties.insert(
            "timeout".into(),
            serde_json::json!({
                "type": "integer",
                "description": format!(
                    "Seconds before the command is stopped (default {}, at most {}).",
                    settings.timeout_secs,
                    settings.max_call_timeout_secs()
                )
            }),
        );
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": ["command"]
    })
}

/// Format captured process output for the transcript: stdout, then a framed
/// stderr section when non-empty, then an exit-code marker when non-zero;
/// clamped to the output budget. The legacy format of the `!cmd` escape and
/// the framework `execute` tool, so both stay byte-identical.
pub(crate) fn format_exec(stdout: String, stderr: String, exit_code: i32) -> String {
    clamp(format_exec_unclamped(stdout, stderr, exit_code))
}

fn format_exec_unclamped(stdout: String, stderr: String, exit_code: i32) -> String {
    let mut text = stdout;
    if !stderr.trim().is_empty() {
        text.push_str("\n--- stderr ---\n");
        text.push_str(&stderr);
    }
    if exit_code != 0 {
        text.push_str(&format!("\n[exit code {exit_code}]"));
    }
    text
}

/// One pipe's capture, shared between its reader and the caller: the reader
/// appends as bytes arrive, so a timeout keeps whatever was printed before
/// it even when the reader is abandoned.
type SharedCapture = Arc<Mutex<Capture>>;

/// Read `pipe` into `capture`, at most `limit` bytes, then drop the pipe:
/// closing our read end gives a still-writing child EPIPE (instead of a
/// full-pipe deadlock that would stall the other read and the wait until
/// the timeout).
async fn pump(mut pipe: impl tokio::io::AsyncRead + Unpin, capture: SharedCapture, limit: u64) {
    let mut chunk = vec![0u8; 8192];
    let mut read: u64 = 0;
    while read < limit {
        let want = chunk.len().min((limit - read) as usize);
        match pipe.read(&mut chunk[..want]).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                read += n as u64;
                if let Ok(mut c) = capture.lock() {
                    c.push(&chunk[..n]);
                }
            }
        }
    }
}

/// After a kill, how long the pipes get to reach EOF before what was read
/// so far is taken as the partial output. The P0 spike measured 2 to 19 ms
/// after a Job Object kill; only a descendant that escaped the kill (a
/// process outside the group or job) holds a pipe open this long.
const DRAIN_AFTER_KILL: Duration = Duration::from_secs(2);

/// Run `cmd` through the host shell in `cwd`, returning captured
/// stdout/stderr/exit code (`sh -c` on Unix, PowerShell on Windows), inside
/// a [`crate::platform_os::ProcessTree`], with `limit` as the wall-clock
/// limit (reading the output to EOF counts).
///
/// Legacy format: each pipe is read at most 2x the budget — a firehose
/// command (e.g. `yes`) must not balloon memory before clamp() trims it.
/// head_tail: each pipe is read up to the spill limit, keeping a head and a
/// tail in memory and the rest in a spill file (`shell_output.rs`). On
/// timeout the tree is killed: the whole Job Object on Windows, the whole
/// process group on Unix with `shell.process_group`, the direct child only
/// on Unix without it (today's behaviour). Under head_tail the pipes are
/// then drained for at most [`DRAIN_AFTER_KILL`] and the output printed
/// before the timeout is returned with the failure.
pub(crate) async fn execute_impl(
    cmd: &str,
    cwd: &Path,
    settings: &ShellSettings,
    limit: Duration,
) -> Result<ExecRaw, ExecFailure> {
    let cancellation = craftsman_harness::TurnCancellation::current();
    execute_impl_core(
        cmd,
        cwd,
        settings,
        limit,
        #[cfg(test)]
        None,
        cancellation.as_ref(),
    )
    .await
}

// Shared original executor. The optional observer exists only in test builds;
// both direct calls and the real runner still use the same ProcessTree/pump.
async fn execute_impl_core(
    cmd: &str,
    cwd: &Path,
    settings: &ShellSettings,
    limit: Duration,
    #[cfg(test)] owner: Option<&observations::Owner>,
    cancellation: Option<&craftsman_harness::TurnCancellation>,
) -> Result<ExecRaw, ExecFailure> {
    let started = tokio::time::Instant::now();
    #[cfg(test)]
    let observation = owner.and_then(|owner| owner.begin(started, limit));
    let deadline = started + limit;
    if cancellation.is_some_and(|turn| turn.is_cancelled()) {
        return Err(ExecFailure::Cancelled { partial: None });
    }
    let mut options = settings.spawn_options();
    if cancellation.is_some() {
        options.process_group = true;
    }
    let mut tree = crate::platform_os::ProcessTree::spawn_shell(cmd, cwd, &options)
        .map_err(ExecFailure::Spawn)?;
    #[cfg(test)]
    if let Some(call) = &observation {
        call.spawned(tree.id());
    }
    let stdout = tree.take_stdout().expect("piped");
    let stderr = tree.take_stderr().expect("piped");
    #[cfg(test)]
    let stdout =
        observations::Reader::new(stdout, observation.clone(), observations::Stream::Stdout);
    #[cfg(test)]
    let stderr =
        observations::Reader::new(stderr, observation.clone(), observations::Stream::Stderr);

    let (out, err, read_limit) = match settings.output_format {
        OutputFormat::Legacy => (
            Capture::legacy("stdout"),
            Capture::legacy("stderr"),
            LEGACY_READ_LIMIT,
        ),
        OutputFormat::HeadTail => {
            let call = next_call_id();
            (
                Capture::head_tail("stdout", call),
                Capture::head_tail("stderr", call),
                SPILL_LIMIT,
            )
        }
    };
    let out: SharedCapture = Arc::new(Mutex::new(out));
    let err: SharedCapture = Arc::new(Mutex::new(err));
    let readers = async {
        tokio::join!(
            pump(stdout, out.clone(), read_limit),
            pump(stderr, err.clone(), read_limit)
        )
    };
    tokio::pin!(readers);

    #[cfg(test)]
    if let Some(call) = &observation {
        call.readers_enter();
    }
    let mut cancelled = false;
    let readers_result = match cancellation {
        Some(turn) => tokio::select! {
            biased;
            _ = turn.cancelled() => { cancelled = true; None },
            result = tokio::time::timeout_at(deadline, &mut readers) => Some(result),
        },
        None => Some(tokio::time::timeout_at(deadline, &mut readers).await),
    };
    #[cfg(test)]
    if let Some(call) = &observation {
        call.readers_return(readers_result.as_ref().is_some_and(|result| result.is_ok()));
    }
    let status = match readers_result {
        Some(Ok(_)) => {
            #[cfg(test)]
            if let Some(call) = &observation {
                call.wait_enter();
            }
            let result = match cancellation {
                Some(turn) => tokio::select! {
                    biased;
                    _ = turn.cancelled() => { cancelled = true; None },
                    result = tokio::time::timeout_at(deadline, tree.wait()) => result.ok(),
                },
                None => tokio::time::timeout_at(deadline, tree.wait()).await.ok(),
            };
            #[cfg(test)]
            if let Some(call) = &observation {
                call.wait_return(&result);
            }
            result
        }
        Some(Err(_)) | None => None,
    };
    let take = |capture: &SharedCapture| match capture.lock() {
        Ok(c) => c.snapshot(),
        Err(poisoned) => poisoned.into_inner().snapshot(),
    };
    match status {
        Some(status) => {
            #[cfg(test)]
            if let Some(call) = &observation {
                call.returned(false);
            }
            Ok(ExecRaw {
                stdout: take(&out),
                stderr: take(&err),
                exit_code: status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1),
                duration: started.elapsed(),
            })
        }
        None => {
            #[cfg(test)]
            if let Some(call) = &observation {
                call.before_stop(tree.id());
            }
            tree.kill_tree().await;
            #[cfg(test)]
            if let Some(call) = &observation {
                call.stop_return();
            }
            if cancelled || settings.output_format == OutputFormat::HeadTail {
                #[cfg(test)]
                if let Some(call) = &observation {
                    call.drain_enter();
                }
                let drain_result = tokio::time::timeout(DRAIN_AFTER_KILL, &mut readers).await;
                #[cfg(test)]
                if let Some(call) = &observation {
                    call.drain_return(drain_result.is_ok());
                }
                let _ = drain_result;
            }
            #[cfg(test)]
            if let Some(call) = &observation {
                call.returned(!cancelled);
            }
            // Legacy does not read the partial output (today's text drops it).
            let partial = Box::new(ExecRaw {
                stdout: take(&out),
                stderr: take(&err),
                exit_code: -1,
                duration: started.elapsed(),
            });
            if cancelled {
                return Err(ExecFailure::Cancelled {
                    partial: Some(partial),
                });
            }
            Err(ExecFailure::TimedOut {
                limit_secs: limit.as_secs(),
                partial,
            })
        }
    }
}

#[cfg(test)]
fn read_file(args: &Value) -> String {
    let path = args["file_path"].as_str().unwrap_or_default();
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"].as_u64().unwrap_or(100).max(1) as usize;
    match read_file_impl(Path::new(path), offset, limit) {
        Ok(s) => s,
        Err(e) => e,
    }
}

/// Read `path` as `cat -n`-numbered text, 0-indexed `offset`, `limit`-line page.
///
/// Numbering starts at 1 (models are trained on that format, and edit_file
/// old_strings come back correct only when the read format is the one they
/// expect). Reuses the framework's
/// [`cat_n`] so the numbering stays byte-identical across both crates.
pub(crate) fn read_file_impl(path: &Path, offset: usize, limit: usize) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Error reading {}: {e}", path.display()))?;
    if text.is_empty() {
        return Ok(EMPTY_FILE_REMINDER.to_string());
    }
    let total = text.lines().count();
    if offset >= total && total > 0 {
        return Err(format!(
            "Error: offset {offset} is past the end of {} ({total} lines)",
            path.display()
        ));
    }
    Ok(clamp(cat_n(&text, offset, limit)))
}

#[cfg(test)]
fn write_file(args: &Value) -> String {
    let path = args["file_path"].as_str().unwrap_or_default();
    let content = args["content"].as_str().unwrap_or_default();
    match write_file_impl(Path::new(path), content) {
        Ok(s) => s,
        Err(e) => e,
    }
}

/// Create `path` with `content`. write_file only CREATES —
/// overwrites must go through read + edit_file so the model has seen what it
/// destroys. Parent dirs are created as needed.
pub(crate) fn write_file_impl(path: &Path, content: &str) -> Result<String, String> {
    if path.exists() {
        return Err(format!(
            "Cannot write to {} because it already exists. \
             Read and then make an edit, or write to a new path.",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Error creating {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(path, content)
        .map(|()| format!("Wrote {} bytes to {}", content.len(), path.display()))
        .map_err(|e| format!("Error writing {}: {e}", path.display()))
}

#[cfg(test)]
fn edit_file(args: &Value) -> String {
    let path = args["file_path"].as_str().unwrap_or_default();
    let old = args["old_string"].as_str().unwrap_or_default();
    let new = args["new_string"].as_str().unwrap_or_default();
    let replace_all = args["replace_all"].as_bool().unwrap_or(false);
    match edit_file_impl(Path::new(path), old, new, replace_all) {
        Ok(s) => s,
        Err(e) => e,
    }
}

/// Replace `old` with `new` in `path`. `old` must occur exactly once unless
/// `replace_all`. Empty `old` is rejected.
pub(crate) fn edit_file_impl(
    path: &Path,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, String> {
    if old.is_empty() {
        return Err("Error: old_string must not be empty".into());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Error reading {}: {e}", path.display()))?;
    let count = text.matches(old).count();
    if count == 0 {
        return Err(format!("Error: old_string not found in {}", path.display()));
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "Error: old_string occurs {count} times in {}; pass replace_all or a longer unique string",
            path.display()
        ));
    }
    let updated = if replace_all {
        text.replace(old, new)
    } else {
        text.replacen(old, new, 1)
    };
    std::fs::write(path, updated)
        .map(|()| {
            format!(
                "Edited {} ({count} replacement{})",
                path.display(),
                if count == 1 { "" } else { "s" }
            )
        })
        .map_err(|e| format!("Error writing {}: {e}", path.display()))
}

#[cfg(test)]
fn ls(args: &Value) -> String {
    let path = args["path"].as_str().unwrap_or(".");
    match ls_impl(Path::new(path)) {
        Ok(entries) => entries
            .into_iter()
            .map(|fi| fi.path)
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => e,
    }
}

/// List directory entries of `path`, sorted, with a trailing `/` on directory
/// names (kept in [`FileInfo::path`]). `size` is 0 for directories.
pub(crate) fn ls_impl(path: &Path) -> Result<Vec<FileInfo>, String> {
    let entries =
        std::fs::read_dir(path).map_err(|e| format!("Error listing {}: {e}", path.display()))?;
    let mut infos: Vec<FileInfo> = entries
        .filter_map(|e| e.ok())
        .map(|e| {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let mut name = e.file_name().to_string_lossy().into_owned();
            if is_dir {
                name.push('/');
            }
            let size = if is_dir {
                0
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            };
            FileInfo { path: name, size }
        })
        .collect();
    infos.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(infos)
}

/// Recursively grep `pattern` under `base`, emitting `{relpath}:{lineno}:{line}`
/// hits. Skips `.git`/`node_modules`/`target` dirs and binary-looking files
/// (NUL byte in the first chunk). Output is capped with the shared
/// [`MAX_OUTPUT_BYTES`] budget + truncation marker; no hits yields the
/// `No matches found for '{pattern}'` sentinel.
pub(crate) fn grep_impl(base: &Path, pattern: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut truncated = false;
    let mut stack = vec![base.to_path_buf()];
    // Deterministic order: sort each directory level.
    while let Some(dir) = stack.pop() {
        let mut children: Vec<std::path::PathBuf> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
            Err(_) => continue,
        };
        // Reverse-sort so the stack pops in ascending order.
        children.sort_by(|a, b| b.cmp(a));
        for child in children {
            let name = child.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if child.is_dir() {
                if matches!(name, ".git" | "node_modules" | "target") {
                    continue;
                }
                stack.push(child);
                continue;
            }
            let content = match std::fs::read(&child) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            // Binary heuristic: a NUL byte in the first 8KiB.
            if content.iter().take(8192).any(|&b| b == 0) {
                continue;
            }
            let text = String::from_utf8_lossy(&content);
            let rel = child.strip_prefix(base).unwrap_or(&child);
            let rel = rel.to_string_lossy().replace('\\', "/");
            for (i, line) in text.lines().enumerate() {
                if line.contains(pattern) {
                    out.push_str(&format!("{}:{}:{}\n", rel, i + 1, line));
                    if out.len() > MAX_OUTPUT_BYTES {
                        truncated = true;
                        break;
                    }
                }
            }
            if truncated {
                break;
            }
        }
        if truncated {
            break;
        }
    }
    if truncated {
        let mut cut = MAX_OUTPUT_BYTES.min(out.len());
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("\n… [output truncated]");
        return Ok(out);
    }
    if out.is_empty() {
        Ok(format!("No matches found for '{pattern}'"))
    } else {
        Ok(out)
    }
}

pub(crate) fn clamp(mut s: String) -> String {
    if s.len() > MAX_OUTPUT_BYTES {
        let mut cut = MAX_OUTPUT_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n… [output truncated]");
    }
    s
}

/// One-line summary of a tool call for the activity transcript.
pub fn summarize_call(name: &str, args: &Value) -> String {
    use crate::util::one_line;
    match name {
        "execute" => format!(
            "🔧 execute › {}",
            one_line(args["command"].as_str().unwrap_or("?"), 160)
        ),
        "read_file" => format!(
            "📖 read_file › {}",
            args["file_path"].as_str().unwrap_or("?")
        ),
        "write_file" | "edit_file" => {
            format!("✏️  {name} › {}", args["file_path"].as_str().unwrap_or("?"))
        }
        // Network egress: the approval prompt reuses this summary, and the
        // human can only make a meaningful decision if they see WHERE the
        // request goes (rule #6 — a bare "web_fetch" would be a rubber stamp).
        "web_fetch" => format!(
            "🌐 web_fetch › {}",
            one_line(args["url"].as_str().unwrap_or("?"), 200)
        ),
        "web_search" => format!(
            "🔎 web_search › {}",
            one_line(args["query"].as_str().unwrap_or("?"), 160)
        ),
        _ => format!("🔧 {name}"),
    }
}

/// Human-readable detail for an approval dialog. Unlike the one-line activity
/// summary, this includes enough context to review a mutation before allowing it.
pub fn approval_details(name: &str, args: &Value) -> String {
    let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    approval_details_at(name, args, &workspace)
}

pub fn approval_details_at(name: &str, args: &Value, workspace: &std::path::Path) -> String {
    fn preview(value: &str) -> String {
        const LIMIT: usize = 1_200;
        if value.chars().count() <= LIMIT {
            value.to_string()
        } else {
            format!(
                "{}\n… [preview truncated]",
                value.chars().take(LIMIT).collect::<String>()
            )
        }
    }
    match name {
        "execute" => format!(
            "Working directory: {}\n\nCommand:\n{}",
            workspace.display(),
            args["command"].as_str().unwrap_or("?")
        ),
        "write_file" => {
            let content = args["content"].as_str().unwrap_or_default();
            format!(
                "Path: {}\nSize: {} bytes · {} line(s)\n\nPreview:\n{}",
                args["file_path"].as_str().unwrap_or("?"),
                content.len(),
                content.lines().count(),
                preview(content),
            )
        }
        "edit_file" => format!(
            "Path: {}\nReplace all: {}\n\n--- before\n{}\n+++ after\n{}",
            args["file_path"].as_str().unwrap_or("?"),
            args["replace_all"].as_bool().unwrap_or(false),
            preview(args["old_string"].as_str().unwrap_or_default()),
            preview(args["new_string"].as_str().unwrap_or_default()),
        ),
        "web_fetch" => format!("Destination: {}", args["url"].as_str().unwrap_or("?")),
        "web_search" => format!("Search query: {}", args["query"].as_str().unwrap_or("?")),
        _ => serde_json::to_string_pretty(args).unwrap_or_else(|_| "{}".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_os::test_procs::BOUND;
    use serde_json::json;

    #[cfg(unix)]
    const STDOUT_STDERR_EXIT: &str = "echo hi; echo err >&2; exit 3";
    #[cfg(windows)]
    const STDOUT_STDERR_EXIT: &str = "Write-Output 'hi'; [Console]::Error.WriteLine('err'); exit 3";

    #[cfg(unix)]
    const FIREHOSE: &str = "yes 0123456789abcdef | head -c 100000000";
    #[cfg(windows)]
    const FIREHOSE: &str =
        "1..100000 | ForEach-Object { [Console]::Out.Write('0123456789abcdef') }";

    #[test]
    fn approval_details_include_reviewable_edit_context() {
        let details = approval_details(
            "edit_file",
            &json!({
                "file_path": "src/main.rs",
                "old_string": "before",
                "new_string": "after",
                "replace_all": false
            }),
        );
        assert!(details.contains("src/main.rs"));
        assert!(details.contains("--- before"));
        assert!(details.contains("+++ after"));
    }

    #[tokio::test]
    async fn execute_captures_stdout_stderr_and_exit_code() {
        let out = run_tool("execute", &json!({"command": STDOUT_STDERR_EXIT})).await;
        assert!(out.contains("hi"));
        assert!(out.contains("err"));
        assert!(out.contains("[exit code 3]"));
    }

    #[tokio::test]
    async fn write_then_read_then_edit_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt").to_string_lossy().into_owned();
        let w = run_tool(
            "write_file",
            &json!({"file_path": p, "content": "alpha beta"}),
        )
        .await;
        assert!(w.starts_with("Wrote"));
        // read_file returns cat -n format.
        let r = run_tool("read_file", &json!({"file_path": p})).await;
        assert!(r.contains("1\talpha beta"), "got: {r}");
        let e = run_tool(
            "edit_file",
            &json!({"file_path": p, "old_string": "beta", "new_string": "gamma"}),
        )
        .await;
        assert!(e.starts_with("Edited"));
        let r2 = run_tool("read_file", &json!({"file_path": p})).await;
        assert!(r2.contains("alpha gamma"), "got: {r2}");
    }

    #[tokio::test]
    async fn write_file_refuses_to_overwrite_existing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt").to_string_lossy().into_owned();
        std::fs::write(&p, "original").unwrap();
        let w = run_tool("write_file", &json!({"file_path": p, "content": "clobber"})).await;
        assert!(w.contains("already exists"), "got: {w}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "original");
    }

    #[tokio::test]
    async fn read_file_paginates_with_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt").to_string_lossy().into_owned();
        let body: String = (1..=250).map(|i| format!("line-{i}\n")).collect();
        std::fs::write(&p, body).unwrap();
        // Default page: 100 lines from the top.
        let r = run_tool("read_file", &json!({"file_path": p})).await;
        assert!(r.contains("1\tline-1"));
        assert!(r.contains("100\tline-100"));
        assert!(!r.contains("101\tline-101"));
        // Page 2: offset is 0-indexed, numbering continues from the file.
        let r2 = run_tool(
            "read_file",
            &json!({"file_path": p, "offset": 100, "limit": 50}),
        )
        .await;
        assert!(
            r2.contains("101\tline-101"),
            "got: {}",
            &r2[..200.min(r2.len())]
        );
        assert!(r2.contains("150\tline-150"));
        assert!(!r2.contains("151\tline-151"));
    }

    #[tokio::test]
    async fn execute_firehose_output_is_capped_not_oom() {
        // A platform-native output firehose must come back clamped, quickly.
        let out = run_tool("execute", &json!({"command": FIREHOSE})).await;
        assert!(out.len() <= MAX_OUTPUT_BYTES + 100, "len: {}", out.len());
        assert!(out.contains("truncated"), "expected truncation marker");
    }

    #[tokio::test]
    async fn edit_requires_unique_match_unless_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt").to_string_lossy().into_owned();
        std::fs::write(&p, "x x x").unwrap();
        let e = run_tool(
            "edit_file",
            &json!({"file_path": p, "old_string": "x", "new_string": "y"}),
        )
        .await;
        assert!(e.contains("3 times"));
        let e2 = run_tool(
            "edit_file",
            &json!({"file_path": p, "old_string": "x", "new_string": "y", "replace_all": true}),
        )
        .await;
        assert!(e2.starts_with("Edited"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "y y y");
    }

    #[tokio::test]
    async fn errors_are_tool_results_not_failures() {
        let out = run_tool("read_file", &json!({"file_path": "/nonexistent/x"})).await;
        assert!(out.starts_with("Error"));
        let out2 = run_tool("nope", &json!({})).await;
        assert!(out2.contains("unknown tool"));
    }

    /// The timeout path kills the whole group (flag on): a background and a
    /// foreground grandchild both die, not only `sh`.
    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_the_process_group_with_the_flag_on() {
        use crate::platform_os::test_procs::*;
        let n = nonce();
        let settings = ShellSettings {
            process_group: true,
            ..Default::default()
        };
        let result = tokio::time::timeout(
            BOUND,
            execute_impl(
                &format!("sleep 60.{n}1 & sleep 60.{n}2"),
                Path::new("."),
                &settings,
                std::time::Duration::from_secs(2),
            ),
        )
        .await
        .expect("the call returns after its timeout");
        let mut failure = result.expect_err("timed out");
        assert_eq!(failure.text(&settings), "Error: command timed out after 2s");
        wait_until("every process of the command gone", || pgrep(&n).is_empty()).await;
    }

    /// Control, flag off (today): the timeout kills `sh` only.
    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_with_the_flag_off_kills_the_direct_child_only() {
        use crate::platform_os::test_procs::*;
        let n = nonce();
        let result = tokio::time::timeout(
            BOUND,
            execute_impl(
                &format!("sleep 60.{n}; echo x"),
                Path::new("."),
                &crate::shell::ShellSettings::default(),
                std::time::Duration::from_secs(2),
            ),
        )
        .await
        .expect("the call returns after its timeout");
        assert!(result.is_err());
        let left = sleepers(&n);
        kill_all(&left);
        assert_eq!(left.len(), 1, "today's behaviour: the grandchild survives");
    }

    #[cfg(unix)]
    const SLEEP_3S: &str = "sleep 3";
    #[cfg(windows)]
    const SLEEP_3S: &str = "Start-Sleep -Seconds 3";

    #[cfg(unix)]
    const PRINT_THEN_HANG: &str = "echo before-the-timeout; echo also-stderr >&2; sleep 60";
    // Keep the timeout fixture's first writes out of PowerShell's output
    // pipeline and avoid an unrelated native ping child. Both ORIGINAL streams
    // must be flushed before the long block; the two-second call limit stays.
    #[cfg(windows)]
    const PRINT_THEN_HANG: &str = concat!(
        "[Console]::Out.WriteLine('before-the-timeout'); [Console]::Out.Flush(); ",
        "[Console]::Error.WriteLine('also-stderr'); [Console]::Error.Flush(); ",
        "[System.Threading.Thread]::Sleep(60000)"
    );

    // TEST-only actual grandchild/PID input. Avoid PowerShell process/output
    // cmdlets without changing the original three-second runner deadline.
    // Child streams stay private to the actual Process, so only its genuine
    // PID line reaches the shell capture before the actual long wait.
    #[cfg(windows)]
    const WINDOWS_TIMED_GRANDCHILD: &str = concat!(
        "$p = [System.Diagnostics.Process]::new(); ",
        "$p.StartInfo.FileName = 'ping.exe'; ",
        "$p.StartInfo.Arguments = '-n 60 127.0.0.1'; ",
        "$p.StartInfo.UseShellExecute = $false; ",
        "$p.StartInfo.CreateNoWindow = $true; ",
        "$p.StartInfo.RedirectStandardOutput = $true; ",
        "$p.StartInfo.RedirectStandardError = $true; ",
        "[void]$p.Start(); ",
        "[Console]::Out.WriteLine('PID=' + $p.Id); [Console]::Out.Flush(); ",
        "$p.WaitForExit()"
    );

    fn runner(shell: Value) -> ShellRunner {
        ShellRunner::new(PathBuf::from("."), ShellSettings::from_value(Some(&shell)))
    }

    /// Exit criterion: a 3 s sleep with a 2 s limit fails with a timeout
    /// error naming the limit, and completes with timeout=10.
    #[tokio::test]
    async fn per_call_timeout_fails_at_the_limit_and_passes_with_a_larger_one() {
        let shell = runner(json!({"timeout_param": true}));
        let short = tokio::time::timeout(BOUND, shell.run(SLEEP_3S, Some(&json!(2))))
            .await
            .unwrap();
        let text = short.expect_err("2 s is too short");
        assert!(text.contains("timed out after 2s"), "{text}");
        // Up to three attempts: on the macOS development host, process
        // launches from a freshly linked test binary were seen to stall for 10
        // to 20 s (every child, pgrep and ps included; the shell existed but
        // had not run `sleep`), which turns this 10 s case into a false
        // failure. One attempt that completes shows the criterion.
        let mut attempts = Vec::new();
        for _ in 0..3 {
            let long = tokio::time::timeout(BOUND, shell.run(SLEEP_3S, Some(&json!(10))))
                .await
                .unwrap();
            if long == Ok(String::new()) {
                return;
            }
            attempts.push(long);
        }
        panic!("sleep 3 never completed within timeout=10: {attempts:?}");
    }

    /// Legacy format (the default): the timeout text is today's, naming the
    /// limit, and the output is discarded.
    #[tokio::test]
    async fn legacy_timeout_text_is_todays() {
        let shell = runner(json!({"timeout_secs": 2}));
        let text = tokio::time::timeout(BOUND, shell.run(PRINT_THEN_HANG, None))
            .await
            .unwrap()
            .expect_err("times out");
        assert_eq!(
            text,
            "tool 'execute' error: Error: command timed out after 2s"
        );
    }

    /// Head-and-tail format: the output printed before the timeout comes
    /// back with the error, stdout and stderr both.
    #[tokio::test]
    async fn head_tail_timeout_returns_the_partial_output() {
        let shell = runner(json!({"timeout_secs": 2, "output_format": "head_tail"}));
        let text = tokio::time::timeout(BOUND, shell.run(PRINT_THEN_HANG, None))
            .await
            .unwrap()
            .expect_err("times out");
        assert!(
            text.starts_with("Error: command timed out after 2s and was stopped."),
            "{text}"
        );
        assert!(text.contains("before-the-timeout"), "{text}");
        assert!(text.contains("--- stderr ---\nalso-stderr"), "{text}");
    }

    /// Exit criterion (Unix with the group on): a timeout kills grandchildren
    /// and the output printed before it is returned.
    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_grandchildren_and_returns_the_partial_output() {
        use crate::platform_os::test_procs::*;
        let n = nonce();
        let shell = runner(json!({
            "process_group": true,
            "timeout_param": true,
            "output_format": "head_tail"
        }));
        let cmd = format!("echo printed-first; sleep 60.{n}1 & sleep 60.{n}2");
        let text = tokio::time::timeout(BOUND, shell.run(&cmd, Some(&json!(2))))
            .await
            .unwrap()
            .expect_err("times out");
        assert!(text.contains("timed out after 2s"), "{text}");
        assert!(
            text.contains("pass a larger `timeout` (at most 600)"),
            "{text}"
        );
        assert!(text.contains("printed-first"), "{text}");
        wait_until("every process of the command gone", || pgrep(&n).is_empty()).await;
    }

    /// Windows: the Job Object (on by default) kills the grandchild on
    /// timeout and the partial output survives.
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_timeout_kills_the_job_and_returns_the_partial_output() {
        use crate::platform_os::test_procs::*;
        let shell = runner(json!({"timeout_secs": 3, "output_format": "head_tail"}));
        let cmd = WINDOWS_TIMED_GRANDCHILD;
        let text = tokio::time::timeout(BOUND, shell.run(cmd, None))
            .await
            .unwrap()
            .expect_err("times out");
        assert!(text.contains("timed out after 3s"), "{text}");
        let pid: u32 = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("PID="))
            .expect("the PID line was printed before the timeout")
            .parse()
            .unwrap();
        wait_until("the ping grandchild gone", || !alive(pid)).await;
    }

    #[cfg(unix)]
    const BIG_OUTPUT: &str = "printf 'HEAD-MARK\\n'; head -c 200000 /dev/zero | tr '\\0' x; printf '\\nTAIL-MARK\\n'; echo oops >&2; exit 5";
    #[cfg(windows)]
    const BIG_OUTPUT: &str = "Write-Output 'HEAD-MARK'; [Console]::Out.Write('x' * 200000); Write-Output ''; Write-Output 'TAIL-MARK'; [Console]::Error.WriteLine('oops'); exit 5";

    /// Exit criterion: 200 kB of output keeps head and tail and names the
    /// spill file; stderr and the exit code survive.
    #[tokio::test]
    async fn head_tail_keeps_head_tail_stderr_exit_code_and_names_the_spill_file() {
        let shell = runner(json!({"output_format": "head_tail"}));
        let text = tokio::time::timeout(BOUND, shell.run(BIG_OUTPUT, None))
            .await
            .unwrap()
            .expect("a normal result");
        assert!(text.starts_with("HEAD-MARK"), "{}", &text[..100]);
        assert!(text.contains("TAIL-MARK"), "tail kept");
        assert!(text.contains("\n--- stderr ---\noops"), "stderr kept");
        assert!(text.ends_with("[exit code 5]"), "exit code kept");
        assert!(text.len() < MAX_OUTPUT_BYTES + 1_000, "len {}", text.len());
        let path = text
            .split("the full stdout is in ")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("the spill file is named");
        let full = std::fs::read_to_string(path).unwrap();
        assert!(full.starts_with("HEAD-MARK") && full.contains("TAIL-MARK"));
        assert!(full.len() > 200_000);
    }

    /// Legacy (default): the same command is cut from the front as today,
    /// losing the tail, stderr and the exit code.
    #[tokio::test]
    async fn legacy_truncation_is_todays() {
        let shell = runner(json!({}));
        let text = tokio::time::timeout(BOUND, shell.run(BIG_OUTPUT, None))
            .await
            .unwrap()
            .expect("a normal result");
        assert!(text.ends_with("\n… [output truncated]"));
        assert!(!text.contains("TAIL-MARK") && !text.contains("[exit code"));
        assert_eq!(
            text.len(),
            MAX_OUTPUT_BYTES + "\n… [output truncated]".len()
        );
    }

    /// The meta of an execute outcome: exit code, duration, bytes,
    /// truncated and the error kind (never part of the content).
    #[tokio::test]
    async fn execute_outcomes_carry_meta() {
        let shell = runner(json!({}));
        let outcome = shell.run_outcome(STDOUT_STDERR_EXIT, None).await;
        assert!(!outcome.is_error);
        assert_eq!(outcome.meta.exit_code, Some(3));
        assert!(outcome.meta.duration.is_some());
        assert!(outcome.meta.bytes.unwrap() >= 6, "{:?}", outcome.meta);
        assert!(!outcome.meta.truncated);
        assert_eq!(outcome.meta.error_kind, None);

        let big = shell.run_outcome(BIG_OUTPUT, None).await;
        assert!(big.meta.truncated);
        // Legacy reads at most 100,000 bytes of a pipe: bytes counts what was read.
        assert!(big.meta.bytes.unwrap() >= 100_000);

        let shell = runner(json!({"timeout_secs": 1}));
        let timed_out = shell.run_outcome(SLEEP_3S, None).await;
        assert!(timed_out.is_error);
        assert_eq!(
            timed_out.meta.error_kind,
            Some(craftsman_harness::ToolErrorKind::Timeout)
        );
    }

    /// `shell.clean_output`: colour codes and progress-bar overwrites are
    /// resolved; off (default) they reach the model as today.
    #[cfg(unix)]
    #[tokio::test]
    async fn clean_output_flag_resolves_escapes_and_carriage_returns() {
        let cmd = "printf '\\033[31mred\\033[0m 10%%\\r100%%\\n'";
        let on = runner(json!({"clean_output": true}));
        // What a terminal shows: "100%" overwrites the start of "red 10%".
        assert_eq!(on.run(cmd, None).await, Ok("100%10%\n".to_string()));
        let off = runner(json!({}));
        assert_eq!(
            off.run(cmd, None).await,
            Ok("\u{1b}[31mred\u{1b}[0m 10%\r100%\n".to_string())
        );
    }

    #[test]
    fn execute_schema_is_todays_unless_the_timeout_param_is_on() {
        let today = json!({
            "type": "object",
            "properties": {"command": {"type": "string", "description": "Shell command to run"}},
            "required": ["command"]
        });
        assert_eq!(execute_parameters(None), today);
        assert_eq!(execute_parameters(Some(&ShellSettings::default())), today);
        let on = ShellSettings::from_value(Some(&json!({"timeout_param": true})));
        let schema = execute_parameters(Some(&on));
        assert_eq!(schema["properties"]["timeout"]["type"], "integer");
        assert_eq!(
            schema["properties"]["timeout"]["description"],
            "Seconds before the command is stopped (default 120, at most 600)."
        );
        assert_eq!(schema["required"], json!(["command"]));
    }

    #[test]
    fn gated_set_matches_python_interrupt_on() {
        assert_eq!(GATED_TOOLS, &["execute", "write_file", "edit_file"]);
    }

    #[cfg(windows)]
    mod windows_timeout_fixture {
        use super::*;

        #[test]
        fn direct_console_flushes_precede_the_long_sleep() {
            assert_eq!(
                PRINT_THEN_HANG.split("; ").collect::<Vec<_>>(),
                [
                    "[Console]::Out.WriteLine('before-the-timeout')",
                    "[Console]::Out.Flush()",
                    "[Console]::Error.WriteLine('also-stderr')",
                    "[Console]::Error.Flush()",
                    "[System.Threading.Thread]::Sleep(60000)",
                ]
            );
        }

        #[tokio::test]
        async fn observed_bytes_survive_the_real_two_second_timeout_in_both_formats() {
            for format in ["legacy", "head_tail"] {
                let shell = runner(json!({"timeout_secs": 2, "output_format": format}));
                let outcome = tokio::time::timeout(BOUND, shell.run_outcome(PRINT_THEN_HANG, None))
                    .await
                    .expect("the real timed-out call returns within the unchanged outer bound");
                assert!(outcome.is_error, "{}", outcome.content);
                assert_eq!(
                    outcome.meta.error_kind,
                    Some(craftsman_harness::ToolErrorKind::Timeout)
                );
                // Windows Console.WriteLine emits CRLF: 20 stdout + 13 stderr
                // ORIGINAL bytes, even when the legacy view discards the text.
                assert_eq!(outcome.meta.bytes, Some(33), "{}", outcome.content);
                assert_eq!(outcome.meta.exit_code, None);
                assert!(outcome.meta.duration.expect("real duration") >= Duration::from_secs(2));
                assert_eq!(outcome.meta.truncated, format == "legacy");
                if format == "legacy" {
                    assert_eq!(
                        outcome.content,
                        "tool 'execute' error: Error: command timed out after 2s"
                    );
                } else {
                    assert!(
                        outcome
                            .content
                            .starts_with("Error: command timed out after 2s and was stopped."),
                        "{}",
                        outcome.content
                    );
                    assert!(
                        outcome.content.contains("before-the-timeout"),
                        "{}",
                        outcome.content
                    );
                    assert!(
                        outcome.content.contains("--- stderr ---\nalso-stderr"),
                        "{}",
                        outcome.content
                    );
                }
            }
        }

        #[tokio::test]
        async fn a_timeout_before_the_first_write_reports_zero_observed_bytes() {
            // This is deliberately NOT a ready fixture. The runner must not
            // synthesize the later writes, retry the call or report completion.
            let command = format!("[System.Threading.Thread]::Sleep(60000); {PRINT_THEN_HANG}");
            for format in ["legacy", "head_tail"] {
                let shell = runner(json!({"timeout_secs": 2, "output_format": format}));
                let outcome = tokio::time::timeout(BOUND, shell.run_outcome(&command, None))
                    .await
                    .expect("the no-output timeout returns within the unchanged outer bound");
                assert!(outcome.is_error, "{}", outcome.content);
                assert_eq!(
                    outcome.meta.error_kind,
                    Some(craftsman_harness::ToolErrorKind::Timeout)
                );
                assert_eq!(outcome.meta.bytes, Some(0), "{}", outcome.content);
                assert_eq!(outcome.meta.exit_code, None);
                assert!(outcome.meta.duration.expect("real duration") >= Duration::from_secs(2));
                assert!(!outcome.meta.truncated);
                assert_eq!(
                    outcome.content,
                    if format == "legacy" {
                        "tool 'execute' error: Error: command timed out after 2s"
                    } else {
                        "Error: command timed out after 2s and was stopped. It printed nothing before the timeout."
                    }
                );
            }
        }
    }

    #[cfg(windows)]
    mod windows_job_fixture {
        use super::*;

        #[test]
        fn native_child_start_and_actual_pid_flush_precede_wait_without_cmdlets() {
            assert_eq!(
                WINDOWS_TIMED_GRANDCHILD.split("; ").collect::<Vec<_>>(),
                [
                    "$p = [System.Diagnostics.Process]::new()",
                    "$p.StartInfo.FileName = 'ping.exe'",
                    "$p.StartInfo.Arguments = '-n 60 127.0.0.1'",
                    "$p.StartInfo.UseShellExecute = $false",
                    "$p.StartInfo.CreateNoWindow = $true",
                    "$p.StartInfo.RedirectStandardOutput = $true",
                    "$p.StartInfo.RedirectStandardError = $true",
                    "[void]$p.Start()",
                    "[Console]::Out.WriteLine('PID=' + $p.Id)",
                    "[Console]::Out.Flush()",
                    "$p.WaitForExit()",
                ]
            );
            assert!(!WINDOWS_TIMED_GRANDCHILD.contains("Start-Process"));
            assert!(!WINDOWS_TIMED_GRANDCHILD.contains("Write-Output"));
            assert!(!WINDOWS_TIMED_GRANDCHILD.contains("Out-Null"));
        }

        #[tokio::test]
        async fn actual_pid_bytes_timeout_and_job_kill_survive_the_original_three_second_limit() {
            use crate::platform_os::test_procs::*;
            let shell = runner(json!({"timeout_secs": 3, "output_format": "head_tail"}));
            let outcome =
                tokio::time::timeout(BOUND, shell.run_outcome(WINDOWS_TIMED_GRANDCHILD, None))
                    .await
                    .expect("the actual call returns within the original outer bound");
            assert!(outcome.is_error, "{}", outcome.content);
            assert_eq!(
                outcome.meta.error_kind,
                Some(craftsman_harness::ToolErrorKind::Timeout)
            );
            assert_eq!(outcome.meta.exit_code, None);
            assert!(outcome.meta.duration.expect("actual duration") >= Duration::from_secs(3));
            assert!(!outcome.meta.truncated);
            assert!(
                outcome.content.contains("timed out after 3s"),
                "{}",
                outcome.content
            );
            let line = outcome
                .content
                .lines()
                .find(|line| line.trim().starts_with("PID="))
                .expect("an actual original PID must be printed, never synthesized");
            let pid: u32 = line.trim().strip_prefix("PID=").unwrap().parse().unwrap();
            assert!(pid > 0);
            assert_eq!(outcome.meta.bytes, Some((line.trim().len() + 2) as u64));
            wait_until("the actual ping grandchild gone", || !alive(pid)).await;
        }
    }
}
