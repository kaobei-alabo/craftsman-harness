//! The `ask_user` tool — mid-task clarification.
//!
//! The agent calls this when it needs a decision only the human can make. In
//! the interactive REPL it prints the question and reads a reply from stdin; in
//! one-shot / headless mode (no controlling human) it declines cleanly so the
//! model proceeds on its own assumptions rather than blocking forever on a
//! read that will never be answered.

use crate::agent::AskUserPrompt;
use async_trait::async_trait;
use craftsman_harness::{AgentError, Tool, ToolCtx, ToolDef, ToolOutcome};
use serde_json::{Value, json};

pub struct AskUserTool {
    /// False in one-shot mode: no human is present, so the tool declines
    /// instead of blocking on stdin.
    interactive: bool,
    prompt: Option<AskUserPrompt>,
}

impl AskUserTool {
    pub fn new(interactive: bool, prompt: Option<AskUserPrompt>) -> Self {
        Self {
            interactive,
            prompt,
        }
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "ask_user".into(),
            description: "Ask the user a question and wait for their typed reply. \
                 Use this only when you genuinely need a decision or detail that \
                 you cannot determine yourself. Returns the user's answer as the \
                 tool result."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string", "description": "The question to ask the user."}
                },
                "required": ["question"]
            }),
        }
    }

    async fn call(&self, args: Value, _ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let question = args["question"].as_str().unwrap_or_default();
        if question.is_empty() {
            return Ok(ToolOutcome::err("Error: 'question' is required"));
        }
        if let Some(prompt) = &self.prompt {
            return Ok(match prompt(question) {
                Some(answer) if !answer.trim().is_empty() => ToolOutcome::ok(answer),
                _ => ToolOutcome::ok(
                    "The user did not provide an answer. Proceed with your best assumption.",
                ),
            });
        }
        if !self.interactive {
            return Ok(ToolOutcome::ok(
                "No interactive user is available (running non-interactively). \
                 Proceed with your best assumption and note it in your answer.",
            ));
        }
        // Prompt on stderr so a piped stdout stays clean; read the reply from
        // stdin. Blocking read is acceptable — the whole point is to wait for
        // the human. EOF (closed stdin) is treated as "no answer".
        use std::io::{BufRead, Write};
        eprintln!("\n❓ {question}");
        eprint!("  > ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => Ok(ToolOutcome::ok(
                "The user did not provide an answer (input closed). Proceed with \
                 your best assumption.",
            )),
            Ok(_) => {
                let answer = line.trim();
                if answer.is_empty() {
                    Ok(ToolOutcome::ok(
                        "The user gave no answer (empty). Proceed with your best assumption.",
                    ))
                } else {
                    Ok(ToolOutcome::ok(answer.to_string()))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn non_interactive_declines_without_blocking() {
        let tool = AskUserTool::new(false, None);
        // No ToolCtx needed for the non-interactive path; build a minimal one.
        use craftsman_harness::{AgentState, Backend, StateFs};
        use std::sync::{Arc, Mutex};
        let state = Arc::new(Mutex::new(AgentState::default()));
        let backend: Arc<dyn Backend> = Arc::new(StateFs::new(state.clone()));
        let mut working = state.lock().unwrap().clone();
        let registry = Arc::new(craftsman_harness::AgentRegistry::default());
        let (notif, _rx) = craftsman_harness::notification_channel();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications: notif,
        };
        let out = tool
            .call(json!({"question": "pick a color?"}), &mut ctx)
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("non-interactively"));
    }

    #[tokio::test]
    async fn empty_question_is_error() {
        let tool = AskUserTool::new(true, None);
        use craftsman_harness::{AgentState, Backend, StateFs};
        use std::sync::{Arc, Mutex};
        let state = Arc::new(Mutex::new(AgentState::default()));
        let backend: Arc<dyn Backend> = Arc::new(StateFs::new(state.clone()));
        let mut working = state.lock().unwrap().clone();
        let registry = Arc::new(craftsman_harness::AgentRegistry::default());
        let (notif, _rx) = craftsman_harness::notification_channel();
        let mut ctx = ToolCtx {
            state: &mut working,
            backend,
            registry,
            notifications: notif,
        };
        let out = tool.call(json!({}), &mut ctx).await.unwrap();
        assert!(out.is_error);
    }
}
