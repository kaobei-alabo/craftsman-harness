//! Opaque private gate -> actual UI receiver -> actual answer linkage.
//! UI ownership and these data types alone prove no human event. The sole
//! production presentation/answer callsites must be the real TUI modal/choice
//! handlers, independently reviewed with the app bootstrap that registers them.

use super::{HumanChoice, InlineDecisionProducer};
use crate::privacy_session::{ConversationBoundary, FileRefusal, ReviewedFile, SessionStop};
use craftsman_harness::ToolCall;
use craftsman_harness::provenance::ActualPlannedInvocation;
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
    /// The reviewed operation this request is about, by identity only.
    review: Option<Weak<ReviewedFile>>,
    shown: Option<[u8; 32]>,
    state: Mutex<RequestState>,
}
/// What a person is shown: the call, which for a reviewed file holds the real
/// content, and the texts made from it. The request holds it from its issue
/// until its answer is consumed, and a presentation while it is read.
struct Preview {
    call: ToolCall,
    summary: String,
    details: String,
}
struct RequestState {
    callback_open: bool,
    preview: Option<Arc<Preview>>,
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
    preview: Arc<Preview>,
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
        review: Option<Weak<ReviewedFile>>,
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
            review,
            shown,
            state: Mutex::new(RequestState {
                callback_open: true,
                preview: Some(Arc::new(Preview {
                    call,
                    summary,
                    details,
                })),
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
        Option<&Weak<ReviewedFile>>,
        Option<[u8; 32]>,
    ) {
        (&self.0.planned, self.0.review.as_ref(), self.0.shown)
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
    /// This request for the gate that issued it: an identity that keeps
    /// nothing of the request alive.
    pub(super) fn issued(&self) -> IssuedRequest {
        IssuedRequest(Arc::downgrade(&self.0))
    }
    pub(super) fn own_callback(&self) -> CallbackOwnership {
        CallbackOwnership(self.clone())
    }
    pub(super) fn close_callback(&self) {
        self.0.state.lock().unwrap().callback_open = false;
    }
    /// The answer to this request, once. Whatever it is, this is the last
    /// reader of what was shown, so the request lets go of it here. A
    /// receiver that keeps the request or its decision keeps no preview.
    pub(super) fn consume(
        &self,
        decision: Option<PrivacyApprovalDecision>,
    ) -> Result<HumanChoice, FileRefusal> {
        let answer = self.answer(decision);
        self.0.state.lock().unwrap().preview.take();
        answer
    }
    fn answer(
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
/// A request a gate issued, as that gate remembers it.
pub(super) struct IssuedRequest(Weak<Request>);
impl IssuedRequest {
    pub(super) fn is(&self, request: &PrivacyApprovalRequest) -> bool {
        std::ptr::eq(self.0.as_ptr(), Arc::as_ptr(&request.0))
    }
    /// Nothing refers to the request any more.
    pub(super) fn gone(&self) -> bool {
        self.0.strong_count() == 0
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
        let preview = state.preview.clone().ok_or(FileRefusal::Spent)?;
        let identity = Arc::new(());
        let presented = Arc::new(Presented {
            request: request.clone(),
            ui: Arc::downgrade(&self.0),
            identity: identity.clone(),
            preview,
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
        let preview = &self.0.preview;
        PrivacyApprovalView {
            call: &preview.call,
            summary: &preview.summary,
            details: &preview.details,
            shown: self.0.request.0.shown,
        }
    }
}

#[cfg(test)]
impl PrivacyApprovalRequest {
    /// For tests of what a request keeps: whether anything still holds what
    /// this request showed. The handle itself holds nothing.
    pub(crate) fn shown_handle(&self) -> Option<Weak<dyn std::any::Any + Send + Sync>> {
        let preview: Arc<dyn std::any::Any + Send + Sync> =
            self.0.state.lock().unwrap().preview.clone()?;
        Some(Arc::downgrade(&preview))
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
