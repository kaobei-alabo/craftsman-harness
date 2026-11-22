//! Optional local observers and sampling admission. The Agent owns finalization.

use crate::{AgentError, AgentState};

/// A local policy may refuse another model request at an actual round boundary.
/// This never supplies a model answer, a tool result or a cancellation request.
#[non_exhaustive]
pub enum ModelBoundaryAction {
    Continue,
    Stop { reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HumanInputKind {
    Initial,
    Steering,
}

/// Frontend-selected, bounded local bookkeeping and sampling admission.
/// Default builders have no policy; all existing loop behavior stays unchanged.
pub trait AgentBoundaryPolicy: Send + Sync {
    /// Compatibility only; a declaration cannot grant application authority.
    fn view_support(&self) -> crate::provenance::ViewSupport {
        crate::provenance::ViewSupport::Unadapted
    }
    /// Actual private source of this installed adapter. The selected host
    /// validates its issuer/controller identity; generic erasure proves nothing.
    fn source_for_view(&self) -> Option<crate::provenance::SourceRef> {
        None
    }
    /// An actual accepted input/source event of this installed controller.
    /// A selected adapter must validate its private producer and restrict
    /// mutation to its own bookkeeping before invoking the original callback.
    /// The default refuses; installation, role, text or custom JSON is no source.
    fn user_input_with_view(
        &self,
        _state: &mut AgentState,
        _event: &crate::provenance::ActualBoundaryInput,
    ) -> Result<(), AgentError> {
        Err(AgentError::State(
            "This controller has no source for its accepted input.".into(),
        ))
    }

    /// The actual human input has been appended. Its ordinary checkpoint follows
    /// this callback, so derived context is saved together with that input.
    /// Runtime notifications and conversation transfer do not call this method.
    fn user_input(
        &self,
        _state: &mut AgentState,
        _message_index: usize,
        _kind: HumanInputKind,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    /// Called after the previous committed tool group and accepted round input,
    /// honoring the selected progress-checkpoint policy, before prepare hooks
    /// or any further model request.
    fn before_model(&self, _state: &AgentState) -> Result<ModelBoundaryAction, AgentError> {
        Ok(ModelBoundaryAction::Continue)
    }

    /// Actual peer/background input arrived. It is not human task authority;
    /// observers may reset derived pressure without inspecting its text.
    fn runtime_input(&self) {}

    /// The owned idle compactor has committed its real replacement in memory;
    /// the ordinary required final checkpoint follows this callback.
    fn idle_compaction_committed(&self, _state: &mut AgentState) -> Result<(), AgentError> {
        Ok(())
    }

    /// Observation only: ordinary end hooks and final save actually succeeded.
    /// It is not an execution lease or proof of background/OS quiescence.
    fn stop_committed(&self, _state: &AgentState) {}
}
