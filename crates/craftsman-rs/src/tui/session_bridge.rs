//! The bridge between the agent session and the UI: events from the agent,
//! approvals, transcript hydration and notifications.

use super::app::{App, BlockState, TranscriptKind, truncate_block_details};
use super::modals::{ApprovalModal, AskModal, Modal};
use crate::agent::{self, ApprovalPromptResult, Session, SessionExtras, ToolActivityEvent};
use crate::config::Config;
use crate::hitl::ApprovalState;
use craftsman_harness::{NotificationKind, TurnResult};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use tokio::sync::mpsc;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug)]
pub(super) enum ApprovalChoice {
    Approve,
    Always,
    Deny(String),
}

pub(super) enum UiEvent {
    ModelRoundStarted,
    Delta(String),
    Activity(String),
    ToolActivity(ToolActivityEvent),
    Approval {
        tool: String,
        summary: String,
        details: String,
        response: std_mpsc::Sender<ApprovalChoice>,
    },
    AskUser {
        question: String,
        response: std_mpsc::Sender<Option<String>>,
    },
    TurnFinished(Result<TurnResult, String>),
    Voice(crate::voice::VoiceEvent),
}

#[derive(Clone)]
pub(super) struct UiBridge {
    pub(super) tx: mpsc::UnboundedSender<UiEvent>,
}

impl UiBridge {
    fn session_extras(&self) -> SessionExtras {
        let round_tx = self.tx.clone();
        let token_tx = self.tx.clone();
        let activity_tx = self.tx.clone();
        let tool_activity_tx = self.tx.clone();
        let approval_tx = self.tx.clone();
        let ask_tx = self.tx.clone();
        SessionExtras {
            #[cfg(feature = "gadgets")]
            local_gadgets: true,
            round_sink: Some(Arc::new(move || {
                let _ = round_tx.send(UiEvent::ModelRoundStarted);
            })),
            token_sink: Some(Arc::new(move |text: &str| {
                let _ = token_tx.send(UiEvent::Delta(text.to_string()));
            })),
            activity_sink: Some(Arc::new(move |line: &str| {
                let _ = activity_tx.send(UiEvent::Activity(line.to_string()));
            })),
            tool_activity_sink: Some(Arc::new(move |event| {
                let _ = tool_activity_tx.send(UiEvent::ToolActivity(event));
            })),
            approval_prompt: Some(Arc::new(move |tool, summary, details| {
                let (response, receive) = std_mpsc::channel();
                if approval_tx
                    .send(UiEvent::Approval {
                        tool: tool.to_string(),
                        summary: summary.to_string(),
                        details: details.to_string(),
                        response,
                    })
                    .is_err()
                {
                    return ApprovalPromptResult::Reject(String::new());
                }
                match tokio::task::block_in_place(|| receive.recv()) {
                    Ok(ApprovalChoice::Approve) => ApprovalPromptResult::Approve,
                    Ok(ApprovalChoice::Always) => ApprovalPromptResult::ApproveAlways,
                    Ok(ApprovalChoice::Deny(feedback)) => ApprovalPromptResult::Reject(feedback),
                    Err(_) => ApprovalPromptResult::Reject(String::new()),
                }
            })),
            ask_user_prompt: Some(Arc::new(move |question| {
                let (response, receive) = std_mpsc::channel();
                ask_tx
                    .send(UiEvent::AskUser {
                        question: question.to_string(),
                        response,
                    })
                    .ok()?;
                tokio::task::block_in_place(|| receive.recv())
                    .ok()
                    .flatten()
            })),
            extra_tools: Vec::new(),
            ..SessionExtras::default()
        }
    }
}

pub(super) struct AgentSlot {
    inner: AgentSlotInner,
    pub(super) bridge: UiBridge,
}

enum AgentSlotInner {
    Pending(tokio::task::JoinHandle<Option<Session>>),
    Ready(Box<Option<Session>>),
}

impl AgentSlot {
    pub(super) fn spawn(
        cfg: &Config,
        approval: Arc<Mutex<ApprovalState>>,
        bridge: UiBridge,
    ) -> Self {
        let cfg = cfg.clone();
        let extras = bridge.session_extras();
        Self {
            inner: AgentSlotInner::Pending(tokio::spawn(async move {
                agent::build_session_ext(&cfg, approval, "", false, false, None, extras).await
            })),
            bridge,
        }
    }

    pub(super) fn rebuild(&mut self, cfg: &Config, approval: Arc<Mutex<ApprovalState>>) {
        if let Some(session) = self.peek() {
            let lsp = session.lsp.clone();
            tokio::spawn(async move {
                lsp.shutdown().await;
            });
        }
        *self = Self::spawn(cfg, approval, self.bridge.clone());
    }

    pub(super) fn is_pending(&self) -> bool {
        matches!(self.inner, AgentSlotInner::Pending(_))
    }

    pub(super) fn peek(&self) -> Option<&Session> {
        match &self.inner {
            AgentSlotInner::Ready(session) => session.as_ref().as_ref(),
            AgentSlotInner::Pending(_) => None,
        }
    }

    pub(super) fn peek_mut(&mut self) -> Option<&mut Session> {
        match &mut self.inner {
            AgentSlotInner::Ready(session) => session.as_mut().as_mut(),
            AgentSlotInner::Pending(_) => None,
        }
    }

    pub(super) async fn get(&mut self) -> &mut Option<Session> {
        if let AgentSlotInner::Pending(handle) = &mut self.inner {
            let session = handle.await.ok().flatten();
            self.inner = AgentSlotInner::Ready(Box::new(session));
        }
        match &mut self.inner {
            AgentSlotInner::Ready(session) => session,
            AgentSlotInner::Pending(_) => unreachable!(),
        }
    }
}

fn handle_ui_event(app: &mut App, event: UiEvent, session: Option<&Session>) {
    match event {
        UiEvent::ModelRoundStarted => {
            app.active_response = None;
            app.dirty = true;
        }
        UiEvent::Delta(delta) => app.append_delta(delta),
        UiEvent::Activity(line) => app.activity(line),
        UiEvent::ToolActivity(event) => match event {
            ToolActivityEvent::Started {
                call_id,
                tool: _,
                summary,
                details,
            } => app.start_tool_block(call_id, summary, details),
            ToolActivityEvent::Finished {
                call_id,
                tool: _,
                content,
                is_error,
            } => app.finish_tool_block(&call_id, content, is_error),
        },
        UiEvent::Approval {
            tool,
            summary,
            details,
            response,
        } => {
            app.active_response = None;
            app.enqueue_modal(Modal::Approval(ApprovalModal {
                tool,
                summary,
                details,
                selected: 0,
                feedback: String::new(),
                feedback_mode: false,
                response,
            }));
        }
        UiEvent::AskUser { question, response } => {
            app.active_response = None;
            app.enqueue_modal(Modal::Ask(AskModal {
                question,
                answer: String::new(),
                response,
            }));
        }
        UiEvent::TurnFinished(result) => app.finish_turn(result, session),
        UiEvent::Voice(event) => match event {
            crate::voice::VoiceEvent::State(state) => {
                app.voice.state = state;
                app.dirty = true;
            }
            crate::voice::VoiceEvent::DownloadProgress { downloaded, total } => {
                app.voice.downloaded = downloaded;
                app.voice.download_total = total;
                app.dirty = true;
            }
            crate::voice::VoiceEvent::Level(level) => {
                app.voice.level = level;
                app.dirty = true;
            }
            crate::voice::VoiceEvent::SpeechStarted => {
                app.voice.segment_active = true;
                app.dirty = true;
            }
            crate::voice::VoiceEvent::Partial {
                committed,
                tentative,
            } if app.voice.active() => {
                app.voice.segment_active = true;
                if let Some(delta) = committed.strip_prefix(&app.voice.committed) {
                    if !delta.is_empty() {
                        app.ensure_voice_leading_separator(delta);
                        app.insert_voice_text(delta);
                    }
                    app.voice.committed = committed;
                }
                app.voice.tentative = tentative;
                app.dirty = true;
            }
            crate::voice::VoiceEvent::Partial { .. } => {}
            crate::voice::VoiceEvent::SegmentFinal {
                text,
                committed,
                partials_enabled,
            } if app.voice.active() => {
                app.voice.segment_active = true;
                if let Some(delta) = committed.strip_prefix(&app.voice.committed) {
                    if !delta.is_empty() {
                        app.ensure_voice_leading_separator(delta);
                        app.insert_voice_text(delta);
                    }
                    app.voice.committed = committed;
                }
                let suffix = text
                    .strip_prefix(&app.voice.committed)
                    .unwrap_or_else(|| final_uncommitted_suffix(&app.voice.committed, &text));
                if !suffix.is_empty() {
                    app.ensure_voice_leading_separator(suffix);
                    app.insert_voice_text(suffix);
                }
                app.voice.partials_enabled = Some(partials_enabled);
                app.voice.finish_segment();
                app.voice.state = crate::voice::VoiceState::Recording;
                app.dirty = true;
            }
            crate::voice::VoiceEvent::SegmentFinal { .. } => {}
            crate::voice::VoiceEvent::SegmentCancelled => {
                if app.voice.active() {
                    app.voice.finish_segment();
                    app.voice.state = crate::voice::VoiceState::Recording;
                    app.dirty = true;
                }
            }
            crate::voice::VoiceEvent::SessionFinished { dictated_text } => {
                let submit_on_finish = app.voice.submit_on_finish;
                let auto_submit = app.voice.auto_submit
                    && crate::voice::dictated_unit_count(&dictated_text)
                        >= app.voice.min_words_to_submit;
                app.ensure_voice_trailing_separator();
                app.voice.finish();
                if (submit_on_finish || auto_submit) && !app.input.trim().is_empty() {
                    app.submit_requested = true;
                }
                app.dirty = true;
            }
            crate::voice::VoiceEvent::Error(error) => {
                let message = error.user_message(&app.voice.model);
                app.voice.finish();
                app.push_error(message);
            }
        },
    }
}

fn final_uncommitted_suffix<'a>(committed: &str, final_text: &'a str) -> &'a str {
    let mut final_boundary = 0usize;
    for ((_, committed_grapheme), (final_index, final_grapheme)) in committed
        .grapheme_indices(true)
        .zip(final_text.grapheme_indices(true))
    {
        if committed_grapheme != final_grapheme {
            break;
        }
        final_boundary = final_index + final_grapheme.len();
    }
    &final_text[final_boundary..]
}

const UI_EVENT_BATCH_LIMIT: usize = 4_096;

pub(super) fn handle_ui_event_batch(
    app: &mut App,
    first: UiEvent,
    receiver: &mut mpsc::UnboundedReceiver<UiEvent>,
    session: Option<&Session>,
) -> usize {
    let mut next = Some(first);
    let mut pending_delta = String::new();
    let mut processed = 0usize;

    while processed < UI_EVENT_BATCH_LIMIT {
        let event = match next.take() {
            Some(event) => event,
            None => match receiver.try_recv() {
                Ok(event) => event,
                Err(_) => break,
            },
        };
        processed += 1;
        match event {
            UiEvent::Delta(delta) => pending_delta.push_str(&delta),
            other => {
                if !pending_delta.is_empty() {
                    app.append_delta(std::mem::take(&mut pending_delta));
                }
                handle_ui_event(app, other, session);
            }
        }
    }
    if !pending_delta.is_empty() {
        app.append_delta(pending_delta);
    }
    processed
}

fn runtime_catalog_notice(content: &str) -> bool {
    content.starts_with("<gadget_catalog_update sha256=\"")
        && content.contains("\nRuntime-discovered gadget metadata, not a user request.")
        || content.starts_with("<gadget_catalog_update>Runtime catalog discovery failed;")
}

pub(super) fn hydrate_transcript(app: &mut App, state: &craftsman_harness::AgentState) {
    use craftsman_harness::Msg;
    app.transcript.clear();
    app.tool_blocks.clear();
    app.active_response = None;
    app.follow_latest();
    app.push("Restored conversation transcript");
    for message in &state.messages {
        match message {
            Msg::System { .. } => {}
            Msg::User { content } if runtime_catalog_notice(content) => app.push_kind(
                TranscriptKind::System,
                "Runtime gadget catalog refreshed".to_owned(),
            ),
            Msg::User { content } => app.push_kind(TranscriptKind::User, content.clone()),
            Msg::Assistant {
                content,
                tool_calls,
            } => {
                if !content.trim().is_empty() {
                    app.push_kind(TranscriptKind::Assistant, content.clone());
                }
                for call in tool_calls {
                    app.start_tool_block(
                        call.id.clone(),
                        crate::tools::summarize_call(&call.name, &call.args),
                        crate::tools::approval_details(&call.name, &call.args),
                    );
                }
            }
            Msg::Tool {
                tool_call_id,
                content,
                is_error,
            } => app.finish_tool_block(tool_call_id, content.clone(), *is_error),
        }
    }
}

pub(super) fn state_as_markdown(state: &craftsman_harness::AgentState) -> String {
    use craftsman_harness::Msg;
    let mut out = String::from("# Craftsman session export\n\n");
    for message in &state.messages {
        match message {
            Msg::User { content } if runtime_catalog_notice(content) => {
                out.push_str(&format!("## Runtime gadget catalog\n\n{content}\n\n"))
            }
            Msg::User { content } => out.push_str(&format!("## You\n\n{content}\n\n")),
            Msg::Assistant { content, .. } if !content.trim().is_empty() => {
                out.push_str(&format!("## Craftsman\n\n{content}\n\n"))
            }
            Msg::Tool {
                content, is_error, ..
            } => out.push_str(&format!(
                "> Tool {}: {}\n\n",
                if *is_error { "error" } else { "result" },
                crate::util::one_line(content, 2_000)
            )),
            _ => {}
        }
    }
    out
}

pub(super) fn drain_notifications(session: &mut Session, app: &mut App) {
    for notification in session.notifications.drain() {
        app.active_response = None;
        match notification.kind {
            NotificationKind::Completed(text) => {
                let failed = session.registry.status(&notification.from)
                    == Some(craftsman_harness::AgentStatus::Failed);
                app.push_collapsible_block(
                    TranscriptKind::Task,
                    format!("background agent '{}' finished", notification.from),
                    truncate_block_details(&text, 30_000),
                    if failed {
                        BlockState::Error
                    } else {
                        BlockState::Done
                    },
                    !failed,
                );
            }
            NotificationKind::Message(message) => {
                app.push_collapsible_block(
                    TranscriptKind::Task,
                    format!("message from '{}'", notification.from),
                    truncate_block_details(&message, 30_000),
                    BlockState::Done,
                    false,
                );
            }
            NotificationKind::NeedsApproval(suspension) => {
                app.push_collapsible_block(
                    TranscriptKind::Task,
                    format!("background agent '{}' needs approval", notification.from),
                    suspension.request.summary,
                    BlockState::Waiting,
                    false,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::input::{VoiceRequest, handle_key};
    use super::super::modals::handle_modal_key;
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    #[test]
    fn ui_event_batch_catches_up_stream_fragments() {
        let mut app = App::new(&Config::default());
        app.running = true;
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(UiEvent::Delta("b".into())).unwrap();
        tx.send(UiEvent::Delta("c".into())).unwrap();
        let processed = handle_ui_event_batch(&mut app, UiEvent::Delta("a".into()), &mut rx, None);
        assert_eq!(processed, 3);
        assert_eq!(
            app.transcript
                .iter()
                .find(|item| item.kind == TranscriptKind::Assistant)
                .map(|item| item.text.as_str()),
            Some("abc")
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn ui_event_batch_preserves_model_round_boundaries() {
        let mut app = App::new(&Config::default());
        app.running = true;
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(UiEvent::ModelRoundStarted).unwrap();
        tx.send(UiEvent::Delta("second".into())).unwrap();
        handle_ui_event_batch(&mut app, UiEvent::Delta("first".into()), &mut rx, None);
        let responses = app
            .transcript
            .iter()
            .filter(|item| item.kind == TranscriptKind::Assistant)
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(responses, vec!["first", "second"]);
    }

    #[test]
    fn voice_partials_are_committed_and_tentative_text_is_virtual() {
        let mut app = App::new(&Config::default());
        app.voice.available = true;
        app.voice.enabled = true;
        app.voice.begin(0);
        app.voice.state = crate::voice::VoiceState::Recording;
        handle_ui_event(
            &mut app,
            UiEvent::Voice(crate::voice::VoiceEvent::Partial {
                committed: "hello ".into(),
                tentative: "wor".into(),
            }),
            None,
        );
        assert_eq!(app.input, "hello ");
        assert_eq!(app.voice.tentative, "wor");
        let (display, cursor, range, _sel) = app.composer_display();
        assert_eq!(display, "hello wor");
        assert_eq!(cursor, "hello ".len());
        assert_eq!(range, Some(("hello ".len(), "hello wor".len())));

        handle_ui_event(
            &mut app,
            UiEvent::Voice(crate::voice::VoiceEvent::SegmentFinal {
                text: "hello world".into(),
                committed: "hello ".into(),
                partials_enabled: true,
            }),
            None,
        );
        assert_eq!(app.input, "hello world");
        assert!(app.voice.active());
        assert_eq!(app.voice.state, crate::voice::VoiceState::Recording);
        assert!(app.voice.tentative.is_empty());

        handle_ui_event(
            &mut app,
            UiEvent::Voice(crate::voice::VoiceEvent::SessionFinished {
                dictated_text: "hello world".into(),
            }),
            None,
        );
        assert!(!app.voice.active());
    }

    #[test]
    fn voice_auto_submit_obeys_dictated_unit_gate_at_session_end() {
        let mut app = App::new(&Config::default());
        app.voice.available = true;
        app.voice.enabled = true;
        app.voice.auto_submit = true;
        app.voice.min_words_to_submit = 3;
        app.voice.begin(0);
        app.voice.state = crate::voice::VoiceState::Finalizing;
        handle_ui_event(
            &mut app,
            UiEvent::Voice(crate::voice::VoiceEvent::SegmentFinal {
                text: "one two".into(),
                committed: String::new(),
                partials_enabled: false,
            }),
            None,
        );
        assert!(!app.submit_requested);
        handle_ui_event(
            &mut app,
            UiEvent::Voice(crate::voice::VoiceEvent::SessionFinished {
                dictated_text: "one two".into(),
            }),
            None,
        );
        assert!(!app.submit_requested);

        app.clear_composer();
        app.voice.begin(0);
        app.voice.state = crate::voice::VoiceState::Finalizing;
        handle_ui_event(
            &mut app,
            UiEvent::Voice(crate::voice::VoiceEvent::SegmentFinal {
                text: "one two three".into(),
                committed: String::new(),
                partials_enabled: false,
            }),
            None,
        );
        handle_ui_event(
            &mut app,
            UiEvent::Voice(crate::voice::VoiceEvent::SessionFinished {
                dictated_text: "one two three".into(),
            }),
            None,
        );
        assert!(app.submit_requested);
    }

    #[test]
    fn continuous_voice_appends_segments_with_spacing_and_enter_submits() {
        let mut app = App::new(&Config::default());
        app.voice.available = true;
        app.voice.enabled = true;
        app.voice.begin(0);
        app.voice.state = crate::voice::VoiceState::Recording;
        for text in ["first sentence", "second sentence"] {
            handle_ui_event(
                &mut app,
                UiEvent::Voice(crate::voice::VoiceEvent::SegmentFinal {
                    text: text.into(),
                    committed: String::new(),
                    partials_enabled: false,
                }),
                None,
            );
        }
        assert_eq!(app.input, "first sentence second sentence");
        assert!(app.voice.active());

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.voice.submit_on_finish);
        assert_eq!(app.voice_request, Some(VoiceRequest::StopAndFinalize));
        handle_ui_event(
            &mut app,
            UiEvent::Voice(crate::voice::VoiceEvent::SessionFinished {
                dictated_text: "first sentence second sentence".into(),
            }),
            None,
        );
        assert!(app.submit_requested);
        assert!(!app.voice.active());
    }

    #[test]
    fn session_hydration_rebuilds_role_aware_transcript() {
        let mut app = App::new(&Config::default());
        let mut state = craftsman_harness::AgentState::default();
        state
            .messages
            .push(craftsman_harness::Msg::user("question"));
        state
            .messages
            .push(craftsman_harness::Msg::assistant("answer"));
        hydrate_transcript(&mut app, &state);
        assert!(
            app.transcript
                .iter()
                .any(|item| item.kind == TranscriptKind::User && item.text == "question")
        );
        assert!(
            app.transcript
                .iter()
                .any(|item| item.kind == TranscriptKind::Assistant && item.text == "answer")
        );
    }

    #[test]
    fn model_round_start_separates_streamed_responses() {
        let mut app = App::new(&Config::default());
        app.running = true;
        app.append_delta("first".into());
        assert!(app.active_response.is_some());
        handle_ui_event(&mut app, UiEvent::ModelRoundStarted, None);
        assert!(app.active_response.is_none());
        app.append_delta("second".into());
        assert_eq!(app.transcript[app.transcript.len() - 2].text, "first");
        assert_eq!(app.transcript.last().unwrap().text, "second");
    }

    #[test]
    fn simultaneous_permission_requests_are_queued_not_replaced() {
        let mut app = App::new(&Config::default());
        let (first_tx, first_rx) = std_mpsc::channel();
        let (second_tx, _second_rx) = std_mpsc::channel();
        handle_ui_event(
            &mut app,
            UiEvent::Approval {
                tool: "execute".into(),
                summary: "first".into(),
                details: "first details".into(),
                response: first_tx,
            },
            None,
        );
        handle_ui_event(
            &mut app,
            UiEvent::Approval {
                tool: "write_file".into(),
                summary: "second".into(),
                details: "second details".into(),
                response: second_tx,
            },
            None,
        );
        assert_eq!(app.modal_queue.len(), 1);
        handle_modal_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(first_rx.recv().unwrap(), ApprovalChoice::Approve));
        let Some(Modal::Approval(modal)) = app.modal.as_ref() else {
            panic!("queued approval should be promoted");
        };
        assert_eq!(modal.summary, "second");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn approval_bridge_delivers_event_and_round_trips_choice() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let prompt = UiBridge { tx }
            .session_extras()
            .approval_prompt
            .expect("approval prompt");
        let waiter =
            tokio::spawn(async move { prompt("execute", "execute rm file", "command: rm file") });
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("approval event timeout")
            .expect("approval event");
        let UiEvent::Approval {
            tool,
            summary,
            details,
            response,
        } = event
        else {
            panic!("expected approval event");
        };
        assert_eq!(tool, "execute");
        assert_eq!(summary, "execute rm file");
        assert_eq!(details, "command: rm file");
        response.send(ApprovalChoice::Approve).unwrap();
        assert_eq!(waiter.await.unwrap(), ApprovalPromptResult::Approve);
    }
}
