//! Small shared helpers.

use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

pub fn one_line(s: &str, limit: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        flat
    } else {
        let cut: String = flat.chars().take(limit.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Work on the runtime's blocking pool, spawned through one closure type.
///
/// `tokio::task::spawn_blocking` is compiled again, with the runtime's task
/// code behind it, for every distinct closure it is given. A site that only
/// awaits the work and maps a failed join to its own fixed error does not
/// need that: [`blocking`] hands every such closure over as the same boxed
/// closure, so that code exists once for all of them.
///
/// Nothing else differs from a direct `spawn_blocking` at the same place:
/// the work is spawned when this is called, not when it is first polled; it
/// runs the same closure on the same pool; a panic in it is a failed join;
/// and dropping the handle leaves the started work running to its return.
pub(crate) fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Blocking<T> {
    let erased: ErasedWork = Box::new(move || Box::new(work()) as Box<dyn Any + Send>);
    Blocking {
        handle: tokio::task::spawn_blocking(erased),
        returns: PhantomData,
    }
}

type ErasedWork = Box<dyn FnOnce() -> Box<dyn Any + Send> + Send>;

/// The handle of work started by [`blocking`]. Awaiting it gives what the
/// closure returned, or [`Unfinished`] where awaiting a `JoinHandle` gives a
/// join error.
pub(crate) struct Blocking<T> {
    handle: tokio::task::JoinHandle<Box<dyn Any + Send>>,
    returns: PhantomData<fn() -> T>,
}

/// The blocking work did not return: it panicked or the runtime is shutting
/// down. It carries nothing; each site reports its own fixed error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Unfinished;

impl<T: 'static> Future for Blocking<T> {
    type Output = Result<T, Unfinished>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let joined = match Pin::new(&mut self.handle).poll(cx) {
            Poll::Ready(joined) => joined,
            Poll::Pending => return Poll::Pending,
        };
        // The box holds what this very closure returned, so it is a `T`.
        // Anything else is reported like a join that failed.
        Poll::Ready(match joined {
            Ok(returned) => match returned.downcast::<T>() {
                Ok(value) => Ok(*value),
                Err(_) => Err(Unfinished),
            },
            Err(_) => Err(Unfinished),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blocking_work_starts_when_called_returns_its_own_value_and_reports_a_panic() {
        // Started at the call: the work runs although its handle is never
        // polled, and runs to its return after the handle is dropped.
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let entered = gate.clone();
        let (done, finished) = std::sync::mpsc::channel::<()>();
        drop(blocking(move || {
            entered.wait();
            done.send(()).unwrap();
        }));
        gate.wait();
        finished
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the work returned after its handle was dropped");

        // The closure's own value and type, whatever they are.
        assert_eq!(blocking(|| 7u8).await, Ok(7));
        let own: Result<Result<u8, &'static str>, Unfinished> =
            blocking(|| Err("the work's own error")).await;
        assert_eq!(own, Ok(Err("the work's own error")));
        // A panic in the work is a join that failed.
        let panicked: Result<u8, Unfinished> = blocking(|| panic!("fixture panic")).await;
        assert_eq!(panicked, Err(Unfinished));
    }

    #[test]
    fn one_line_flattens_and_truncates() {
        assert_eq!(one_line("a\n b\t c", 100), "a b c");
        let out = one_line(&"x".repeat(50), 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }
}
