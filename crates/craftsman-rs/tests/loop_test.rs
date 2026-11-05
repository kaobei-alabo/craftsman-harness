//! End-to-end agent-loop tests against a mock OpenAI-compatible server.
//!
//! Round 1: the model streams a tool call (`execute` echo). The loop must
//! run the tool and POST a follow-up request whose messages include the
//! assistant tool_calls entry and the tool result. Round 2: the model
//! streams plain text and stops.

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn set_test_home(command: &mut tokio::process::Command, home: &std::path::Path) {
    command.env("HOME", home);
    command.env("CRAFTSMAN_CONFIG_DIR", home.join(".craftsman"));
    #[cfg(windows)]
    command.env("USERPROFILE", home);
}

struct ScriptedLlm;

fn sse(events: &[&str]) -> String {
    let mut out = String::new();
    for e in events {
        out.push_str("data: ");
        out.push_str(e);
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

impl Respond for ScriptedLlm {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        let has_tool_result = messages.iter().any(|m| m["role"] == "tool");

        let events = if !has_tool_result {
            // Round 1: ask for a tool run, split across deltas.
            sse(&[
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"execute","arguments":""}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\": \"echo loop-ok\"}"}}]}}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":5}}"#,
            ])
        } else {
            // Round 2: the tool result must have made it back verbatim.
            let tool_msg = messages.iter().find(|m| m["role"] == "tool").unwrap();
            let content = tool_msg["content"].as_str().unwrap_or_default();
            assert!(
                content.contains("loop-ok"),
                "tool result not fed back, got: {content}"
            );
            assert_eq!(tool_msg["tool_call_id"], "call_1");
            // The assistant tool_calls message must precede it.
            let assistant = messages
                .iter()
                .find(|m| m["role"] == "assistant" && !m["tool_calls"].is_null())
                .expect("assistant tool_calls message missing from history");
            assert_eq!(assistant["tool_calls"][0]["function"]["name"], "execute");
            sse(&[
                r#"{"choices":[{"delta":{"content":"done: "}}]}"#,
                r#"{"choices":[{"delta":{"content":"loop-ok"}}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":30,"completion_tokens":4}}"#,
            ])
        };
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(events)
    }
}

// The agent/llm modules live in the binary crate; re-compile them here via
// include! is ugly — instead this test drives the REAL binary in one-shot
// mode with the mock server injected through a temp config HOME.
#[tokio::test]
async fn one_shot_runs_tool_loop_against_mock_server() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ScriptedLlm)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let craftsman_dir = home.path().join(".craftsman");
    std::fs::create_dir_all(&craftsman_dir).unwrap();
    std::fs::write(
        craftsman_dir.join("config.json"),
        serde_json::json!({
            "base_url": server.uri(),
            "api_key": "test-key",
            "model": "mock-model"
        })
        .to_string(),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_craftsman");
    let mut command = tokio::process::Command::new(bin);
    command.args(["-m", "run the echo", "--auto-approve"]);
    set_test_home(&mut command, home.path());
    let out = command.output().await.unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status
    );
    // Final streamed answer reached stdout; activity lines went to stderr.
    assert!(stdout.contains("done: loop-ok"), "stdout: {stdout}");
    assert!(stderr.contains("execute"), "stderr: {stderr}");
}

#[tokio::test]
async fn one_shot_without_auto_approve_denies_on_eof_and_reports() {
    // Closed stdin → the HITL prompt reads EOF → deny. The turn must still
    // complete (the model gets the rejection as the tool result).
    let server = MockServer::start().await;

    struct DenyAware;
    impl Respond for DenyAware {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let messages = body["messages"].as_array().unwrap();
            let has_tool_result = messages.iter().any(|m| m["role"] == "tool");
            let events = if !has_tool_result {
                sse(&[
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"execute","arguments":"{\"command\":\"rm -rf /tmp/x\"}"}}]}}]}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
                ])
            } else {
                let tool_msg = messages.iter().find(|m| m["role"] == "tool").unwrap();
                assert!(
                    tool_msg["content"].as_str().unwrap().contains("denied"),
                    "denial must reach the model"
                );
                sse(&[
                    r#"{"choices":[{"delta":{"content":"understood"}}]}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                ])
            };
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(events)
        }
    }

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(DenyAware)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let craftsman_dir = home.path().join(".craftsman");
    std::fs::create_dir_all(&craftsman_dir).unwrap();
    std::fs::write(
        craftsman_dir.join("config.json"),
        serde_json::json!({
            "base_url": server.uri(),
            "api_key": "k",
            "model": "mock-model"
        })
        .to_string(),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_craftsman");
    let mut command = tokio::process::Command::new(bin);
    command
        .args(["-m", "try something destructive"])
        .stdin(std::process::Stdio::null());
    set_test_home(&mut command, home.path());
    let out = command.output().await.unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("understood"), "stdout: {stdout}");
}

struct ProfileAwareLlm;

impl Respond for ProfileAwareLlm {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["temperature"], 0.37);
        assert_eq!(body["max_tokens"], 77);

        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(
            system.starts_with(
                "You are Craftsman, a capable general assistant running in a terminal."
            ),
            "general fallback base was not preserved: {system}"
        );
        assert!(system.contains("<approach>"), "Nemotron suffix missing");
        assert!(
            system.contains("You are running non-interactively."),
            "one-shot prompt layer missing"
        );

        let read_file = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == "read_file")
            .expect("read_file tool missing");
        let description = read_file["function"]["description"].as_str().unwrap();
        assert!(description.contains("pagination"));
        assert!(description.contains("always read a file before editing it"));

        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse(&[
                r#"{"choices":[{"delta":{"content":"profile-ok"}}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            ]))
    }
}

#[tokio::test]
async fn one_shot_applies_harness_provider_and_python_model_profiles() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("x-billing-invoke-origin", "DeepAgents"))
        .respond_with(ProfileAwareLlm)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let craftsman_dir = home.path().join(".craftsman");
    std::fs::create_dir_all(&craftsman_dir).unwrap();
    std::fs::write(
        craftsman_dir.join("config.json"),
        serde_json::json!({
            "base_url": server.uri(),
            "api_key": "test-key",
            "model": "nemotron-3-ultra",
            "provider_profile": "nvidia",
            "model_profiles": {
                "nemotron-3-ultra": {
                    "temperature": 0.37,
                    "max_tokens": 77
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_craftsman");
    let mut command = tokio::process::Command::new(bin);
    command.args(["-m", "verify the active profile", "--auto-approve"]);
    set_test_home(&mut command, home.path());
    let out = command.output().await.unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status
    );
    assert!(stdout.contains("profile-ok"), "stdout: {stdout}");
}

struct PlainReply;

impl Respond for PlainReply {
    fn respond(&self, _req: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse(&[
                r#"{"choices":[{"delta":{"content":"plugin-ok"}}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":2}}"#,
            ]))
    }
}

/// Plugins are data on disk; this drives the real binary to prove they reach
/// a real session build: the global and project directories both load, a
/// project plugin overrides a global one of the same name, a broken manifest
/// is ignored, instructions land in the system prompt, and the plugin's
/// agent is offered to the model through the `task` tool.
#[tokio::test]
async fn one_shot_loads_global_and_project_plugins() {
    assert_one_shot_loads_extensions(false).await;
}

#[cfg(feature = "gadgets")]
#[tokio::test]
async fn one_shot_loads_migrated_global_and_project_gadgets_without_duplicates() {
    assert_one_shot_loads_extensions(true).await;
}

async fn assert_one_shot_loads_extensions(migrate: bool) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(PlainReply)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    // CRAFTSMAN_CONFIG_DIR deliberately differs from $HOME/.craftsman: a global
    // plugin directory derived from HOME instead would not be found.
    let craftsman_dir = home.path().join("custom-config");
    std::fs::create_dir_all(craftsman_dir.join("plugins")).unwrap();
    std::fs::write(
        craftsman_dir.join("config.json"),
        serde_json::json!({
            "base_url": server.uri(),
            "api_key": "test-key",
            "model": "mock-model",
            "gadgets": {"enabled": migrate}
        })
        .to_string(),
    )
    .unwrap();
    for (file, name, rule) in [
        ("house.json", "house", "GLOBAL-HOUSE-RULE"),
        ("shared.json", "shared", "GLOBAL-SHARED-RULE"),
    ] {
        std::fs::write(
            craftsman_dir.join("plugins").join(file),
            serde_json::json!({"version": 1, "name": name, "prompt": rule}).to_string(),
        )
        .unwrap();
    }

    let workspace = tempfile::tempdir().unwrap();
    let project_plugins = workspace.path().join(".craftsman/plugins");
    std::fs::create_dir_all(&project_plugins).unwrap();
    std::fs::write(
        project_plugins.join("zz-project-override.json"),
        serde_json::json!({
            "version": 1,
            "name": "shared",
            "prompt": "PROJECT-SHARED-RULE",
            "agents": [{
                "name": "e2e-reviewer",
                "description": "Reviews code in the end-to-end test",
                "system_prompt": "You review code.",
                "tools": ["read_file"]
            }]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(project_plugins.join("broken.json"), "}{").unwrap();

    let bin = env!("CARGO_BIN_EXE_craftsman");
    if migrate {
        let originals: Vec<_> = [
            craftsman_dir.join("plugins/house.json"),
            craftsman_dir.join("plugins/shared.json"),
            project_plugins.join("zz-project-override.json"),
            project_plugins.join("broken.json"),
        ]
        .into_iter()
        .map(|p| {
            let bytes = std::fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect();
        for args in [
            vec!["gadgets", "migrate"],
            vec!["gadgets", "migrate", "--project"],
        ] {
            let mut migration = tokio::process::Command::new(bin);
            migration.args(args).current_dir(workspace.path());
            set_test_home(&mut migration, home.path());
            migration.env("CRAFTSMAN_CONFIG_DIR", &craftsman_dir);
            let out = migration.output().await.unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["complete"],
                true
            );
        }
        for (path, bytes) in originals {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
    let mut command = tokio::process::Command::new(bin);
    command
        .args(["-m", "hello", "--auto-approve"])
        .current_dir(workspace.path());
    set_test_home(&mut command, home.path());
    command.env("CRAFTSMAN_CONFIG_DIR", &craftsman_dir);
    let out = command.output().await.unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status
    );
    assert!(stdout.contains("plugin-ok"), "stdout: {stdout}");
    assert_eq!(
        stderr.matches("Legacy plugins are deprecated").count(),
        usize::from(!migrate),
        "warn once for effective legacy manifests, not for retained originals"
    );

    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains(if migrate {
            "# Gadget instructions"
        } else {
            "# Plugin instructions"
        }),
        "{system}"
    );
    assert_eq!(system.matches("GLOBAL-HOUSE-RULE").count(), 1, "{system}");
    assert_eq!(system.matches("PROJECT-SHARED-RULE").count(), 1, "{system}");
    assert!(
        !system.contains("GLOBAL-SHARED-RULE"),
        "the project plugin must replace the global one of the same name"
    );
    // The subagent middleware lists the available agent types in the system
    // prompt; the plugin's agent must sit beside the built-in one.
    assert!(
        body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "task"),
        "the task tool is offered"
    );
    assert!(system.contains("general-purpose"), "{system}");
    assert!(system.contains("e2e-reviewer"), "{system}");
    assert!(
        system.contains("Reviews code in the end-to-end test"),
        "{system}"
    );
}

/// The central safety property of plugin agents: the tools a plugin agent
/// asks for stay behind the approval gate. With no approver (no
/// `--auto-approve`, and a delegated run has no interactive channel) a gated
/// call from a plugin subagent is refused, nothing is written, and the refusal
/// is what the subagent's model sees.
#[tokio::test]
async fn plugin_agent_tools_stay_behind_the_approval_gate() {
    assert_extension_agent_tools_stay_gated(false).await;
}

#[cfg(feature = "gadgets")]
#[tokio::test]
async fn migrated_gadget_agent_tools_stay_behind_the_approval_gate() {
    assert_extension_agent_tools_stay_gated(true).await;
}

async fn assert_extension_agent_tools_stay_gated(migrate: bool) {
    const AGENT_MARKER: &str = "PLUGIN-AGENT-SYSTEM-PROMPT-9d41";

    struct DelegatingLlm {
        target: String,
    }

    fn tool_call(id: &str, name: &str, args: serde_json::Value) -> String {
        let call = serde_json::json!({"choices": [{"delta": {"tool_calls": [{
            "index": 0,
            "id": id,
            "function": {"name": name, "arguments": args.to_string()}
        }]}}]})
        .to_string();
        sse(&[
            &call,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ])
    }

    fn text(content: &str) -> String {
        let delta = serde_json::json!({"choices": [{"delta": {"content": content}}]}).to_string();
        sse(&[
            &delta,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ])
    }

    impl Respond for DelegatingLlm {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let messages = body["messages"].as_array().unwrap();
            let system = messages[0]["content"].as_str().unwrap_or_default();
            let has_tool_result = messages.iter().any(|m| m["role"] == "tool");
            let events = if system.contains(AGENT_MARKER) {
                // The plugin agent: try to write the file, then report.
                if has_tool_result {
                    text("subagent-finished")
                } else {
                    tool_call(
                        "sub_write",
                        "write_file",
                        serde_json::json!({
                            "file_path": self.target,
                            "content": "written by the plugin agent"
                        }),
                    )
                }
            } else if has_tool_result {
                text("main-finished")
            } else {
                tool_call(
                    "main_task",
                    "task",
                    serde_json::json!({
                        "description": "write the file",
                        "subagent_type": "writer"
                    }),
                )
            };
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(events)
        }
    }

    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("created-by-plugin-agent.txt");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(DelegatingLlm {
            target: target.to_string_lossy().into_owned(),
        })
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let craftsman_dir = home.path().join(".craftsman");
    std::fs::create_dir_all(&craftsman_dir).unwrap();
    std::fs::write(
        craftsman_dir.join("config.json"),
        serde_json::json!({
            "base_url": server.uri(),
            "api_key": "test-key",
            "model": "mock-model",
            "gadgets": {"enabled": migrate}
        })
        .to_string(),
    )
    .unwrap();
    let plugins = workspace.path().join(".craftsman/plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(
        plugins.join("writer.json"),
        serde_json::json!({
            "version": 1,
            "name": "writer-plugin",
            "agents": [{
                "name": "writer",
                "description": "Writes files when asked",
                "system_prompt": format!("You write files. {AGENT_MARKER}"),
                "tools": ["write_file"]
            }]
        })
        .to_string(),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_craftsman");
    if migrate {
        let mut migration = tokio::process::Command::new(bin);
        migration
            .args(["gadgets", "migrate", "--project"])
            .current_dir(workspace.path());
        set_test_home(&mut migration, home.path());
        let out = migration.output().await.unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            plugins.join("writer.json").is_file(),
            "migration must keep its original"
        );
    }
    let mut command = tokio::process::Command::new(bin);
    command
        .args(["-m", "have the writer agent create the file"])
        .current_dir(workspace.path())
        .stdin(std::process::Stdio::null());
    set_test_home(&mut command, home.path());
    let out = command.output().await.unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status
    );
    assert!(stdout.contains("main-finished"), "stdout: {stdout}");
    assert!(
        !stdout.contains("subagent-finished"),
        "child-provider deltas must not contaminate one-shot stdout: {stdout}"
    );
    assert!(
        !target.exists(),
        "the gated write must not run without approval"
    );

    // The plugin agent's model saw the refusal as its tool result.
    let requests = server.received_requests().await.unwrap();
    let refused = requests
        .iter()
        .filter_map(|request| serde_json::from_slice::<serde_json::Value>(&request.body).ok())
        .any(|body| {
            let messages = body["messages"].as_array().cloned().unwrap_or_default();
            let system = messages[0]["content"].as_str().unwrap_or_default();
            system.contains(AGENT_MARKER)
                && messages.iter().any(|message| {
                    message["role"] == "tool"
                        && message["content"]
                            .as_str()
                            .is_some_and(|content| content.contains("requires user approval"))
                })
        });
    assert!(
        refused,
        "the subagent should have been told the call needs approval"
    );
}

/// Planted in the service-agent test's home: that runtime is structurally
/// sandboxed and must never fold plugin text into its prompt.
const PLUGIN_LEAK_MARKER: &str = "PLUGIN-LEAK-MARKER-7f3a";

struct ServiceAgentLlm;

impl Respond for ServiceAgentLlm {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        let tools = body["tools"].as_array().unwrap();
        let tool_names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        assert_eq!(tool_names, ["read_file", "write_todos"]);

        let system = messages[0]["content"].as_str().unwrap();
        assert!(system.contains("shared service-agent profile"));
        assert!(system.contains("no real host filesystem or shell access"));
        assert!(
            !system.contains(PLUGIN_LEAK_MARKER),
            "the service-agent runtime must never load plugins"
        );

        let has_tool_result = messages.iter().any(|message| message["role"] == "tool");
        let events = if has_tool_result {
            let tool_message = messages
                .iter()
                .find(|message| message["role"] == "tool")
                .unwrap();
            let content = tool_message["content"].as_str().unwrap_or_default();
            assert!(content.contains("Demo skill instructions"));
            sse(&[
                r#"{"choices":[{"delta":{"content":"skill-read-ok"}}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":3}}"#,
            ])
        } else {
            sse(&[
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"skill_1","function":{"name":"read_file","arguments":"{\"file_path\":\"/skills/demo/SKILL.md\"}"}}]}}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":4}}"#,
            ])
        };
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(events)
    }
}

#[tokio::test]
async fn service_agent_reads_virtual_skill_without_host_or_shell_tools() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ServiceAgentLlm)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let craftsman_dir = home.path().join(".craftsman");
    std::fs::create_dir_all(&craftsman_dir).unwrap();
    std::fs::write(
        craftsman_dir.join("config.json"),
        serde_json::json!({
            "base_url": server.uri(),
            "api_key": "test-key",
            "model": "mock-model",
            "gadgets": {"enabled": true}
        })
        .to_string(),
    )
    .unwrap();
    let request_path = home.path().join("request.json");
    // A global plugin must stay out of the sandboxed runtime's prompt.
    std::fs::create_dir_all(craftsman_dir.join("plugins")).unwrap();
    std::fs::write(
        craftsman_dir.join("plugins/leak.json"),
        serde_json::json!({"version": 1, "name": "leak", "prompt": PLUGIN_LEAK_MARKER}).to_string(),
    )
    .unwrap();
    // Even an opted-in user config must not load gadget prompts/agents here.
    std::fs::create_dir_all(craftsman_dir.join("gadgets/leak")).unwrap();
    std::fs::write(
        craftsman_dir.join("gadgets/leak/gadget.toml"),
        format!("id='leak'\nversion='0.1.0'\nprompt='{PLUGIN_LEAK_MARKER}'\n"),
    )
    .unwrap();
    std::fs::write(
        &request_path,
        serde_json::json!({
            "version": 1,
            "system_prompt": "Follow the mounted skill when it is relevant.",
            "input": "Read the demo skill and confirm.",
            "allowed_tools": ["read_file"],
            "files": [{
                "path": "/skills/demo/SKILL.md",
                "content": "# Demo skill instructions\nReturn a concise confirmation."
            }]
        })
        .to_string(),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_craftsman");
    let mut command = tokio::process::Command::new(bin);
    command.args(["service-agent", "--request", request_path.to_str().unwrap()]);
    set_test_home(&mut command, home.path());
    let out = command.output().await.unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status
    );
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["profile"], "service-agent");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["output"], "skill-read-ok");
    assert_eq!(result["modified_files"], serde_json::json!([]));
    assert!(!stderr.contains("Legacy plugins are deprecated"));
}

struct ServiceAgentWebInventoryLlm;

impl Respond for ServiceAgentWebInventoryLlm {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let tool_names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        assert_eq!(tool_names, ["web_fetch", "web_search", "write_todos"]);
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("public HTTPS destinations"));
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse(&[
                r#"{"choices":[{"delta":{"content":"web-tools-ready"}}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            ]))
    }
}

#[tokio::test]
async fn service_agent_registers_only_selected_public_web_tools() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ServiceAgentWebInventoryLlm)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let craftsman_dir = home.path().join(".craftsman");
    std::fs::create_dir_all(&craftsman_dir).unwrap();
    std::fs::write(
        craftsman_dir.join("config.json"),
        serde_json::json!({
            "base_url": server.uri(),
            "api_key": "test-key",
            "model": "mock-model"
        })
        .to_string(),
    )
    .unwrap();
    let request_path = home.path().join("request.json");
    std::fs::write(
        &request_path,
        serde_json::json!({
            "version": 1,
            "system_prompt": "Use public web tools when needed.",
            "input": "Confirm the selected web tools.",
            "allowed_tools": ["web_fetch", "web_search"]
        })
        .to_string(),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_craftsman");
    let mut command = tokio::process::Command::new(bin);
    command.args(["service-agent", "--request", request_path.to_str().unwrap()]);
    set_test_home(&mut command, home.path());
    let out = command.output().await.unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status
    );
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["output"], "web-tools-ready");
}
