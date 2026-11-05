//! Private checked finite snapshots of the actual main retained messages.
//! Original state/control remains private and unchanged. No ordered/all-role
//! ledger, public result or storage writer is wired. Selection is a separate
//! private whole-message suffix operation AFTER this unchanged capture.
use super::provider::request::{Fault, retained_message_view};
use super::runtime::MessageScope;

mod selection;
pub(crate) use selection::Selection;
pub(in crate::exec) use selection::checked_message_layout;

/// Only capture(actual Runtime scope) can mint this immutable checked view.
/// No raw state, executable argument, Debug/Serialize or caller-byte constructor.
pub(crate) struct View<'a> {
    scope: MessageScope<'a>,
    count: usize,
    bytes: Vec<u8>,
}
impl View<'_> {
    pub(crate) fn bytes(&self) -> Result<&[u8], Fault> {
        self.scope.ready()?;
        Ok(&self.bytes)
    }
    pub(crate) fn message_count(&self) -> Result<usize, Fault> {
        self.scope.ready()?;
        Ok(self.count)
    }
    pub(crate) fn bind(&self, scope: &MessageScope<'_>) -> Result<(), Fault> {
        if !self.scope.same(scope) {
            return Err(Fault::Collision);
        }
        self.scope.ready()?;
        scope.ready()
    }
}

/// The SAME state lock covers original admission, copying and finite encoding.
/// No whole AgentState clone precedes bounds; other state fields are unselected.
pub(super) fn capture(scope: MessageScope<'_>) -> Result<View<'_>, Fault> {
    scope.ready()?;
    let state = scope.state().lock().map_err(|_| Fault::Collision)?;
    let count = state.messages.len();
    let result = retained_message_view(scope.redactor(), &state.messages);
    drop(state);
    // A typed late stop dominates codec errors, never guessed from text.
    scope.ready()?;
    let bytes = result?;
    Ok(View {
        scope,
        count,
        bytes,
    })
}

impl<'a> View<'a> {
    /// Consume THIS checked snapshot; a byte limit can only tighten private caps.
    pub(crate) fn select(self, max_bytes: usize) -> Result<Selection<'a>, Fault> {
        selection::select(self, max_bytes)
    }
    #[cfg(test)]
    pub(crate) fn select_after_work_for_tests(
        self,
        max_bytes: usize,
        after: impl FnOnce(),
    ) -> Result<Selection<'a>, Fault> {
        selection::select_after(self, max_bytes, after)
    }
}
