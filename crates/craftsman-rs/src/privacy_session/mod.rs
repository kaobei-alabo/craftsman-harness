//! Private admission owner for one conversation: the only place raw text is
//! turned into text with placeholders.
//!
//! Nothing selects this module. No frontend, configuration key, command, key
//! source, provider, tool, approval or display path uses it yet, and normal
//! sessions are unchanged. It joins objects that already exist: one live
//! vault scope, the send guard minted from that scope, and one selected
//! detector.
//!
//! One admission is one serialized operation:
//!
//! 1. Refuse, before any effect, text this owner cannot admit safely.
//! 2. Ask the detector for spans and check them against the text.
//! 3. Refuse, still before any effect, what the vault would refuse for the
//!    content: conflicting categories, a known value left uncovered, a scope
//!    that would overflow.
//! 4. Save the mappings in the vault, then admit them to the send guard.
//! 5. Release the admitted text only after both have returned.
//!
//! Raw text that already contains a placeholder issued in this scope is
//! refused as ambiguous. It is never restored and masked again, and nothing
//! here treats a channel, a tool name, a flag, a digest or a spelling as proof
//! that text was already admitted. Text that the model produced, and mixed
//! text, need their own provenance and are not handled here.

#[cfg(test)]
mod tests;

use crate::privacy_hook::{Detector, HookError, MAX_TEXT_BYTES, TextSpan};
use crate::privacy_send::{SendGuard, SendRefusal};
use crate::privacy_vault::{
    Category, DeclaredIdentifiers, DetectorReport, DetectorSpan, HostNamespace, MappingSession,
    PrivacyError, ResolutionRecords, Vault,
};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use zeroize::Zeroizing;

/// The vault's own per-scope ceiling. A scope that would exceed it is refused
/// here before any effect; the vault's whole-file ceilings stay with the vault.
const SCOPE_MAPPINGS: usize = 512;
/// No placeholder is longer than this, which bounds the admitted text.
const MAX_PLACEHOLDER_BYTES: usize = 256;

/// A refusal of one text, decided before any vault effect. The session stays
/// usable and nothing was saved or released.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdmitRefusal {
    #[error("The text is too large to be admitted as one unit.")]
    TooLarge,
    #[error("The text contains a privacy placeholder already issued here.")]
    IssuedPlaceholder,
    #[error("The text contains a known private value that was not reported.")]
    UncoveredValue,
    #[error("The text reports one value under two categories.")]
    ConflictingCategory,
    #[error("This conversation has reached its private value capacity.")]
    ScopeFull,
}

/// Why the whole session can no longer admit, restore or send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SessionStop {
    #[error("The privacy session is closed.")]
    Closed,
    #[error("The privacy session scope could not be created.")]
    Scope,
    #[error("The private vault refused or failed.")]
    Vault(PrivacyError),
    #[error("The private send guard refused.")]
    Guard(SendRefusal),
    #[error("The selected detector source is unavailable or not approved.")]
    Detector,
}

/// The outcome of one refused admission. The three kinds are not
/// interchangeable: only `Stopped` ends the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdmitError {
    #[error(transparent)]
    Refused(AdmitRefusal),
    /// The approved detector gave no usable result for this text. Nothing was
    /// saved or released; the session stays usable.
    #[error(transparent)]
    Detector(HookError),
    #[error(transparent)]
    Stopped(SessionStop),
}

/// The vault scope of one actual conversation: a fresh random conversation
/// identity and the workspace it belongs to. Main, delegated and compaction
/// lanes of that conversation share it. A new conversation gets a new scope,
/// so each one counts against the vault's fixed scope capacity; nothing here
/// prunes, reuses or raises it.
pub struct ConversationScope {
    namespace: HostNamespace,
}

impl ConversationScope {
    /// `workspace` is the session's absolute workspace root. It is resolved
    /// to its canonical path once; the scope keeps only that path's SHA-256.
    pub fn fresh(workspace: &Path) -> Result<Self, SessionStop> {
        if !workspace.is_absolute() {
            return Err(SessionStop::Scope);
        }
        let workspace = std::fs::canonicalize(workspace).map_err(|_| SessionStop::Scope)?;
        let mut digest = Sha256::new();
        digest.update(b"craftsman/privacy-session/workspace/v1\0");
        digest.update(workspace.as_os_str().as_encoded_bytes());
        let project = format!("{:x}", digest.finalize());
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| SessionStop::Scope)?;
        let thread: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        Self::labelled(&project, &thread)
    }

    fn labelled(project: &str, thread: &str) -> Result<Self, SessionStop> {
        HostNamespace::supplied_by_host(
            "craftsman/privacy-session/v1",
            project,
            thread,
            "conversation",
            "main+delegated+compaction/v1",
        )
        .map(|namespace| Self { namespace })
        .map_err(|_| SessionStop::Scope)
    }
}

struct Ledger {
    /// Every authenticated mapping of the scope, read from the vault.
    records: ResolutionRecords,
    /// Their placeholders, which are not private.
    placeholders: Vec<String>,
}

impl Ledger {
    fn read(records: ResolutionRecords) -> Result<Self, PrivacyError> {
        let mut placeholders = Vec::new();
        for index in 0..records.len()? {
            placeholders
                .push(records.with_host_field(index, |field| field.known_alias().to_owned())?);
        }
        Ok(Self {
            records,
            placeholders,
        })
    }
}

struct Inner {
    session: Arc<MappingSession>,
    guard: Arc<SendGuard>,
    detector: Detector,
    /// One admission at a time: the vault has a single operation slot and
    /// closes a session that finds it busy.
    operation: tokio::sync::Mutex<Ledger>,
    stopped: AtomicBool,
    #[cfg(test)]
    after_commit: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Inner {
    fn live(&self) -> Result<(), SessionStop> {
        if self.stopped.load(Ordering::Acquire) || self.guard.check().is_err() {
            return Err(SessionStop::Closed);
        }
        Ok(())
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.guard.close();
        self.session.close();
    }

    fn stopped(&self, stop: SessionStop) -> AdmitError {
        self.stop();
        AdmitError::Stopped(stop)
    }
}

/// One conversation's privacy owner. Dropping or stopping it closes its send
/// guard and its vault scope, and text it admitted can no longer be read. It
/// never reopens.
pub struct PrivacySession {
    inner: Arc<Inner>,
}

impl PrivacySession {
    /// Opens the scope in an already open vault and binds the guard to it.
    /// The vault, its key and its location are the caller's; this function
    /// opens no file and reads no configuration. The detector is the host's
    /// selected one; its source is checked on every call it makes.
    pub async fn open(
        vault: &Vault,
        scope: ConversationScope,
        detector: Detector,
    ) -> Result<Self, SessionStop> {
        let session = Arc::new(vault.session(scope.namespace).map_err(SessionStop::Vault)?);
        let records = session.scope_records().await.map_err(SessionStop::Vault)?;
        let guard = SendGuard::from_vault(session.clone(), &records).map_err(SessionStop::Guard)?;
        let ledger = Ledger::read(records).map_err(SessionStop::Vault)?;
        Ok(Self {
            inner: Arc::new(Inner {
                session,
                guard,
                detector,
                operation: tokio::sync::Mutex::new(ledger),
                stopped: AtomicBool::new(false),
                #[cfg(test)]
                after_commit: std::sync::Mutex::new(None),
            }),
        })
    }

    /// The guard every model client of this conversation must be bound to.
    pub fn send_guard(&self) -> Arc<SendGuard> {
        self.inner.guard.clone()
    }

    pub fn check(&self) -> Result<(), SessionStop> {
        self.inner.live()
    }

    /// Ends the session: the guard refuses every later send and the scope
    /// refuses every later mapping. It cannot retract text already released.
    pub fn stop(&self) {
        self.inner.stop();
    }

    /// The live scope and all of its authenticated records, for a host view
    /// such as display or file restoration. Held for the callback only.
    pub async fn with_scope_records<T>(
        &self,
        read: impl FnOnce(&MappingSession, &ResolutionRecords) -> T,
    ) -> Result<T, SessionStop> {
        let ledger = self.inner.operation.lock().await;
        self.inner.live()?;
        let result = read(&self.inner.session, &ledger.records);
        self.inner.live()?;
        Ok(result)
    }

    /// Admits one raw text: host-origin text that has never been admitted.
    ///
    /// On success the mappings are saved, the guard knows every original, and
    /// the returned text contains placeholders where values were reported. On
    /// `Refused` or `Detector` nothing was saved and the session continues. On
    /// `Stopped` the session is closed. Nothing raw is returned in any case.
    pub async fn admit_raw(&self, text: &str) -> Result<AdmittedText, AdmitError> {
        let inner = &self.inner;
        let mut ledger = inner.operation.lock().await;
        inner.live().map_err(AdmitError::Stopped)?;
        if text.len() > MAX_TEXT_BYTES {
            return Err(AdmitError::Refused(AdmitRefusal::TooLarge));
        }
        if text.is_empty() {
            return Ok(self.release(String::new(), 0));
        }
        // An issued placeholder in raw text is ambiguous: it may be a value
        // of the user's that happens to look like one. Never guess.
        if ledger
            .placeholders
            .iter()
            .any(|placeholder| text.contains(placeholder.as_str()))
        {
            return Err(AdmitError::Refused(AdmitRefusal::IssuedPlaceholder));
        }
        let spans = match inner.detector.detect(text).await {
            Ok(spans) => spans,
            Err(HookError::Input) => return Err(AdmitError::Refused(AdmitRefusal::TooLarge)),
            Err(HookError::Source) => return Err(inner.stopped(SessionStop::Detector)),
            Err(error) => return Err(AdmitError::Detector(error)),
        };
        inner.live().map_err(AdmitError::Stopped)?;
        let new_values = preflight(text, &spans, &ledger).map_err(|error| match error {
            Preflight::Refused(refusal) => AdmitError::Refused(refusal),
            Preflight::Vault(error) => inner.stopped(SessionStop::Vault(error)),
        })?;
        let admitted = if spans.is_empty() {
            text.to_owned()
        } else {
            let vault = |error| inner.stopped(SessionStop::Vault(error));
            let captured = inner.session.capture_text(text).map_err(vault)?;
            let report = DetectorReport {
                input_sha256: captured.input_sha256_for_host_detector().map_err(vault)?,
                spans: spans
                    .iter()
                    .map(|span| DetectorSpan {
                        start: span.start,
                        end: span.end,
                        category: span.category,
                    })
                    .collect(),
            };
            // The mappings are saved before anything else happens to them.
            let masked = inner
                .session
                .mask(captured, report, DeclaredIdentifiers::none())
                .await
                .map_err(vault)?;
            #[cfg(test)]
            if let Some(hook) = inner.after_commit.lock().unwrap().as_ref() {
                hook();
            }
            // Then the guard learns their originals, before any release.
            let records = masked.resolution_records_for_host_fields().map_err(vault)?;
            inner
                .guard
                .admit(records)
                .map_err(|refusal| inner.stopped(SessionStop::Guard(refusal)))?;
            if new_values > 0 {
                let records = inner.session.scope_records().await.map_err(vault)?;
                *ledger = Ledger::read(records).map_err(vault)?;
            }
            masked
                .text_for_later_model_request()
                .map_err(vault)?
                .to_owned()
        };
        // The guard's own verifier over the text about to be released.
        inner
            .guard
            .seal(b"null".to_vec(), &[admitted.as_str()])
            .map_err(|refusal| inner.stopped(SessionStop::Guard(refusal)))?;
        Ok(self.release(admitted, spans.len()))
    }

    fn release(&self, text: String, replacements: usize) -> AdmittedText {
        AdmittedText {
            text: Zeroizing::new(text),
            replacements,
            owner: self.inner.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn after_commit(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.inner.after_commit.lock().unwrap() = Some(Box::new(hook));
    }
}

impl Drop for PrivacySession {
    fn drop(&mut self) {
        self.inner.stop();
    }
}

enum Preflight {
    Refused(AdmitRefusal),
    Vault(PrivacyError),
}

/// Everything the vault would refuse for the content of this text, decided
/// here from the text, the checked spans and the scope's authenticated
/// records. Returns the number of values that are new to the scope.
fn preflight(text: &str, spans: &[TextSpan], ledger: &Ledger) -> Result<usize, Preflight> {
    let refuse = |refusal| Err(Preflight::Refused(refusal));
    // A conservative bound on the admitted text.
    if text.len() + spans.len() * MAX_PLACEHOLDER_BYTES > MAX_TEXT_BYTES {
        return refuse(AdmitRefusal::TooLarge);
    }
    let covered = |value: &str| {
        text.match_indices(value).all(|(start, found)| {
            spans
                .iter()
                .any(|span| span.start <= start && span.end >= start + found.len())
        })
    };
    let mut values: Vec<(&str, Category)> = Vec::new();
    for span in spans {
        let value = &text[span.start..span.end];
        match values.iter().find(|(prior, _)| *prior == value) {
            Some((_, category)) if *category != span.category => {
                return refuse(AdmitRefusal::ConflictingCategory);
            }
            Some(_) => {}
            None => values.push((value, span.category)),
        }
    }
    // Every other occurrence of a reported value must be reported too.
    if values.iter().any(|(value, _)| !covered(value)) {
        return refuse(AdmitRefusal::UncoveredValue);
    }
    let known = ledger.records.len().map_err(Preflight::Vault)?;
    let mut reused = 0;
    for index in 0..known {
        // The original is read in place and never copied out.
        let verdict = ledger
            .records
            .with_host_field(index, |field| {
                let original = field.original_for_validated_host_field();
                match values.iter().find(|(value, _)| *value == original) {
                    Some((_, category)) if *category != field.category() => {
                        Some(AdmitRefusal::ConflictingCategory)
                    }
                    Some(_) => {
                        reused += 1;
                        None
                    }
                    // A known value that the detector did not report here.
                    None if !covered(original) => Some(AdmitRefusal::UncoveredValue),
                    None => None,
                }
            })
            .map_err(Preflight::Vault)?;
        if let Some(refusal) = verdict {
            return refuse(refusal);
        }
    }
    let new_values = values.len() - reused;
    if known + new_values > SCOPE_MAPPINGS {
        return refuse(AdmitRefusal::ScopeFull);
    }
    Ok(new_values)
}

/// Text this session admitted. It holds placeholders wherever the detector
/// reported a value, and it is the only form in which admitted text leaves
/// this module. Never printed or serialized.
pub struct AdmittedText {
    text: Zeroizing<String>,
    replacements: usize,
    owner: Arc<Inner>,
}

impl AdmittedText {
    /// The admitted text, while its session is live.
    pub fn text(&self) -> Result<&str, SessionStop> {
        self.owner.live()?;
        Ok(&self.text)
    }

    pub fn replacements(&self) -> usize {
        self.replacements
    }

    /// Whether this exact session admitted the text. An equal-looking session
    /// or text never qualifies.
    pub fn admitted_by(&self, session: &PrivacySession) -> bool {
        Arc::ptr_eq(&self.owner, &session.inner)
    }
}

macro_rules! private_debug {
    ($($type:ty),+ $(,)?) => {$(
        impl std::fmt::Debug for $type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($type), "([private])"))
            }
        }
    )+};
}

private_debug!(ConversationScope, PrivacySession, AdmittedText);
