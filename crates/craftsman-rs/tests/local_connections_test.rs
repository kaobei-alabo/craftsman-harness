//! Retired service configuration never creates a background remote connection.
use serde_json::json;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn isolated_command(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_craftsman"));
    command
        .current_dir(root)
        .env("CRAFTSMAN_CONFIG_DIR", root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env_remove("CRAFTSMAN_BRIDGE_TOKEN")
        .env("CRAFTSMAN_PLATFORM_TOKEN", "retired-fixture-token")
        .env("CRAFTSMAN_INBOX_POLL_SECS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

#[tokio::test]
async fn acp_bridge_never_polls_the_retired_platform_even_with_legacy_credentials() {
    let root = tempfile::tempdir().unwrap();
    let retired = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(204))
        .mount(&retired)
        .await;
    let config = json!({"platform":{"base_url":retired.uri(),"api_token":"retired-fixture-token"}})
        .to_string();
    std::fs::write(root.path().join("config.json"), &config).unwrap();
    let reserve = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reserve.local_addr().unwrap().port();
    drop(reserve);
    let mut command = isolated_command(root.path());
    command.args([
        "acp-bridge",
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
    ]);
    let mut child = command.spawn().unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "bridge exited before readiness"
            );
            if let Ok(response) = client
                .get(format!("http://127.0.0.1:{port}/healthz"))
                .send()
                .await
            {
                if response.status().is_success() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("bridge did not become ready");
    // The former poller ticked immediately and every configured second.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(retired.received_requests().await.unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(root.path().join("config.json")).unwrap(),
        config
    );
    child.start_kill().unwrap();
    tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn service_agent_refuses_a_model_without_a_configured_connection() {
    let root = tempfile::tempdir().unwrap();
    let retired = MockServer::start().await;
    std::fs::write(root.path().join("config.json"), json!({"model":"model-only","platform":{"base_url":retired.uri(),"api_token":"retired-fixture-token"}}).to_string()).unwrap();
    let request = root.path().join("request.json");
    std::fs::write(&request, json!({"version":1,"input":"No model request should start.","system_prompt":"fixture","allowed_tools":[]}).to_string()).unwrap();
    let mut command = isolated_command(root.path());
    command.arg("service-agent").arg("--request").arg(&request);
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("No model connection configured")
    );
    assert!(retired.received_requests().await.unwrap().is_empty());
}
