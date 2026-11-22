//! A fresh private terminal's real receiver/input/task owner. This is never
//! called by a normal selector. Password acquisition has finished before this
//! sole ordinary reader exists; active-entry password reacquisition is refused.
use super::{OwnedSecretTerminal, PrivateTerminalError, UnixPopup};
use crate::config::Config;
use crate::hitl::ApprovalState;
use crate::privacy_bootstrap::{
    BootstrapError, PrivateBuiltSession, PrivateConversation, PrivateTurnError,
};
use crate::privacy_entry::{PrivacyApprovalPrompt, PrivacyApprovalUi, PrivateEntryError};
use crate::tui::{
    app::App,
    input::handle_terminal_event,
    modals::AppAction,
    privacy_approval,
    render::draw_session,
    session_bridge::{UiBridge, UiEvent, apply_turn_cancellation},
    terminal::copy_to_clipboard,
};
use crossterm::event::{Event, KeyCode, KeyEventKind};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinSet};

const EVENT_BATCH: usize = 32;

/// Private sole normal reader; no EventStream wake thread is created. Its
/// poll/read calls run synchronously in the owning task and finish before the
/// task can transfer input to a secret popup. Ordinary TUI stays unchanged.
struct OwnedNormalInput;
impl OwnedNormalInput {
    async fn next(&mut self) -> Option<std::io::Result<Event>> {
        loop {
            match crossterm::event::poll(Duration::ZERO) {
                Ok(true) => return Some(crossterm::event::read()),
                Ok(false) => tokio::time::sleep(Duration::from_millis(10)).await,
                Err(error) => return Some(Err(error)),
            }
        }
    }
}
async fn next_normal(input: &mut Option<OwnedNormalInput>) -> Option<std::io::Result<Event>> {
    match input {
        Some(input) => input.next().await,
        None => std::future::pending().await,
    }
}

enum Returned {
    Turn(Result<craftsman_harness::TurnResult, PrivateTurnError>),
    Steer(String, Result<bool, PrivateTurnError>),
    Decision(Result<Option<craftsman_harness::TurnResult>, PrivateTurnError>),
}

pub(super) struct Frontend {
    popup: OwnedSecretTerminal,
    owner: Arc<PrivateConversation>,
    cfg: Config,
    approval: Arc<Mutex<ApprovalState>>,
    app: App,
    bridge: UiBridge,
    receiver: mpsc::UnboundedReceiver<UiEvent>,
    prompt: Arc<PrivacyApprovalPrompt>,
    ui: Arc<PrivacyApprovalUi>,
    cancellation: craftsman_harness::SessionCancellation,
    input: Option<OwnedNormalInput>,
    built: Option<PrivateBuiltSession>,
    work: JoinSet<Returned>,
    queued_blocked: bool,
}
impl Frontend {
    pub(super) fn from_popup(
        mut popup: UnixPopup,
        owner: Arc<PrivateConversation>,
        cfg: Config,
        approval: Arc<Mutex<ApprovalState>>,
    ) -> Result<Self, PrivateTerminalError> {
        let result = (|| {
            owner
                .check_sources()
                .map_err(PrivateTerminalError::Source)?;
            popup.check().map_err(|_| PrivateTerminalError::Terminal)?;
            if !popup.secret_reader {
                return Err(PrivateTerminalError::Terminal);
            }
            // Voice has no reviewed private acquisition adapter. Refuse the
            // configured feature; never turn it off to get a smaller session.
            if cfg
                .voice_setting("enabled")
                .and_then(serde_json::Value::as_bool)
                != Some(false)
            {
                return Err(PrivateTerminalError::Build(
                    crate::privacy_bootstrap::PrivateBuildError::Refused(
                        crate::privacy_session::BuildRefusal::RequestedPart,
                    ),
                ));
            }
            popup
                .input
                .finish_input()
                .map_err(|_| PrivateTerminalError::Terminal)?;
            let cancellation = popup
                .shutdown
                .cancellation
                .child()
                .map_err(|_| PrivateTerminalError::Cancelled)?;
            let (tx, receiver) = mpsc::unbounded_channel();
            let bridge = UiBridge { tx };
            let (prompt, ui) = privacy_approval::register(&bridge);
            // This is the one fresh input handoff. No password input future or
            // owned decoder fragment survives it, and Drop may not subsequently
            // flush the ordinary reader's accepted drafts/type-ahead.
            popup.secret_reader = false;
            let input = OwnedNormalInput;
            Ok((cancellation, receiver, bridge, prompt, ui, input))
        })();
        let (cancellation, receiver, bridge, prompt, ui, input) = match result {
            Ok(parts) => parts,
            Err(error) => {
                owner.close();
                return Err(error);
            }
        };
        Ok(Self {
            popup: OwnedSecretTerminal { inner: popup },
            owner,
            app: App::new(&cfg),
            cfg,
            approval,
            bridge,
            receiver,
            prompt,
            ui,
            cancellation,
            input: Some(input),
            built: None,
            work: JoinSet::new(),
            queued_blocked: false,
        })
    }

    pub(super) async fn build_full(&mut self) -> Result<(), PrivateTerminalError> {
        self.owner
            .check_sources()
            .map_err(PrivateTerminalError::Source)?;
        let cfg = self.cfg.clone();
        let approval = self.approval.clone();
        let owner = self.owner.clone();
        let mut extras = self.bridge.session_extras();
        extras.cancellation_session = Some(self.cancellation.clone());
        extras.privacy_approval_prompt = Some(self.prompt.clone());
        // All original terminal extras remain requested. The actual consumer
        // owns their receiver and pumps prompts through the entire build.
        let mut build: Pin<
            Box<
                dyn Future<
                        Output = Result<
                            PrivateBuiltSession,
                            crate::privacy_bootstrap::PrivateBuildError,
                        >,
                    > + Send
                    + '_,
            >,
        > = Box::pin(async move { owner.request_full_session(&cfg, approval, extras).await });
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.app.running = true;
        loop {
            tokio::select! {
                biased;
                result = &mut build => {
                    self.built = Some(result.map_err(PrivateTerminalError::Build)?);
                    self.app.running = false;
                    self.app.dirty = true;
                    return Ok(());
                }
                _ = self.popup.inner.shutdown.cancellation.cancelled() => return Err(PrivateTerminalError::Cancelled),
                Some(event) = self.receiver.recv() => self.events(event),
                event = next_normal(&mut self.input) => {
                    let Some(Ok(event)) = event else { return Err(PrivateTerminalError::Terminal); };
                    if self.input_event(event) || self.app.cancel_requested {
                        self.cancellation.cancel();
                        self.app.cancel_pending_prompts();
                        return Err(PrivateTerminalError::Cancelled);
                    }
                    self.settings();
                }
                _ = tick.tick() => self.draw(true)?,
            }
        }
    }

    fn session(&self) -> Option<&crate::agent::Session> {
        self.built
            .as_ref()
            .map(|built| built.session_for_terminal())
    }
    fn draw(&mut self, pending: bool) -> Result<(), PrivateTerminalError> {
        let session = self
            .built
            .as_ref()
            .map(|built| built.session_for_terminal());
        draw_session(
            &mut self.popup.inner.terminal,
            &mut self.app,
            &self.cfg,
            session,
            pending,
            &self.approval,
        )
        .map_err(|_| PrivateTerminalError::Terminal)
    }
    fn events(&mut self, first: UiEvent) {
        let mut next = Some(first);
        // A new private 32-event batch; ordinary 4,096-event batching is intact.
        // Source/transport/review bounds do not imply a global queue-size cap.
        for _ in 0..EVENT_BATCH {
            let event = match next.take() {
                Some(event) => event,
                None => match self.receiver.try_recv() {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };
            let session = self
                .built
                .as_ref()
                .map(|built| built.session_for_terminal());
            privacy_approval::handle(&mut self.app, event, &self.ui, session);
        }
    }
    fn input_event(&mut self, event: Event) -> bool {
        if matches!(&event, Event::Key(key) if key.kind == KeyEventKind::Press
            && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('g' | 'G')))
        {
            self.app
                .push_error("Voice input has no reviewed private adapter in this entry.");
            return false;
        }
        // Raw human edits/answers and terminal drafts retain their own limits.
        // A refused whole paste/key leaves the existing draft untouched.
        let extra = match &event {
            Event::Paste(text) => Some(text.len()),
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && !key.modifiers.intersects(
                        crossterm::event::KeyModifiers::CONTROL
                            | crossterm::event::KeyModifiers::ALT
                            | crossterm::event::KeyModifiers::SUPER,
                    ) =>
            {
                match key.code {
                    KeyCode::Char(character) => Some(character.len_utf8()),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(extra) = extra {
            let current = match &self.app.modal {
                Some(crate::tui::modals::Modal::Ask(modal)) => Some(modal.answer.len()),
                None => Some(self.app.input.len()),
                _ => None,
            };
            if current.is_some_and(|current| {
                current
                    .checked_add(extra)
                    .is_none_or(|size| size > crate::privacy_hook::MAX_TEXT_BYTES)
            }) {
                self.app.push_error("This whole private input exceeds its byte bound; the earlier draft is retained.");
                return false;
            }
        }
        handle_terminal_event(&mut self.app, event)
    }
    fn settings(&mut self) {
        let saving = std::mem::take(&mut self.app.mode_cycle_requested);
        let action = self.app.pending_action.take();
        if saving || action.is_some() {
            // Before any permission/model/effort/setup configuration mutation.
            // The ordinary terminal's save and transfer paths remain untouched.
            if matches!(action, Some(AppAction::RemoveQueued(_))) {
                if let Some(AppAction::RemoveQueued(index)) = action {
                    self.app.queued_messages.remove(index);
                }
            } else {
                let result = self.owner.before_configuration_save();
                self.app.push_error(match result {
                    Err(error) => error.to_string(),
                    Ok(()) => BootstrapError::SettingsSaveRefused.to_string(),
                });
            }
        }
        if self.app.voice_request.take().is_some() {
            self.app
                .push_error("Voice input has no reviewed private adapter in this entry.");
        }
    }
    fn submission(&mut self) -> Option<(String, bool)> {
        if !std::mem::take(&mut self.app.submit_requested) {
            return None;
        }
        let queue = std::mem::take(&mut self.app.queue_submission_requested);
        if self.app.input.trim().is_empty() {
            return None;
        }
        // Unlike the normal command router, this typed entry never trims or
        // expands the accepted human source or sends it to a shell escape.
        let typed = std::mem::take(&mut self.app.input);
        self.app.cursor = 0;
        self.app.selection_anchor = None;
        self.app.history_index = None;
        self.app.history_draft = None;
        self.app.dirty = true;
        Some((typed, queue))
    }
    fn start(&mut self, typed: String, queue: bool) -> bool {
        if matches!(typed.trim(), "/quit" | "/exit") {
            return true;
        }
        if typed.trim() == "/clear" {
            self.app.push_error("In-place private clear requires a fresh exclusive unlock and is unsupported; the current conversation is retained.");
            return false;
        }
        if typed.starts_with('/') || typed.starts_with('!') {
            // No normal settings/save/reopen/transfer/shell/extension adapter is
            // silently routed around the selected entry's actual source paths.
            self.app.push_error(
                "This command has no reviewed private entry adapter; it was not executed.",
            );
            self.app.queue_follow_up(typed);
            self.queued_blocked = true;
            return false;
        }
        if queue && self.app.running {
            self.app.queue_follow_up(typed);
            return false;
        }
        let Some(built) = self.built.as_ref() else {
            self.app.queue_follow_up(typed);
            return false;
        };
        let parked = built
            .session_for_terminal()
            .agent
            .pending_suspension()
            .is_some();
        let task = match built.task_owner() {
            Ok(task) => task,
            Err(error) => {
                self.app.queue_follow_up(typed);
                self.queued_blocked = true;
                self.app.push_error(error.to_string());
                return false;
            }
        };
        if self.app.running {
            let source = typed.clone();
            let future: Pin<Box<dyn Future<Output = Returned> + Send>> =
                Box::pin(async move { Returned::Steer(source, task.steer(&typed).await) });
            self.work.spawn(future);
        } else if parked {
            let future: Pin<Box<dyn Future<Output = Returned> + Send>> =
                Box::pin(async move { Returned::Decision(task.request_decision(&typed).await) });
            self.app.running = true;
            self.work.spawn(future);
        } else {
            self.app.push_user(typed.clone());
            self.app.running = true;
            self.app.discard_stream_deltas = false;
            self.app.active_response = None;
            let future: Pin<Box<dyn Future<Output = Returned> + Send>> =
                Box::pin(async move { Returned::Turn(task.turn(&typed).await) });
            self.work.spawn(future);
        }
        false
    }
    fn returned(&mut self, returned: Returned) {
        match returned {
            Returned::Steer(typed, Ok(false)) => self.app.queue_follow_up(typed),
            Returned::Steer(_, Ok(true)) => self
                .app
                .push("The typed input was accepted into the running private turn."),
            Returned::Steer(typed, Err(error)) => {
                self.app.queue_follow_up(typed);
                self.queued_blocked = true;
                self.app.push_error(error.to_string());
            }
            Returned::Decision(Ok(None)) => {
                self.app.running = false;
                self.app.program_status = crate::tui::program_status::Status::Permission;
                self.app.push("No private decision was accepted.");
            }
            Returned::Decision(Ok(Some(result))) => self.finished(Ok(result)),
            Returned::Decision(Err(error)) => self.finished(Err(error)),
            Returned::Turn(result) => self.finished(result),
        }
    }
    fn finished(&mut self, result: Result<craftsman_harness::TurnResult, PrivateTurnError>) {
        let event = match result {
            Ok(result) => UiEvent::turn_result(Ok(result)),
            Err(PrivateTurnError::Entry(PrivateEntryError::Agent(error))) => {
                UiEvent::turn_result_with_controls(
                    Err(error),
                    self.session()
                        .and_then(|session| session.controls.as_deref()),
                )
            }
            Err(error) => UiEvent::TurnFinished(Err(error.to_string())),
        };
        // The committed reply and other actual turn events were sent before
        // the task returned. Complete on that SAME FIFO, so the UI cannot
        // become idle and discard a reply that it has not consumed yet.
        // Retirement closes the receiver before awaiting tasks; keep their
        // actual returned errors/results directly in that cleanup path.
        if self.receiver.is_closed() {
            self.events(event);
        } else if let Err(returned) = self.bridge.tx.send(event) {
            self.events(returned.0);
        }
    }

    pub(super) async fn run(&mut self) -> Result<i32, PrivateTerminalError> {
        let result = self.loop_inner().await;
        let cleanup = self.retire_owners().await;
        if !matches!(cleanup, Ok(true)) {
            return Err(match result {
                Err(primary) => PrivateTerminalError::CleanupAfter(Box::new(primary)),
                Ok(_) => PrivateTerminalError::Cleanup,
            });
        }
        let cursor = self.popup.inner.terminal.show_cursor();
        match result {
            Err(primary) => {
                if cursor.is_err() {
                    Err(PrivateTerminalError::CleanupAfter(Box::new(primary)))
                } else {
                    Err(primary)
                }
            }
            Ok(code) => {
                cursor.map_err(|_| PrivateTerminalError::Terminal)?;
                Ok(code)
            }
        }
    }
    /// Actual cleanup half shared by exit and the pending fresh-clear path.
    /// No reader relinquishment or unlock is implied by its return.
    async fn retire_owners(&mut self) -> Result<bool, tokio::time::error::Elapsed> {
        // Close real sampling/dispatch owners BEFORE any queued prompt wakes.
        self.owner.close();
        self.cancellation.cancel();
        if let Some(session) = self.session() {
            session.agent.request_cancel();
        }
        self.app.closing = true;
        self.receiver.close();
        self.app.cancel_pending_prompts();
        while let Ok(event) = self.receiver.try_recv() {
            self.events(event);
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut failed = false;
            while let Some(returned) = self.work.join_next().await {
                match returned {
                    Ok(returned) => self.returned(returned),
                    Err(_) => failed = true,
                }
            }
            if let Some(session) = self.session() {
                for join in session.registry.take_background_joins() {
                    failed |= join.await.is_err();
                }
                session.lsp.shutdown().await;
            }
            !failed
        })
        .await
    }
    async fn fresh_clear(&mut self) -> Result<(), PrivateTerminalError> {
        // The actual read future has returned/dropped at the select boundary.
        // Move its sole ownership out before cancelling/draining any old work;
        // no normal read can run during the subsequent requested password.
        let input = self
            .input
            .take()
            .ok_or(PrivateTerminalError::InputHandoff)?;
        let old = self.owner.clone();
        let old_id = old.thread_id().to_owned();
        let old_guard = old.session_guard_for_retirement();
        let cleanup = self.retire_owners().await;
        if !matches!(cleanup, Ok(true))
            || self
                .session()
                .is_some_and(|session| session.agent.checkpoint_failed())
        {
            return Err(PrivateTerminalError::Cleanup);
        }
        if old_guard.check().is_ok() || old.check_sources().is_ok() {
            return Err(PrivateTerminalError::InputHandoff);
        }
        // Retain decoded ordinary type-ahead before secret entry. These reads
        // still belong to the moved NORMAL owner, before a password is asked.
        // Bound this handoff; overflow refuses instead of dropping draft data.
        let mut retained = 0;
        while crossterm::event::poll(Duration::ZERO)
            .map_err(|_| PrivateTerminalError::InputHandoff)?
        {
            if retained >= EVENT_BATCH {
                return Err(PrivateTerminalError::InputHandoff);
            }
            let event = crossterm::event::read().map_err(|_| PrivateTerminalError::InputHandoff)?;
            if matches!(&event, Event::Key(key) if key.kind == KeyEventKind::Press
                && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c' | 'C')))
            {
                return Err(PrivateTerminalError::Cancelled);
            }
            if self.input_event(event) {
                return Err(PrivateTerminalError::Cancelled);
            }
            retained += 1;
        }
        drop(input);
        self.built = None; // no closed lane/client/view is used for the rebuild
        let bootstrap = old
            .capture_after_retirement()
            .map_err(PrivateTerminalError::Source)?;
        self.popup.inner.secret_reader = true;
        self.popup
            .inner
            .input
            .finish_input()
            .map_err(|_| PrivateTerminalError::InputHandoff)?;
        let fresh = Arc::new(
            bootstrap
                .unlock_private(&mut self.popup)
                .await
                .map_err(PrivateTerminalError::Source)?,
        );
        if fresh.thread_id() == old_id {
            fresh.close();
            return Err(PrivateTerminalError::InputHandoff);
        }
        fresh
            .check_sources()
            .map_err(PrivateTerminalError::Source)?;
        let cancellation = self
            .popup
            .inner
            .shutdown
            .cancellation
            .child()
            .map_err(|_| PrivateTerminalError::Cancelled)?;
        let (tx, receiver) = mpsc::unbounded_channel();
        let bridge = UiBridge { tx };
        let (prompt, ui) = privacy_approval::register(&bridge);
        self.owner = fresh;
        self.cancellation = cancellation;
        self.bridge = bridge;
        self.receiver = receiver;
        self.prompt = prompt;
        self.ui = ui;
        self.app.closing = false;
        self.app.running = false;
        self.app.cancel_requested = false;
        self.app.dirty = true;
        self.popup.inner.secret_reader = false;
        self.input = Some(OwnedNormalInput);
        self.build_full().await?;
        Ok(())
    }
    async fn loop_inner(&mut self) -> Result<i32, PrivateTerminalError> {
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            self.owner
                .check_sources()
                .map_err(PrivateTerminalError::Source)?;
            tokio::select! {
                biased;
                _ = self.popup.inner.shutdown.cancellation.cancelled() => return Ok(self.popup.inner.shutdown.exit_code()),
                Some(returned) = self.work.join_next() => match returned {
                    Ok(returned) => self.returned(returned),
                    Err(_) => return Err(PrivateTerminalError::Terminal),
                },
                Some(event) = self.receiver.recv() => self.events(event),
                event = next_normal(&mut self.input) => {
                    let Some(Ok(event)) = event else { return Err(PrivateTerminalError::Terminal); };
                    if self.input_event(event) { return Ok(0); }
                }
                _ = tick.tick() => self.draw(false)?,
            }
            let agent = self
                .built
                .as_ref()
                .map(|built| &built.session_for_terminal().agent);
            apply_turn_cancellation(&mut self.app, agent);
            self.settings();
            if std::mem::take(&mut self.app.copy_requested) {
                let last = self
                    .app
                    .transcript
                    .iter()
                    .rev()
                    .find(|entry| entry.kind == crate::tui::app::TranscriptKind::Assistant);
                if let Some(last) = last {
                    if copy_to_clipboard(&last.text).is_err() {
                        self.app.push_error("The private response was not copied.");
                    }
                }
            }
            if self.app.modal.is_none() {
                let submitted = self.submission();
                if submitted.is_some() {
                    self.queued_blocked = false;
                }
                let next = submitted.or_else(|| {
                    (!self.app.running && !self.queued_blocked)
                        .then(|| self.app.pop_queued_follow_up().map(|typed| (typed, false)))
                        .flatten()
                });
                if let Some((typed, queue)) = next {
                    if typed.trim() == "/clear" {
                        self.fresh_clear().await?;
                        continue;
                    }
                    if self.start(typed, queue) {
                        return Ok(0);
                    }
                }
            }
            if let Some(built) = self.built.as_mut() {
                // This is the original real receiver, drained even while the
                // foreground task retains the same view/agent/policy owners.
                crate::tui::session_bridge::drain_notifications(
                    built.session_for_terminal_mut(),
                    &mut self.app,
                );
            }
        }
    }
}
impl Drop for Frontend {
    fn drop(&mut self) {
        self.owner.close();
        self.cancellation.cancel();
        if let Some(session) = self.session() {
            session.agent.request_cancel();
        }
        self.app.closing = true;
        self.receiver.close();
        self.app.cancel_pending_prompts();
        // JoinSet aborts only owned async waiters. It does not claim interrupted
        // native/blocking work or completed cleanup when this future is dropped.
    }
}
