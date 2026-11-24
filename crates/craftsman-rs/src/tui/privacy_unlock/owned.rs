//! A private popup that owns input before construction or after old owners
//! retire. No ordinary reader, UI channel or shell reader receives secrets.
#[cfg(unix)]
use super::SecretField;
use crate::privacy_cli::PrivateKeyOperations;
#[cfg(unix)]
use crate::privacy_key::Passphrase;
use crate::privacy_key::{KeyError, Result, UnlockedVault};
#[cfg(unix)]
mod live;
#[cfg(unix)]
mod settings;

/// What the owned interactive router lends the frontend it selected: the
/// person's own settings changes for the running private conversation. A
/// frontend without one refuses every settings change, as it always did.
/// `settings.rs` is the one implementation and says what each step does.
#[cfg(unix)]
trait SettingsAuthority: Send {
    /// A typed control or a picker's action. `true` when it was dealt with
    /// here: a list was shown, a refusal was pushed, a change or local work
    /// now waits, or `prompt` was set. `prompt` is the expansion of an
    /// extension command: model input only, which the frontend hands to the
    /// turn and never reads again as a control.
    fn asked(
        &mut self,
        asked: SettingsAsked<'_>,
        cfg: &crate::config::Config,
        built: Option<&crate::privacy_bootstrap::PrivateBuiltSession>,
        app: &mut crate::tui::app::App,
        prompt: &mut Option<String>,
    ) -> bool;
    /// Local work that the line just asked about started, to run as the
    /// frontend's own owned work. It runs inside a new turn of `session`,
    /// which is left in `running` for the person's cancellation.
    fn work(
        &mut self,
        session: &craftsman_harness::SessionCancellation,
        running: &mut Option<craftsman_harness::TurnCancellation>,
    ) -> Option<SettingsWork<'static>>;
    /// Whether a validated change waits to be carried out.
    fn pending(&self) -> bool;
    /// Retires the conversation of `built` for its successor. `busy`: the
    /// frontend still has work running, which refuses. It returns as the
    /// frontend's other owned local work does: an error line when nothing
    /// was retired, and then the change is dropped.
    fn retire<'a>(
        &'a mut self,
        built: Option<&'a crate::privacy_bootstrap::PrivateBuiltSession>,
        busy: bool,
    ) -> SettingsWork<'a>;
    /// After the frontend retired its owners: commits the change, once, and
    /// captures this invocation's startup anew for the successor. `live`
    /// becomes that startup's own approval state, with what the person
    /// granted during the conversation kept. Asked again after a refused
    /// password, it captures the startup again and writes nothing.
    fn recapture(
        &mut self,
        live: &mut std::sync::Arc<std::sync::Mutex<crate::hitl::ApprovalState>>,
    ) -> std::result::Result<crate::privacy_bootstrap::PrivateBootstrap, PrivateTerminalError>;
    /// The unlock for the successor ended with `error`. `Ok`: the vault
    /// refused the password and the retired conversation is kept for that
    /// many more attempts; the frontend asks for the next one through the
    /// same secret reader. `Err`: the handoff is over, with the last error
    /// to report.
    fn locked(
        &mut self,
        error: crate::privacy_bootstrap::BootstrapError,
    ) -> std::result::Result<u8, PrivateTerminalError>;
    /// An error of the frontend after this authority retired a conversation
    /// and before a successor adopted it: the same error, saying that the
    /// conversation was retired and what became of the change. Any other
    /// error is returned as it is.
    fn ended(&mut self, error: PrivateTerminalError) -> PrivateTerminalError;
    /// After the successor's session was built: adopts the retired record
    /// into it. Returns the line to show, or why nothing was adopted.
    fn adopt<'a>(
        &'a mut self,
        built: Option<&'a crate::privacy_bootstrap::PrivateBuiltSession>,
    ) -> SettingsWork<'a>;
}

/// Work of the settings authority that the frontend awaits where it stands.
/// It returns what the frontend's own local work returns: a line, or an
/// error line.
#[cfg(unix)]
type SettingsWork<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = live::Returned> + Send + 'a>>;

/// What a frontend asks its settings authority about.
#[cfg(unix)]
enum SettingsAsked<'a> {
    Line(&'a str),
    Action(&'a crate::tui::modals::AppAction),
}

#[derive(thiserror::Error)]
pub enum PrivateTerminalError {
    #[error("The private terminal bootstrap stopped: {0}")]
    Source(crate::privacy_bootstrap::BootstrapError),
    #[error("The private terminal session could not be built: {0}")]
    Build(crate::privacy_bootstrap::PrivateBuildError),
    #[error("The private terminal input or display stopped.")]
    Terminal,
    #[error("The private terminal entry was cancelled.")]
    Cancelled,
    #[error("Private terminal cleanup did not finish; work may still be completing.")]
    Cleanup,
    #[error("Private terminal cleanup also failed after this entry error: {0}")]
    CleanupAfter(Box<PrivateTerminalError>),
    #[error(
        "Fresh private clear could not complete input handoff; the old conversation has ended and no new work started."
    )]
    InputHandoff,
    #[error(
        "The captured terminal approval source is unavailable or foreign; nothing was unlocked."
    )]
    ApprovalSource,
    #[error("{0}")]
    Startup(crate::privacy_startup::StartupError),
    /// The conversation was retired for a settings change of the person's
    /// own and has no successor. Nothing of it is discarded silently.
    #[error(
        "The conversation was retired for a settings change and could not be carried into a new private scope. {0} It has ended; its record up to that point remains in the thread store."
    )]
    CarryOver(String),
}
impl std::fmt::Debug for PrivateTerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateTerminalError([fixed])")
    }
}

/// No frontend/command constructs this owner. It is the complete terminal
/// acquisition path for private init/unlock, not an active-turn modal adapter.
pub struct OwnedSecretTerminal {
    #[cfg(unix)]
    inner: UnixPopup,
}

impl std::fmt::Debug for OwnedSecretTerminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OwnedPrivacyPasswordTerminal([private])")
    }
}

/// Only the separate private entry constructors select these variants. A
/// captured variant carries no Config: every use checks the actual owner.
enum FrontendConfiguration {
    Legacy(crate::config::Config),
    Captured,
}
impl FrontendConfiguration {
    fn check_bootstrap(
        &self,
        bootstrap: &crate::privacy_bootstrap::PrivateBootstrap,
    ) -> std::result::Result<(), PrivateTerminalError> {
        let captured = bootstrap
            .captured_configuration()
            .map_err(PrivateTerminalError::Source)?;
        match (self, captured) {
            (Self::Legacy(_), None) => Ok(()),
            (Self::Captured, Some(cfg)) => {
                // A build with voice input owns the ordinary controller for
                // a captured startup. A build without it refuses a source
                // that asks for voice; the setting is never turned off.
                if !cfg!(feature = "voice") {
                    check_voice_configuration(cfg)?;
                }
                // The ordinary terminal refuses an invalid status setting
                // before any effect; so does this one, before any key work.
                match crate::tui::program_status::enabled(cfg) {
                    Ok(_) => Ok(()),
                    Err(_) => Err(PrivateTerminalError::Startup(
                        crate::privacy_startup::StartupError::Invalid,
                    )),
                }
            }
            _ => Err(PrivateTerminalError::Build(
                crate::privacy_bootstrap::PrivateBuildError::Unavailable,
            )),
        }
    }
    fn source<'a>(
        &'a self,
        owner: &'a std::sync::Arc<crate::privacy_bootstrap::PrivateConversation>,
    ) -> std::result::Result<&'a crate::config::Config, PrivateTerminalError> {
        match self {
            Self::Legacy(cfg) => Ok(cfg),
            Self::Captured => owner
                .captured_configuration()
                .map_err(PrivateTerminalError::Source)?
                .ok_or(PrivateTerminalError::Build(
                    crate::privacy_bootstrap::PrivateBuildError::Unavailable,
                )),
        }
    }
    fn legacy_for_build(&self) -> Option<crate::config::Config> {
        match self {
            Self::Legacy(cfg) => Some(cfg.clone()),
            Self::Captured => None,
        }
    }
    fn is_captured(&self) -> bool {
        matches!(self, Self::Captured)
    }
}

fn check_voice_configuration(
    cfg: &crate::config::Config,
) -> std::result::Result<(), PrivateTerminalError> {
    // The captured entry checks this before key work. Its popup already
    // exists; this is not normal before-terminal readiness or a downgrade.
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
    Ok(())
}

impl OwnedSecretTerminal {
    /// Read-only captured-source eligibility, before key work. This neither
    /// enters a terminal nor supplies ownership/permission from caller values.
    pub(crate) fn check_captured_bootstrap(
        bootstrap: &crate::privacy_bootstrap::PrivateBootstrap,
    ) -> std::result::Result<(), PrivateTerminalError> {
        FrontendConfiguration::Captured.check_bootstrap(bootstrap)
    }
    /// Called by the owned interactive router alone, for the startup it
    /// selected: the frontend of this terminal may change the person's
    /// model, effort and permission mode and carry the conversation over.
    /// Every other entry leaves this unset and refuses settings changes.
    pub(crate) fn lend_router_settings(&mut self) {
        #[cfg(unix)]
        {
            self.inner.settings = Some(Box::new(settings::RouterSettings::default()));
        }
    }
    /// Capture modes/authority and install signal ownership before raw entry.
    /// Windows refuses before secret input until secure storage is supported.
    pub fn enter() -> Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                inner: UnixPopup::enter()?,
            })
        }
        #[cfg(not(unix))]
        {
            Err(KeyError::SecureStorageUnsupported)
        }
    }

    pub async fn initialize(&mut self, operations: &PrivateKeyOperations) -> Result<UnlockedVault> {
        #[cfg(unix)]
        let result = async {
            let first = self.inner.prompt(Prompt::Initialize).await?;
            let confirmation = self.inner.prompt(Prompt::Confirm).await?;
            self.inner
                .work(Box::pin(operations.initialize(first, confirmation)))
                .await
        }
        .await;
        #[cfg(not(unix))]
        let result = Err(KeyError::SecureStorageUnsupported);
        // Cancellation drops only the waiter, closes admission, and then waits
        // for every started handle. It cannot interrupt a running KDF or undo IO.
        finish(result, operations.close_and_wait().await)
    }

    pub async fn unlock(&mut self, operations: &PrivateKeyOperations) -> Result<UnlockedVault> {
        #[cfg(unix)]
        let result = async {
            // After a refused password the prompt says so, once.
            let prompt = match std::mem::take(&mut self.inner.refused) {
                0 => Prompt::Unlock,
                left => Prompt::UnlockAgain(left),
            };
            let phrase = self.inner.prompt(prompt).await?;
            self.inner.work(Box::pin(operations.unlock(phrase))).await
        }
        .await;
        #[cfg(not(unix))]
        let result = Err(KeyError::SecureStorageUnsupported);
        finish(result, operations.close_and_wait().await)
    }

    /// Private post-unlock request using the actual UI bridge's COMPLETE normal
    /// extras and owned cancellation scope. It omits no unsupported part; the
    /// current shared builder refuses before effects. No normal route calls it.
    pub async fn request_full_session(
        &mut self,
        owner: &std::sync::Arc<crate::privacy_bootstrap::PrivateConversation>,
        cfg: &crate::config::Config,
        approval: std::sync::Arc<std::sync::Mutex<crate::hitl::ApprovalState>>,
    ) -> std::result::Result<
        crate::privacy_bootstrap::PrivateBuiltSession,
        crate::privacy_bootstrap::PrivateBuildError,
    > {
        let _ = (&cfg, &approval);
        #[cfg(unix)]
        {
            use crate::privacy_bootstrap::{BootstrapError, PrivateBuildError};
            self.inner
                .check()
                .map_err(|error| PrivateBuildError::Source(BootstrapError::Key(error)))?;
            owner.check_sources().map_err(PrivateBuildError::Source)?;
            // This historical private entry has no running UI receiver. It
            // cannot manufacture a successful session with queued human data.
            Err(PrivateBuildError::Refused(
                crate::privacy_session::BuildRefusal::RequestedPart,
            ))
        }
        #[cfg(not(unix))]
        {
            let _ = (owner, cfg, approval);
            Err(crate::privacy_bootstrap::PrivateBuildError::Source(
                crate::privacy_bootstrap::BootstrapError::Key(KeyError::SecureStorageUnsupported),
            ))
        }
    }

    /// The only successful private frontend route consumes this fresh popup,
    /// constructs the actual receiver/input/shutdown owner, and pumps it while
    /// the COMPLETE builder is pending. No caller-ready Boolean or arbitrary
    /// sender/bridge can substitute for that retained frontend.
    #[cfg(unix)]
    async fn request_full_session_on_bridge(
        self,
        owner: std::sync::Arc<crate::privacy_bootstrap::PrivateConversation>,
        configuration: FrontendConfiguration,
        approval: std::sync::Arc<std::sync::Mutex<crate::hitl::ApprovalState>>,
    ) -> std::result::Result<live::Frontend, PrivateTerminalError> {
        let mut frontend = live::Frontend::from_popup(self.inner, owner, configuration, approval)?;
        if let Err(primary) = frontend.build_full().await {
            return Err(frontend.retire_failed_build(primary).await);
        }
        Ok(frontend)
    }

    /// Actual fresh terminal bootstrap and loop, unselected by every normal
    /// route. The secret phase ends before the ordinary event reader begins;
    /// private clear retires that reader and the old owners for another unlock.
    /// Reopen/transfer and settings saves remain explicit refusals.
    pub async fn run_private(
        self,
        bootstrap: crate::privacy_bootstrap::PrivateBootstrap,
        cfg: crate::config::Config,
        approval: std::sync::Arc<std::sync::Mutex<crate::hitl::ApprovalState>>,
    ) -> std::result::Result<i32, PrivateTerminalError> {
        self.run_owned(bootstrap, FrontendConfiguration::Legacy(cfg), approval)
            .await
    }

    /// Private consuming entry for an actual startup-issued bootstrap. No
    /// caller configuration or approval state can become its build, voice,
    /// renderer or permission authority: the live approval state is the one
    /// the same startup issued, taken through the bootstrap's own retained
    /// requirements. The owned interactive router enters here after it
    /// selected the startup; any other caller's startup is not a selected one.
    pub async fn run_captured_private(
        self,
        bootstrap: crate::privacy_bootstrap::PrivateBootstrap,
        approval: crate::privacy_startup::CapturedApprovalStart,
    ) -> std::result::Result<i32, PrivateTerminalError> {
        let approval = Self::captured_approval(&bootstrap, &approval)?;
        self.run_owned(bootstrap, FrontendConfiguration::Captured, approval)
            .await
    }

    /// Before any key work: the bootstrap rechecks its source and lends its
    /// retained requirements, and only the start that was issued with them
    /// yields its state. That same live state goes on unchanged; nothing is
    /// reset, copied or derived here, and a foreign start is refused.
    fn captured_approval(
        bootstrap: &crate::privacy_bootstrap::PrivateBootstrap,
        start: &crate::privacy_startup::CapturedApprovalStart,
    ) -> std::result::Result<
        std::sync::Arc<std::sync::Mutex<crate::hitl::ApprovalState>>,
        PrivateTerminalError,
    > {
        let Some(requirements) = bootstrap
            .startup_requirements()
            .map_err(PrivateTerminalError::Source)?
        else {
            return Err(PrivateTerminalError::Build(
                crate::privacy_bootstrap::PrivateBuildError::Unavailable,
            ));
        };
        match start.state_for(requirements) {
            Ok(state) => Ok(state),
            Err(_) => Err(PrivateTerminalError::ApprovalSource),
        }
    }

    async fn run_owned(
        self,
        bootstrap: crate::privacy_bootstrap::PrivateBootstrap,
        configuration: FrontendConfiguration,
        approval: std::sync::Arc<std::sync::Mutex<crate::hitl::ApprovalState>>,
    ) -> std::result::Result<i32, PrivateTerminalError> {
        #[cfg(unix)]
        {
            // Verify genuine startup ownership and captured voice eligibility
            // before any key/unlock operation; compatibility stays separate.
            if configuration.is_captured() {
                Self::check_captured_bootstrap(&bootstrap)?;
            } else {
                configuration.check_bootstrap(&bootstrap)?;
            }
            let mut terminal = self;
            let owner = match bootstrap.unlock_private(&mut terminal).await {
                Ok(owner) => std::sync::Arc::new(owner),
                Err(error) => {
                    if matches!(
                        error,
                        crate::privacy_bootstrap::BootstrapError::Key(KeyError::Cancelled)
                    ) && terminal.inner.shutdown.exit_code() != 0
                    {
                        return Ok(terminal.inner.shutdown.exit_code());
                    }
                    return Err(PrivateTerminalError::Source(error));
                }
            };
            let mut frontend = terminal
                .request_full_session_on_bridge(owner, configuration, approval)
                .await?;
            frontend.run().await
        }
        #[cfg(not(unix))]
        {
            let _ = (self, bootstrap, configuration, approval);
            Err(PrivateTerminalError::Source(
                crate::privacy_bootstrap::BootstrapError::Key(KeyError::SecureStorageUnsupported),
            ))
        }
    }

    pub fn pending_ui_events(&self) -> usize {
        #[cfg(unix)]
        {
            self.inner
                .ui_events
                .as_ref()
                .map_or(0, |events| events.len())
        }
        #[cfg(not(unix))]
        {
            0
        }
    }
}

fn finish(result: Result<UnlockedVault>, cleanup: Result<()>) -> Result<UnlockedVault> {
    // Retain the actual key/source/cancellation error. Cleanup failure cannot
    // turn it into success, and a successful key cannot hide failed cleanup.
    match result {
        Err(error) => Err(error),
        Ok(unlocked) => match cleanup {
            Ok(()) => Ok(unlocked),
            Err(error) => {
                unlocked.close();
                Err(error)
            }
        },
    }
}

#[cfg(unix)]
use crate::tui::{
    shutdown::Shutdown,
    terminal::{PanicHookGuard, SafeBackend, TerminalGuard, TuiBackend},
};
#[cfg(unix)]
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(unix)]
use ratatui::{Terminal, backend::CrosstermBackend, widgets::Paragraph};
#[cfg(unix)]
use std::{io::IsTerminal, os::fd::AsRawFd, time::Duration};
#[cfg(unix)]
use zeroize::{Zeroize, Zeroizing};

#[cfg(unix)]
const INPUT_BYTES: usize = 16 * 1024;
#[cfg(unix)]
const TICK: Duration = Duration::from_millis(10);

#[cfg(unix)]
enum Prompt {
    Initialize,
    Confirm,
    Unlock,
    /// The unlock of a conversation that was retired for a settings change,
    /// after the vault refused a password. The conversation is kept, for
    /// this many more attempts.
    UnlockAgain(u8),
}
#[cfg(unix)]
impl Prompt {
    fn title(&self) -> &'static str {
        match self {
            Self::Initialize => "Initialize private privacy key",
            Self::Confirm => "Confirm private passphrase",
            Self::Unlock => "Unlock private privacy key",
            Self::UnlockAgain(1) => {
                "That password did not open the vault. The conversation is kept, locked.\nUnlock private privacy key again to continue it. One more attempt."
            }
            Self::UnlockAgain(_) => {
                "That password did not open the vault. The conversation is kept, locked.\nUnlock private privacy key again to continue it. Two more attempts."
            }
        }
    }
}

#[cfg(unix)]
struct UnixPopup {
    terminal: Terminal<TuiBackend>,
    input: SecretInput,
    // Declaration order restores the terminal before restoring the old hook
    // and releasing the signal owner. The hook prints fixed text only.
    guard: TerminalGuard,
    panic: PanicHookGuard,
    shutdown: Shutdown,
    ui_events: Option<tokio::sync::mpsc::UnboundedReceiver<crate::tui::session_bridge::UiEvent>>,
    secret_reader: bool,
    /// The vault refused the last password of a kept conversation, which has
    /// this many attempts left; the next unlock prompt says so. Zero
    /// otherwise.
    refused: u8,
    /// Lent by the owned interactive router alone; see [`SettingsAuthority`].
    settings: Option<Box<dyn SettingsAuthority>>,
}

#[cfg(unix)]
impl UnixPopup {
    fn enter() -> Result<Self> {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return Err(KeyError::Terminal);
        }
        let shutdown = Shutdown::install().map_err(|_| KeyError::Terminal)?;
        let guard = TerminalGuard::enter(false).map_err(|_| KeyError::Terminal)?;
        let panic = guard.panic_hook(shutdown.cancellation.clone());
        let descriptor = guard
            .retained_secret_input()
            .map_err(|_| KeyError::Terminal)?;
        let terminal = Terminal::new(SafeBackend(CrosstermBackend::new(std::io::stdout())))
            .map_err(|_| KeyError::Terminal)?;
        Ok(Self {
            terminal,
            input: SecretInput::new(descriptor),
            guard,
            panic,
            shutdown,
            ui_events: None,
            secret_reader: true,
            refused: 0,
            settings: None,
        })
    }

    fn check(&self) -> Result<()> {
        if self.shutdown.cancellation.is_cancelled() || self.panic.triggered() {
            return Err(KeyError::Cancelled);
        }
        self.input.check_modes()
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<Passphrase> {
        // The guard outlives the borrowed inner future. Dropping a pending
        // prompt first drops its SecretField, then clears/flushes this SAME
        // sole-reader input even when the terminal owner itself is retained.
        let mut scope = PopupInputScope::new(self);
        let result = scope.popup.prompt_inner(prompt).await;
        let cleanup = scope.finish();
        match result {
            Err(error) => Err(error),
            Ok(phrase) => cleanup.map(|()| phrase),
        }
    }

    async fn prompt_inner(&mut self, prompt: Prompt) -> Result<Passphrase> {
        self.check()?;
        self.input.begin_prompt();
        let mut field = SecretField::default();
        let mut dirty = true;
        loop {
            self.check()?;
            if dirty {
                self.terminal
                    .draw(|frame| {
                        let area = frame.area();
                        let header = ratatui::layout::Rect::new(area.x, area.y, area.width, 2);
                        frame.render_widget(Paragraph::new(prompt.title()), header);
                        let body = ratatui::layout::Rect::new(
                            area.x,
                            area.y.saturating_add(2),
                            area.width,
                            area.height.saturating_sub(4),
                        );
                        field.render(frame, body);
                        let footer = ratatui::layout::Rect::new(
                            area.x,
                            area.bottom().saturating_sub(2),
                            area.width,
                            2,
                        );
                        frame.render_widget(
                            Paragraph::new(
                                "Enter confirms. Escape or Ctrl-C cancels this key operation.",
                            ),
                            footer,
                        );
                    })
                    .map_err(|_| KeyError::Terminal)?;
                dirty = false;
            }
            let before = self.input.consumed;
            if let Some(event) = self.input.next()? {
                match event {
                    SecretEvent::Key(key) => {
                        if let Some(phrase) = field.accept_key(key)? {
                            self.input.clear_secret_buffers();
                            return Ok(phrase);
                        }
                    }
                    SecretEvent::Paste(paste) => field.accept_owned_paste(paste)?,
                }
                dirty = true;
                continue;
            }
            if self.input.consumed != before {
                // Already consumed UTF8/paste framing is decoded immediately,
                // under the same whole input cap, rather than one tick/byte.
                continue;
            }
            tokio::select! {
                biased;
                _ = self.shutdown.cancellation.cancelled() => {
                    field.cancel();
                    return Err(KeyError::Cancelled);
                }
                _ = tokio::time::sleep(TICK) => {},
            }
        }
    }

    async fn work(
        &mut self,
        operation: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<UnlockedVault>> + Send + '_>,
        >,
    ) -> Result<UnlockedVault> {
        let mut scope = PopupInputScope::new(self);
        scope.popup.input.clear_secret_buffers();
        let result = scope.popup.work_inner(operation).await;
        let cleanup = scope.finish();
        finish(result, cleanup)
    }

    async fn work_inner(
        &mut self,
        mut operation: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<UnlockedVault>> + Send + '_>,
        >,
    ) -> Result<UnlockedVault> {
        // An already cancelled/restored owner cannot start polling key work.
        // Cancellation can still race a later poll; started work stays owned.
        self.check()?;
        self.terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new("Private key work is running. Escape or Ctrl-C closes admission; started work is awaited."),
                    frame.area(),
                );
            })
            .map_err(|_| KeyError::Terminal)?;
        loop {
            tokio::select! {
                biased;
                result = &mut operation => {
                    match result {
                        Err(error) => return Err(error),
                        Ok(unlocked) => {
                            if let Err(error) = self.check() {
                                unlocked.close();
                                return Err(error);
                            }
                            return Ok(unlocked);
                        }
                    }
                },
                _ = self.shutdown.cancellation.cancelled() => return Err(KeyError::Cancelled),
                _ = tokio::time::sleep(TICK) => {
                    self.check()?;
                    self.input.discard_during_work()?;
                },
            }
        }
    }
}

#[cfg(unix)]
struct PopupInputScope<'a> {
    popup: &'a mut UnixPopup,
    finished: bool,
}

#[cfg(unix)]
impl<'a> PopupInputScope<'a> {
    fn new(popup: &'a mut UnixPopup) -> Self {
        Self {
            popup,
            finished: false,
        }
    }
    fn finish(&mut self) -> Result<()> {
        self.finished = true;
        self.popup.input.finish_input()
    }
}

#[cfg(unix)]
impl Drop for PopupInputScope<'_> {
    fn drop(&mut self) {
        if !self.finished {
            // An abandoned future has no result to report a native flush
            // error through. Its owned buffers still clear; native flushing
            // is best effort here, and never claims another reader is safe.
            let _ = self.popup.input.finish_input();
        }
    }
}

#[cfg(unix)]
impl Drop for UnixPopup {
    fn drop(&mut self) {
        // Sole-reader cleanup on this popup's retained descriptor, before the
        // guard restores it. TCIFLUSH discards ALL queued terminal type-ahead,
        // including bytes intended for a later reader. Bytes arriving after
        // that flush are outside this sampled cleanup observation.
        if self.secret_reader {
            let _ = self.input.finish_input();
        }
        // The retained guard restores the captured modes. No secret, draft,
        // history, exit-success or OS quiescence message is printed here.
        let _ = &self.guard;
    }
}

#[cfg(unix)]
enum SecretEvent {
    Key(KeyEvent),
    Paste(Zeroizing<String>),
}

#[cfg(unix)]
struct SecretInput {
    descriptor: std::os::fd::OwnedFd,
    consumed: usize,
    utf8: Zeroizing<Vec<u8>>,
    escape: Zeroizing<Vec<u8>>,
    escape_at: Option<tokio::time::Instant>,
    paste: Zeroizing<Vec<u8>>,
    in_paste: bool,
}

#[cfg(unix)]
impl SecretInput {
    fn new(descriptor: std::os::fd::OwnedFd) -> Self {
        Self {
            descriptor,
            consumed: 0,
            utf8: Zeroizing::new(Vec::with_capacity(4)),
            escape: Zeroizing::new(Vec::with_capacity(6)),
            escape_at: None,
            paste: Zeroizing::new(Vec::with_capacity(1024)),
            in_paste: false,
        }
    }
    fn clear_secret_buffers(&mut self) {
        self.utf8.zeroize();
        self.escape.zeroize();
        self.paste.zeroize();
        self.escape_at = None;
        self.in_paste = false;
    }
    fn finish_input(&mut self) -> Result<()> {
        // Clear owned decoder storage on every path, even a failed native
        // flush. The caller cannot treat an unsuccessful cleanup as success.
        self.clear_secret_buffers();
        if unsafe { libc::tcflush(self.descriptor.as_raw_fd(), libc::TCIFLUSH) } < 0 {
            return Err(KeyError::Terminal);
        }
        Ok(())
    }
    fn begin_prompt(&mut self) {
        self.clear_secret_buffers();
        self.consumed = 0;
    }
    fn check_modes(&self) -> Result<()> {
        let mut mode = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(self.descriptor.as_raw_fd(), &mut mode) } < 0
            || mode.c_lflag & (libc::ECHO | libc::ECHONL | libc::ICANON) != 0
        {
            return Err(KeyError::Terminal);
        }
        Ok(())
    }
    fn read_byte(&mut self, count_prompt: bool) -> Result<Option<u8>> {
        self.check_modes()?;
        let mut ready = libc::pollfd {
            fd: self.descriptor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // The popup is the sole reader. No global event-stream queue or
        // background secret reader exists. Poll never waits on a worker.
        let result = unsafe { libc::poll(&mut ready, 1, 0) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                return Ok(None);
            }
            return Err(KeyError::Terminal);
        }
        if ready.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(KeyError::Terminal);
        }
        if result == 0 {
            return Ok(None);
        }
        if count_prompt {
            self.consumed = self.consumed.checked_add(1).ok_or(KeyError::Bounds)?;
            if self.consumed > INPUT_BYTES {
                return Err(KeyError::Bounds);
            }
        }
        let mut byte = Zeroizing::new([0u8; 1]);
        let count = unsafe { libc::read(self.descriptor.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };
        if count != 1 {
            return Err(KeyError::Terminal);
        }
        Ok(Some(byte[0]))
    }
    fn next(&mut self) -> Result<Option<SecretEvent>> {
        if let Some(byte) = self.read_byte(true)? {
            return self.decode(byte);
        }
        if self
            .escape_at
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(30))
        {
            self.clear_secret_buffers();
            return Err(KeyError::Cancelled);
        }
        Ok(None)
    }
    fn discard_during_work(&mut self) -> Result<()> {
        // A bounded raw batch acts only on Ctrl-C or a lone Escape. Navigation,
        // paste and other bytes are discarded without passphrase decoding.
        for _ in 0..64 {
            // These bytes are not passphrase input. Each work tick retains
            // its own 64-read bound without spending the prompt allowance.
            match self.read_byte(false)? {
                Some(0x03) => return Err(KeyError::Cancelled),
                Some(0x1b) => self.escape_at = Some(tokio::time::Instant::now()),
                Some(_) => self.escape_at = None,
                None => break,
            }
        }
        if self
            .escape_at
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(30))
        {
            return Err(KeyError::Cancelled);
        }
        Ok(())
    }
    fn decode(&mut self, byte: u8) -> Result<Option<SecretEvent>> {
        if self.escape_at.is_some() {
            let expected = if self.in_paste { b"[201~" } else { b"[200~" };
            let index = self.escape.len();
            if expected.get(index) != Some(&byte) {
                self.clear_secret_buffers();
                return Err(KeyError::Cancelled);
            }
            self.escape.push(byte);
            if self.escape.len() < expected.len() {
                return Ok(None);
            }
            self.escape.zeroize();
            self.escape_at = None;
            if !self.in_paste {
                self.in_paste = true;
                return Ok(None);
            }
            self.in_paste = false;
            // Validate the owned bytes before conversion, so a UTF-8 error
            // never owns a non-zeroizing FromUtf8Error copy of the secret.
            std::str::from_utf8(&self.paste).map_err(|_| KeyError::Passphrase)?;
            let bytes = std::mem::take(&mut *self.paste);
            // SAFETY: the preceding check covers exactly these unchanged bytes.
            let text = unsafe { String::from_utf8_unchecked(bytes) };
            self.paste = Zeroizing::new(Vec::with_capacity(1024));
            return Ok(Some(SecretEvent::Paste(Zeroizing::new(text))));
        }
        if byte == 0x1b {
            if !self.utf8.is_empty() {
                return Err(KeyError::Passphrase);
            }
            self.escape_at = Some(tokio::time::Instant::now());
            return Ok(None);
        }
        if self.in_paste {
            if self.paste.len() >= 1024 {
                return Err(KeyError::Passphrase);
            }
            self.paste.push(byte);
            return Ok(None);
        }
        if self.utf8.is_empty() && byte.is_ascii() {
            let (code, modifiers) = match byte {
                0x03 => (KeyCode::Char('c'), KeyModifiers::CONTROL),
                b'\r' | b'\n' => (KeyCode::Enter, KeyModifiers::NONE),
                0x08 | 0x7f => (KeyCode::Backspace, KeyModifiers::NONE),
                0x20..=0x7e => (KeyCode::Char(char::from(byte)), KeyModifiers::NONE),
                _ => return Err(KeyError::Passphrase),
            };
            return Ok(Some(SecretEvent::Key(KeyEvent::new(code, modifiers))));
        }
        if self.utf8.len() >= 4 {
            return Err(KeyError::Passphrase);
        }
        self.utf8.push(byte);
        let character = match std::str::from_utf8(&self.utf8) {
            Ok(text) => text.chars().next().ok_or(KeyError::Passphrase)?,
            Err(error) if error.error_len().is_none() => return Ok(None),
            Err(_) => return Err(KeyError::Passphrase),
        };
        self.utf8.zeroize();
        if character.is_control() {
            return Err(KeyError::Passphrase);
        }
        Ok(Some(SecretEvent::Key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE,
        ))))
    }
}
