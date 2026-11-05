use crate::error::AgentError;
use crate::middleware::AgentMiddleware;
use crate::model::ToolDef;
use crate::state::{AgentState, Todo};
use crate::tool::{Tool, ToolCtx, ToolOutcome};
use async_trait::async_trait;
use std::sync::Arc;

/// Prompt section describing the `write_todos` tool.
// PROVENANCE(P5): adapted from langchain 1.2.4 WRITE_TODOS_SYSTEM_PROMPT (todo.py);
// see docs/oss-review.md 5.1 #4. Rewritten in P5.
const TODO_SYSTEM_PROMPT: &str = "## `write_todos`\n\nYou have access to the `write_todos` tool to help you manage and plan complex objectives.\nUse this tool for complex objectives to ensure that you are tracking each necessary step and giving the user visibility into your progress.\nThis tool is very helpful for planning complex objectives, and for breaking down these larger complex objectives into smaller steps.\n\nIt is critical that you mark todos as completed as soon as you are done with a step. Do not batch up multiple steps before marking them as completed.\nFor simple objectives that only require a few steps, it is better to just complete the objective directly and NOT use this tool.\nWriting todos takes time and tokens, use it when it helps you stay organized and track progress.";

pub struct TodoListMiddleware;

impl TodoListMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TodoListMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

struct WriteTodosTool;

#[async_trait]
impl Tool for WriteTodosTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "write_todos".into(),
            description: "Create and manage a structured task list for the current session. Overwrites the entire list; include every item each time.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                            },
                            "required": ["content", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let todos: Vec<Todo> =
            serde_json::from_value(args["todos"].clone()).map_err(|e| AgentError::Tool {
                name: "write_todos".into(),
                message: e.to_string(),
            })?;
        ctx.state.todos = todos;
        let listing = serde_json::to_string(&ctx.state.todos).unwrap_or_default();
        Ok(ToolOutcome::ok(format!("Updated todo list to {listing}")))
    }
}

#[async_trait]
impl AgentMiddleware for TodoListMiddleware {
    fn name(&self) -> &str {
        "todos"
    }

    fn tools(&self) -> Vec<(ToolDef, Arc<dyn Tool>)> {
        let t = Arc::new(WriteTodosTool);
        vec![(t.def(), t)]
    }

    async fn prepare_model(&self, st: &mut AgentState) -> Result<(), AgentError> {
        let mut section = TODO_SYSTEM_PROMPT.to_string();
        if !st.todos.is_empty() {
            section.push_str("\n\nCurrent todo list:\n");
            for t in &st.todos {
                section.push_str(&format!("- [{:?}] {}\n", t.status, t.content));
            }
        }
        st.system_extras.push(section);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Agent;
    use crate::state::{Msg, TodoStatus};
    use crate::testing::ScriptedModel;
    use std::sync::Arc;

    #[tokio::test]
    async fn write_todos_replaces_list_in_state() {
        let model = Arc::new(ScriptedModel::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall {
                    id: "c1".into(),
                    name: "write_todos".into(),
                    args: serde_json::json!({"todos":[{"content":"step 1","status":"in_progress"}]}),
                }],
            },
            Msg::assistant("planned"),
        ]));
        let agent = Agent::builder(model)
            .middleware(Arc::new(TodoListMiddleware::new()))
            .build();
        agent.run_turn("plan it").await.unwrap();
        let st = agent.state.lock().unwrap();
        assert_eq!(st.todos.len(), 1);
        assert_eq!(st.todos[0].status, TodoStatus::InProgress);
    }

    #[tokio::test]
    async fn current_todos_surface_in_system_prompt() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("k")]));
        let mw = Arc::new(TodoListMiddleware::new());
        let agent = Agent::builder(model.clone()).middleware(mw).build();
        agent.state.lock().unwrap().todos.push(crate::state::Todo {
            content: "carried over".into(),
            status: TodoStatus::Pending,
        });
        agent.run_turn("next").await.unwrap();
        let reqs = model.requests.lock().unwrap();
        assert!(reqs[0].system.contains("carried over"));
    }
}
