use crate::error::AgentError;
use crate::middleware::{AgentMiddleware, Flow};
use crate::state::{AgentState, Msg};
use async_trait::async_trait;

/// Repairs malformed tool-call args on the last assistant message
/// (string-encoded JSON → object; garbage → {} with `_raw` preserved).
pub struct PatchToolCallsMiddleware;

#[async_trait]
impl AgentMiddleware for PatchToolCallsMiddleware {
    fn name(&self) -> &str {
        "patch_tool_calls"
    }

    async fn review_model(&self, st: &mut AgentState) -> Result<Flow, AgentError> {
        if let Some(Msg::Assistant { tool_calls, .. }) = st.messages.last_mut() {
            for tc in tool_calls.iter_mut() {
                if let serde_json::Value::String(raw) = &tc.args {
                    tc.args = serde_json::from_str::<serde_json::Value>(raw)
                        .ok()
                        .filter(|v| v.is_object())
                        .unwrap_or_else(|| serde_json::json!({ "_raw": raw }));
                }
            }
        }
        Ok(Flow::Proceed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AgentState, Msg, ToolCall};

    #[tokio::test]
    async fn string_args_are_parsed_into_objects() {
        let mw = PatchToolCallsMiddleware;
        let mut st = AgentState::default();
        st.messages.push(Msg::Assistant {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                args: serde_json::Value::String("{\"text\": \"hi\"}".into()),
            }],
        });
        use crate::middleware::AgentMiddleware;
        mw.review_model(&mut st).await.unwrap();
        let Msg::Assistant { tool_calls, .. } = &st.messages[0] else {
            panic!()
        };
        assert_eq!(tool_calls[0].args["text"], "hi");
    }

    #[tokio::test]
    async fn garbage_args_become_empty_object_with_raw() {
        let mw = PatchToolCallsMiddleware;
        let mut st = AgentState::default();
        st.messages.push(Msg::Assistant {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                args: serde_json::Value::String("not json{".into()),
            }],
        });
        use crate::middleware::AgentMiddleware;
        mw.review_model(&mut st).await.unwrap();
        let Msg::Assistant { tool_calls, .. } = &st.messages[0] else {
            panic!()
        };
        assert!(tool_calls[0].args.is_object());
        assert_eq!(tool_calls[0].args["_raw"], "not json{");
    }
}
