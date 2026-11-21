//! Private, default-absent final-send guard for known vault originals.
//!
//! No CLI, configuration, frontend, hook or key source selects this module. A
//! guard exists only when host code mints one from an actual live vault
//! `MappingSession` and its own `ResolutionRecords`. A bound model client then
//! sends only bytes this guard sealed, and rechecks the seal before every
//! attempt.
//!
//! This is an additional refusal seam. It looks for originals that real
//! mapping records already hold. It is not a detector of unknown personal data,
//! it performs no masking or restoration, and it does not establish privacy
//! integration. See `docs/privacy-send.md` for the supported representations.

mod scan;

#[cfg(test)]
pub(crate) mod tests;

use crate::privacy_vault::{MappingSession, PrivacyError, ResolutionRecords};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

/// Fixed diagnostics. They carry no body, value, alias, path or provider text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SendRefusal {
    #[error("The private send guard is closed. The model request was not sent.")]
    Closed,
    #[error(
        "The model request contains private data that is not allowed to leave. It was not sent."
    )]
    KnownOriginal,
    #[error("The model request exceeds the private send guard's bounds. It was not sent.")]
    Bounds,
    #[error(
        "The model request uses a representation the private send guard cannot check. It was not sent."
    )]
    Unsupported,
    #[error(
        "The model request was not sealed by this session's private send guard. It was not sent."
    )]
    Unsealed,
    #[error(
        "The private send data belongs to a different session. The model request was not sent."
    )]
    Foreign,
}

/// Hard ceilings. A test may tighten them; nothing can raise them.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    /// Largest final encoded body, matching the native transports' request cap.
    pub(crate) body_bytes: usize,
    /// Parsed, scanned and compared bytes for one verification.
    pub(crate) work: u64,
    /// Open JSON containers, summed across embedded documents.
    pub(crate) containers: usize,
    /// JSON documents nested inside string values, below the body itself.
    pub(crate) embedded: usize,
    pub(crate) originals: usize,
    pub(crate) original_bytes: usize,
    pub(crate) total_original_bytes: usize,
}

pub(crate) const MIN_ORIGINAL_BYTES: usize = 4;

impl Default for Limits {
    fn default() -> Self {
        Self {
            body_bytes: 4 * 1024 * 1024,
            work: 128 * 1024 * 1024,
            containers: 96,
            embedded: 4,
            originals: 512,
            original_bytes: 1024,
            total_original_bytes: 256 * 1024,
        }
    }
}

impl Limits {
    fn validate(self) -> Result<Self, SendRefusal> {
        let max = Self::default();
        if self.body_bytes == 0
            || self.body_bytes > max.body_bytes
            || self.work == 0
            || self.work > max.work
            || self.containers == 0
            || self.containers > max.containers
            || self.embedded > max.embedded
            || self.originals == 0
            || self.originals > max.originals
            || self.original_bytes < MIN_ORIGINAL_BYTES
            || self.original_bytes > max.original_bytes
            || self.total_original_bytes == 0
            || self.total_original_bytes > max.total_original_bytes
        {
            return Err(SendRefusal::Bounds);
        }
        Ok(self)
    }
}

struct State {
    originals: Arc<scan::Originals>,
    /// Advances whenever the admitted set changes; a seal records the value it
    /// was verified against and is verified again when this has moved on.
    generation: u64,
}

/// One guard is shared by every model client of one explicitly selected
/// session. Equal-looking guards never substitute for each other.
pub struct SendGuard {
    session: Arc<MappingSession>,
    state: Mutex<State>,
    closed: AtomicBool,
    limits: Limits,
}

/// Exact final encoded bytes that this guard verified. The only body a bound
/// client accepts. Never cloned, printed or serialized.
pub struct Sealed {
    bytes: Vec<u8>,
    guard: Arc<SendGuard>,
    generation: AtomicU64,
}

impl SendGuard {
    /// The only constructor. Both arguments are actual vault objects: a live
    /// session and records that this same session produced. Caller strings,
    /// hashes, status flags and records of another scope cannot mint a guard.
    pub fn from_vault(
        session: Arc<MappingSession>,
        records: &ResolutionRecords,
    ) -> Result<Arc<Self>, SendRefusal> {
        Self::build(session, records, Limits::default())
    }

    /// Test-only tighter ceilings over the same construction.
    #[cfg(test)]
    pub(crate) fn with_limits(
        session: Arc<MappingSession>,
        records: &ResolutionRecords,
        limits: Limits,
    ) -> Result<Arc<Self>, SendRefusal> {
        Self::build(session, records, limits)
    }

    fn build(
        session: Arc<MappingSession>,
        records: &ResolutionRecords,
        limits: Limits,
    ) -> Result<Arc<Self>, SendRefusal> {
        let limits = limits.validate()?;
        let guard = Arc::new(Self {
            session,
            state: Mutex::new(State {
                originals: Arc::new(scan::Originals::empty()),
                generation: 0,
            }),
            closed: AtomicBool::new(false),
            limits,
        });
        guard.admit(records)?;
        Ok(guard)
    }

    /// Adds the originals of further records from this same session. Whole
    /// admission or a closed guard: an original this guard cannot represent
    /// must never be silently dropped while requests keep flowing.
    pub fn admit(&self, records: &ResolutionRecords) -> Result<(), SendRefusal> {
        let result = self.admit_inner(records);
        if result.is_err() {
            self.close();
        }
        result
    }

    fn admit_inner(&self, records: &ResolutionRecords) -> Result<(), SendRefusal> {
        self.live()?;
        records
            .validate_session_binding(&self.session)
            .map_err(|error| match error {
                PrivacyError::ForeignScope => SendRefusal::Foreign,
                _ => SendRefusal::Closed,
            })?;
        let count = records.len().map_err(|_| SendRefusal::Closed)?;
        if count > self.limits.originals {
            return Err(SendRefusal::Bounds);
        }
        let mut added = Vec::with_capacity(count);
        for index in 0..count {
            // The vault's checked host-field view. The copy stays in this
            // guard's zeroized store and is never returned or formatted.
            let original = records
                .with_host_field(index, |field| {
                    Zeroizing::new(
                        field
                            .original_for_validated_host_field()
                            .as_bytes()
                            .to_vec(),
                    )
                })
                .map_err(|_| SendRefusal::Closed)?;
            scan::supported_original(&original, &self.limits)?;
            added.push(original);
        }
        let mut state = self.state.lock().map_err(|_| SendRefusal::Closed)?;
        let Some(next) = state.originals.with(added, &self.limits)? else {
            return Ok(()); // Nothing new: existing seals stay current.
        };
        state.originals = Arc::new(next);
        state.generation = state.generation.checked_add(1).ok_or(SendRefusal::Bounds)?;
        drop(state);
        self.live()
    }

    /// Session construction refuses a selection that is no longer live.
    pub(crate) fn check(&self) -> Result<(), SendRefusal> {
        self.live()
    }

    /// Denies all further seals and sends through this guard.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// An empty capture is the vault's existing checked liveness observation
    /// for a session: no IO, no data, and a fixed refusal once the session or
    /// its vault has closed.
    fn live(&self) -> Result<(), SendRefusal> {
        if self.closed.load(Ordering::Acquire) || self.session.capture_text("").is_err() {
            return Err(SendRefusal::Closed);
        }
        Ok(())
    }

    fn snapshot(&self) -> Result<(Arc<scan::Originals>, u64), SendRefusal> {
        let state = self.state.lock().map_err(|_| SendRefusal::Closed)?;
        Ok((state.originals.clone(), state.generation))
    }

    /// Verifies the exact final encoded body. `portable` carries the request's
    /// own text nodes when the encoding may split them (replayed native items);
    /// it is checked, never sent.
    pub(crate) fn seal(
        self: &Arc<Self>,
        encoded: Vec<u8>,
        portable: &[&str],
    ) -> Result<Sealed, SendRefusal> {
        self.live()?;
        let (originals, generation) = self.snapshot()?;
        scan::verify(&originals, &self.limits, &encoded, portable)?;
        self.live()?;
        Ok(Sealed {
            bytes: encoded,
            guard: self.clone(),
            generation: AtomicU64::new(generation),
        })
    }

    /// Called before every actual send attempt. A seal of another guard is
    /// refused. A seal older than the admitted set is verified again against
    /// the current set, so it can never carry a newly admitted original.
    pub(crate) fn ready(&self, sealed: &Sealed, portable: &[&str]) -> Result<(), SendRefusal> {
        if !std::ptr::eq(Arc::as_ptr(&sealed.guard), self) {
            return Err(SendRefusal::Foreign);
        }
        self.live()?;
        let (originals, generation) = self.snapshot()?;
        if sealed.generation.load(Ordering::Acquire) != generation {
            scan::verify(&originals, &self.limits, &sealed.bytes, portable)?;
            sealed.generation.store(generation, Ordering::Release);
            self.live()?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn admitted_generation(&self) -> u64 {
        self.state.lock().unwrap().generation
    }
}

impl Sealed {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl fmt::Debug for SendGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SendGuard([private])")
    }
}

impl fmt::Debug for Sealed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sealed([private])")
    }
}
