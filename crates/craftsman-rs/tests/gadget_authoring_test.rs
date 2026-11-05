//! G3 tests use the real CLI with isolated configuration. Rustup/Cargo paths
//! locate the already installed public toolchain/cache, not user credentials.
#![cfg(feature = "gadgets")]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

// Cold public-crate compilation is not a 120-second performance gate. Both
// build fixtures select the SAME supported user-config budget, with a larger
// outer process bound. Production ShellSettings defaults remain unchanged.
const NATIVE_BUILD_TIMEOUT_SECS: u64 = 240;
const COMMAND_TIMEOUT_SECS: u64 = 300;

fn fixture_config(mut fields: Value) -> Value {
    fields.as_object_mut().unwrap().insert(
        "shell".into(),
        json!({"timeout_secs": NATIVE_BUILD_TIMEOUT_SECS}),
    );
    fields
}

struct Home {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    home: PathBuf,
    config: PathBuf,
}
impl Home {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let home = root.path().join("home");
        let config = home.join(".craftsman");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.json"),
            fixture_config(json!({})).to_string(),
        )
        .unwrap();
        Self {
            _root: root,
            workspace,
            home,
            config,
        }
    }
    fn command(&self, toolchain: bool) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_craftsman"));
        command
            .current_dir(&self.workspace)
            .env("HOME", &self.home)
            .env("CRAFTSMAN_CONFIG_DIR", &self.config)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.env("USERPROFILE", &self.home);
        if toolchain {
            let original_home = dirs::home_dir().unwrap();
            command.env(
                "RUSTUP_HOME",
                std::env::var_os("RUSTUP_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| original_home.join(".rustup")),
            );
            command.env(
                "CARGO_HOME",
                std::env::var_os("CARGO_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| original_home.join(".cargo")),
            );
        } else {
            command.env("RUSTUP_HOME", self.home.join("empty-rustup"));
        }
        command
    }
}
async fn run(command: &mut tokio::process::Command) -> std::process::Output {
    tokio::time::timeout(Duration::from_secs(COMMAND_TIMEOUT_SECS), command.output())
        .await
        .expect("authoring command exceeded its generous bound")
        .unwrap()
}
fn json_output(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cold_cli_and_agent_build_fixtures_select_the_same_bounded_budget() {
    let home = Home::new();
    let cli: Value =
        serde_json::from_slice(&std::fs::read(home.config.join("config.json")).unwrap()).unwrap();
    let agent = fixture_config(json!({"model":"synthetic-model", "gadgets":{"enabled":true}}));
    assert_eq!(cli["shell"]["timeout_secs"], NATIVE_BUILD_TIMEOUT_SECS);
    assert_eq!(agent["shell"], cli["shell"]);
    let selected = cli["shell"]["timeout_secs"].as_u64().unwrap();
    assert!(selected < COMMAND_TIMEOUT_SECS);
    assert_eq!(COMMAND_TIMEOUT_SECS, 300);
    assert!(cli.get("auto_approve").is_none());
}

#[tokio::test]
async fn cli_scaffolds_builds_and_calls_a_gadget_without_running_unapproved_code() {
    let home = Home::new();
    let created = json_output(run(home.command(true).args(["gadgets", "new", "word-count"])).await);
    let root = PathBuf::from(created["source"].as_str().unwrap());
    assert!(!root.join("gadget.wasm").exists());
    assert!(
        created["skill"]
            .as_str()
            .is_some_and(|p| std::path::Path::new(p).is_file())
    );
    let built = json_output(run(home.command(true).args(["gadgets", "build", "word-count"])).await);
    assert_eq!(built["status"], "built");
    assert!(
        !home.config.join("gadgets/.grants.json").exists(),
        "build must not manufacture approval"
    );
    let listed = json_output(run(home.command(true).args(["gadgets", "list"])).await);
    assert_eq!(listed[0]["status"], "needs-approval");
    assert_eq!(listed[0]["functions"], json!([]));
    let result = json_output(
        run(home.command(true).args([
            "gadgets",
            "call",
            "word-count",
            "count_words",
            "--allow-gadget",
            "word-count",
            "--args",
            r#"{"text":"one two three"}"#,
        ]))
        .await,
    );
    assert_eq!(result, 3);
    let before = std::fs::read(root.join("gadget.wasm")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "this is not Rust").unwrap();
    let failed = run(home.command(true).args(["gadgets", "build", "word-count"])).await;
    assert!(!failed.status.success());
    assert_eq!(
        std::fs::read(root.join("gadget.wasm")).unwrap(),
        before,
        "failed build must preserve the previous component"
    );
}

#[tokio::test]
async fn doctor_reports_missing_target_with_an_explicit_fix_and_does_not_install_it() {
    let home = Home::new();
    let report = json_output(run(home.command(false).args(["gadgets", "doctor"])).await);
    assert_eq!(report["target_installed"], false);
    assert!(
        report["fix"]
            .as_str()
            .unwrap()
            .contains("--target wasm32-wasip2")
    );
    // rustup may initialize an empty directory on its first read-only query;
    // verify no compiler was installed, rather than asserting no directory I/O.
    let toolchains = home.home.join("empty-rustup/toolchains");
    assert!(
        std::fs::read_dir(&toolchains)
            .map(|entries| entries.count() == 0)
            .unwrap_or(true)
    );
}

fn shell_binary() -> String {
    let path = env!("CARGO_BIN_EXE_craftsman");
    #[cfg(windows)]
    {
        format!("& '{}'", path.replace('\'', "''"))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

struct AuthoringModel {
    config: PathBuf,
}
impl Respond for AuthoringModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        let round = messages.iter().filter(|m| m["role"] == "tool").count();
        let id = "agent-counter";
        let binary = shell_binary();
        let source = self.config.join("gadgets").join(id).join("src/lib.rs");
        let call = match round {
            0 => Some((
                "execute",
                json!({"command":format!("{binary} gadgets new {id}")}),
            )),
            1 => Some((
                "edit_file",
                json!({"file_path":source,"old_string":"Ok(args.text.split_whitespace().count())","new_string":"Ok(args.text.split_whitespace().count() * 2)"}),
            )),
            2 => Some((
                "execute",
                json!({"command":format!("{binary} gadgets build {id}")}),
            )),
            3 => {
                assert!(
                    messages.iter().any(|m| m["role"] == "user"
                        && m["content"]
                            .as_str()
                            .is_some_and(|s| s.starts_with("<gadget_catalog_update")
                                && s.contains("count_words"))),
                    "successful build must be picked up without a gadget_list call"
                );
                Some((
                    "gadget_call",
                    json!({"gadget":id,"function":"count_words","args":{"text":"one two three"}}),
                ))
            }
            _ => {
                assert!(
                    messages
                        .iter()
                        .any(|m| m["role"] == "tool" && m["content"] == "6"),
                    "built function result must reach the model"
                );
                None
            }
        };
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("build-gadget"),
            "built-in authoring skill must be discoverable"
        );
        let event = if let Some((name, args)) = call {
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":format!("call-{round}"),"function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]})
        } else {
            json!({"choices":[{"delta":{"content":"The new gadget returned 6."},"finish_reason":"stop"}]})
        };
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(format!("data: {event}\n\ndata: [DONE]\n\n"))
    }
}

#[tokio::test]
async fn agent_scaffolds_edits_builds_and_uses_a_new_gadget_in_one_session() {
    let home = Home::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(AuthoringModel {
            config: home.config.clone(),
        })
        .mount(&server)
        .await;
    std::fs::write(home.config.join("config.json"),fixture_config(json!({"base_url":server.uri(),"api_key":"synthetic-test-key","model":"mock-model","gadgets":{"enabled":true}})).to_string()).unwrap();
    let output = run(home.command(true).args([
        "-m",
        "Build a missing word-count function and use it",
        "--auto-approve",
        "--allow-gadget",
        "agent-counter",
    ]))
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("returned 6"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 5);
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    for request in &requests[1..] {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            first["tools"], body["tools"],
            "authoring must keep the tool list fixed"
        );
    }
    assert!(
        !home.config.join("gadgets/.grants.json").exists(),
        "explicit invocation grant stays ephemeral"
    );
}
