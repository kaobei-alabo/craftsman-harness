//! Default-refusing data-object and retained-effect contracts.
//! Actual canonical derivation issues requests. Neither a path, format,
//! purpose, pointer nor Boolean constructs a validated data/effect grant.

use super::json_ranges::{JsonRanges, PathStep, StringRole};
use super::view::PlannedOperation;
use super::{FileChange, FileRefusal, Inner};
use crate::privacy_restore::FileDataFormat;
use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use zeroize::Zeroizing;

/// The value of `$result`, or for any error a return of `$error`. A plain
/// match where an adapter call with a closure stood: the same work in the
/// same order, without a function of its own for each place.
macro_rules! or_return {
    ($result:expr, $error:expr) => {
        match $result {
            Ok(value) => value,
            Err(_) => return Err($error),
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportedDataApplication {
    ContactLinesV1,
    ContactJsonV1,
}

/// Actual target of one source-bound planned operation, before private
/// values are read, restored or compared. It has no caller constructor.
#[derive(Clone)]
pub struct FileTargetRequest(Arc<TargetRequest>);
pub(super) struct TargetRequest {
    pub(super) owner: Arc<Inner>,
    pub(super) operation: Arc<PlannedOperation>,
    pub(super) workspace: String,
    pub(super) destination: PathBuf,
    pub(super) change: FileChange,
}

/// Refusal-only target eligibility from the selected actual policy. It
/// grants no preview, restored image, effect or tool permission. All whole
/// image/range and retained-effect records remain separately mandatory.
pub struct FileTargetPrecheck {
    request: Weak<TargetRequest>,
    application: SupportedDataApplication,
}

impl FileTargetRequest {
    pub(super) fn issued(request: TargetRequest) -> Self {
        Self(Arc::new(request))
    }
    pub fn require_session(&self, session: &super::PrivacySession) -> Result<(), FileRefusal> {
        if !Arc::ptr_eq(&self.0.owner, &session.inner) {
            return Err(FileRefusal::Stopped(super::SessionStop::Unbound));
        }
        session.check().map_err(FileRefusal::Stopped)
    }
    pub fn workspace_sha256(&self) -> &str {
        &self.0.workspace
    }
    pub fn destination(&self) -> &Path {
        &self.0.destination
    }
    pub fn operation(&self) -> FileChange {
        self.0.change
    }
    pub fn planned(&self) -> &craftsman_harness::provenance::ActualPlannedInvocation {
        self.0.operation.plan()
    }
    /// Compiled refusal checks only. The actual captured provider must first
    /// validate its source, declared target/application/operation and native
    /// role, and return this record to its selecting consumer. This is not a
    /// supported data image or a write grant.
    pub fn check_application(
        &self,
        application: SupportedDataApplication,
    ) -> Result<FileTargetPrecheck, FileRefusal> {
        self.0.owner.live().map_err(FileRefusal::Stopped)?;
        if forbidden_destination(&self.0.destination) {
            return Err(FileRefusal::Unsupported);
        }
        Ok(FileTargetPrecheck {
            request: Arc::downgrade(&self.0),
            application,
        })
    }
}
impl FileTargetPrecheck {
    pub(super) fn matches(&self, request: &FileTargetRequest) -> bool {
        self.request
            .upgrade()
            .is_some_and(|actual| Arc::ptr_eq(&actual, &request.0))
    }
    pub(super) fn application(&self) -> SupportedDataApplication {
        self.application
    }
    pub(super) fn format(&self) -> FileDataFormat {
        match self.application {
            SupportedDataApplication::ContactLinesV1 => FileDataFormat::PlainText,
            SupportedDataApplication::ContactJsonV1 => FileDataFormat::JsonDocument,
        }
    }
}

#[derive(Clone)]
pub struct FileDataRequest(Arc<DataRequest>);
pub(super) struct DataRequest {
    pub(super) owner: Arc<Inner>,
    pub(super) operation: Arc<PlannedOperation>,
    pub(super) workspace: String,
    pub(super) destination: PathBuf,
    pub(super) change: FileChange,
    pub(super) format: FileDataFormat,
    pub(super) before: Option<Zeroizing<String>>,
    pub(super) after: Zeroizing<String>,
    pub(super) selectors: Vec<Range<usize>>,
    pub(super) replacements: Vec<Range<usize>>,
}
pub struct SupportedFileData {
    request: Weak<DataRequest>,
    application: SupportedDataApplication,
}
pub struct FileEffectRequest {
    pub(super) data: FileDataRequest,
    #[cfg(unix)]
    pub(super) parent_device: u64,
    #[cfg(unix)]
    pub(super) parent_inode: u64,
}
pub struct RetainedFileEffect {
    request: Weak<DataRequest>,
    #[cfg(unix)]
    directory: File,
}

impl FileDataRequest {
    pub(super) fn issued(value: DataRequest) -> Self {
        Self(Arc::new(value))
    }
    /// Bind a policy provider to its own actual conversation, including every
    /// delegated lane of that same Inner. Identity is only a refusal check;
    /// it supplies no application/schema/permission/effect authority.
    pub fn require_session(&self, session: &super::PrivacySession) -> Result<(), FileRefusal> {
        if !Arc::ptr_eq(&self.0.owner, &session.inner) {
            return Err(FileRefusal::Stopped(super::SessionStop::Unbound));
        }
        session.check().map_err(FileRefusal::Stopped)
    }

    pub fn workspace_sha256(&self) -> &str {
        &self.0.workspace
    }
    pub fn destination(&self) -> &Path {
        &self.0.destination
    }
    pub fn operation(&self) -> FileChange {
        self.0.change
    }
    pub fn format(&self) -> FileDataFormat {
        self.0.format
    }
    pub fn planned(&self) -> &craftsman_harness::provenance::ActualPlannedInvocation {
        self.0.operation.plan()
    }
    /// An actual compiled decoder checks the full object/images and restored
    /// ranges. Selection is a proposal; it does not bypass user/root/source,
    /// current native permission, approval or final destination checks.
    pub fn validate_application(
        &self,
        application: SupportedDataApplication,
    ) -> Result<SupportedFileData, FileRefusal> {
        self.0.owner.live().map_err(FileRefusal::Stopped)?;
        if forbidden_destination(&self.0.destination) {
            return Err(FileRefusal::Unsupported);
        }
        let expected = match application {
            SupportedDataApplication::ContactLinesV1 => FileDataFormat::PlainText,
            SupportedDataApplication::ContactJsonV1 => FileDataFormat::JsonDocument,
        };
        if self.0.format != expected {
            return Err(FileRefusal::Unsupported);
        }
        checked_values(application, &self.0.after, &self.0.replacements)?;
        match &self.0.before {
            Some(before) => {
                checked_values(application, before, &self.0.selectors)?;
            }
            None if !self.0.selectors.is_empty() => return Err(FileRefusal::Unsupported),
            None => {}
        }
        self.0.owner.live().map_err(FileRefusal::Stopped)?;
        Ok(SupportedFileData {
            request: Arc::downgrade(&self.0),
            application,
        })
    }
    pub(super) fn after(&self) -> &str {
        &self.0.after
    }
}

/// Internal contact-format filter over borrowed actual before-image bytes.
/// It can only refuse; it creates no source, policy, image or effect record.
/// The selecting consumer must already have the actual target precheck and
/// still obtain the real final image/range/current-policy/effect/decision.
pub(super) struct BeforeFilter<'a> {
    application: SupportedDataApplication,
    text: &'a str,
    values: Vec<Range<usize>>,
}

impl<'a> BeforeFilter<'a> {
    pub(super) fn check(
        application: SupportedDataApplication,
        text: &'a str,
    ) -> Result<Self, FileRefusal> {
        let values = or_return!(checked_values(application, text, &[]), FileRefusal::NoRule);
        Ok(Self {
            application,
            text,
            values,
        })
    }

    pub(super) fn selectors(&self, ranges: &[Range<usize>]) -> Result<(), FileRefusal> {
        if !inside_all(ranges, &self.values) {
            return Err(FileRefusal::NoRule);
        }
        check_json_ranges(self.application, self.text, ranges).map_err(|_| FileRefusal::NoRule)
    }
}

/// The existing compiled whole-object/data-field checks shared by the
/// refusal filter and the final supported-image validator. No grant is
/// issued here, and final-validator errors keep their original meanings.
fn checked_values(
    application: SupportedDataApplication,
    text: &str,
    ranges: &[Range<usize>],
) -> Result<Vec<Range<usize>>, FileRefusal> {
    let values = permitted_values(application, text)?;
    if !inside_all(ranges, &values) {
        return Err(FileRefusal::Unsupported);
    }
    check_json_ranges(application, text, ranges)?;
    Ok(values)
}
impl SupportedFileData {
    pub fn application(&self) -> SupportedDataApplication {
        self.application
    }
    pub(super) fn matches(&self, request: &FileDataRequest) -> bool {
        self.request
            .upgrade()
            .is_some_and(|actual| Arc::ptr_eq(&actual, &request.0))
    }
}
impl FileEffectRequest {
    pub(super) fn issued(
        data: &FileDataRequest,
        parent: &std::fs::Metadata,
    ) -> Result<Self, FileRefusal> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !parent.is_dir() {
                return Err(FileRefusal::Stale);
            }
            Ok(Self {
                data: data.clone(),
                parent_device: parent.dev(),
                parent_inode: parent.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (data, parent);
            Err(FileRefusal::Unsupported)
        }
    }
    pub fn data(&self) -> &FileDataRequest {
        &self.data
    }
    pub fn destination(&self) -> &Path {
        self.data.destination()
    }
    pub fn operation(&self) -> FileChange {
        self.data.operation()
    }
    /// The actual captured-user provider supplies its retained no-follow
    /// directory. Its real identity must equal this operation's observed
    /// parent. No path/FD number/label is accepted as a substitute.
    pub fn bind_retained_parent(&self, parent: &File) -> Result<RetainedFileEffect, FileRefusal> {
        self.data.0.owner.live().map_err(FileRefusal::Stopped)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let owned = or_return!(parent.try_clone(), FileRefusal::Stale);
            let meta = or_return!(owned.metadata(), FileRefusal::Stale);
            if !meta.is_dir() || meta.dev() != self.parent_device || meta.ino() != self.parent_inode
            {
                return Err(FileRefusal::Stale);
            }
            Ok(RetainedFileEffect {
                request: Arc::downgrade(&self.data.0),
                directory: owned,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = parent;
            Err(FileRefusal::Unsupported)
        }
    }
}
impl RetainedFileEffect {
    pub(super) fn matches(&self, request: &FileEffectRequest) -> bool {
        self.request
            .upgrade()
            .is_some_and(|actual| Arc::ptr_eq(&actual, &request.data.0))
    }
    #[cfg(unix)]
    pub(super) fn directory(&self) -> &File {
        &self.directory
    }
}

fn inside_all(ranges: &[Range<usize>], values: &[Range<usize>]) -> bool {
    ranges.iter().all(|range| {
        range.start < range.end
            && values
                .iter()
                .any(|value| value.start <= range.start && range.end <= value.end)
    })
}
fn check_json_ranges(
    application: SupportedDataApplication,
    text: &str,
    ranges: &[Range<usize>],
) -> Result<(), FileRefusal> {
    if application != SupportedDataApplication::ContactJsonV1 {
        return Ok(());
    }
    let indexed = or_return!(
        JsonRanges::index(text.as_bytes()),
        FileRefusal::Restore(crate::privacy_restore::RestoreError::Input)
    );
    for range in ranges {
        let located = or_return!(
            indexed.locate(range.clone()),
            FileRefusal::Restore(crate::privacy_restore::RestoreError::Input)
        );
        match indexed.path(located.string()).as_deref() {
            Some([PathStep::Key("owner" | "note")]) => {}
            _ => return Err(FileRefusal::Unsupported),
        }
    }
    Ok(())
}
fn forbidden_destination(path: &Path) -> bool {
    let names = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    if names.iter().any(|name| {
        matches!(
            name.as_str(),
            ".git"
                | ".craftsman"
                | ".ssh"
                | ".aws"
                | ".config"
                | ".claude"
                | ".codex"
                | ".agents"
                | "node_modules"
        )
    }) {
        return true;
    }
    let Some(name) = names.last() else {
        return true;
    };
    if name.starts_with(".env")
        || name.contains("credential")
        || name.contains("schema")
        || matches!(
            name.as_str(),
            "agents.md"
                | "skill.md"
                | "claude.md"
                | "config.json"
                | "package.json"
                | "manifest.json"
                | "tsconfig.json"
                | "privacy-key.v1"
                | "privacy-vault.db"
                | "privacy-vault.db-wal"
                | "privacy-vault.db-shm"
        )
    {
        return true;
    }
    path.extension().is_some_and(|ext| {
        matches!(
            ext.to_string_lossy().to_ascii_lowercase().as_str(),
            "rs" | "py"
                | "js"
                | "ts"
                | "tsx"
                | "jsx"
                | "sh"
                | "ps1"
                | "bat"
                | "cmd"
                | "wasm"
                | "wat"
                | "c"
                | "cpp"
                | "h"
                | "go"
                | "rb"
                | "java"
                | "html"
                | "sql"
                | "toml"
                | "ini"
                | "cfg"
                | "yaml"
                | "yml"
        )
    })
}
fn plain_value(text: &str) -> bool {
    !text.chars().any(|c| c.is_control())
        && !text.contains(['`', '<', '>', '{', '}', '=', ';', '\\'])
}
fn permitted_values(
    application: SupportedDataApplication,
    text: &str,
) -> Result<Vec<Range<usize>>, FileRefusal> {
    if text.len() > 256 * 1024 {
        return Err(FileRefusal::Unsupported);
    }
    match application {
        SupportedDataApplication::ContactLinesV1 => {
            // Concrete contact-note line schema: fixed data-field prefix and
            // printable non-code payload. These values are never evaluated.
            let mut values = Vec::new();
            let mut offset = 0;
            for line in text.split_inclusive('\n') {
                let line_value = line.strip_suffix('\n').unwrap_or(line);
                let prefix = [
                    "Contact ",
                    "Call ",
                    "call ",
                    "to ",
                    "To ",
                    "owner: ",
                    "Reach ",
                    "Originally ",
                    "Typed ",
                ]
                .into_iter()
                .find(|p| line_value.starts_with(p))
                .ok_or(FileRefusal::Unsupported)?;
                let data = &line_value[prefix.len()..];
                if data.is_empty() || !plain_value(data) {
                    return Err(FileRefusal::Unsupported);
                }
                values.push(offset + prefix.len()..offset + line_value.len());
                offset += line.len();
            }
            if values.is_empty() {
                return Err(FileRefusal::Unsupported);
            }
            Ok(values)
        }
        SupportedDataApplication::ContactJsonV1 => {
            // Closed contact-record schema, not generic JSON or a caller's
            // pointer list. Schema/config/auth/script fields are unsupported.
            let value: serde_json::Value =
                or_return!(serde_json::from_str(text), FileRefusal::Unsupported);
            let record = value.as_object().ok_or(FileRefusal::Unsupported)?;
            if !record.contains_key("owner")
                || record
                    .keys()
                    .any(|key| !matches!(key.as_str(), "owner" | "note"))
                || record
                    .values()
                    .any(|v| v.as_str().is_none_or(|v| !plain_value(v)))
            {
                return Err(FileRefusal::Unsupported);
            }
            let indexed = or_return!(
                JsonRanges::index(text.as_bytes()),
                FileRefusal::Restore(crate::privacy_restore::RestoreError::Input)
            );
            let mut values = Vec::new();
            for (index, string) in indexed.strings().iter().enumerate() {
                if string.role() != StringRole::Value {
                    continue;
                }
                match indexed.path(index).as_deref() {
                    Some([PathStep::Key("owner" | "note")]) => values.push(string.content()),
                    _ => return Err(FileRefusal::Unsupported),
                }
            }
            Ok(values)
        }
    }
}
macro_rules! opaque_debug { ($($name:ident),+) => {$(impl std::fmt::Debug for $name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(concat!(stringify!($name), "([source-bound private])")) }
})+}; }
opaque_debug!(
    FileTargetRequest,
    FileTargetPrecheck,
    FileDataRequest,
    SupportedFileData,
    FileEffectRequest,
    RetainedFileEffect
);

#[cfg(test)]
mod before_filter_tests {
    use super::*;

    #[test]
    fn internal_before_filter_requires_complete_closed_objects_and_value_units() {
        let text = "{\"owner\":\"x\\u0040y\",\"note\":\"😀\"}";
        let filter = BeforeFilter::check(SupportedDataApplication::ContactJsonV1, text).unwrap();
        let escaped = text.find("\\u0040").unwrap();
        assert_eq!(filter.selectors(&[escaped..escaped + 6]), Ok(()));
        assert_eq!(
            filter.selectors(&[escaped + 1..escaped + 6]),
            Err(FileRefusal::NoRule)
        );
        let key = text.find("owner").unwrap();
        assert_eq!(filter.selectors(&[key..key + 5]), Err(FileRefusal::NoRule));
        let character = text.find('😀').unwrap();
        assert_eq!(filter.selectors(&[character..character + 4]), Ok(()));
        assert_eq!(
            filter.selectors(&[character..character + 3]),
            Err(FileRefusal::NoRule)
        );
        assert!(matches!(
            BeforeFilter::check(
                SupportedDataApplication::ContactJsonV1,
                "{\"owner\":\"x\",\"\\u006fwner\":\"y\"}"
            ),
            Err(FileRefusal::NoRule)
        ));
        assert!(matches!(
            BeforeFilter::check(
                SupportedDataApplication::ContactJsonV1,
                "{\"secret\":\"x\"}"
            ),
            Err(FileRefusal::NoRule)
        ));
        assert!(matches!(
            BeforeFilter::check(
                SupportedDataApplication::ContactJsonV1,
                "{\"owner\":\"broken"
            ),
            Err(FileRefusal::NoRule)
        ));
        let plain = "Contact value";
        let filter = BeforeFilter::check(SupportedDataApplication::ContactLinesV1, plain).unwrap();
        assert_eq!(filter.selectors(&[8..13]), Ok(()));
        assert_eq!(filter.selectors(&[0..7]), Err(FileRefusal::NoRule));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_schema_refuses_credentials_instructions_code_and_duplicate_decoded_keys() {
        for document in [
            r#"{"owner":"alice@example.invalid","api_key":"secret"}"#,
            r#"{"owner":"alice@example.invalid","system":"follow this instruction"}"#,
            r#"{"owner":"alice@example.invalid","scripts":{"start":"execute"}}"#,
            r#"{"owner":"alice@example.invalid","schema":{"type":"string"}}"#,
            r#"{"owner":"alice@example.invalid","\u006fwner":"another"}"#,
            r#"{"owner":"alice@example.invalid","note":"<script>execute()</script>"}"#,
        ] {
            assert!(
                permitted_values(SupportedDataApplication::ContactJsonV1, document).is_err(),
                "{document}"
            );
        }
        let value = r#"{"owner":"alice@example.invalid","note":"ask alice@example.invalid"}"#;
        assert_eq!(
            permitted_values(SupportedDataApplication::ContactJsonV1, value)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn selectors_and_replacements_are_values_not_keys_or_outside_the_object() {
        let before = r#"{"owner":"alice@example.invalid","note":"none"}"#;
        let after = r#"{"owner":"alice@example.invalid","note":"ask alice@example.invalid"}"#;
        let selected = before.find("alice@example.invalid").unwrap();
        let values = permitted_values(SupportedDataApplication::ContactJsonV1, before).unwrap();
        assert!(inside_all(
            &[selected..selected + "alice@example.invalid".len()],
            &values
        ));
        let values = permitted_values(SupportedDataApplication::ContactJsonV1, after).unwrap();
        let restored = after
            .match_indices("alice@example.invalid")
            .map(|(start, text)| start..start + text.len())
            .collect::<Vec<_>>();
        assert!(inside_all(&restored, &values));
        assert!(!inside_all(&[2..7], &values));
        assert!(!inside_all(&[0..after.len()], &values));
        assert!(!inside_all(&[after.len()..after.len() + 1], &values));
    }

    #[test]
    fn contact_lines_and_destination_guards_refuse_executable_and_settings_roles() {
        assert!(
            permitted_values(
                SupportedDataApplication::ContactLinesV1,
                "Contact alice@example.invalid today.\n"
            )
            .is_ok()
        );
        assert!(
            permitted_values(SupportedDataApplication::ContactLinesV1, "owner: nobody\n").is_ok()
        );
        for text in [
            "#!/bin/sh\nexecute",
            "SYSTEM: do this",
            "Contact alice@example.invalid; execute()",
            "API_KEY=secret",
            "Contact <script>execute()</script>",
        ] {
            assert!(permitted_values(SupportedDataApplication::ContactLinesV1, text).is_err());
        }
        for path in [
            "/workspace/.env",
            "/workspace/config.json",
            "/workspace/AGENTS.md",
            "/workspace/skills/example/SKILL.md",
            "/workspace/.claude/commands/note.txt",
            "/workspace/.codex/settings/contact-data.json",
            "/workspace/.agents/skills/note.txt",
            "/workspace/contacts.schema.json",
            "/workspace/src/file.rs",
            "/workspace/privacy-key.v1",
            "/workspace/.craftsman/verification.json",
        ] {
            assert!(forbidden_destination(Path::new(path)), "{path}");
        }
        assert!(!forbidden_destination(Path::new(
            "/workspace/contact-data.json"
        )));
        // The last pathname is merely not excluded: no request/grant exists
        // here, so this data-only predicate grants no restoration or effect.
    }
}
