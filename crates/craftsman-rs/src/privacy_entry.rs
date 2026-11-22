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

use crate::privacy_session::{
    AdmitError, AdmitRefusal, CapturedRaw, PrivacyView, RawOrigin, SessionStop,
};
use craftsman_harness::provenance::SourceRef;
use craftsman_harness::{Agent, AgentError, HitlAction, ToolCall, TurnResult};

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

    /// Applies what a person decided about the parked call `shown`, which is
    /// the call the interface actually showed them. A reason typed with a
    /// denial is checked as a draft first.
    pub async fn decide(
        &self,
        agent: &Agent,
        shown: &ToolCall,
        action: HitlAction,
        deny_default: &str,
    ) -> Result<TurnResult, PrivateEntryError> {
        if let HitlAction::Deny { feedback } = &action
            && !feedback.is_empty()
        {
            self.typed(feedback).await?;
        }
        let decision = self
            .session()
            .human_decision(shown, &action)
            .map_err(PrivateEntryError::Stopped)?;
        agent
            .resume_with_view(action, deny_default, decision)
            .await
            .map_err(PrivateEntryError::Agent)
    }
}
