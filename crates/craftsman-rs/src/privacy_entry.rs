//! Typed entries of a session that was built for a privacy conversation.
//! Nothing calls them: no frontend builds such a session.
//!
//! Each entry is the producer of one raw capture. What a person typed is
//! captured here as a draft and admitted before it is accepted, so a draft
//! that cannot be admitted is turned away unchanged and the conversation
//! continues. Only then is the text handed to the agent, exactly as typed,
//! with the admitted node as its source.
//!
//! `agent` is always the agent of the session that holds this view.

use crate::privacy_session::PrivacySession;
use crate::privacy_session::{
    AdmitError, AdmitRefusal, CapturedRaw, PrivacyView, RawOrigin, SessionStop,
};
use craftsman_harness::provenance::{ActualParkedCall, SourceRef};
use craftsman_harness::{Agent, AgentError, HitlAction, TurnResult};
use std::sync::{Arc, Mutex, Weak};

mod approval_ui;
mod inline;
pub(crate) use approval_ui::PrivacyApprovalPrompt;
// The receiver's side: the terminal's modal on unix, and cases everywhere.
// The request and the decision are named through this module only by that
// receiver; the gate's own producer takes them from `approval_ui` itself.
#[cfg(any(unix, test))]
pub(crate) use approval_ui::{
    PrivacyApprovalDecision, PrivacyApprovalPresentation, PrivacyApprovalRequest,
    PrivacyApprovalUi, PrivacyApprovalView, register_privacy_approval_ui,
};
pub(crate) use inline::{InlineDecisionInput, InlineDecisionProducer};

/// Why a typed entry did not run.
#[derive(Debug)]
pub enum PrivateEntryError {
    /// The draft was turned away before it was accepted. Nothing was
    /// recorded or sent, and the conversation continues.
    Refused(AdmitRefusal),
    /// The privacy conversation has ended.
    Stopped(SessionStop),
    /// The agent's own result for an accepted input.
    Agent(AgentError),
}

impl std::fmt::Display for PrivateEntryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => write!(f, "This input was not sent. {refusal}"),
            Self::Stopped(stop) => write!(f, "Privacy stopped this conversation. {stop}"),
            Self::Agent(error) => write!(f, "{error}"),
        }
    }
}

impl From<AdmitError> for PrivateEntryError {
    fn from(error: AdmitError) -> Self {
        match error {
            AdmitError::Refused(refusal) => Self::Refused(refusal),
            AdmitError::Stopped(stop) => Self::Stopped(stop),
        }
    }
}

impl PrivacyView {
    /// Captures what a person typed as a draft and admits it.
    async fn typed(&self, typed: &str) -> Result<SourceRef, PrivateEntryError> {
        let raw = CapturedRaw::from_producer(RawOrigin::TerminalDraft, typed);
        let node = self.session().admit_raw(&raw).await?;
        self.session()
            .source(&node)
            .map_err(PrivateEntryError::Stopped)
    }

    /// One turn with what a person typed.
    pub async fn turn(&self, agent: &Agent, typed: &str) -> Result<TurnResult, PrivateEntryError> {
        let source = self.typed(typed).await?;
        agent
            .run_turn_with_view(typed, source)
            .await
            .map_err(PrivateEntryError::Agent)
    }

    /// Adds what a person typed to the running turn. `false` means no turn
    /// took it; the draft is then still the caller's.
    pub async fn steer(&self, agent: &Agent, typed: &str) -> Result<bool, PrivateEntryError> {
        let source = self.typed(typed).await?;
        Ok(agent.steer_with_view(typed, source))
    }

    /// Manual compaction through this session's tracked summarizer path.
    pub async fn compact(
        &self,
        agent: &Agent,
        compaction: &craftsman_harness::builtin::CompactionMiddleware,
    ) -> Result<Option<(usize, usize)>, PrivateEntryError> {
        self.session().check().map_err(PrivateEntryError::Stopped)?;
        agent
            .compact_idle_with_view(compaction)
            .await
            .map_err(PrivateEntryError::Agent)
    }

    /// Requests input through the source registered by the actual builder.
    /// No caller Msg/action/label can create a human decision receipt.
    pub async fn request_decision(
        &self,
        agent: &Agent,
        deny_default: &str,
    ) -> Result<Option<TurnResult>, PrivateEntryError> {
        let source = self
            .human_source
            .as_ref()
            .ok_or(PrivateEntryError::Stopped(SessionStop::Unbound))?;
        let parked = agent
            .actual_parked_call()
            .ok_or(PrivateEntryError::Stopped(SessionStop::Unbound))?;
        self.boundary()
            .require_agent(parked.owner())
            .map_err(PrivateEntryError::Agent)?;
        let input = source.receive(&parked)?;
        let Some(action) = input.action() else {
            return Ok(None);
        };
        let receipt = SourceRef::new(input.clone());
        agent
            .resume_with_view(action.clone(), deny_default, receipt)
            .await
            .map(Some)
            .map_err(PrivateEntryError::Agent)
    }
}

/// Actual values returned by the registered input owner. Edit carries the raw
/// JSON source before parsing, never a serialization guessed from a Value.
#[derive(Clone)]
pub enum HumanChoice {
    Absent,
    Approve,
    ApproveAlways,
    Deny(String),
    Edit(String),
}

type HumanReader = Arc<dyn Fn(&ActualParkedCall, Option<[u8; 32]>) -> HumanChoice + Send + Sync>;
type AcceptedHumanEffect = Arc<dyn Fn(&HumanChoice, &craftsman_harness::ToolCall) + Send + Sync>;

/// Private registered input owner. Registration alone mints no receipt: only
/// the actual read below, against a real parked-call witness, creates a fact.
pub(crate) struct HumanDecisionProducer {
    session: Arc<PrivacySession>,
    read: HumanReader,
    accepted_effect: Option<AcceptedHumanEffect>,
    received: Mutex<Vec<Arc<HumanDecisionInput>>>,
}

pub struct HumanDecisionInput {
    owner: Weak<PrivacySession>,
    parked: ActualParkedCall,
    choice: HumanChoice,
    action: Option<HitlAction>,
    edited_raw: Option<CapturedRaw>,
    shown: Option<[u8; 32]>,
    accepted_effect: Option<AcceptedHumanEffect>,
    effect_applied: std::sync::atomic::AtomicBool,
}

impl HumanDecisionInput {
    pub fn action(&self) -> Option<&HitlAction> {
        self.action.as_ref()
    }
    pub fn was_absent(&self) -> bool {
        matches!(self.choice, HumanChoice::Absent)
    }
    /// The source-bound operation digest carried by the actual parked event
    /// and delivered to the registered input callback. No caller digest is
    /// accepted by this producer.
    pub fn shown(&self) -> Option<[u8; 32]> {
        self.shown
    }
    pub(crate) fn binds(
        &self,
        session: &Arc<PrivacySession>,
        parked: &ActualParkedCall,
        action: &HitlAction,
    ) -> bool {
        self.owner
            .upgrade()
            .is_some_and(|owner| Arc::ptr_eq(&owner, session))
            && self.parked.same_park(parked)
            && self.shown == parked.shown()
            && self.action.as_ref() == Some(action)
    }
    /// The agent's own event for the proposal this input decided, as the
    /// parked call carried it. For a consumer that has to know which plan
    /// a decision is about; it grants nothing.
    pub(crate) fn planned(&self) -> &craftsman_harness::provenance::ActualPlannedInvocation {
        self.parked.planned()
    }
    pub(crate) fn edited_raw(&self) -> Option<&CapturedRaw> {
        self.edited_raw.as_ref()
    }
    /// Called only after this exact parked decision and its raw edit/feedback
    /// have passed the boundary. The input read itself grants no permission.
    pub(crate) fn apply_accepted_effect(&self) {
        if !self
            .effect_applied
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            if let Some(effect) = &self.accepted_effect {
                effect(&self.choice, self.parked.call());
            }
        }
    }
}

impl HumanDecisionProducer {
    /// Called only at the actual app input registration seam. The callback is
    /// kept as that owner; source text and project/model flags cannot make it.
    #[cfg(test)]
    pub(crate) fn register(session: Arc<PrivacySession>, read: HumanReader) -> Arc<Self> {
        Self::registered(session, read, None)
    }

    #[cfg(test)]
    pub(crate) fn register_with_effect(
        session: Arc<PrivacySession>,
        read: HumanReader,
        effect: AcceptedHumanEffect,
    ) -> Arc<Self> {
        Self::registered(session, read, Some(effect))
    }

    #[cfg(test)]
    fn registered(
        session: Arc<PrivacySession>,
        read: HumanReader,
        accepted_effect: Option<AcceptedHumanEffect>,
    ) -> Arc<Self> {
        Arc::new(Self {
            session,
            read,
            accepted_effect,
            received: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn receive(
        &self,
        parked: &ActualParkedCall,
    ) -> Result<Arc<HumanDecisionInput>, PrivateEntryError> {
        self.session
            .check_agent(parked.owner())
            .map_err(PrivateEntryError::Stopped)?;
        let shown = parked.shown();
        let choice = (self.read)(parked, shown);
        // The original source is retained before any parse/projection failure.
        let raw = match &choice {
            HumanChoice::Edit(text) if text.len() <= crate::privacy_hook::MAX_TEXT_BYTES => {
                Some(CapturedRaw::from_producer(RawOrigin::HumanReply, text))
            }
            _ => None,
        };
        let action = match &choice {
            HumanChoice::Absent => None,
            HumanChoice::Approve | HumanChoice::ApproveAlways => Some(HitlAction::Approve),
            HumanChoice::Deny(feedback) => Some(HitlAction::Deny {
                feedback: feedback.clone(),
            }),
            HumanChoice::Edit(text) if text.len() <= crate::privacy_hook::MAX_TEXT_BYTES => {
                serde_json::from_str(text)
                    .ok()
                    .map(|args| HitlAction::ApproveEdited { args })
            }
            HumanChoice::Edit(_) => None,
        };
        let input = Arc::new(HumanDecisionInput {
            owner: Arc::downgrade(&self.session),
            parked: parked.clone(),
            choice,
            action,
            edited_raw: raw,
            shown,
            accepted_effect: self.accepted_effect.clone(),
            effect_applied: std::sync::atomic::AtomicBool::new(false),
        });
        self.received.lock().unwrap().push(input.clone());
        if matches!(input.choice, HumanChoice::Edit(_)) && input.action.is_none() {
            self.session.stop();
            return Err(PrivateEntryError::Stopped(SessionStop::Unbound));
        }
        Ok(input)
    }

    #[cfg(test)]
    pub(crate) fn received(&self) -> Vec<Arc<HumanDecisionInput>> {
        self.received.lock().unwrap().clone()
    }
}

impl std::fmt::Debug for HumanDecisionInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HumanDecisionInput([actual input])")
    }
}
