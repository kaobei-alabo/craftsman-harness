use super::*;
use crate::exec::shell::observations::Recorder;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::task::{Context, Wake, Waker};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Default)]
struct CountWake(AtomicUsize);
impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct Scripted {
    step: usize,
    original_waker: Waker,
    state: Arc<Mutex<State>>,
}
impl AsyncRead for Scripted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        assert!(
            cx.waker().will_wake(&self.original_waker),
            "the exact ORIGINAL reader waker must reach the inner poll"
        );
        assert!(
            self.state.try_lock().is_ok(),
            "no recorder lock around the inner poll"
        );
        self.step += 1;
        match self.step {
            1 => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            2 => {
                buffer.put_slice(b"DATA");
                Poll::Ready(Ok(()))
            }
            _ => Poll::Ready(Ok(())),
        }
    }
}

#[test]
fn original_pending_data_eof_context_buffer_result_and_wake_are_preserved() {
    let recorder = Recorder::default();
    let call = recorder.begin();
    let wake = Arc::new(CountWake::default());
    let waker = Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    let mut reader = call.reader(
        Scripted {
            step: 0,
            original_waker: waker.clone(),
            state: recorder.0.clone(),
        },
        false,
    );
    let mut storage = [0; 5];
    let mut buffer = ReadBuf::new(&mut storage);
    buffer.put_slice(b"P");
    assert!(
        Pin::new(&mut reader)
            .poll_read(&mut cx, &mut buffer)
            .is_pending()
    );
    assert_eq!(buffer.filled(), b"P");
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
    assert!(matches!(
        Pin::new(&mut reader).poll_read(&mut cx, &mut buffer),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(buffer.filled(), b"PDATA");
    buffer.clear();
    assert!(matches!(
        Pin::new(&mut reader).poll_read(&mut cx, &mut buffer),
        Poll::Ready(Ok(()))
    ));
    let progress = recorder.view().poll_progress_report_for_tests();
    assert!(
        progress.contains("stdout_polls=Some(3) stdout_pending_polls=Some(1)"),
        "{progress}"
    );
    assert!(progress.contains("stdout_last_poll_result=eof stdout_count_loss=false"));
    assert!(progress.contains("stderr_polls=Some(0) stderr_pending_polls=Some(0) stderr_last_poll_us=None stderr_last_poll_result=not-polled"));
    assert!(
        recorder
            .view()
            .report()
            .contains("stdout_bytes=4 stdout_eof=true")
    );
    assert!(!progress.contains("DATA"));
}

#[test]
fn empty_capacity_is_not_recorded_as_eof_data_or_pending() {
    let recorder = Recorder::default();
    let call = recorder.begin();
    let waker = Waker::from(Arc::new(CountWake::default()));
    let mut cx = Context::from_waker(&waker);
    let mut reader = call.reader(&b"original"[..], false);
    let mut empty = [];
    let mut buffer = ReadBuf::new(&mut empty);
    assert!(matches!(
        Pin::new(&mut reader).poll_read(&mut cx, &mut buffer),
        Poll::Ready(Ok(()))
    ));
    let progress = recorder.view().poll_progress_report_for_tests();
    assert!(progress.contains("stdout_polls=Some(1) stdout_pending_polls=Some(0)"));
    assert!(progress.contains("stdout_last_poll_result=empty-capacity"));
    assert!(
        recorder
            .view()
            .report()
            .contains("stdout_bytes=0 stdout_eof=false")
    );
}

#[test]
fn original_read_error_is_returned_without_retaining_its_text() {
    struct Error;
    impl AsyncRead for Error {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "original-private-error",
            )))
        }
    }
    let recorder = Recorder::default();
    let call = recorder.begin();
    let mut reader = call.reader(Error, true);
    let waker = Waker::from(Arc::new(CountWake::default()));
    let mut cx = Context::from_waker(&waker);
    let mut storage = [0; 1];
    let mut buffer = ReadBuf::new(&mut storage);
    let Poll::Ready(Err(error)) = Pin::new(&mut reader).poll_read(&mut cx, &mut buffer) else {
        panic!("original error");
    };
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    assert_eq!(error.to_string(), "original-private-error");
    let progress = recorder.view().poll_progress_report_for_tests();
    assert!(progress.contains("stderr_polls=Some(1) stderr_pending_polls=Some(0)"));
    assert!(progress.contains("stderr_last_poll_result=error"));
    assert!(!progress.contains("original-private-error"));
    assert!(buffer.filled().is_empty());
}

#[test]
fn counter_overflow_stays_unknown_instead_of_reset_refund_or_saturation() {
    let mut read = ReadProgress {
        polls: Some(u64::MAX),
        pending: Some(u64::MAX),
        ..Default::default()
    };
    read.observe(7, &Poll::Pending, false, true);
    assert_eq!(read.polls, None);
    assert_eq!(read.pending, None);
    assert_eq!(read.last_us, Some(7));
    read.observe(8, &Poll::Ready(Ok(())), true, true);
    assert_eq!(read.polls, None);
    assert_eq!(read.pending, None);
    assert_eq!(read.last_us, Some(8));
    assert_eq!(read.last.map(ReadReturn::label), Some("data"));
}

#[test]
fn selection_retains_first_real_enter_return_and_labels_duplicate_loss() {
    let mut selection = Selection::default();
    selection.enter(10);
    selection.returned(20, Selected::ReturnedOk);
    selection.enter(30);
    selection.returned(40, Selected::Timeout);
    assert_eq!(selection.enter_us, Some(10));
    assert_eq!(selection.return_us, Some(20));
    assert_eq!(selection.result.map(Selected::label), Some("returned-ok"));
    assert!(selection.loss);
}

#[test]
fn return_without_selection_entry_is_unknown_not_fabricated_completion() {
    let mut selection = Selection::default();
    selection.returned(20, Selected::ReturnedStatus);
    assert!(selection.loss);
    assert!(
        selection.enter_us.is_none() && selection.return_us.is_none() && selection.result.is_none()
    );
}

#[test]
fn actual_call_selection_branches_are_fixed_history_not_os_wait_polling() {
    let recorder = Recorder::default();
    let call = recorder.begin();
    call.readers_selected();
    call.readers_returned(false, false, Some(ToolErrorKind::NeedsApproval));
    let progress = recorder.view().poll_progress_report_for_tests();
    assert!(progress.contains("readers_result=approval-branch readers_phase_loss=false"));
    assert!(progress.contains("wait_enter_us=None wait_return_us=None wait_result=not-returned"));
    assert!(progress.contains("stdout_polls=Some(0)"));
    assert_eq!(
        Selected::stop(Some(ToolErrorKind::DeniedCapability)).label(),
        "other-stop-branch"
    );
    assert_eq!(
        Selected::stop(Some(ToolErrorKind::Timeout)).label(),
        "timeout-branch"
    );
}

#[test]
fn evicted_call_progress_does_not_rebind_to_new_history() {
    let recorder = Recorder::default();
    let first = recorder.begin();
    for _ in 0..super::super::RETAINED_CALLS {
        drop(recorder.begin());
    }
    first.readers_selected();
    first.readers_returned(true, true, None);
    let state = lock(&recorder.0);
    assert_eq!(state.entries.len(), super::super::RETAINED_CALLS);
    assert_eq!(state.omitted, 1);
    assert!(
        state
            .entries
            .iter()
            .all(|entry| entry.progress.readers.enter_us.is_none())
    );
}

#[test]
fn full_progress_section_refuses_whole_overbound_history_without_clipping_old_facts() {
    let recorder = Recorder::default();
    for _ in 0..super::super::RETAINED_CALLS {
        drop(recorder.begin());
    }
    {
        let mut state = lock(&recorder.0);
        for entry in &mut state.entries {
            for read in &mut entry.progress.reads {
                read.polls = Some(u64::MAX);
                read.pending = Some(u64::MAX);
                read.last_us = Some(u64::MAX);
                read.last = Some(ReadReturn::Eof);
            }
        }
    }
    let progress = recorder.view().poll_progress_report_for_tests();
    assert_eq!(progress, "native-shell-test-poll-progress=refused");
    assert!(
        recorder
            .view()
            .report()
            .contains("calls=8 omitted=0 loss=false")
    );
    assert!(!progress.contains("call=1"));
}

#[test]
fn bounded_writer_admits_exact_ceiling_and_never_copies_an_overbound_suffix() {
    let mut out = Bounded(String::with_capacity(MAX_REPORT_BYTES));
    out.write_str(&"a".repeat(MAX_REPORT_BYTES)).unwrap();
    assert!(out.write_str("b").is_err());
    assert_eq!(out.0.len(), MAX_REPORT_BYTES);
    assert!(!out.0.contains('b'));
    let recorder = Recorder::default();
    drop(recorder.begin());
    let progress = recorder.view().poll_progress_report_for_tests();
    assert!(progress.len() <= MAX_REPORT_BYTES);
    assert!(
        progress
            .bytes()
            .all(|b| b == b'\n' || (32..=126).contains(&b))
    );
    assert!(progress.contains("clock=call"));
}

#[cfg(target_os = "macos")]
#[test]
fn pre_stop_poll_progress_is_original_history_not_later_drain_or_a_pid_query() {
    struct PendingThenEof(bool);
    impl AsyncRead for PendingThenEof {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.0 {
                Poll::Ready(Ok(()))
            } else {
                self.0 = true;
                Poll::Pending
            }
        }
    }
    let recorder = Recorder::default();
    let call = recorder.begin();
    call.readers_selected();
    let mut reader = call.reader(PendingThenEof(false), false);
    let waker = Waker::from(Arc::new(CountWake::default()));
    let mut cx = Context::from_waker(&waker);
    let mut storage = [0; 1];
    let mut buffer = ReadBuf::new(&mut storage);
    assert!(
        Pin::new(&mut reader)
            .poll_read(&mut cx, &mut buffer)
            .is_pending()
    );
    call.readers_returned(false, false, Some(ToolErrorKind::Timeout));
    assert!(
        super::super::snapshots::note_before_stop(&call, None, Some(ToolErrorKind::Timeout))
            .is_none()
    );
    assert!(matches!(
        Pin::new(&mut reader).poll_read(&mut cx, &mut buffer),
        Poll::Ready(Ok(()))
    ));
    let state = lock(&recorder.0);
    let entry = state.entries.back().unwrap();
    assert!(
        entry.native_snapshot.is_none(),
        "missing actual PID never authorizes a native query"
    );
    let before = &entry.before_stop.as_ref().unwrap().progress.reads[0];
    assert_eq!(before.polls, Some(1));
    assert_eq!(before.last.map(ReadReturn::label), Some("pending"));
    assert_eq!(entry.progress.reads[0].polls, Some(2));
    assert_eq!(
        entry.progress.reads[0].last.map(ReadReturn::label),
        Some("eof")
    );
    let progress = report(&state);
    assert!(progress.contains("poll_progress_phase=before-stop-history stdout_polls=Some(1)"));
    assert!(progress.contains("poll_progress_phase=current-history stdout_polls=Some(2)"));
}
