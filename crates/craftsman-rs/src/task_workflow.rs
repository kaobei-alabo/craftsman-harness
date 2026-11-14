//! App-selected task workflow and bounded hints from actual allowed tool errors.
use async_trait::async_trait;
use craftsman_harness::{
    AgentError, AgentMiddleware, AgentState, ToolCall, ToolCtx, ToolErrorKind, ToolNext,
    ToolOutcome,
};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

// PROVENANCE(P5): original Craftsman task workflow guidance.
pub(crate) const WORKFLOW_GUIDANCE: &str = "Task workflow:\n\
Scale planning and delegation to the work you actually find. For a small change, inspect the relevant files, make the change, and verify it directly. Use delegated agents when independent work benefits from separate context or parallel execution.\n\
Run every check required by the user or repository, plus checks appropriate to the affected behavior. Read each check's output and exit status to determine whether it passed. Preserve existing tests and explain any necessary test changes.\n\
Once the requested work and required verification are complete, give the result promptly. Repeat reads, checks, planning updates, or reviews when changed files, new evidence, or an unresolved concern justify them. If a check or tool is unavailable, use an appropriate available check or explain the limitation. Respect current instructions, permission decisions, and any effects an unsuccessful operation may already have had.";

// PROVENANCE(P5): original Craftsman tool-error recovery guidance.
pub(crate) const REPEATED_ERROR_GUIDANCE: &str = "Recent allowed calls to the same tool returned the same error repeatedly. Inspect the error and any partial effects before another attempt. Use new evidence or a different permitted approach; report the blocker when it cannot be resolved. Keep permission decisions in force.";

const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_TOOL_NAME_BYTES: usize = 512;

#[derive(Default)]
struct Progress {
    last: Option<([u8; 32], [u8; 32])>,
    repeated: bool,
}

#[derive(Default)]
pub(crate) struct TaskWorkflowMiddleware {
    progress: Mutex<Progress>,
}

impl TaskWorkflowMiddleware {
    fn clear(&self) {
        *self.progress.lock().unwrap() = Progress::default();
    }

    fn observe(&self, call: &ToolCall, outcome: &ToolOutcome) {
        // Permission decisions and cancellation retain their existing owners.
        // Observe only bounded, model-visible error results from actual calls;
        // nonzero shell exits alone are not classified as errors here.
        if !outcome.is_error
            || matches!(
                outcome.meta.error_kind,
                Some(
                    ToolErrorKind::NeedsApproval
                        | ToolErrorKind::DeniedCapability
                        | ToolErrorKind::Cancelled
                )
            )
            || outcome.content.len() > MAX_ERROR_BYTES
            || call.name.len() > MAX_TOOL_NAME_BYTES
        {
            self.clear();
            return;
        }
        let signature = (
            Sha256::digest(call.name.as_bytes()).into(),
            Sha256::digest(outcome.content.as_bytes()).into(),
        );
        let mut progress = self.progress.lock().unwrap();
        progress.repeated = progress.last == Some(signature);
        progress.last = Some(signature);
    }
}

#[async_trait]
impl AgentMiddleware for TaskWorkflowMiddleware {
    fn name(&self) -> &str {
        "task_workflow"
    }

    async fn turn_start(&self, _: &mut AgentState) -> Result<(), AgentError> {
        self.clear();
        Ok(())
    }

    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        state.system_extras.push(WORKFLOW_GUIDANCE.into());
        if self.progress.lock().unwrap().repeated {
            state.system_extras.push(REPEATED_ERROR_GUIDANCE.into());
        }
        Ok(())
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        match next.run(call.clone(), ctx).await {
            Ok(outcome) => {
                self.observe(&call, &outcome);
                Ok(outcome)
            }
            Err(error) => {
                self.clear();
                Err(error)
            }
        }
    }

    async fn turn_end(&self, _: &mut AgentState) -> Result<(), AgentError> {
        self.clear();
        Ok(())
    }
}

#[cfg(test)]
#[path = "task_workflow/tests.rs"]
mod tests;
