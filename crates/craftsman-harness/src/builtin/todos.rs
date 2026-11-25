use crate::error::AgentError;
use crate::middleware::AgentMiddleware;
use crate::model::ToolDef;
use crate::provenance::{OutcomePart, SourcePart, TrackedDispatch, TrackedPrepare, ViewSupport};
use crate::state::{AgentState, Todo, TodoStatus};
use crate::tool::{Tool, ToolCtx, ToolOutcome};
use async_trait::async_trait;
use std::sync::{Arc, OnceLock};

/// The fixed pieces of the todo section, each owned once for the process so
/// that the same piece is the same source in every round.
fn fixed(cell: &'static OnceLock<Arc<str>>, text: &str) -> Arc<str> {
    cell.get_or_init(|| Arc::from(text)).clone()
}

fn status_label(status: TodoStatus) -> Arc<str> {
    static PENDING: OnceLock<Arc<str>> = OnceLock::new();
    static IN_PROGRESS: OnceLock<Arc<str>> = OnceLock::new();
    static COMPLETED: OnceLock<Arc<str>> = OnceLock::new();
    let cell = match status {
        TodoStatus::Pending => &PENDING,
        TodoStatus::InProgress => &IN_PROGRESS,
        TodoStatus::Completed => &COMPLETED,
    };
    fixed(cell, &format!("- [{status:?}] "))
}

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

    /// The same call on an agent with a source view. The stored list is the
    /// `content` strings of this call's own arguments, in order, and the tool
    /// says so, so that each stored todo keeps the call it came from.
    async fn call_with_view(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
        tracked: &TrackedDispatch,
    ) -> Result<ToolOutcome, AgentError> {
        let outcome = self.call(args, ctx).await?;
        tracked.todos_from_arguments(
            (0..ctx.state.todos.len())
                .map(|index| format!("/todos/{index}/content"))
                .collect(),
        );
        // The outcome repeats the list. Its text is the tool's own fixed
        // words and punctuation around the JSON encoding of each stored
        // todo's content, which is a string of this call's arguments.
        let fixed = |text: String| OutcomePart::Fixed(Arc::from(text));
        let mut parts = vec![fixed("Updated todo list to [".into())];
        for (index, todo) in ctx.state.todos.iter().enumerate() {
            let before = if index == 0 { "" } else { "," };
            parts.push(fixed(format!("{before}{{\"content\":")));
            parts.push(OutcomePart::ArgumentJson {
                pointer: format!("/todos/{index}/content"),
                text: Arc::from(serde_json::to_string(&todo.content).unwrap_or_default()),
            });
            let status = serde_json::to_string(&todo.status).unwrap_or_default();
            parts.push(fixed(format!(",\"status\":{status}}}")));
        }
        parts.push(fixed("]".into()));
        tracked.outcome_is(parts);
        Ok(outcome)
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

    /// On an agent with a source view the section is the same text, added as
    /// parts: the fixed prompt, heading, status labels and line ends are the
    /// application's, and each todo's text is the text of the call that
    /// stored it.
    fn view_support(&self) -> ViewSupport {
        ViewSupport::Tracked
    }

    async fn prepare_model_with_view(
        &self,
        prepare: &mut TrackedPrepare<'_>,
    ) -> Result<(), AgentError> {
        static PROMPT: OnceLock<Arc<str>> = OnceLock::new();
        static HEADING: OnceLock<Arc<str>> = OnceLock::new();
        static LINE_END: OnceLock<Arc<str>> = OnceLock::new();
        let mut parts = vec![SourcePart::fixed(fixed(&PROMPT, TODO_SYSTEM_PROMPT))];
        let todos = prepare.state().todos.clone();
        if !todos.is_empty() {
            parts.push(SourcePart::fixed(fixed(
                &HEADING,
                "\n\nCurrent todo list:\n",
            )));
            for (index, todo) in todos.iter().enumerate() {
                let from = prepare.todo_origin(index).ok_or_else(|| {
                    AgentError::State(
                        "A stored todo of this conversation has no source. Nothing further was sent."
                            .into(),
                    )
                })?;
                parts.push(SourcePart::fixed(status_label(todo.status)));
                parts.push(SourcePart {
                    text: Arc::from(todo.content.as_str()),
                    from,
                });
                parts.push(SourcePart::fixed(fixed(&LINE_END, "\n")));
            }
        }
        prepare.add_system(parts)
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
