//! Private key ownership; no CLI/frontend/hook selects it. Started work is owned
//! through actual return, not interrupted, and has no absolute OS time guarantee.
mod envelope;
pub(crate) mod root;
use crate::privacy_vault::{HostKey, HostKeySource, Limits, Vault};
pub use envelope::{ENVELOPE_BYTES, ITERATIONS, Passphrase};
pub use root::ConfigRoot;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::Poll,
};
use tokio::{runtime::Handle, sync::oneshot, task::JoinHandle};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("The private key randomness source is unavailable.")]
    Random,
    #[error("The private passphrase does not meet the declared byte policy.")]
    Passphrase,
    #[error("The private key envelope is invalid or unsupported.")]
    Invalid,
    #[error("The private key envelope did not authenticate.")]
    Unlock,
    #[error("The private key source is missing.")]
    Missing,
    #[error("The private key operation requires an unchanged private user config root.")]
    Root,
    #[error("The private key source changed during the operation.")]
    Changed,
    #[error("The private key operation did not complete its IO.")]
    Io,
    #[error("The private key operation exceeds its byte bounds.")]
    Bounds,
    #[error("The private key operation is closed.")]
    Closed,
    #[error("A private key operation is already in progress.")]
    Busy,
    #[error("The private key attempt allowance is exhausted.")]
    Attempts,
    #[error("The private key worker did not return successfully.")]
    Worker,
    #[error("The private key initialization requires both files and sidecars to be absent.")]
    AlreadyInitialized,
    #[error("Secure private key file ownership is unsupported on this platform.")]
    SecureStorageUnsupported,
    #[error("The existing private vault did not authenticate or open successfully.")]
    Vault,
}
pub type Result<T> = std::result::Result<T, KeyError>;

/// No raw-key/status/hash constructor. Created only after the owned envelope
/// authenticates, or by the explicit create-only initialization below.
pub struct UnlockedKey {
    bytes: Mutex<Option<Zeroizing<[u8; 32]>>>,
    root: Arc<ConfigRoot>,
    source: root::FileLease,
}
impl fmt::Debug for UnlockedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UnlockedPrivacyKey([private])")
    }
}
impl UnlockedKey {
    fn new(bytes: Zeroizing<[u8; 32]>, root: Arc<ConfigRoot>, source: root::FileLease) -> Self {
        Self {
            bytes: Mutex::new(Some(bytes)),
            root,
            source,
        }
    }
    pub fn close(&self) {
        if let Ok(mut key) = self.bytes.lock() {
            *key = None;
        }
    }
}
impl HostKeySource for UnlockedKey {
    fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
        if self
            .root
            .recheck()
            .and_then(|_| self.source.recheck())
            .is_err()
        {
            self.close();
            return Err(crate::privacy_vault::PrivacyError::Key);
        }
        let key = self
            .bytes
            .lock()
            .map_err(|_| crate::privacy_vault::PrivacyError::Key)?;
        let bytes = key
            .as_ref()
            .ok_or(crate::privacy_vault::PrivacyError::Key)?;
        Ok(HostKey::supplied_by_host(Zeroizing::new(**bytes)))
    }
}

pub struct UnlockedVault {
    key: UnlockedKey,
    vault: Vault,
}
impl fmt::Debug for UnlockedVault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UnlockedPrivacyVault([private])")
    }
}
impl UnlockedVault {
    pub fn vault(&self) -> &Vault {
        &self.vault
    }
    pub fn key_source(&self) -> &dyn HostKeySource {
        &self.key
    }
    pub fn close(&self) {
        self.vault.close();
        self.key.close();
    }
}
impl Drop for UnlockedVault {
    fn drop(&mut self) {
        self.close();
    }
}

struct WorkState {
    closed: bool,
    active: bool,
    attempts: u8,
    handles: Vec<JoinHandle<()>>,
}
/// Three attempts per owner, one worker at a time, no queue or reset API.
pub struct KeyManager {
    root: Arc<ConfigRoot>,
    state: Arc<Mutex<WorkState>>,
}
impl fmt::Debug for KeyManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivacyKeyManager([private])")
    }
}
impl KeyManager {
    pub fn files_present(&self) -> Result<(bool, bool)> {
        self.root.file_presence()
    }
    pub fn capture_actual() -> Result<Self> {
        Ok(Self::from_root(ConfigRoot::capture_actual()?))
    }
    pub fn from_root(root: ConfigRoot) -> Self {
        Self {
            root: Arc::new(root),
            state: Arc::new(Mutex::new(WorkState {
                closed: false,
                active: false,
                attempts: 0,
                handles: Vec::with_capacity(3),
            })),
        }
    }
    pub async fn initialize(&self, passphrase: Passphrase) -> Result<UnlockedVault> {
        self.perform(passphrase, Operation::Initialize).await
    }
    pub async fn unlock(&self, passphrase: Passphrase) -> Result<UnlockedVault> {
        self.perform(passphrase, Operation::Unlock).await
    }
    pub fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
    }

    /// The owner keeps pending handles if this wait is dropped. No interrupted
    /// KDF/IO, rollback, deadline or OS quiescence claim is made.
    pub async fn close_and_wait(&self) -> Result<()> {
        self.close();
        self.wait_started().await
    }
    async fn wait_started(&self) -> Result<()> {
        std::future::poll_fn(|cx| {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => return Poll::Ready(Err(KeyError::Worker)),
            };
            let mut index = 0;
            let mut failed = false;
            while index < state.handles.len() {
                match Pin::new(&mut state.handles[index]).poll(cx) {
                    Poll::Ready(result) => {
                        failed |= result.is_err();
                        drop(state.handles.swap_remove(index));
                    }
                    Poll::Pending => index += 1,
                }
            }
            if failed {
                return Poll::Ready(Err(KeyError::Worker));
            }
            if state.handles.is_empty() && !state.active {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
        .await
    }

    async fn perform(&self, passphrase: Passphrase, operation: Operation) -> Result<UnlockedVault> {
        let runtime = Handle::try_current().map_err(|_| KeyError::Worker)?;
        let (sender, receiver) = oneshot::channel();
        let mut state = self.state.lock().map_err(|_| KeyError::Closed)?;
        if state.closed {
            return Err(KeyError::Closed);
        }
        if state.active || state.handles.iter().any(|handle| !handle.is_finished()) {
            return Err(KeyError::Busy);
        }
        if state.attempts >= 3 {
            state.closed = true;
            return Err(KeyError::Attempts);
        }
        state.attempts += 1;
        state.active = true;
        let shared = self.state.clone();
        let root = self.root.clone();
        let handle = tokio::task::spawn_blocking(move || {
            let _activity = Activity(shared.clone());
            let result = execute(root, &shared, passphrase, operation, runtime);
            let result = if check_open(&shared).is_ok() {
                result
            } else {
                drop(result);
                Err(KeyError::Closed)
            };
            let _ = sender.send(result);
        });
        state.handles.push(handle);
        drop(state);
        let mut waiter = Waiter {
            state: self.state.clone(),
            accepted: false,
        };
        let result = receiver.await.map_err(|_| KeyError::Worker)?;
        self.wait_started().await?;
        if check_open(&self.state).is_err() {
            drop(result);
            return Err(KeyError::Closed);
        }
        waiter.accepted = true;
        result
    }
}
impl Drop for KeyManager {
    fn drop(&mut self) {
        self.close();
    }
}
#[derive(Clone, Copy)]
enum Operation {
    Initialize,
    Unlock,
}
struct Activity(Arc<Mutex<WorkState>>);
impl Drop for Activity {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.lock() {
            state.active = false;
        }
    }
}
struct Waiter {
    state: Arc<Mutex<WorkState>>,
    accepted: bool,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        if !self.accepted
            && let Ok(mut state) = self.state.lock()
        {
            state.closed = true;
        }
    }
}
fn check_open(state: &Mutex<WorkState>) -> Result<()> {
    if state.lock().map_err(|_| KeyError::Closed)?.closed {
        Err(KeyError::Closed)
    } else {
        Ok(())
    }
}
fn execute(
    root: Arc<ConfigRoot>,
    state: &Mutex<WorkState>,
    passphrase: Passphrase,
    operation: Operation,
    runtime: Handle,
) -> Result<UnlockedVault> {
    check_open(state)?;
    let (bytes, lease) = match operation {
        Operation::Initialize => {
            root.ensure_absent_pair()?;
            let (envelope, key) = envelope::Envelope::create(&passphrase)?;
            check_open(state)?;
            let lease = root.create_envelope(envelope.as_bytes())?;
            (key, lease)
        }
        Operation::Unlock => {
            let (bytes, lease) = root.read_envelope()?;
            let key = envelope::Envelope::parse(bytes)?.unlock(&passphrase)?;
            (key, lease)
        }
    };
    check_open(state)?;
    let key = UnlockedKey::new(bytes, root.clone(), lease);
    let location = root.vault_location()?;
    let opened = match operation {
        Operation::Initialize => {
            runtime.block_on(Vault::create_new(location, &key, Limits::default()))
        }
        Operation::Unlock => {
            runtime.block_on(Vault::open_existing(location, &key, Limits::default()))
        }
    }
    .map_err(|_| KeyError::Vault)?;
    let result = UnlockedVault { key, vault: opened };
    root.recheck()?;
    check_open(state)?;
    Ok(result)
}

#[cfg(test)]
#[path = "privacy_key/tests.rs"]
mod tests;
