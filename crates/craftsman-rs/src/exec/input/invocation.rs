//! One admitted input, one actual shared-builder private invocation.
use super::{Admitted, Facts, Sources};
use crate::exec::construction::{InputPreparationError, Options, Prepared, PreparedInput};
use crate::exec::runtime::{Runtime, RuntimeError};
use craftsman_harness::{AgentState, TurnOutput};

pub(crate) struct Invocation {
    runtime: Runtime,
    input: Admitted,
}
impl Invocation {
    pub(crate) async fn prepare(
        options: Options,
        sources: Sources<'_>,
    ) -> Result<Self, InputPreparationError> {
        let prepared = PreparedInput::prepare(options, sources).await?;
        let (prepared, input) = prepared.into_parts();
        let runtime = Runtime::build(prepared)
            .await
            .map_err(|_| InputPreparationError::Runtime)?;
        Ok(Self { runtime, input })
    }
    /// Select a pure request cap before the SAME input-first preparation.
    pub(crate) async fn prepare_with_request_limit(
        options: Options,
        sources: Sources<'_>,
        requests: crate::exec::meter::RequestLimit,
    ) -> Result<Self, InputPreparationError> {
        let prepared = PreparedInput::prepare(options, sources).await?;
        let (prepared, input) = prepared.into_parts();
        let runtime = Runtime::build_with_request_limit(prepared, requests)
            .await
            .map_err(|_| InputPreparationError::Runtime)?;
        Ok(Self { runtime, input })
    }
    /// Pure explicit limits are chosen before SAME input-first preparation.
    /// This private opt-in retains cumulative allocations, not settled usage.
    pub(crate) async fn prepare_with_token_limit(
        options: Options,
        sources: Sources<'_>,
        requests: crate::exec::meter::RequestLimit,
        tokens: crate::exec::meter::TokenLimit,
    ) -> Result<Self, InputPreparationError> {
        let prepared = PreparedInput::prepare(options, sources).await?;
        let (prepared, input) = prepared.into_parts();
        let runtime = Runtime::build_with_token_limit(prepared, requests, tokens)
            .await
            .map_err(|_| InputPreparationError::Runtime)?;
        Ok(Self { runtime, input })
    }
    pub(crate) fn prepared(&self) -> &Prepared {
        self.runtime.prepared()
    }
    /// Consumes its source once. Failed turns retain the original partial
    /// runtime/state, rather than inventing success or dropping failure facts.
    pub(crate) async fn run(self) -> Run {
        let (body, facts) = self.input.into_parts();
        let outcome = self.runtime.run_turn(body).await;
        Run {
            runtime: self.runtime,
            facts,
            outcome,
        }
    }
}
/// Raw/private original state and outcome, NOT a public result or safe view.
pub(crate) struct Run {
    runtime: Runtime,
    facts: Facts,
    outcome: Result<TurnOutput, RuntimeError>,
}
impl Run {
    pub(crate) fn facts(&self) -> &Facts {
        &self.facts
    }
    pub(crate) fn outcome(&self) -> &Result<TurnOutput, RuntimeError> {
        &self.outcome
    }
    pub(crate) fn prepared(&self) -> &Prepared {
        self.runtime.prepared()
    }
    pub(crate) fn retained_state(&self) -> AgentState {
        self.runtime.retained_state()
    }
    /// Private point-in-time request observations, not a token/public ledger.
    pub(crate) fn request_counts(&self) -> Result<RequestCounts, RuntimeError> {
        Ok(RequestCounts {
            snapshot: self.runtime.request_snapshot()?,
        })
    }
    /// PRIVATE cumulative reservations only; missing usage is NOT reported zero.
    /// None means this Run selected the unchanged request-only default.
    pub(crate) fn token_reservations(&self) -> Result<Option<TokenReservations>, RuntimeError> {
        Ok(self
            .runtime
            .request_snapshot()?
            .tokens()
            .map(|snapshot| TokenReservations { snapshot }))
    }
    /// Admission-correlated private send-future/header facts; None is the
    /// unchanged request-only selection, not a fabricated zero transport ledger.
    pub(crate) fn admission_receipts(&self) -> Result<Option<AdmissionReceipts>, RuntimeError> {
        Ok(self
            .runtime
            .request_snapshot()?
            .receipts()
            .map(|observations| AdmissionReceipts { observations }))
    }
    /// Checked finite messages, not raw state cloning or public result output.
    pub(crate) fn message_view(
        &self,
    ) -> Result<crate::exec::messages::View<'_>, crate::exec::provider::request::Fault> {
        self.runtime.message_view()
    }
    /// Whole-message selection of the SAME checked runtime snapshot only.
    pub(crate) fn message_selection(
        &self,
        max_bytes: usize,
    ) -> Result<crate::exec::messages::Selection<'_>, crate::exec::provider::request::Fault> {
        self.runtime.message_selection(max_bytes)
    }
}

impl Run {
    /// Consume the actual returned invocation, permanently close execution and
    /// transfer original outcome/runtime into an independent private authority.
    pub(crate) fn terminal(self) -> crate::exec::terminal::Terminal {
        self.runtime.close_for_terminal();
        crate::exec::terminal::Terminal::from_run(self)
    }
    pub(in crate::exec) fn matches_runtime(&self, runtime: &Runtime) -> bool {
        self.runtime.same_runtime(runtime)
    }
    pub(in crate::exec) fn terminal_scope(
        &self,
    ) -> Result<crate::exec::runtime::TerminalScope<'_>, crate::exec::terminal::Error> {
        self.runtime.terminal_scope(self)
    }
    #[cfg(test)]
    pub(in crate::exec) fn runtime_for_tests(&self) -> &Runtime {
        &self.runtime
    }
}

impl Invocation {
    /// Only the admitted pre-run file wrapper uses the SAME existing runtime.
    pub(super) fn from_runtime_input(runtime: Runtime, input: Admitted) -> Self {
        Self { runtime, input }
    }
}

/// Only an actual Run can select these immutable, same-runtime observations.
/// No raw constructor/Debug/Serialize, public outcome or delivery/usage promise.
pub(crate) struct RequestCounts {
    snapshot: crate::exec::meter::Snapshot,
}
impl RequestCounts {
    pub(crate) fn maximum(&self) -> u64 {
        self.snapshot.maximum()
    }
    pub(crate) fn admitted(&self) -> u64 {
        self.snapshot.admitted()
    }
    /// admitted/active/supplied-inner ok/error/abandoned. Not HTTP attempts.
    pub(crate) fn role(&self, role: crate::exec::meter::Role) -> [u64; 5] {
        self.snapshot.role(role)
    }
    pub(crate) fn refusal(&self) -> Option<(crate::exec::meter::Role, u64, u64)> {
        self.snapshot.refusal()
    }
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.snapshot.same_source(&other.snapshot)
    }
}

/// Only an actual Run selects these source-bound historical allocations. They
/// cannot produce public observed usage/remaining/billing/receipts or status.
pub(crate) struct TokenReservations {
    snapshot: crate::exec::meter::TokenSnapshot,
}
impl TokenReservations {
    pub(crate) fn maximum(&self) -> u64 {
        self.snapshot.maximum()
    }
    pub(crate) fn allocated(&self) -> u64 {
        self.snapshot.allocated()
    }
    pub(crate) fn active(&self) -> u64 {
        self.snapshot.active()
    }
    pub(crate) fn unsettled(&self) -> u64 {
        self.snapshot.unsettled()
    }
    /// Estimated input / bounded reply / active / retained unsettled allocation.
    pub(crate) fn role(&self, role: crate::exec::meter::Role) -> [u64; 4] {
        self.snapshot.role(role)
    }
    /// Role / budget / already allocated / refused input / minimum reply / unallocated.
    pub(crate) fn refusal(&self) -> Option<(crate::exec::meter::Role, u64, u64, u64, u64, u64)> {
        self.snapshot.refusal()
    }
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.snapshot.same_source(&other.snapshot)
    }
}

/// Only actual Run selects these immutable facts; no raw constructor/Debug/
/// Serialize/public status/billing/usage or completed-body/OS-quiescence claim.
pub(crate) struct AdmissionReceipts {
    observations: crate::exec::meter::receipts::Observations,
}
impl AdmissionReceipts {
    pub(crate) fn len(&self) -> usize {
        self.observations.len()
    }
    pub(crate) fn call(
        &self,
        index: usize,
    ) -> Option<(
        u64,
        crate::exec::meter::Role,
        Option<crate::exec::meter::receipts::Return>,
        [crate::exec::meter::receipts::SendState; 2],
        [Option<u16>; 2],
    )> {
        self.observations.call(index)
    }
    pub(crate) fn totals(&self) -> [u64; 4] {
        self.observations.totals()
    }
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.observations.same_source(&other.observations)
    }
}
