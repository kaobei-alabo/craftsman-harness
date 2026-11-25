//! A file write or edit of this conversation as it will actually run.
//!
//! The model writes placeholders, also into the content of a file it asks
//! to write. The recorded call keeps them: it is the canonical call, it is
//! what the model is shown again, and nothing here changes it. What reaches
//! the disk is derived from it, separately, as an execution view: the same
//! call with the values put back into its content fields.
//!
//! The view is derived once, in the agent's own event before a call is
//! reviewed, from the plan the agent issued for that call. The agent keeps
//! it as the source of that review and carries its digest beside the plan.
//! Whoever then decides, the automatic policy of a gate or a person the
//! gate asks, decides on the content that will really be written, and the
//! decision is recorded where it is taken and carried to the call's
//! terminal. There the file tool checks the plan, the source, the digest,
//! the decision, the user's policy and the destination again, and the write
//! goes through what the view held on to when it was derived: the open file
//! for an edit, the absence of the file for a create.
//!
//! What is restored: the `content` of `write_file`, and `old_string` and
//! `new_string` of `edit_file`, as plain text or as part of a complete JSON
//! document, and only where the user's policy names that data format for the
//! file the operation is bound to. A path, a command, a URL, a tool or gadget
//! argument, a credential, a setting, an instruction, a schema or a memory
//! file is never restored. A file's name or extension is not a rule. Where
//! anything is missing or unclear, the tool answers with a fixed refusal
//! before any effect, and the call stays as the model wrote it.
//!
//! The file tool itself still runs. It notes the file for the workspace
//! checkpoint, performs the write, finishes the checkpoint and asks for the
//! language diagnostics, as for any other write; only the write is taken
//! from the derived operation ([`authorized_write`]) instead of the call's
//! arguments. A delegated agent's file tools work the same way inside that
//! agent's own boundary, behind the gate every delegated tool has.
//!
//! This grants no permission. A plan or deny mode, a protected path and a
//! person's refusal apply to the derived operation as they apply to any
//! other. Copied content cannot be taken back, so every check that matters
//! for the write happens before the write. Delete and rename are untouched.

use super::file_data::{
    BeforeFilter, DataRequest, FileDataRequest, FileEffectRequest, FileTargetRequest,
    SupportedDataApplication, TargetRequest,
};
use super::view::{
    ApprovedOperation, ConversationBoundary, FieldPlace, PlannedOperation, SourceField,
};
use super::{CapturedUserPolicy, Inner, PrivacySession, SessionStop};
use crate::privacy_restore::{FileDataFormat, RestoreError, Restored, Restorer, file_candidate};
#[cfg(test)]
use craftsman_harness::provenance::ActualParkedCall;
use craftsman_harness::provenance::{
    ActualPlannedInvocation, ActualToolInvocation, OutcomePart, SourceRef, TrackedDispatch,
};
use craftsman_harness::{AgentError, HitlAction, Tool, ToolCall, ToolCtx, ToolDef, ToolOutcome};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::io::Write;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
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

/// The most bytes of a file an edit is derived for.
const MAX_FILE: usize = 256 * 1024;

/// Why an operation was not derived, decided or run. Every one of them is
/// decided before the file is touched.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FileRefusal {
    #[error("Privacy stopped this conversation. {0}")]
    Stopped(SessionStop),
    #[error("Private values can be restored only into the content of a file write or edit.")]
    Unsupported,
    #[error(
        "The user's policy names no data format for this file, so private values are not restored into it. Nothing was written."
    )]
    NoRule,
    #[error("A private alias stands in {0}, where it is never restored. Nothing was written.")]
    Excluded(&'static str),
    #[error("{0} Nothing was written.")]
    Restore(RestoreError),
    #[error("The file is not what this operation was derived and shown for. Nothing was written.")]
    Stale,
    #[error("This file operation was not approved as it was shown. Nothing was written.")]
    NotApproved,
    #[error("This file operation has already run.")]
    Spent,
    /// The tool's own ordinary answer for a call it cannot run.
    #[error("{0}")]
    Tool(String),
}

/// What an operation does to its destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    /// `write_file`: the file did not exist when the operation was derived.
    Create,
    /// `edit_file`: the file was opened and read when the operation was
    /// derived.
    Update,
}

/// What identifies a file or a directory on this host, where the host has
/// such a thing.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}

fn identity(metadata: &std::fs::Metadata) -> Option<Identity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(Identity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        // No portable identity. The open handle of an edit is still the one
        // that is read and written; a create has only its atomic refusal of
        // an existing file.
        let _ = metadata;
        None
    }
}

/// A final observation of the held update and the name under the policy's
/// retained parent. This does not grant permission or prevent same-user
/// writers from racing a subsequent write.
#[cfg(unix)]
fn validate_held_update(
    file: &std::fs::File,
    directory: &std::fs::File,
    name: &std::ffi::OsStr,
    bound: Option<Identity>,
) -> Result<(), FileRefusal> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    let held = or_return!(file.metadata(), FileRefusal::Stale);
    if !held.is_file()
        || held.nlink() != 1
        || held.uid() != unsafe { libc::geteuid() }
        || held.mode() & 0o111 != 0
        || identity(&held) != bound
    {
        return Err(FileRefusal::Stale);
    }
    let name = or_return!(
        std::ffi::CString::new(name.as_encoded_bytes()),
        FileRefusal::Unsupported
    );
    let mut named = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: directory is retained and live, name is NUL-terminated, and
    // fstatat initializes named only on success. No link is followed.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            named.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(FileRefusal::Stale);
    }
    // SAFETY: the successful fstatat initialized the complete stat.
    let named = unsafe { named.assume_init() };
    if named.st_mode & libc::S_IFMT != libc::S_IFREG
        || named.st_nlink != 1
        || named.st_uid != held.uid()
        || named.st_mode & 0o111 != 0
        || named.st_dev as u64 != held.dev()
        || named.st_ino as u64 != held.ino()
    {
        return Err(FileRefusal::Stale);
    }
    Ok(())
}

/// What the operation held on to when it was derived.
enum Destination {
    /// Nothing was at the path. The directory it goes into, as it was.
    Absent { parent: Option<Identity> },
    /// The file, opened once. It is read and written through this handle.
    Existing {
        file: std::fs::File,
        identity: Option<Identity>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Derived,
    Refused,
    Done,
}

/// What an operation holds until it is over: the real content, the call's
/// arguments with it, and the destination. It is released when the
/// operation has run or was refused, so that the source the agent keeps for
/// the review afterwards holds no content and no open file.
struct Held {
    arguments: serde_json::Value,
    destination: Destination,
    old: Option<Zeroizing<String>>,
    new: Zeroizing<String>,
    data_request: FileDataRequest,
    application: SupportedDataApplication,
}

/// One derived operation. It is made only inside a [`ReviewedFile`], from
/// what the committed reply proposed with the plan the agent issued.
pub struct DerivedFileOperation {
    owner: Arc<Inner>,
    change: FileChange,
    /// The destination the operation is bound to, as the host resolved it
    /// when the operation was derived. Never the model's spelling.
    path: PathBuf,
    policy: Weak<dyn CapturedUserPolicy>,
    digest: [u8; 32],
    held: Mutex<Option<Held>>,
    state: Mutex<State>,
    /// Whether the preview was handed to whoever asks a person.
    presented: AtomicBool,
}

/// What a reviewer is shown: the real content, never the placeholders.
pub struct FilePreview<'a> {
    pub change: FileChange,
    /// The destination the operation is bound to.
    pub path: &'a Path,
    /// The file as it is now. `None` for a create.
    pub old: Option<&'a str>,
    /// The file as it will be.
    pub new: &'a str,
    /// The call's arguments with the real content in its content fields.
    /// The path and every other argument are as the model wrote them.
    pub arguments: &'a serde_json::Value,
    /// What a decision on this preview is bound to.
    pub digest: [u8; 32],
}

impl std::fmt::Debug for DerivedFileOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DerivedFileOperation([private])")
    }
}

/// The string at `name` of the call's arguments.
fn argument<'a>(args: &'a serde_json::Value, name: &str) -> Result<&'a str, FileRefusal> {
    args.get(name)
        .and_then(serde_json::Value::as_str)
        .ok_or(FileRefusal::Unsupported)
}

/// One content field with the values put back. Each occurrence is checked
/// against the live record of the commit that issued its placeholder. An
/// alias nobody validated, or one inside a longer token, refuses the field.
struct RestoredText {
    value: Zeroizing<String>,
    ranges: Vec<std::ops::Range<usize>>,
}

fn restored_field(
    format: FileDataFormat,
    whole_document: bool,
    text: &str,
    fields: &[&SourceField],
) -> Result<RestoredText, FileRefusal> {
    let mut restored = Vec::with_capacity(fields.len());
    for field in fields {
        let range = field.range();
        let original = field
            .with_record(|mapping, records, index| {
                Restorer::from_records(mapping, records)
                    .and_then(|restorer| restorer.occurrence(text, range.clone(), index))
            })
            .map_err(FileRefusal::Stopped)?
            .map_err(FileRefusal::Restore)?
            .ok_or(FileRefusal::Restore(RestoreError::UnknownAlias))?;
        // A piece of a JSON document that is not the whole document: the
        // value goes where a string of that document is, encoded as one.
        let original = match (format, whole_document) {
            (FileDataFormat::JsonDocument, false) => {
                let quoted = Zeroizing::new(
                    serde_json::to_string(original.as_str())
                        .map_err(|_| FileRefusal::Restore(RestoreError::Input))?,
                );
                Zeroizing::new(quoted[1..quoted.len() - 1].to_owned())
            }
            _ => original,
        };
        restored.push(Restored { range, original });
    }
    let mut removed = 0usize;
    let mut inserted = 0usize;
    let mut ranges = Vec::with_capacity(restored.len());
    for occurrence in &restored {
        let length = if format == FileDataFormat::JsonDocument && whole_document {
            serde_json::to_string(occurrence.original.as_str())
                .map_err(|_| FileRefusal::Restore(RestoreError::Input))?
                .len()
                - 2
        } else {
            occurrence.original.len()
        };
        let start = occurrence
            .range
            .start
            .checked_sub(removed)
            .and_then(|n| n.checked_add(inserted))
            .ok_or(FileRefusal::Restore(RestoreError::Bounds))?;
        ranges.push(start..start + length);
        removed += occurrence.range.len();
        inserted += length;
    }
    let format = match whole_document {
        true => format,
        false => FileDataFormat::PlainText,
    };
    let value = file_candidate(format, text, &restored).map_err(FileRefusal::Restore)?;
    Ok(RestoredText { value, ranges })
}

/// Same ordinary nonoverlapping replacement semantics, retaining the exact
/// before selector and after replacement ranges of every restored occurrence.
fn replace_with_ranges(
    before: &str,
    old: &RestoredText,
    new: &RestoredText,
    replace_all: bool,
) -> Result<
    (
        Zeroizing<String>,
        Vec<std::ops::Range<usize>>,
        Vec<std::ops::Range<usize>>,
    ),
    FileRefusal,
> {
    let mut after = Zeroizing::new(String::new());
    let mut copied = 0;
    let mut selectors = Vec::new();
    let mut replacements = Vec::new();
    for (offset, _) in before.match_indices(old.value.as_str()) {
        after.push_str(&before[copied..offset]);
        let start = after.len();
        selectors.extend(
            old.ranges
                .iter()
                .map(|range| offset + range.start..offset + range.end),
        );
        replacements.extend(
            new.ranges
                .iter()
                .map(|range| start + range.start..start + range.end),
        );
        after.push_str(&new.value);
        if after.len() > MAX_FILE {
            return Err(FileRefusal::Restore(RestoreError::Bounds));
        }
        copied = offset + old.value.len();
        if !replace_all {
            break;
        }
    }
    after.push_str(&before[copied..]);
    if after.len() > MAX_FILE {
        return Err(FileRefusal::Restore(RestoreError::Bounds));
    }
    Ok((after, selectors, replacements))
}

impl PrivacySession {
    /// Derives what a planned `write_file` or `edit_file` will actually do.
    /// `policy` is the user's captured policy and `root` the directory
    /// relative paths are resolved against, as the file tools resolve them.
    ///
    /// Nothing is written here. The destination is resolved and observed
    /// first, and the policy is asked about that resolved destination. For
    /// an edit the file is opened and read, and stays open in the result.
    pub(super) fn derive_file_operation(
        &self,
        policy: &Weak<dyn CapturedUserPolicy>,
        operation: &Arc<PlannedOperation>,
        root: &Path,
    ) -> Result<DerivedFileOperation, FileRefusal> {
        self.inner.live().map_err(FileRefusal::Stopped)?;
        if !operation.model().committed_by(self) {
            return Err(FileRefusal::Stopped(SessionStop::Unbound));
        }
        let call = operation.call().map_err(FileRefusal::Stopped)?;
        let content_fields: &[&str] = match call.name.as_str() {
            "write_file" => &["/content"],
            "edit_file" => &["/old_string", "/new_string"],
            _ => return Err(FileRefusal::Unsupported),
        };
        let path_text = argument(&call.args, "file_path")?;
        // Where the model wrote placeholders. An occurrence anywhere but in
        // a content field is in a place that is never restored.
        let fields = operation
            .fields()
            .map_err(FileRefusal::Stopped)?
            .ok_or(FileRefusal::Restore(RestoreError::Input))?;
        let mut by_field: Vec<Vec<&SourceField>> = vec![Vec::new(); content_fields.len()];
        for field in fields {
            let at = match field.place() {
                FieldPlace::Argument { tool, pointer } if *tool == call.name => {
                    content_fields.iter().position(|name| name == pointer)
                }
                _ => None,
            };
            match at {
                Some(at) => by_field[at].push(field),
                None => return Err(FileRefusal::Excluded("another argument of the call")),
            }
        }
        if path_text.contains("pv1-") {
            return Err(FileRefusal::Excluded("the path"));
        }
        // A memory file is kept in the application's store, not on the
        // filesystem this operation binds.
        let memory = crate::store_fs::MEMORY_PREFIX;
        if path_text == memory.trim_end_matches('/') || path_text.starts_with(memory) {
            return Err(FileRefusal::Excluded("a memory file"));
        }
        // The user's policy, which the session keeps. A review that outlived
        // its session has none, and so no rule.
        let policy_owner = policy.upgrade().ok_or(FileRefusal::NoRule)?;
        let workspace = self.workspace_sha256().to_owned();
        // Source-bound target eligibility precedes private data reads and
        // selector restoration/comparison. It refuses only; whole images,
        // ranges, retained effects and tool permission remain mandatory.
        // The same compiled application selects the decoder in every build.
        let precheck_target = |bound: &Path, change| {
            let request = FileTargetRequest::issued(TargetRequest {
                owner: self.inner.clone(),
                operation: operation.clone(),
                workspace: workspace.clone(),
                destination: bound.to_owned(),
                change,
            });
            let precheck = policy_owner
                .precheck_file_target(&request)
                .filter(|precheck| precheck.matches(&request))
                .ok_or(FileRefusal::NoRule)?;
            let format = precheck.format();
            #[cfg(test)]
            if let Some(legacy) = policy_owner.restorable_file_data(&workspace, bound) {
                if legacy != format {
                    return Err(FileRefusal::NoRule);
                }
            }
            self.inner.live().map_err(FileRefusal::Stopped)?;
            Ok((format, precheck.application()))
        };
        let path = match Path::new(path_text).is_absolute() {
            true => PathBuf::from(path_text),
            false => root.join(path_text),
        };

        let mut arguments = call.args.clone();
        let mut shown = |name: &str, text: &str| {
            if let Some(members) = arguments.as_object_mut() {
                members.insert(name.to_owned(), serde_json::Value::String(text.to_owned()));
            }
        };
        let (
            change,
            destination,
            bound,
            format,
            selected_application,
            old,
            new,
            selectors,
            replacements,
        ) = match call.name.as_str() {
            "write_file" => {
                // A create binds the absence of the file, as the tool does.
                match std::fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => {
                        return Err(FileRefusal::Tool(format!(
                            "Cannot write to {} because it already exists. Read and then make an edit, or write to a new path.",
                            path.display()
                        )));
                    }
                }
                let parent = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or(root);
                let held = std::fs::metadata(parent)
                    .ok()
                    .filter(std::fs::Metadata::is_dir)
                    .ok_or_else(|| {
                        FileRefusal::Tool(format!(
                            "Error creating {}: its directory does not exist",
                            path.display()
                        ))
                    })?;
                // The destination: the directory as the host resolves it,
                // which must be the directory just observed, and the file's
                // own name in it.
                let name = path.file_name().ok_or(FileRefusal::Unsupported)?;
                let resolved = or_return!(std::fs::canonicalize(parent), FileRefusal::Stale);
                let again = or_return!(std::fs::metadata(&resolved), FileRefusal::Stale);
                if !again.is_dir() || identity(&again) != identity(&held) {
                    return Err(FileRefusal::Stale);
                }
                let bound = resolved.join(name);
                let (format, application) = precheck_target(&bound, FileChange::Create)?;
                let new =
                    restored_field(format, true, argument(&call.args, "content")?, &by_field[0])?;
                shown("content", &new.value);
                (
                    FileChange::Create,
                    Destination::Absent {
                        parent: identity(&held),
                    },
                    bound,
                    format,
                    application,
                    None,
                    new.value,
                    Vec::new(),
                    new.ranges,
                )
            }
            _ => {
                // An update binds the file itself: a regular file, opened
                // once, and read through that handle.
                let unreadable = |error: std::io::Error| {
                    FileRefusal::Tool(format!("Error reading {}: {error}", path.display()))
                };
                let named = std::fs::symlink_metadata(&path).map_err(unreadable)?;
                if !named.is_file() {
                    return Err(FileRefusal::Unsupported);
                }
                let mut file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .map_err(unreadable)?;
                let opened = file.metadata().map_err(unreadable)?;
                if identity(&opened) != identity(&named) {
                    return Err(FileRefusal::Stale);
                }
                // The destination: the opened file as the host resolves its
                // path, which must name that same file.
                let bound = or_return!(std::fs::canonicalize(&path), FileRefusal::Stale);
                let again = or_return!(std::fs::symlink_metadata(&bound), FileRefusal::Stale);
                if !again.is_file() || identity(&again) != identity(&opened) {
                    return Err(FileRefusal::Stale);
                }
                let (format, application) = precheck_target(&bound, FileChange::Update)?;
                if opened.len() > MAX_FILE as u64 {
                    return Err(FileRefusal::NoRule);
                }
                let mut text = Zeroizing::new(String::new());
                or_return!(
                    (&mut file)
                        .take(MAX_FILE as u64 + 1)
                        .read_to_string(&mut text),
                    FileRefusal::NoRule
                );
                if text.len() > MAX_FILE {
                    return Err(FileRefusal::NoRule);
                }
                // The actual source-bound precheck selected this application.
                // Reject the complete containing before-image before any
                // private alias is restored or selector observation is made.
                // This internal filter grants no image, effect or permission.
                let before_filter = BeforeFilter::check(application, &text)?;
                let old_string = restored_field(
                    format,
                    false,
                    argument(&call.args, "old_string")?,
                    &by_field[0],
                )?;
                let new_string = restored_field(
                    format,
                    false,
                    argument(&call.args, "new_string")?,
                    &by_field[1],
                )?;
                shown("old_string", &old_string.value);
                shown("new_string", &new_string.value);
                let replace_all = call
                    .args
                    .get("replace_all")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if old_string.value.is_empty() {
                    return Err(FileRefusal::Tool(
                        "Error: old_string must not be empty".into(),
                    ));
                }
                let mut count = 0;
                let mut actual_selectors = Vec::new();
                for (offset, _) in text.match_indices(old_string.value.as_str()) {
                    count += 1;
                    actual_selectors.extend(
                        old_string
                            .ranges
                            .iter()
                            .map(|range| offset + range.start..offset + range.end),
                    );
                }
                // Every restored selector span must be in a supported data
                // value and between complete units before count/miss answers.
                before_filter.selectors(&actual_selectors)?;
                if count == 0 {
                    return Err(FileRefusal::Tool(format!(
                        "Error: old_string not found in {}",
                        path.display()
                    )));
                }
                if count > 1 && !replace_all {
                    return Err(FileRefusal::Tool(format!(
                        "Error: old_string occurs {count} times in {}; pass replace_all or a longer unique string",
                        path.display()
                    )));
                }
                let (new, selectors, replacements) =
                    replace_with_ranges(&text, &old_string, &new_string, replace_all)?;
                drop(before_filter);
                if new.len() > MAX_FILE {
                    return Err(FileRefusal::Restore(RestoreError::Bounds));
                }
                if format == FileDataFormat::JsonDocument
                    && serde_json::from_str::<serde_json::Value>(&new).is_err()
                {
                    return Err(FileRefusal::Restore(RestoreError::Input));
                }
                (
                    FileChange::Update,
                    Destination::Existing {
                        file,
                        identity: identity(&opened),
                    },
                    bound,
                    format,
                    application,
                    Some(text),
                    new,
                    selectors,
                    replacements,
                )
            }
        };

        let data_request = FileDataRequest::issued(DataRequest {
            owner: self.inner.clone(),
            operation: operation.clone(),
            workspace: workspace.clone(),
            destination: bound.clone(),
            change,
            format,
            before: old.clone(),
            after: new.clone(),
            selectors,
            replacements,
        });
        let supported = policy_owner
            .supported_file_data(&data_request)
            .filter(|supported| {
                supported.matches(&data_request) && supported.application() == selected_application
            })
            .ok_or(FileRefusal::NoRule)?;
        let application = supported.application();

        // What a decision is bound to: the call as the model wrote it, the
        // data format, the bound destination and both contents. The call's
        // id alone names nothing.
        let mut hasher = Sha256::new();
        hasher.update(b"craftsman-privacy-file-operation-v3\0");
        hasher.update([change as u8, format as u8]);
        let written = call.args.to_string();
        for piece in [
            call.id.as_bytes(),
            call.name.as_bytes(),
            written.as_bytes(),
            bound.as_os_str().as_encoded_bytes(),
        ] {
            hasher.update((piece.len() as u64).to_le_bytes());
            hasher.update(piece);
        }
        match &old {
            Some(old) => hasher.update(Sha256::digest(old.as_bytes())),
            None => hasher.update([0; 32]),
        }
        hasher.update(Sha256::digest(new.as_bytes()));
        self.inner.live().map_err(FileRefusal::Stopped)?;
        Ok(DerivedFileOperation {
            owner: self.inner.clone(),
            change,
            path: bound,
            policy: policy.clone(),
            digest: hasher.finalize().into(),
            held: Mutex::new(Some(Held {
                arguments,
                destination,
                old,
                new,
                data_request,
                application,
            })),
            state: Mutex::new(State::Derived),
            presented: AtomicBool::new(false),
        })
    }
}

impl DerivedFileOperation {
    pub fn change(&self) -> FileChange {
        self.change
    }

    /// What a reviewer is shown, for the duration of `read`. An operation
    /// that is over shows nothing.
    pub fn preview<T>(&self, read: impl FnOnce(FilePreview<'_>) -> T) -> Result<T, FileRefusal> {
        self.owner.live().map_err(FileRefusal::Stopped)?;
        let held = self.held.lock().unwrap();
        let held = held.as_ref().ok_or(FileRefusal::Spent)?;
        let result = read(FilePreview {
            change: self.change,
            path: &self.path,
            old: held.old.as_ref().map(|old| old.as_str()),
            new: &held.new,
            arguments: &held.arguments,
            digest: self.digest,
        });
        self.owner.live().map_err(FileRefusal::Stopped)?;
        Ok(result)
    }

    /// The operation is over in `state`. What it held is released: the
    /// content, the arguments with it and the open file.
    fn finish(&self, state: State) {
        let mut current = self.state.lock().unwrap();
        if *current != State::Done {
            *current = state;
        }
        self.held.lock().unwrap().take();
    }

    /// The write itself, once. The user's policy must still name the same
    /// data format for the bound destination, and the destination must be
    /// what it was: for a create the directory and the absence of the file,
    /// for an edit the opened file and what was read from it.
    fn write(&self) -> Result<usize, FileRefusal> {
        self.owner.live().map_err(FileRefusal::Stopped)?;
        let mut held = self.held.lock().unwrap().take().ok_or(FileRefusal::Spent)?;
        // Keep the actual selected policy owner through both grant returns
        // and the effect. A supplied grant or a schema/directory check alone
        // never substitutes for that provider call.
        let policy = self.policy.upgrade().ok_or(FileRefusal::NoRule)?;
        #[cfg(test)]
        {
            let rule =
                policy.restorable_file_data(held.data_request.workspace_sha256(), &self.path);
            // A legacy fixture proposal is never sufficient and may be None.
            if rule.is_some() && rule != Some(held.data_request.format()) {
                return Err(FileRefusal::NoRule);
            }
        }
        let supported = policy
            .supported_file_data(&held.data_request)
            .filter(|supported| {
                supported.matches(&held.data_request) && supported.application() == held.application
            })
            .ok_or(FileRefusal::NoRule)?;
        if supported.application() != held.application
            || held.data_request.after() != held.new.as_str()
        {
            return Err(FileRefusal::Stale);
        }
        let failed = |error: std::io::Error| {
            FileRefusal::Tool(format!("Error writing {}: {error}", self.path.display()))
        };
        match &mut held.destination {
            Destination::Absent { parent } => {
                let now = self
                    .path
                    .parent()
                    .and_then(|parent| std::fs::metadata(parent).ok());
                if parent.is_some() && now.as_ref().and_then(identity) != *parent {
                    return Err(FileRefusal::Stale);
                }
                let observed = now.as_ref().ok_or(FileRefusal::Stale)?;
                let effect_request = FileEffectRequest::issued(&held.data_request, observed)?;
                let effect = policy
                    .retained_file_effect(&effect_request)
                    .filter(|effect| effect.matches(&effect_request))
                    .ok_or(FileRefusal::NoRule)?;
                #[cfg(not(unix))]
                let _ = &effect;
                self.owner.live().map_err(FileRefusal::Stopped)?;
                // The actual policy-retained parent anchors the effect.
                // No fresh absolute-path open or source/permission shortcut.
                #[cfg(unix)]
                let mut file = {
                    use std::os::fd::{AsRawFd, FromRawFd};
                    let name = self.path.file_name().ok_or(FileRefusal::Unsupported)?;
                    let name = or_return!(
                        std::ffi::CString::new(name.as_encoded_bytes()),
                        FileRefusal::Unsupported
                    );
                    let descriptor = unsafe {
                        libc::openat(
                            effect.directory().as_raw_fd(),
                            name.as_ptr(),
                            libc::O_WRONLY
                                | libc::O_CREAT
                                | libc::O_EXCL
                                | libc::O_NOFOLLOW
                                | libc::O_CLOEXEC,
                            0o666,
                        )
                    };
                    if descriptor < 0 {
                        let error = std::io::Error::last_os_error();
                        return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
                            FileRefusal::Stale
                        } else {
                            failed(error)
                        });
                    }
                    // SAFETY: openat returned one new owned descriptor.
                    unsafe { std::fs::File::from_raw_fd(descriptor) }
                };
                #[cfg(not(unix))]
                return Err(FileRefusal::Unsupported);
                #[cfg(unix)]
                {
                    file.write_all(held.new.as_bytes()).map_err(failed)?;
                    file.flush().map_err(failed)?;
                }
            }
            Destination::Existing {
                file,
                identity: bound,
            } => {
                let parent = self.path.parent().ok_or(FileRefusal::Stale)?;
                let observed = or_return!(std::fs::metadata(parent), FileRefusal::Stale);
                let effect_request = FileEffectRequest::issued(&held.data_request, &observed)?;
                let effect = policy
                    .retained_file_effect(&effect_request)
                    .filter(|effect| effect.matches(&effect_request))
                    .ok_or(FileRefusal::NoRule)?;
                #[cfg(not(unix))]
                let _ = &effect;
                self.owner.live().map_err(FileRefusal::Stopped)?;
                // The path still names the file that was opened, and the
                // file still holds what was read and shown.
                let named = or_return!(std::fs::symlink_metadata(&self.path), FileRefusal::Stale);
                if !named.is_file() || identity(&named) != *bound {
                    return Err(FileRefusal::Stale);
                }
                let mut current = Zeroizing::new(String::new());
                file.seek(SeekFrom::Start(0)).map_err(failed)?;
                or_return!(file.read_to_string(&mut current), FileRefusal::Stale);
                if Some(current.as_str()) != held.old.as_ref().map(|old| old.as_str()) {
                    return Err(FileRefusal::Stale);
                }
                file.seek(SeekFrom::Start(0)).map_err(failed)?;
                #[cfg(unix)]
                validate_held_update(
                    file,
                    effect.directory(),
                    self.path.file_name().ok_or(FileRefusal::Unsupported)?,
                    *bound,
                )?;
                #[cfg(not(unix))]
                return Err(FileRefusal::Unsupported);
                #[cfg(unix)]
                self.owner.live().map_err(FileRefusal::Stopped)?;
                #[cfg(unix)]
                {
                    file.set_len(0).map_err(failed)?;
                    file.write_all(held.new.as_bytes()).map_err(failed)?;
                    file.flush().map_err(failed)?;
                }
            }
        }
        #[cfg(unix)]
        return Ok(held.new.len());
    }
}

/// What one planned write or edit of this conversation will really do, as it
/// was derived when the agent sent the plan to its review. It is the source
/// the agent keeps for that review: the gate that decides, the person who is
/// asked and the file tool all refer to this one object. It is made only by
/// the conversation's boundary, in the agent's own event before a review,
/// for the plan the agent issued. Holding it approves nothing.
///
/// Where the operation could not be derived, this holds the refusal, which
/// is the tool's answer before anyone is asked and before any effect.
pub struct ReviewedFile {
    owner: Arc<Inner>,
    operation: Arc<PlannedOperation>,
    outcome: Result<DerivedFileOperation, FileRefusal>,
}

impl std::fmt::Debug for ReviewedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReviewedFile([private])")
    }
}

impl ReviewedFile {
    /// Called only by the independently registered input/policy producer.
    /// Source/digest come from this actual dispatch, never caller tuple data.
    pub(crate) fn producer_preview(
        self: &Arc<Self>,
        session: &PrivacySession,
        tracked: &TrackedDispatch,
    ) -> Result<(ToolCall, Option<[u8; 32]>), FileRefusal> {
        let derived = self.derived()?;
        if !Arc::ptr_eq(&self.owner, &session.inner)
            || !tracked
                .planned()
                .is_some_and(|plan| plan.same_plan(self.operation.plan()))
            || !Self::of(tracked).is_some_and(|source| Arc::ptr_eq(&source, self))
            || tracked.shown() != Some(derived.digest)
        {
            return Err(FileRefusal::NotApproved);
        }
        derived.preview(|preview| {
            (
                ToolCall {
                    id: self.operation.plan().call().id.clone(),
                    name: self.operation.plan().call().name.clone(),
                    args: preview.arguments.clone(),
                },
                Some(preview.digest),
            )
        })
    }
    pub(crate) fn producer_presented(&self) -> Result<(), FileRefusal> {
        self.derived()?.presented.store(true, Ordering::Release);
        Ok(())
    }
    /// The reviewed operation of the dispatch a gate is deciding, from the
    /// source the agent kept for that dispatch's review. `None` for every
    /// call that has none, which a gate then decides as it always does.
    pub(crate) fn of(tracked: &TrackedDispatch) -> Option<Arc<Self>> {
        tracked.review_source()?.downcast::<Self>()
    }

    /// The reviewed operation of a parked call, for the reader that asks a
    /// person about it.
    #[cfg(test)]
    pub(crate) fn of_parked(parked: &ActualParkedCall) -> Option<Arc<Self>> {
        parked.review_source()?.downcast::<Self>()
    }

    /// The derived operation, or the refusal that is the tool's answer.
    pub fn derived(&self) -> Result<&DerivedFileOperation, FileRefusal> {
        self.owner.live().map_err(FileRefusal::Stopped)?;
        self.outcome.as_ref().map_err(Clone::clone)
    }

    /// The call's arguments with the real content in its content fields, for
    /// a gate that decides on a call by its arguments and shows them. The
    /// recorded call is not changed.
    pub(crate) fn shown_arguments(&self) -> Result<serde_json::Value, FileRefusal> {
        self.derived()?.preview(|preview| preview.arguments.clone())
    }

    /// For the reader that asks a person about a parked call: the preview
    /// to show, if `shown` is the digest the agent carried for this very
    /// operation. A reader that does not take the preview has shown nothing,
    /// and the decision it returns is then not a decision on this operation.
    #[cfg(test)]
    pub(crate) fn shown_to_reader<T>(
        &self,
        shown: Option<[u8; 32]>,
        read: impl FnOnce(FilePreview<'_>) -> T,
    ) -> Result<T, FileRefusal> {
        let derived = self.derived()?;
        if shown != Some(derived.digest) {
            return Err(FileRefusal::NotApproved);
        }
        let result = derived.preview(read)?;
        derived.presented.store(true, Ordering::Release);
        Ok(result)
    }

    /// Whether the operation has written its file.
    pub fn is_done(&self) -> bool {
        matches!(&self.outcome, Ok(derived) if *derived.state.lock().unwrap() == State::Done)
    }

    /// The operation is over: refused, failed or replaced by what a person
    /// wrote. Nothing can come of it any more, and what it held is released.
    pub(crate) fn over(&self) {
        if let Ok(derived) = &self.outcome {
            derived.finish(State::Refused);
        }
    }

    /// End only this dispatch's actual still-derived review when a gate
    /// refuses before the producer receives it. A foreign, ended or spent
    /// operation keeps its own state and owners.
    pub(crate) fn refused_before_receive(self: &Arc<Self>, tracked: &TrackedDispatch) {
        let Some(plan) = tracked.planned() else {
            return;
        };
        if !plan.in_review()
            || !plan.same_plan(self.operation.plan())
            || !Self::of(tracked).is_some_and(|actual| Arc::ptr_eq(&actual, self))
        {
            return;
        }
        if let Ok(derived) = &self.outcome {
            let mut state = derived.state.lock().unwrap();
            if *state != State::Derived {
                return;
            }
            *state = State::Refused;
            derived.held.lock().unwrap().take();
        }
    }

    /// The decision comes from an independently registered actual inline
    /// input/policy producer, or the sealed
    /// input of a person who was asked about the parked call, was shown this
    /// operation and approved it as it is.
    fn decided(
        &self,
        derived: &DerivedFileOperation,
        invocation: &ActualToolInvocation,
    ) -> Result<(), FileRefusal> {
        let source = invocation
            .decision_source()
            .ok_or(FileRefusal::NotApproved)?;
        let plan = self.operation.plan();
        if let Some(input) = source.downcast::<crate::privacy_entry::InlineDecisionInput>() {
            let session = input.registered_session().ok_or(FileRefusal::NotApproved)?;
            if !Arc::ptr_eq(&session.inner, &self.owner) {
                return Err(FileRefusal::NotApproved);
            }
            return input.consume_file(&session, invocation, derived.digest);
        }
        if let Some(input) = source.downcast::<crate::privacy_entry::HumanDecisionInput>() {
            let approved = invocation.resumed()
                && invocation.decision_action() == Some(&HitlAction::Approve)
                && input.action() == Some(&HitlAction::Approve)
                && input.planned().same_plan(plan)
                && input.shown() == Some(derived.digest)
                && derived.presented.load(Ordering::Acquire);
            return match approved {
                true => Ok(()),
                false => Err(FileRefusal::NotApproved),
            };
        }
        Err(FileRefusal::NotApproved)
    }

    /// Writes the file. Called where the file tool performs its write, with
    /// the invocation the agent issued for the running call and the
    /// operation the boundary checked for it. Everything is checked again
    /// first: the conversation is live; the invocation is this plan's, with
    /// this source and this digest; the call runs exactly as it was planned
    /// and no person changed it; the decision is one on this operation; the
    /// user's policy still names the same data format for the destination;
    /// and the destination is what it was. The write goes through what was
    /// held on to, and runs once. Returns the number of bytes written.
    fn execute(
        self: &Arc<Self>,
        invocation: &ActualToolInvocation,
        running: &ApprovedOperation,
    ) -> Result<usize, FileRefusal> {
        let derived = self.derived()?;
        match *derived.state.lock().unwrap() {
            State::Derived => {}
            State::Done => return Err(FileRefusal::Spent),
            State::Refused => return Err(FileRefusal::NotApproved),
        }
        let plan = self.operation.plan();
        let this_source = invocation
            .review_source()
            .and_then(|source| source.downcast::<Self>())
            .is_some_and(|carried| Arc::ptr_eq(&carried, self));
        let as_planned = invocation.was_planned_as(plan)
            && this_source
            && invocation.shown() == Some(derived.digest)
            && running.is_for(invocation)
            && running.model().is(self.operation.model())
            && !running.edited_by_human()
            && !invocation.human_edited()
            && invocation.call() == plan.call();
        if !as_planned {
            return Err(FileRefusal::NotApproved);
        }
        self.decided(derived, invocation)?;
        // From here on the operation is over, whatever happens.
        let result = derived.write();
        derived.finish(match result {
            Ok(_) => State::Done,
            Err(_) => State::Refused,
        });
        // Completed IO keeps its actual return even if closure follows it.
        // Pre-effect liveness was checked by write; later stop is reported
        // separately through the real tool/fact/end/save path.
        result
    }
}

/// For the reader that asks a person about a parked call: the call to show.
/// For a file operation that was derived for the parked plan, that is the
/// call with the real content, taken from the operation's preview under the
/// digest the agent handed over with the parked call; taking it is what
/// makes the person's answer a decision on that operation. A call without
/// such an operation, and one whose operation could not be derived, is
/// shown as the model wrote it; the tool answers the latter with its
/// refusal. `None` when the digest is not the operation's: nobody is asked.
#[cfg(test)]
pub(crate) fn call_to_show(parked: &ActualParkedCall, shown: Option<[u8; 32]>) -> Option<ToolCall> {
    let call = parked.call();
    let Some(reviewed) = ReviewedFile::of_parked(parked) else {
        return Some(call.clone());
    };
    let taken = reviewed.shown_to_reader(shown, |preview| ToolCall {
        id: call.id.clone(),
        name: call.name.clone(),
        args: preview.arguments.clone(),
    });
    match taken {
        Ok(shown) => Some(shown),
        Err(_) if reviewed.outcome.is_err() => Some(call.clone()),
        Err(_) => None,
    }
}

/// The digest of the reviewed operation behind `source`, if `source` is one
/// and was made for exactly `planned`. For the boundary, which hands it to
/// the agent to carry beside the plan.
pub(super) fn reviewed_digest(
    planned: &ActualPlannedInvocation,
    source: &SourceRef,
) -> Option<[u8; 32]> {
    let reviewed = source.downcast::<ReviewedFile>()?;
    if !reviewed.operation.plan().same_plan(planned) {
        return None;
    }
    reviewed.derived().ok().map(|derived| derived.digest)
}

/// The file review of a privacy conversation, for its root agent and for
/// every agent it delegates to. The conversation's boundaries ask it, in the
/// agent's event before a review, what a planned write or edit will really
/// do. It holds the user's policy only weakly: the session that was built
/// for the conversation keeps the policy, and a review, a tool or a
/// delegated agent that outlives that session keeps nothing of it alive.
/// Holding a review grants no access to a file and approves nothing.
pub struct FileReview {
    policy: Weak<dyn CapturedUserPolicy>,
    root: PathBuf,
    /// The root agent's boundary, once the review is set on it. Every other
    /// agent's boundary is one of its family.
    conversation: OnceLock<Weak<ConversationBoundary>>,
}

impl FileReview {
    pub(crate) fn boundary_for_gates(&self) -> Option<Arc<ConversationBoundary>> {
        self.conversation.get()?.upgrade()
    }
    /// `policy` is the user's captured policy, which the caller keeps for
    /// the life of the session. `root` is the directory relative paths are
    /// resolved against, as the file tools resolve them.
    pub fn new(policy: &Arc<dyn CapturedUserPolicy>, root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            policy: Arc::downgrade(policy),
            root,
            conversation: OnceLock::new(),
        })
    }

    pub(super) fn attach(&self, boundary: &Arc<ConversationBoundary>) {
        let _ = self.conversation.set(Arc::downgrade(boundary));
    }

    /// The boundary of the agent a call runs in, from what its dispatch
    /// carries.
    fn boundary_for(&self, tracked: &TrackedDispatch) -> Option<Arc<ConversationBoundary>> {
        let conversation = tracked.boundary()?.conversation()?;
        self.conversation.get()?.upgrade()?.member(&conversation)
    }

    /// Called by a boundary of the conversation for a planned write or
    /// edit. `None` when the call names no placeholder: it then has nothing
    /// to derive and runs as it always does. Whether it names one is what
    /// the conversation recorded for the reply that wrote it, not how a
    /// string looks: an alias need not carry a marker.
    pub(super) fn review(
        &self,
        session: &PrivacySession,
        operation: Arc<PlannedOperation>,
    ) -> Option<Arc<ReviewedFile>> {
        let names_one = match operation.fields() {
            Ok(Some(fields)) => !fields.is_empty(),
            _ => true,
        };
        let marked = operation
            .call()
            .is_ok_and(|call| call.args.to_string().contains("pv1-"));
        if !names_one && !marked {
            return None;
        }
        let outcome = session.derive_file_operation(&self.policy, &operation, &self.root);
        Some(Arc::new(ReviewedFile {
            owner: session.inner.clone(),
            operation,
            outcome,
        }))
    }
}

/// The write of one reviewed operation, for the handler that performs it.
struct DerivedWrite {
    reviewed: Arc<ReviewedFile>,
    invocation: ActualToolInvocation,
    running: Arc<ApprovedOperation>,
}

tokio::task_local! {
    /// Set by [`RestoredFileTool`] around the tool it wraps, for the one
    /// call it runs.
    static DERIVED_WRITE: DerivedWrite;
}

/// For the handler of a file tool: the write of the operation that was
/// derived, reviewed and decided for the call running now, performed here in
/// place of the handler's own write, through what the derivation held on to.
/// `None` when the call has no such operation; the handler then writes as it
/// always does. Everything the handler does around its write stays its own:
/// its checkpoints, its diagnostics and its result.
pub(crate) fn authorized_write() -> Option<Result<usize, FileRefusal>> {
    DERIVED_WRITE
        .try_with(|write| write.reviewed.execute(&write.invocation, &write.running))
        .ok()
}

/// What a file tool says about a write, as pieces: its fixed words, the path
/// as the call wrote it, and whatever it added after that, which is host
/// text. Stated only when the outcome is exactly of that form.
fn state_outcome(tracked: &TrackedDispatch, call: &ToolCall, outcome: &ToolOutcome) {
    let Some(path) = call
        .args
        .get("file_path")
        .and_then(serde_json::Value::as_str)
    else {
        return;
    };
    let Some(added) = outcome
        .content
        .strip_prefix("Updated file ")
        .and_then(|rest| rest.strip_prefix(path))
    else {
        return;
    };
    let mut parts = vec![
        OutcomePart::Fixed(Arc::from("Updated file ")),
        OutcomePart::Argument {
            pointer: "/file_path".into(),
            text: Arc::from(path),
        },
    ];
    if !added.is_empty() {
        parts.push(OutcomePart::Read {
            what: "language diagnostics".into(),
            text: Arc::from(added),
        });
    }
    tracked.outcome_is(parts);
}

/// A file tool of a privacy conversation: `write_file` or `edit_file`, for
/// the root agent or a delegated one. It stands directly on the tool, behind
/// every gate. A call that names no placeholder runs the wrapped tool as
/// always. A call that names one runs only as the operation that was derived
/// for its plan, reviewed and decided: the wrapped tool does everything it
/// always does, and takes its write from [`authorized_write`].
///
/// A call whose operation did not write is answered with a refusal.
pub struct RestoredFileTool {
    inner: Arc<dyn Tool>,
    review: Arc<FileReview>,
}

impl RestoredFileTool {
    pub fn new(inner: Arc<dyn Tool>, review: Arc<FileReview>) -> Self {
        Self { inner, review }
    }
}

#[async_trait::async_trait]
impl Tool for RestoredFileTool {
    fn def(&self) -> ToolDef {
        self.inner.def()
    }

    /// Without a source view there is no plan, no review and no checked
    /// operation, so nothing can be restored. A call that names a
    /// placeholder of this conversation, with a marker or without one, is
    /// refused; so is every call once the conversation's root boundary is
    /// gone. Any other runs as always.
    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let name = self.inner.def().name;
        let conversation = self.review.conversation.get().and_then(Weak::upgrade);
        let clear = match conversation {
            Some(boundary) => !boundary.names_placeholder(&name, &args).await,
            None => false,
        };
        if !clear {
            return Ok(ToolOutcome::err(FileRefusal::NotApproved.to_string()));
        }
        self.inner.call(args, ctx).await
    }

    async fn call_with_view(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
        tracked: &TrackedDispatch,
    ) -> Result<ToolOutcome, AgentError> {
        let refused = |refusal: FileRefusal| Ok(ToolOutcome::err(refusal.to_string()));
        // The invocation the agent issued for this call, the agent it runs
        // in, and the operation that agent's boundary checked for exactly
        // this invocation. Without all three, nothing about the call is
        // known, and a file tool does not run on that.
        let Some(invocation) = tracked.invocation() else {
            return refused(FileRefusal::NotApproved);
        };
        let Some(boundary) = self.review.boundary_for(tracked) else {
            return refused(FileRefusal::NotApproved);
        };
        let running = boundary
            .operation_in_flight()
            .filter(|running| running.is_for(invocation));
        let Some(running) = running else {
            return refused(FileRefusal::NotApproved);
        };
        let Ok(call) = running.call().cloned() else {
            return refused(FileRefusal::NotApproved);
        };
        let reviewed = invocation
            .review_source()
            .and_then(|source| source.downcast::<ReviewedFile>());
        if running.edited_by_human() || invocation.human_edited() {
            // What a person wrote in place of the arguments is that
            // person's text. Nothing is restored into it, what was derived
            // from the model's call is over, and it runs as it is. The
            // conversation does not accept a person's text that holds one
            // of its aliases; should one stand here all the same, it is not
            // written to a file.
            if let Some(reviewed) = reviewed {
                reviewed.over();
            }
            if boundary.names_placeholder(&call.name, &args).await {
                return refused(FileRefusal::Excluded("arguments a person edited"));
            }
            return self.inner.call_with_view(args, ctx, tracked).await;
        }
        let Some(reviewed) = reviewed else {
            // Nothing was derived for this call. The conversation's own
            // record of the reply says whether it names a placeholder; if
            // it does, it was not reviewed as it would run, and does not.
            let names_one = !matches!(running.fields(), Ok(Some(fields)) if fields.is_empty())
                || args.to_string().contains("pv1-");
            if names_one {
                return refused(FileRefusal::NotApproved);
            }
            return self.inner.call_with_view(args, ctx, tracked).await;
        };
        // The operation was derived in this agent's own lane, and a refusal
        // of the derivation is the answer, before any effect.
        if !reviewed.operation.in_lane_of(&boundary) {
            return refused(FileRefusal::NotApproved);
        }
        if let Err(refusal) = reviewed.derived() {
            return refused(refusal);
        }
        let write = DerivedWrite {
            reviewed: reviewed.clone(),
            invocation: invocation.clone(),
            running,
        };
        let outcome = DERIVED_WRITE
            .scope(write, self.inner.call_with_view(args, ctx, tracked))
            .await;
        if !reviewed.is_done() {
            // The handler refused or the write failed. Whatever it was,
            // this operation is over.
            reviewed.over();
            return match outcome {
                Ok(outcome) if !outcome.is_error => refused(FileRefusal::NotApproved),
                other => other,
            };
        }
        let outcome = outcome?;
        state_outcome(tracked, &call, &outcome);
        Ok(outcome)
    }
}

#[cfg(all(test, unix))]
mod held_update_tests {
    use super::*;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    #[test]
    fn final_held_update_observes_the_retained_name_role_and_single_link() {
        let root = tempfile::tempdir().unwrap();
        let mut options = std::fs::OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let directory = options.open(root.path()).unwrap();
        let path = root.path().join("contacts.txt");
        std::fs::write(&path, "Contact owner@example.test.\n").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let bound = identity(&file.metadata().unwrap());
        let check = || validate_held_update(&file, &directory, path.file_name().unwrap(), bound);
        assert_eq!(check(), Ok(()));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(check(), Err(FileRefusal::Stale));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(check(), Ok(()));

        let link = root.path().join("also-contacts.txt");
        std::fs::hard_link(&path, &link).unwrap();
        assert_eq!(check(), Err(FileRefusal::Stale));
        std::fs::remove_file(link).unwrap();
        assert_eq!(check(), Ok(()));

        std::fs::rename(&path, root.path().join("old.txt")).unwrap();
        std::fs::write(&path, "Contact owner@example.test.\n").unwrap();
        assert_eq!(check(), Err(FileRefusal::Stale));
    }
}
