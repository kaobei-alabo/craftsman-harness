//! Test doubles, exported so app crates can reuse them.
use crate::error::AgentError;
use crate::model::{ModelProvider, ModelRequest, ModelResponse, ToolDef, Usage};
use crate::state::{AgentState, Msg};
use crate::tool::{Tool, ToolCtx, ToolOutcome};
use async_trait::async_trait;
use std::sync::Mutex;

/// A tool that returns a fixed ok result — for exercising the loop/middleware
/// without real side effects.
pub struct NoopTool {
    name: String,
}

impl NoopTool {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl Tool for NoopTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name.clone(),
            description: "noop".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _args: serde_json::Value,
        _ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        Ok(ToolOutcome::ok("noop"))
    }
}

/// Replays a fixed sequence of assistant messages; errors when exhausted.
pub struct ScriptedModel {
    script: Mutex<std::vec::IntoIter<Msg>>,
    pub requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
    pub fn new(script: Vec<Msg>) -> Self {
        Self {
            script: Mutex::new(script.into_iter()),
            requests: Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl ModelProvider for ScriptedModel {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.requests.lock().unwrap().push(req);
        match self.script.lock().unwrap().next() {
            Some(message) => Ok(ModelResponse {
                message,
                usage: Usage::default(),
            }),
            None => Err(AgentError::Model("scripted model exhausted".into())),
        }
    }
}

/// In-memory Checkpointer recording every save — for asserting persistence hooks.
#[derive(Default)]
pub struct MemCheckpointer {
    #[allow(clippy::type_complexity)]
    pub saves: Mutex<
        Vec<(
            String,
            u64,
            AgentState,
            Option<crate::interrupt::Suspension>,
        )>,
    >,
}

#[async_trait]
impl crate::checkpoint::Checkpointer for MemCheckpointer {
    async fn save(
        &self,
        thread_id: &str,
        seq: u64,
        state: &AgentState,
        suspension: Option<&crate::interrupt::Suspension>,
    ) -> Result<(), AgentError> {
        self.saves.lock().unwrap().push((
            thread_id.into(),
            seq,
            state.clone(),
            suspension.cloned(),
        ));
        Ok(())
    }
    async fn load_latest(
        &self,
        thread_id: &str,
    ) -> Result<Option<(u64, AgentState, Option<crate::interrupt::Suspension>)>, AgentError> {
        Ok(self
            .saves
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(t, ..)| t == thread_id)
            .map(|(_, s, st, su)| (*s, st.clone(), su.clone())))
    }
    async fn list_threads(&self) -> Result<Vec<crate::checkpoint::ThreadInfo>, AgentError> {
        Ok(vec![])
    }
    async fn delete_thread(&self, _: &str) -> Result<(), AgentError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelProvider, ModelRequest};
    use crate::state::Msg;

    #[tokio::test]
    async fn scripted_model_replays_in_order_and_errors_when_exhausted() {
        let m = ScriptedModel::new(vec![Msg::assistant("one"), Msg::assistant("two")]);
        let req = ModelRequest {
            system: String::new(),
            messages: vec![],
            tools: vec![],
        };
        assert_eq!(
            m.complete(req.clone()).await.unwrap().message,
            Msg::assistant("one")
        );
        assert_eq!(
            m.complete(req.clone()).await.unwrap().message,
            Msg::assistant("two")
        );
        assert!(m.complete(req).await.is_err());
    }
}
