//! Private cumulative reservations, NOT settled token usage or public #97 data.
//! Returned/error/dropped calls retain their allocation until a future actual
//! receipt/partial-output settlement adapter exists. No fake release or billing.
use super::{Fault, Meter, Permit, Role, Row, Snapshot};
use crate::agent::TokenProviderScope;
use crate::exec::provider::Provider;
use crate::exec::provider::request::reservation::Plan;
use crate::exec::provider::request::{Checked, Fault as RequestFault, Source};
use async_trait::async_trait;
use craftsman_harness::{AgentError, ModelProvider, ModelRequest, ModelResponse};
use std::sync::Arc;
use thiserror::Error;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenLimit(u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("token reservation limit must be a positive integer")]
pub(crate) struct InvalidTokenLimit;
impl TokenLimit {
    pub(crate) fn new(maximum: u64) -> Result<Self, InvalidTokenLimit> {
        if maximum == 0 {
            Err(InvalidTokenLimit)
        } else {
            Ok(Self(maximum))
        }
    }
    pub(crate) fn maximum(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct TokenRow {
    input: u64,
    output: u64,
    active: u64,
    unsettled: u64,
}
impl TokenRow {
    fn coherent(self, row: Row) -> bool {
        let total = u128::from(self.input) + u128::from(self.output);
        total == u128::from(self.active) + u128::from(self.unsettled)
            && total >= u128::from(row.admitted)
            && self.active >= row.active
            && row
                .admitted
                .checked_sub(row.active)
                .is_some_and(|n| self.unsettled >= n)
            && (row.active != 0 || self.active == 0)
            && (row.admitted != row.active || self.unsettled == 0)
            && (row.admitted != 0 || total == 0)
    }
    fn values(self) -> [u64; 4] {
        [self.input, self.output, self.active, self.unsettled]
    }
}
#[derive(Clone, Copy)]
pub(super) struct TokenRefusal {
    pub(super) role: Role,
    pub(super) maximum: u64,
    pub(super) allocated: u64,
    pub(super) input: u64,
    pub(super) reply: u64,
    pub(super) remaining: u64,
}
#[derive(Clone, Copy)]
pub(super) struct TokenState {
    pub(super) maximum: u64,
    pub(super) allocated: u64,
    rows: [TokenRow; 3],
    pub(super) refusal: Option<TokenRefusal>,
}
impl TokenState {
    pub(super) fn new(limit: TokenLimit) -> Self {
        Self {
            maximum: limit.maximum(),
            allocated: 0,
            rows: [TokenRow::default(); 3],
            refusal: None,
        }
    }
    pub(super) fn coherent(self, limit: TokenLimit, rows: &[Row; 3]) -> bool {
        self.maximum == limit.maximum()
            && self.allocated <= self.maximum
            && self.rows.iter().zip(rows).all(|(t, r)| t.coherent(*r))
            && self
                .rows
                .iter()
                .map(|r| u128::from(r.input) + u128::from(r.output))
                .sum::<u128>()
                == u128::from(self.allocated)
            && self.refusal.is_none_or(|r| {
                r.maximum == self.maximum
                    && r.allocated == self.allocated
                    && r.remaining == self.maximum - self.allocated
                    && r.reply == 1
                    && u128::from(r.input) + u128::from(r.reply) > u128::from(r.remaining)
            })
    }
    pub(super) fn reserve(&mut self, role: Role, allocation: Allocation) -> Option<()> {
        let amount = allocation.total()?;
        self.allocated = self
            .allocated
            .checked_add(amount)
            .filter(|n| *n <= self.maximum)?;
        let row = &mut self.rows[role.index()];
        row.input = row.input.checked_add(allocation.input)?;
        row.output = row.output.checked_add(allocation.output)?;
        row.active = row.active.checked_add(amount)?;
        Some(())
    }
    pub(super) fn end(&mut self, role: Role, allocation: Allocation) -> Option<()> {
        let amount = allocation.total()?;
        let row = &mut self.rows[role.index()];
        row.active = row.active.checked_sub(amount)?;
        row.unsettled = row.unsettled.checked_add(amount)?;
        Some(())
    }
    fn active(self) -> u64 {
        // Coherence is checked BEFORE a snapshot is minted. Wider sums cannot
        // silently manufacture an apparent free allowance at an integer edge.
        self.rows.iter().map(|r| u128::from(r.active)).sum::<u128>() as u64
    }
    fn unsettled(self) -> u64 {
        self.rows
            .iter()
            .map(|r| u128::from(r.unsettled))
            .sum::<u128>() as u64
    }
}
#[derive(Clone, Copy)]
pub(super) struct Allocation {
    pub(super) input: u64,
    pub(super) output: u64,
}
impl Allocation {
    pub(super) fn total(self) -> Option<u64> {
        self.input.checked_add(self.output)
    }
}
/// Pure arithmetic, not a caller estimate/usage/receipt constructor.
fn allocate(input: u64, ceiling: u64, remaining: u64) -> Option<Allocation> {
    if ceiling == 0 || u128::from(input) + 1 > u128::from(remaining) {
        return None;
    }
    Some(Allocation {
        input,
        output: ceiling.min(remaining - input),
    })
}

/// Owns the ACTUAL builder-selected concrete client, role and same meter/gate.
/// It cannot accept a caller provider/route/estimate/receipt or role label.
struct ReservedProvider {
    original: Provider,
    source: Arc<Meter>,
    role: Role,
}
pub(crate) fn wrap_provider(scope: TokenProviderScope) -> Result<Arc<dyn ModelProvider>, Fault> {
    let (source, gate, role, original) = scope.into_parts();
    if !source.matches_gate(&gate) || !source.tokens_enabled() {
        return Err(Fault::Foreign);
    }
    source.ready()?;
    Ok(Arc::new(ReservedProvider {
        original,
        source,
        role,
    }))
}

/// Exact checked body PLUS a non-clone actual combined-admission permit. Only
/// the reserved provider creates this; the ordinary client cannot send it.
/// No Debug/Serialize/raw bytes/receipt constructor or detached grant exists.
pub(crate) struct Admitted {
    checked: Checked,
    permit: Option<Permit>,
}
impl Admitted {
    pub(crate) fn checked(&self) -> &Checked {
        &self.checked
    }
    pub(crate) fn ready_for(&self, source: &Arc<Source>) -> Result<(), RequestFault> {
        source.ready(&self.checked)?;
        let permit = self.permit.as_ref().ok_or(RequestFault::Collision)?;
        if permit.ended {
            return Err(RequestFault::Collision);
        }
        permit.source.ready().map_err(|fault| match fault {
            Fault::ApprovalRequired | Fault::Closed | Fault::Limit | Fault::Tokens => {
                RequestFault::Stopped
            }
            _ => RequestFault::Collision,
        })
    }
    /// Only the actual admitted body can bind this client's exact request lease.
    pub(crate) fn observation_scope(
        &self,
        lease: crate::exec::provider::request::Lease,
    ) -> Result<super::receipts::Scope<'_>, Fault> {
        let permit = self.permit.as_ref().ok_or(Fault::Accounting)?;
        super::receipts::Scope::from_scope(ObservationScope {
            permit,
            checked: &self.checked,
            lease,
        })
    }
    fn returned(mut self, ok: bool) -> Result<(), Fault> {
        self.permit.take().ok_or(Fault::Accounting)?.returned(ok)
    }
}
#[async_trait]
impl ModelProvider for ReservedProvider {
    async fn complete(&self, original: ModelRequest) -> Result<ModelResponse, AgentError> {
        let fixed = |_: Fault| AgentError::Model("[REDACTED]".into());
        self.source.ready().map_err(fixed)?;
        let checked = self
            .original
            .prepare_reservation(&original)
            .map_err(|_| fixed(Fault::Preflight))?;
        let counter = self
            .source
            .counter
            .as_ref()
            .ok_or_else(|| fixed(Fault::Accounting))?;
        let plan = Plan::prepare(checked, counter).map_err(|_| fixed(Fault::Preflight))?;
        let (permit, checked) = self.source.admit(self.role, Some(plan)).map_err(fixed)?;
        let admitted = Admitted {
            checked: checked.ok_or_else(|| fixed(Fault::Accounting))?,
            permit: Some(permit),
        };
        let returned = self.original.complete_reserved(&admitted).await;
        admitted.returned(returned.is_ok()).map_err(fixed)?;
        // Record the actual return without promoting Usage's indistinguishable
        // missing/zero fields into a reported receipt or releasing allocation.
        self.source.ready().map_err(fixed)?;
        returned
    }
}

impl Meter {
    pub(crate) fn tokens_enabled(&self) -> bool {
        self.token_limit.is_some()
    }
    pub(super) fn token_allocation(
        &self,
        state: &mut TokenState,
        role: Role,
        plan: &Plan,
    ) -> Result<Allocation, Fault> {
        let remaining = state
            .maximum
            .checked_sub(state.allocated)
            .ok_or_else(|| self.failed())?;
        match allocate(plan.input(), plan.ceiling(), remaining) {
            Some(allocation) => Ok(allocation),
            None => {
                state.refusal = Some(TokenRefusal {
                    role,
                    maximum: state.maximum,
                    allocated: state.allocated,
                    input: plan.input(),
                    reply: 1,
                    remaining,
                });
                self.gate.close_execution();
                Err(Fault::Tokens)
            }
        }
    }
    pub(crate) fn tokens_refused(&self) -> Result<bool, Fault> {
        Ok(self.lock()?.tokens.is_some_and(|t| t.refusal.is_some()))
    }
}

/// Historical PRIVATE allocations from the actual Run. Not measured input/
/// output usage, #97's observed/remaining, classified calls, billing or receipts.
pub(crate) struct TokenSnapshot {
    source: Snapshot,
    facts: TokenState,
}
impl TokenSnapshot {
    pub(in crate::exec) fn maximum(&self) -> u64 {
        self.facts.maximum
    }
    pub(in crate::exec) fn allocated(&self) -> u64 {
        self.facts.allocated
    }
    pub(in crate::exec) fn active(&self) -> u64 {
        self.facts.active()
    }
    pub(in crate::exec) fn unsettled(&self) -> u64 {
        self.facts.unsettled()
    }
    /// Estimated input / bounded reply / active allocation / retained unsettled.
    pub(in crate::exec) fn role(&self, role: Role) -> [u64; 4] {
        self.facts.rows[role.index()].values()
    }
    pub(in crate::exec) fn refusal(&self) -> Option<(Role, u64, u64, u64, u64, u64)> {
        self.facts.refusal.map(|r| {
            (
                r.role,
                r.maximum,
                r.allocated,
                r.input,
                r.reply,
                r.remaining,
            )
        })
    }
    pub(in crate::exec) fn same_source(&self, other: &Self) -> bool {
        self.source.same_source(&other.source)
    }
}
impl Snapshot {
    pub(in crate::exec) fn tokens(self) -> Option<TokenSnapshot> {
        self.tokens.map(|facts| TokenSnapshot {
            source: self,
            facts,
        })
    }
}

#[cfg(test)]
mod reservation_tests {
    use super::*;
    #[test]
    fn positive_limit_has_no_float_repair_or_overflow() {
        for n in [1, 50, u64::MAX] {
            assert_eq!(TokenLimit::new(n).unwrap().maximum(), n);
        }
        assert!(TokenLimit::new(0).is_err());
    }
    #[test]
    fn exact_minimum_fit_is_admitted_and_reply_is_clamped() {
        let a = allocate(80, 4096, 81).unwrap();
        assert_eq!((a.input, a.output, a.total()), (80, 1, Some(81)));
    }
    #[test]
    fn required_reply_not_inflated_ceiling_decides_refusal() {
        for ceiling in [1, 4096, u64::MAX] {
            assert!(allocate(80, ceiling, 70).is_none());
            assert_eq!(allocate(80, ceiling, 81).unwrap().output, 1);
        }
    }
    #[test]
    fn zero_estimate_still_requires_a_real_positive_reply() {
        assert!(allocate(0, 1, 0).is_none());
        assert_eq!(allocate(0, 4096, 1).unwrap().total(), Some(1));
    }
    #[test]
    fn requested_ceiling_not_remaining_is_the_upper_bound() {
        assert_eq!(allocate(30, 10, 100).unwrap().output, 10);
        assert_eq!(allocate(30, 100, 40).unwrap().output, 10);
        assert!(allocate(30, 0, 100).is_none());
    }
    #[test]
    fn u64_sum_and_subtraction_edges_never_wrap() {
        assert!(allocate(u64::MAX, 1, u64::MAX).is_none());
        let a = allocate(u64::MAX - 1, u64::MAX, u64::MAX).unwrap();
        assert_eq!((a.output, a.total()), (1, Some(u64::MAX)));
        assert_eq!(
            allocate(0, u64::MAX, u64::MAX).unwrap().total(),
            Some(u64::MAX)
        );
    }
}

// Test-only actual-owner observations/entry; no production authority/visibility
// expansion or caller estimate/receipt constructors for app-side fixtures.
#[cfg(test)]
fn prepare_for_tests(
    scope: TokenProviderScope,
    original: &ModelRequest,
) -> Result<(Arc<Meter>, Role, Plan), Fault> {
    let (source, gate, role, provider) = scope.into_parts();
    if !source.matches_gate(&gate) || !source.tokens_enabled() {
        return Err(Fault::Foreign);
    }
    source.ready()?;
    let checked = provider
        .prepare_reservation(original)
        .map_err(|_| Fault::Preflight)?;
    let plan = Plan::prepare(checked, source.counter.as_ref().ok_or(Fault::Accounting)?)
        .map_err(|_| Fault::Preflight)?;
    if !plan.matches_gate(&gate) {
        return Err(Fault::Foreign);
    }
    Ok((source, role, plan))
}
#[cfg(test)]
pub(crate) fn input_for_tests(
    scope: TokenProviderScope,
    original: &ModelRequest,
) -> Result<u64, Fault> {
    Ok(prepare_for_tests(scope, original)?.2.input())
}
#[cfg(test)]
pub(crate) fn admit_for_tests(
    scope: TokenProviderScope,
    original: &ModelRequest,
) -> Result<Admitted, Fault> {
    let (source, role, plan) = prepare_for_tests(scope, original)?;
    let (permit, checked) = source.admit(role, Some(plan))?;
    Ok(Admitted {
        checked: checked.ok_or(Fault::Accounting)?,
        permit: Some(permit),
    })
}

/// Opaque actual-admission lease; no caller meter/role/ordinal/checked constructor.
pub(super) struct ObservationScope<'a> {
    permit: &'a super::Permit,
    checked: &'a Checked,
    lease: crate::exec::provider::request::Lease,
}
impl<'a> ObservationScope<'a> {
    pub(super) fn into_parts(
        self,
    ) -> (
        &'a super::Permit,
        &'a Checked,
        crate::exec::provider::request::Lease,
    ) {
        (self.permit, self.checked, self.lease)
    }
}

#[cfg(test)]
pub(crate) fn observation_admit_for_tests(
    scope: TokenProviderScope,
    original: &ModelRequest,
) -> Result<(Provider, Admitted), Fault> {
    let (source, gate, role, provider) = scope.into_parts();
    if !source.matches_gate(&gate) || !source.tokens_enabled() {
        return Err(Fault::Foreign);
    }
    source.ready()?;
    let checked = provider
        .prepare_reservation(original)
        .map_err(|_| Fault::Preflight)?;
    let plan = Plan::prepare(checked, source.counter.as_ref().ok_or(Fault::Accounting)?)
        .map_err(|_| Fault::Preflight)?;
    let (permit, checked) = source.admit(role, Some(plan))?;
    Ok((
        provider,
        Admitted {
            checked: checked.ok_or(Fault::Accounting)?,
            permit: Some(permit),
        },
    ))
}
