//! The controllers of a privacy conversation's agent.
//!
//! The session builder configures the same controllers as for any session:
//! the context and failure controller, and the verification controller. For
//! a privacy conversation it installs each on the agent behind an adapter
//! from this module. The adapter keeps the original controller and calls it;
//! it never stands in for one.
//!
//! An adapter is made for one boundary of one conversation, before that
//! boundary's agent is built, and that boundary accepts the agent only if
//! the very adapters it holds are the controllers the agent's builder
//! installed. What an adapter states about itself, its type or its source
//! alone is never enough.
//!
//! The accepted-input callback reaches the original controller only for the
//! agent's own event of that input: issued for this installed adapter, in its
//! own callback, once, for text this conversation admitted for exactly the
//! recorded message. The original then sees that one message and its own
//! bookkeeping keys, and nothing else of the recorded state; only those keys
//! are written back. The callback after an idle compaction is reached the
//! same way, for the agent's own event of that replacement, and sees only
//! those keys.

use super::view::ConversationBoundary;
use crate::agent_controls::AppControls;
use crate::context_compiler::InputStamp;
use crate::verification::VerificationController;
use craftsman_harness::provenance::{
    ActualBoundaryInput, ActualControllerBindings, ActualIdleCompaction, SourceRef, ViewSupport,
};
use craftsman_harness::{
    AgentBoundaryPolicy, AgentError, AgentState, CompletionAction, CompletionAfterCheck,
    CompletionCheck, HumanInputKind, ModelBoundaryAction, Msg,
};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// The most events of the agent an adapter remembers having taken. An event
/// is usable only during its own callback, so the latest ones are enough to
/// refuse a second use.
const TAKEN: usize = 32;

fn refused() -> AgentError {
    AgentError::State(
        "This controller of a privacy conversation has no source for that input. Nothing further was sent."
            .into(),
    )
}

/// What an installed adapter hands the agent as its source. It says nothing:
/// the boundary recognizes it only as the very object of an adapter it holds.
struct ControlSource {
    _of: &'static str,
}

fn own_source(of: &'static str) -> SourceRef {
    SourceRef::new(Arc::new(ControlSource { _of: of }))
}

/// The adapters made for one boundary. Each place is filled once, before the
/// boundary's agent is built.
#[derive(Default)]
pub(super) struct Installed {
    policy: OnceLock<Weak<SelectedControls>>,
    completion: OnceLock<Weak<SelectedVerification>>,
}

impl Installed {
    /// Whether the controllers the agent's builder installed are exactly the
    /// adapters held here, each with the source it was made with, and no
    /// other. An agent without controllers fits a boundary without adapters.
    pub(super) fn fit(&self, bindings: &ActualControllerBindings) -> bool {
        let policy = match self.policy.get().map(Weak::upgrade) {
            None => !bindings.has_policy(),
            Some(None) => false,
            Some(Some(adapter)) => {
                let installed: Arc<dyn AgentBoundaryPolicy> = adapter.clone();
                bindings.policy_matches(&installed)
                    && bindings
                        .policy_source()
                        .is_some_and(|source| source.is(&adapter.source))
            }
        };
        let completion = match self.completion.get().map(Weak::upgrade) {
            None => !bindings.has_completion(),
            Some(None) => false,
            Some(Some(adapter)) => {
                let installed: Arc<dyn CompletionCheck> = adapter.clone();
                bindings.completion_matches(&installed)
                    && bindings
                        .completion_source()
                        .is_some_and(|source| source.is(&adapter.source))
            }
        };
        policy && completion
    }

    /// Whether `installed` is the policy adapter held here: the same object.
    pub(super) fn policy_is(&self, installed: &Arc<dyn AgentBoundaryPolicy>) -> bool {
        self.policy
            .get()
            .is_some_and(|held| std::ptr::addr_eq(Weak::as_ptr(held), Arc::as_ptr(installed)))
    }

    /// The completion adapter held here, while it exists.
    pub(super) fn completion(&self) -> Option<Arc<SelectedVerification>> {
        self.completion.get().and_then(Weak::upgrade)
    }

    pub(super) fn set_policy(&self, adapter: &Arc<SelectedControls>) -> bool {
        self.policy.set(Arc::downgrade(adapter)).is_ok()
    }

    pub(super) fn set_completion(&self, adapter: &Arc<SelectedVerification>) -> bool {
        self.completion.set(Arc::downgrade(adapter)).is_ok()
    }
}

/// Runs `callback` on a state that holds `message` alone and the values
/// under `keys` of `state`'s custom state, and writes those keys back. The
/// callback's result is returned as it is; what it recorded under its own
/// keys is kept either way, as the ordinary callback keeps it. A callback
/// that changed anything else is refused and nothing is written back.
fn on_own_keys(
    state: &mut AgentState,
    message: Option<&Msg>,
    keys: &[&'static str],
    callback: impl FnOnce(&mut AgentState) -> Result<(), AgentError>,
) -> Result<(), AgentError> {
    let mut own = AgentState::default();
    own.messages.extend(message.cloned());
    for key in keys {
        if let Some(value) = state.custom.get(*key) {
            own.custom.insert((*key).to_owned(), value.clone());
        }
    }
    let result = callback(&mut own);
    let mut written = Vec::with_capacity(keys.len());
    for key in keys {
        written.push((*key, own.custom.remove(*key)));
    }
    let untouched = own.messages.as_slice() == message.map(std::slice::from_ref).unwrap_or(&[])
        && own.custom.is_empty()
        && own.todos.is_empty()
        && own.files.is_empty()
        && own.system_extras.is_empty()
        && own.last_usage.is_none()
        && own.last_usage_msg_count == 0;
    if !untouched {
        return Err(refused());
    }
    for (key, value) in written {
        match value {
            Some(value) => {
                state.custom.insert(key.to_owned(), value);
            }
            None => {
                state.custom.remove(key);
            }
        }
    }
    result
}

/// The original context and failure controller of a session, installed on
/// the agent of a privacy conversation.
pub(crate) struct SelectedControls {
    original: Arc<AppControls>,
    boundary: Arc<ConversationBoundary>,
    source: SourceRef,
    own: Weak<SelectedControls>,
    taken: Mutex<Vec<ActualBoundaryInput>>,
    compacted: Mutex<Vec<ActualIdleCompaction>>,
}

impl SelectedControls {
    /// The adapter for `original` on the agent that `boundary` will be
    /// selected on. `None` when the conversation has ended, when the
    /// boundary is not the root agent's, already has its agent or already
    /// has such an adapter.
    pub(crate) fn install(
        boundary: &Arc<ConversationBoundary>,
        original: Arc<AppControls>,
    ) -> Option<Arc<Self>> {
        let adapter = Arc::new_cyclic(|own| Self {
            original,
            boundary: boundary.clone(),
            source: own_source("controls"),
            own: own.clone(),
            taken: Mutex::new(Vec::new()),
            compacted: Mutex::new(Vec::new()),
        });
        boundary.install_controls(&adapter).then_some(adapter)
    }
}

impl AgentBoundaryPolicy for SelectedControls {
    fn view_support(&self) -> ViewSupport {
        ViewSupport::Tracked
    }

    fn source_for_view(&self) -> Option<SourceRef> {
        Some(self.source.clone())
    }

    fn user_input_with_view(
        &self,
        state: &mut AgentState,
        event: &ActualBoundaryInput,
    ) -> Result<(), AgentError> {
        let installed: Arc<dyn AgentBoundaryPolicy> = self.own.upgrade().ok_or_else(refused)?;
        // The conversation's own side: it is live, the event is its agent's,
        // issued for this installed adapter and still in its callback, and
        // the input is text it admitted for exactly the recorded message.
        let quoted = self.boundary.accepted_input(event, &installed)?;
        let message = event.input().commit().message();
        let Msg::User { content } = message else {
            return Err(refused());
        };
        if state.messages.get(event.index()) != Some(message) {
            return Err(refused());
        }
        {
            let mut taken = self.taken.lock().map_err(|_| refused())?;
            if taken.iter().any(|known| known.same_input_event(event)) {
                return Err(refused());
            }
            if taken.len() >= TAKEN {
                taken.remove(0);
            }
            taken.push(event.clone());
        }
        let keys = self.original.own_keys();
        let kind = event.kind();
        let result = on_own_keys(state, Some(message), &keys, |own| {
            self.original.user_input(own, 0, kind)
        });
        // The source goes with the input the original controller actually
        // recorded for this text, whatever it returned after recording it.
        if let Some(stamp) = self.original.recorded_input() {
            if InputStamp::for_input(stamp.revision, content).ok() == Some(stamp) {
                self.original.context.retain_input_source(stamp, quoted);
            }
        }
        result
    }

    /// An agent with a source view hands over an accepted input only with
    /// its event. Without one there is nothing to check it against.
    fn user_input(
        &self,
        _: &mut AgentState,
        _: usize,
        _: HumanInputKind,
    ) -> Result<(), AgentError> {
        Err(refused())
    }

    fn before_model(&self, state: &AgentState) -> Result<ModelBoundaryAction, AgentError> {
        self.boundary.controls_live()?;
        self.original.before_model(state)
    }

    fn runtime_input(&self) {
        self.original.runtime_input();
    }

    fn idle_compaction_committed_with_view(
        &self,
        state: &mut AgentState,
        event: &ActualIdleCompaction,
    ) -> Result<(), AgentError> {
        let installed: Arc<dyn AgentBoundaryPolicy> = self.own.upgrade().ok_or_else(refused)?;
        // The conversation's own side: it is live, the event is its agent's
        // real replacement, issued for this installed adapter with this
        // adapter's source and still in its callback, and what stands in the
        // record now is what was kept and the summary the agent staged.
        self.boundary
            .idle_compaction(event, &installed, &self.source)?;
        let recorded = event.after().len() == state.messages.len()
            && event
                .after()
                .iter()
                .zip(&state.messages)
                .all(|(after, message)| after.commit().message() == message);
        if !recorded {
            return Err(refused());
        }
        {
            let mut compacted = self.compacted.lock().map_err(|_| refused())?;
            if compacted.iter().any(|known| known.same_compaction(event)) {
                return Err(refused());
            }
            if compacted.len() >= TAKEN {
                compacted.remove(0);
            }
            compacted.push(event.clone());
        }
        // The original callback sees its own keys and no message.
        let keys = self.original.own_keys();
        on_own_keys(state, None, &keys, |own| {
            self.original.idle_compaction_committed(own)
        })
    }

    /// An agent with a source view tells its controller of an idle
    /// compaction only with its event.
    fn idle_compaction_committed(&self, _: &mut AgentState) -> Result<(), AgentError> {
        Err(refused())
    }

    fn stop_committed(&self, state: &AgentState) {
        self.original.stop_committed(state);
    }
}

/// The original verification controller of a session, installed as the
/// completion check on the agent of a privacy conversation.
pub(crate) struct SelectedVerification {
    original: Arc<VerificationController>,
    boundary: Arc<ConversationBoundary>,
    source: SourceRef,
    /// The check the original controller directed and the agent has not
    /// scheduled yet.
    directed: Mutex<Option<(String, serde_json::Value)>>,
}

impl SelectedVerification {
    /// The adapter for `original` on the agent that `boundary` will be
    /// selected on. `None` under the conditions of
    /// [`SelectedControls::install`].
    pub(crate) fn install(
        boundary: &Arc<ConversationBoundary>,
        original: Arc<VerificationController>,
    ) -> Option<Arc<Self>> {
        let adapter = Arc::new(Self {
            original,
            boundary: boundary.clone(),
            source: own_source("verification"),
            directed: Mutex::new(None),
        });
        boundary.install_verification(&adapter).then_some(adapter)
    }

    /// Whether `source` is the one this adapter was made with.
    pub(super) fn has_source(&self, source: &SourceRef) -> bool {
        source.is(&self.source)
    }

    /// The check the original controller directed last, once. The boundary
    /// takes it when the agent schedules a call for this adapter.
    pub(super) fn take_directed(&self) -> Option<(String, serde_json::Value)> {
        self.directed.lock().ok()?.take()
    }
}

#[async_trait::async_trait]
impl CompletionCheck for SelectedVerification {
    fn source_for_view(&self) -> Option<SourceRef> {
        Some(self.source.clone())
    }

    async fn before_finish(&self, state: &AgentState) -> Result<CompletionAction, AgentError> {
        self.boundary.controls_live()?;
        if let Ok(mut directed) = self.directed.lock() {
            *directed = None;
        }
        let action = self.original.before_finish(state).await?;
        if let CompletionAction::Check { tool, args } = &action {
            *self.directed.lock().map_err(|_| refused())? = Some((tool.clone(), args.clone()));
        }
        Ok(action)
    }

    async fn after_check(&self, state: &AgentState) -> Result<CompletionAfterCheck, AgentError> {
        self.original.after_check(state).await
    }
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
