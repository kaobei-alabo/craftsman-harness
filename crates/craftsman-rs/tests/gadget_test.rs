//! Real CLI and model-wire coverage for the G1 gadget host. Components are
//! built from Rust source; all child environments and homes are synthetic.
#![cfg(feature = "gadgets")]

#[path = "common/gadget_fixture.rs"]
mod gadget_fixture;

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

struct Reply {
    call: bool,
}
impl Respond for Reply {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let has_result = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool");
        let event = if self.call && !has_result {
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"gadget-1","function":{"name":"gadget_call","arguments":json!({"gadget":"demo","function":"echo","args":{"text":"from model"}}).to_string()}}]},"finish_reason":"tool_calls"}]})
        } else {
            json!({"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]})
        };
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(format!("data: {event}\n\ndata: [DONE]\n\n"))
    }
}
struct Home {
    _root: tempfile::TempDir,
    home: PathBuf,
    workspace: PathBuf,
    config: PathBuf,
}
impl Home {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        let config = home.join(".craftsman");
        let gadget = config.join("gadgets/demo");
        std::fs::create_dir_all(&gadget).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            workspace.join("Cargo.toml"),
            "must not be readable through WASI",
        )
        .unwrap();
        std::fs::write(
            gadget.join("gadget.toml"),
            "id='demo'\nversion='0.1.0'\ndescription='Fixture'\n",
        )
        .unwrap();
        std::fs::write(gadget.join("gadget.wasm"), &gadget_fixture::fixtures().0).unwrap();
        Self {
            _root: root,
            home,
            workspace,
            config,
        }
    }
    fn write_config(&self, route: &str, gadgets: Option<Value>) {
        let mut cfg =
            json!({"base_url": route, "api_key":"synthetic-test-key", "model":"mock-model"});
        if let Some(gadgets) = gadgets {
            cfg["gadgets"] = gadgets;
        }
        std::fs::write(self.config.join("config.json"), cfg.to_string()).unwrap();
    }
    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_craftsman"));
        command
            .current_dir(&self.workspace)
            .env("HOME", &self.home)
            .env("CRAFTSMAN_CONFIG_DIR", &self.config)
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        command.env("USERPROFILE", &self.home);
        command
    }
}
async fn run(command: &mut tokio::process::Command) -> std::process::Output {
    command.kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .expect("CLI timed out")
        .unwrap()
}
async fn server(call: bool) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(Reply { call })
        .mount(&server)
        .await;
    server
}
fn result(requests: &[Request]) -> String {
    requests
        .iter()
        .find_map(|request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "tool")
                .and_then(|m| m["content"].as_str())
                .map(str::to_owned)
        })
        .expect("model never received a tool result")
}

#[tokio::test]
async fn actual_one_shot_calls_a_gadget_and_preserves_the_fixed_tool_list() {
    let server = server(true).await;
    let home = Home::new();
    home.write_config(&server.uri(), Some(json!({"enabled":true})));
    let output = run(home
        .command()
        .args(["-m", "test gadgets", "--allow-gadget", "demo"]))
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result(&requests)).unwrap(),
        json!({"text":"from model", "version":"0.1.0"})
    );
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(first["tools"], second["tools"]);
    assert!(
        first["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "user"
                && m["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("<gadget_catalog_update")
                        && text.contains("\"name\":\"echo\""))),
        "actual terminal session must announce functions without a gadget_list request"
    );
    let names: Vec<_> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"gadget_list") && names.contains(&"gadget_call"));
    assert!(!names.iter().any(|n| n.starts_with("demo__")));
}

#[tokio::test]
async fn bypass_permissions_is_not_first_load_gadget_approval() {
    let server = server(true).await;
    let home = Home::new();
    home.write_config(&server.uri(), Some(json!({"enabled":true})));
    let output = run(home
        .command()
        .args(["-m", "test gadgets", "--auto-approve"]))
    .await;
    assert!(output.status.success());
    let requests = server.received_requests().await.unwrap();
    assert!(result(&requests).contains("NeedsApproval"));
    assert!(!home.config.join("gadgets/.grants.json").exists());
}

#[tokio::test]
async fn enabled_false_and_absent_are_identical_model_requests() {
    let server = server(false).await;
    let home = Home::new();
    home.write_config(&server.uri(), None);
    assert!(
        run(home.command().args(["-m", "test gadgets"]))
            .await
            .status
            .success()
    );
    home.write_config(&server.uri(), Some(json!({"enabled":false})));
    assert!(
        run(home.command().args(["-m", "test gadgets"]))
            .await
            .status
            .success()
    );
    let requests = server.received_requests().await.unwrap();
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(first, second);
    assert!(!first["tools"].as_array().unwrap().iter().any(|t| {
        t["function"]["name"]
            .as_str()
            .is_some_and(|n| n.starts_with("gadget_"))
    }));
}

#[tokio::test]
async fn actual_cli_denies_environment_files_and_a_listening_socket_without_a_model() {
    let home = Home::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let args = json!({"address": listener.local_addr().unwrap().to_string()}).to_string();
    let output = run(home
        .command()
        .env("GADGET_TEST_SECRET", "synthetic-child-only-secret")
        .args([
            "gadgets",
            "call",
            "demo",
            "ambient",
            "--allow-gadget",
            "demo",
            "--args",
            &args,
        ]))
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let returned: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(returned, json!({"env":null, "file":false, "socket":false}));
}
