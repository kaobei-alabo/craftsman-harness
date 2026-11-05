//! Private web tests consume an actual builder lease/unique gate and synthetic
//! selected config. Loopback transport never supplies arbitrary gates/seeds.
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::runtime::InvocationGate;
use crate::exec::web::Web;
use craftsman_harness::ToolErrorKind;
use std::time::{Duration, Instant};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    server: MockServer,
}
struct Harness {
    web: Arc<Web>,
    filesystem: Arc<crate::exec::confined_fs::TextFs>,
    gate: Arc<InvocationGate>,
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
    async fn harness(&self, extra: Value, selectors: Selectors) -> Harness {
        let mut config = json!({"model":"synthetic","api_key":"synthetic-secret","search_api_key":"search-secret","_test_web_search_endpoint":format!("{}/search",self.server.uri())});
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
                selectors,
            },
            BTreeMap::new(),
        )
        .await
        .unwrap();
        let (_, gate) = build_prepared_session(&prepared).await.unwrap();
        Harness {
            web: Arc::new(Web::from_scope(WebScope {
                prepared: &prepared,
                gate: gate.clone(),
            })),
            filesystem: Arc::new(crate::exec::confined_fs::TextFs::from_scope(
                FilesystemScope {
                    prepared: &prepared,
                    gate: gate.clone(),
                },
            )),
            gate,
        }
    }
}
fn allow() -> Selectors {
    Selectors {
        allow: vec![
            "tool:web_fetch".into(),
            "tool:web_search".into(),
            "tool:write_file".into(),
        ],
        ..Default::default()
    }
}
#[tokio::test]
async fn original_fetch_url_and_payload_are_not_redacted_request_substitutions() {
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/synthetic-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_string("before synthetic-secret after"))
        .mount(&f.server)
        .await;
    let h = f
        .harness(
            json!({"model_kwargs":{"default_headers":{"x-model-only":"synthetic-secret"}}}),
            allow(),
        )
        .await;
    let url = format!("{}/synthetic-secret", f.server.uri());
    let out = h.web.call("web_fetch", json!({"url":url})).await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "before [REDACTED] after");
    assert_eq!(out.meta.network_calls, 1);
    assert_eq!(out.meta.bytes, Some(29));
    assert!(!out.meta.truncated);
    let requests = f.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/synthetic-secret");
    assert!(!requests[0].headers.contains_key("x-model-only"));
    assert!(!requests[0].headers.contains_key("authorization"));
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}
#[tokio::test]
async fn actual_html_joined_and_entity_decoded_credentials_are_redacted_before_output_cuts() {
    let f = Fixture::new().await;
    let body =
        "<h1>before</h1><p>syn<span>thetic</span>-s&#101;cret</p><style>ignored</style>after";
    Mock::given(path("/html"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(body.as_bytes(), "text/html; charset=utf-8"),
        )
        .mount(&f.server)
        .await;
    let h = f.harness(json!({}), allow()).await;
    let out = h
        .web
        .call(
            "web_fetch",
            json!({"url":format!("{}/html",f.server.uri())}),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "before\n[REDACTED]\nafter");
    assert_eq!(out.meta.bytes, Some(body.len() as u64));
    assert!(!out.meta.truncated);
}
#[tokio::test]
async fn malformed_and_non_http_or_credential_urls_never_initiate_a_request() {
    let f = Fixture::new().await;
    let h = f.harness(json!({}), allow()).await;
    for args in [
        json!({}),
        json!({"url":null}),
        json!({"url":12}),
        json!({"url":""}),
        json!({"url":"not a url"}),
        json!({"url":"ftp://example.com"}),
        json!({"url":"http://user:password@localhost/"}),
        json!({"url":"http://169.254.169.254/"}),
        json!({"url":"http://[fe80::1]/"}),
        json!({"url":"http://[::ffff:169.254.169.254]/"}),
    ] {
        let out = h.web.call("web_fetch", args).await;
        assert!(out.is_error);
        assert_eq!(out.meta.network_calls, 0);
        assert_eq!(out.meta.bytes, None);
        assert!(!out.content.contains("password"));
    }
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn unapproved_and_denied_web_calls_keep_original_typed_policy_and_zero_attempts() {
    let f = Fixture::new().await;
    for selectors in [
        Selectors::default(),
        Selectors {
            deny: vec!["tool:web_fetch".into()],
            ..allow()
        },
    ] {
        let h = f.harness(json!({}), selectors).await;
        let out = h.web.call("web_fetch", json!({"url":f.server.uri()})).await;
        assert!(out.is_error);
        assert_eq!(out.meta.network_calls, 0);
        assert_eq!(out.meta.bytes, None);
        assert!(!h.gate.approval_required());
    }
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn relative_redirects_keep_original_authorized_targets_and_count_real_attempts() {
    let f = Fixture::new().await;
    Mock::given(path("/start"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/synthetic-secret"))
        .mount(&f.server)
        .await;
    Mock::given(path("/synthetic-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_string("synthetic-secret"))
        .mount(&f.server)
        .await;
    let h = f.harness(json!({}), allow()).await;
    let out = h
        .web
        .call(
            "web_fetch",
            json!({"url":format!("{}/start",f.server.uri())}),
        )
        .await;
    assert_eq!(out.content, "[REDACTED]");
    assert!(!out.is_error);
    assert_eq!(out.meta.network_calls, 2);
    assert_eq!(out.meta.bytes, Some(16));
    assert_eq!(f.server.received_requests().await.unwrap().len(), 2);
}
#[tokio::test]
async fn redirect_limits_and_linklocal_or_credential_targets_fail_before_a_later_send() {
    let f = Fixture::new().await;
    for (route, location, calls) in [
        ("/loop", "/loop", 6),
        ("/bad", "http://169.254.169.254/", 1),
        ("/private", "http://10.0.0.1/", 1),
        ("/unique", "http://[fd00::1]/", 1),
        ("/mapped-private", "http://[::ffff:10.0.0.1]/", 1),
        ("/creds", "http://user:synthetic-secret@localhost/", 1),
    ] {
        Mock::given(path(route))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", location))
            .mount(&f.server)
            .await;
        let h = f.harness(json!({}), allow()).await;
        let out = h
            .web
            .call(
                "web_fetch",
                json!({"url":format!("{}{route}",f.server.uri())}),
            )
            .await;
        assert!(out.is_error);
        assert_eq!(out.meta.network_calls, calls);
        assert!(!out.content.contains("synthetic-secret"));
        assert_eq!(out.meta.bytes, None);
    }
}
#[tokio::test]
async fn unsuccessful_status_and_untrusted_error_body_are_not_echoed_or_falsely_counted() {
    let f = Fixture::new().await;
    Mock::given(path("/error"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string("synthetic-secret unsafe provider diagnostic"),
        )
        .mount(&f.server)
        .await;
    let h = f.harness(json!({"api_key":"500"}), allow()).await;
    let out = h
        .web
        .call(
            "web_fetch",
            json!({"url":format!("{}/error",f.server.uri())}),
        )
        .await;
    assert!(out.is_error);
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::Failed));
    assert_eq!(out.meta.network_calls, 1);
    assert_eq!(out.meta.bytes, None);
    assert!(!out.content.contains("500") && !out.content.contains("unsafe provider diagnostic"));
}
#[tokio::test]
async fn invalid_original_body_utf8_fails_closed_without_lossy_decode() {
    let f = Fixture::new().await;
    Mock::given(path("/invalid"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![255]))
        .mount(&f.server)
        .await;
    let h = f.harness(json!({}), allow()).await;
    let out = h
        .web
        .call(
            "web_fetch",
            json!({"url":format!("{}/invalid",f.server.uri())}),
        )
        .await;
    assert!(out.is_error);
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::Failed));
    assert_eq!(out.meta.bytes, Some(1));
    assert_eq!(out.meta.network_calls, 1);
    assert!(!out.content.contains('\u{fffd}'));
}
#[tokio::test]
async fn raw_html_body_read_cap_drops_unfinished_state_and_preserves_observed_bytes() {
    let f = Fixture::new().await;
    // A short safe prefix may still be intentionally held by the two delayed
    // publication matchers at a forced cut. Make this a REAL released-prefix
    // proof without finishing holdback or widening any production/read limit.
    let body = format!(
        "verified-prefix-{}<script>{}synthetic-secret</script>tail",
        "q".repeat(64),
        "q".repeat(210_000)
    );
    Mock::given(path("/cap"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body.as_bytes(), "text/html"))
        .mount(&f.server)
        .await;
    let h = f.harness(json!({}), allow()).await;
    let out = h
        .web
        .call("web_fetch", json!({"url":format!("{}/cap",f.server.uri())}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.meta.truncated);
    assert!(out.meta.bytes.unwrap() >= 200_000);
    assert!(
        out.content.starts_with("verified-prefix-")
            && !out.content.contains("synthetic")
            && !out.content.contains("tail")
    );
    assert!(out.content.len() <= 100_000);
}
#[tokio::test]
async fn captured_search_key_and_original_query_are_sent_then_result_fields_are_redacted() {
    let f = Fixture::new().await;
    Mock::given(path("/search")).and(header("X-Subscription-Token","search-secret")).and(query_param("q","synthetic-secret query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"web":{"results":[{"title":"synthetic-secret title","url":"https://example.com/search-secret","description":"search-secret description"}]}}))).mount(&f.server).await;
    let h = f.harness(json!({}), allow()).await;
    let out = h
        .web
        .call("web_search", json!({"query":"  synthetic-secret query  "}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("[REDACTED]")
            && !out.content.contains("synthetic-secret")
            && !out.content.contains("search-secret")
    );
    assert_eq!(out.meta.network_calls, 1);
    assert!(out.meta.bytes.unwrap() > 0);
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}
#[tokio::test]
async fn immutable_search_auth_and_seeds_do_not_reopen_replaced_config() {
    let f = Fixture::new().await;
    Mock::given(path("/search"))
        .and(header("X-Subscription-Token", "search-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"web":{"results":[{"title":"search-secret replacement-secret"}]}}),
        ))
        .mount(&f.server)
        .await;
    let h = f.harness(json!({}), allow()).await;
    std::fs::write(f.config.join("config.json"),json!({"model":"synthetic","api_key":"replacement-secret","search_api_key":"replacement-secret"}).to_string()).unwrap();
    let out = h.web.call("web_search", json!({"query":"original"})).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("[REDACTED] replacement-secret"));
    assert!(!out.content.contains("search-secret"));
}
#[tokio::test]
async fn search_setup_unknown_provider_invalid_query_or_test_route_do_not_fake_results_or_traffic()
{
    let f = Fixture::new().await;
    for extra in [
        json!({"search_api_key":""}),
        json!({"search_api_key":"invalid\nsynthetic-secret"}),
        json!({"search_provider":"unsupported"}),
        json!({"_test_web_search_endpoint":"https://example.com/"}),
    ] {
        let h = f.harness(extra, allow()).await;
        let out = h.web.call("web_search", json!({"query":"query"})).await;
        assert!(out.is_error);
        assert_eq!(out.meta.network_calls, 0);
        assert_eq!(out.meta.bytes, None);
        assert!(!out.content.contains("No results"));
    }
    let h = f.harness(json!({}), allow()).await;
    for args in [
        json!({}),
        json!({"query":42}),
        json!({"query":" "}),
        json!({"query":"q".repeat(2049)}),
    ] {
        let out = h.web.call("web_search", args).await;
        assert!(out.is_error);
        assert_eq!(out.meta.network_calls, 0);
    }
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn search_does_not_forward_auth_to_redirects_or_publish_untrusted_error_bodies() {
    let f = Fixture::new().await;
    Mock::given(path("/search"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "/steal")
                .set_body_string("search-secret"),
        )
        .mount(&f.server)
        .await;
    let h = f.harness(json!({}), allow()).await;
    let out = h.web.call("web_search", json!({"query":"query"})).await;
    assert!(out.is_error);
    assert_eq!(out.meta.network_calls, 1);
    assert_eq!(out.meta.bytes, None);
    assert!(!out.content.contains("search-secret"));
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}
#[tokio::test]
async fn search_requires_complete_bounded_json_and_safe_structured_source_fields() {
    let f = Fixture::new().await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'q'; 512 * 1024 + 1]))
        .mount(&f.server)
        .await;
    let h = f.harness(json!({}), allow()).await;
    let out = h.web.call("web_search", json!({"query":"query"})).await;
    assert!(out.is_error);
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::OutputLimit));
    assert_eq!(out.meta.network_calls, 1);
    assert_eq!(out.meta.bytes, None);
}
#[tokio::test]
async fn typed_shared_failure_stops_an_actually_received_http_request_before_its_timeout() {
    let f = Fixture::new().await;
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    Mock::given(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(30))
                .set_body_string("synthetic-secret"),
        )
        .mount(&f.server)
        .await;
    let h = f.harness(json!({}), selectors).await;
    let task = tokio::spawn({
        let web = h.web.clone();
        let url = format!("{}/slow", f.server.uri());
        async move { web.call("web_fetch", json!({"url":url})).await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.server.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .expect("actual HTTP receipt before stop measurement");
    let at = Instant::now();
    let denied = h
        .filesystem
        .call("write_file", json!({"file_path":"build.rs","content":"no"}))
        .await;
    assert_eq!(denied.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert!(h.gate.approval_required());
    let out = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert!(at.elapsed() < Duration::from_secs(3));
    assert!(out.is_error);
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert_eq!(out.meta.network_calls, 1);
    assert_eq!(out.meta.bytes, None);
    assert_eq!(out.content, "[REDACTED]");
    let later = h
        .web
        .call(
            "web_fetch",
            json!({"url":format!("{}/later",f.server.uri())}),
        )
        .await;
    assert_eq!(later.meta.network_calls, 0);
    assert_eq!(later.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert!(!f.workspace.join("build.rs").exists());
}

#[tokio::test]
async fn actual_web_timeout_preserves_attempts_and_does_not_publish_partial_or_fake_usage() {
    let f = Fixture::new().await;
    Mock::given(path("/timeout"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(30))
                .set_body_string("synthetic-secret"),
        )
        .mount(&f.server)
        .await;
    let h = f.harness(json!({}), allow()).await;
    let at = Instant::now();
    let out = h
        .web
        .call(
            "web_fetch",
            json!({"url":format!("{}/timeout",f.server.uri())}),
        )
        .await;
    assert!(out.is_error);
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::Timeout));
    assert_eq!(out.meta.network_calls, 1);
    assert_eq!(out.meta.bytes, None);
    assert!(at.elapsed() >= Duration::from_secs(15) && at.elapsed() < Duration::from_secs(20));
    assert!(!out.content.contains("synthetic-secret"));
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn actual_search_malformed_and_scalar_colliding_json_are_fixed_failures_not_no_results() {
    for (key, body) in [
        ("synthetic-secret", b"{unfinished".as_slice()),
        ("42", br#"{"web":{"results":[]},"number":42}"#.as_slice()),
    ] {
        let f = Fixture::new().await;
        Mock::given(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&f.server)
            .await;
        let h = f.harness(json!({"api_key":key}), allow()).await;
        let out = h.web.call("web_search", json!({"query":"query"})).await;
        assert!(out.is_error);
        assert_eq!(out.meta.error_kind, Some(ToolErrorKind::Failed));
        assert_eq!(out.meta.network_calls, 1);
        assert_eq!(out.meta.bytes, Some(body.len() as u64));
        assert!(!out.content.contains("No results") && !out.content.contains(key));
    }
}

#[tokio::test]
async fn late_publication_stop_overrides_assembled_success_or_failure_without_inventing_facts() {
    for (body, failed) in [
        (b"synthetic-secret actual view".as_slice(), false),
        ([255].as_slice(), true),
    ] {
        let f = Fixture::new().await;
        Mock::given(path("/completed"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/plain"))
            .mount(&f.server)
            .await;
        let mut selectors = allow();
        selectors.on_unapproved = OnUnapproved::Fail;
        let h = f.harness(json!({}), selectors).await;
        let assembled = h
            .web
            .call(
                "web_fetch",
                json!({"url":format!("{}/completed", f.server.uri())}),
            )
            .await;
        assert_eq!(assembled.is_error, failed);
        assert_eq!(assembled.meta.bytes, Some(body.len() as u64));
        assert_eq!(assembled.meta.network_calls, 1);
        let bytes = assembled.meta.bytes;
        let attempts = assembled.meta.network_calls;
        let duration = assembled.meta.duration;
        // Deterministically seal AFTER a real HTTP result was assembled, then
        // exercise the same last-moment publisher check used in production.
        let stopped = h
            .filesystem
            .call(
                "write_file",
                json!({"file_path":"build.rs", "content":"no"}),
            )
            .await;
        assert_eq!(stopped.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
        assert!(h.gate.approval_required());
        let out = h.web.recheck_publication_for_tests(assembled);
        assert!(out.is_error && out.meta.truncated);
        assert_eq!(out.content, "[REDACTED]");
        assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
        assert_eq!(out.meta.bytes, bytes);
        assert_eq!(out.meta.network_calls, attempts);
        assert_eq!(out.meta.duration, duration);
        assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
        assert!(!f.workspace.join("build.rs").exists());
    }
}
