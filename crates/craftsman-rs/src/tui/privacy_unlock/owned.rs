//! A private pre-session popup. It owns input before any ordinary event stream
//! exists; no crossterm event queue, UI channel or shell reader receives secrets.
#[cfg(unix)]
use super::SecretField;
use crate::privacy_cli::PrivateKeyOperations;
#[cfg(unix)]
use crate::privacy_key::Passphrase;
use crate::privacy_key::{KeyError, Result, UnlockedVault};

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

impl OwnedSecretTerminal {
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
                .work(operations.initialize(first, confirmation))
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
            let phrase = self.inner.prompt(Prompt::Unlock).await?;
            self.inner.work(operations.unlock(phrase)).await
        }
        .await;
        #[cfg(not(unix))]
        let result = Err(KeyError::SecureStorageUnsupported);
        finish(result, operations.close_and_wait().await)
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
}
#[cfg(unix)]
impl Prompt {
    fn title(&self) -> &'static str {
        match self {
            Self::Initialize => "Initialize private privacy key",
            Self::Confirm => "Confirm private passphrase",
            Self::Unlock => "Unlock private privacy key",
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
        })
    }

    fn check(&self) -> Result<()> {
        if self.shutdown.cancellation.is_cancelled() || self.panic.triggered() {
            return Err(KeyError::Cancelled);
        }
        self.input.check_modes()
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<Passphrase> {
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
                            Paragraph::new("Enter confirms. Escape or Ctrl-C cancels; privacy stays unavailable."),
                            footer,
                        );
                    })
                    .map_err(|_| KeyError::Terminal)?;
                dirty = false;
            }
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
        operation: impl std::future::Future<Output = Result<UnlockedVault>>,
    ) -> Result<UnlockedVault> {
        self.terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new("Private key work is running. Escape or Ctrl-C closes admission; started work is awaited."),
                    frame.area(),
                );
            })
            .map_err(|_| KeyError::Terminal)?;
        tokio::pin!(operation);
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
                    if let Some(event) = self.input.next()? {
                        // No text is submitted while key work is outstanding.
                        if let SecretEvent::Key(key) = event
                            && (key.code == KeyCode::Esc
                                || key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL))
                        {
                            return Err(KeyError::Cancelled);
                        }
                    }
                },
            }
        }
    }
}

#[cfg(unix)]
impl Drop for UnixPopup {
    fn drop(&mut self) {
        self.input.clear_secret_buffers();
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
    fn next(&mut self) -> Result<Option<SecretEvent>> {
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
            if self
                .escape_at
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(30))
            {
                self.clear_secret_buffers();
                return Err(KeyError::Cancelled);
            }
            return Ok(None);
        }
        self.consumed = self.consumed.checked_add(1).ok_or(KeyError::Bounds)?;
        if self.consumed > INPUT_BYTES {
            return Err(KeyError::Bounds);
        }
        let mut byte = Zeroizing::new([0u8; 1]);
        let count = unsafe { libc::read(self.descriptor.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };
        if count != 1 {
            return Err(KeyError::Terminal);
        }
        self.decode(byte[0])
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
