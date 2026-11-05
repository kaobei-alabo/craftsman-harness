//! Actual-builder/client/HTTP diagnostic fixtures, never fake receipts/seeds.
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::provider::diagnostics::{Diagnostic, Failure, Kind};
use crate::exec::provider::request::Source;
use craftsman_harness::ToolErrorKind;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    server: MockServer,
}
struct Harness {
    session: Session,
    client: LlmClient,
    twin: LlmClient,
    source: Arc<Source>,
    provider: Arc<crate::exec::provider::Provider>,
    filesystem: Arc<crate::exec::confined_fs::TextFs>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let config = root.path().join("selected");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        Self {
            _root: root,
            workspace,
            config,
            server: MockServer::start().await,
        }
    }
    async fn harness(&self, extra: Value) -> Harness {
        let mut config = json!({"model":"synthetic","api_key":"synthetic-secret",
            "search_api_key":"search-secret", "base_url":self.server.uri()});
        config
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        std::fs::write(self.config.join("config.json"), config.to_string()).unwrap();
        let prepared = Prepared::prepare_with_environment_for_tests(
            Options {
                workspace: self.workspace.clone(),
                config_dir: self.config.clone(),
                session: SessionSelection::Ephemeral,
                trust_workspace: false,
                selectors: Selectors {
                    allow: vec!["tool:write_file".into()],
                    on_unapproved: OnUnapproved::Fail,
                    ..Default::default()
                },
            },
            BTreeMap::new(),
        )
        .await
        .unwrap();
        let (session, gate) = build_prepared_session(&prepared).await.unwrap();
        let model = prepared.config().model.as_deref().unwrap();
        let mut options = prepared.config().model_profile(model);
        if let Some(v) = prepared
            .config()
            .extra
            .get("model_kwargs")
            .and_then(Value::as_object)
        {
            options.extend(v.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        let agents = vec!["general-purpose".into()];
        let scope = || ProviderScope {
            prepared: &prepared,
            gate: gate.clone(),
            model,
            options: &options,
            usage: session.usage.clone(),
            agents: &agents,
        };
        let client = LlmClient::headless(scope()).unwrap();
        let twin = LlmClient::headless(scope()).unwrap();
        let source = client.request_source_for_tests().unwrap();
        let provider = Arc::new(crate::exec::provider::Provider::from_scope(scope()).unwrap());
        let filesystem = Arc::new(crate::exec::confined_fs::TextFs::from_scope(
            FilesystemScope {
                prepared: &prepared,
                gate,
            },
        ));
        Harness {
            session,
            client,
            twin,
            source,
            provider,
            filesystem,
        }
    }
    async fn error(&self, status: u16, bytes: impl Into<Vec<u8>>) {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_raw(bytes.into(), "text/plain; charset=utf-8"),
            )
            .mount(&self.server)
            .await;
    }
    fn no_store(&self) {
        assert!(
            !self.config.join("threads.db").exists() && !self.config.join("sessions-rs").exists()
        );
    }
}
fn request() -> ModelRequest {
    ModelRequest {
        system: "safe".into(),
        messages: vec![],
        tools: vec![],
    }
}
async fn failure(h: &Harness) -> Arc<Diagnostic> {
    let error = h.client.stream_model_turn(&request()).await.unwrap_err();
    assert_eq!(error.to_string(), "[REDACTED]");
    let d = error.downcast_ref::<Failure>().unwrap().0.clone();
    assert!(Arc::ptr_eq(&d, &h.client.diagnostic_for_tests().unwrap()));
    d
}

/// A native in-process HTTP peer, with actual request consumption and explicit
/// body framing. It never fabricates client reads or adjusts production clocks.
async fn peer(
    bytes: Vec<u8>,
    declared: usize,
    chunked: bool,
    hold: Option<Arc<tokio::sync::Notify>>,
) -> (
    String,
    Arc<tokio::sync::Notify>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let sent = Arc::new(tokio::sync::Notify::new());
    let signal = sent.clone();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "peer saw no actual HTTP request");
            request.extend_from_slice(&chunk[..n]);
            assert!(request.len() < 64 * 1024, "bounded fixture request");
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = std::str::from_utf8(&request[..end]).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        assert!(std::str::from_utf8(&request).unwrap().contains("safe"));
        let framing = if chunked {
            "Transfer-Encoding: chunked\r\n".to_owned()
        } else {
            format!("Content-Length: {declared}\r\n")
        };
        stream.write_all(format!("HTTP/1.1 400 Bad Request\r\n{framing}Content-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        if chunked {
            for byte in bytes {
                stream
                    .write_all(&[b'1', b'\r', b'\n', byte, b'\r', b'\n'])
                    .await
                    .unwrap();
                stream.flush().await.unwrap();
                tokio::task::yield_now().await;
            }
            stream.write_all(b"0\r\n\r\n").await.unwrap();
        } else {
            stream.write_all(&bytes).await.unwrap();
        }
        stream.flush().await.unwrap();
        signal.notify_one();
        if let Some(release) = hold {
            tokio::time::timeout(Duration::from_secs(5), release.notified())
                .await
                .unwrap();
        }
        let _ = stream.shutdown().await;
    });
    (url, sent, task)
}

#[tokio::test]
async fn complete_error_body_is_redacted_before_the_400_scalar_diagnostic_cut() {
    let f = Fixture::new().await;
    let body = format!(
        "{}synthetic-secret search-secret{}",
        "q".repeat(370),
        "z".repeat(500)
    );
    f.error(400, body.clone()).await;
    let h = f.harness(json!({})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Http);
    let text = d.checked_text().unwrap();
    assert!(
        text.contains("[REDACTED]") && !text.contains("synthetic-") && !text.contains("search-")
    );
    let a = &d.attempts()[0];
    assert_eq!(a.status, Some(400));
    assert_eq!(a.observed_bytes, Some(body.len() as u64));
    assert!(a.eof && a.diagnostic_cut && !a.truncated);
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    assert_eq!(*h.session.usage.lock().unwrap(), (0, 0));
    f.no_store();
}
#[tokio::test]
async fn exact_checked_request_and_original_auth_headers_survive_diagnostic_selection() {
    let f = Fixture::new().await;
    f.error(400, b"synthetic-secret search-secret").await;
    let h = f
        .harness(json!({"model_kwargs":{"default_headers":{"x-original":"search-secret"}}}))
        .await;
    let checked = h.source.build(&request()).unwrap();
    let error = h.client.send_checked_for_tests(&checked).await.unwrap_err();
    let d = &error.downcast_ref::<Failure>().unwrap().0;
    d.bind(&h.source, &checked).unwrap();
    let requests = f.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, checked.bytes());
    assert_eq!(
        requests[0].headers["authorization"],
        "Bearer synthetic-secret"
    );
    assert_eq!(requests[0].headers["x-original"], "search-secret");
    f.no_store();
}
#[tokio::test]
async fn diagnostic_cannot_bind_to_a_foreign_client_or_an_equal_fresh_request() {
    let f = Fixture::new().await;
    f.error(400, b"synthetic-secret").await;
    let h = f.harness(json!({})).await;
    let checked = h.source.build(&request()).unwrap();
    let error = h.client.send_checked_for_tests(&checked).await.unwrap_err();
    let d = &error.downcast_ref::<Failure>().unwrap().0;
    d.bind(&h.source, &checked).unwrap();
    let fresh = h.source.build(&request()).unwrap();
    assert!(d.bind(&h.source, &fresh).is_err());
    let twin = h.twin.request_source_for_tests().unwrap();
    let other = twin.build(&request()).unwrap();
    assert!(d.bind(&twin, &other).is_err());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}
#[tokio::test]
async fn split_unicode_and_credentials_use_real_chunked_http_without_lossy_repair() {
    let f = Fixture::new().await;
    let bytes = "before 钥匙🙂 after synthetic-secret".as_bytes().to_vec();
    let (url, _, task) = peer(bytes.clone(), bytes.len(), true, None).await;
    let h = f
        .harness(json!({"base_url":url,"search_api_key":"钥匙🙂"}))
        .await;
    let d = failure(&h).await;
    task.await.unwrap();
    assert_eq!(d.kind(), Kind::Http);
    assert!(
        d.checked_text()
            .unwrap()
            .ends_with("before [REDACTED] after [REDACTED]")
    );
    assert_eq!(d.attempts()[0].observed_bytes, Some(bytes.len() as u64));
    assert!(d.attempts()[0].eof);
}
#[tokio::test]
async fn overlapping_frozen_credentials_are_one_checked_text_view() {
    let f = Fixture::new().await;
    f.error(400, b"xabababaz").await;
    let h = f
        .harness(json!({"api_key":"ababa","search_api_key":"babab"}))
        .await;
    let d = failure(&h).await;
    assert!(d.checked_text().unwrap().ends_with("x[REDACTED]z"));
    assert_eq!(d.attempts()[0].observed_bytes, Some(9));
}
#[tokio::test]
async fn invalid_original_utf8_is_fixed_even_when_mime_suggests_another_charset() {
    let f = Fixture::new().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_raw(vec![b'x', 255], "text/plain; charset=iso-8859-1"),
        )
        .mount(&f.server)
        .await;
    let h = f.harness(json!({})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Utf8);
    assert_eq!(d.checked_text().unwrap(), "[REDACTED]");
    assert_eq!(d.attempts()[0].observed_bytes, Some(2));
    assert!(d.attempts()[0].truncated);
}
#[tokio::test]
async fn incomplete_utf8_at_real_eof_does_not_release_or_repair_suffixes() {
    let f = Fixture::new().await;
    f.error(400, vec![0xe2, 0x82]).await;
    let h = f.harness(json!({})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Utf8);
    assert!(d.attempts()[0].eof && d.attempts()[0].truncated);
    assert_eq!(d.attempts()[0].observed_bytes, Some(2));
}
#[tokio::test]
async fn unmatched_credential_prefix_is_released_only_at_a_real_complete_body_eof() {
    let f = Fixture::new().await;
    f.error(400, b"credential-").await;
    let h = f
        .harness(json!({"search_api_key":"credential-value"}))
        .await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Http);
    assert!(d.checked_text().unwrap().ends_with("credential-"));
    assert_eq!(d.attempts()[0].observed_bytes, Some(11));
    assert!(d.attempts()[0].eof);
}
#[tokio::test]
async fn real_truncated_body_never_finishes_a_held_credential_prefix() {
    let f = Fixture::new().await;
    let (url, _, task) = peer(b"credential-".to_vec(), 200, false, None).await;
    let h = f
        .harness(json!({"base_url":url,"search_api_key":"credential-value"}))
        .await;
    let d = failure(&h).await;
    task.await.unwrap();
    assert_eq!(d.kind(), Kind::Read);
    assert_eq!(d.checked_text().unwrap(), "[REDACTED]");
    let a = &d.attempts()[0];
    assert_eq!(a.observed_bytes, Some(11));
    assert!(a.read_error && a.truncated && !a.eof);
}
#[tokio::test]
async fn forced_view_cut_discards_all_diagnostic_detail_and_preserves_observed_bytes() {
    let f = Fixture::new().await;
    f.error(503, format!("{}credential-", "q".repeat(99500)))
        .await;
    let h = f
        .harness(json!({"search_api_key":"credential-value"}))
        .await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Limit);
    assert_eq!(d.checked_text().unwrap(), "[REDACTED]");
    let a = &d.attempts()[0];
    assert!(a.observed_bytes.unwrap() >= 98_976);
    assert!(a.truncated && !a.eof);
    assert_eq!(a.status, Some(503));
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1); // no privacy-failure retry
}
#[tokio::test]
async fn original_body_work_limit_is_independent_of_a_highly_compressed_redacted_view() {
    let f = Fixture::new().await;
    f.error(400, "K".repeat(1024 * 1024 + 16384)).await;
    let h = f.harness(json!({"search_api_key":"K".repeat(16384)})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Limit);
    assert_eq!(d.checked_text().unwrap(), "[REDACTED]");
    assert!(d.attempts()[0].observed_bytes.unwrap() > 1024 * 1024);
    assert!(d.attempts()[0].truncated && !d.attempts()[0].eof);
}
#[tokio::test]
async fn a_400_scalar_cut_through_a_replacement_marker_fails_fixed_not_as_an_alias() {
    let f = Fixture::new().await;
    f.error(400, format!("{}synthetic-secret suffix", "q".repeat(395)))
        .await;
    let h = f.harness(json!({})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Redaction);
    assert_eq!(d.checked_text().unwrap(), "[REDACTED]");
    assert!(d.attempts()[0].eof && d.attempts()[0].diagnostic_cut);
}
#[tokio::test]
async fn replacement_created_literals_are_rejected_by_the_delayed_rendered_check() {
    let f = Fixture::new().await;
    f.error(400, b"xsecret").await;
    let h = f
        .harness(json!({"api_key":"secret","search_api_key":"x[REDACTED]"}))
        .await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Redaction);
    assert_eq!(d.checked_text().unwrap(), "[REDACTED]");
}
#[tokio::test]
async fn final_diagnostic_framing_is_checked_without_rewriting_original_status_facts() {
    let f = Fixture::new().await;
    f.error(400, b"value").await;
    let h = f.harness(json!({"search_api_key":"HTTP 400"})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Http);
    assert!(!d.checked_text().unwrap().contains("HTTP 400"));
    assert!(d.checked_text().unwrap().contains("[REDACTED]"));
    assert_eq!(d.attempts()[0].status, Some(400));
}
#[tokio::test]
async fn replaced_configuration_does_not_change_original_auth_or_diagnostic_seeds() {
    let f = Fixture::new().await;
    f.error(400, b"synthetic-secret changed-secret").await;
    let h = f.harness(json!({})).await;
    std::fs::write(
        f.config.join("config.json"),
        json!({"api_key":"changed-secret","model":"changed"}).to_string(),
    )
    .unwrap();
    let d = failure(&h).await;
    assert!(
        d.checked_text()
            .unwrap()
            .ends_with("[REDACTED] changed-secret")
    );
    let r = f.server.received_requests().await.unwrap();
    assert_eq!(r[0].headers["authorization"], "Bearer synthetic-secret");
}
#[tokio::test]
async fn typed_late_filesystem_stop_hides_a_completed_diagnostic_without_losing_original_facts() {
    let f = Fixture::new().await;
    f.error(400, b"synthetic-secret").await;
    let h = f.harness(json!({})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Http);
    assert_eq!(d.attempts()[0].observed_bytes, Some(16));
    let out = h
        .filesystem
        .call("write_file", json!({"file_path":"build.rs","content":"no"}))
        .await;
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert_eq!(d.kind(), Kind::Stopped);
    assert!(d.checked_text().is_err());
    assert_eq!(d.attempts()[0].status, Some(400));
    assert_eq!(d.attempts()[0].observed_bytes, Some(16));
    assert!(d.attempts()[0].eof && !f.workspace.join("build.rs").exists());
}
#[tokio::test]
async fn same_typed_latch_stops_a_really_observed_held_http_body_before_the_read_timeout() {
    let f = Fixture::new().await;
    let release = Arc::new(tokio::sync::Notify::new());
    let (url, sent, peer) = peer(b"credential-".to_vec(), 200, false, Some(release.clone())).await;
    let h = Arc::new(
        f.harness(json!({"base_url":url,"search_api_key":"credential-value"}))
            .await,
    );
    let task = tokio::spawn({
        let h = h.clone();
        async move { failure(&h).await }
    });
    tokio::time::timeout(Duration::from_secs(5), sent.notified())
        .await
        .unwrap();
    // Startup/receipt gets its own unchanged five-second bound. The strict
    // stop measurement starts ONLY after this client's actual eleven-byte read,
    // not after server write, scheduler delay or an assumed native launch.
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.source.observed_for_tests() != 11 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("actual received-body prefix before typed stop");
    let at = Instant::now();
    let out = h
        .filesystem
        .call("write_file", json!({"file_path":"build.rs","content":"no"}))
        .await;
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    let d = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(at.elapsed() < Duration::from_secs(2));
    assert_eq!(d.kind(), Kind::Stopped);
    let a = &d.attempts()[0];
    assert_eq!(a.observed_bytes, Some(11)); // actual pre-stop reads, not a held server prefix alone
    assert!(a.truncated && !a.eof && !a.read_error);
    assert!(d.checked_text().is_err());
    release.notify_one();
    peer.await.unwrap();
    f.no_store();
}
#[tokio::test]
async fn approval_wording_in_an_error_body_is_not_latching_authority() {
    let f = Fixture::new().await;
    f.error(400, b"NeedsApproval: approve/resume suspended")
        .await;
    let h = f.harness(json!({})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Http);
    let allowed = h
        .filesystem
        .call(
            "write_file",
            json!({"file_path":"ordinary.txt","content":"allowed"}),
        )
        .await;
    assert!(!allowed.is_error);
    assert_eq!(
        std::fs::read(f.workspace.join("ordinary.txt")).unwrap(),
        b"allowed"
    );
}
#[tokio::test]
async fn genuine_transient_error_receipts_retry_once_with_original_observations_and_no_usage() {
    let f = Fixture::new().await;
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            let i = n.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(if i == 0 { 503 } else { 400 }).set_body_raw(
                if i == 0 {
                    b"synthetic-secret".to_vec()
                } else {
                    b"search-secret".to_vec()
                },
                "text/plain",
            )
        })
        .mount(&f.server)
        .await;
    let h = f.harness(json!({})).await;
    let start = Instant::now();
    let d = failure(&h).await;
    assert!(start.elapsed() >= Duration::from_millis(250));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(d.attempts().len(), 2);
    assert_eq!(d.attempts()[0].status, Some(503));
    assert_eq!(d.attempts()[0].observed_bytes, Some(16));
    assert_eq!(d.attempts()[1].status, Some(400));
    assert_eq!(d.attempts()[1].observed_bytes, Some(13));
    assert!(d.attempts().iter().all(|a| a.eof && !a.truncated));
    assert_eq!(*h.session.usage.lock().unwrap(), (0, 0));
    f.no_store();
}
#[tokio::test]
async fn successful_retry_keeps_original_sse_control_and_clears_last_failure_not_fake_receipts() {
    let f = Fixture::new().await;
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    Mock::given(method("POST")).respond_with(move|_:&wiremock::Request|{
        if n.fetch_add(1,Ordering::SeqCst)==0 {ResponseTemplate::new(429).set_body_raw(b"search-secret".to_vec(),"text/plain")}
        else {ResponseTemplate::new(200).set_body_raw(b"data: {\"choices\":[{\"delta\":{\"content\":\"ORIGINAL synthetic-secret\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_vec(),"text/event-stream")}
    }).mount(&f.server).await;
    let h = f.harness(json!({})).await;
    let turn = h.client.stream_model_turn(&request()).await.unwrap();
    assert_eq!(turn.content, "ORIGINAL synthetic-secret");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(h.client.diagnostic_for_tests().is_none());
}
#[tokio::test]
async fn connection_failures_have_fixed_diagnostics_no_url_echo_or_fabricated_body_counts() {
    let f = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let h = f
        .harness(json!({"base_url":format!("http://{addr}/synthetic-secret")}))
        .await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Connection);
    assert_eq!(d.checked_text().unwrap(), "[REDACTED]");
    assert_eq!(d.attempts().len(), 2);
    assert!(
        d.attempts().iter().all(|a| a.connection_error
            && a.status.is_none()
            && a.observed_bytes.is_none()
            && !a.eof)
    );
    assert_eq!(*h.session.usage.lock().unwrap(), (0, 0));
    f.no_store();
}
#[tokio::test]
async fn visible_incomplete_sse_has_fixed_diagnostics_but_original_counters_not_response_views() {
    let f = Fixture::new().await;
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"synthetic-secret\"}}]}\n\n";
    f.error(200, body.to_vec()).await;
    let h = f.harness(json!({})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Stream);
    assert_eq!(d.checked_text().unwrap(), "[REDACTED]");
    assert_eq!(d.attempts()[0].observed_bytes, Some(body.len() as u64));
    assert!(d.attempts()[0].eof);
    assert!(d.attempts()[0].truncated);
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}
#[tokio::test]
async fn empty_real_error_body_has_truthful_zero_observation_and_eof() {
    let f = Fixture::new().await;
    f.error(400, Vec::new()).await;
    let h = f.harness(json!({})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Http);
    assert_eq!(d.attempts()[0].observed_bytes, Some(0));
    assert!(d.attempts()[0].eof && !d.attempts()[0].truncated);
}

#[tokio::test]
async fn framework_error_erasure_keeps_the_actual_private_client_diagnostic_facts() {
    let f = Fixture::new().await;
    f.error(400, b"synthetic-secret").await;
    let h = f.harness(json!({})).await;
    let error = h.provider.complete(request()).await.unwrap_err();
    assert!(matches!(error,AgentError::Model(ref text) if text=="[REDACTED]"));
    let d = h.provider.diagnostic_for_tests().unwrap();
    assert_eq!(d.kind(), Kind::Http);
    assert_eq!(d.attempts().len(), 1);
    assert_eq!(d.attempts()[0].status, Some(400));
    assert_eq!(d.attempts()[0].observed_bytes, Some(16));
    assert!(d.attempts()[0].eof);
    assert_eq!(*h.session.usage.lock().unwrap(), (0, 0));
    f.no_store();
}

#[tokio::test]
async fn two_transient_http_failures_retain_two_genuine_receipts_not_a_third_retry() {
    let f = Fixture::new().await;
    f.error(503, b"synthetic-secret").await;
    let h = f.harness(json!({})).await;
    let d = failure(&h).await;
    assert_eq!(d.kind(), Kind::Http);
    assert_eq!(d.attempts().len(), 2);
    assert_eq!(f.server.received_requests().await.unwrap().len(), 2);
    assert!(
        d.attempts()
            .iter()
            .all(|a| a.status == Some(503) && a.observed_bytes == Some(16) && a.eof)
    );
    assert_eq!(*h.session.usage.lock().unwrap(), (0, 0));
}
