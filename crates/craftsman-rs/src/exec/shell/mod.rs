//! Mandatory private shell-output adapter selected by the shared builder's
//! opaque scope. Not general tool/provider/storage/output publication or exec.

mod capture;
use crate::agent::ShellScope;
use crate::exec::redaction::Redactor;
use crate::exec::runtime::InvocationGate;
use crate::shell::{OutputFormat, ShellSettings};
use crate::shell_output::{LEGACY_READ_LIMIT, OUTPUT_BUDGET, SPILL_LIMIT};
use capture::{Capture, Fault, View};
use craftsman_harness::{ToolErrorKind, ToolMeta, ToolOutcome};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

pub(crate) struct Shell {
    root: PathBuf,
    settings: ShellSettings,
    redactor: Redactor,
    gate: Arc<InvocationGate>,
}
impl Shell {
    /// Only the actual builder can construct the scope that pairs preparation
    /// and its unique invocation latch. No path/settings/arbitrary-seed fallback.
    pub(crate) fn from_scope(scope: ShellScope<'_>) -> Self {
        let prepared = scope.prepared();
        Self {
            root: prepared.workspace().to_path_buf(),
            settings: prepared.shell().clone(),
            redactor: prepared.credentials().redactor().clone(),
            gate: scope.gate(),
        }
    }
    pub(crate) async fn run(&self, command: &str, timeout: Option<&Value>) -> ToolOutcome {
        let started = Instant::now();
        if let Some(outcome) = self.gate.denied("execute", &json!({"command":command})) {
            return self.publish(outcome.content, true, outcome.meta);
        }
        // Existing terminal cleanup is a whole-string, post-capture operation.
        // Until a bounded streaming cleaner exists, never silently select it.
        if self.settings.clean_output || command.is_empty() {
            return self.fixed(
                ToolErrorKind::InvalidArgs,
                started.elapsed(),
                None,
                false,
                None,
            );
        }
        let limit_secs = self.settings.call_timeout_secs(timeout);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(limit_secs);
        let mut tree = match crate::platform_os::ProcessTree::spawn_shell(
            command,
            &self.root,
            &self.settings.headless_spawn_options(),
        ) {
            Ok(tree) => tree,
            Err(_) => {
                return self.fixed(ToolErrorKind::Spawn, started.elapsed(), None, false, None);
            }
        };
        let (head, tail, read_limit) = match self.settings.output_format {
            OutputFormat::Legacy => (LEGACY_READ_LIMIT as usize, 0, LEGACY_READ_LIMIT),
            OutputFormat::HeadTail => (OUTPUT_BUDGET, OUTPUT_BUDGET, SPILL_LIMIT),
        };
        let out = Arc::new(Mutex::new(Capture::new(&self.redactor, head, tail)));
        let err = Arc::new(Mutex::new(Capture::new(&self.redactor, head, tail)));
        let stdout = tree.take_stdout().expect("ProcessTree stdout is piped");
        let stderr = tree.take_stderr().expect("ProcessTree stderr is piped");
        let readers = async {
            tokio::try_join!(
                pump(stdout, out.clone(), read_limit),
                pump(stderr, err.clone(), read_limit)
            )
        };
        tokio::pin!(readers);
        let mut stop = None;
        let mut readers_done = false;
        let read_ok = tokio::select! { biased;
            _ = self.gate.wait_approval_required() => { stop=Some(ToolErrorKind::NeedsApproval); false },
            _ = tokio::time::sleep_until(deadline) => { stop=Some(ToolErrorKind::Timeout); false },
            result = &mut readers => { readers_done = true; result.is_ok() },
        };
        let mut exit = None;
        if stop.is_none() && read_ok {
            let status = tokio::select! { biased;
                _ = self.gate.wait_approval_required() => { stop=Some(ToolErrorKind::NeedsApproval); None },
                _ = tokio::time::sleep_until(deadline) => { stop=Some(ToolErrorKind::Timeout); None },
                result = tree.wait() => Some(result),
            };
            match status {
                Some(Ok(status)) => exit = Some(status.code().unwrap_or(-1)),
                Some(Err(_)) => stop = Some(ToolErrorKind::Failed),
                None => {}
            }
        } else if stop.is_none() {
            stop = Some(ToolErrorKind::Failed);
        }
        if stop.is_some() {
            // Exactly the existing process-tree kill contract; Unix groups
            // remain configured/default-off and this is not OS containment/P2g.
            tree.kill_tree().await;
            if self.settings.output_format == OutputFormat::HeadTail && !readers_done {
                let _ = tokio::time::timeout(Duration::from_secs(2), &mut readers).await;
            }
        }
        let normal = stop.is_none();
        let stdout = out.lock().unwrap().finish(normal);
        let stderr = err.lock().unwrap().finish(normal);
        let bytes = stdout.raw_bytes + stderr.raw_bytes;
        let mut meta = ToolMeta::new();
        meta.exit_code = exit;
        meta.duration = Some(started.elapsed());
        meta.bytes = Some(bytes);
        meta.truncated = stdout.omitted || stderr.omitted;
        if self.gate.approval_required() {
            stop = Some(ToolErrorKind::NeedsApproval);
        }
        meta.error_kind = stop;
        if matches!(
            stop,
            Some(ToolErrorKind::NeedsApproval | ToolErrorKind::Failed)
        ) {
            return self.publish("[REDACTED]".into(), true, meta);
        }
        if stdout.fault.is_some() || stderr.fault.is_some() {
            if meta.error_kind.is_none() {
                meta.error_kind = Some(ToolErrorKind::Failed);
            }
            meta.truncated = true;
            return self.publish("[REDACTED]".into(), true, meta);
        }
        if stop == Some(ToolErrorKind::Timeout)
            && self.settings.output_format == OutputFormat::Legacy
        {
            meta.truncated |= bytes > 0;
            return self.publish(
                format!("tool 'execute' error: Error: command timed out after {limit_secs}s"),
                true,
                meta,
            );
        }
        let formatted = self.format(&stdout, &stderr, exit, stop.map(|_| limit_secs));
        match formatted {
            Ok((text, cut)) => {
                meta.truncated |= cut;
                self.publish(text, stop.is_some(), meta)
            }
            Err(_) => {
                if meta.error_kind.is_none() {
                    meta.error_kind = Some(ToolErrorKind::Failed);
                }
                meta.truncated = true;
                self.publish("[REDACTED]".into(), true, meta)
            }
        }
    }
    fn format(
        &self,
        out: &View,
        err: &View,
        exit: Option<i32>,
        timeout: Option<u64>,
    ) -> Result<(String, bool), Fault> {
        let has_error = err.rendered_bytes > 0;
        let (stdout, stderr, mut cut) = match self.settings.output_format {
            OutputFormat::Legacy => (
                out.head_text()?,
                err.head_text()?,
                !out.complete() || !err.complete(),
            ),
            OutputFormat::HeadTail => {
                let budget = OUTPUT_BUDGET - 2048;
                let error_budget = if has_error {
                    (budget / 5)
                        .max(budget.saturating_sub(out.rendered_bytes as usize))
                        .min(err.rendered_bytes as usize)
                } else {
                    0
                };
                let (stdout, out_cut) = out.body(budget - error_budget, "stdout")?;
                let (stderr, err_cut) = if has_error {
                    err.body(error_budget, "stderr")?
                } else {
                    (String::new(), false)
                };
                (stdout, stderr, out_cut || err_cut)
            }
        };
        let mut text = timeout.map(|s|format!("Error: command timed out after {s}s and was stopped. Output printed before the timeout:\n")).unwrap_or_default();
        text.push_str(&stdout);
        if has_error {
            text.push_str("\n--- stderr ---\n");
            text.push_str(&stderr);
        }
        if let Some(code) = exit.filter(|c| *c != 0) {
            text.push_str(&format!("\n[exit code {code}]"));
        }
        // Frame/number/section boundaries can create a literal value. Redact
        // them BEFORE any final clamp, then verify the exact published text.
        text = self.redactor.text(&text).map_err(|_| Fault::Redaction)?;
        let mut check = self.redactor.byte_stream();
        for chunk in text.as_bytes().chunks(64 * 1024) {
            check.push(chunk).map_err(|_| Fault::Redaction)?;
            if check.matched() {
                return Err(Fault::Rendered);
            }
        }
        if text.len() > OUTPUT_BUDGET {
            if self.settings.output_format == OutputFormat::HeadTail {
                return Err(Fault::Redaction);
            }
            let mut end = OUTPUT_BUDGET - 64;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str("\n… [output truncated]");
            cut = true;
        }
        Ok((text, cut))
    }
    fn fixed(
        &self,
        kind: ToolErrorKind,
        duration: Duration,
        bytes: Option<u64>,
        truncated: bool,
        exit: Option<i32>,
    ) -> ToolOutcome {
        let mut meta = ToolMeta::new();
        meta.error_kind = Some(kind);
        meta.duration = Some(duration);
        meta.bytes = bytes;
        meta.truncated = truncated;
        meta.exit_code = exit;
        self.publish("[REDACTED]".into(), true, meta)
    }
    fn publish(&self, text: String, mut is_error: bool, mut meta: ToolMeta) -> ToolOutcome {
        let mut safe = self
            .redactor
            .text(&text)
            .ok()
            .filter(|s| s.len() <= OUTPUT_BUDGET);
        if let Some(text) = safe.as_ref() {
            let mut check = self.redactor.byte_stream();
            for chunk in text.as_bytes().chunks(64 * 1024) {
                if check.push(chunk).is_err() || check.matched() {
                    safe = None;
                    break;
                }
            }
        }
        if self.gate.approval_required() {
            meta.error_kind = Some(ToolErrorKind::NeedsApproval);
            is_error = true;
            safe = None;
        }
        if safe.is_none() {
            if meta.error_kind.is_none() {
                meta.error_kind = Some(ToolErrorKind::OutputLimit);
            }
            meta.truncated = true;
            is_error = true;
        }
        let text = safe.unwrap_or_else(|| "[REDACTED]".into());
        if is_error {
            ToolOutcome::err(text).with_meta(meta)
        } else {
            ToolOutcome::ok(text).with_meta(meta)
        }
    }
}

async fn pump(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    capture: Arc<Mutex<Capture>>,
    limit: u64,
) -> Result<(), ()> {
    let mut chunk = [0; 8192];
    let mut read = 0u64;
    while read < limit {
        let count = chunk.len().min((limit - read) as usize);
        match pipe.read(&mut chunk[..count]).await {
            Ok(0) => {
                capture.lock().unwrap().eof();
                return Ok(());
            }
            Ok(n) => {
                read += n as u64;
                if capture.lock().unwrap().push(&chunk[..n]).is_err() {
                    return Err(());
                }
            }
            Err(_) => {
                capture.lock().unwrap().read_failed();
                return Err(());
            }
        }
    }
    Ok(()) // Read cap is NOT natural EOF; finish discards both delayed suffixes.
}

#[cfg(test)]
mod tests;
