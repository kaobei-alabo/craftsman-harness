//! Private shared logical-request admission and optional cumulative token reservations.
//!
//! The actual builder alone mints source/role bindings. A cap refusal closes its
//! SAME gate deny-only; observed inner returns/drops never refund admitted calls.
//! Reservations stay unsettled; no token usage/billing/serialization/public output.

pub(crate) mod receipts;
pub(crate) mod tokens;
use crate::exec::provider::request::{Checked, reservation::Plan};
use tokens::{Allocation, TokenState};
pub(crate) use tokens::{TokenLimit, TokenSnapshot};

use crate::agent::{RequestMeterScope, RequestProviderScope};
use crate::exec::runtime::InvocationGate;
use async_trait::async_trait;
use craftsman_harness::{AgentError, ModelProvider, ModelRequest, ModelResponse};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use thiserror::Error;

/// Accepted R5 bound, selected explicitly rather than from ambient config.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestLimit(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("model request limit must be an integer from 1 through 10000")]
pub(crate) struct InvalidLimit;

impl RequestLimit {
    pub(crate) fn new(maximum: u64) -> Result<Self, InvalidLimit> {
        if (1..=10_000).contains(&maximum) {
            Ok(Self(maximum))
        } else {
            Err(InvalidLimit)
        }
    }
    pub(crate) fn maximum(self) -> u64 {
        self.0
    }
}
impl Default for RequestLimit {
    fn default() -> Self {
        Self(50)
    }
}

/// Concrete construction role, never inferred from a name, model or response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Main,
    Subagent,
    Compaction,
}
impl Role {
    fn index(self) -> usize {
        match self {
            Self::Main => 0,
            Self::Subagent => 1,
            Self::Compaction => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("[REDACTED]")]
pub(crate) enum Fault {
    ApprovalRequired,
    Closed,
    Limit,
    Accounting,
    Foreign,
    Tokens,
    Preflight,
}

#[derive(Clone, Copy, Default)]
struct Row {
    admitted: u64,
    active: u64,
    returned_ok: u64,
    returned_error: u64,
    abandoned: u64,
}
impl Row {
    fn values(self) -> [u64; 5] {
        [
            self.admitted,
            self.active,
            self.returned_ok,
            self.returned_error,
            self.abandoned,
        ]
    }
    fn coherent(self) -> bool {
        u128::from(self.admitted)
            == u128::from(self.active)
                + u128::from(self.returned_ok)
                + u128::from(self.returned_error)
                + u128::from(self.abandoned)
    }
}

#[derive(Clone, Copy)]
struct Refusal {
    role: Role,
    maximum: u64,
    observed: u64,
}
#[derive(Default)]
struct State {
    admitted: u64,
    rows: [Row; 3],
    refusal: Option<Refusal>,
    tokens: Option<TokenState>,
    receipts: Option<receipts::Ledger>,
}
impl State {
    fn coherent(&self, maximum: u64, token_limit: Option<TokenLimit>) -> bool {
        self.admitted <= maximum
            && self.rows.iter().all(|r| r.coherent())
            && self
                .rows
                .iter()
                .map(|r| u128::from(r.admitted))
                .sum::<u128>()
                == u128::from(self.admitted)
            && self.refusal.is_none_or(|r| {
                r.maximum == maximum && r.observed == maximum && r.observed == self.admitted
            })
            && match (self.tokens, token_limit) {
                (None, None) => true,
                (Some(tokens), Some(limit)) => tokens.coherent(limit, &self.rows),
                _ => false,
            }
            && !(self.refusal.is_some() && self.tokens.is_some_and(|t| t.refusal.is_some()))
            && match (&self.receipts, token_limit) {
                (None, None) => true,
                (Some(ledger), Some(_)) => ledger.coherent(self.admitted, &self.rows),
                _ => false,
            }
    }
}

/// Only an actual-builder RequestMeterScope can construct this authority.
/// No reset, caller gate/role/status/receipt constructor or raw writer exists.
pub(crate) struct Meter {
    gate: Arc<InvocationGate>,
    limit: RequestLimit,
    token_limit: Option<TokenLimit>,
    counter: Option<crate::token_counter::TiktokenCounter>,
    state: Mutex<State>,
    failed: AtomicBool,
}

/// Immutable, private point-in-time facts. Not Debug/Serialize, terminal status,
/// an all-role/OS quiescence proof, or the schema's classified token-usage ledger.
pub(crate) struct Snapshot {
    source: Arc<Meter>,
    maximum: u64,
    admitted: u64,
    rows: [Row; 3],
    refusal: Option<Refusal>,
    tokens: Option<TokenState>,
    receipts: Option<receipts::Ledger>,
}
impl Snapshot {
    pub(in crate::exec) fn maximum(&self) -> u64 {
        self.maximum
    }
    pub(in crate::exec) fn admitted(&self) -> u64 {
        self.admitted
    }
    /// admitted, active, supplied-inner returned-ok, returned-error, abandoned.
    pub(in crate::exec) fn role(&self, role: Role) -> [u64; 5] {
        self.rows[role.index()].values()
    }
    pub(in crate::exec) fn refusal(&self) -> Option<(Role, u64, u64)> {
        self.refusal.map(|r| (r.role, r.maximum, r.observed))
    }
    pub(in crate::exec) fn same_source(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
    }
}

impl Meter {
    pub(crate) fn from_scope(scope: RequestMeterScope) -> Arc<Self> {
        let (gate, limit, token_limit) = scope.into_parts();
        Arc::new(Self {
            gate,
            limit,
            token_limit,
            counter: token_limit.map(|_| crate::token_counter::TiktokenCounter::new()),
            state: Mutex::new(State {
                tokens: token_limit.map(TokenState::new),
                receipts: token_limit.map(|_| receipts::Ledger::default()),
                ..Default::default()
            }),
            failed: AtomicBool::new(false),
        })
    }

    #[cfg(test)]
    pub(crate) fn poison_for_tests(self: &Arc<Self>) {
        let source = self.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owned_guard = source.state.lock().unwrap();
            panic!("synthetic actual-meter poison");
        }));
    }

    pub(crate) fn matches_gate(&self, gate: &Arc<InvocationGate>) -> bool {
        Arc::ptr_eq(&self.gate, gate)
    }

    fn failed(&self) -> Fault {
        self.failed.store(true, Ordering::Release);
        self.gate.close_execution();
        Fault::Accounting
    }
    fn lock(&self) -> Result<MutexGuard<'_, State>, Fault> {
        if self.failed.load(Ordering::Acquire) {
            return Err(Fault::Accounting);
        }
        let state = self.state.lock().map_err(|_| self.failed())?;
        // A waiter may have passed the first flag read before another owner
        // failed; acquiring a coherent old row must not reopen accounting.
        if self.failed.load(Ordering::Acquire) {
            return Err(Fault::Accounting);
        }
        if !state.coherent(self.limit.maximum(), self.token_limit) {
            return Err(self.failed());
        }
        Ok(state)
    }
    fn ready(&self) -> Result<(), Fault> {
        if self.gate.approval_required() {
            return Err(Fault::ApprovalRequired);
        }
        let state = self.lock()?;
        if self.gate.approval_required() {
            Err(Fault::ApprovalRequired)
        } else if state.refusal.is_some() {
            Err(Fault::Limit)
        } else if state.tokens.is_some_and(|t| t.refusal.is_some()) {
            Err(Fault::Tokens)
        } else if self.gate.execution_closed() {
            Err(Fault::Closed)
        } else {
            Ok(())
        }
    }

    /// ONE bounded decision BEFORE the charge. Token preflight and final exact
    /// reply-body checks must succeed before either request/allocation changes.
    /// Request-only scopes cannot bypass a selected token ceiling.
    fn admit(
        self: &Arc<Self>,
        role: Role,
        plan: Option<Plan>,
    ) -> Result<(Permit, Option<Checked>), Fault> {
        if self.token_limit.is_some() != plan.is_some()
            || plan.as_ref().is_some_and(|p| !p.matches_gate(&self.gate))
        {
            return Err(Fault::Foreign);
        }
        if self.gate.approval_required() {
            return Err(Fault::ApprovalRequired);
        }
        let mut state = self.lock()?;
        if self.gate.approval_required() {
            return Err(Fault::ApprovalRequired);
        }
        if state.refusal.is_some() {
            return Err(Fault::Limit);
        }
        if state.tokens.is_some_and(|t| t.refusal.is_some()) {
            return Err(Fault::Tokens);
        }
        if self.gate.execution_closed() {
            return Err(Fault::Closed);
        }
        // Deterministic request-first precedence if BOTH ceilings prevent this
        // attempt. This freezes an actual refusal, never a count heuristic.
        if state.admitted == self.limit.maximum() {
            state.refusal = Some(Refusal {
                role,
                maximum: self.limit.maximum(),
                observed: state.admitted,
            });
            self.gate.close_execution();
            return Err(Fault::Limit);
        }
        let mut next_tokens = state.tokens;
        let (allocation, checked) = match (next_tokens.as_mut(), plan) {
            (Some(tokens), Some(plan)) => {
                let allocation = match self.token_allocation(tokens, role, &plan) {
                    Ok(allocation) => allocation,
                    Err(Fault::Tokens) => {
                        // Preserve ONLY the real first refusal; no call/reserved
                        // total/row increments, rollback or fabricated usage.
                        state.tokens = next_tokens;
                        return Err(Fault::Tokens);
                    }
                    Err(fault) => return Err(fault),
                };
                let result = plan.finish(allocation.output);
                if self.gate.approval_required() {
                    return Err(Fault::ApprovalRequired);
                }
                if self.gate.execution_closed() {
                    return Err(Fault::Closed);
                }
                let checked = result.map_err(|_| Fault::Preflight)?;
                tokens
                    .reserve(role, allocation)
                    .ok_or_else(|| self.failed())?;
                (Some(allocation), Some(checked))
            }
            (None, None) => (None, None),
            _ => return Err(self.failed()),
        };
        // Admit bounded correlation storage BEFORE either request/allocation
        // changes. Capacity alone is not a fabricated call/send/receipt fact.
        if let Some(ledger) = &mut state.receipts {
            ledger
                .reserve_storage(self.limit.maximum())
                .ok_or_else(|| self.failed())?;
        }
        let ordinal = state.admitted.checked_add(1).ok_or_else(|| self.failed())?;
        let mut row = state.rows[role.index()];
        row.admitted = row.admitted.checked_add(1).ok_or_else(|| self.failed())?;
        row.active = row.active.checked_add(1).ok_or_else(|| self.failed())?;
        let mut rows = state.rows;
        rows[role.index()] = row;
        let mut receipts = state.receipts.take();
        if let Some(ledger) = &mut receipts {
            ledger.admit(ordinal, role);
        }
        let next = State {
            admitted: ordinal,
            rows,
            refusal: state.refusal,
            tokens: next_tokens,
            receipts,
        };
        if !next.coherent(self.limit.maximum(), self.token_limit) {
            return Err(self.failed());
        }
        *state = next;
        Ok((
            Permit {
                source: self.clone(),
                role,
                ordinal,
                ended: false,
                allocation,
            },
            checked,
        ))
    }

    fn end(
        &self,
        role: Role,
        ordinal: u64,
        allocation: Option<Allocation>,
        returned: End,
    ) -> Result<(), Fault> {
        let mut state = self.lock()?;
        if ordinal == 0 || ordinal > state.admitted {
            return Err(self.failed());
        }
        let mut row = state.rows[role.index()];
        row.active = row.active.checked_sub(1).ok_or_else(|| self.failed())?;
        let count = match returned {
            End::Ok => &mut row.returned_ok,
            End::Error => &mut row.returned_error,
            End::Abandoned => &mut row.abandoned,
        };
        *count = count.checked_add(1).ok_or_else(|| self.failed())?;
        if !row.coherent() {
            return Err(self.failed());
        }
        let mut next_tokens = state.tokens;
        match (next_tokens.as_mut(), allocation) {
            (Some(tokens), Some(allocation)) => {
                tokens.end(role, allocation).ok_or_else(|| self.failed())?
            }
            (None, None) => (),
            _ => return Err(self.failed()),
        }
        let mut rows = state.rows;
        rows[role.index()] = row;
        let mut receipts = state.receipts.take();
        if let Some(ledger) = &mut receipts {
            ledger
                .end(ordinal, role, returned)
                .ok_or_else(|| self.failed())?;
        }
        let next = State {
            admitted: state.admitted,
            rows,
            refusal: state.refusal,
            tokens: next_tokens,
            receipts,
        };
        if !next.coherent(self.limit.maximum(), self.token_limit) {
            return Err(self.failed());
        }
        *state = next;
        Ok(())
    }

    /// Typed actual stop fact; no diagnostic-string or observed-count heuristic.
    pub(crate) fn limit_refused(&self) -> Result<bool, Fault> {
        Ok(self.lock()?.refusal.is_some())
    }

    pub(in crate::exec) fn snapshot(self: &Arc<Self>) -> Result<Snapshot, Fault> {
        let state = self.lock()?;
        Ok(Snapshot {
            source: self.clone(),
            maximum: self.limit.maximum(),
            admitted: state.admitted,
            rows: state.rows,
            refusal: state.refusal,
            tokens: state.tokens,
            receipts: state.receipts.clone(),
        })
    }
    pub(in crate::exec) fn bind_snapshot(
        self: &Arc<Self>,
        snapshot: &Snapshot,
        gate: &Arc<InvocationGate>,
    ) -> Result<(), Fault> {
        if !Arc::ptr_eq(self, &snapshot.source) || !self.matches_gate(gate) {
            return Err(Fault::Foreign);
        }
        // Never promote a copied row into authority or accept mutated framing.
        let state = State {
            admitted: snapshot.admitted,
            rows: snapshot.rows,
            refusal: snapshot.refusal,
            tokens: snapshot.tokens,
            receipts: snapshot.receipts.clone(),
        };
        if snapshot.maximum != self.limit.maximum()
            || !state.coherent(snapshot.maximum, self.token_limit)
        {
            return Err(Fault::Accounting);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum End {
    Ok,
    Error,
    Abandoned,
}
struct Permit {
    source: Arc<Meter>,
    role: Role,
    ordinal: u64,
    ended: bool,
    allocation: Option<Allocation>,
}
impl Permit {
    fn returned(mut self, ok: bool) -> Result<(), Fault> {
        self.ended = true;
        self.source.end(
            self.role,
            self.ordinal,
            self.allocation,
            if ok { End::Ok } else { End::Error },
        )
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if !self.ended {
            self.ended = true;
            // Any accounting failure is retained by Meter::failed and closes
            // the SAME gate. Drop cannot return it, never repairs/refunds it.
            let _ = self
                .source
                .end(self.role, self.ordinal, self.allocation, End::Abandoned);
        }
    }
}

/// The actual builder alone provides the concrete role AND same gate/meter.
pub(crate) fn wrap_provider(scope: RequestProviderScope) -> Result<Arc<dyn ModelProvider>, Fault> {
    let (source, gate, role, inner) = scope.into_parts();
    if !source.matches_gate(&gate) || source.tokens_enabled() {
        return Err(Fault::Foreign);
    }
    source.ready()?;
    Ok(Arc::new(CountedProvider {
        inner,
        source,
        role,
    }))
}
struct CountedProvider {
    inner: Arc<dyn ModelProvider>,
    source: Arc<Meter>,
    role: Role,
}
#[async_trait]
impl ModelProvider for CountedProvider {
    async fn complete(&self, original: ModelRequest) -> Result<ModelResponse, AgentError> {
        let fixed = |_: Fault| AgentError::Model("[REDACTED]".into());
        let (permit, _) = self.source.admit(self.role, None).map_err(fixed)?;
        let returned = self.inner.complete(original).await;
        permit.returned(returned.is_ok()).map_err(fixed)?;
        // Preserve the real inner-return observation, but a later actual stop
        // dominates eligibility for further framework dispatch/publication.
        self.source.ready().map_err(fixed)?;
        returned
    }
}

// Read-only test facade for app-side builder fixtures. The production snapshot
// API remains exec-only; no raw constructor or production visibility expansion.
#[cfg(test)]
pub(crate) struct TestSnapshot(Snapshot);
#[cfg(test)]
impl TestSnapshot {
    pub(crate) fn maximum(&self) -> u64 {
        self.0.maximum()
    }
    pub(crate) fn admitted(&self) -> u64 {
        self.0.admitted()
    }
    pub(crate) fn role(&self, role: Role) -> [u64; 5] {
        self.0.role(role)
    }
    pub(crate) fn refusal(&self) -> Option<(Role, u64, u64)> {
        self.0.refusal()
    }
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.0.same_source(&other.0)
    }
}
#[cfg(test)]
impl Meter {
    pub(crate) fn snapshot_for_tests(self: &Arc<Self>) -> Result<TestSnapshot, Fault> {
        self.snapshot().map(TestSnapshot)
    }
    pub(crate) fn bind_snapshot_for_tests(
        self: &Arc<Self>,
        snapshot: &TestSnapshot,
        gate: &Arc<InvocationGate>,
    ) -> Result<(), Fault> {
        self.bind_snapshot(&snapshot.0, gate)
    }
}

#[cfg(test)]
pub(crate) struct TestTokenSnapshot(TokenSnapshot);
#[cfg(test)]
impl TestTokenSnapshot {
    pub(crate) fn maximum(&self) -> u64 {
        self.0.maximum()
    }
    pub(crate) fn allocated(&self) -> u64 {
        self.0.allocated()
    }
    pub(crate) fn active(&self) -> u64 {
        self.0.active()
    }
    pub(crate) fn unsettled(&self) -> u64 {
        self.0.unsettled()
    }
    pub(crate) fn role(&self, role: Role) -> [u64; 4] {
        self.0.role(role)
    }
    pub(crate) fn refusal(&self) -> Option<(Role, u64, u64, u64, u64, u64)> {
        self.0.refusal()
    }
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.0.same_source(&other.0)
    }
}
#[cfg(test)]
impl TestSnapshot {
    pub(crate) fn tokens(self) -> Option<TestTokenSnapshot> {
        self.0.tokens().map(TestTokenSnapshot)
    }
}

// App-side fixtures read actual-owner facts without broadening production
// exec-only snapshot visibility or adding caller receipt/status constructors.
#[cfg(test)]
pub(crate) struct TestObservations(receipts::Observations);
#[cfg(test)]
impl TestObservations {
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
    pub(crate) fn call(
        &self,
        index: usize,
    ) -> Option<(
        u64,
        Role,
        Option<receipts::Return>,
        [receipts::SendState; 2],
        [Option<u16>; 2],
    )> {
        self.0.call(index)
    }
    pub(crate) fn totals(&self) -> [u64; 4] {
        self.0.totals()
    }
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.0.same_source(&other.0)
    }
}
#[cfg(test)]
impl TestSnapshot {
    pub(crate) fn receipts(self) -> Option<TestObservations> {
        self.0.receipts().map(TestObservations)
    }
}
