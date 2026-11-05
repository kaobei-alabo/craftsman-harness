//! Private filesystem views selected by the actual shared-builder scope.
//! ORIGINAL paths/queries/mutations use the retained ConfinedFs. Only publication
//! selects captured seeds, before page/line/name/framing/output cuts. Not exec,
//! guest registration, provider/state/storage/trace/public result production.

mod output;
use super::{
    ConfinedFs, FsError, Inner, MAX_FILE_BYTES, MAX_MATCH_WORK, MAX_OUTPUT_BYTES, MAX_SEARCH_BYTES,
    checked_path,
};
use crate::agent::FilesystemScope;
use crate::exec::redaction::{RedactionError, Redactor};
use crate::exec::runtime::InvocationGate;
use async_trait::async_trait;
use craftsman_harness::backend::{ExecOutcome, glob_match};
use craftsman_harness::{
    AgentError, Backend, FileInfo, Tool, ToolCtx, ToolDef, ToolErrorKind, ToolMeta, ToolOutcome,
};
use output::{Fault, Output};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

type Result<T> = std::result::Result<T, Failure>;

struct Failure {
    cause: Cause,
    bytes: Option<u64>,
}
enum Cause {
    Fs(FsError),
    Privacy(Fault),
    Policy(ToolErrorKind),
}
impl Failure {
    fn fs(error: FsError) -> Self {
        Self {
            cause: Cause::Fs(error),
            bytes: None,
        }
    }
    fn privacy(error: Fault, bytes: Option<u64>) -> Self {
        Self {
            cause: Cause::Privacy(error),
            bytes,
        }
    }
    fn observed(mut self, bytes: u64) -> Self {
        self.bytes = Some(bytes);
        self
    }
    fn kind(&self) -> ToolErrorKind {
        match self.cause {
            Cause::Policy(kind) => kind,
            Cause::Privacy(Fault::Limit) => ToolErrorKind::OutputLimit,
            Cause::Privacy(_) => ToolErrorKind::Failed,
            Cause::Fs(error) => match error {
                FsError::Unapproved | FsError::ProtectedWrite => ToolErrorKind::NeedsApproval,
                FsError::NotFound => ToolErrorKind::NotFound,
                FsError::FileTooLarge | FsError::InputTooLarge | FsError::OutputLimit => {
                    ToolErrorKind::OutputLimit
                }
                FsError::OutsideRoot
                | FsError::ToolDenied
                | FsError::PathDenied
                | FsError::HardDenied
                | FsError::NotFile
                | FsError::Symlink
                | FsError::HardLink
                | FsError::Exists => ToolErrorKind::DeniedCapability,
                FsError::InvalidScope
                | FsError::InvalidPath
                | FsError::InvalidSelector
                | FsError::NotDirectory
                | FsError::InvalidUtf8
                | FsError::Offset
                | FsError::EmptyEdit
                | FsError::NoMatch
                | FsError::NonUnique => ToolErrorKind::InvalidArgs,
                FsError::Changed | FsError::Io | FsError::Worker => ToolErrorKind::Failed,
            },
        }
    }
    fn message(&self) -> String {
        match self.cause {
            Cause::Fs(error) => error.to_string(),
            Cause::Policy(ToolErrorKind::NeedsApproval) => {
                "This filesystem operation requires an explicit invocation allow.".into()
            }
            Cause::Policy(_) => {
                "This filesystem operation is denied by the invocation policy.".into()
            }
            Cause::Privacy(_) => "[REDACTED]".into(),
        }
    }
    fn truncated(&self) -> bool {
        matches!(
            self.cause,
            Cause::Privacy(_) | Cause::Fs(FsError::FileTooLarge | FsError::OutputLimit)
        )
    }
}
impl From<FsError> for Failure {
    fn from(value: FsError) -> Self {
        Self::fs(value)
    }
}
impl From<RedactionError> for Failure {
    fn from(value: RedactionError) -> Self {
        Self::privacy(
            if value == RedactionError::Record {
                Fault::Limit
            } else {
                Fault::Redaction
            },
            None,
        )
    }
}
impl From<Fault> for Failure {
    fn from(value: Fault) -> Self {
        Self::privacy(value, None)
    }
}

struct Record {
    view: output::View,
    bytes: Option<u64>,
}
struct Names {
    entries: Vec<FileInfo>,
    bytes: u64,
}

pub(crate) struct TextFs {
    filesystem: ConfinedFs,
    redactor: Redactor,
    gate: Arc<InvocationGate>,
}
impl TextFs {
    /// No arbitrary path/filesystem/gate/seed constructor or ambient fallback.
    pub(crate) fn from_scope(scope: FilesystemScope<'_>) -> Self {
        Self {
            filesystem: scope.prepared().filesystem().clone(),
            redactor: scope.prepared().credentials().redactor().clone(),
            gate: scope.gate(),
        }
    }
    pub(crate) fn wrap_host_tool(self: &Arc<Self>, original: Arc<dyn Tool>) -> Arc<dyn Tool> {
        let def = original.def();
        if matches!(
            def.name.as_str(),
            "read_file" | "write_file" | "edit_file" | "ls" | "glob" | "grep"
        ) {
            Arc::new(ScopedTool {
                def,
                filesystem: self.clone(),
            })
        } else {
            original
        }
    }
    fn precheck(&self, name: &str, args: &Value) -> Result<()> {
        match self.gate.denied(name, args) {
            Some(outcome) => Err(Failure {
                cause: Cause::Policy(
                    outcome
                        .meta
                        .error_kind
                        .unwrap_or(ToolErrorKind::DeniedCapability),
                ),
                bytes: None,
            }),
            None => Ok(()),
        }
    }
    async fn worker<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Inner) -> Result<T> + Send + 'static,
    {
        let gate = self.gate.clone();
        self.filesystem
            .run_typed(move |inner| {
                if gate.approval_required() {
                    return Err(FsError::Unapproved);
                }
                // The original serialized/no-follow worker and error boundaries.
                // Already-started blocking IO is not rollback/OS containment/P2g.
                Ok(operation(inner))
            })
            .await
            .map_err(Failure::fs)?
    }
    fn publication(&self, result: Result<Record>, started: Instant) -> ToolOutcome {
        let mut meta = ToolMeta::new();
        meta.duration = Some(started.elapsed());
        match result {
            Ok(record) => {
                meta.bytes = record.bytes;
                meta.truncated = record.view.truncated;
                if self.gate.approval_required() {
                    meta.error_kind = Some(ToolErrorKind::NeedsApproval);
                    meta.truncated = true; // The already-produced view is withheld.
                    return ToolOutcome::err("[REDACTED]").with_meta(meta);
                }
                ToolOutcome::ok(record.view.text).with_meta(meta)
            }
            Err(failure) => {
                let kind = failure.kind();
                self.gate.observe_error_kind(kind); // Typed facts BEFORE any formatting.
                meta.bytes = failure.bytes;
                meta.truncated = failure.truncated();
                meta.error_kind = Some(kind);
                if self.gate.approval_required() {
                    meta.error_kind = Some(ToolErrorKind::NeedsApproval);
                    return ToolOutcome::err("[REDACTED]").with_meta(meta);
                }
                let mut out = Output::new(&self.redactor);
                let safe = out
                    .frame(&failure.message())
                    .and_then(|complete| out.finish(complete));
                let text = match safe {
                    Ok(view) => {
                        meta.truncated |= view.truncated;
                        view.text
                    }
                    Err(_) => {
                        meta.truncated = true;
                        "[REDACTED]".into()
                    }
                };
                ToolOutcome::err(text).with_meta(meta)
            }
        }
    }
    fn backend_error(&self, error: Failure) -> AgentError {
        // Even callers catching an AgentError cannot hide a typed approval failure.
        let outcome = self.publication(Err(error), Instant::now());
        AgentError::State(outcome.content)
    }
    fn text_result(&self, result: Result<Record>) -> std::result::Result<String, AgentError> {
        let outcome = self.publication(result, Instant::now());
        if outcome.is_error {
            Err(AgentError::State(outcome.content))
        } else {
            Ok(outcome.content)
        }
    }
    fn unit_result(&self, result: Result<()>) -> std::result::Result<(), AgentError> {
        match result {
            Err(error) => Err(self.backend_error(error)),
            Ok(()) if self.gate.approval_required() => {
                Err(self.backend_error(Failure::fs(FsError::Unapproved)))
            }
            Ok(()) => Ok(()),
        }
    }
    pub(crate) async fn call(&self, name: &str, args: Value) -> ToolOutcome {
        let started = Instant::now();
        let result = self.operation(name, &args).await;
        self.publication(result, started)
    }
    async fn operation(&self, name: &str, args: &Value) -> Result<Record> {
        match name {
            "read_file" => {
                self.read_record(
                    args["file_path"].as_str().unwrap_or_default(),
                    args["offset"].as_u64().unwrap_or(0) as usize,
                    args["limit"].as_u64().unwrap_or(100).max(1) as usize,
                )
                .await
            }
            "write_file" => {
                let path = args["file_path"].as_str().unwrap_or_default();
                self.write_original(path, args["content"].as_str().unwrap_or_default())
                    .await?;
                self.summary(path)
            }
            "edit_file" => {
                let path = args["file_path"].as_str().unwrap_or_default();
                self.edit_original(
                    path,
                    args["old_string"].as_str().unwrap_or_default(),
                    args["new_string"].as_str().unwrap_or_default(),
                    args["replace_all"].as_bool().unwrap_or(false),
                )
                .await?;
                self.summary(path)
            }
            "ls" => {
                let names = self
                    .names("ls", args["path"].as_str().unwrap_or("."))
                    .await?;
                self.names_record(names, None)
            }
            "glob" => {
                let pattern = args["pattern"].as_str().unwrap_or_default();
                let names = self.names("glob", pattern).await?;
                self.names_record(names, Some(pattern))
            }
            "grep" => {
                self.grep_record(
                    args["pattern"].as_str().unwrap_or_default(),
                    args["path"].as_str(),
                )
                .await
            }
            _ => Err(Failure {
                cause: Cause::Policy(ToolErrorKind::DeniedCapability),
                bytes: None,
            }),
        }
    }
    fn summary(&self, original_path: &str) -> Result<Record> {
        let mut output = Output::new(&self.redactor);
        let complete = output.frame("Updated file ")? && output.frame(original_path)?;
        Ok(Record {
            view: output.finish(complete)?,
            bytes: None,
        })
    }
    async fn read_record(&self, path: &str, offset: usize, limit: usize) -> Result<Record> {
        self.precheck(
            "read_file",
            &json!({"file_path":path,"offset":offset,"limit":limit}),
        )?;
        let path = checked_path(path)?;
        let redactor = self.redactor.clone();
        self.worker(move |inner| {
            let parts = inner.relative(&path, false)?;
            inner.authorize("read_file", &parts, false)?;
            let bytes = inner.read_bytes(&parts)?;
            let observed = bytes.len() as u64;
            (|| {
                let text = std::str::from_utf8(&bytes).map_err(|_| FsError::InvalidUtf8)?;
                if !text.is_empty() && offset >= text.lines().count() {
                    return Err(FsError::Offset.into());
                }
                // COMPLETE original-file coverage, not coverage of a selected page.
                let source = redactor.masked_text(text)?;
                let mut output = Output::new(&redactor);
                let mut complete = true;
                if text.is_empty() {
                    complete = output.frame(craftsman_harness::backend::EMPTY_FILE_REMINDER)?;
                } else {
                    for (index, line) in source.lines().skip(offset).take(limit).enumerate() {
                        if (index > 0 && !output.frame("\n")?)
                            || !output.frame(&format!("{:>6}\t", line.number()))?
                            || !body(&redactor, &line, &mut output)?
                        {
                            complete = false;
                            break;
                        }
                    }
                }
                Ok(Record {
                    view: output.finish(complete)?,
                    bytes: Some(observed),
                })
            })()
            .map_err(|error: Failure| error.observed(observed))
        })
        .await
    }
    async fn write_original(&self, path: &str, content: &str) -> Result<()> {
        self.precheck("write_file", &json!({"file_path":path}))?;
        let path = checked_path(path)?;
        if content.len() > MAX_FILE_BYTES {
            return Err(FsError::FileTooLarge.into());
        }
        let content = content.to_owned();
        self.worker(move |inner| {
            let parts = inner.relative(&path, false)?;
            inner.authorize("write_file", &parts, true)?;
            inner.write_file(&parts, content.as_bytes())?;
            Ok(())
        })
        .await
    }
    async fn edit_original(
        &self,
        path: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> Result<()> {
        self.precheck("edit_file", &json!({"file_path":path}))?;
        let path = checked_path(path)?;
        if old.len() > MAX_FILE_BYTES || new.len() > MAX_FILE_BYTES {
            return Err(FsError::FileTooLarge.into());
        }
        let old = old.to_owned();
        let new = new.to_owned();
        self.worker(move |inner| {
            let parts = inner.relative(&path, false)?;
            inner.authorize("edit_file", &parts, true)?;
            inner.edit_file(&parts, &old, &new, replace_all)?;
            Ok(())
        })
        .await
    }
    async fn names(&self, tool: &'static str, value: &str) -> Result<Names> {
        self.precheck(
            tool,
            &if tool == "ls" {
                json!({"path":value})
            } else {
                json!({"pattern":value})
            },
        )?;
        if value.len() > MAX_OUTPUT_BYTES {
            return Err(FsError::OutputLimit.into());
        }
        let value = if tool == "ls" {
            checked_path(value)?
        } else {
            value.to_owned()
        };
        let redactor = self.redactor.clone();
        self.worker(move |inner| {
            let entries = if tool == "ls" {
                let parts = inner.relative(&value, true)?;
                inner.authorize("ls", &parts, false)?;
                inner.list(&parts)?
            } else {
                if value.len() > 256
                    || value.starts_with('/')
                    || value.contains(['\\', ':'])
                    || value.split('/').any(|p| p == "..")
                    || value.chars().any(char::is_control)
                {
                    return Err(FsError::InvalidPath.into());
                }
                inner.authorize("glob", &[], false)?;
                let mut work = 0usize;
                let mut bytes = 0usize;
                let mut entries = Vec::new();
                for parts in inner.walk_files(&[])? {
                    let path = parts.join("/");
                    work = work
                        .saturating_add(value.chars().count().saturating_mul(path.chars().count()));
                    if work > MAX_MATCH_WORK {
                        return Err(FsError::OutputLimit.into());
                    }
                    // Match ORIGINAL names, never a redacted alias or query.
                    if glob_match(&value, &path) {
                        bytes = bytes.saturating_add(path.len());
                        if bytes > MAX_OUTPUT_BYTES {
                            return Err(FsError::OutputLimit.into());
                        }
                        entries.push(FileInfo { path, size: 0 });
                    }
                }
                entries
            };
            let observed = entries.iter().map(|entry| entry.path.len() as u64).sum();
            let mut seen = BTreeSet::new();
            let mut safe = Vec::new();
            for entry in entries {
                let path = output::name(&redactor, &entry.path)
                    .map_err(|error| Failure::privacy(error, Some(observed)))?;
                if !seen.insert(path.clone()) {
                    return Err(Failure::privacy(Fault::Rendered, Some(observed)));
                }
                // Retain ORIGINAL ordering and size. Paths are view labels, not
                // authority aliases; there is never a reverse substitution.
                safe.push(FileInfo {
                    path,
                    size: entry.size,
                });
            }
            Ok(Names {
                entries: safe,
                bytes: observed,
            })
        })
        .await
    }
    fn names_record(&self, names: Names, pattern: Option<&str>) -> Result<Record> {
        let mut output = Output::new(&self.redactor);
        let mut complete = true;
        if names.entries.is_empty() {
            if let Some(pattern) = pattern {
                complete = output.frame("No files match pattern '")?
                    && output.frame(pattern)?
                    && output.frame("'")?;
            }
        } else {
            for (index, entry) in names.entries.iter().enumerate() {
                if (index > 0 && !output.frame("\n")?) || !output.frame(&entry.path)? {
                    complete = false;
                    break;
                }
            }
        }
        output
            .finish(complete)
            .map(|view| Record {
                view,
                bytes: Some(names.bytes),
            })
            .map_err(|error| Failure::privacy(error, Some(names.bytes)))
    }
    async fn grep_record(&self, pattern: &str, path: Option<&str>) -> Result<Record> {
        self.precheck("grep", &json!({"pattern":pattern,"path":path}))?;
        if pattern.len() > MAX_OUTPUT_BYTES {
            return Err(FsError::OutputLimit.into());
        }
        let pattern = pattern.to_owned();
        let path = checked_path(path.unwrap_or(""))?;
        let redactor = self.redactor.clone();
        self.worker(move |inner| {
            let parts = inner.relative(&path, true)?;
            inner.authorize("grep", &parts, false)?;
            let files = if !parts.is_empty() {
                let parent = inner.parent(&parts, false)?;
                let metadata = parent
                    .directory
                    .symlink_metadata(parts.last().unwrap())
                    .map_err(super::io)?;
                if metadata.is_file() && !metadata.file_type().is_symlink() {
                    vec![parts.clone()]
                } else {
                    inner.walk_files(&parts)?
                }
            } else {
                inner.walk_files(&parts)?
            };
            let direct = files.len() == 1 && files[0] == parts && !parts.is_empty();
            let mut searched = 0u64;
            let mut output = Output::new(&redactor);
            let mut matched = false;
            let mut complete = true;
            let mut labels = BTreeMap::new();
            (|| {
                'files: for file in files {
                    let bytes = match inner.read_bytes(&file) {
                        Ok(bytes) => bytes,
                        Err(error) if direct => return Err(Failure::fs(error)),
                        Err(_) => continue,
                    };
                    searched += bytes.len() as u64;
                    if searched > MAX_SEARCH_BYTES as u64 {
                        return Err(FsError::OutputLimit.into());
                    }
                    // Keep established binary/UTF-8 skip behavior and charge ALL
                    // successfully read bytes, including nonmatching/binary files.
                    if bytes.iter().take(8192).any(|b| *b == 0) {
                        continue;
                    }
                    let Ok(text) = std::str::from_utf8(&bytes) else {
                        continue;
                    };
                    let source = redactor.masked_text(text)?;
                    let original_path = file.join("/");
                    let mut label = None;
                    for line in source.lines() {
                        if !line.matches(&pattern) {
                            continue;
                        }
                        matched = true;
                        if label.is_none() {
                            let safe = output::name(&redactor, &original_path)?;
                            if labels
                                .get(&safe)
                                .is_some_and(|original| original != &original_path)
                            {
                                return Err(Failure::privacy(Fault::Rendered, Some(searched)));
                            }
                            labels.insert(safe.clone(), original_path.clone());
                            label = Some(safe);
                        }
                        if !output.frame(label.as_ref().unwrap())?
                            || !output.frame(&format!(":{}:", line.number()))?
                            || !body(&redactor, &line, &mut output)?
                            || !output.frame("\n")?
                        {
                            complete = false;
                            break 'files;
                        }
                    }
                }
                if !matched {
                    complete = output.frame("No matches found for '")?
                        && output.frame(&pattern)?
                        && output.frame("'")?;
                }
                Ok(Record {
                    view: output.finish(complete)?,
                    bytes: Some(searched),
                })
            })()
            .map_err(|error: Failure| error.observed(searched))
        })
        .await
    }
}

/// Body replacement collisions are rejected BEFORE framing or a head cut. An
/// output cap aborts this guard too; only the actual original line end finishes.
fn body(
    redactor: &Redactor,
    line: &crate::exec::redaction::MaskedLine<'_, '_>,
    output: &mut Output,
) -> std::result::Result<bool, Fault> {
    let mut check = redactor.byte_stream();
    for piece in line.chunks() {
        let piece = piece.map_err(|_| Fault::Utf8)?;
        let safe = check.push(piece.as_bytes()).map_err(|_| Fault::Redaction)?;
        if check.matched() {
            return Err(Fault::Rendered);
        }
        if !output.push(&safe)? {
            check.abort();
            return Ok(false);
        }
    }
    let safe = check.finish().map_err(|_| Fault::Redaction)?;
    output.push(&safe)
}

struct ScopedTool {
    def: ToolDef,
    filesystem: Arc<TextFs>,
}
#[async_trait]
impl Tool for ScopedTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }
    async fn call(
        &self,
        args: Value,
        _: &mut ToolCtx<'_>,
    ) -> std::result::Result<ToolOutcome, AgentError> {
        Ok(self.filesystem.call(&self.def.name, args).await)
    }
}

#[async_trait]
impl Backend for TextFs {
    async fn read(
        &self,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> std::result::Result<String, AgentError> {
        self.text_result(self.read_record(path, offset, limit).await)
    }
    async fn write(&self, path: &str, content: &str) -> std::result::Result<(), AgentError> {
        self.unit_result(self.write_original(path, content).await)
    }
    async fn edit(
        &self,
        path: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> std::result::Result<(), AgentError> {
        self.unit_result(self.edit_original(path, old, new, replace_all).await)
    }
    async fn ls(&self, path: &str) -> std::result::Result<Vec<FileInfo>, AgentError> {
        let names = self
            .names("ls", path)
            .await
            .map_err(|error| self.backend_error(error))?;
        self.array_bound(&names)?;
        Ok(names.entries)
    }
    async fn glob(&self, pattern: &str) -> std::result::Result<Vec<String>, AgentError> {
        let names = self
            .names("glob", pattern)
            .await
            .map_err(|error| self.backend_error(error))?;
        self.array_bound(&names)?;
        Ok(names.entries.into_iter().map(|entry| entry.path).collect())
    }
    async fn grep(
        &self,
        pattern: &str,
        path: Option<&str>,
    ) -> std::result::Result<String, AgentError> {
        self.text_result(self.grep_record(pattern, path).await)
    }
    async fn execute(&self, _: &str) -> std::result::Result<ExecOutcome, AgentError> {
        Err(self.backend_error(Failure {
            cause: Cause::Policy(ToolErrorKind::DeniedCapability),
            bytes: None,
        }))
    }
}
impl TextFs {
    fn array_bound(&self, names: &Names) -> std::result::Result<(), AgentError> {
        let size = names
            .entries
            .iter()
            .map(|entry| entry.path.len())
            .sum::<usize>()
            .saturating_add(names.entries.len().saturating_sub(1));
        if size > output::BUDGET {
            return Err(self.backend_error(Failure::privacy(Fault::Limit, Some(names.bytes))));
        }
        if self.gate.approval_required() {
            return Err(self.backend_error(Failure::fs(FsError::Unapproved)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
