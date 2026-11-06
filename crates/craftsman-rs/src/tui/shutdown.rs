//! Owned signal routing, installed before terminal entry or agent construction.
use craftsman_harness::SessionCancellation;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

pub(super) struct Shutdown {
    pub(super) cancellation: SessionCancellation,
    code: Arc<AtomicI32>,
    listener: tokio::task::JoinHandle<()>,
}

impl Shutdown {
    pub(super) fn install() -> std::io::Result<Self> {
        let cancellation = SessionCancellation::new();
        let stopped = cancellation.clone();
        let code = Arc::new(AtomicI32::new(0));
        let received = code.clone();
        #[cfg(unix)]
        let listener = {
            use tokio::signal::unix::{SignalKind, signal};
            let mut interrupt = signal(SignalKind::interrupt())?;
            let mut terminate = signal(SignalKind::terminate())?;
            let mut hangup = signal(SignalKind::hangup())?;
            tokio::spawn(async move {
                let exit = tokio::select! {
                    _ = interrupt.recv() => 130,
                    _ = terminate.recv() => 143,
                    _ = hangup.recv() => 129,
                };
                received.store(exit, Ordering::Release);
                stopped.cancel();
            })
        };
        #[cfg(windows)]
        let listener = {
            use tokio::signal::windows;
            let mut interrupt = windows::ctrl_c()?;
            let mut stop = windows::ctrl_break()?;
            let mut close = windows::ctrl_close()?;
            let mut logoff = windows::ctrl_logoff()?;
            let mut shutdown = windows::ctrl_shutdown()?;
            tokio::spawn(async move {
                tokio::select! {
                    _ = interrupt.recv() => {}, _ = stop.recv() => {},
                    _ = close.recv() => {}, _ = logoff.recv() => {}, _ = shutdown.recv() => {},
                }
                received.store(130, Ordering::Release);
                stopped.cancel();
            })
        };
        Ok(Self {
            cancellation,
            code,
            listener,
        })
    }
    pub(super) fn exit_code(&self) -> i32 {
        self.code.load(Ordering::Acquire)
    }
}

impl Drop for Shutdown {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.listener.abort();
    }
}

#[cfg(all(test, unix))]
#[path = "shutdown/tests.rs"]
mod tests;
