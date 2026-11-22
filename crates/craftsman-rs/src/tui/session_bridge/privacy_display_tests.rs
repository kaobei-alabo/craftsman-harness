//! The terminal's own sinks with a selected privacy conversation: the
//! bridge's session extras, the shared builder, the application's event
//! handling and a frame drawn through the safe backend. Typing a turn into
//! such a conversation is not the terminal's yet; the fixture enters it
//! through the conversation's own typed entry.
use super::super::terminal::SafeBackend;
use super::*;
use crate::privacy_session::PrivacyViewSelection;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::widgets::{Paragraph, Wrap};

/// What the terminal showed.
pub(crate) struct Drawn {
    /// The text of every assistant item of the transcript, in order.
    pub(crate) replies: Vec<String>,
    /// The text of every activity item of the transcript, in order.
    pub(crate) notices: Vec<String>,
    /// Every cell of one frame of the transcript, drawn through the safe
    /// backend.
    pub(crate) screen: String,
    /// How many stream deltas reached the application.
    pub(crate) deltas: usize,
}

/// The application and the bridge of one terminal, with a session built
/// from the bridge's own extras for a selected privacy conversation.
pub(crate) struct PrivacyTerminal {
    app: App,
    events: mpsc::UnboundedReceiver<UiEvent>,
    /// The terminal's own slot, holding the session that was built.
    slot: AgentSlot,
    deltas: usize,
}

impl PrivacyTerminal {
    pub(crate) async fn build(
        cfg: &Config,
        directory: &std::path::Path,
        selection: PrivacyViewSelection,
    ) -> Self {
        let (tx, events) = mpsc::unbounded_channel();
        let bridge = UiBridge { tx };
        let root_cancellation = craftsman_harness::SessionCancellation::default();
        let cancellation = root_cancellation.child().unwrap();
        // The terminal's own sinks and prompts. The parts a privacy
        // conversation still refuses are left out, as a host that selects
        // one has to.
        let mut extras = bridge.session_extras();
        extras.privacy_view = Some(selection);
        extras.workspace_root = Some(directory.to_path_buf());
        extras.verification = false;
        extras.context_compilation = false;
        extras.failure_controls = false;
        #[cfg(feature = "gadgets")]
        {
            extras.local_gadgets = false;
        }
        let session = agent::build_session_ext(
            cfg,
            Arc::new(Mutex::new(ApprovalState::default())),
            "",
            false,
            false,
            None,
            extras,
        )
        .await
        .expect("the terminal's extras build a privacy conversation");
        Self {
            app: App::new(cfg),
            events,
            slot: AgentSlot {
                inner: AgentSlotInner::Ready(Box::new(Some(session))),
                bridge,
                root_cancellation,
                cancellation,
                retired: tokio::task::JoinSet::new(),
            },
            deltas: 0,
        }
    }

    pub(crate) fn session(&self) -> &Session {
        self.slot.peek().unwrap()
    }

    /// One turn a person typed, with every event it sent handled as the
    /// terminal's loop handles it. `stopping` is the state the terminal is
    /// in after a person asked to stop the turn. Returns the turn's final
    /// text as the agent recorded it, or the text of its failure.
    pub(crate) async fn turn(&mut self, typed: &str, stopping: bool) -> Result<String, String> {
        self.app.running = true;
        self.app.discard_stream_deltas = stopping;
        let session = self.slot.peek().unwrap();
        let view = session.privacy_view.as_ref().unwrap();
        let result = view.turn(&session.agent, typed).await;
        while let Ok(event) = self.events.try_recv() {
            if matches!(event, UiEvent::Delta(_)) {
                self.deltas += 1;
            }
            handle_ui_event(&mut self.app, event, Some(session));
        }
        let (finished, outcome) = match result {
            Ok(TurnResult::Done(output)) => {
                let text = output.final_text().to_owned();
                (Ok(TurnResult::Done(output)), Ok(text))
            }
            Ok(other) => (Ok(other), Ok(String::new())),
            Err(error) => (Err(error.to_string()), Err(error.to_string())),
        };
        self.app.finish_turn(finished, None);
        outcome
    }

    /// One reply event as the bridge's sink sends it, during a turn.
    pub(crate) fn show(&mut self, reply: crate::privacy_session::HumanReply) {
        self.app.running = true;
        self.app.discard_stream_deltas = false;
        handle_ui_event(&mut self.app, UiEvent::HumanReply(reply), self.slot.peek());
        self.app.running = false;
    }

    /// Draws one frame on the process's own terminal as the terminal's loop
    /// does: inside the terminal guard, with the status reporter on, through
    /// the safe backend over Crossterm, by the application's own draw.
    #[cfg(unix)]
    pub(crate) fn draw_on_terminal(&mut self, cfg: &Config) -> anyhow::Result<()> {
        use super::super::program_status::Status;
        use super::super::terminal::TerminalGuard;
        let guard = TerminalGuard::enter(false)?;
        guard.enable_program_status(true);
        guard.report_program_status(Status::Working, None, false)?;
        let mut terminal = Terminal::new(SafeBackend(ratatui::backend::CrosstermBackend::new(
            std::io::stdout(),
        )))?;
        terminal.clear()?;
        let approval = Arc::new(Mutex::new(ApprovalState::default()));
        super::super::render::draw(&mut terminal, &mut self.app, cfg, &self.slot, &approval)?;
        guard.report_program_status(Status::Done, None, false)?;
        drop(terminal);
        drop(guard);
        Ok(())
    }

    pub(crate) fn drawn(&self) -> Drawn {
        let of = |kind: TranscriptKind| -> Vec<String> {
            let mut texts = Vec::new();
            for item in &self.app.transcript {
                if item.kind == kind {
                    texts.push(item.text.clone());
                }
            }
            texts
        };
        let mut terminal = Terminal::new(SafeBackend(TestBackend::new(120, 40))).unwrap();
        terminal
            .draw(|frame| {
                let text = super::super::render::render_transcript(&self.app.transcript, 118);
                frame.render_widget(
                    Paragraph::new(text).wrap(Wrap { trim: false }),
                    frame.area(),
                );
            })
            .unwrap();
        let mut screen = String::new();
        for cell in terminal.backend().0.buffer().content() {
            screen.push_str(cell.symbol());
        }
        Drawn {
            replies: of(TranscriptKind::Assistant),
            notices: of(TranscriptKind::Activity),
            screen,
            deltas: self.deltas,
        }
    }
}
