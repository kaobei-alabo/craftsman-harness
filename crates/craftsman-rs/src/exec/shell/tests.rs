//! Pipe admission/EOF tests with synthetic bytes; no fake builder scope.
use super::*;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn original_read_ceiling_is_not_natural_eof_and_never_releases_a_cut_key() {
    let redactor = Redactor::from_values(vec![b"credential-value".to_vec()]).unwrap();
    let capture = Arc::new(Mutex::new(Capture::new(&redactor, 100, 100)));
    let (mut writer, reader) = tokio::io::duplex(128);
    let input = format!("{}credential-value after", "q".repeat(40));
    writer.write_all(input.as_bytes()).await.unwrap();
    drop(writer);
    pump(reader, capture.clone(), 48).await.unwrap();
    let view = capture.lock().unwrap().finish(true);
    assert_eq!(view.raw_bytes, 48);
    assert!(view.omitted && view.fault.is_none());
    assert!(!view.head_text().unwrap().contains("credent"));
}
#[tokio::test]
async fn actual_eof_releases_only_verified_completed_output() {
    let redactor = Redactor::from_values(vec![b"credential-value".to_vec()]).unwrap();
    let capture = Arc::new(Mutex::new(Capture::new(&redactor, 100, 100)));
    let (mut writer, reader) = tokio::io::duplex(128);
    writer.write_all(b"credential-value tail").await.unwrap();
    drop(writer);
    pump(reader, capture.clone(), 128).await.unwrap();
    let view = capture.lock().unwrap().finish(true);
    assert_eq!(view.raw_bytes, 21);
    assert!(!view.omitted);
    assert_eq!(view.head_text().unwrap(), "[REDACTED] tail");
}
#[tokio::test]
async fn read_errors_are_typed_capture_failure_not_echoed_os_details() {
    struct Failing;
    impl tokio::io::AsyncRead for Failing {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("synthetic-sensitive-error")))
        }
    }
    let redactor = Redactor::from_values(vec![b"synthetic-sensitive-error".to_vec()]).unwrap();
    let capture = Arc::new(Mutex::new(Capture::new(&redactor, 100, 100)));
    assert!(pump(Failing, capture.clone(), 128).await.is_err());
    let view = capture.lock().unwrap().finish(false);
    assert_eq!(view.fault, Some(Fault::Read));
    assert_eq!(view.raw_bytes, 0);
    assert!(view.head_text().unwrap().is_empty());
}
