use crate::backend::Backend;
use crate::error::AgentError;
use crate::model::ToolDef;
use crate::state::AgentState;
use async_trait::async_trait;
use std::sync::Arc;

/// What a tool hands back to the loop.
///
/// `#[non_exhaustive]`: build it with [`ToolOutcome::ok`] or
/// [`ToolOutcome::err`] (plus [`ToolOutcome::with_meta`]), so adding a field
/// never breaks a call site.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ToolOutcome {
    /// What the model reads.
    pub content: String,
    pub is_error: bool,
    /// Facts about the call for the harness, never sent to the model.
    pub meta: ToolMeta,
}

impl ToolOutcome {
    pub fn ok(s: impl Into<String>) -> Self {
        Self {
            content: s.into(),
            is_error: false,
            meta: ToolMeta::new(),
        }
    }
    pub fn err(s: impl Into<String>) -> Self {
        Self {
            content: s.into(),
            is_error: true,
            meta: ToolMeta::new(),
        }
    }
    /// The same outcome carrying `meta`.
    pub fn with_meta(mut self, meta: ToolMeta) -> Self {
        self.meta = meta;
        self
    }
}

/// Facts about one tool call that the harness may use and the model never
/// sees: the single extension point of [`ToolOutcome`] (later phases add
/// fields here, which `#[non_exhaustive]` keeps from breaking anyone).
/// Construct with [`ToolMeta::new`] and set the public fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolMeta {
    /// Exit code of a process the tool ran (`-1` when it had none, for
    /// example killed by a signal).
    pub exit_code: Option<i32>,
    /// Wall-clock time of the call.
    pub duration: Option<std::time::Duration>,
    /// Bytes of output the tool produced before any cut.
    pub bytes: Option<u64>,
    /// HTTP requests initiated by this tool (including unsuccessful requests).
    pub network_calls: u64,
    /// Whether `content` is a cut of a larger output.
    pub truncated: bool,
    /// Why the call failed, when it did.
    pub error_kind: Option<ToolErrorKind>,
}

impl ToolMeta {
    /// Empty facts: nothing known.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Why a tool call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolErrorKind {
    /// A real turn/session stop interrupted the operation.
    Cancelled,
    /// The call hit its time limit and was stopped.
    Timeout,
    /// The tool could not start its process.
    Spawn,
    /// A named resource or function does not exist.
    NotFound,
    /// Execution requires an explicit user grant.
    NeedsApproval,
    /// The supplied arguments are malformed.
    InvalidArgs,
    /// A declared or approved capability does not permit the operation.
    DeniedCapability,
    /// Sandboxed code trapped.
    Trap,
    /// Sandboxed code exceeded its memory budget.
    OutOfMemory,
    /// Output exceeded the tool's byte budget.
    OutputLimit,
    /// Another tool-specific failure.
    Failed,
}

/// Execution context handed to tools: mutable state, the active backend, the
/// shared subagent registry (so the `task` tool can track spawned children), and
/// the notification queue (so tools can post inter-agent / completion messages).
pub struct ToolCtx<'a> {
    pub state: &'a mut AgentState,
    pub backend: Arc<dyn Backend>,
    pub registry: Arc<crate::subagent::AgentRegistry>,
    pub notifications: crate::subagent::NotificationQueue,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> ToolDef;
    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_start_with_empty_meta_and_with_meta_sets_it() {
        let ok = ToolOutcome::ok("fine");
        assert!(!ok.is_error);
        assert_eq!(ok.meta, ToolMeta::new());
        let mut meta = ToolMeta::new();
        meta.exit_code = Some(3);
        meta.truncated = true;
        meta.error_kind = Some(ToolErrorKind::Timeout);
        let err = ToolOutcome::err("bad").with_meta(meta.clone());
        assert!(err.is_error);
        assert_eq!(err.content, "bad");
        assert_eq!(err.meta, meta);
    }
}
