//! Opaque private gate -> actual UI receiver -> actual answer linkage.
//! UI ownership and these data types alone prove no human event. The sole
//! production presentation/answer callsites must be the real TUI modal/choice
//! handlers, independently reviewed with the app bootstrap that registers them.

use super::{HumanChoice, InlineDecisionProducer};
use crate::privacy_session::{ConversationBoundary, FileRefusal, SessionStop};
use craftsman_harness::ToolCall;
use craftsman_harness::provenance::{ActualPlannedInvocation, SourceRef};
use std::sync::{Arc, Mutex, Weak};

type Callback =
    Arc<dyn Fn(PrivacyApprovalRequest) -> Option<PrivacyApprovalDecision> + Send + Sync>;

struct UiOwner;
pub(crate) struct PrivacyApprovalUi(Arc<UiOwner>);
pub(crate) struct PrivacyApprovalPrompt {
    owner: Weak<UiOwner>,
    callback: Callback,
}

/// A creates this pair at the actual fresh TUI receiver registration. Only the
/// receiver/modal owns Ui; the callback carries the event sender and returns
/// the original opaque decision received from that receiver, never a factory.
pub(crate) fn register_privacy_approval_ui(
    callback: Callback,
) -> (Arc<PrivacyApprovalPrompt>, Arc<PrivacyApprovalUi>) {
    let ui = Arc::new(PrivacyApprovalUi(Arc::new(UiOwner)));
    let prompt = Arc::new(PrivacyApprovalPrompt {
        owner: Arc::downgrade(&ui.0),
        callback,
    });
    (prompt, ui)
}

#[derive(Clone)]
pub(crate) struct PrivacyApprovalRequest(Arc<Request>);
struct Request {
    gate: Weak<InlineDecisionProducer>,
    ui: Weak<UiOwner>,
    lane: Arc<ConversationBoundary>,
    planned: ActualPlannedInvocation,
    review_source: Option<SourceRef>,
    call: ToolCall,
    summary: String,
    details: String,
    shown: Option<[u8; 32]>,
    state: Mutex<RequestState>,
}
struct RequestState {
    callback_open: bool,
    presented: Option<Arc<()>>,
    answered: Option<Arc<Decision>>,
    consumed: bool,
    returned: Option<PrivacyApprovalDecision>,
}

pub(crate) struct PrivacyApprovalPresentation(Arc<Presented>);
struct Presented {
    request: PrivacyApprovalRequest,
    ui: Weak<UiOwner>,
    identity: Arc<()>,
}
#[derive(Clone)]
pub(crate) struct PrivacyApprovalDecision(Arc<Decision>);
struct Decision {
    request: Weak<Request>,
    presentation: Arc<()>,
    ui: Weak<UiOwner>,
    choice: HumanChoice,
}

pub(crate) struct PrivacyApprovalView<'a> {
    pub(crate) call: &'a ToolCall,
    pub(crate) summary: &'a str,
    pub(crate) details: &'a str,
    pub(crate) shown: Option<[u8; 32]>,
}

impl PrivacyApprovalPrompt {
    pub(super) fn issue(
        &self,
        gate: &Arc<InlineDecisionProducer>,
        lane: Arc<ConversationBoundary>,
        planned: ActualPlannedInvocation,
        review_source: Option<SourceRef>,
        call: ToolCall,
        summary: String,
        details: String,
        shown: Option<[u8; 32]>,
    ) -> Result<PrivacyApprovalRequest, FileRefusal> {
        if self.owner.upgrade().is_none() {
            return Err(FileRefusal::NotApproved);
        }
        Ok(PrivacyApprovalRequest(Arc::new(Request {
            gate: Arc::downgrade(gate),
            ui: self.owner.clone(),
            lane,
            planned,
            review_source,
            call,
            summary,
            details,
            shown,
            state: Mutex::new(RequestState {
                callback_open: true,
                presented: None,
                answered: None,
                consumed: false,
                returned: None,
            }),
        })))
    }
    pub(super) fn read(&self, request: PrivacyApprovalRequest) -> Option<PrivacyApprovalDecision> {
        (self.callback)(request)
    }
}

impl PrivacyApprovalRequest {
    pub(super) fn issued_review(
        &self,
    ) -> (
        &ActualPlannedInvocation,
        Option<&SourceRef>,
        Option<[u8; 32]>,
    ) {
        (&self.0.planned, self.0.review_source.as_ref(), self.0.shown)
    }
    fn live(&self) -> Result<(), FileRefusal> {
        let gate = self.0.gate.upgrade().ok_or(FileRefusal::NotApproved)?;
        gate.owns_request(self)?;
        self.0
            .planned
            .check_live()
            .map_err(|_| FileRefusal::NotApproved)?;
        self.0
            .lane
            .require_agent(self.0.planned.owner())
            .map_err(|_| FileRefusal::Stopped(SessionStop::Unbound))
    }
    pub(super) fn is_for(&self, gate: &Arc<InlineDecisionProducer>) -> bool {
        self.0
            .gate
            .upgrade()
            .is_some_and(|issued| Arc::ptr_eq(&issued, gate))
    }
    pub(super) fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub(super) fn own_callback(&self) -> CallbackOwnership {
        CallbackOwnership(self.clone())
    }
    pub(super) fn close_callback(&self) {
        self.0.state.lock().unwrap().callback_open = false;
    }
    pub(super) fn consume(
        &self,
        decision: Option<PrivacyApprovalDecision>,
    ) -> Result<HumanChoice, FileRefusal> {
        // Retain what actually returned before a late source/liveness check.
        self.0.state.lock().unwrap().returned = decision.clone();
        self.live()?;
        let mut state = self.0.state.lock().unwrap();
        if state.callback_open || state.consumed {
            return Err(FileRefusal::Spent);
        }
        state.consumed = true;
        match (decision, &state.answered) {
            (None, None) => Ok(HumanChoice::Absent),
            (Some(reply), Some(actual))
                if Arc::ptr_eq(&reply.0, actual)
                    && reply
                        .0
                        .request
                        .upgrade()
                        .is_some_and(|request| Arc::ptr_eq(&request, &self.0))
                    && state
                        .presented
                        .as_ref()
                        .is_some_and(|shown| Arc::ptr_eq(shown, &reply.0.presentation))
                    && reply.0.ui.upgrade().is_some_and(|ui| {
                        self.0
                            .ui
                            .upgrade()
                            .is_some_and(|expected| Arc::ptr_eq(&ui, &expected))
                    }) =>
            {
                Ok(reply.0.choice.clone())
            }
            _ => Err(FileRefusal::NotApproved),
        }
    }
}
pub(super) struct CallbackOwnership(PrivacyApprovalRequest);
impl Drop for CallbackOwnership {
    fn drop(&mut self) {
        self.0.close_callback();
    }
}

impl PrivacyApprovalUi {
    /// A uses this only when its receiver installs the exact request's real
    /// modal. It does not prove rendered/human origin without those callsites.
    pub(crate) fn present(
        &self,
        request: &PrivacyApprovalRequest,
    ) -> Result<PrivacyApprovalPresentation, FileRefusal> {
        request.live()?;
        if !request
            .0
            .ui
            .upgrade()
            .is_some_and(|ui| Arc::ptr_eq(&self.0, &ui))
        {
            return Err(FileRefusal::NotApproved);
        }
        let mut state = request.0.state.lock().unwrap();
        if !state.callback_open || state.presented.is_some() {
            return Err(FileRefusal::Spent);
        }
        let identity = Arc::new(());
        let presented = Arc::new(Presented {
            request: request.clone(),
            ui: Arc::downgrade(&self.0),
            identity: identity.clone(),
        });
        state.presented = Some(identity);
        Ok(PrivacyApprovalPresentation(presented))
    }
    /// Called by A's actual modal answer handler. The raw variant/text is
    /// retained in the private request before any parse or model projection.
    pub(crate) fn decide(
        &self,
        presentation: PrivacyApprovalPresentation,
        choice: HumanChoice,
    ) -> Result<PrivacyApprovalDecision, FileRefusal> {
        let request = &presentation.0.request;
        request.live()?;
        if !presentation
            .0
            .ui
            .upgrade()
            .is_some_and(|ui| Arc::ptr_eq(&self.0, &ui))
        {
            return Err(FileRefusal::NotApproved);
        }
        let mut state = request.0.state.lock().unwrap();
        if !state.callback_open
            || state.answered.is_some()
            || !state
                .presented
                .as_ref()
                .is_some_and(|actual| Arc::ptr_eq(actual, &presentation.0.identity))
        {
            return Err(FileRefusal::Spent);
        }
        let decision = Arc::new(Decision {
            request: Arc::downgrade(&request.0),
            presentation: presentation.0.identity.clone(),
            ui: presentation.0.ui.clone(),
            choice,
        });
        state.answered = Some(decision.clone());
        Ok(PrivacyApprovalDecision(decision))
    }
}

impl PrivacyApprovalPresentation {
    pub(crate) fn review(&self) -> PrivacyApprovalView<'_> {
        let request = &self.0.request.0;
        PrivacyApprovalView {
            call: &request.call,
            summary: &request.summary,
            details: &request.details,
            shown: request.shown,
        }
    }
}

macro_rules! opaque_debug {
    ($($name:ident),+ $(,)?) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "([private])"))
            }
        }
    )+};
}
opaque_debug!(
    PrivacyApprovalRequest,
    PrivacyApprovalPresentation,
    PrivacyApprovalDecision,
    PrivacyApprovalPrompt,
    PrivacyApprovalUi
);
