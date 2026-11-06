//! Opted-in turn cancellation. Foreground work shares a turn; background work
//! receives a fresh turn under the same session shutdown owner.
use crate::{AgentError, ModelResponse};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

#[derive(Default)]
struct Signal {
    stopped: AtomicBool,
    changed: Notify,
}

impl Signal {
    fn cancel(&self) {
        self.stopped.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }
    fn is_cancelled(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    async fn cancelled(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            changed.await;
        }
    }
}

/// A session's permanent shutdown latch. Cancelling it stops existing and
/// future opted-in turns, including background children, without aborting tools.
#[derive(Clone, Default)]
pub struct SessionCancellation {
    signal: Arc<Signal>,
    parent: Option<Arc<SessionCancellation>>,
    depth: u8,
}

impl SessionCancellation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.signal.cancel();
    }
    pub fn is_cancelled(&self) -> bool {
        self.signal.is_cancelled()
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_cancelled())
    }
    pub async fn cancelled(&self) {
        match &self.parent {
            Some(parent) => tokio::select! {
                _ = self.signal.cancelled() => {},
                _ = Box::pin(parent.cancelled()) => {},
            },
            None => self.signal.cancelled().await,
        }
    }
    /// A separately closable session under this shutdown owner. Parent depth is
    /// bounded; the terminal uses one level for each session generation.
    pub fn child(&self) -> Result<Self, AgentError> {
        if self.depth >= 32 {
            return Err(AgentError::State(
                "cancellation scope depth exceeded".into(),
            ));
        }
        Ok(Self {
            signal: Arc::new(Signal::default()),
            parent: Some(Arc::new(self.clone())),
            depth: self.depth + 1,
        })
    }
    pub fn new_turn(&self) -> TurnCancellation {
        TurnCancellation {
            turn: Arc::new(Signal::default()),
            session: self.clone(),
        }
    }
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.signal, &other.signal)
    }
}

/// Stop authority for one actual turn. Native tools observe this latch and
/// finish their own kill/reap/drain path; it never force-drops a tool future.
#[derive(Clone)]
pub struct TurnCancellation {
    turn: Arc<Signal>,
    session: SessionCancellation,
}

tokio::task_local! { static CURRENT: Option<TurnCancellation>; }

impl TurnCancellation {
    /// The opted-in turn on this task. Tokio spawns do not inherit task locals.
    pub fn current() -> Option<Self> {
        CURRENT.try_with(Clone::clone).ok().flatten()
    }
    pub fn cancel(&self) {
        self.turn.cancel();
    }
    pub fn is_cancelled(&self) -> bool {
        self.turn.is_cancelled() || self.session.is_cancelled()
    }
    pub fn session(&self) -> SessionCancellation {
        self.session.clone()
    }
    /// Independent of the parent's ordinary stop; session shutdown still stops it.
    pub fn background(&self) -> Self {
        self.session.new_turn()
    }
    pub async fn cancelled(&self) {
        tokio::select! { _ = self.turn.cancelled() => {}, _ = self.session.cancelled() => {} }
    }
    pub async fn scope<F: Future>(&self, work: F) -> F::Output {
        CURRENT.scope(Some(self.clone()), work).await
    }
}

pub(crate) async fn unscoped<F: Future>(work: F) -> F::Output {
    CURRENT.scope(None, work).await
}

/// Model sampling is safe to drop only in an explicitly opted-in scope. Tools
/// and mutation/persistence hooks must continue to own their cleanup instead.
pub(crate) async fn model<F>(work: F) -> Result<ModelResponse, AgentError>
where
    F: Future<Output = Result<ModelResponse, AgentError>>,
{
    match TurnCancellation::current() {
        Some(turn) => tokio::select! {
            biased;
            _ = turn.cancelled() => Err(AgentError::Cancelled),
            result = work => result,
        },
        None => work.await,
    }
}
