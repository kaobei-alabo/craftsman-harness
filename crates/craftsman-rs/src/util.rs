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

/// `tokio::task::spawn_blocking` for a site that reports or inspects a failed
/// join, through the same one closure type as [`blocking`]. Awaiting what it
/// returns gives what the closure returned or tokio's own `JoinError` for
/// that very work. It is not a `JoinHandle`: a site that stores, aborts or
/// polls the handle keeps the direct call.
pub(crate) fn spawn_blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Joined<T> {
    Joined(blocking(work))
}

/// The handle of work started by [`spawn_blocking`].
pub(crate) struct Joined<T>(Blocking<T>);

impl<T: 'static> Future for Joined<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let joined = match Pin::new(&mut self.0.handle).poll(cx) {
            Poll::Ready(joined) => joined,
            Poll::Pending => return Poll::Pending,
        };
        Poll::Ready(joined.map(|returned| match returned.downcast::<T>() {
            Ok(value) => *value,
            Err(_) => unreachable!("the box holds what this very closure returned"),
        }))
    }
}

/// A task that returns nothing, spawned through one future type.
///
/// `tokio::spawn` is compiled again, with the runtime's task code behind it,
/// for every distinct future it is given. [`spawn_unit`] hands every such
/// future over as the same boxed future, so that code exists once for all of
/// them.
///
/// Nothing else differs from a direct `tokio::spawn` at the same place: the
/// task is spawned when this is called, on the same runtime, and what comes
/// back is that task's own `JoinHandle<()>`. Joining it, aborting it,
/// detaching it by drop and the join error of a panic are tokio's, unchanged.
#[track_caller]
pub(crate) fn spawn_unit(
    task: impl Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    let erased: UnitTask = Box::pin(task);
    tokio::spawn(erased)
}

type UnitTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// [`spawn_unit`] on a runtime the caller already holds a handle of.
///
/// The future goes to that very handle's own `spawn`, as the same boxed
/// future, when this is called. What comes back is that task's own
/// `JoinHandle<()>`. Nothing looks for another runtime and no entered one is
/// needed; joining, aborting, detaching by drop and the join error of a
/// panic are tokio's, unchanged.
#[track_caller]
pub(crate) fn spawn_unit_on(
    runtime: &tokio::runtime::Handle,
    task: impl Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    let erased: UnitTask = Box::pin(task);
    runtime.spawn(erased)
}

/// A task of a `JoinSet`, spawned through one future type for its output.
///
/// `JoinSet::spawn` is compiled again, with the runtime's task code behind
/// it, for every distinct future it is given. [`spawn_in`] hands the future
/// to that very set's own `spawn` as the same boxed future, so that code
/// exists once for every task with that output.
///
/// Nothing else differs from a direct `set.spawn(..)` at the same place: the
/// task is spawned when this is called, on the same runtime, into the same
/// set, and what comes back is that task's own `AbortHandle`. The set owns
/// the task as before: its `join_next` gives this task's return or tokio's
/// own join error for it, and dropping the set aborts it.
#[track_caller]
pub(crate) fn spawn_in<T: Send + 'static>(
    set: &mut tokio::task::JoinSet<T>,
    task: impl Future<Output = T> + Send + 'static,
) -> tokio::task::AbortHandle {
    set.spawn(erased(task))
}

/// All that [`spawn_in`] does to a task before the set has it: the same
/// future, boxed. Awaiting it gives what the task returned.
fn erased<T>(
    task: impl Future<Output = T> + Send + 'static,
) -> Pin<Box<dyn Future<Output = T> + Send + 'static>> {
    Box::pin(task)
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

    #[tokio::test]
    async fn spawned_blocking_work_returns_its_own_value_or_tokios_own_join_error() {
        // Started at the call, like `blocking`: the work has run before its
        // handle is first polled.
        let (done, finished) = std::sync::mpsc::channel::<()>();
        let handle = spawn_blocking(move || {
            done.send(()).unwrap();
            7u8
        });
        finished
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the work ran before its handle was polled");
        assert!(matches!(handle.await, Ok(7)));
        let own: Result<Result<u8, &'static str>, tokio::task::JoinError> =
            spawn_blocking(|| Err("the work's own error")).await;
        assert!(matches!(own, Ok(Err("the work's own error"))));

        // A panic in the work is tokio's own join error: its kind, the text
        // a site prints for it and the panic's payload.
        let error = spawn_blocking(|| -> u8 { panic!("fixture panic") })
            .await
            .unwrap_err();
        assert!(error.is_panic() && !error.is_cancelled());
        assert_eq!(
            error.to_string(),
            format!(
                "task {} panicked with message \"fixture panic\"",
                error.id()
            )
        );
        assert_eq!(
            error.into_panic().downcast_ref::<&str>(),
            Some(&"fixture panic")
        );
    }

    #[tokio::test]
    async fn a_unit_task_starts_when_spawned_and_its_handle_is_the_tasks_own() {
        use tokio::sync::oneshot;

        // Spawned at the call: the task runs while its handle is held and has
        // not been polled. The handle is the handle of that very task, and
        // joining it gives the task's return.
        let (ran, seen) = oneshot::channel();
        let handle: tokio::task::JoinHandle<()> = spawn_unit(async move {
            ran.send(tokio::task::id()).unwrap();
        });
        let inside = tokio::time::timeout(std::time::Duration::from_secs(30), seen)
            .await
            .expect("the task ran without a poll of its handle")
            .unwrap();
        assert_eq!(inside, handle.id());
        assert!(matches!(handle.await, Ok(())));

        // A dropped handle detaches the task; it still runs to its return.
        let (ran, seen) = oneshot::channel();
        drop(spawn_unit(async move {
            tokio::task::yield_now().await;
            ran.send(()).unwrap();
        }));
        seen.await
            .expect("the task returned after its handle was dropped");

        // A panic is tokio's own join error and carries the panic's payload.
        let error = spawn_unit(async {
            panic!("fixture panic");
        })
        .await
        .unwrap_err();
        assert!(error.is_panic() && !error.is_cancelled());
        assert_eq!(
            error.into_panic().downcast_ref::<&str>(),
            Some(&"fixture panic")
        );

        // An abort cancels the task where it waits and drops what it holds.
        let (held, released) = oneshot::channel::<()>();
        let (started, entered) = oneshot::channel();
        let handle = spawn_unit(async move {
            let _held = held;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        entered.await.unwrap();
        assert!(!handle.is_finished());
        handle.abort();
        let error = handle.await.unwrap_err();
        assert!(error.is_cancelled() && !error.is_panic());
        assert!(released.await.is_err());
    }

    #[test]
    fn one_line_flattens_and_truncates() {
        assert_eq!(one_line("a\n b\t c", 100), "a b c");
        let out = one_line(&"x".repeat(50), 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }

    #[tokio::test]
    async fn a_task_spawned_in_a_set_is_that_sets_own_task_with_its_own_abort_handle() {
        use tokio::sync::oneshot;
        let wait = std::time::Duration::from_secs(30);

        // Spawned at the call, into that set: the task runs although the set
        // has not been polled, the set counts it, and what came back is the
        // abort handle of that very task.
        let mut set = tokio::task::JoinSet::new();
        let (ran, seen) = oneshot::channel();
        let handle: tokio::task::AbortHandle = spawn_in(&mut set, async move {
            ran.send(tokio::task::id()).unwrap();
        });
        assert_eq!(set.len(), 1);
        let inside = tokio::time::timeout(wait, seen)
            .await
            .expect("the task ran without a poll of its set")
            .unwrap();
        assert_eq!(inside, handle.id());
        // The set completes with that task's id and its return.
        let (joined, ()) = set.join_next_with_id().await.unwrap().unwrap();
        assert_eq!(joined, handle.id());
        assert!(handle.is_finished() && set.is_empty());

        // A panic is tokio's own join error for that task, with its payload.
        let handle = spawn_in(&mut set, async {
            panic!("fixture panic");
        });
        let error = set.join_next().await.unwrap().unwrap_err();
        assert!(error.is_panic() && !error.is_cancelled());
        assert_eq!(error.id(), handle.id());
        assert_eq!(
            error.into_panic().downcast_ref::<&str>(),
            Some(&"fixture panic")
        );

        // The handle aborts its own task where it waits, and no other. The
        // set reports the cancellation, and what the task held is dropped.
        let (held, released) = oneshot::channel::<()>();
        let (started, entered) = oneshot::channel();
        let aborted = spawn_in(&mut set, async move {
            let _held = held;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let (other_held, other_released) = oneshot::channel::<()>();
        let (other_started, other_entered) = oneshot::channel();
        let kept = spawn_in(&mut set, async move {
            let _held = other_held;
            other_started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        entered.await.unwrap();
        other_entered.await.unwrap();
        assert_ne!(aborted.id(), kept.id());
        aborted.abort();
        let error = set.join_next().await.unwrap().unwrap_err();
        assert!(error.is_cancelled() && !error.is_panic());
        assert_eq!(error.id(), aborted.id());
        assert!(released.await.is_err());
        assert!(!kept.is_finished());
        assert_eq!(set.len(), 1);

        // The set still owns what it was given: dropping it aborts the task
        // that is left, which drops what that task holds.
        drop(set);
        assert!(
            tokio::time::timeout(wait, other_released)
                .await
                .expect("dropping the set aborted its task")
                .is_err()
        );
        assert!(kept.is_finished());
    }

    #[tokio::test]
    async fn a_unit_task_on_a_supplied_handle_runs_on_that_runtime_with_its_own_join_handle() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::sync::oneshot;
        let wait = std::time::Duration::from_secs(30);
        // This test's own runtime is the one entered here. The supplied one
        // is another, with one worker thread that carries its name.
        let supplied = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("owned-supplied-runtime")
            .build()
            .unwrap();
        let handle = supplied.handle().clone();
        let on_supplied = || std::thread::current().name() == Some("owned-supplied-runtime");
        assert!(!on_supplied());

        // Spawned at the call, on the supplied runtime and not on the one
        // that is entered: the task runs on the supplied worker although its
        // handle has not been polled, and what came back is the join handle
        // of that very task.
        let there = Arc::new(AtomicBool::new(false));
        let seen_there = there.clone();
        let (ran, seen) = oneshot::channel();
        let join: tokio::task::JoinHandle<()> = spawn_unit_on(&handle, async move {
            seen_there.store(on_supplied(), Ordering::SeqCst);
            ran.send(tokio::task::id()).unwrap();
        });
        let inside = tokio::time::timeout(wait, seen)
            .await
            .expect("the task ran without a poll of its handle")
            .unwrap();
        assert_eq!(inside, join.id());
        assert!(there.load(Ordering::SeqCst));
        assert!(matches!(join.await, Ok(())));

        // A dropped handle detaches the task; it still runs to its return.
        let (ran, seen) = oneshot::channel();
        drop(spawn_unit_on(&handle, async move {
            tokio::task::yield_now().await;
            ran.send(()).unwrap();
        }));
        seen.await
            .expect("the task returned after its handle was dropped");

        // A panic is tokio's own join error and carries the panic's payload.
        let error = spawn_unit_on(&handle, async {
            panic!("fixture panic");
        })
        .await
        .unwrap_err();
        assert!(error.is_panic() && !error.is_cancelled());
        assert_eq!(
            error.into_panic().downcast_ref::<&str>(),
            Some(&"fixture panic")
        );

        // An abort cancels the task where it waits and drops what it holds.
        let (held, released) = oneshot::channel::<()>();
        let (started, entered) = oneshot::channel();
        let join = spawn_unit_on(&handle, async move {
            let _held = held;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        entered.await.unwrap();
        assert!(!join.is_finished());
        join.abort();
        let error = join.await.unwrap_err();
        assert!(error.is_cancelled() && !error.is_panic());
        assert!(released.await.is_err());
        supplied.shutdown_background();
    }

    #[test]
    fn the_boxed_future_of_a_set_task_returns_what_the_task_returned() {
        // Boxing is all the helper does to a task. Polled as it is, with no
        // runtime behind it, the boxed future gives the task's own value, of
        // its own type, an error of the task's own included.
        let mut context = Context::from_waker(std::task::Waker::noop());
        let mut task = erased(async { Ok::<u8, &'static str>(7) });
        assert_eq!(task.as_mut().poll(&mut context), Poll::Ready(Ok(7)));
        let mut task = erased(async { Err::<u8, &'static str>("the task's own error") });
        assert_eq!(
            task.as_mut().poll(&mut context),
            Poll::Ready(Err("the task's own error"))
        );
    }
}
