//! Private, default-absent final-send guard for known vault originals.
//!
//! No CLI, configuration, frontend, hook or key source selects this module. A
//! guard exists only when host code mints one from an actual live vault
//! `MappingSession` and its own `ResolutionRecords`. A bound model client then
//! hands the network only bytes this guard sealed, and asks the guard again
//! before every attempt.
//!
//! This is an additional refusal seam. It looks for originals that real
//! mapping records already hold. It is not a detector of unknown personal data,
//! it performs no masking or restoration, and it does not establish privacy
//! integration. See `docs/privacy-send.md` for the supported representations.
//!
//! A decision covers the instant it is made. A refusal says that the guard
//! refused one operation or one attempt. It does not say that nothing was sent
//! earlier, and it cannot retract an attempt that was already allowed or stop
//! network work that has already started.

mod scan;

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

#[cfg(test)]
pub(crate) mod tests;

use crate::privacy_vault::{MappingSession, PrivacyError, ResolutionRecords};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use zeroize::Zeroizing;

/// Fixed diagnostics. Each names the refused operation or attempt only. They
/// carry no body, value, alias, path or provider text, and make no statement
/// about earlier attempts, earlier effects, usage or a provider's billing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SendRefusal {
    #[error("The private send guard is closed and refused this operation.")]
    Closed,
    #[error(
        "The private send guard refused this attempt because the request contains private data that is not allowed to leave."
    )]
    KnownOriginal,
    #[error("The private send guard refused this operation because it exceeds the guard's bounds.")]
    Bounds,
    #[error(
        "The private send guard refused this operation because it has a shape the guard cannot check."
    )]
    Unsupported,
    #[error(
        "The private send guard refused this attempt because its request body was not sealed by this session's guard."
    )]
    Unsealed,
    #[error(
        "The private send guard refused this operation because its data belongs to a different session or guard."
    )]
    Foreign,
}

/// Hard ceilings. A test may tighten them; nothing can raise them.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    /// Largest final encoded body, matching the native transports' request cap.
    /// The request's own text supplied with it has the same ceiling.
    pub(crate) body_bytes: usize,
    /// Work units for one verification, as counted in `scan`. A count of the
    /// operations listed there, not a measure of CPU time.
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

/// The guard's own state. One lock covers all of it, so every decision reads
/// one consistent admitted set, generation and closed flag.
struct State {
    originals: scan::Originals,
    /// Advances whenever the admitted set changes; a seal records the value it
    /// was verified against.
    generation: u64,
    closed: bool,
}

/// One guard is shared by every model client of one explicitly selected
/// session. Equal-looking guards never substitute for each other.
///
/// Decisions (`seal`, `ready`, `check`) hold the state lock shared; `admit` and
/// `close` hold it exclusively. A decision and an admission therefore never
/// interleave: a decision is made against exactly one admitted set, and an
/// admission that arrives meanwhile takes effect after that decision has
/// returned. Nothing awaits while the lock is held.
pub struct SendGuard {
    session: Arc<MappingSession>,
    state: RwLock<State>,
    limits: Limits,
}

/// Exact final encoded bytes that this guard verified, together with the
/// request text it verified them with. The only body a bound client accepts.
/// Never cloned, printed or serialized.
pub struct Sealed {
    bytes: Vec<u8>,
    /// The request's own text nodes this seal was verified with, joined by a
    /// NUL. No admitted original contains a control byte, so a match can never
    /// span two nodes. Retained so a later decision verifies the same inputs,
    /// never a caller's new ones.
    portable: String,
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
            state: RwLock::new(State {
                originals: scan::Originals::empty(),
                generation: 0,
                closed: false,
            }),
            limits,
        });
        guard.admit(records)?;
        Ok(guard)
    }

    /// Adds the originals of further records from this same session. Whole
    /// admission or a closed guard: an original this guard cannot represent
    /// must never be silently dropped while requests keep flowing.
    ///
    /// An admission waits for decisions in flight and applies to decisions
    /// made after it returns. It does not reach back to an attempt that an
    /// earlier decision already allowed.
    pub fn admit(&self, records: &ResolutionRecords) -> Result<(), SendRefusal> {
        let result = self.admit_inner(records);
        if result.is_err() {
            self.close();
        }
        result
    }

    fn admit_inner(&self, records: &ResolutionRecords) -> Result<(), SendRefusal> {
        records
            .validate_session_binding(&self.session)
            .map_err(|error| match error {
                PrivacyError::ForeignScope => SendRefusal::Foreign,
                _ => SendRefusal::Closed,
            })?;
        // Admission work is bounded by these ceilings alone. It is separate
        // from, and not charged to, a verification's work budget.
        let count = or_return!(records.len(), SendRefusal::Closed);
        if count > self.limits.originals {
            return Err(SendRefusal::Bounds);
        }
        let mut added = Vec::with_capacity(count);
        let mut bytes = 0usize;
        for index in 0..count {
            // The vault's checked host-field view. Shape and size are checked
            // on the borrowed value, and its length is reserved against the
            // total ceiling, before anything is copied. Only then is it copied
            // into this guard's zeroized store; it is never returned or
            // formatted.
            let original = records
                .with_host_field(index, |field| {
                    let original = field.original_for_validated_host_field().as_bytes();
                    scan::supported_original(original, &self.limits)?;
                    bytes = bytes
                        .checked_add(original.len())
                        .filter(|total| *total <= self.limits.total_original_bytes)
                        .ok_or(SendRefusal::Bounds)?;
                    Ok::<_, SendRefusal>(Zeroizing::new(original.to_vec()))
                })
                .map_err(|_| SendRefusal::Closed)??;
            added.push(original);
        }
        let mut state = or_return!(self.state.write(), SendRefusal::Closed);
        self.live(&state)?;
        let Some(next) = state.originals.with(added, &self.limits)? else {
            return Ok(()); // Nothing new: existing seals stay current.
        };
        // The next generation is decided before anything is committed, so a
        // refusal here leaves the admitted set and its generation unchanged.
        let generation = state.generation.checked_add(1).ok_or(SendRefusal::Bounds)?;
        state.originals = next;
        state.generation = generation;
        Ok(())
    }

    /// Denies every later decision through this guard. Like an admission, it
    /// waits for decisions in flight and cannot retract one already made.
    pub fn close(&self) {
        match self.state.write() {
            Ok(mut state) => state.closed = true,
            // A poisoned lock already refuses every decision.
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.closed = true;
            }
        }
    }

    /// Session construction refuses a selection that is no longer live.
    pub(crate) fn check(&self) -> Result<(), SendRefusal> {
        let state = or_return!(self.state.read(), SendRefusal::Closed);
        self.live(&state)
    }

    /// The guard's own flag, read under the caller's lock, and the vault's
    /// existing checked liveness observation for a session: an empty capture
    /// performs no IO, holds no data, and refuses once the session or its
    /// vault has closed. The vault's state is sampled here, not locked.
    fn live(&self, state: &State) -> Result<(), SendRefusal> {
        if state.closed || self.session.capture_text("").is_err() {
            return Err(SendRefusal::Closed);
        }
        Ok(())
    }

    /// Verifies the exact final encoded body against the current admitted set.
    /// `portable` carries the request's own text nodes when the encoding may
    /// split them (replayed native items). They are checked and retained with
    /// the seal, never sent.
    pub(crate) fn seal(
        self: &Arc<Self>,
        encoded: Vec<u8>,
        portable: &[&str],
    ) -> Result<Sealed, SendRefusal> {
        let state = or_return!(self.state.read(), SendRefusal::Closed);
        self.live(&state)?;
        scan::verify(&state.originals, &self.limits, &encoded, portable)?;
        self.live(&state)?;
        Ok(Sealed {
            bytes: encoded,
            // One copy, bounded by the body ceiling `verify` just enforced.
            portable: portable.join("\0"),
            guard: self.clone(),
            generation: AtomicU64::new(state.generation),
        })
    }

    /// The decision for one send attempt, made immediately before it. A seal
    /// of another guard is refused. A seal verified against an older admitted
    /// set is verified again, with its own retained inputs, against the set
    /// that is current at this decision.
    ///
    /// The decision is consistent with one admitted set and one closed flag.
    /// It does not extend past its return: an admission or a close that
    /// happens afterwards does not retract this attempt, and a refusal or a
    /// closed guard is no evidence that earlier network work has stopped.
    pub(crate) fn ready(&self, sealed: &Sealed) -> Result<(), SendRefusal> {
        if !std::ptr::eq(Arc::as_ptr(&sealed.guard), self) {
            return Err(SendRefusal::Foreign);
        }
        let state = or_return!(self.state.read(), SendRefusal::Closed);
        self.live(&state)?;
        if sealed.generation.load(Ordering::Acquire) != state.generation {
            let portable = [sealed.portable.as_str()];
            scan::verify(&state.originals, &self.limits, &sealed.bytes, &portable)?;
            sealed.generation.store(state.generation, Ordering::Release);
            self.live(&state)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn admitted_generation(&self) -> u64 {
        self.state.read().unwrap().generation
    }

    /// Test-only: the outcome and the work units one verification of these
    /// inputs charges against the current admitted set.
    #[cfg(test)]
    pub(crate) fn verification_work(
        &self,
        encoded: &[u8],
        portable: &[&str],
    ) -> (Result<(), SendRefusal>, u64) {
        let state = self.state.read().unwrap();
        scan::verify_counting(&state.originals, &self.limits, encoded, portable)
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
