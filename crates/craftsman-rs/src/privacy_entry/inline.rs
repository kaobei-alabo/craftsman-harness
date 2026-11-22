//! Actual registered gate input and automatic-policy producers. A reviewed
//! operation supplies data, never its own approval. No public issuer exists.

use super::HumanChoice;
use super::approval_ui::{PrivacyApprovalPrompt, PrivacyApprovalRequest};
use crate::hitl::{ApprovalState, ToolPermission};
use crate::privacy_session::{
    CapturedRaw, ConversationBoundary, FileRefusal, PrivacySession, RawOrigin, ReviewedFile,
    SessionStop,
};
use craftsman_harness::provenance::{
    ActualPlannedInvocation, ActualToolInvocation, SourceRef, TrackedDispatch,
};
use craftsman_harness::{HitlAction, ToolCall};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

#[cfg(test)]
type InlineReader =
    Arc<dyn Fn(&ToolCall, &str, &str, Option<[u8; 32]>) -> HumanChoice + Send + Sync>;

pub(crate) struct InlineDecisionProducer {
    owner: Weak<PrivacySession>,
    conversation: Weak<ConversationBoundary>,
    approval: Arc<Mutex<ApprovalState>>,
    workspace: PathBuf,
    privacy_prompt: Option<Arc<PrivacyApprovalPrompt>>,
    verification: Option<Arc<crate::verification::VerificationController>>,
    plans: Mutex<Vec<RegisteredPlan>>,
    received: Mutex<Vec<Arc<InlineDecisionInput>>>,
    requests: Mutex<Vec<PrivacyApprovalRequest>>,
}
struct RegisteredPlan {
    planned: ActualPlannedInvocation,
    review_source: Option<SourceRef>,
    shown: Option<[u8; 32]>,
}

enum Observation {
    Policy(ToolPermission),
    Human(HumanChoice),
}

pub(crate) struct InlineDecisionInput {
    owner: Weak<PrivacySession>,
    issuer: Weak<InlineDecisionProducer>,
    lane: Arc<ConversationBoundary>,
    parent: Option<ActualToolInvocation>,
    planned: ActualPlannedInvocation,
    review_source: Option<SourceRef>,
    shown: Option<[u8; 32]>,
    call: ToolCall,
    shown_call: ToolCall,
    verification: Option<(
        Arc<crate::verification::VerificationController>,
        Arc<crate::verification::VerificationReview>,
    )>,
    observation: Observation,
    action: Mutex<Option<HitlAction>>,
    admitted: AtomicBool,
    terminal: Mutex<Option<ActualToolInvocation>>,
    used: AtomicBool,
    effect_applied: AtomicBool,
}

pub(crate) struct InlineResolution {
    pub(crate) input: Arc<InlineDecisionInput>,
    pub(crate) permission: ToolPermission,
    pub(crate) call: ToolCall,
}

impl InlineDecisionProducer {
    /// The already accepted real parked decision is not a second inline read.
    /// The app boundary has admitted its exact raw answer and issued the
    /// current operation; this gate still rechecks its own current permission.
    pub(crate) fn resumed_permission(
        &self,
        tracked: &TrackedDispatch,
    ) -> Result<ToolPermission, craftsman_harness::AgentError> {
        let refused = || craftsman_harness::AgentError::Tool {
            name: "privacy_approval".into(),
            message: FileRefusal::NotApproved.to_string(),
        };
        let invocation = tracked
            .invocation()
            .filter(|invocation| invocation.resumed())
            .ok_or_else(refused)?;
        let conversation = tracked
            .boundary()
            .and_then(|boundary| boundary.conversation())
            .ok_or_else(refused)?;
        let lane = self
            .conversation
            .upgrade()
            .and_then(|root| root.member(&conversation))
            .ok_or_else(refused)?;
        lane.require_agent(invocation.owner())?;
        let operation = lane
            .operation_in_flight()
            .filter(|operation| operation.is_for(invocation))
            .ok_or_else(refused)?;
        let input = invocation
            .decision_source()
            .and_then(|source| source.downcast::<super::HumanDecisionInput>())
            .ok_or_else(refused)?;
        if !input.planned().same_plan(invocation.planned())
            || input.action() != invocation.decision_action()
            || !matches!(
                input.action(),
                Some(HitlAction::Approve | HitlAction::ApproveEdited { .. })
            )
            || operation.call().map_err(|_| refused())? != invocation.call()
        {
            return Err(refused());
        }
        let mut shown = invocation.call().clone();
        if !invocation.human_edited() {
            if let Some(reviewed) = ReviewedFile::of(tracked) {
                shown.args = reviewed.shown_arguments().map_err(|_| refused())?;
            }
        }
        Ok(match self.permission_for(&shown, None) {
            ToolPermission::Deny(reason) => ToolPermission::Deny(reason),
            _ => ToolPermission::Allow,
        })
    }

    /// Used only at the actual shared-builder/gate registration seam. Its
    /// original state and callback are retained as the independent issuers.
    pub(crate) fn register(
        conversation: &Arc<ConversationBoundary>,
        approval: Arc<Mutex<ApprovalState>>,
        workspace: PathBuf,
        privacy_prompt: Option<Arc<PrivacyApprovalPrompt>>,
        verification: Option<Arc<crate::verification::VerificationController>>,
    ) -> Arc<Self> {
        Self::registered(
            conversation,
            approval,
            workspace,
            privacy_prompt,
            verification,
        )
    }

    #[cfg(test)]
    pub(crate) fn register_reader(
        conversation: &Arc<ConversationBoundary>,
        approval: Arc<Mutex<ApprovalState>>,
        workspace: PathBuf,
        read: InlineReader,
    ) -> Arc<Self> {
        // A synthetic owned UI receiver for ordinary source/file tests only.
        // The production constructor accepts solely the bound UI transport.
        let owner = Arc::new(Mutex::new(
            None::<Arc<super::approval_ui::PrivacyApprovalUi>>,
        ));
        let receiver = owner.clone();
        let (prompt, ui) =
            super::approval_ui::register_privacy_approval_ui(Arc::new(move |request| {
                let ui = receiver.lock().unwrap().clone()?;
                let shown = ui.present(&request).ok()?;
                let review = shown.review();
                let choice = read(review.call, review.summary, review.details, review.shown);
                ui.decide(shown, choice).ok()
            }));
        *owner.lock().unwrap() = Some(ui);
        Self::registered(conversation, approval, workspace, Some(prompt), None)
    }

    fn registered(
        conversation: &Arc<ConversationBoundary>,
        approval: Arc<Mutex<ApprovalState>>,
        workspace: PathBuf,
        privacy_prompt: Option<Arc<PrivacyApprovalPrompt>>,
        verification: Option<Arc<crate::verification::VerificationController>>,
    ) -> Arc<Self> {
        let owner = conversation.session_owner();
        let producer = Arc::new(Self {
            owner: Arc::downgrade(&owner),
            conversation: Arc::downgrade(conversation),
            approval,
            workspace,
            privacy_prompt,
            verification,
            plans: Mutex::new(Vec::new()),
            received: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
        });
        owner.register_inline_issuer(&producer);
        producer
    }

    /// Presentation id is a UI locator only. Plan, source, arguments and
    /// digest are obtained here from the actual dispatch and private owner.
    pub(crate) async fn receive_inline(
        self: &Arc<Self>,
        tracked: &TrackedDispatch,
        presentation_id: Option<&str>,
    ) -> Result<InlineResolution, craftsman_harness::AgentError> {
        let planned = tracked
            .planned()
            .ok_or_else(|| craftsman_harness::AgentError::Tool {
                name: "privacy_approval".into(),
                message: FileRefusal::NotApproved.to_string(),
            })?;
        planned.check_live()?;
        let result = self.receive_inline_inner(tracked, presentation_id).await;
        match result {
            Ok(result) => Ok(result),
            Err(_)
                if matches!(
                    planned.check_live(),
                    Err(craftsman_harness::AgentError::Cancelled)
                ) =>
            {
                Err(craftsman_harness::AgentError::Cancelled)
            }
            Err(refusal) => Err(craftsman_harness::AgentError::Tool {
                name: "privacy_approval".into(),
                message: refusal.to_string(),
            }),
        }
    }

    async fn receive_inline_inner(
        self: &Arc<Self>,
        tracked: &TrackedDispatch,
        presentation_id: Option<&str>,
    ) -> Result<InlineResolution, FileRefusal> {
        let session = self
            .owner
            .upgrade()
            .ok_or(FileRefusal::Stopped(SessionStop::Closed))?;
        let plan = tracked.planned().ok_or(FileRefusal::NotApproved)?;
        let conversation = tracked
            .boundary()
            .and_then(|boundary| boundary.conversation())
            .ok_or(FileRefusal::NotApproved)?;
        let lane = self
            .conversation
            .upgrade()
            .and_then(|registered| registered.member(&conversation))
            .ok_or(FileRefusal::NotApproved)?;
        if !Arc::ptr_eq(&session, &lane.session_owner()) {
            return Err(FileRefusal::NotApproved);
        }
        lane.require_agent(plan.owner())
            .map_err(|_| FileRefusal::Stopped(SessionStop::Unbound))?;
        if !session.owns_inline_issuer(self) || !plan.in_review() {
            return Err(FileRefusal::NotApproved);
        }
        let reviewed = ReviewedFile::of(tracked);
        let (shown_call, shown) = match &reviewed {
            Some(reviewed) => reviewed.producer_preview(&session, tracked)?,
            None if tracked.review_source().is_none() && tracked.shown().is_none() => {
                (plan.call().clone(), None)
            }
            None => return Err(FileRefusal::NotApproved),
        };
        {
            let mut plans = self.plans.lock().unwrap();
            if plans.iter().any(|known| known.planned.same_plan(plan)) {
                return Err(FileRefusal::Spent);
            }
            plans.push(RegisteredPlan {
                planned: plan.clone(),
                review_source: tracked.review_source().cloned(),
                shown,
            });
        }
        // The actual automatic decision is captured from the registered
        // original state, under the same synchronous lock as ordinary gates.
        let verification = match self
            .verification
            .as_ref()
            .filter(|_| plan.call().name == "verify")
        {
            Some(controller) => Some((
                controller.clone(),
                Arc::new(
                    controller
                        .approval_context()
                        .await
                        .map_err(|_| FileRefusal::NotApproved)?,
                ),
            )),
            None => None,
        };
        let permission = self.permission_for(&shown_call, verification.as_ref());
        let policy = self.capture(
            tracked,
            lane.clone(),
            &shown_call,
            shown,
            Observation::Policy(permission.clone()),
            verification.clone(),
        );
        let input = if permission == ToolPermission::Prompt {
            let mut presented = shown_call.clone();
            if let Some(id) = presentation_id {
                presented.id = id.to_owned();
            }
            let summary = verification.as_ref().map_or_else(
                || crate::tools::summarize_call(&presented.name, &presented.args),
                |(_, review)| review.summary.clone(),
            );
            let details = verification.as_ref().map_or_else(
                || {
                    crate::tools::approval_details_at(
                        &presented.name,
                        &presented.args,
                        &self.workspace,
                    )
                },
                |(_, review)| review.details.clone(),
            );
            // Only an actual registered UI request/response can be consumed.
            // Ordinary summary callbacks cannot echo a tuple into authority.
            let prompt = self.privacy_prompt.as_ref().ok_or(FileRefusal::Excluded(
                "call requires user approval from a registered private UI",
            ))?;
            let request = prompt.issue(
                self,
                lane.clone(),
                plan.clone(),
                tracked.review_source().cloned(),
                presented,
                summary,
                details,
                shown,
            )?;
            self.requests.lock().unwrap().push(request.clone());
            let _callback = request.own_callback();
            let reply = prompt.read(request.clone());
            request.close_callback();
            let choice = match request.consume(reply) {
                Ok(choice) => choice,
                Err(refusal) => {
                    if !matches!(
                        plan.check_live(),
                        Err(craftsman_harness::AgentError::Cancelled)
                    ) {
                        session.stop();
                    }
                    return Err(refusal);
                }
            };
            let received = self.capture(
                tracked,
                lane.clone(),
                &shown_call,
                shown,
                Observation::Human(choice),
                verification.clone(),
            );
            if let Some(reviewed) = &reviewed {
                reviewed.producer_presented()?;
            }
            received
        } else {
            policy
        };
        if let Observation::Human(HumanChoice::Edit(raw) | HumanChoice::Deny(raw)) =
            &input.observation
        {
            // The actual raw answer is owned and admitted before parsing or
            // projection. Issued aliases cannot become human/model authority.
            let capture = CapturedRaw::from_producer(RawOrigin::HumanReply, raw);
            if session.admit_raw(&capture).await.is_err() {
                session.stop();
                return Err(FileRefusal::NotApproved);
            }
        }
        let action = match &input.observation {
            Observation::Policy(ToolPermission::Allow) => Some(HitlAction::Approve),
            Observation::Policy(_) | Observation::Human(HumanChoice::Absent) => None,
            Observation::Human(HumanChoice::Approve | HumanChoice::ApproveAlways) => {
                Some(HitlAction::Approve)
            }
            Observation::Human(HumanChoice::Deny(text)) => Some(HitlAction::Deny {
                feedback: text.clone(),
            }),
            Observation::Human(HumanChoice::Edit(raw)) => {
                if raw.len() > crate::privacy_hook::MAX_TEXT_BYTES {
                    session.stop();
                    return Err(FileRefusal::NotApproved);
                }
                match serde_json::from_str(raw) {
                    Ok(args) => Some(HitlAction::ApproveEdited { args }),
                    Err(_) => {
                        session.stop();
                        return Err(FileRefusal::NotApproved);
                    }
                }
            }
        };
        *input.action.lock().unwrap() = action.clone();
        // A gate whose real permission became Deny while input/admission
        // returned cannot execute; completed input/map facts remain retained.
        lane.require_agent(plan.owner())
            .map_err(|_| FileRefusal::Stopped(SessionStop::Unbound))?;
        if !plan.in_review() {
            return Err(FileRefusal::NotApproved);
        }
        let mut call = plan.call().clone();
        if let Some(HitlAction::ApproveEdited { args }) = &action {
            call.args = args.clone();
        }
        let permission_call = if input.edited() { &call } else { &shown_call };
        let current = self.permission_for(permission_call, verification.as_ref());
        let permission = match (&input.observation, &action) {
            (Observation::Policy(ToolPermission::Allow), _) if current != ToolPermission::Allow => {
                ToolPermission::Deny(
                    "This call's automatic permission is no longer available.".into(),
                )
            }
            (Observation::Policy(permission), _) => permission.clone(),
            (
                Observation::Human(_),
                Some(HitlAction::Approve | HitlAction::ApproveEdited { .. }),
            ) => match current {
                ToolPermission::Deny(reason) => ToolPermission::Deny(reason),
                _ => ToolPermission::Allow,
            },
            (Observation::Human(_), Some(HitlAction::Deny { feedback })) => {
                ToolPermission::Deny(feedback.clone())
            }
            _ if input.was_absent() => ToolPermission::Deny(
                "This delegated or main call has no actual answered approval.".into(),
            ),
            _ => ToolPermission::Deny("This call has no admitted actual decision.".into()),
        };
        input.admitted.store(true, Ordering::Release);
        if permission == ToolPermission::Allow {
            if tracked
                .note_decision_action(SourceRef::new(input.clone()), action.clone())
                .is_err()
            {
                return Err(FileRefusal::NotApproved);
            }
        } else if let Some(reviewed) = reviewed {
            reviewed.over();
        }
        Ok(InlineResolution {
            input,
            permission,
            call,
        })
    }

    fn permission_for(
        &self,
        call: &ToolCall,
        verification: Option<&(
            Arc<crate::verification::VerificationController>,
            Arc<crate::verification::VerificationReview>,
        )>,
    ) -> ToolPermission {
        if let Some((controller, review)) = verification {
            let (plan_mode, allowed) = {
                let state = self.approval.lock().unwrap();
                (
                    state.mode == crate::config::PermissionMode::Plan,
                    state.auto_approve
                        || state.mode == crate::config::PermissionMode::BypassPermissions,
                )
            };
            if plan_mode {
                ToolPermission::Deny("Permission mode 'plan' does not run verification commands. Switch modes to permit these checks.".into())
            } else if controller.needs_project_approval()
                || controller.needs_protected_approval(review)
                || !allowed
            {
                ToolPermission::Prompt
            } else {
                ToolPermission::Allow
            }
        } else {
            self.approval
                .lock()
                .unwrap()
                .permission(&call.name, &call.args)
        }
    }

    pub(super) fn owns_request(
        self: &Arc<Self>,
        request: &PrivacyApprovalRequest,
    ) -> Result<(), FileRefusal> {
        let session = self.owner.upgrade().ok_or(FileRefusal::NotApproved)?;
        session.check().map_err(FileRefusal::Stopped)?;
        let (planned, source, shown) = request.issued_review();
        let registered = self.plans.lock().unwrap().iter().any(|known| {
            known.planned.same_plan(planned)
                && known.shown == shown
                && match (&known.review_source, source) {
                    (None, None) => true,
                    (Some(actual), Some(source)) => actual.is(source),
                    _ => false,
                }
        });
        if !session.owns_inline_issuer(self)
            || !request.is_for(self)
            || !registered
            || !self
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|known| known.same_request(request))
        {
            return Err(FileRefusal::NotApproved);
        }
        Ok(())
    }

    fn capture(
        self: &Arc<Self>,
        tracked: &TrackedDispatch,
        lane: Arc<ConversationBoundary>,
        shown_call: &ToolCall,
        shown: Option<[u8; 32]>,
        observation: Observation,
        verification: Option<(
            Arc<crate::verification::VerificationController>,
            Arc<crate::verification::VerificationReview>,
        )>,
    ) -> Arc<InlineDecisionInput> {
        let input = Arc::new(InlineDecisionInput {
            owner: self.owner.clone(),
            issuer: Arc::downgrade(self),
            lane,
            parent: tracked.invocation().cloned(),
            planned: tracked.planned().expect("validated real plan").clone(),
            review_source: tracked.review_source().cloned(),
            shown,
            call: tracked.planned().unwrap().call().clone(),
            shown_call: shown_call.clone(),
            observation,
            verification,
            action: Mutex::new(None),
            admitted: AtomicBool::new(false),
            terminal: Mutex::new(None),
            used: AtomicBool::new(false),
            effect_applied: AtomicBool::new(false),
        });
        self.received.lock().unwrap().push(input.clone());
        input
    }
}

impl InlineDecisionInput {
    #[cfg(test)]
    pub(crate) fn raw_edit(&self) -> Option<&str> {
        match &self.observation {
            Observation::Human(HumanChoice::Edit(raw)) => Some(raw),
            _ => None,
        }
    }
    pub(crate) fn planned(&self) -> &ActualPlannedInvocation {
        &self.planned
    }
    pub(crate) fn registered_session(&self) -> Option<Arc<PrivacySession>> {
        let session = self.owner.upgrade()?;
        let issuer = self.issuer.upgrade()?;
        let conversation = issuer.conversation.upgrade()?;
        if !session.owns_inline_issuer(&issuer)
            || !Arc::ptr_eq(&conversation.session_owner(), &session)
            || !issuer
                .owner
                .upgrade()
                .is_some_and(|owner| Arc::ptr_eq(&owner, &session))
            || !issuer
                .received
                .lock()
                .unwrap()
                .iter()
                .any(|input| std::ptr::eq(Arc::as_ptr(input), self))
        {
            return None;
        }
        Some(session)
    }
    pub(crate) fn action(&self) -> Option<HitlAction> {
        self.action.lock().unwrap().clone()
    }
    pub(crate) fn edited(&self) -> bool {
        matches!(self.observation, Observation::Human(HumanChoice::Edit(_)))
    }
    pub(crate) fn was_absent(&self) -> bool {
        matches!(self.observation, Observation::Human(HumanChoice::Absent))
    }
    pub(crate) fn validate_terminal(
        &self,
        session: &Arc<PrivacySession>,
        invocation: &ActualToolInvocation,
    ) -> Result<(), FileRefusal> {
        let owner = self.registered_session().ok_or(FileRefusal::NotApproved)?;
        if !Arc::ptr_eq(&owner, session)
            || !Arc::ptr_eq(&owner, &self.lane.session_owner())
            || !self.admitted.load(Ordering::Acquire)
            || !self.planned.in_review()
            || !invocation.was_planned_as(&self.planned)
            || match (&self.parent, invocation.parent()) {
                (None, None) => false,
                (Some(actual), Some(parent)) => !actual.same_invocation(parent),
                _ => true,
            }
            || !invocation
                .decision_source()
                .and_then(|source| source.downcast::<Self>())
                .is_some_and(|source| std::ptr::eq(Arc::as_ptr(&source), self))
            || invocation.shown() != self.shown
            || match (&self.review_source, invocation.review_source()) {
                (None, None) => false,
                (Some(a), Some(b)) => !a.is(b),
                _ => true,
            }
        {
            return Err(FileRefusal::NotApproved);
        }
        self.lane
            .require_agent(invocation.owner())
            .map_err(|_| FileRefusal::Stopped(SessionStop::Unbound))?;
        let mut expected = self.call.clone();
        if let Some(HitlAction::ApproveEdited { args }) = self.action() {
            expected.args = args;
        }
        if invocation.call() != &expected {
            return Err(FileRefusal::NotApproved);
        }
        if invocation.decision_action() != self.action().as_ref() {
            return Err(FileRefusal::NotApproved);
        }
        if !matches!(
            self.action(),
            Some(HitlAction::Approve | HitlAction::ApproveEdited { .. })
        ) {
            return Err(FileRefusal::NotApproved);
        }
        let issuer = self.issuer.upgrade().ok_or(FileRefusal::NotApproved)?;
        let permission_call = if self.edited() {
            &expected
        } else {
            &self.shown_call
        };
        let current = issuer.permission_for(permission_call, self.verification.as_ref());
        if matches!(current, ToolPermission::Deny(_))
            || matches!(self.observation, Observation::Policy(ToolPermission::Allow))
                && current != ToolPermission::Allow
        {
            return Err(FileRefusal::NotApproved);
        }
        let mut terminal = self.terminal.lock().unwrap();
        match &*terminal {
            Some(prior) if !prior.same_invocation(invocation) => return Err(FileRefusal::Spent),
            Some(_) => {}
            None => *terminal = Some(invocation.clone()),
        }
        drop(terminal);
        self.apply_accepted_effect();
        Ok(())
    }
    pub(crate) fn consume_file(
        &self,
        session: &Arc<PrivacySession>,
        invocation: &ActualToolInvocation,
        shown: [u8; 32],
    ) -> Result<(), FileRefusal> {
        self.validate_terminal(session, invocation)?;
        if self.shown != Some(shown) || self.edited() {
            return Err(FileRefusal::NotApproved);
        }
        if self.used.swap(true, Ordering::AcqRel) {
            return Err(FileRefusal::Spent);
        }
        Ok(())
    }
    fn apply_accepted_effect(&self) {
        if !matches!(
            self.observation,
            Observation::Human(HumanChoice::Approve | HumanChoice::ApproveAlways)
        ) || self.effect_applied.swap(true, Ordering::AcqRel)
        {
            return;
        }
        if let Some((controller, review)) = &self.verification {
            controller.approve_protected(review);
        }
        if matches!(
            self.observation,
            Observation::Human(HumanChoice::ApproveAlways)
        ) {
            if let Some(issuer) = self.issuer.upgrade() {
                let mut state = issuer.approval.lock().unwrap();
                if crate::tools::NETWORK_TOOLS.contains(&self.call.name.as_str()) {
                    state.auto_approve_network = true;
                } else {
                    state.auto_approve = true;
                }
            }
        }
    }
}

impl std::fmt::Debug for InlineDecisionInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InlineDecisionInput([registered actual input/policy])")
    }
}
