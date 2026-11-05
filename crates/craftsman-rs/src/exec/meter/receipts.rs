//! Admission-correlated PRIVATE send-future/header facts. Never token usage,
//! billing, body completion or reservation settlement. Client-only witnesses
//! retain real polls/results; the same bounded meter owns every concrete role.
use super::tokens::ObservationScope;
use super::{End, Fault, Permit, Role, Row, Snapshot};
use crate::exec::provider::request::Lease;
use crate::llm::{PolledSend, Received, SendFailed, Sent};
use std::sync::Arc;

const ATTEMPTS: usize = 2;
const CALLS: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendState {
    Unprepared,
    Prepared,
    NeverPolled,
    Polled,
    SendError,
    Received,
    Abandoned,
}
#[derive(Clone)]
struct Slot {
    identity: Option<Arc<()>>,
    state: SendState,
    status: Option<u16>,
}
impl Default for Slot {
    fn default() -> Self {
        Self {
            identity: None,
            state: SendState::Unprepared,
            status: None,
        }
    }
}
impl Slot {
    fn coherent(&self, ended: bool) -> bool {
        match self.state {
            SendState::Unprepared => self.identity.is_none() && self.status.is_none(),
            SendState::Prepared | SendState::Polled => {
                !ended && self.identity.is_some() && self.status.is_none()
            }
            SendState::NeverPolled | SendState::SendError | SendState::Abandoned => {
                self.identity.is_some() && self.status.is_none()
            }
            SendState::Received => {
                self.identity.is_some() && self.status.is_some_and(|s| (100..=999).contains(&s))
            }
        }
    }
    fn polled(&self) -> bool {
        matches!(
            self.state,
            SendState::Polled | SendState::SendError | SendState::Received | SendState::Abandoned
        )
    }
}
#[derive(Clone)]
struct Call {
    ordinal: u64,
    role: Role,
    lease: Option<Lease>,
    ended: Option<End>,
    slots: [Slot; ATTEMPTS],
}
impl Call {
    fn coherent(&self) -> bool {
        self.slots.iter().all(|s| s.coherent(self.ended.is_some()))
            && (self.slots.iter().all(|s| s.state == SendState::Unprepared) || self.lease.is_some())
            && (self.slots[1].state == SendState::Unprepared
                || matches!(
                    self.slots[0].state,
                    SendState::SendError | SendState::Received
                ))
    }
}
#[derive(Clone, Default)]
pub(super) struct Ledger {
    calls: Vec<Call>,
}
impl Ledger {
    pub(super) fn coherent(&self, admitted: u64, rows: &[Row; 3]) -> bool {
        if self.calls.len() > CALLS || self.calls.len() as u64 != admitted {
            return false;
        }
        let mut sums = [[0u128; 5]; 3];
        for (i, c) in self.calls.iter().enumerate() {
            if c.ordinal != i as u64 + 1 || !c.coherent() {
                return false;
            }
            let sum = &mut sums[c.role.index()];
            sum[0] += 1;
            sum[match c.ended {
                None => 1,
                Some(End::Ok) => 2,
                Some(End::Error) => 3,
                Some(End::Abandoned) => 4,
            }] += 1;
        }
        sums.iter().zip(rows).all(|(sum, row)| {
            sum.iter()
                .zip(row.values())
                .all(|(s, r)| *s == u128::from(r))
        })
    }
    pub(super) fn reserve_storage(&mut self, maximum: u64) -> Option<()> {
        if self.calls.len() >= CALLS || self.calls.len() as u64 >= maximum {
            return None;
        }
        self.calls.try_reserve(1).ok()
    }
    /// Only the SAME already-admitted next State inserts its concrete role/key.
    pub(super) fn admit(&mut self, ordinal: u64, role: Role) {
        self.calls.push(Call {
            ordinal,
            role,
            lease: None,
            ended: None,
            slots: std::array::from_fn(|_| Slot::default()),
        });
    }
    fn call_mut(&mut self, ordinal: u64, role: Role) -> Option<&mut Call> {
        let index = usize::try_from(ordinal.checked_sub(1)?).ok()?;
        let call = self.calls.get_mut(index)?;
        (call.ordinal == ordinal && call.role == role && call.ended.is_none()).then_some(call)
    }
    pub(super) fn end(&mut self, ordinal: u64, role: Role, ended: End) -> Option<()> {
        let call = self.call_mut(ordinal, role)?;
        for slot in &mut call.slots {
            slot.state = match slot.state {
                SendState::Prepared => SendState::NeverPolled,
                SendState::Polled => SendState::Abandoned,
                other => other,
            };
        }
        call.ended = Some(ended);
        Some(())
    }
}

/// Borrowed ONLY from one actual token-admitted body/permit/checked lease.
/// No arbitrary source/role/ordinal/gate/status constructor or Clone exists.
pub(crate) struct Scope<'a> {
    permit: &'a Permit,
    lease: Lease,
}
impl<'a> Scope<'a> {
    pub(super) fn from_scope(scope: ObservationScope<'a>) -> Result<Self, Fault> {
        let (permit, checked, lease) = scope.into_parts();
        if permit.ended || permit.allocation.is_none() || !checked.matches_lease(&lease) {
            return Err(Fault::Foreign);
        }
        permit.source.ready()?;
        Ok(Self { permit, lease })
    }
    fn witness(&self, lease: &Lease, ordinal: usize) -> Result<usize, Fault> {
        if !self.lease.same_request(lease) {
            return Err(Fault::Foreign);
        }
        ordinal
            .checked_sub(1)
            .filter(|i| *i < ATTEMPTS)
            .ok_or_else(|| self.permit.source.failed())
    }
    /// Preparation is not a poll/send. Only client-created Sent supplies the
    /// actual loop ordinal; subsequent PolledSend supplies a real first poll.
    pub(crate) fn prepare(&self, sent: &Sent) -> Result<Ticket<'_>, Fault> {
        let (lease, ordinal) = sent.metadata();
        let index = self.witness(lease, ordinal)?;
        let source = &self.permit.source;
        source.ready()?;
        let mut state = source.lock()?;
        if source.gate.approval_required() {
            return Err(Fault::ApprovalRequired);
        }
        if source.gate.execution_closed() {
            return Err(Fault::Closed);
        }
        let call = state
            .receipts
            .as_mut()
            .and_then(|l| l.call_mut(self.permit.ordinal, self.permit.role))
            .ok_or_else(|| source.failed())?;
        if call.slots[index].state != SendState::Unprepared
            || (index == 1
                && !matches!(
                    call.slots[0].state,
                    SendState::SendError | SendState::Received
                ))
            || call
                .lease
                .as_ref()
                .is_some_and(|l| !l.same_request(&self.lease))
        {
            return Err(source.failed());
        }
        let identity = Arc::new(());
        call.lease.get_or_insert_with(|| self.lease.clone());
        call.slots[index].identity = Some(identity.clone());
        call.slots[index].state = SendState::Prepared;
        Ok(Ticket {
            scope: self,
            index,
            identity,
            ended: false,
            polled: false,
        })
    }
}

pub(crate) struct Ticket<'a> {
    scope: &'a Scope<'a>,
    index: usize,
    identity: Arc<()>,
    ended: bool,
    polled: bool,
}
impl Ticket<'_> {
    fn update(
        &self,
        state: SendState,
        status: Option<u16>,
        expected: SendState,
    ) -> Result<(), Fault> {
        let source = &self.scope.permit.source;
        let mut ledger = source.lock()?;
        let call = ledger
            .receipts
            .as_mut()
            .and_then(|l| l.call_mut(self.scope.permit.ordinal, self.scope.permit.role))
            .ok_or_else(|| source.failed())?;
        let slot = &mut call.slots[self.index];
        if slot.state != expected
            || slot
                .identity
                .as_ref()
                .is_none_or(|id| !Arc::ptr_eq(id, &self.identity))
        {
            return Err(source.failed());
        }
        slot.state = state;
        slot.status = status;
        if !call.coherent() {
            return Err(source.failed());
        }
        Ok(())
    }
    /// Before first poll only: already-started send futures are not interrupted.
    pub(crate) fn before_poll(&self) -> Result<(), Fault> {
        if self.ended || self.polled {
            return Err(self.scope.permit.source.failed());
        }
        self.scope.permit.source.ready()?;
        Ok(())
    }
    /// The real first poll has ALREADY happened. Late stop cannot erase it.
    pub(crate) fn observed_poll(&mut self, polled: PolledSend) -> Result<(), Fault> {
        let (lease, ordinal) = polled.into_parts();
        if self
            .scope
            .witness(&lease, ordinal)
            .map_err(|_| self.scope.permit.source.failed())?
            != self.index
            || self.ended
            || self.polled
        {
            return Err(self.scope.permit.source.failed());
        }
        self.update(SendState::Polled, None, SendState::Prepared)?;
        self.polled = true;
        Ok(())
    }
    pub(crate) fn failed(&mut self, failed: SendFailed) -> Result<(), Fault> {
        let (lease, ordinal) = failed.into_parts();
        if self
            .scope
            .witness(&lease, ordinal)
            .map_err(|_| self.scope.permit.source.failed())?
            != self.index
            || self.ended
            || !self.polled
        {
            return Err(self.scope.permit.source.failed());
        }
        self.update(SendState::SendError, None, SendState::Polled)?;
        self.ended = true;
        Ok(())
    }
    pub(crate) fn received(&mut self, received: &Received) -> Result<(), Fault> {
        let (lease, ordinal, status) = received.metadata();
        if self
            .scope
            .witness(lease, ordinal)
            .map_err(|_| self.scope.permit.source.failed())?
            != self.index
            || self.ended
            || !self.polled
        {
            return Err(self.scope.permit.source.failed());
        }
        self.update(SendState::Received, Some(status), SendState::Polled)?;
        self.ended = true;
        Ok(())
    }
}
impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        if !self.ended {
            self.ended = true;
            let _ = self.update(
                if self.polled {
                    SendState::Abandoned
                } else {
                    SendState::NeverPolled
                },
                None,
                if self.polled {
                    SendState::Polled
                } else {
                    SendState::Prepared
                },
            );
        }
    }
}

/// Immutable selected historical facts, not public/billing/usage/terminal data.
/// Source stays private; equal fresh numbers are not identity aliases.
pub(crate) struct Observations {
    snapshot: Snapshot,
}
impl Observations {
    pub(in crate::exec) fn len(&self) -> usize {
        self.snapshot.receipts.as_ref().map_or(0, |l| l.calls.len())
    }
    pub(in crate::exec) fn call(
        &self,
        index: usize,
    ) -> Option<(u64, Role, Option<Return>, [SendState; 2], [Option<u16>; 2])> {
        let call = self.snapshot.receipts.as_ref()?.calls.get(index)?;
        Some((
            call.ordinal,
            call.role,
            call.ended.map(|e| match e {
                End::Ok => Return::Ok,
                End::Error => Return::Error,
                End::Abandoned => Return::Abandoned,
            }),
            std::array::from_fn(|i| call.slots[i].state),
            std::array::from_fn(|i| call.slots[i].status),
        ))
    }
    /// Actual first-poll observations / response headers / actual send errors /
    /// abandoned polled sends. Not HTTP wire counts, body/usage or billing.
    pub(in crate::exec) fn totals(&self) -> [u64; 4] {
        let mut counts = [0u64; 4];
        if let Some(ledger) = &self.snapshot.receipts {
            for call in &ledger.calls {
                for slot in &call.slots {
                    counts[0] += u64::from(slot.polled());
                    counts[1] += u64::from(slot.state == SendState::Received);
                    counts[2] += u64::from(slot.state == SendState::SendError);
                    counts[3] += u64::from(slot.state == SendState::Abandoned);
                }
            }
        }
        counts
    }
    pub(in crate::exec) fn same_source(&self, other: &Self) -> bool {
        self.snapshot.same_source(&other.snapshot)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Return {
    Ok,
    Error,
    Abandoned,
}
impl Snapshot {
    pub(in crate::exec) fn receipts(self) -> Option<Observations> {
        self.receipts.as_ref()?;
        Some(Observations { snapshot: self })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(n: u64, active: u64, ok: u64, error: u64, abandoned: u64) -> Row {
        Row {
            admitted: n,
            active,
            returned_ok: ok,
            returned_error: error,
            abandoned,
        }
    }
    fn ledger(n: u64) -> Ledger {
        let mut l = Ledger::default();
        for i in 1..=n {
            l.reserve_storage(n).unwrap();
            l.admit(i, Role::Main);
        }
        l
    }
    #[test]
    fn empty_selected_ledger_is_coherent_not_a_wire_or_usage_receipt() {
        assert!(Ledger::default().coherent(0, &[Row::default(); 3]));
    }
    #[test]
    fn missing_or_extra_admission_counts_fail_without_zeroing_original_ledger() {
        let l = ledger(1);
        let rows = [row(1, 1, 0, 0, 0), Row::default(), Row::default()];
        assert!(!l.coherent(0, &rows));
        assert!(!l.coherent(2, &rows));
        assert_eq!(l.calls.len(), 1);
        assert_eq!(l.calls[0].ordinal, 1);
        assert!(l.coherent(1, &rows));
    }
    #[test]
    fn every_logical_key_and_concrete_role_sum_must_match() {
        let mut l = ledger(2);
        let rows = [row(2, 2, 0, 0, 0), Row::default(), Row::default()];
        assert!(l.coherent(2, &rows));
        l.calls[1].ordinal = 1;
        assert!(!l.coherent(2, &rows));
        l.calls[1].ordinal = 2;
        l.calls[1].role = Role::Subagent;
        assert!(!l.coherent(2, &rows));
    }
    #[test]
    fn ending_a_call_preserves_admission_and_exact_return_classification() {
        let mut l = ledger(3);
        l.end(1, Role::Main, End::Ok).unwrap();
        l.end(2, Role::Main, End::Error).unwrap();
        l.end(3, Role::Main, End::Abandoned).unwrap();
        assert!(l.coherent(3, &[row(3, 0, 1, 1, 1), Row::default(), Row::default()]));
        assert!(l.end(3, Role::Main, End::Ok).is_none());
    }
    #[test]
    fn slot_identity_status_and_state_are_independently_coherent() {
        let mut s = Slot::default();
        assert!(s.coherent(false));
        s.state = SendState::Received;
        assert!(!s.coherent(false));
        s.identity = Some(Arc::new(()));
        assert!(!s.coherent(false));
        s.status = Some(200);
        assert!(s.coherent(true));
        s.status = Some(99);
        assert!(!s.coherent(false));
        s.status = Some(1000);
        assert!(!s.coherent(false));
        s.state = SendState::SendError;
        assert!(!s.coherent(false));
        s.status = None;
        assert!(s.coherent(false));
    }
    #[test]
    fn preparing_or_never_polling_is_not_a_send_future_poll() {
        for state in [
            SendState::Unprepared,
            SendState::Prepared,
            SendState::NeverPolled,
        ] {
            let s = Slot {
                identity: (state != SendState::Unprepared).then(|| Arc::new(())),
                state,
                status: None,
            };
            assert!(!s.polled());
        }
        for state in [
            SendState::Polled,
            SendState::SendError,
            SendState::Received,
            SendState::Abandoned,
        ] {
            let s = Slot {
                identity: Some(Arc::new(())),
                state,
                status: (state == SendState::Received).then_some(200),
            };
            assert!(s.polled());
        }
    }
    #[test]
    fn second_attempt_requires_a_real_returned_first_slot() {
        let mut l = ledger(1);
        l.calls[0].slots[1] = Slot {
            identity: Some(Arc::new(())),
            state: SendState::Prepared,
            status: None,
        };
        assert!(!l.calls[0].coherent());
        // Pure coherence data, not an actual live witness/receipt constructor.
        l.calls[0].slots[0] = Slot {
            identity: Some(Arc::new(())),
            state: SendState::SendError,
            status: None,
        };
        assert!(
            !l.calls[0].coherent(),
            "a real source lease is independently required"
        );
    }
    #[test]
    fn ended_calls_cannot_keep_prepared_or_pending_poll_states() {
        let mut l = ledger(1);
        l.calls[0].slots[0] = Slot {
            identity: Some(Arc::new(())),
            state: SendState::Prepared,
            status: None,
        };
        l.end(1, Role::Main, End::Abandoned).unwrap();
        assert_eq!(l.calls[0].slots[0].state, SendState::NeverPolled);
        let mut l = ledger(1);
        l.calls[0].slots[0] = Slot {
            identity: Some(Arc::new(())),
            state: SendState::Polled,
            status: None,
        };
        l.end(1, Role::Main, End::Error).unwrap();
        assert_eq!(l.calls[0].slots[0].state, SendState::Abandoned);
    }
    #[test]
    fn bounded_storage_accepts_last_call_and_refuses_an_extra_slot_without_reset() {
        let mut l = ledger(10_000);
        assert_eq!(l.calls.len(), 10_000);
        assert!(l.reserve_storage(10_000).is_none());
        assert!(l.reserve_storage(u64::MAX).is_none());
        assert!(l.coherent(
            10_000,
            &[row(10_000, 10_000, 0, 0, 0), Row::default(), Row::default()]
        ));
    }
    #[test]
    fn zero_foreign_or_overflowing_logical_keys_cannot_end_a_call() {
        let mut l = ledger(1);
        for n in [0, 2, u64::MAX] {
            assert!(l.end(n, Role::Main, End::Ok).is_none());
        }
        assert!(l.end(1, Role::Subagent, End::Ok).is_none());
        assert!(l.coherent(1, &[row(1, 1, 0, 0, 0), Row::default(), Row::default()]));
    }
}
