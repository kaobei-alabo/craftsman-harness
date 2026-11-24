//! A fresh private terminal's real receiver/input/task owner. This is never
//! called by a normal selector. Password acquisition has finished before this
//! sole ordinary reader exists; active-entry password reacquisition is refused.
use super::{
    FrontendConfiguration, OwnedSecretTerminal, PrivateTerminalError, SettingsAsked, UnixPopup,
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

/// Local work of this frontend that waits for its owner: a read-only listing
/// or the person's own shell escape.
enum LocalWork {
    Listing(crate::tui::commands::Listing),
    Shell(
        String,
        crate::shell::ShellSettings,
        craftsman_harness::TurnCancellation,
    ),
}

/// Runs local work for the conversation `owner`. The source is checked
/// where the work runs and again before its result is published; a source
/// that stopped gives the fixed line below and not the result. The shell
/// runs inside its own cancellation turn.
/// The most bytes a voice event can add to the composer draft, for the
/// events that add any: the committed text beyond what the draft already
/// holds of this segment (`held` bytes), the final text, a separator before
/// each and the one that may follow the dictation.
#[cfg(feature = "voice")]
fn dictated_bytes(event: &crate::voice::VoiceEvent, held: usize) -> Option<usize> {
    use crate::voice::VoiceEvent;
    match event {
        VoiceEvent::Partial { committed, .. } => {
            Some(committed.len().saturating_sub(held).saturating_add(2))
        }
        VoiceEvent::SegmentFinal {
            text, committed, ..
        } => Some(
            committed
                .len()
                .saturating_sub(held)
                .saturating_add(text.len())
                .saturating_add(3),
        ),
        _ => None,
    }
}

/// The value of `$result`, or for any error a return of `$error`. A plain
/// match where an adapter call with a closure stood: the same work in the
/// same order, without a function of its own for each place.
macro_rules! or_return {
    ($result:expr, $error:expr) => {
        match $result {
            Ok(value) => value,
            Err(_) => return Err($error),
        }
    };
}

async fn local_work(work: LocalWork, owner: Arc<PrivateConversation>) -> Returned {
    const STOPPED: &str = "The private source owner stopped; this local result is not shown.";
    let before = owner.check_sources().is_ok();
    match work {
        LocalWork::Listing(listing) => {
            if !before {
                return Returned::Listing(Err(STOPPED.to_owned()));
            }
            let text = listing.text().await;
            match owner.check_sources() {
                Ok(()) => Returned::Listing(text),
                Err(_) => Returned::Listing(Err(STOPPED.to_owned())),
            }
        }
        LocalWork::Shell(command, settings, turn) => {
            if !before {
                return Returned::Operation(STOPPED.to_owned());
            }
            let output = turn
                .scope(crate::tools::run_shell_escape(&command, &settings))
                .await;
            match owner.check_sources() {
                Ok(()) => Returned::Operation(output),
                Err(_) => Returned::Operation(STOPPED.to_owned()),
            }
        }
    }
}

pub(super) enum Returned {
    Turn(Result<craftsman_harness::TurnResult, PrivateTurnError>),
    Steer(String, Result<bool, PrivateTurnError>),
    Decision(Result<Option<craftsman_harness::TurnResult>, PrivateTurnError>),
    /// A read-only listing's text, or its error line.
    Listing(Result<String, String>),
    /// The output of the person's own shell escape.
    Operation(String),
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
    /// The adoption of a retired conversation, from its start until it
    /// returns or the person or a shutdown ends it.
    adoption: Option<crate::privacy_bootstrap::OwnedAdoption>,
    work: JoinSet<Returned>,
    queued_blocked: bool,
    /// Listings that were started and have not returned. They are local
    /// work, not a model turn: the status reports them, and only their
    /// actual return or the retirement ends them.
    listings: usize,
    /// The ordinary terminal's local voice owner while this frontend has
    /// one: the controller, and the task that hands its events to this
    /// frontend's own UI channel. Both are retired before any password
    /// prompt and inside the retirement's 30-second bound.
    #[cfg(feature = "voice")]
    voice: Option<(crate::voice::VoiceController, tokio::task::JoinHandle<()>)>,
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
            or_return!(popup.check(), PrivateTerminalError::Terminal);
            if !popup.secret_reader {
                return Err(PrivateTerminalError::Terminal);
            }
            // A captured startup in a build with voice input keeps the
            // setting it asked for: this frontend owns the ordinary voice
            // controller. Every other entry refuses a source that does not
            // turn voice off; it is never turned off for the user.
            let cfg = configuration.source(&owner)?;
            if !(cfg!(feature = "voice") && configuration.is_captured()) {
                check_voice_configuration(cfg)?;
            }
            or_return!(popup.input.finish_input(), PrivateTerminalError::Terminal);
            let cancellation = or_return!(
                popup.shutdown.cancellation.child(),
                PrivateTerminalError::Cancelled
            );
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
            adoption: None,
            work: JoinSet::new(),
            queued_blocked: false,
            listings: 0,
            #[cfg(feature = "voice")]
            voice: None,
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
            .report_program_status(
                self.app
                    .current_program_status(pending || self.listings != 0),
                registry,
                pending,
            )
            .is_err()
        {
            return Err(PrivateTerminalError::Terminal);
        }
        match draw_session(
            &mut self.popup.inner.terminal,
            &mut self.app,
            cfg,
            session,
            pending,
            &self.approval,
        ) {
            Ok(drawn) => Ok(drawn),
            Err(_) => Err(PrivateTerminalError::Terminal),
        }
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
            // Dictated text enters the same draft as typed text and keeps
            // its bound. A segment that would take the draft past it is
            // refused whole and the dictation is cancelled; the earlier
            // draft stays as it is.
            #[cfg(feature = "voice")]
            if let UiEvent::Voice(dictated) = &event
                && self.app.voice.active()
                && let Some(extra) = dictated_bytes(dictated, self.app.voice.committed.len())
                && self
                    .app
                    .input
                    .len()
                    .checked_add(extra)
                    .is_none_or(|size| size > crate::privacy_hook::MAX_TEXT_BYTES)
            {
                crate::tui::input::request_voice_cancel(&mut self.app);
                self.app.push_error("This whole private input exceeds its byte bound; the earlier draft is retained.");
                continue;
            }
            let session = self
                .built
                .as_ref()
                .map(|built| built.session_for_terminal());
            privacy_approval::handle(&mut self.app, event, &self.ui, session);
        }
    }
    fn input_event(&mut self, event: Event) -> bool {
        // Without an owned voice controller the voice key is refused here.
        // A captured startup in a build with voice input leaves the key to
        // the ordinary input owner, which asks this frontend for the voice.
        if !(cfg!(feature = "voice") && self.configuration.is_captured())
            && matches!(&event, Event::Key(key) if key.kind == KeyEventKind::Press
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
        let mut action = self.app.pending_action.take();
        // A frontend the router selected holds its settings authority: the
        // model, effort and mode pickers go there. What it does not take
        // keeps its refusal below.
        if let Some(asked) = &action
            && !matches!(asked, AppAction::RemoveQueued(_) | AppAction::CancelTask(_))
        {
            let owner = self.owner.clone();
            if let Some(authority) = &mut self.popup.inner.settings
                && let Ok(cfg) = self.configuration.source(&owner)
                && authority.asked(
                    SettingsAsked::Action(asked),
                    cfg,
                    self.built.as_ref(),
                    &mut self.app,
                    &mut None,
                )
            {
                action = None;
            }
        }
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
        #[cfg(feature = "voice")]
        if self.configuration.is_captured() {
            self.voice_request();
        }
        if self.app.voice_request.take().is_some() {
            self.app
                .push_error("Voice input has no reviewed private adapter in this entry.");
        }
    }
    /// What the ordinary input owner asks of the voice, done by the ordinary
    /// local controller that this frontend owns. Its events go to this
    /// frontend's own UI channel and from there to the ordinary handler, so
    /// dictated text enters the same composer draft as typed text and is
    /// submitted as the same private input. The settings are the captured
    /// source's own; nothing is changed, downloaded or enabled beyond them.
    #[cfg(feature = "voice")]
    fn voice_request(&mut self) {
        use crate::tui::input::VoiceRequest;
        let Some(request) = self.app.voice_request.take() else {
            return;
        };
        if matches!(request, VoiceRequest::Start) && self.voice.is_none() {
            let owner = self.owner.clone();
            let config = match self.configuration.source(&owner) {
                Ok(cfg) => crate::voice::VoiceConfig::from_config(cfg),
                Err(error) => {
                    crate::tui::input::request_voice_cancel(&mut self.app);
                    self.app.voice_request = None;
                    self.app.push_error(error.to_string());
                    return;
                }
            };
            let (events, mut received) = mpsc::unbounded_channel();
            let ui = self.bridge.tx.clone();
            let forward = tokio::spawn(async move {
                while let Some(event) = received.recv().await {
                    if ui.send(UiEvent::Voice(event)).is_err() {
                        break;
                    }
                }
            });
            self.voice = Some((
                crate::voice::VoiceController::spawn(config, events),
                forward,
            ));
        }
        if matches!(request, VoiceRequest::Recreate) {
            // As in the ordinary terminal: the controller is shut down for
            // real and the voice state starts over. The next Start spawns a
            // new one from the captured settings. The shutdown is this
            // frontend's own local work, joined by the retirement.
            if let Some((controller, forward)) = self.voice.take() {
                let future: Pin<Box<dyn Future<Output = Returned> + Send>> = Box::pin(async move {
                    controller.shutdown().await;
                    forward.abort();
                    let _ = forward.await;
                    Returned::Listing(Ok("Voice input was shut down and reset.".into()))
                });
                self.work.spawn(future);
                self.listings += 1;
            }
            self.app.voice.state = crate::voice::VoiceState::Idle;
            self.app.voice.partials_enabled = None;
            self.app.dirty = true;
            return;
        }
        let Some((controller, _)) = &self.voice else {
            return;
        };
        match request {
            VoiceRequest::Start => controller.start(),
            VoiceRequest::StopAndFinalize => controller.stop_and_finalize(),
            VoiceRequest::CancelSegment => controller.cancel_segment(),
            VoiceRequest::Cancel | VoiceRequest::Recreate => controller.cancel(),
        }
    }
    /// Cancels and shuts the owned voice controller down for real, then
    /// ends its event task. Called first in every retirement, so before any
    /// password prompt of a fresh clear and inside the 30-second bound. The
    /// controller's own shutdown stops capture and transcription. The event
    /// task is aborted and then awaited: it would otherwise run until every
    /// sender of the controller is gone, and a sender kept somewhere would
    /// hold up every cleanup behind it. The UI receiver is closed by then.
    #[cfg(feature = "voice")]
    async fn retire_voice(&mut self) {
        if let Some((controller, forward)) = self.voice.take() {
            controller.cancel();
            controller.shutdown().await;
            forward.abort();
            let _ = forward.await;
        }
        crate::tui::input::request_voice_cancel(&mut self.app);
        self.app.voice_request = None;
    }
    /// The ordinary terminal's task cancellation, on this session's own
    /// registry and only through its bound owner. The lines are the ordinary
    /// ones; a refused owner says why.
    fn cancel_task(&mut self, name: &str) {
        // The bound owner's own checks, then this session's registry. `true`
        // is the registry accepting the abort of a running task; the task's
        // own cleanup is awaited by the retirement, not here.
        let aborted = match self.built.as_ref() {
            Some(built) => match built.task_owner() {
                Ok(_) => Ok(built.session_for_terminal().registry.abort(name)),
                Err(error) => Err(error),
            },
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
    /// The ordinary terminal's shell escape: the person's own command, run
    /// locally by the ordinary runner with the captured configuration's
    /// shell settings and its native limits, inside a turn of this
    /// frontend's own cancellation. Its output is shown here. Neither the
    /// command nor the output becomes model input or a record of the
    /// conversation. The ordinary runner takes no directory and runs where
    /// the process stands, so the command runs only while that is still the
    /// workspace this conversation was captured for. `false`: nothing was
    /// started, and the caller keeps the line.
    fn shell_escape(&mut self, command: String) -> bool {
        if self.app.running {
            self.app
                .push_error("A shell escape waits for the running work; it was not executed.");
            return false;
        }
        let owner = self.owner.clone();
        let settings = match self.configuration.source(&owner) {
            Ok(cfg) => crate::shell::ShellSettings::from_config(cfg),
            Err(error) => {
                self.app.push_error(error.to_string());
                return false;
            }
        };
        let here = match std::env::current_dir() {
            Ok(here) => std::fs::canonicalize(here.as_path()),
            Err(error) => Err(error),
        };
        let same = match (&here, &self.built) {
            (Ok(here), Some(built)) => here.as_path() == built.workspace(),
            _ => false,
        };
        if !same {
            self.app.push_error("The current directory is not the workspace this conversation was captured for; the shell escape was not executed.");
            return false;
        }
        let turn = self.cancellation.new_turn();
        self.app.running = true;
        self.app.operation_cancellation = Some(turn.clone());
        let future: Pin<Box<dyn Future<Output = Returned> + Send>> =
            Box::pin(local_work(LocalWork::Shell(command, settings, turn), owner));
        self.work.spawn(future);
        true
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
        // As the ordinary submission: no dictation marker or text of the
        // submitted draft stays behind for the next one.
        #[cfg(feature = "voice")]
        {
            self.app.voice.marker = None;
            self.app.voice.tentative.clear();
            self.app.voice.committed.clear();
        }
        self.app.dirty = true;
        Some((typed, queue))
    }
    fn start(&mut self, mut typed: String, queue: bool) -> bool {
        // The expansion of an extension command, where the lent authority
        // gave one for this line.
        let mut expanded = None;
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
                        self.app.push("In this private conversation these run: /help, /status, /context, /doctor, /config, /version, /tokens, /queue, /copy, /editor, /tasks, /tasks cancel <name>, /mcp, /skills, /agents (list), /checkpoints, /memories, /lsp, /gadgets, /diff, /clear and /quit. Where the ordinary `craftsman` start opened this terminal, /model, /effort and /mode change that setting: the conversation is carried over after a new unlock; /compact summarizes it; and an extension command sends its expansion to the model. Every other command is not executed.");
                    }
                    return false;
                }
                // The person's own model, effort and mode changes, a manual
                // compaction and the extension commands, where the router
                // lent their authority.
                if let Some(authority) = &mut self.popup.inner.settings
                    && authority.asked(
                        SettingsAsked::Line(control),
                        cfg,
                        self.built.as_ref(),
                        &mut self.app,
                        &mut expanded,
                    )
                    && expanded.is_none()
                {
                    // Work it started for this line is this frontend's own
                    // local work, inside a turn of its own cancellation.
                    if let Some(work) =
                        authority.work(&self.cancellation, &mut self.app.operation_cancellation)
                    {
                        self.app.running = true;
                        self.work.spawn(work);
                    }
                    return false;
                }
                // A listing that waits for its owner runs as a task of this
                // frontend; its text arrives like any other returned work.
                if expanded.is_none()
                    && let Some(built) = self.built.as_ref()
                    && let Some(listing) = crate::tui::commands::Listing::of(
                        control,
                        built.session_for_terminal(),
                        built.workspace(),
                    )
                {
                    // Only for this conversation's own bound session.
                    match (built.task_owner(), listing) {
                        (Err(error), _) => self.app.push_error(error.to_string()),
                        (Ok(_), Err(unavailable)) => self.app.push(unavailable),
                        (Ok(_), Ok(listing)) => {
                            let future: Pin<Box<dyn Future<Output = Returned> + Send>> =
                                Box::pin(local_work(LocalWork::Listing(listing), owner.clone()));
                            self.work.spawn(future);
                            self.listings += 1;
                        }
                    }
                    return false;
                }
            }
        }
        if let Some(command) = control.strip_prefix('!')
            && self.configuration.is_captured()
        {
            let waits = self.app.running;
            if !self.shell_escape(command.trim().to_owned()) {
                // The line is kept as it was typed. One that only waited
                // for running work is tried again after it, as in the
                // ordinary terminal; a refused one waits for a new prompt.
                self.app.queue_follow_up(typed);
                if !waits {
                    self.queued_blocked = true;
                }
            }
            return false;
        }
        if expanded.is_some() && (self.app.running || self.app.operation_cancellation.is_some()) {
            // As in the ordinary terminal, an extension command waits for
            // running work as it was typed.
            self.app.queue_follow_up(typed);
            return false;
        }
        if expanded.is_none() && (control.starts_with('/') || control.starts_with('!')) {
            // No normal settings/save/reopen/transfer/extension adapter is
            // silently routed around the selected entry's actual source paths.
            self.app.push_error(
                "This command has no reviewed private entry adapter; it was not executed.",
            );
            self.app.queue_follow_up(typed);
            self.queued_blocked = true;
            return false;
        }
        if (queue && self.app.running) || self.app.operation_cancellation.is_some() {
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
        } else if parked && expanded.is_some() {
            self.app
                .push_error("Resolve the pending approval before running an extension command.");
            self.app.queue_follow_up(typed);
            self.queued_blocked = true;
        } else if parked {
            let future: Pin<Box<dyn Future<Output = Returned> + Send>> =
                Box::pin(async move { Returned::Decision(task.request_decision(&typed).await) });
            self.app.running = true;
            self.work.spawn(future);
        } else {
            // The person sees the line they typed. An extension command's
            // expansion is model input only: it goes straight to the turn
            // below and is never read again as a control or a shell escape.
            let shown = match expanded {
                Some(prompt) => {
                    let line = typed;
                    typed = prompt;
                    line
                }
                None => typed.clone(),
            };
            self.app.push_user(shown);
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
            Returned::Listing(text) => {
                self.listings = self.listings.saturating_sub(1);
                match text {
                    Ok(text) => self.app.push(text),
                    Err(error) => self.app.push_error(error),
                }
            }
            Returned::Operation(output) => self.events(UiEvent::OperationFinished(output)),
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
                or_return!(cursor, PrivateTerminalError::Terminal);
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
            #[cfg(feature = "voice")]
            self.retire_voice().await;
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
    /// `carry`: the conversation is retired for a settings change and its
    /// record is adopted by the conversation that is built here.
    async fn fresh_clear(&mut self, carry: bool) -> Result<(), PrivateTerminalError> {
        if carry {
            // The lent authority retires the conversation through its own
            // owners first. A refusal keeps it, and this frontend, as they
            // are.
            let busy = self.app.running || !self.work.is_empty() || self.listings != 0;
            let Some(authority) = &mut self.popup.inner.settings else {
                return Err(PrivateTerminalError::InputHandoff);
            };
            if let Returned::Listing(Err(line)) = authority.retire(self.built.as_ref(), busy).await
            {
                self.app.push_error(line);
                return Ok(());
            }
        }
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
        while or_return!(
            crossterm::event::poll(Duration::ZERO),
            PrivateTerminalError::InputHandoff
        ) {
            if retained >= EVENT_BATCH {
                return Err(PrivateTerminalError::InputHandoff);
            }
            let event = or_return!(crossterm::event::read(), PrivateTerminalError::InputHandoff);
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
        // A voice key among that type-ahead asked for the microphone. No
        // such request crosses the password prompt: after the unlock the
        // voice starts only on a new request.
        #[cfg(feature = "voice")]
        {
            crate::tui::input::request_voice_cancel(&mut self.app);
            self.app.voice_request = None;
        }
        drop(input);
        self.built = None; // no closed lane/client/view is used for the rebuild
        // A conversation that was retired for a settings change is kept
        // through a refused password. Its authority says whether the handoff
        // goes on; the password is then asked again through this same
        // secret reader, for a startup captured anew and the successor
        // binding the retirement named. Until the vault opens nothing else
        // reads input and no work is admitted. A clear has no record to
        // keep and ends on its first refusal, as before.
        let fresh = loop {
            let bootstrap = match (carry, &mut self.popup.inner.settings) {
                // The change is committed, once, and the startup captured
                // anew, for the successor of the conversation that was just
                // retired. Its approval state takes the place of the
                // retired start's.
                (true, Some(authority)) => authority.recapture(&mut self.approval),
                (true, None) => Err(PrivateTerminalError::InputHandoff),
                (false, _) => old
                    .capture_after_retirement()
                    .map_err(PrivateTerminalError::Source),
            }?;
            self.configuration.check_bootstrap(&bootstrap)?;
            self.popup.inner.secret_reader = true;
            or_return!(
                self.popup.inner.input.finish_input(),
                PrivateTerminalError::InputHandoff
            );
            match bootstrap.unlock_private(&mut self.popup).await {
                Ok(fresh) => break Arc::new(fresh),
                Err(error) => match (carry, &mut self.popup.inner.settings) {
                    (true, Some(authority)) => match authority.locked(error) {
                        Ok(left) => self.popup.inner.refused = left,
                        Err(last) => return Err(last),
                    },
                    _ => return Err(PrivateTerminalError::Source(error)),
                },
            }
        };
        // A clear starts another thread; a carried conversation keeps its
        // own, in a scope of its own.
        if (fresh.thread_id() == old_id) != carry {
            fresh.close();
            return Err(PrivateTerminalError::InputHandoff);
        }
        fresh
            .check_sources()
            .map_err(PrivateTerminalError::Source)?;
        let cancellation = or_return!(
            self.popup.inner.shutdown.cancellation.child(),
            PrivateTerminalError::Cancelled
        );
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
        if carry {
            let Some(authority) = &mut self.popup.inner.settings else {
                return Err(PrivateTerminalError::InputHandoff);
            };
            // The adoption is this frontend's own kept work, polled like
            // its build: the terminal is drawn, a shutdown is observed and
            // typed input is read. What is typed stays an editable draft
            // or a queued line as typed. Nothing is submitted, no control
            // runs and no waiter is answered until the adoption returned.
            // The pending handoff stays with the authority throughout.
            self.adoption = match authority.adoption(self.built.as_ref()) {
                Ok(work) => Some(work),
                Err(reason) => return Err(PrivateTerminalError::CarryOver(reason)),
            };
            let mut tick = tokio::time::interval(Duration::from_millis(16));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            self.app.running = true;
            let adopted = loop {
                tokio::select! {
                    biased;
                    adopted = self::retained(&mut self.adoption) => break adopted,
                    _ = self.popup.inner.shutdown.cancellation.cancelled() => {
                        // The work is dropped: nothing was adopted.
                        self.adoption = None;
                        return Err(PrivateTerminalError::Cancelled);
                    }
                    event = next_normal(&mut self.input) => {
                        let cancelled = match event {
                            Some(Ok(event)) => self.input_event(event) || self.app.cancel_requested,
                            _ => {
                                self.adoption = None;
                                return Err(PrivateTerminalError::Terminal);
                            }
                        };
                        if cancelled {
                            // Ctrl-C or an exit: the owned work is ended
                            // with the session's own cancellation, and the
                            // error tells what became of the change.
                            self.cancellation.cancel();
                            self.adoption = None;
                            return Err(PrivateTerminalError::Cancelled);
                        }
                    }
                    _ = tick.tick() => {
                        if let Err(error) = self.draw(true) {
                            self.adoption = None;
                            return Err(error);
                        }
                    }
                }
            };
            self.app.running = false;
            self.app.dirty = true;
            let Some(authority) = &mut self.popup.inner.settings else {
                return Err(PrivateTerminalError::InputHandoff);
            };
            match authority.adopted(adopted) {
                Ok(line) => self.app.push(line),
                Err(reason) => return Err(PrivateTerminalError::CarryOver(reason)),
            }
        }
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
                        self.fresh_clear(false).await?;
                        continue;
                    }
                    if self.start(typed, queue) {
                        return Ok(0);
                    }
                }
            }
            // A validated settings change of the person's own: the
            // conversation is retired and carried over, like a clear that
            // keeps its record.
            if self.app.modal.is_none()
                && let Some(authority) = &self.popup.inner.settings
                && authority.pending()
            {
                // An error after the retirement says that the conversation
                // was retired and what became of the change.
                if let Err(error) = self.fresh_clear(true).await {
                    return Err(match &mut self.popup.inner.settings {
                        Some(authority) => authority.ended(error),
                        None => error,
                    });
                }
                continue;
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
        // An owned voice controller is told to cancel and its event task is
        // aborted; nothing can be awaited here, and nothing is claimed about
        // native capture that was still stopping.
        #[cfg(feature = "voice")]
        if let Some((controller, forward)) = self.voice.take() {
            controller.cancel();
            forward.abort();
        }
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
