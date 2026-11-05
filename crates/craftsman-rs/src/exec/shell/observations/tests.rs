use super::*;
use tokio::io::AsyncReadExt;

#[tokio::test]
async fn delegated_bytes_eof_and_fixed_diagnostics_do_not_retain_source_text() {
    let recorder = Recorder::default();
    let call = recorder.begin();
    let mut reader = call.reader(&b"synthetic-private-value"[..], false);
    let mut output = Vec::new();
    reader.read_to_end(&mut output).await.unwrap();
    assert_eq!(output, b"synthetic-private-value");
    let report = recorder.view().report();
    assert!(report.contains("stdout_bytes=23"), "{report}");
    assert!(report.contains("stdout_eof=true"), "{report}");
    assert!(!report.contains("synthetic-private-value"));
    assert!(!report.contains("/"));
    drop(reader);
    drop(call);
    assert!(recorder.view().report().contains("scope_end_us=Some("));
}

#[tokio::test]
async fn zero_capacity_read_does_not_fabricate_eof_or_received_bytes() {
    let recorder = Recorder::default();
    let call = recorder.begin();
    let mut reader = call.reader(&b"actual-data"[..], false);
    let mut storage = [];
    let mut empty = ReadBuf::new(&mut storage);
    std::future::poll_fn(|cx| Pin::new(&mut reader).poll_read(cx, &mut empty))
        .await
        .unwrap();
    assert!(recorder.view().report().contains("stdout_eof=false"));
    assert!(recorder.view().report().contains("stdout_bytes=0"));
    let mut actual = [0; 11];
    reader.read_exact(&mut actual).await.unwrap();
    assert_eq!(&actual, b"actual-data");
    assert!(recorder.view().report().contains("stdout_bytes=11"));
    assert!(recorder.view().report().contains("stdout_eof=false"));
}

#[test]
fn bounded_history_never_rebinds_an_evicted_call_to_a_new_entry() {
    let recorder = Recorder::default();
    let first = recorder.begin();
    for _ in 0..RETAINED_CALLS {
        drop(recorder.begin());
    }
    first.launch_returned(Some(999));
    let state = lock(&recorder.0);
    assert_eq!(state.entries.len(), RETAINED_CALLS);
    assert_eq!(state.omitted, 1);
    assert_eq!(state.entries.front().unwrap().sequence, 2);
    assert!(state.entries.iter().all(|entry| entry.pid.is_none()));
}

#[tokio::test]
async fn original_reader_errors_are_delegated_without_diagnostic_error_strings() {
    struct Failed;
    impl AsyncRead for Failed {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "private-error-text",
            )))
        }
    }
    let recorder = Recorder::default();
    let call = recorder.begin();
    let mut reader = call.reader(Failed, true);
    let mut buf = [0; 1];
    assert_eq!(
        reader.read(&mut buf).await.unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    let report = recorder.view().report();
    assert!(report.contains("stderr_read_failed=true"));
    assert!(report.contains("stderr_eof=false"));
    assert!(!report.contains("private-error-text"));
}
