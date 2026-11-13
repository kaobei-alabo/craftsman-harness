//! Marked native builder/HTTP/local-store memory-policy proof; no hosted models.
use super::*;
use craftsman_harness::Backend;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "agent::memory_tests::native_memory_policy_fixture";
const MARKER: &str = "craftsman-memory-policy=";
const DONE: &str = "actual memory policy, HTTP and local-store assertions completed";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned memory-policy fixture; unmarked startup returns before config access"]
async fn native_memory_policy_fixture() {
    let selected: Vec<_> = std::env::args_os()
        .filter_map(|value| {
            value
                .to_str()
                .and_then(|value| value.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if selected.len() != 1 || !matches!(selected[0].as_str(), "focused" | "legacy") {
        return;
    }
    let focused = selected[0] == "focused";
    let directory = std::env::current_dir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert!(system.contains("Persisted fixture preference."));
            if focused {
                assert!(system.contains("turn-start snapshot loaded by the harness"));
                assert!(system.contains("current user request, repository instructions, and permissions take priority"));
                assert!(!system.contains("update `/memories/` IMMEDIATELY"));
            } else {
                assert!(system.contains("update `/memories/` IMMEDIATELY"));
                assert!(!system.contains("turn-start snapshot loaded by the harness"));
            }
            ResponseTemplate::new(200).set_body_raw(
                "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
                "text/event-stream",
            )
        })
        .expect(1)
        .mount(&server)
        .await;
    let cfg: Config = serde_json::from_value(json!({
        "base_url":server.uri(), "api_key":"fixture-key", "model":"fixture-model",
        "harness_profile":"general", "mcp_servers":[], "lsp_servers":{},
        "voice":{"enabled":false}, "gadgets":{"enabled":false}
    }))
    .unwrap();
    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    let session = build_session_ext(
        &cfg,
        approval,
        "",
        false,
        false,
        None,
        SessionExtras {
            workspace_root: Some(directory),
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(|_| {})),
            task_focused_memory: focused,
            ..SessionExtras::default()
        },
    )
    .await
    .unwrap();
    let memories = session.memories.as_ref().unwrap();
    memories
        .write("/memories/AGENTS.md", "Persisted fixture preference.")
        .await
        .unwrap();
    session
        .agent
        .run_turn("Finish this fixture task.")
        .await
        .unwrap();
    assert_eq!(
        memories
            .read("/memories/AGENTS.md", 0, 100)
            .await
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(*session.usage.lock().unwrap(), (3, 2));
    eprintln!("{DONE}");
}

async fn run_case(mode: &str) {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}{mode}"))
        .current_dir(directory.path())
        .env_clear()
        .env("CRAFTSMAN_CONFIG_DIR", &config)
        .env("HOME", directory.path())
        .env("USERPROFILE", directory.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = command.spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap().take(65537);
    let mut stderr = child.stderr.take().unwrap().take(65537);
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    })
    .await;
    let (status, out, err) = match result {
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned memory fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
    assert!(config.join("threads.db").is_file());
}

#[tokio::test]
async fn actual_builder_selects_focused_memory_without_rewriting_persisted_preferences() {
    run_case("focused").await;
}

#[tokio::test]
async fn actual_legacy_builder_keeps_original_memory_guidance_and_usage() {
    run_case("legacy").await;
}
