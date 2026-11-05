//! Private finalization authority from an actual consumed, returned input Run.
//! Not a public result/TTY/ASCII/storage writer, ordered shutdown or usage meter.
use crate::exec::input::Run;
use crate::exec::provider::request::{Fault, terminal_fragment};
use crate::exec::runtime::TerminalScope;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

mod selection;
pub(crate) use selection::Selection;
mod ascii;
pub(crate) use ascii::Ascii;
mod human;
pub(crate) use human::Human;
mod record;
pub(in crate::exec) use record::Record;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    Closed,
    Unsettled,
    Superseded,
    Foreign,
    Codec(Fault),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Error {}
impl From<Fault> for Error {
    fn from(f: Fault) -> Self {
        Self::Codec(f)
    }
}

/// Actual private return variant, NOT public success/refusal/exit/stop mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReturnKind {
    Returned,
    ApprovalRequired,
    TurnFailed,
    ConstructionFailed,
    Suspended,
}
impl ReturnKind {
    pub(super) fn tag(self) -> &'static str {
        match self {
            Self::Returned => "returned",
            Self::ApprovalRequired => "approval_required",
            Self::TurnFailed => "turn_failed",
            Self::ConstructionFailed => "construction_failed",
            Self::Suspended => "suspended",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Facts {
    returned: ReturnKind,
    approval_required: bool,
}
impl Facts {
    pub(crate) fn returned(self) -> ReturnKind {
        self.returned
    }
    pub(crate) fn approval_required(self) -> bool {
        self.approval_required
    }
}
struct Publication {
    closed: AtomicBool,
}

/// Owns the ORIGINAL Run/outcome/runtime. No raw outcome/config/gate constructor,
/// execution/resume/Runtime accessor, Debug/Serialize or caller-supplied facts.
pub(crate) struct Terminal {
    run: Run,
    publication: Arc<Publication>,
}
impl Terminal {
    // Run::terminal closes its actual execution gate BEFORE handing ownership here.
    pub(super) fn from_run(run: Run) -> Self {
        Self {
            run,
            publication: Arc::new(Publication {
                closed: AtomicBool::new(false),
            }),
        }
    }
    pub(crate) fn scope(&self) -> Result<Scope<'_>, Error> {
        let scope = Scope {
            runtime: self.run.terminal_scope()?,
            publication: &self.publication,
        };
        scope.ready(scope.runtime.approval_required())?;
        Ok(scope)
    }
    pub(crate) fn snapshot(&self) -> Result<Snapshot<'_>, Error> {
        capture(self.scope()?, || {})
    }
    /// Independent deny-only publication revocation, never execution reopening.
    pub(crate) fn close(&self) {
        self.publication.closed.store(true, Ordering::Release);
    }
    #[cfg(test)]
    pub(crate) fn snapshot_after_work_for_tests(
        &self,
        after: impl FnOnce(),
    ) -> Result<Snapshot<'_>, Error> {
        capture(self.scope()?, after)
    }
}

/// Only this owned actual Terminal can mint a scope; fields are opaque.
pub(crate) struct Scope<'a> {
    runtime: TerminalScope<'a>,
    publication: &'a Arc<Publication>,
}
impl Scope<'_> {
    fn ready(&self, approval: bool) -> Result<(), Error> {
        if self.publication.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        self.runtime.ready()?;
        if self.runtime.approval_required() != approval {
            return Err(Error::Superseded);
        }
        Ok(())
    }
    fn same(&self, other: &Scope<'_>) -> bool {
        Arc::ptr_eq(self.publication, other.publication) && self.runtime.same(&other.runtime)
    }
}

/// Checked immutable private fragment only; no raw/Serialize/Debug/IO interface.
/// The captured typed approval bit is an epoch, not an execution-latch exemption.
pub(crate) struct Snapshot<'a> {
    scope: Scope<'a>,
    facts: Facts,
    count: usize,
    bytes: Vec<u8>,
}
impl Snapshot<'_> {
    pub(crate) fn bytes(&self) -> Result<&[u8], Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(&self.bytes)
    }
    pub(crate) fn facts(&self) -> Result<Facts, Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(self.facts)
    }
    pub(crate) fn message_count(&self) -> Result<usize, Error> {
        self.scope.ready(self.facts.approval_required)?;
        Ok(self.count)
    }
    pub(crate) fn bind(&self, other: &Scope<'_>) -> Result<(), Error> {
        if !self.scope.same(other) {
            return Err(Error::Foreign);
        }
        self.scope.ready(self.facts.approval_required)?;
        other.ready(self.facts.approval_required)
    }
}
fn capture(scope: Scope<'_>, after: impl FnOnce()) -> Result<Snapshot<'_>, Error> {
    let facts = Facts {
        returned: scope.runtime.returned(),
        approval_required: scope.runtime.approval_required(),
    };
    scope.ready(facts.approval_required)?;
    let result = (|| {
        let state = scope
            .runtime
            .state()
            .lock()
            .map_err(|_| Error::Codec(Fault::Collision))?;
        // ORIGINAL main state is admitted under its real lock before message
        // copies. Original output is immutable in the owned actual returned Run.
        let bytes = terminal_fragment(
            scope.runtime.redactor(),
            &state.messages,
            scope.runtime.output(),
            facts.returned.tag(),
            facts.approval_required,
        )?;
        Ok::<_, Error>((state.messages.len(), bytes))
    })();
    after();
    // Revocation/typed epoch changes dominate codec failures too. No stale
    // returned fragment is released after a late actual approval stop.
    scope.ready(facts.approval_required)?;
    let (count, bytes) = result?;
    Ok(Snapshot {
        scope,
        facts,
        count,
        bytes,
    })
}

impl Terminal {
    /// Exact private full-fragment budget; only whole messages may be omitted.
    /// Header/output admission and original message caps are never bypassed.
    pub(crate) fn selection(&self, max_bytes: usize) -> Result<Selection<'_>, Error> {
        selection::select(self.scope()?, max_bytes, || {})
    }
    #[cfg(test)]
    pub(crate) fn selection_after_work_for_tests(
        &self,
        max_bytes: usize,
        after: impl FnOnce(),
    ) -> Result<Selection<'_>, Error> {
        selection::select(self.scope()?, max_bytes, after)
    }
}

impl Terminal {
    /// Complete checked final answer/facts plus an ASCII-byte-bounded newest
    /// whole-message suffix; no public result/IO or live-latch exemption.
    pub(crate) fn ascii(&self, max_bytes: usize) -> Result<Ascii<'_>, Error> {
        ascii::encode(self.scope()?, max_bytes, || {})
    }
    #[cfg(test)]
    pub(crate) fn ascii_after_work_for_tests(
        &self,
        max_bytes: usize,
        after: impl FnOnce(),
    ) -> Result<Ascii<'_>, Error> {
        ascii::encode(self.scope()?, max_bytes, after)
    }
}

impl Terminal {
    /// Complete actual final answer and fixed private-fact diagnostic, checked
    /// for human display only; no public stdout/stderr eligibility or writer.
    pub(crate) fn human(&self, max_bytes: usize) -> Result<Human<'_>, Error> {
        human::encode(self.scope()?, max_bytes, || {})
    }
    #[cfg(test)]
    pub(crate) fn human_after_work_for_tests(
        &self,
        max_bytes: usize,
        after: impl FnOnce(),
    ) -> Result<Human<'_>, Error> {
        human::encode(self.scope()?, max_bytes, after)
    }
}

impl Terminal {
    /// Only the paired pre-run file controller selects these private records.
    pub(in crate::exec) fn file_record(
        &self,
        format: crate::exec::output::Format,
        maximum: usize,
    ) -> Result<Record<'_>, Error> {
        record::capture(self, format, maximum)
    }
}
