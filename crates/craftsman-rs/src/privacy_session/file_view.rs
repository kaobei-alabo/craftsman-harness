//! A file write or edit of this conversation as it will actually run.
//!
//! The model writes placeholders, also into the content of a file it asks
//! to write. The recorded call keeps them: it is the canonical call, it is
//! what the model is shown again, and nothing here changes it. What reaches
//! the disk is derived from it, separately, as an execution view: the same
//! call with the values put back into its content fields.
//!
//! The view is derived before anyone reviews the call, so that a person or a
//! policy decides on the content that will really be written. The decision
//! is bound to exactly that view by its digest. The view is checked again
//! where the file is written, and the write goes through what the view held
//! on to when it was derived: the open file for an edit, the absence of the
//! file for a create.
//!
//! What is restored: the `content` of `write_file`, and `old_string` and
//! `new_string` of `edit_file`, as plain text or as part of a complete JSON
//! document, and only where the user's policy names that data format for the
//! file. A path, a command, a URL, a tool or gadget argument, a credential,
//! a setting, an instruction or a schema is never restored. A file's name or
//! extension is not a rule. Where anything is missing or unclear, the tool
//! answers with a fixed refusal before any effect, and the call stays as the
//! model wrote it.
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

use super::view::{
    ApprovedOperation, ConversationBoundary, FieldPlace, ProposedOperation, SourceField,
};
use super::{CapturedUserPolicy, Inner, PrivacySession, SessionStop};
use crate::privacy_restore::{FileDataFormat, RestoreError, Restored, Restorer, file_candidate};
use craftsman_harness::provenance::{OutcomePart, TrackedDispatch};
use craftsman_harness::{AgentError, Tool, ToolCall, ToolCtx, ToolDef, ToolOutcome};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use zeroize::Zeroizing;

/// The most bytes of a file an edit is derived for.
const MAX_FILE: usize = 256 * 1024;

/// Why an operation was not derived, approved or run. Every one of them is
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
    Approved,
    Refused,
    Done,
}

/// One derived operation. It is made by [`PrivacySession::derive_file_operation`]
/// only, from what the committed reply proposed with the call.
pub struct DerivedFileOperation {
    owner: Arc<Inner>,
    operation: Arc<ProposedOperation>,
    /// The call's arguments with the content fields as they will be
    /// written, for a review that shows a call by its arguments.
    arguments: serde_json::Value,
    change: FileChange,
    path: PathBuf,
    destination: Mutex<Destination>,
    old: Option<Zeroizing<String>>,
    new: Zeroizing<String>,
    digest: [u8; 32],
    state: Mutex<State>,
}

/// What a reviewer is shown: the real content, never the placeholders.
pub struct FilePreview<'a> {
    pub change: FileChange,
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
fn restored_field(
    format: FileDataFormat,
    whole_document: bool,
    text: &str,
    fields: &[&SourceField],
) -> Result<Zeroizing<String>, FileRefusal> {
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
    let format = match whole_document {
        true => format,
        false => FileDataFormat::PlainText,
    };
    file_candidate(format, text, &restored).map_err(FileRefusal::Restore)
}

impl PrivacySession {
    /// Derives what a `write_file` or `edit_file` call will actually do,
    /// from the operation that was checked for the running call. `policy` is
    /// the user's captured policy and `root` the directory relative paths
    /// are resolved against, as the file tools resolve them.
    ///
    /// Nothing is written here. For an edit the file is opened and read, and
    /// stays open in the result.
    pub fn derive_file_operation(
        &self,
        policy: &dyn CapturedUserPolicy,
        operation: &Arc<ProposedOperation>,
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
        // The data format is the user's rule for this file, never a guess
        // from its name.
        let format = policy
            .restorable_file_data(self.workspace_sha256(), path_text)
            .ok_or(FileRefusal::NoRule)?;
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
        let (change, destination, old, new) = match call.name.as_str() {
            "write_file" => {
                let new =
                    restored_field(format, true, argument(&call.args, "content")?, &by_field[0])?;
                shown("content", &new);
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
                let parent = std::fs::metadata(parent)
                    .ok()
                    .filter(std::fs::Metadata::is_dir)
                    .ok_or_else(|| {
                        FileRefusal::Tool(format!(
                            "Error creating {}: its directory does not exist",
                            path.display()
                        ))
                    })?;
                (
                    FileChange::Create,
                    Destination::Absent {
                        parent: identity(&parent),
                    },
                    None,
                    new,
                )
            }
            _ => {
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
                shown("old_string", &old_string);
                shown("new_string", &new_string);
                let replace_all = call
                    .args
                    .get("replace_all")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if old_string.is_empty() {
                    return Err(FileRefusal::Tool(
                        "Error: old_string must not be empty".into(),
                    ));
                }
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
                if identity(&opened) != identity(&named) || opened.len() > MAX_FILE as u64 {
                    return Err(FileRefusal::Stale);
                }
                let mut text = Zeroizing::new(String::new());
                file.read_to_string(&mut text).map_err(unreadable)?;
                let count = text.matches(old_string.as_str()).count();
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
                let new = Zeroizing::new(match replace_all {
                    true => text.replace(old_string.as_str(), &new_string),
                    false => text.replacen(old_string.as_str(), &new_string, 1),
                });
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
                    Some(text),
                    new,
                )
            }
        };

        // What a decision is bound to: the call as the model wrote it, the
        // data format, the destination and both contents. The call's id
        // alone names nothing.
        let mut hasher = Sha256::new();
        hasher.update(b"craftsman-privacy-file-operation-v2\0");
        hasher.update([change as u8, format as u8]);
        let written = call.args.to_string();
        for piece in [
            call.id.as_bytes(),
            call.name.as_bytes(),
            written.as_bytes(),
            path.as_os_str().as_encoded_bytes(),
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
            operation: operation.clone(),
            arguments,
            change,
            path,
            destination: Mutex::new(destination),
            old,
            new,
            digest: hasher.finalize().into(),
            state: Mutex::new(State::Derived),
        })
    }
}

impl DerivedFileOperation {
    pub fn change(&self) -> FileChange {
        self.change
    }

    /// The call this was derived from, which stays as the model wrote it.
    pub fn operation(&self) -> &Arc<ProposedOperation> {
        &self.operation
    }

    /// What a reviewer is shown, for the duration of `read`.
    pub fn preview<T>(&self, read: impl FnOnce(FilePreview<'_>) -> T) -> Result<T, SessionStop> {
        self.owner.live()?;
        let result = read(FilePreview {
            change: self.change,
            path: &self.path,
            old: self.old.as_ref().map(|old| old.as_str()),
            new: &self.new,
            arguments: &self.arguments,
            digest: self.digest,
        });
        self.owner.live()?;
        Ok(result)
    }

    /// The decision that was actually taken on this operation. `shown` is
    /// the digest of the preview the decision was about. A decision about
    /// anything else approves nothing, and a refused or finished operation
    /// cannot be approved.
    pub(crate) fn approve(&self, shown: [u8; 32]) -> Result<(), FileRefusal> {
        self.owner.live().map_err(FileRefusal::Stopped)?;
        let mut state = self.state.lock().unwrap();
        if *state != State::Derived || shown != self.digest {
            return Err(FileRefusal::NotApproved);
        }
        *state = State::Approved;
        Ok(())
    }

    /// The decision of a gate that stands between the file tool and its
    /// handler: the call reached the handler with exactly the arguments this
    /// operation shows. A gate that refuses never lets the call get there,
    /// and arguments that are not the ones shown approve nothing.
    pub(crate) fn approve_reached(&self, arguments: &serde_json::Value) -> Result<(), FileRefusal> {
        if *arguments != self.arguments {
            return Err(FileRefusal::NotApproved);
        }
        self.approve(self.digest)
    }

    /// The call's arguments with the real content in its content fields, for
    /// a gate that reviews a call by the arguments it is handed.
    pub(crate) fn shown_arguments(&self) -> Result<serde_json::Value, SessionStop> {
        self.owner.live()?;
        Ok(self.arguments.clone())
    }

    /// Whether the operation has written its file.
    pub fn is_done(&self) -> bool {
        *self.state.lock().unwrap() == State::Done
    }

    /// Whether this was derived from exactly `call`.
    fn is_for(&self, call: &ToolCall) -> bool {
        matches!(self.operation.call(), Ok(derived) if derived == call)
    }

    /// Whether the operation was refused or has run. Nothing can come of it
    /// any more.
    fn is_over(&self) -> bool {
        matches!(*self.state.lock().unwrap(), State::Refused | State::Done)
    }

    /// The operation was refused: by a person, a policy or a mode. It can
    /// no longer be approved or run.
    pub fn refuse(&self) {
        let mut state = self.state.lock().unwrap();
        if *state != State::Done {
            *state = State::Refused;
        }
    }

    /// Writes the file. Called where the tool runs, with the operation that
    /// is checked for the call running now. Everything is checked again
    /// first: the conversation is live, this was approved as shown, the
    /// running call is the one this was derived from, and the destination is
    /// what it was. The write goes through what was held on to. It runs
    /// once. Returns the number of bytes written.
    pub fn execute(
        &self,
        in_flight: Option<&Arc<ApprovedOperation>>,
    ) -> Result<usize, FileRefusal> {
        self.owner.live().map_err(FileRefusal::Stopped)?;
        let mut state = self.state.lock().unwrap();
        match *state {
            State::Approved => {}
            State::Done => return Err(FileRefusal::Spent),
            State::Derived | State::Refused => return Err(FileRefusal::NotApproved),
        }
        // The call that runs now is the call this was derived from: asked
        // for by the same committed reply, with the same arguments, and not
        // changed by a person since.
        let same = in_flight.is_some_and(|running| {
            running.model().is(self.operation.model())
                && !running.edited_by_human()
                && matches!((running.call(), self.operation.call()),
                    (Ok(running), Ok(derived)) if running == derived)
        });
        if !same {
            return Err(FileRefusal::NotApproved);
        }
        // From here on the operation is spent, whatever happens.
        *state = State::Refused;
        let mut destination = self.destination.lock().unwrap();
        let failed = |error: std::io::Error| {
            FileRefusal::Tool(format!("Error writing {}: {error}", self.path.display()))
        };
        match &mut *destination {
            Destination::Absent { parent } => {
                let now = self
                    .path
                    .parent()
                    .and_then(|parent| std::fs::metadata(parent).ok());
                if parent.is_some() && now.as_ref().and_then(identity) != *parent {
                    return Err(FileRefusal::Stale);
                }
                // The create itself refuses a file that appeared since.
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&self.path)
                    .map_err(|error| match error.kind() {
                        std::io::ErrorKind::AlreadyExists => FileRefusal::Stale,
                        _ => failed(error),
                    })?;
                file.write_all(self.new.as_bytes()).map_err(failed)?;
                file.flush().map_err(failed)?;
            }
            Destination::Existing {
                file,
                identity: held,
            } => {
                // The path still names the file that was opened, and the
                // file still holds what was read and shown.
                let named =
                    std::fs::symlink_metadata(&self.path).map_err(|_| FileRefusal::Stale)?;
                if !named.is_file() || identity(&named) != *held {
                    return Err(FileRefusal::Stale);
                }
                let mut current = Zeroizing::new(String::new());
                file.seek(SeekFrom::Start(0)).map_err(failed)?;
                file.read_to_string(&mut current)
                    .map_err(|_| FileRefusal::Stale)?;
                if Some(current.as_str()) != self.old.as_ref().map(|old| old.as_str()) {
                    return Err(FileRefusal::Stale);
                }
                file.seek(SeekFrom::Start(0)).map_err(failed)?;
                file.set_len(0).map_err(failed)?;
                file.write_all(self.new.as_bytes()).map_err(failed)?;
                file.flush().map_err(failed)?;
            }
        }
        *state = State::Done;
        Ok(self.new.len())
    }
}

/// The most derived operations that wait for their call to run.
const MAX_WAITING: usize = 16;

/// The file review of a privacy conversation, for its root agent and for
/// every agent it delegates to. Whoever reviews a write or an edit asks it
/// what the call will really do, and the file tools run what was derived
/// and approved. An agent is found by its own boundary; the review of one
/// agent's call is never used for another's. Holding a review, a session or
/// a record grants no access to a file and approves nothing.
pub struct FileReview {
    session: Arc<PrivacySession>,
    /// The root agent's boundary. Every other agent's is one of its family.
    boundary: Arc<ConversationBoundary>,
    policy: Arc<dyn CapturedUserPolicy>,
    root: PathBuf,
    /// Derived operations by agent, from the review of a call until that
    /// call runs. A parked call keeps its entry across the suspension.
    waiting: Mutex<Vec<(Weak<ConversationBoundary>, Arc<DerivedFileOperation>)>>,
}

impl FileReview {
    pub fn new(
        session: Arc<PrivacySession>,
        boundary: Arc<ConversationBoundary>,
        policy: Arc<dyn CapturedUserPolicy>,
        root: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            session,
            boundary,
            policy,
            root,
            waiting: Mutex::new(Vec::new()),
        })
    }

    /// The boundary of the agent a call runs in, from what its dispatch
    /// carries.
    fn boundary_for(&self, tracked: &TrackedDispatch) -> Option<Arc<ConversationBoundary>> {
        let conversation = tracked.boundary()?.conversation()?;
        self.boundary.member(&conversation)
    }

    /// Called for the root agent before anything reviews `call`. For a file
    /// write or edit that names a placeholder it derives what will really be
    /// written and keeps it for the review and for the tool. `None` for
    /// every other call, which runs as it always does. A refusal is the
    /// tool's answer: nothing was touched, and the call stays as the model
    /// wrote it.
    pub async fn propose(
        &self,
        call: &ToolCall,
    ) -> Result<Option<Arc<DerivedFileOperation>>, FileRefusal> {
        self.propose_in(&self.boundary, call).await
    }

    /// As [`propose`](Self::propose), for the agent whose boundary this is.
    async fn propose_in(
        &self,
        boundary: &Arc<ConversationBoundary>,
        call: &ToolCall,
    ) -> Result<Option<Arc<DerivedFileOperation>>, FileRefusal> {
        if !matches!(call.name.as_str(), "write_file" | "edit_file") {
            return Ok(None);
        }
        let proposed = boundary
            .proposed_operation(call)
            .await
            .map_err(|_| FileRefusal::Stopped(SessionStop::Unbound))?;
        // Whether the call names a placeholder is what the conversation
        // recorded for the reply that wrote it, not how a string looks: an
        // alias need not carry a marker. A call that names none has nothing
        // to derive.
        let names_one = match proposed.fields() {
            Ok(Some(fields)) => !fields.is_empty(),
            _ => true,
        };
        if !names_one && !call.args.to_string().contains("pv1-") {
            return Ok(None);
        }
        let derived = Arc::new(self.session.derive_file_operation(
            self.policy.as_ref(),
            &proposed,
            &self.root,
        )?);
        let mut waiting = self.waiting.lock().unwrap();
        // What is over, what belongs to an agent that is gone and what was
        // derived for this call before are dropped, with the files they
        // hold open.
        waiting.retain(|(agent, known)| {
            agent.strong_count() > 0
                && !known.is_over()
                && !(is_agent(agent, boundary) && known.is_for(call))
        });
        if waiting.len() >= MAX_WAITING {
            waiting.remove(0).1.refuse();
        }
        waiting.push((Arc::downgrade(boundary), derived.clone()));
        Ok(Some(derived))
    }

    /// The derived operation that waits for the root agent's call `id`, for
    /// the interface that shows it to a person.
    pub fn waiting(&self, id: &str) -> Option<Arc<DerivedFileOperation>> {
        let waiting = self.waiting.lock().unwrap();
        for (agent, derived) in waiting.iter() {
            let named = matches!(derived.operation.call(), Ok(call) if call.id == id);
            if named && is_agent(agent, &self.boundary) {
                return Some(derived.clone());
            }
        }
        None
    }

    /// The operation that waits for exactly `call` of the agent whose
    /// boundary this is.
    fn take(
        &self,
        boundary: &Arc<ConversationBoundary>,
        call: &ToolCall,
    ) -> Option<Arc<DerivedFileOperation>> {
        let mut waiting = self.waiting.lock().unwrap();
        let at = waiting
            .iter()
            .position(|(agent, derived)| is_agent(agent, boundary) && derived.is_for(call))?;
        Some(waiting.remove(at).1)
    }
}

/// Whether `agent` is this boundary: the same object, not an equal one. The
/// list keeps the object's place alive, so its address names nothing else.
fn is_agent(agent: &Weak<ConversationBoundary>, boundary: &Arc<ConversationBoundary>) -> bool {
    std::ptr::eq(agent.as_ptr(), Arc::as_ptr(boundary))
}

/// The write of one derived operation, for the handler that performs it.
struct DerivedWrite {
    derived: Arc<DerivedFileOperation>,
    running: Arc<ApprovedOperation>,
}

tokio::task_local! {
    /// Set by [`RestoredFileTool`] around the tool it wraps, for the one
    /// call it runs.
    static DERIVED_WRITE: DerivedWrite;
}

/// For the handler of a file tool: the write of the operation that was
/// derived and approved for the call running now, performed here in place of
/// the handler's own write, through what the derivation held on to. `None`
/// when the call has no derived operation; the handler then writes as it
/// always does. Everything the handler does around its write stays its own:
/// its checkpoints, its diagnostics and its result.
pub(crate) fn authorized_write() -> Option<Result<usize, FileRefusal>> {
    DERIVED_WRITE
        .try_with(|write| write.derived.execute(Some(&write.running)))
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
/// the root agent or a delegated one. A call that names no placeholder runs
/// the wrapped tool as always. A call that names one runs only as it was
/// derived, shown and approved: the wrapped tool does everything it always
/// does, and takes its write from [`authorized_write`].
///
/// For the root agent the approval gate derives and decides before the tool
/// runs. A delegated agent's gate is inside its tool, so the operation is
/// derived here, before that gate, and the gate is handed the arguments
/// with the real content: its rules, its prompt and its activity line are
/// about what will really be written. [`ReachedFileTool`] stands behind that
/// gate and records that the gate let the call through. The recorded call
/// is not changed in either case.
///
/// A call whose derived operation did not write is answered with a refusal.
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

    /// Without a source view there is no checked operation, so nothing can
    /// be restored. A call that names a placeholder, with a marker or
    /// without one, is refused; any other runs as always.
    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let name = self.inner.def().name;
        if self.review.boundary.names_placeholder(&name, &args).await {
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
        let review = &self.review;
        // The agent this call runs in, the invocation the agent issued for
        // it, and the operation that was checked for that invocation.
        // Without all three, nothing about the call is known, and a file
        // tool does not run on that.
        let Some(boundary) = review.boundary_for(tracked) else {
            return refused(FileRefusal::NotApproved);
        };
        let running = tracked.invocation().and_then(|invocation| {
            boundary
                .operation_in_flight()
                .filter(|running| running.is_for(invocation))
        });
        let Some(running) = running else {
            return refused(FileRefusal::NotApproved);
        };
        let Ok(call) = running.call().cloned() else {
            return refused(FileRefusal::NotApproved);
        };
        let waiting = review.take(&boundary, &call);
        if running.edited_by_human() {
            // What a person wrote in place of the arguments is that
            // person's text. Nothing is restored into it, and it runs as it
            // is. The conversation does not accept a person's text that
            // holds one of its aliases; should one stand here all the same,
            // it is not written to a file.
            if let Some(derived) = waiting {
                derived.refuse();
            }
            if boundary.names_placeholder(&call.name, &args).await {
                return refused(FileRefusal::Excluded("arguments a person edited"));
            }
            return self.inner.call_with_view(args, ctx, tracked).await;
        }
        // The conversation's own record of the reply says whether the call
        // names a placeholder.
        let names_one = !matches!(running.fields(), Ok(Some(fields)) if fields.is_empty())
            || args.to_string().contains("pv1-");
        let (derived, handed) = match waiting {
            Some(derived) => (derived, args),
            None if !names_one => return self.inner.call_with_view(args, ctx, tracked).await,
            // The root agent's gate derives before the tool runs. A call
            // that got here without that was not reviewed as it would run.
            None if boundary.is_root() => return refused(FileRefusal::NotApproved),
            // A delegated agent's gate is inside this tool: derive now,
            // before it, and hand that gate what will really be written.
            None => match review.propose_in(&boundary, &call).await {
                Ok(Some(_)) => match review.take(&boundary, &call) {
                    Some(derived) => match derived.shown_arguments() {
                        Ok(shown) => (derived, shown),
                        Err(stop) => return refused(FileRefusal::Stopped(stop)),
                    },
                    None => return refused(FileRefusal::NotApproved),
                },
                Ok(None) => return refused(FileRefusal::NotApproved),
                Err(refusal) => return refused(refusal),
            },
        };
        let write = DerivedWrite {
            derived: derived.clone(),
            running,
        };
        let outcome = DERIVED_WRITE
            .scope(write, self.inner.call_with_view(handed, ctx, tracked))
            .await;
        if !derived.is_done() {
            // A gate refused, the handler refused, or the write failed.
            // Whatever it was, this operation is over.
            derived.refuse();
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

/// A delegated agent's file tool as that agent's gate reaches it. It stands
/// directly behind the gate. When the gate lets through a call for which an
/// operation was derived, with exactly the arguments that operation shows,
/// that is the gate's decision on what was shown, and it is recorded here.
/// A gate that refuses never calls this. Every other call passes through.
pub struct ReachedFileTool {
    inner: Arc<dyn Tool>,
}

impl ReachedFileTool {
    pub fn new(inner: Arc<dyn Tool>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl Tool for ReachedFileTool {
    fn def(&self) -> ToolDef {
        self.inner.def()
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let reached = DERIVED_WRITE.try_with(|write| write.derived.approve_reached(&args));
        if let Ok(Err(refusal)) = reached {
            return Ok(ToolOutcome::err(refusal.to_string()));
        }
        self.inner.call(args, ctx).await
    }
}
