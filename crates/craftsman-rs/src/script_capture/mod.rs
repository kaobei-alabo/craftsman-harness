//! Private normal-session reads and complete historical CRUD previews.
//!
//! This app binary has no public script selector or route. Native handles and
//! constructors are private; only actual TUI/ACP construction can select the
//! owner. Source consent, VM execution, task roots, review and apply are later
//! work. A preview never proves current staleness or permits a native effect.

mod native;
mod preview;

use crate::agent::{FrontendCaptureBinding, FrontendCaptureContext, Session};
use crate::hitl::{ApprovalState, ToolPermission};
use craftsman_harness::{AgentState, Backend, Tool, TurnCancellation};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex, Weak};
use thiserror::Error;
use tokio::task::JoinHandle;

pub use preview::{Operation, ProspectivePreview};

pub const MAX_TARGETS: usize = 128;
pub const MAX_FILE_BYTES: usize = 64 * 1024;
pub const MAX_TOTAL_BYTES: usize = 192 * 1024;
pub const MAX_PATH_BYTES: usize = 32 * 1024;
pub const MAX_PREVIEW_BYTES: usize = 1024 * 1024;

/// Fixed diagnostics contain no paths, contents, configuration or OS details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CaptureError {
    #[error("The normal-session capture scope is unavailable or unsupported.")]
    Scope,
    #[error("The selected target name is invalid or ambiguous.")]
    Target,
    #[error("Configuration, credential or runtime capture is denied.")]
    Denied,
    #[error("The exact existing target parent is unavailable.")]
    Parent,
    #[error("The selected entry is not a supported regular single-link file.")]
    Unsupported,
    #[error("The native target or retained directory changed during capture.")]
    Changed,
    #[error("The complete capture or prospective preview exceeds its bounds.")]
    Bounds,
    #[error("This capture belongs to another actual normal session.")]
    Foreign,
    #[error("This capture generation is no longer eligible.")]
    Revoked,
    #[error("The previous native capture has not returned.")]
    Busy,
    #[error("The owned native capture worker failed.")]
    Worker,
    #[error("The native capture could not complete.")]
    Io,
    #[error("The complete final-candidate data is invalid or unsupported.")]
    Candidate,
}

type Result<T> = std::result::Result<T, CaptureError>;

struct Lifetime {
    generation: u64,
    serial: u64,
    closed: bool,
    active: bool,
    workers: Vec<JoinHandle<()>>,
}

/// An unbound capture can be produced only from the shared builder's witness.
/// There is no Config/Prepared/path/permission/hash constructor.
pub(crate) struct PendingCapture {
    root: Arc<native::ReadRoot>,
    approval: Arc<Mutex<ApprovalState>>,
    frontend: Arc<()>,
    nonce: [u8; 16],
    construction_stop: Option<TurnCancellation>,
}

impl PendingCapture {
    pub(crate) async fn from_actual(actual: FrontendCaptureContext) -> Result<Self> {
        let stop = TurnCancellation::current();
        if stop.as_ref().is_some_and(TurnCancellation::is_cancelled) {
            return Err(CaptureError::Revoked);
        }
        let observed_stop = stop.clone();
        // Only explicit root/origin walks occur. No listing, search or Git work.
        // As with the existing native readers, dropping this wait cannot
        // interrupt a started blocking syscall. The closure retains its owned
        // inputs/handles through actual return; it cannot publish an authority.
        let prepared = tokio::task::spawn_blocking(move || {
            native::ReadRoot::capture_actual(actual, observed_stop.as_ref())
        })
        .await
        .map_err(|_| CaptureError::Worker);
        if stop.as_ref().is_some_and(TurnCancellation::is_cancelled) {
            return Err(CaptureError::Revoked);
        }
        let (root, approval, frontend) = prepared??;
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).map_err(|_| CaptureError::Worker)?;
        Ok(Self {
            root: Arc::new(root),
            approval,
            frontend,
            nonce,
            construction_stop: stop,
        })
    }

    pub(crate) fn bind(self, actual: FrontendCaptureBinding) -> SessionCapture {
        let (state, backend, identity_tool) = actual.into_parts();
        SessionCapture {
            context: Arc::new(CaptureContext {
                root: self.root,
                approval: self.approval,
                frontend: self.frontend,
                state: Arc::downgrade(&state),
                backend,
                identity_tool,
                nonce: self.nonce,
                construction_stop: self.construction_stop,
                lifetime: Mutex::new(Lifetime {
                    generation: 1,
                    serial: 0,
                    closed: false,
                    active: false,
                    workers: Vec::new(),
                }),
                #[cfg(test)]
                hook: Mutex::new(None),
            }),
        }
    }
}

struct CaptureContext {
    root: Arc<native::ReadRoot>,
    approval: Arc<Mutex<ApprovalState>>,
    frontend: Arc<()>,
    state: Weak<Mutex<AgentState>>,
    backend: Arc<dyn Backend>,
    identity_tool: Arc<dyn Tool>,
    nonce: [u8; 16],
    construction_stop: Option<TurnCancellation>,
    lifetime: Mutex<Lifetime>,
    #[cfg(test)]
    hook: Mutex<Option<native::Hook>>,
}

/// Owned only by the actual Session. Drop seals admission; explicit frontend
/// close also awaits retained workers. Neither operation asserts OS quiescence.
pub struct SessionCapture {
    context: Arc<CaptureContext>,
}

impl Drop for SessionCapture {
    fn drop(&mut self) {
        self.context.seal();
    }
}

impl CaptureContext {
    fn matches(&self, actual: &Session) -> Result<()> {
        let selected = actual
            .script_capture
            .as_ref()
            .ok_or(CaptureError::Foreign)?;
        if !std::ptr::eq(self, selected.context.as_ref())
            || !Weak::ptr_eq(&self.state, &Arc::downgrade(&actual.agent.state))
            || !Arc::ptr_eq(&self.backend, &selected.context.backend)
            || !Arc::ptr_eq(&self.frontend, &selected.context.frontend)
            || !actual
                .agent
                .registered_tool_matches("ask_user", &self.identity_tool)
        {
            return Err(CaptureError::Foreign);
        }
        Ok(())
    }

    fn check(&self, generation: u64, stop: Option<&TurnCancellation>) -> Result<()> {
        let lifetime = self.lifetime.lock().map_err(|_| CaptureError::Worker)?;
        if lifetime.closed || lifetime.generation != generation {
            return Err(CaptureError::Revoked);
        }
        drop(lifetime);
        if stop.is_some_and(TurnCancellation::is_cancelled)
            || self
                .construction_stop
                .as_ref()
                .is_some_and(TurnCancellation::is_cancelled)
        {
            return Err(CaptureError::Revoked);
        }
        let policy = self.approval.lock().map_err(|_| CaptureError::Worker)?;
        if policy.permission("read_file", &json!({})) != ToolPermission::Allow {
            return Err(CaptureError::Denied);
        }
        Ok(())
    }

    fn seal(&self) {
        // Poison recovery is denial/cleanup only, never renewed admission.
        let mut lifetime = self.lifetime.lock().unwrap_or_else(|p| p.into_inner());
        lifetime.closed = true;
    }

    fn validate_snapshot(&self, actual: &Session, snapshots: &SnapshotData) -> Result<()> {
        self.matches(actual)?;
        if !std::ptr::eq(self, snapshots.context.as_ref()) {
            return Err(CaptureError::Foreign);
        }
        self.check(snapshots.generation, None)
    }
}

impl SessionCapture {
    /// Only selected relative names are inspected, and the whole request is
    /// admitted before native content copying. No ambient Backend read occurs.
    pub async fn capture(&self, actual: &Session, targets: &[&str]) -> Result<SnapshotSet> {
        self.context.matches(actual)?;
        let targets = native::SelectedTargets::admit(targets)?;
        self.context.root.check_names(&targets)?;
        let stop = TurnCancellation::current();
        let (send, receive) = tokio::sync::oneshot::channel();
        {
            // Admission and join insertion share one lock with close. Close
            // cannot miss a just-started worker, and no await holds this lock.
            let mut lifetime = self
                .context
                .lifetime
                .lock()
                .map_err(|_| CaptureError::Worker)?;
            if lifetime.closed {
                return Err(CaptureError::Revoked);
            }
            if lifetime.active {
                return Err(CaptureError::Busy);
            }
            if stop.as_ref().is_some_and(TurnCancellation::is_cancelled)
                || self
                    .context
                    .construction_stop
                    .as_ref()
                    .is_some_and(TurnCancellation::is_cancelled)
            {
                return Err(CaptureError::Revoked);
            }
            lifetime.workers.retain(|worker| !worker.is_finished());
            lifetime.serial = lifetime.serial.checked_add(1).ok_or(CaptureError::Bounds)?;
            let generation = lifetime.generation;
            let serial = lifetime.serial;
            lifetime.active = true;
            let context = self.context.clone();
            let reader = context.clone();
            let observed_stop = stop.clone();
            #[cfg(test)]
            let hook = context
                .hook
                .lock()
                .map_err(|_| CaptureError::Worker)?
                .clone();
            let blocking = tokio::task::spawn_blocking(move || {
                reader.check(generation, observed_stop.as_ref())?;
                reader.root.capture(
                    targets,
                    || reader.check(generation, observed_stop.as_ref()),
                    #[cfg(test)]
                    hook.as_ref(),
                )
            });
            lifetime.workers.push(tokio::spawn(async move {
                let result = blocking
                    .await
                    .map_err(|_| CaptureError::Worker)
                    .and_then(|v| v);
                // Check after the real worker returns, including failures.
                // Dropped receivers cannot abandon join ownership or expose data.
                let result = context.check(generation, stop.as_ref()).and_then(|()| {
                    result.and_then(|records| {
                        SnapshotData::from_actual(context.clone(), generation, serial, records)
                    })
                });
                if let Ok(mut lifetime) = context.lifetime.lock() {
                    lifetime.active = false;
                }
                let _ = send.send(result.map(|inner| SnapshotSet {
                    inner: Arc::new(inner),
                }));
            }));
        }
        let snapshots = receive.await.map_err(|_| CaptureError::Worker)??;
        self.context.validate_snapshot(actual, &snapshots.inner)?;
        Ok(snapshots)
    }

    /// Reconfiguration/thread operations invalidate old generations before
    /// their effects. Already-started native work keeps its actual owner.
    pub(crate) fn invalidate(&self) {
        let mut lifetime = self
            .context
            .lifetime
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match lifetime.generation.checked_add(1) {
            Some(generation) => lifetime.generation = generation,
            None => lifetime.closed = true,
        }
    }

    pub(crate) fn revoke(&self) {
        self.context.seal();
    }

    pub(crate) async fn close(&self) {
        let workers = {
            let mut lifetime = self
                .context
                .lifetime
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            lifetime.closed = true;
            std::mem::take(&mut lifetime.workers)
        };
        for worker in workers {
            let _ = worker.await;
        }
        // Existing frontend shutdown bounds still govern the actual wait.
        // A dropped/expired close future is not a native containment receipt.
    }

    /// Strict candidate data produces a complete, host-derived historical plan.
    /// There is no current re-read, consent callback, writer or apply method.
    pub fn preview(
        &self,
        actual: &Session,
        snapshots: &SnapshotSet,
        final_candidates: &[u8],
    ) -> Result<ProspectivePreview> {
        self.context.validate_snapshot(actual, &snapshots.inner)?;
        let preview = preview::derive(snapshots.inner.clone(), final_candidates)?;
        self.context.validate_snapshot(actual, &snapshots.inner)?;
        Ok(preview)
    }

    #[cfg(test)]
    fn set_hook(&self, hook: native::Hook) {
        *self.context.hook.lock().unwrap() = Some(hook);
    }

    #[cfg(test)]
    fn clear_hook(&self) {
        *self.context.hook.lock().unwrap() = None;
    }
}

struct SnapshotData {
    context: Arc<CaptureContext>,
    generation: u64,
    serial: u64,
    records: Vec<native::TargetSnapshot>,
    digest: [u8; 32],
}

impl SnapshotData {
    fn from_actual(
        context: Arc<CaptureContext>,
        generation: u64,
        serial: u64,
        records: Vec<native::TargetSnapshot>,
    ) -> Result<Self> {
        let binding = json!({
            "version": 1,
            "session": hex(&context.nonce),
            "generation": generation,
            "capture": serial,
            "scope": context.root.binding(),
            "targets": records.iter().map(native::TargetSnapshot::binding).collect::<Vec<_>>(),
        });
        let bytes = serde_json::to_vec(&binding).map_err(|_| CaptureError::Worker)?;
        if bytes.len() > MAX_PREVIEW_BYTES {
            return Err(CaptureError::Bounds);
        }
        let digest = Sha256::digest(&bytes).into();
        Ok(Self {
            context,
            generation,
            serial,
            records,
            digest,
        })
    }

    fn record(&self, name: &str) -> Result<(usize, &native::TargetSnapshot)> {
        self.records
            .iter()
            .enumerate()
            .find(|(_, record)| record.name() == name)
            .ok_or(CaptureError::Candidate)
    }

    fn check(&self, actual: &Session) -> Result<()> {
        self.context.validate_snapshot(actual, self)
    }
}

/// Immutable host originals. Omitted, refused and unsupported targets have no
/// record and can never be inferred absent from a missing map entry.
pub struct SnapshotSet {
    inner: Arc<SnapshotData>,
}

impl SnapshotSet {
    pub fn target_count(&self, actual: &Session) -> Result<usize> {
        self.inner.check(actual)?;
        Ok(self.inner.records.len())
    }

    /// Some includes a zero-byte PRESENT file; None is exact checked absence.
    /// Uncaptured targets are errors, never None.
    pub fn original_bytes<'a>(
        &'a self,
        actual: &Session,
        target: &str,
    ) -> Result<Option<&'a [u8]>> {
        self.inner.check(actual)?;
        let (_, record) = self.inner.record(target)?;
        let bytes = record.bytes();
        self.inner.check(actual)?;
        Ok(bytes)
    }

    pub fn historical_facts(&self, actual: &Session, target: &str) -> Result<Value> {
        self.inner.check(actual)?;
        let (_, record) = self.inner.record(target)?;
        let facts = record.binding();
        self.inner.check(actual)?;
        Ok(facts)
    }

    pub fn capture_digest(&self, actual: &Session) -> Result<[u8; 32]> {
        self.inner.check(actual)?;
        Ok(self.inner.digest)
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}

#[cfg(test)]
pub(crate) mod tests;
