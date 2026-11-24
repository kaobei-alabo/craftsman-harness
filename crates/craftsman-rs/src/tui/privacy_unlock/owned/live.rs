//! A fresh private terminal's real receiver/input/task owner. This is never
//! called by a normal selector. Password acquisition has finished before this
//! sole ordinary reader exists; active-entry password reacquisition is refused.
use super::{
    FrontendConfiguration, OwnedSecretTerminal, PrivateTerminalError, UnixPopup,
    check_voice_configuration,
};
use crate::hitl::ApprovalState;
use crate::privacy_bootstrap::{
    BootstrapError, PrivateBuildError, PrivateBuiltSession, PrivateConversation, PrivateTurnError,
};
use crate::privacy_entry::{PrivacyApprovalPrompt, PrivacyApprovalUi, PrivateEntryError};
use crate::tui::{
    app::App,
    input::handle_terminal_event,
    modals::AppAction,
    privacy_approval,
    program_status::Status,
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

/// The frontend's actual full-session construction, as it was requested.
type Construction =
    Pin<Box<dyn Future<Output = Result<PrivateBuiltSession, PrivateBuildError>> + Send>>;

/// Polls work where its owner keeps it. Its return empties the slot. A wait
/// that is dropped first leaves the pending work in the slot, untouched.
async fn retained<T>(slot: &mut Option<Pin<Box<dyn Future<Output = T> + Send>>>) -> T {
    match slot {
        Some(work) => {
            let returned = work.as_mut().await;
            *slot = None;
            returned
        }
        None => std::future::pending().await,
    }
}

/// A refused construction whose own cleanup did not finish keeps its refusal
/// inside the terminal's existing cleanup report.
fn build_error(error: PrivateBuildError) -> PrivateTerminalError {
    match error {
        PrivateBuildError::CleanupAfter(refusal) => {
            PrivateTerminalError::CleanupAfter(Box::new(PrivateTerminalError::Build(*refusal)))
        }
        error => PrivateTerminalError::Build(error),
    }
}

enum Returned {
    Turn(Result<craftsman_harness::TurnResult, PrivateTurnError>),
    Steer(String, Result<bool, PrivateTurnError>),
    Decision(Result<Option<craftsman_harness::TurnResult>, PrivateTurnError>),
    /// A read-only listing's text, or its error line.
    Listing(Result<String, String>),
}

pub(super) struct Frontend {
    popup: OwnedSecretTerminal,
    owner: Arc<PrivateConversation>,
    configuration: FrontendConfiguration,
    approval: Arc<Mutex<ApprovalState>>,
    app: App,
    bridge: UiBridge,
    receiver: mpsc::UnboundedReceiver<UiEvent>,
    prompt: Arc<PrivacyApprovalPrompt>,
    ui: Arc<PrivacyApprovalUi>,
    cancellation: craftsman_harness::SessionCancellation,
    input: Option<OwnedNormalInput>,
    built: Option<PrivateBuiltSession>,
    /// The construction while it has not returned. The captured route keeps
    /// it here through a failed build, for its retirement to await.
    construction: Option<Construction>,
    work: JoinSet<Returned>,
    queued_blocked: bool,
}
impl Frontend {
    pub(super) fn from_popup(
        mut popup: UnixPopup,
        owner: Arc<PrivateConversation>,
        configuration: FrontendConfiguration,
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
            let cfg = configuration.source(&owner)?;
            check_voice_configuration(cfg)?;
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
            let app = App::new(cfg);
            // The ordinary terminal's fixed-text OSC 7501 status, for a
            // startup the router selected and unless the user turned it off.
            if let Ok(Some(requirements)) = owner.startup_requirements()
                && requirements.normal_routing_selected()
                && crate::tui::program_status::enabled(cfg) == Ok(true)
            {
                popup.guard.enable_program_status(true);
            }
            Ok((cancellation, receiver, bridge, prompt, ui, input, app))
        })();
        let (cancellation, receiver, bridge, prompt, ui, input, app) = match result {
            Ok(parts) => parts,
            Err(error) => {
                owner.close();
                return Err(error);
            }
        };
        Ok(Self {
            popup: OwnedSecretTerminal { inner: popup },
            owner,
            app,
            configuration,
            approval,
            bridge,
            receiver,
            prompt,
            ui,
            cancellation,
            input: Some(input),
            built: None,
            construction: None,
            work: JoinSet::new(),
            queued_blocked: false,
        })
    }

    pub(super) async fn build_full(&mut self) -> Result<(), PrivateTerminalError> {
        // The password prompt has ended and the ordinary reader owns input.
        // A startup that the router selected now gets the mouse capture its
        // actual invocation chose; the private entry keeps none.
        if let Ok(Some(requirements)) = self.owner.startup_requirements()
            && requirements.normal_routing_selected()
            && let Some(options) = requirements.terminal_options()
            && options.mouse_capture()
            && self.popup.inner.guard.capture_mouse(true).is_err()
        {
            return Err(PrivateTerminalError::Terminal);
        }
        let result = self.construct().await;
        if !self.configuration.is_captured() {
            // The legacy route drops a construction that has not returned
            // here, exactly as its local future was dropped.
            self.construction = None;
        }
        result
    }

    async fn construct(&mut self) -> Result<(), PrivateTerminalError> {
        self.owner
            .check_sources()
            .map_err(PrivateTerminalError::Source)?;
        let legacy_cfg = self.configuration.legacy_for_build();
        let approval = self.approval.clone();
        let owner = self.owner.clone();
        let mut extras = self.bridge.session_extras();
        extras.cancellation_session = Some(self.cancellation.clone());
        extras.privacy_approval_prompt = Some(self.prompt.clone());
        // All original terminal extras remain requested. The actual consumer
        // owns their receiver and pumps prompts through the entire build.
        // The construction is kept in the frontend, not in this function: an
        // early return below leaves it pending there.
        let construction: Construction = Box::pin(async move {
            match legacy_cfg {
                Some(cfg) => owner.request_full_session(&cfg, approval, extras).await,
                None => owner.request_captured_full_session(approval, extras).await,
            }
        });
        self.construction = Some(construction);
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.app.running = true;
        loop {
            tokio::select! {
                biased;
                result = retained(&mut self.construction) => {
                    self.built = Some(result.map_err(build_error)?);
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
        let owner = self.owner.clone();
        let cfg = self.configuration.source(&owner)?;
        let session = self
            .built
            .as_ref()
            .map(|built| built.session_for_terminal());
        // Change-only, fixed states and messages; nothing is written when the
        // reporter is off. No name, prompt or model text enters it.
        let registry = match session {
            Some(session) => Some(session.registry.as_ref()),
            None => None,
        };
        if self
            .popup
            .inner
            .guard
            .report_program_status(self.app.current_program_status(pending), registry, pending)
            .is_err()
        {
            return Err(PrivateTerminalError::Terminal);
        }
        draw_session(
            &mut self.popup.inner.terminal,
            &mut self.app,
            cfg,
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
    fn refuse_save(&mut self) {
        let result = self.owner.before_configuration_save();
        self.app.push_error(match result {
            Err(error) => error.to_string(),
            Ok(()) => BootstrapError::SettingsSaveRefused.to_string(),
        });
    }
    fn settings(&mut self) {
        let saving = std::mem::take(&mut self.app.mode_cycle_requested);
        let action = self.app.pending_action.take();
        match action {
            // The user's own queued drafts: a local list, as in the ordinary terminal.
            Some(AppAction::RemoveQueued(index)) => {
                self.app.queued_messages.remove(index);
            }
            // A running background task of this session, cancelled for real.
            Some(AppAction::CancelTask(name)) => self.cancel_task(&name),
            // Not saves: a picker and a reopen. Neither has a private
            // adapter; nothing was changed.
            Some(
                AppAction::ChooseModel(_)
                | AppAction::EditProvider(_)
                | AppAction::ResumeSession(_),
            ) => self.app.push_error(
                "This control has no reviewed private entry adapter; it was not executed.",
            ),
            // Everything else would change the captured settings: refused
            // before any permission, model, effort or setup mutation. The
            // ordinary terminal's save and transfer paths remain untouched.
            Some(_) => self.refuse_save(),
            None if saving => self.refuse_save(),
            None => {}
        }
        if self.app.voice_request.take().is_some() {
            self.app
                .push_error("Voice input has no reviewed private adapter in this entry.");
        }
    }
    /// The ordinary terminal's task cancellation, on this session's own
    /// registry and only through its bound owner. The lines are the ordinary
    /// ones; a refused owner says why.
    fn cancel_task(&mut self, name: &str) {
        let aborted = match self.built.as_ref() {
            Some(built) => built.abort_background(name),
            None => Ok(false),
        };
        match aborted {
            Ok(true) => self.app.push_kind(
                crate::tui::app::TranscriptKind::Task,
                format!("Cancelled task {name}"),
            ),
            Ok(false) => self.app.push_error(format!(
                "Task {name} is not a cancellable running background task"
            )),
            Err(error) => self.app.push_error(error.to_string()),
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
        // One borrowed, trimmed view decides whether the line is a control.
        // Leading spaces do not turn a control into a model prompt, and the
        // raw line is what a refused control keeps as its draft.
        let control = typed.trim();
        if matches!(control, "/quit" | "/exit") {
            return true;
        }
        if control == "/clear" {
            self.app.push_error("In-place private clear requires a fresh exclusive unlock and is unsupported; the current conversation is retained.");
            return false;
        }
        if control.starts_with('/') && self.configuration.is_captured() {
            let mut parts = control.split_whitespace();
            if parts.next() == Some("/tasks") && parts.next() == Some("cancel") {
                match parts.next() {
                    Some(name) => self.cancel_task(name),
                    None => self.app.push("usage: /tasks cancel <name>"),
                }
                return false;
            }
            // The ordinary terminal's own read-only local commands. They read
            // the retained configuration and session; nothing is saved,
            // started or sent, and the line never becomes model input. The
            // legacy caller-Config entry keeps its earlier refusal.
            let owner = self.owner.clone();
            if let Ok(cfg) = self.configuration.source(&owner) {
                let session = match self.built.as_ref() {
                    Some(built) => Some(built.session_for_terminal()),
                    None => None,
                };
                if crate::tui::commands::handle_local(
                    control,
                    cfg,
                    session,
                    &self.approval,
                    &mut self.app,
                ) {
                    if control == "/help" {
                        self.app.push("In this private conversation these run: /help, /status, /context, /doctor, /config, /version, /tokens, /queue, /copy, /editor, /tasks, /tasks cancel <name>, /mcp, /skills, /agents (list), /checkpoints, /memories, /lsp, /gadgets, /diff, /clear and /quit. Settings changes and every other command are not executed.");
                    }
                    return false;
                }
                // A listing that waits for its owner runs as a task of this
                // frontend; its text arrives like any other returned work.
                if let Some(session) = session
                    && let Some(listing) = crate::tui::commands::Listing::of(control, session)
                {
                    match listing {
                        Ok(listing) => {
                            let future: Pin<Box<dyn Future<Output = Returned> + Send>> =
                                Box::pin(async move { Returned::Listing(listing.text().await) });
                            self.work.spawn(future);
                        }
                        Err(unavailable) => self.app.push(unavailable),
                    }
                    return false;
                }
            }
        }
        if control.starts_with('/') || control.starts_with('!') {
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
            Returned::Listing(Ok(text)) => self.app.push(text),
            Returned::Listing(Err(error)) => self.app.push_error(error),
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
        // The ordinary terminal's final verdict, one attempt.
        let status = if result.is_err() || cursor.is_err() {
            Status::Error
        } else if self.popup.inner.shutdown.exit_code() != 0 || self.app.running {
            Status::Idle
        } else {
            match self.app.program_status {
                Status::Done => Status::Done,
                Status::Error => Status::Error,
                _ => Status::Idle,
            }
        };
        let _ = self.popup.inner.guard.finish_program_status(status);
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
    /// The build ended and no normal read is in flight. A captured route's
    /// construction that had not returned is still in the frontend, and the
    /// retirement awaits its actual return inside its 30-second bound. The
    /// primary error is kept; a retirement that did not finish is reported
    /// around it. The legacy route returns its error and drops, as before.
    pub(super) async fn retire_failed_build(
        &mut self,
        primary: PrivateTerminalError,
    ) -> PrivateTerminalError {
        if !self.configuration.is_captured() {
            return primary;
        }
        drop(self.input.take());
        match self.retire_owners().await {
            Ok(true) => primary,
            _ => PrivateTerminalError::CleanupAfter(Box::new(primary)),
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
            // A captured route's construction that had not returned when its
            // build ended. Its actual return is awaited here; nothing of it
            // is installed. A session that still materialized is shut down
            // for real, and a refused one was shut down where it was refused.
            if let Some(mut construction) = self.construction.take() {
                failed |= match construction.as_mut().await {
                    Ok(late) => !late.shut_down().await,
                    Err(error) => matches!(error, PrivateBuildError::CleanupAfter(_)),
                };
            }
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
        // No mouse report may reach the password prompt that follows; the
        // rebuild switches the capture on again.
        if self.popup.inner.guard.capture_mouse(false).is_err() {
            return Err(PrivateTerminalError::InputHandoff);
        }
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
        self.configuration.check_bootstrap(&bootstrap)?;
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
                // A new prompt releases a blocked queue. A control does not:
                // a refused line that is kept there is not tried again, and
                // refused again, after every local command.
                if let Some((typed, _)) = &submitted
                    && !typed.trim_start().starts_with(['/', '!'])
                {
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
        // A construction still in its slot is dropped here without any claim.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Waker};

    #[test]
    fn a_wait_dropped_early_leaves_the_pending_work_and_its_return_empties_the_slot() {
        let released = Arc::new(AtomicBool::new(false));
        let gate = released.clone();
        let mut slot: Option<Pin<Box<dyn Future<Output = u8> + Send>>> =
            Some(Box::pin(std::future::poll_fn(move |_| {
                match gate.load(Ordering::SeqCst) {
                    true => Poll::Ready(7),
                    false => Poll::Pending,
                }
            })));
        let mut context = Context::from_waker(Waker::noop());
        {
            let mut wait = std::pin::pin!(retained(&mut slot));
            assert!(wait.as_mut().poll(&mut context).is_pending());
        }
        assert!(
            slot.is_some(),
            "the dropped wait took the pending work with it"
        );
        released.store(true, Ordering::SeqCst);
        {
            let mut wait = std::pin::pin!(retained(&mut slot));
            assert_eq!(wait.as_mut().poll(&mut context), Poll::Ready(7));
        }
        assert!(slot.is_none(), "work that returned stayed in its slot");
        let mut wait = std::pin::pin!(retained(&mut slot));
        assert!(wait.as_mut().poll(&mut context).is_pending());
    }

    #[test]
    fn a_refusal_whose_cleanup_did_not_finish_stays_inside_the_cleanup_report() {
        let refused = PrivateBuildError::CleanupAfter(Box::new(PrivateBuildError::Unavailable));
        assert!(matches!(
            build_error(refused),
            PrivateTerminalError::CleanupAfter(primary)
                if matches!(*primary, PrivateTerminalError::Build(PrivateBuildError::Unavailable))
        ));
        assert!(matches!(
            build_error(PrivateBuildError::Unavailable),
            PrivateTerminalError::Build(PrivateBuildError::Unavailable)
        ));
    }
}
