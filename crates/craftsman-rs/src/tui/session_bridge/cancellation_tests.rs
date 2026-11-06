//! Cancellation reaches real bridge waiters without granting a pending tool.
use super::*;
use crate::tui::input::handle_key;
use craftsman_harness::{
    Agent, AgentError, ModelProvider, ModelRequest, ModelResponse, Msg, Tool, ToolCall, ToolCtx,
    ToolDef, ToolOutcome, Usage,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(1);

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

#[test]
fn ctrl_c_reaches_the_turn_from_approval_feedback_without_editing_it() {
    let mut app = App::new(&Config::default());
    app.running = true;
    let (response, _receive) = std_mpsc::channel();
    app.enqueue_modal(Modal::Approval(ApprovalModal {
        tool: "execute".into(),
        summary: "pending native build".into(),
        details: "original build details".into(),
        selected: 3,
        feedback: "keep this feedback".into(),
        feedback_mode: true,
        response,
    }));

    assert!(!handle_key(&mut app, ctrl_c()));
    assert!(app.cancel_requested);
    assert!(app.discard_stream_deltas);
    let Some(Modal::Approval(modal)) = app.modal.as_ref() else {
        panic!("the prompt must wait until the real agent has been cancelled");
    };
    assert_eq!(modal.feedback, "keep this feedback");
}

#[test]
fn ctrl_c_reaches_the_turn_from_a_question_without_typing_an_answer() {
    let mut app = App::new(&Config::default());
    app.running = true;
    let (response, _receive) = std_mpsc::channel();
    app.enqueue_modal(Modal::Ask(AskModal {
        question: "which artifact?".into(),
        answer: "keep this answer".into(),
        response,
    }));

    assert!(!handle_key(&mut app, ctrl_c()));
    assert!(app.cancel_requested);
    assert!(app.discard_stream_deltas);
    let Some(Modal::Ask(modal)) = app.modal.as_ref() else {
        panic!("the prompt must wait until the real agent has been cancelled");
    };
    assert_eq!(modal.answer, "keep this answer");
}

struct PromptModel {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ModelProvider for PromptModel {
    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ModelResponse {
            message: Msg::Assistant {
                content: String::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "prompt-call".into(),
                        name: "prompt_fixture".into(),
                        args: json!({}),
                    },
                    ToolCall {
                        id: "later-call".into(),
                        name: "later_fixture".into(),
                        args: json!({}),
                    },
                ],
            },
            usage: Usage::default(),
        })
    }
}

enum Prompt {
    Approval(crate::agent::ApprovalPrompt),
    Ask(crate::agent::AskUserPrompt),
}

struct PromptTool {
    prompt: Prompt,
    replies: AtomicUsize,
}

#[async_trait::async_trait]
impl Tool for PromptTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "prompt_fixture".into(),
            description: "Wait for the real terminal prompt bridge.".into(),
            parameters: json!({"type":"object"}),
        }
    }

    async fn call(
        &self,
        _arguments: serde_json::Value,
        _context: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        match &self.prompt {
            Prompt::Approval(prompt) => assert_eq!(
                prompt("execute", "native gadget build", "original Cargo command"),
                ApprovalPromptResult::Reject(String::new()),
            ),
            Prompt::Ask(prompt) => assert_eq!(prompt("choose a gadget"), None),
        }
        self.replies.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutcome::err("prompt cancelled"))
    }
}

struct LaterTool {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Tool for LaterTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "later_fixture".into(),
            description: "Must not run after cancellation.".into(),
            parameters: json!({"type":"object"}),
        }
    }

    async fn call(
        &self,
        _arguments: serde_json::Value,
        _context: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutcome::ok("unexpected later tool"))
    }
}

async fn cancel_real_waiter(approval: bool) {
    let (tx, mut receive) = mpsc::unbounded_channel();
    let extras = UiBridge { tx }.session_extras();
    let prompt = Arc::new(PromptTool {
        prompt: if approval {
            Prompt::Approval(extras.approval_prompt.unwrap())
        } else {
            Prompt::Ask(extras.ask_user_prompt.unwrap())
        },
        replies: AtomicUsize::new(0),
    });
    let later = Arc::new(LaterTool {
        calls: AtomicUsize::new(0),
    });
    let model = Arc::new(PromptModel {
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::builder(model.clone())
        .tool(prompt.clone())
        .tool(later.clone())
        .build();
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("build a gadget").await });
    let mut app = App::new(&Config::default());
    app.running = true;
    let event = tokio::time::timeout(WAIT, receive.recv())
        .await
        .expect("prompt event must arrive")
        .expect("prompt channel must be open");
    handle_ui_event(&mut app, event, None);
    assert!(agent.is_turn_active());
    assert_eq!(prompt.replies.load(Ordering::SeqCst), 0);

    assert!(!handle_key(&mut app, ctrl_c()));
    // Replying here would race with the still-uncancelled agent tool loop.
    assert_eq!(prompt.replies.load(Ordering::SeqCst), 0);
    apply_turn_cancellation(&mut app, Some(&agent));
    let result = tokio::time::timeout(WAIT, task)
        .await
        .expect("cancelled waiter and turn must return")
        .unwrap()
        .unwrap();
    let TurnResult::Done(outcome) = result else {
        panic!("a cancelled prompt must not suspend for approval");
    };
    assert_eq!(outcome.final_text(), "Stopped by the user.");
    assert_eq!(prompt.replies.load(Ordering::SeqCst), 1);
    assert_eq!(later.calls.load(Ordering::SeqCst), 0);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert!(!agent.is_turn_active());
    assert!(!app.cancel_requested);
    assert!(app.discard_stream_deltas);
    assert!(app.modal.is_none());
    let state = agent.state.lock().unwrap();
    assert!(state.messages.iter().any(|message| matches!(
        message,
        Msg::Tool { tool_call_id, is_error: true, content }
            if tool_call_id == "later-call" && content == "Tool call cancelled by the user."
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_approval_waiter_cancels_before_the_next_tool_or_provider_call() {
    cancel_real_waiter(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_question_waiter_cancels_before_the_next_tool_or_provider_call() {
    cancel_real_waiter(false).await;
}

#[test]
fn cancellation_releases_all_prompt_waiters_and_preserves_editor_drafts() {
    use crate::tui::modals::EditorModal;
    let mut app = App::new(&Config::default());
    app.running = true;
    let (first, first_rx) = std_mpsc::channel();
    let (second, second_rx) = std_mpsc::channel();
    let (third, third_rx) = std_mpsc::channel();
    app.enqueue_modal(Modal::Approval(ApprovalModal {
        tool: "execute".into(),
        summary: "first".into(),
        details: "first details".into(),
        selected: 1, // The highlighted "Always" choice must never be applied.
        feedback: String::new(),
        feedback_mode: false,
        response: first,
    }));
    app.enqueue_modal(Modal::Editor(EditorModal {
        text: "keep the queued draft".into(),
    }));
    app.enqueue_modal(Modal::Ask(AskModal {
        question: "second".into(),
        answer: "must not be submitted".into(),
        response: second,
    }));
    app.enqueue_modal(Modal::Approval(ApprovalModal {
        tool: "write_file".into(),
        summary: "third".into(),
        details: "third details".into(),
        selected: 0,
        feedback: String::new(),
        feedback_mode: false,
        response: third,
    }));
    app.input = "keep the composer draft".into();
    handle_key(&mut app, ctrl_c());
    assert!(matches!(
        first_rx.try_recv(),
        Err(std_mpsc::TryRecvError::Empty)
    ));
    apply_turn_cancellation(&mut app, None);
    assert!(
        matches!(first_rx.recv_timeout(WAIT).unwrap(), ApprovalChoice::Deny(text) if text.is_empty())
    );
    assert_eq!(second_rx.recv_timeout(WAIT).unwrap(), None);
    assert!(
        matches!(third_rx.recv_timeout(WAIT).unwrap(), ApprovalChoice::Deny(text) if text.is_empty())
    );
    assert!(app.modal_queue.is_empty());
    let Some(Modal::Editor(modal)) = app.modal.as_ref() else {
        panic!("the original editor must be promoted after prompts are removed");
    };
    assert_eq!(modal.text, "keep the queued draft");
    assert_eq!(app.input, "keep the composer draft");
}

#[test]
fn ctrl_c_in_an_idle_background_prompt_refuses_without_cancelling_a_new_turn() {
    let mut app = App::new(&Config::default());
    let (response, receive) = std_mpsc::channel();
    app.enqueue_modal(Modal::Approval(ApprovalModal {
        tool: "execute".into(),
        summary: "background prompt".into(),
        details: String::new(),
        selected: 1,
        feedback: String::new(),
        feedback_mode: false,
        response,
    }));
    assert!(!handle_key(&mut app, ctrl_c()));
    assert!(
        matches!(receive.recv_timeout(WAIT).unwrap(), ApprovalChoice::Deny(text) if text.is_empty())
    );
    assert!(!app.cancel_requested);
    assert!(!app.discard_stream_deltas);
    assert!(app.modal.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_late_prompt_waiters_refuse_until_turn_completion_then_new_prompts_work() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let extras = UiBridge { tx }.session_extras();
    let approval = extras.approval_prompt.unwrap();
    let question = extras.ask_user_prompt.unwrap();
    let mut app = App::new(&Config::default());
    app.running = true;
    app.request_turn_cancellation();
    apply_turn_cancellation(&mut app, None);
    // The pending flag has been consumed; the persistent stopping state must
    // still refuse events that were posted after the visible modal was closed.
    assert!(!app.cancel_requested);
    let first = approval.clone();
    let approval_task = tokio::spawn(async move { first("execute", "late", "original details") });
    let question_task = tokio::spawn(async move { question("late question") });
    for _ in 0..2 {
        let event = tokio::time::timeout(WAIT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        handle_ui_event(&mut app, event, None);
    }
    assert_eq!(
        tokio::time::timeout(WAIT, approval_task)
            .await
            .unwrap()
            .unwrap(),
        ApprovalPromptResult::Reject(String::new()),
    );
    assert_eq!(
        tokio::time::timeout(WAIT, question_task)
            .await
            .unwrap()
            .unwrap(),
        None
    );
    assert!(app.modal.is_none());
    assert!(app.modal_queue.is_empty());
    app.finish_turn(Err("synthetic completion".into()), None);
    app.running = true;
    let next = tokio::spawn(async move { approval("execute", "next turn", "new details") });
    let event = tokio::time::timeout(WAIT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    handle_ui_event(&mut app, event, None);
    assert!(matches!(app.modal, Some(Modal::Approval(_))));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        tokio::time::timeout(WAIT, next).await.unwrap().unwrap(),
        ApprovalPromptResult::Approve,
    );
}

#[test]
fn turn_cancellation_is_reachable_inside_an_editor_without_losing_its_draft() {
    use crate::tui::modals::EditorModal;
    let mut app = App::new(&Config::default());
    app.running = true;
    app.enqueue_modal(Modal::Editor(EditorModal {
        text: "original editor text".into(),
    }));
    handle_key(&mut app, ctrl_c());
    assert!(app.cancel_requested);
    apply_turn_cancellation(&mut app, None);
    let Some(Modal::Editor(modal)) = app.modal.as_ref() else {
        panic!("cancelling the agent must not discard the editor");
    };
    assert_eq!(modal.text, "original editor text");
}

#[test]
fn modified_shortcuts_do_not_insert_control_letters_or_accept_a_prompt() {
    let mut app = App::new(&Config::default());
    app.running = true;
    let (response, receive) = std_mpsc::channel();
    app.enqueue_modal(Modal::Ask(AskModal {
        question: "original question".into(),
        answer: "original answer".into(),
        response,
    }));
    for modifiers in [
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        KeyModifiers::ALT,
    ] {
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('c'), modifiers));
    }
    let Some(Modal::Ask(modal)) = app.modal.as_ref() else {
        panic!("modified copy shortcuts must not answer the question");
    };
    assert_eq!(modal.answer, "original answer");
    assert!(!app.cancel_requested);
    assert!(matches!(
        receive.try_recv(),
        Err(std_mpsc::TryRecvError::Empty)
    ));
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT),
    );
    let Some(Modal::Ask(modal)) = app.modal.as_ref() else {
        unreachable!()
    };
    assert_eq!(modal.answer, "original answerC");
}

/// Own the synthetic TCP server even when an assertion unwinds.
struct StreamServer(tokio::task::JoinHandle<()>);
impl Drop for StreamServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn cancel_real_stream(send_delta: bool) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const STREAM_WAIT: Duration = Duration::from_secs(5);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let received = Arc::new(tokio::sync::Notify::new());
    let server_received = received.clone();
    let mut server = StreamServer(tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        // Consume the actual bounded request before checking connection EOF.
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() <= 65536);
            if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                assert!(headers.starts_with("POST /chat/completions HTTP/1.1\r\n"));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .expect("the client sends a content length");
                if request.len() == end + 4 + length {
                    break;
                }
            }
        }
        if send_delta {
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial fixture\"}}]}\n\n").await.unwrap();
            socket.flush().await.unwrap();
        }
        server_received.notify_one();
        // The server never completes the response. Cancellation must close
        // this actual connection without a stream timeout or fixture release.
        assert_eq!(socket.read(&mut buffer).await.unwrap(), 0);
    }));
    let (tx, mut receive) = mpsc::unbounded_channel();
    let extras = UiBridge { tx }.session_extras();
    assert!(extras.interrupt_model_on_cancel);
    assert!(!SessionExtras::default().interrupt_model_on_cancel);
    let usage = Arc::new(std::sync::Mutex::new((0, 0)));
    let provider = Arc::new(agent::StreamingLlmProvider {
        client: crate::llm::LlmClient::new_with_options(
            &format!("http://{address}"),
            "synthetic-key",
            "synthetic-model",
            &std::collections::BTreeMap::new(),
        ),
        usage: usage.clone(),
        sink: extras.token_sink.unwrap(),
        round_sink: extras.round_sink,
        status_sink: extras.activity_sink,
    });
    let agent = Agent::builder(provider)
        .interrupt_model_on_cancel(extras.interrupt_model_on_cancel)
        .build();
    let running = agent.clone();
    let task = tokio::spawn(async move { running.run_turn("isolated synthetic stream").await });
    let mut app = App::new(&Config::default());
    app.running = true;
    tokio::time::timeout(STREAM_WAIT, received.notified())
        .await
        .unwrap();
    if send_delta {
        loop {
            let event = tokio::time::timeout(STREAM_WAIT, receive.recv())
                .await
                .unwrap()
                .unwrap();
            let is_delta = matches!(&event, UiEvent::Delta(text) if text == "partial fixture");
            handle_ui_event(&mut app, event, None);
            if is_delta {
                break;
            }
        }
    }
    assert!(!task.is_finished());
    handle_key(&mut app, ctrl_c());
    apply_turn_cancellation(&mut app, Some(&agent));
    let result = tokio::time::timeout(STREAM_WAIT, task)
        .await
        .expect("Ctrl-C must release the pending client request")
        .unwrap()
        .unwrap();
    let TurnResult::Done(output) = &result else {
        panic!("model cancellation cannot park a tool approval")
    };
    assert_eq!(output.final_text(), "Stopped by the user.");
    tokio::time::timeout(STREAM_WAIT, &mut server.0)
        .await
        .expect("the actual TCP peer must observe client disconnect")
        .unwrap();
    assert!(!agent.is_turn_active());
    assert!(agent.state.lock().unwrap().last_usage.is_none());
    // No completed usage was received; the existing aggregate is unchanged.
    assert_eq!(*usage.lock().unwrap(), (0, 0));
    handle_ui_event(&mut app, UiEvent::TurnFinished(Ok(result)), None);
    assert!(!app.running);
    assert!(!app.discard_stream_deltas);
    assert!(agent.state.lock().unwrap().messages.iter().all(|message| !matches!(message, Msg::Assistant { content, .. } if content.contains("partial fixture"))));
}

#[tokio::test]
async fn ctrl_c_interrupts_the_real_main_provider_awaiting_headers() {
    cancel_real_stream(false).await;
}

#[tokio::test]
async fn ctrl_c_interrupts_the_real_main_provider_after_a_partial_sse_delta() {
    cancel_real_stream(true).await;
}
