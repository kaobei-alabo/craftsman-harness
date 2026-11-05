use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::permissions::{InvocationPermissions, Mode, OnUnapproved, Selectors};
use async_trait::async_trait;
use craftsman_harness::{
    AgentError, AgentMiddleware, AgentRegistry, Backend, Checkpointer, ModelProvider, ModelRequest,
    ModelResponse, Msg, StateFs, Tool, ToolCall, ToolCtx, ToolDef, ToolOutcome, Usage,
};
use serde_json::{Value, json};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

struct Fixture {
    root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    server: MockServer,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let f = Self {
            root,
            workspace,
            config,
            server: MockServer::start().await,
        };
        f.configure(json!({}));
        f
    }
    fn configure(&self, extra: Value) {
        let mut config = json!({
            "model":"synthetic-model", "api_key":"synthetic-provider-key",
            "base_url":self.server.uri(),
        });
        config
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        write(&self.config, "config.json", &config.to_string());
    }
    fn options(&self, selectors: Selectors) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session: SessionSelection::Ephemeral,
            selectors,
        }
    }
    async fn runtime(&self, selectors: Selectors) -> Runtime {
        Runtime::build(Prepared::prepare(self.options(selectors)).await.unwrap())
            .await
            .unwrap()
    }
    async fn script(&self, messages: Vec<Msg>) {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(Script(Mutex::new(messages.into())))
            .mount(&self.server)
            .await;
    }
    async fn requests(&self) -> Vec<Value> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == "/chat/completions")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }
}

fn write(base: &Path, relative: &str, text: &str) {
    let target = base.join(relative);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, text).unwrap();
}
fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        args,
    }
}
fn tools(calls: Vec<ToolCall>) -> Msg {
    Msg::Assistant {
        content: "synthetic partial progress".into(),
        tool_calls: calls,
    }
}
fn allowed(names: &[&str]) -> Selectors {
    Selectors {
        allow: names.iter().map(|name| format!("tool:{name}")).collect(),
        ..Default::default()
    }
}
fn plugin(name: &str, tools: &[&str]) -> String {
    json!({"version":1,"name":"scoped-extension","agents":[{
        "name":name,"description":"Scoped delegate", "system_prompt":"DECLARED-ROLE-CONTEXT",
        "tools":tools,
    }]})
    .to_string()
}
fn system(request: &Value) -> &str {
    request["messages"][0]["content"].as_str().unwrap()
}
fn names(request: &Value) -> BTreeSet<&str> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect()
}
fn result<'a>(state: &'a AgentState, id: &str) -> (&'a str, bool) {
    state
        .messages
        .iter()
        .find_map(|message| match message {
            Msg::Tool {
                tool_call_id,
                content,
                is_error,
            } if tool_call_id == id => Some((content.as_str(), *is_error)),
            _ => None,
        })
        .unwrap()
}

struct Script(Mutex<VecDeque<Msg>>);
impl Respond for Script {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let Some(message) = self.0.lock().unwrap().pop_front() else {
            return ResponseTemplate::new(400).set_body_string("unexpected synthetic request");
        };
        let Msg::Assistant {
            content,
            tool_calls,
        } = message
        else {
            panic!("assistant script")
        };
        let calls: Vec<Value> = tool_calls
            .iter()
            .enumerate()
            .map(|(index, call)| {
                json!({
                    "index":index, "id":call.id, "type":"function",
                    "function":{"name":call.name,"arguments":call.args.to_string()},
                })
            })
            .collect();
        let finish = if calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        let mut body = format!(
            "data: {}\n\n",
            json!({"choices":[{"delta":{"content":content,"tool_calls":calls}}]})
        );
        body.push_str(&format!(
            "data: {}\n\n",
            json!({"choices":[{"delta":{},"finish_reason":finish}]})
        ));
        body.push_str("data: {\"choices\":[],\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":5}}\n\ndata: [DONE]\n\n");
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body)
    }
}

#[tokio::test]
async fn preparation_enters_shared_builder_without_legacy_services_or_writers() {
    let f = Fixture::new().await;
    f.configure(json!({
        "auto_approve":true,"permission_mode":"bypass-permissions", "auto_approve_web":true,
        "mcp_servers":[{"transport":"stdio","command":"must-not-start"}],
        "lsp_servers":{"custom":{"command":"must-not-start","extensions":["rs"]}},
        "shell":{"env_scrub":false},"gadgets":{"enabled":true},
    }));
    f.script(vec![Msg::assistant("done")]).await;
    let runtime = f.runtime(Selectors::default()).await;
    assert!(
        f.requests().await.is_empty(),
        "build is not a model request"
    );
    assert!(runtime.prepared().ignored_ambient_settings());
    assert!(runtime.session.memories.is_none());
    assert!(runtime.session.checkpointer.is_none());
    assert!(runtime.session.workspace_checkpoints.is_none());
    assert!(runtime.session.mcp.is_empty());
    assert!(runtime.session.mcp_errors.is_empty());
    assert!(
        runtime.session.lsp.statuses().await.is_empty(),
        "empty declarations alone would select built-ins"
    );
    assert!(runtime.session.lsp.diagnostics("missing.rs").await.is_err());
    #[cfg(feature = "gadgets")]
    assert!(runtime.session.gadgets.is_none());
    runtime.run_turn("test".into()).await.unwrap();
    let requests = f.requests().await;
    assert_eq!(requests.len(), 1);
    assert!(
        !names(&requests[0])
            .iter()
            .any(|name| name.starts_with("lsp_") || name.starts_with("gadget_"))
    );
    assert!(!system(&requests[0]).contains("<agent_memory>"));
    assert!(!system(&requests[0]).contains("Long-term memory lives"));
    assert!(!system(&requests[0]).contains("# Memory protocol"));
    assert!(runtime.prepared().persistence().thread_id().is_none());
    assert!(!f.config.join("threads.db").exists());
    assert!(!f.config.join("sessions-rs").exists());
    assert!(!f.config.join("gadgets/.sdk").exists());
}

#[tokio::test]
async fn scoped_snapshot_reaches_main_and_declared_child_without_rediscovery() {
    let f = Fixture::new().await;
    write(
        &f.config,
        "agents/craftsman/AGENTS.md",
        "ORIGINAL-USER-INSTRUCTION",
    );
    write(
        &f.config,
        "skills/local/SKILL.md",
        "---\nname: local-skill\n---\nINLINE-SKILL-CONTENT",
    );
    write(
        &f.config,
        "plugins/local.json",
        &plugin(
            "scoped-review",
            &[
                "read_file",
                "write_file",
                "task",
                "send_message",
                "lsp_diagnostics",
                "gadget_call",
            ],
        ),
    );
    write(
        &f.workspace,
        ".craftsman/AGENTS.md",
        "UNTRUSTED-PROJECT-MUST-NOT-LOAD",
    );
    let prepared = Prepared::prepare(f.options(Selectors::default()))
        .await
        .unwrap();
    // The adapter must not repeat global/project discovery after preparation.
    write(
        &f.config,
        "agents/craftsman/AGENTS.md",
        "SWAPPED-SOURCE-MUST-NOT-LOAD",
    );
    write(
        &f.config,
        "plugins/local.json",
        &plugin("swapped-review", &["execute"]),
    );
    f.script(vec![
        tools(vec![call(
            "delegate",
            "task",
            json!({"subagent_type":"scoped-review","description":"inspect"}),
        )]),
        Msg::assistant("child done"),
        Msg::assistant("main done"),
    ])
    .await;
    let runtime = Runtime::build(prepared).await.unwrap();
    runtime.run_turn("test".into()).await.unwrap();
    let requests = f.requests().await;
    assert_eq!(requests.len(), 3);
    for request in &requests {
        let prompt = system(request);
        assert!(prompt.contains("ORIGINAL-USER-INSTRUCTION"));
        assert!(prompt.contains("INLINE-SKILL-CONTENT"));
        assert!(!prompt.contains("SWAPPED-SOURCE-MUST-NOT-LOAD"));
        assert!(!prompt.contains("UNTRUSTED-PROJECT-MUST-NOT-LOAD"));
        assert!(!prompt.contains("<agent_memory>"));
        assert!(
            !prompt.contains(&f.config.to_string_lossy().to_string()),
            "skills are inline, not inaccessible paths"
        );
    }
    assert!(system(&requests[1]).contains("DECLARED-ROLE-CONTEXT"));
    assert_eq!(
        names(&requests[1]),
        BTreeSet::from(["read_file", "write_file"])
    );
    let task = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["function"]["name"] == "task")
        .unwrap();
    assert!(
        task["function"]["parameters"]["properties"]
            .get("run_in_background")
            .is_none()
    );
}

#[tokio::test]
async fn workspace_trust_controls_runtime_context_and_agent_declarations() {
    for trusted in [false, true] {
        let f = Fixture::new().await;
        write(
            &f.workspace,
            ".craftsman/AGENTS.md",
            "PROJECT-TRUST-CONTEXT",
        );
        write(
            &f.workspace,
            ".craftsman/plugins/local.json",
            &plugin("project-role", &["read_file"]),
        );
        f.script(vec![Msg::assistant("done")]).await;
        let mut options = f.options(Selectors::default());
        options.trust_workspace = trusted;
        let runtime = Runtime::build(Prepared::prepare(options).await.unwrap())
            .await
            .unwrap();
        runtime.run_turn("test".into()).await.unwrap();
        let requests = f.requests().await;
        assert_eq!(
            system(&requests[0]).contains("PROJECT-TRUST-CONTEXT"),
            trusted
        );
        assert_eq!(system(&requests[0]).contains("project-role"), trusted);
    }
}

#[tokio::test]
async fn main_and_delegated_tools_share_policy_and_confined_authority() {
    let f = Fixture::new().await;
    write(&f.workspace, "local.txt", "LOCAL-DATA");
    write(&f.workspace, ".env", "HIDDEN-CREDENTIAL-DATA");
    write(f.root.path(), "outside.txt", "OUTSIDE-MUST-NOT-READ");
    write(
        &f.config,
        "plugins/local.json",
        &plugin(
            "scoped-review",
            &["read_file", "write_file", "task", "send_message"],
        ),
    );
    let mut selectors = allowed(&["write_file"]);
    selectors.deny.push("path:blocked.txt".into());
    f.script(vec![
        tools(vec![
            call("local", "read_file", json!({"file_path":"local.txt"})),
            call(
                "outside",
                "read_file",
                json!({"file_path":f.root.path().join("outside.txt")}),
            ),
            call(
                "main-write",
                "write_file",
                json!({"file_path":"main.txt","content":"main"}),
            ),
            call(
                "protected",
                "write_file",
                json!({"file_path":"build.rs","content":"must not write"}),
            ),
            call(
                "delegate",
                "task",
                json!({"subagent_type":"scoped-review","description":"inspect"}),
            ),
        ]),
        tools(vec![
            call(
                "child-write",
                "write_file",
                json!({"file_path":"child.txt","content":"child"}),
            ),
            call("child-secret", "read_file", json!({"file_path":".env"})),
            call(
                "child-deny",
                "write_file",
                json!({"file_path":"blocked.txt","content":"must not write"}),
            ),
        ]),
        Msg::assistant("child done"),
        Msg::assistant("main done"),
    ])
    .await;
    let runtime = f.runtime(selectors).await;
    runtime.run_turn("test".into()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(f.workspace.join("main.txt")).unwrap(),
        "main"
    );
    assert_eq!(
        std::fs::read_to_string(f.workspace.join("child.txt")).unwrap(),
        "child"
    );
    assert!(!f.workspace.join("blocked.txt").exists());
    assert!(!f.workspace.join("build.rs").exists());
    let state = runtime.retained_state();
    assert!(result(&state, "local").0.contains("LOCAL-DATA"));
    assert!(result(&state, "outside").1);
    assert!(
        !result(&state, "outside")
            .0
            .contains("OUTSIDE-MUST-NOT-READ")
    );
    assert!(result(&state, "protected").1);
    let requests = f.requests().await;
    assert_eq!(requests.len(), 4);
    let child = &requests[2]["messages"];
    for id in ["child-secret", "child-deny"] {
        let message = child
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["tool_call_id"] == id)
            .unwrap();
        // The OpenAI wire does not carry is_error; content records the denial.
        assert!(
            !message["content"]
                .as_str()
                .unwrap()
                .contains("HIDDEN-CREDENTIAL-DATA")
        );
        assert!(message["content"].as_str().unwrap().contains("denied"));
    }
}

#[tokio::test]
async fn explicit_plan_cannot_be_widened_by_allowed_tools() {
    let f = Fixture::new().await;
    write(&f.workspace, "existing.txt", "before");
    let mut selectors = allowed(&["write_file", "edit_file", "execute"]);
    selectors.mode = Mode::Plan;
    f.script(vec![
        tools(vec![
            call(
                "write",
                "write_file",
                json!({"file_path":"new.txt","content":"after"}),
            ),
            call(
                "edit",
                "edit_file",
                json!({"file_path":"existing.txt","old_string":"before","new_string":"after"}),
            ),
            call("exec", "execute", json!({"command":"echo MUST-NOT-RUN"})),
        ]),
        Msg::assistant("plan done"),
    ])
    .await;
    let runtime = f.runtime(selectors).await;
    runtime.run_turn("test".into()).await.unwrap();
    let state = runtime.retained_state();
    for id in ["write", "edit", "exec"] {
        assert!(result(&state, id).1);
    }
    assert!(!f.workspace.join("new.txt").exists());
    assert_eq!(
        std::fs::read_to_string(f.workspace.join("existing.txt")).unwrap(),
        "before"
    );
    assert!(!runtime.gate.approval_required());
}

#[cfg(unix)]
#[tokio::test]
async fn actual_shell_dispatch_requires_whole_command_prefix_match() {
    let f = Fixture::new().await;
    let selectors = Selectors {
        allow: vec!["exec:[\"printf\",\"PERMITTED-NATIVE-OUTPUT\"]".into()],
        ..Default::default()
    };
    f.script(vec![
        tools(vec![
            call(
                "plain",
                "execute",
                json!({"command":"printf PERMITTED-NATIVE-OUTPUT"}),
            ),
            call(
                "chain",
                "execute",
                json!({"command":"printf PERMITTED-NATIVE-OUTPUT; printf BAD > chain.txt"}),
            ),
        ]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = f.runtime(selectors).await;
    runtime.run_turn("test".into()).await.unwrap();
    let state = runtime.retained_state();
    assert!(!result(&state, "plain").1);
    assert_eq!(result(&state, "plain").0, "PERMITTED-NATIVE-OUTPUT");
    assert!(result(&state, "chain").1);
    assert!(!f.workspace.join("chain.txt").exists());
}

fn sentinel_command() -> &'static str {
    if cfg!(windows) {
        "Set-Content -LiteralPath 'later.txt' -Value 'must-not-run'"
    } else {
        "printf must-not-run > later.txt"
    }
}

#[tokio::test]
async fn unapproved_fail_latches_before_later_same_response_tools_or_model_calls() {
    let f = Fixture::new().await;
    let mut selectors = allowed(&["execute"]);
    selectors.on_unapproved = OnUnapproved::Fail;
    f.script(vec![tools(vec![
        call(
            "unapproved",
            "write_file",
            json!({"file_path":"unapproved.txt","content":"no"}),
        ),
        call("later", "execute", json!({"command":sentinel_command()})),
        call("later-read", "ls", json!({"path":"."})),
    ])])
    .await;
    let runtime = f.runtime(selectors).await;
    assert!(matches!(
        runtime.run_turn("test".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    assert!(!f.workspace.join("unapproved.txt").exists());
    assert!(!f.workspace.join("later.txt").exists());
    let state = runtime.retained_state();
    assert!(state.messages.iter().any(
        |m| matches!(m, Msg::Assistant { content, .. } if content == "synthetic partial progress")
    ));
    for id in ["unapproved", "later", "later-read"] {
        assert!(result(&state, id).1);
    }
    assert!(result(&state, "later").0.contains("not executed"));
    assert_eq!(f.requests().await.len(), 1);
    let count = state.messages.len();
    assert!(matches!(
        runtime
            .run_turn("cannot restart a sealed invocation".into())
            .await,
        Err(RuntimeError::ApprovalRequired)
    ));
    assert_eq!(runtime.retained_state().messages.len(), count);
    assert_eq!(f.requests().await.len(), 1);
    // Compaction bypasses around_model, so its provider must also be guarded.
    assert!(
        runtime
            .session
            .compaction
            .compact_now(&state.messages)
            .await
            .is_err()
    );
    assert_eq!(f.requests().await.len(), 1);
}

#[tokio::test]
async fn child_unapproved_fail_seals_parent_and_sibling_tools() {
    let f = Fixture::new().await;
    let mut selectors = allowed(&["write_file"]);
    selectors.on_unapproved = OnUnapproved::Fail;
    f.script(vec![
        tools(vec![
            call(
                "delegate",
                "task",
                json!({"subagent_type":"general-purpose","description":"child"}),
            ),
            call(
                "parent-later",
                "write_file",
                json!({"file_path":"parent-later.txt","content":"no"}),
            ),
        ]),
        tools(vec![
            call(
                "child-unapproved",
                "execute",
                json!({"command":sentinel_command()}),
            ),
            call(
                "child-later",
                "write_file",
                json!({"file_path":"child-later.txt","content":"no"}),
            ),
        ]),
    ])
    .await;
    let runtime = f.runtime(selectors).await;
    assert!(matches!(
        runtime.run_turn("test".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    assert_eq!(f.requests().await.len(), 2);
    assert!(!f.workspace.join("parent-later.txt").exists());
    assert!(!f.workspace.join("child-later.txt").exists());
    assert!(!f.workspace.join("later.txt").exists());
    assert!(result(&runtime.retained_state(), "parent-later").1);
    assert!(
        runtime
            .session
            .registry
            .list()
            .iter()
            .all(|(_, status)| *status == craftsman_harness::AgentStatus::Failed)
    );
}

#[tokio::test]
async fn deny_and_hard_unavailable_calls_are_not_approval_failures() {
    let f = Fixture::new().await;
    let mut selectors = allowed(&["write_file"]);
    selectors.deny.push("tool:write_file".into());
    selectors.on_unapproved = OnUnapproved::Fail;
    f.script(vec![tools(vec![
        call("deny", "write_file", json!({"file_path":"denied.txt","content":"no"})),
        call("ask", "ask_user", json!({"question":"must not read stdin"})),
        call("background", "task", json!({"subagent_type":"general-purpose","description":"no","run_in_background":true})),
    ]), Msg::assistant("done despite denials")]).await;
    let runtime = f.runtime(selectors).await;
    assert_eq!(
        runtime.run_turn("test".into()).await.unwrap().final_text(),
        "done despite denials"
    );
    assert!(!runtime.gate.approval_required());
    assert!(runtime.session.registry.list().is_empty());
    assert!(!f.workspace.join("denied.txt").exists());
}

#[tokio::test]
async fn background_and_recursive_delegation_never_dispatch() {
    let f = Fixture::new().await;
    f.script(vec![
        tools(vec![
            call("background", "task", json!({"subagent_type":"general-purpose","description":"no","run_in_background":true})),
            call("ambiguous", "task", json!({"subagent_type":"general-purpose","description":"no","run_in_background":"true"})),
            call("foreground", "task", json!({"subagent_type":"general-purpose","description":"yes","run_in_background":false})),
        ]),
        tools(vec![
            call("recurse", "task", json!({"subagent_type":"general-purpose","description":"no"})),
            call("message", "send_message", json!({"to":"main","message":"no"})),
        ]),
        Msg::assistant("child done"), Msg::assistant("main done"),
    ]).await;
    let runtime = f.runtime(Selectors::default()).await;
    runtime.run_turn("test".into()).await.unwrap();
    let requests = f.requests().await;
    assert_eq!(requests.len(), 4);
    assert!(!names(&requests[1]).contains("task"));
    assert!(!names(&requests[1]).contains("send_message"));
    let entries = runtime.session.registry.list();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "general-purpose#0");
    assert_eq!(entries[0].1, craftsman_harness::AgentStatus::Done);
    let state = runtime.retained_state();
    assert!(result(&state, "background").1);
    assert!(result(&state, "ambiguous").1);
}

#[tokio::test]
async fn resumed_state_has_no_approval_bypass_or_raw_checkpoint_and_journal_writes() {
    let f = Fixture::new().await;
    let store = crate::checkpoints::LibsqlCheckpointer::open_at_for_headless(
        f.config.join("threads.db"),
        f.workspace
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    )
    .await
    .unwrap();
    store.create_headless_thread("saved").await.unwrap();
    let mut state = AgentState::default();
    state.messages.push(Msg::user("prior conversation"));
    state.messages.push(tools(vec![call(
        "pending",
        "execute",
        json!({"command":sentinel_command()}),
    )]));
    state.custom.insert("auto_approve".into(), json!(true));
    state.system_extras.push("STALE-BROAD-POLICY".into());
    state.files.insert(
        "/memories/prior".into(),
        craftsman_harness::FileData::text("STALE-FILE-CONTEXT"),
    );
    store.save("saved", 1, &state, None).await.unwrap();
    let before = store.load_latest("saved").await.unwrap().unwrap();
    let mut options = f.options(Selectors::default());
    options.session = SessionSelection::Resume("saved".into());
    let runtime = Runtime::build(Prepared::prepare(options).await.unwrap())
        .await
        .unwrap();
    assert!(runtime.session.agent.pending_suspension().is_none());
    assert!(runtime.retained_state().custom.is_empty());
    assert!(runtime.retained_state().files.is_empty());
    assert!(result(&runtime.retained_state(), "pending").1);
    assert!(runtime.session.checkpointer.is_none());
    f.script(vec![
        tools(vec![call(
            "new-unapproved",
            "execute",
            json!({"command":sentinel_command()}),
        )]),
        Msg::assistant("SYNTHETIC-NEW-PRIVATE-OUTPUT"),
    ])
    .await;
    runtime
        .run_turn("SYNTHETIC-NEW-PRIVATE-INPUT".into())
        .await
        .unwrap();
    assert!(!f.workspace.join("later.txt").exists());
    let requests = f.requests().await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]["messages"]
            .to_string()
            .contains("prior conversation")
    );
    assert!(!system(&requests[0]).contains("STALE-BROAD-POLICY"));
    let after = store.load_latest("saved").await.unwrap().unwrap();
    assert_eq!(after.0, before.0);
    assert_eq!(
        serde_json::to_value(after.1).unwrap(),
        serde_json::to_value(before.1).unwrap()
    );
    assert!(!f.config.join("sessions-rs").exists());
    assert!(
        runtime.retained_state().messages.iter().any(
            |m| matches!(m, Msg::User { content } if content == "SYNTHETIC-NEW-PRIVATE-INPUT")
        )
    );
}

#[cfg(unix)]
#[tokio::test]
async fn retained_filesystem_authority_is_not_reopened_by_runtime_construction() {
    let f = Fixture::new().await;
    write(&f.workspace, "identity.txt", "ORIGINAL-ROOT");
    let prepared = Prepared::prepare(f.options(allowed(&["write_file"])))
        .await
        .unwrap();
    let moved = f.root.path().join("retained-root");
    std::fs::rename(&f.workspace, &moved).unwrap();
    std::fs::create_dir(&f.workspace).unwrap();
    write(&f.workspace, "identity.txt", "REPLACEMENT-MUST-NOT-READ");
    f.script(vec![
        tools(vec![
            call("root", "read_file", json!({"file_path":"identity.txt"})),
            call(
                "write",
                "write_file",
                json!({"file_path":"new.txt","content":"retained"}),
            ),
        ]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = Runtime::build(prepared).await.unwrap();
    runtime.run_turn("test".into()).await.unwrap();
    assert!(
        result(&runtime.retained_state(), "root")
            .0
            .contains("ORIGINAL-ROOT")
    );
    assert!(
        !result(&runtime.retained_state(), "root")
            .0
            .contains("REPLACEMENT-MUST-NOT-READ")
    );
    assert!(moved.join("new.txt").is_file());
    assert!(!f.workspace.join("new.txt").exists());
}

#[cfg(feature = "gadgets")]
#[tokio::test]
async fn opted_in_declarative_gadgets_do_not_enable_callable_host_or_materialization() {
    let f = Fixture::new().await;
    f.configure(json!({"gadgets":{"enabled":true}}));
    write(
        &f.config,
        "gadgets/data/gadget.toml",
        "id='data'\nversion='1'\ndescription='Data only'\nprompt='DECLARATIVE-GADGET-CONTEXT'\n",
    );
    write(
        &f.config,
        "gadgets/data/gadget.wasm",
        "NOT-A-COMPONENT-MUST-NOT-COMPILE",
    );
    write(
        &f.config,
        "gadgets/.grants.json",
        "POISON-GRANTS-MUST-NOT-OPEN",
    );
    write(
        &f.config,
        "skills/build-gadget/SKILL.md",
        "---\nname: build-gadget\n---\nMUST-NOT-PROVIDE-BUILD-GUIDE",
    );
    f.script(vec![
        tools(vec![call(
            "guest",
            "gadget_call",
            json!({"gadget":"data","function":"no","arguments":{}}),
        )]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&["gadget_call", "gadget_list"])).await;
    runtime.run_turn("test".into()).await.unwrap();
    assert!(runtime.session.gadgets.is_none());
    let requests = f.requests().await;
    assert!(system(&requests[0]).contains("DECLARATIVE-GADGET-CONTEXT"));
    assert!(!system(&requests[0]).contains("MUST-NOT-PROVIDE-BUILD-GUIDE"));
    assert!(!names(&requests[0]).contains("gadget_call"));
    assert!(!names(&requests[0]).contains("gadget_list"));
    assert!(result(&runtime.retained_state(), "guest").1);
    assert!(!f.config.join("gadgets/.sdk").exists());
    assert!(!f.config.join("gadgets/.cache").exists());
    assert_eq!(
        std::fs::read_to_string(f.config.join("gadgets/.grants.json")).unwrap(),
        "POISON-GRANTS-MUST-NOT-OPEN"
    );
}

// Synthetic environment belongs ONLY to the isolated test child. Never mutate
// HOME/config/cwd or real credential variables of this agent's host process.
#[tokio::test]
async fn isolated_child_checks_scrub_scopes_no_stdio_fallback_and_idle_stdin() {
    let home = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "exec::runtime::tests::isolated_runtime_child",
            "--nocapture",
        ])
        .env("CRAFTSMAN_ADAPTER_CHILD", "1")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CRAFTSMAN_CONFIG_DIR", home.path().join("ambient-config"))
        .env("TMPDIR", home.path())
        .env("TMP", home.path())
        .env("TEMP", home.path())
        .env(
            "CRAFTSMAN_ADAPTER_SOURCE",
            "SYNTHETIC-DISABLED-MCP-CREDENTIAL",
        )
        .env(
            "CRAFTSMAN_ADAPTER_DENIED_KEY",
            "SYNTHETIC-AMBIENT-PASS-SECRET",
        )
        .env("CRAFTSMAN_ADAPTER_PUBLIC", "SYNTHETIC-EXPLICIT-PASS")
        .env("LANG", "SYNTHETIC-LEGACY-PASS-CREDENTIAL")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let idle_stdin = child.stdin.take().unwrap(); // Keep an idle, open pipe until exit.
    let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    drop(idle_stdin);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.contains("1 passed"),
        "the isolated probe must actually execute"
    );
    for marker in [
        "SYNTHETIC-MAIN-NO-PRINT",
        "SYNTHETIC-CHILD-NO-PRINT",
        "SYNTHETIC-COMPACTION-NO-PRINT",
        "SYNTHETIC-DISABLED-MCP-CREDENTIAL",
        "SYNTHETIC-AMBIENT-PASS-SECRET",
        "SYNTHETIC-EXPLICIT-PASS",
    ] {
        assert!(
            !output.contains(marker),
            "raw runtime data escaped a fallback sink"
        );
    }
    assert!(
        !home.path().join("ambient-config").exists(),
        "no ambient store or SDK may be opened"
    );
    assert!(!home.path().join(".craftsman").exists());
}

#[tokio::test]
async fn isolated_runtime_child() {
    if std::env::var_os("CRAFTSMAN_ADAPTER_CHILD").is_none() {
        return;
    }
    let f = Fixture::new().await;
    f.configure(json!({
        "permission_mode":"bypass-permissions","auto_approve":true,
        "shell":{"env_scrub":false,"env_pass":["CRAFTSMAN_ADAPTER_DENIED_KEY"],"output_format":"head_tail"},
        "mcp_servers":[{"transport":"stdio","command":"must-not-start", "env":{"key":"${CRAFTSMAN_ADAPTER_SOURCE}","locale":"${LANG}"}}],
    }));
    let command = if cfg!(windows) {
        "if (Test-Path Env:CRAFTSMAN_ADAPTER_SOURCE) {throw 'source survived'}; if (Test-Path Env:CRAFTSMAN_ADAPTER_DENIED_KEY) {throw 'ambient pass survived'}; if (Test-Path Env:LANG) {throw 'credential pass-list source survived'}; if ($env:CRAFTSMAN_ADAPTER_PUBLIC -ne 'SYNTHETIC-EXPLICIT-PASS') {throw 'explicit pass lost'}; Write-Output 'SCRUB-CONFIRMED'"
    } else {
        "test -z \"${CRAFTSMAN_ADAPTER_SOURCE+x}\" && test -z \"${CRAFTSMAN_ADAPTER_DENIED_KEY+x}\" && test -z \"${LANG+x}\" && test \"$CRAFTSMAN_ADAPTER_PUBLIC\" = SYNTHETIC-EXPLICIT-PASS && printf SCRUB-CONFIRMED"
    };
    f.script(vec![
        tools(vec![
            call(
                "ask",
                "ask_user",
                json!({"question":"idle stdin must not be read"}),
            ),
            call("shell", "execute", json!({"command":command})),
            call(
                "task",
                "task",
                json!({"description":"child","subagent_type":"general-purpose"}),
            ),
        ]),
        Msg::assistant("SYNTHETIC-CHILD-NO-PRINT"),
        Msg::assistant("SYNTHETIC-MAIN-NO-PRINT"),
        Msg::assistant("SYNTHETIC-COMPACTION-NO-PRINT"),
    ])
    .await;
    let mut selectors = allowed(&["execute"]);
    selectors.env_pass.push("CRAFTSMAN_ADAPTER_PUBLIC".into());
    let runtime = f.runtime(selectors).await;
    // Rebuilding shell settings from the sanitized Config loses these names.
    assert!(
        !crate::shell::ShellSettings::from_config(runtime.prepared().config())
            .secret_vars
            .contains(&"CRAFTSMAN_ADAPTER_SOURCE".into())
    );
    assert!(
        runtime
            .prepared()
            .shell()
            .secret_vars
            .contains(&"CRAFTSMAN_ADAPTER_SOURCE".into())
    );
    runtime.run_turn("test".into()).await.unwrap();
    let state = runtime.retained_state();
    assert_eq!(result(&state, "shell").0.trim(), "SCRUB-CONFIRMED");
    assert!(result(&state, "ask").1);
    runtime
        .session
        .compaction
        .compact_now(&state.messages)
        .await
        .unwrap();
    assert_eq!(f.requests().await.len(), 4);
    assert!(!f.config.join("threads.db").exists());
    assert!(!f.config.join("sessions-rs").exists());
    assert!(!f.config.join("gadgets/.sdk").exists());
}

struct Recorder {
    name: &'static str,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Tool for Recorder {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name.into(),
            description: "record".into(),
            parameters: json!({"type":"object","properties":{"run_in_background":{"type":"boolean"}}}),
        }
    }
    async fn call(&self, _: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutcome::ok("recorded"))
    }
}
struct Contributed(Arc<AtomicUsize>);
#[async_trait]
impl AgentMiddleware for Contributed {
    fn name(&self) -> &str {
        "contributed"
    }
    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: craftsman_harness::ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst); // A profile-hook side effect must be gated too.
        next.run(call, ctx).await
    }
    fn tools(&self) -> Vec<(ToolDef, Arc<dyn Tool>)> {
        ["write_file", "task", "lsp_diagnostics", "gadget_call"]
            .into_iter()
            .map(|name| {
                let tool: Arc<dyn Tool> = Arc::new(Recorder {
                    name,
                    calls: self.0.clone(),
                });
                (tool.def(), tool)
            })
            .collect()
    }
}
fn gate(selectors: Selectors) -> Arc<InvocationGate> {
    Arc::new(InvocationGate::new(Arc::new(
        InvocationPermissions::from_explicit(selectors, &[]).unwrap(),
    )))
}
async fn direct_call(tool: &Arc<dyn Tool>, args: Value) -> ToolOutcome {
    let mut state = AgentState::default();
    let backend: Arc<dyn Backend> =
        Arc::new(StateFs::new(Arc::new(Mutex::new(AgentState::default()))));
    let (notifications, _rx) = craftsman_harness::notification_channel();
    tool.call(
        args,
        &mut ToolCtx {
            state: &mut state,
            backend,
            registry: Arc::new(AgentRegistry::default()),
            notifications,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn middleware_and_profile_contributed_tools_have_terminal_gates() {
    let gate = gate(Selectors::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let middleware = gate.wrap_middleware(Arc::new(Contributed(calls.clone())));
    // A child profile hook runs before host-supplied middleware. The wrapper
    // must deny before that hook, not only at its contributed tool terminal.
    let chain = craftsman_harness::MiddlewareChain::new(vec![middleware.clone()]);
    let mut state = AgentState::default();
    let (notifications, _rx) = craftsman_harness::notification_channel();
    let mut ctx = ToolCtx {
        state: &mut state,
        backend: Arc::new(StateFs::new(Arc::new(Mutex::new(AgentState::default())))),
        registry: Arc::new(AgentRegistry::default()),
        notifications,
    };
    fn terminal<'a>(
        _: ToolCall,
        _: &'a mut ToolCtx<'_>,
    ) -> craftsman_harness::middleware::BoxedToolFuture<'a> {
        Box::pin(async { panic!("denied call must never reach terminal") })
    }
    let denied = chain
        .around_tool(call("hook", "write_file", json!({})), &mut ctx, &terminal)
        .await
        .unwrap();
    assert!(denied.is_error);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let wrapped = middleware.tools();
    assert_eq!(
        wrapped
            .iter()
            .map(|(def, _)| def.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["write_file", "task"])
    );
    assert!(
        direct_call(&wrapped[0].1, json!({"file_path":"no"}))
            .await
            .is_error
    );
    assert!(
        direct_call(&wrapped[1].1, json!({"run_in_background":true}))
            .await
            .is_error
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let source = HarnessProfile {
        extra_middleware: vec![
            craftsman_harness::NamedMiddlewareFactory::new("contributed", {
                let calls = calls.clone();
                move || Arc::new(Contributed(calls.clone()))
            }),
            craftsman_harness::NamedMiddlewareFactory::new("memory", || {
                panic!("headless must omit before materializing")
            }),
        ],
        ..Default::default()
    };
    let profile = harness_profile(&source, &gate);
    let materialized = profile.materialize_extra_middleware();
    assert_eq!(materialized.len(), 1);
    assert!(
        direct_call(&materialized[0].tools()[0].1, json!({}))
            .await
            .is_error
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut profile = profile;
    profile
        .config
        .excluded_middleware
        .insert("exec_invocation".into());
    assert!(
        craftsman_harness::apply_middleware_profile(
            vec![gate],
            &profile,
            &BTreeSet::from(["exec_invocation"])
        )
        .is_err()
    );
}

struct LatchingProvider {
    gate: Arc<InvocationGate>,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl ModelProvider for LatchingProvider {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let denied = self.gate.wrap_tool(Arc::new(Recorder {
            name: "write_file",
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        direct_call(&denied, json!({})).await;
        Ok(ModelResponse {
            message: tools(vec![call(
                "post-failure",
                "execute",
                json!({"command":sentinel_command()}),
            )]),
            usage: Usage::default(),
        })
    }
}
#[tokio::test]
async fn all_role_provider_guards_check_before_and_after_same_poll_response() {
    let gate = gate(Selectors {
        on_unapproved: OnUnapproved::Fail,
        ..Default::default()
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = gate.wrap_provider(Arc::new(LatchingProvider {
        gate: gate.clone(),
        calls: calls.clone(),
    }));
    let request = || ModelRequest {
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    // A fail discovered while the provider is executing cannot hand tool calls
    // to the loop. All later role requests are refused before provider traffic.
    assert!(provider.complete(request()).await.is_err());
    for _role in ["main", "subagent", "compaction"] {
        assert!(provider.complete(request()).await.is_err());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

struct ReturnedOutcome(ToolOutcome);
#[async_trait]
impl Tool for ReturnedOutcome {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "read_file".into(),
            description: "synthetic typed outcome".into(),
            parameters: json!({"type":"object"}),
        }
    }
    async fn call(&self, _: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        Ok(self.0.clone())
    }
}
fn typed_outcome(is_error: bool, kind: Option<craftsman_harness::ToolErrorKind>) -> ToolOutcome {
    let mut meta = craftsman_harness::ToolMeta::new();
    meta.error_kind = kind;
    (if is_error {
        ToolOutcome::err("untrusted text can say NeedsApproval")
    } else {
        ToolOutcome::ok("data, not a failure")
    })
    .with_meta(meta)
}

#[tokio::test]
async fn typed_inner_needs_approval_seals_but_strings_denials_and_success_metadata_do_not() {
    use craftsman_harness::ToolErrorKind;
    for (is_error, kind, seals) in [
        (true, Some(ToolErrorKind::NeedsApproval), true),
        (true, None, false),
        (true, Some(ToolErrorKind::DeniedCapability), false),
        (false, Some(ToolErrorKind::NeedsApproval), false),
    ] {
        let gate = gate(Selectors {
            on_unapproved: OnUnapproved::Fail,
            ..Default::default()
        });
        let tool = gate.wrap_tool(Arc::new(ReturnedOutcome(typed_outcome(is_error, kind))));
        direct_call(&tool, json!({})).await;
        assert_eq!(gate.approval_required(), seals);
        let count = Arc::new(AtomicUsize::new(0));
        let later = gate.wrap_tool(Arc::new(Recorder {
            name: "read_file",
            calls: count.clone(),
        }));
        direct_call(&later, json!({})).await;
        assert_eq!(count.load(Ordering::SeqCst), usize::from(!seals));
    }
    let gate = gate(Selectors::default());
    let tool = gate.wrap_tool(Arc::new(ReturnedOutcome(typed_outcome(
        true,
        Some(ToolErrorKind::NeedsApproval),
    ))));
    direct_call(&tool, json!({})).await;
    assert!(
        !gate.approval_required(),
        "deny policy permits a later allowed path"
    );
}

#[tokio::test]
async fn approval_notification_wakes_all_roles_even_if_sealed_before_registration() {
    let gate = gate(Selectors {
        on_unapproved: OnUnapproved::Fail,
        ..Default::default()
    });
    let first = tokio::spawn({
        let gate = gate.clone();
        async move { gate.wait_approval_required().await }
    });
    let second = tokio::spawn({
        let gate = gate.clone();
        async move { gate.wait_approval_required().await }
    });
    gate.observe_error_kind(craftsman_harness::ToolErrorKind::NeedsApproval);
    tokio::time::timeout(Duration::from_secs(3), async {
        first.await.unwrap();
        second.await.unwrap();
        gate.wait_approval_required().await
    })
    .await
    .unwrap();
    assert!(gate.approval_required());
}

struct TypedFailureMiddleware;
#[async_trait]
impl AgentMiddleware for TypedFailureMiddleware {
    fn name(&self) -> &str {
        "synthetic_typed_failure"
    }
    async fn around_tool(
        &self,
        _: ToolCall,
        _: &mut ToolCtx<'_>,
        _: craftsman_harness::ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        Ok(typed_outcome(
            true,
            Some(craftsman_harness::ToolErrorKind::NeedsApproval),
        ))
    }
}
#[tokio::test]
async fn middleware_returned_typed_approval_failure_is_observed_before_later_dispatch() {
    let gate = gate(Selectors {
        on_unapproved: OnUnapproved::Fail,
        ..Default::default()
    });
    let chain = craftsman_harness::MiddlewareChain::new(vec![
        gate.wrap_middleware(Arc::new(TypedFailureMiddleware)),
    ]);
    let mut state = AgentState::default();
    let (notifications, _rx) = craftsman_harness::notification_channel();
    let mut ctx = ToolCtx {
        state: &mut state,
        backend: Arc::new(StateFs::new(Arc::new(Mutex::new(AgentState::default())))),
        registry: Arc::new(AgentRegistry::default()),
        notifications,
    };
    fn terminal<'a>(
        _: ToolCall,
        _: &'a mut ToolCtx<'_>,
    ) -> craftsman_harness::middleware::BoxedToolFuture<'a> {
        Box::pin(async { panic!("middleware must not call terminal") })
    }
    let result = chain
        .around_tool(call("typed", "read_file", json!({})), &mut ctx, &terminal)
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(gate.approval_required());
}

#[cfg(feature = "gadgets")]
struct CatchGuest {
    host: Arc<crate::gadgets::headless::GuestHost>,
}
#[cfg(feature = "gadgets")]
#[async_trait]
impl Tool for CatchGuest {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "read_file".into(),
            description: "synthetic caught import".into(),
            parameters: json!({"type":"object"}),
        }
    }
    async fn call(&self, _: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let error = self
            .host
            .call(
                crate::gadgets::headless::SourceKind::User,
                "demo",
                "catch_write_then_read",
                json!({}),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, crate::gadgets::ErrorKind::NeedsApproval);
        // Deliberately hide the typed tool failure: the import's SAME invocation
        // latch must still seal the parent, not believe this successful string.
        Ok(ToolOutcome::ok(
            "caught guest failure, not a completed turn",
        ))
    }
}
#[cfg(feature = "gadgets")]
struct OneResponse {
    calls: Arc<AtomicUsize>,
    message: Msg,
}
#[cfg(feature = "gadgets")]
#[async_trait]
impl ModelProvider for OneResponse {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ModelResponse {
            message: self.message.clone(),
            usage: Usage::default(),
        })
    }
}
#[cfg(feature = "gadgets")]
async fn guest_for_runtime(
    f: &Fixture,
    runtime: &Runtime,
) -> Arc<crate::gadgets::headless::GuestHost> {
    use crate::gadgets::headless::{GuestHost, GuestLimits};
    write(&f.workspace, "safe.txt", "synthetic safe bytes");
    write(
        &f.config,
        "gadgets/demo/gadget.toml",
        "id='demo'\nversion='0.1.0'\n[capabilities]\nread=['workspace']\nwrite=['workspace']\n",
    );
    std::fs::write(
        f.config.join("gadgets/demo/gadget.wasm"),
        crate::gadgets::headless::fixture::fixture(),
    )
    .unwrap();
    Arc::new(
        GuestHost::from_scope(
            runtime.guest_scope(),
            vec!["demo".into()],
            GuestLimits::default(),
        )
        .await
        .unwrap(),
    )
}

#[cfg(feature = "gadgets")]
#[tokio::test]
async fn caught_guest_import_during_main_response_seals_same_response_and_compaction() {
    let f = Fixture::new().await;
    f.configure(json!({"gadgets":{"enabled":true}}));
    let mut runtime = f
        .runtime(Selectors {
            on_unapproved: OnUnapproved::Fail,
            ..Default::default()
        })
        .await;
    let host = guest_for_runtime(&f, &runtime).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let later = Arc::new(AtomicUsize::new(0));
    runtime.session.agent =
        craftsman_harness::Agent::builder(runtime.gate.wrap_provider(Arc::new(OneResponse {
            calls: calls.clone(),
            message: tools(vec![
                call("guest", "read_file", json!({})),
                call("later", "write_todos", json!({})),
            ]),
        })))
        .tool(runtime.gate.wrap_tool(Arc::new(CatchGuest { host })))
        .tool(runtime.gate.wrap_tool(Arc::new(Recorder {
            name: "write_todos",
            calls: later.clone(),
        })))
        .middleware(runtime.gate.clone())
        .backend(Arc::new(runtime.prepared().filesystem().clone()))
        .build();
    assert!(matches!(
        runtime.run_turn("synthetic".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(later.load(Ordering::SeqCst), 0);
    let state = runtime.retained_state();
    assert!(
        state.messages.iter().any(
            |m| matches!(m,Msg::Assistant {content,..} if content=="synthetic partial progress")
        )
    );
    assert!(
        !result(&state, "guest").1,
        "outer tool deliberately caught the error"
    );
    assert!(result(&state, "later").1);
    assert!(!f.workspace.join("unapproved.txt").exists());
    assert!(
        runtime
            .session
            .compaction
            .compact_now(&state.messages)
            .await
            .is_err()
    );
    assert!(f.requests().await.is_empty());
    assert!(matches!(
        runtime.run_turn("cannot resume".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "gadgets")]
struct ForegroundChild(Arc<craftsman_harness::Agent>);
#[cfg(feature = "gadgets")]
#[async_trait]
impl Tool for ForegroundChild {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "task".into(),
            description: "synthetic foreground child".into(),
            parameters: json!({"type":"object"}),
        }
    }
    async fn call(&self, _: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        assert!(self.0.run_turn("child").await.is_err());
        Ok(ToolOutcome::ok("child returned partial failure"))
    }
}
#[cfg(feature = "gadgets")]
#[tokio::test]
async fn caught_foreground_child_guest_import_seals_child_parent_and_all_provider_roles() {
    let f = Fixture::new().await;
    f.configure(json!({"gadgets":{"enabled":true}}));
    let mut runtime = f
        .runtime(Selectors {
            on_unapproved: OnUnapproved::Fail,
            ..Default::default()
        })
        .await;
    let host = guest_for_runtime(&f, &runtime).await;
    let main = Arc::new(AtomicUsize::new(0));
    let child = Arc::new(AtomicUsize::new(0));
    let later = Arc::new(AtomicUsize::new(0));
    let child_agent = Arc::new(
        craftsman_harness::Agent::builder(runtime.gate.wrap_provider(Arc::new(OneResponse {
            calls: child.clone(),
            message: tools(vec![
                call("child-guest", "read_file", json!({})),
                call("child-later", "write_todos", json!({})),
            ]),
        })))
        .tool(runtime.gate.wrap_tool(Arc::new(CatchGuest { host })))
        .tool(runtime.gate.wrap_tool(Arc::new(Recorder {
            name: "write_todos",
            calls: later.clone(),
        })))
        .middleware(runtime.gate.clone())
        .backend(Arc::new(runtime.prepared().filesystem().clone()))
        .build(),
    );
    runtime.session.agent =
        craftsman_harness::Agent::builder(runtime.gate.wrap_provider(Arc::new(OneResponse {
            calls: main.clone(),
            message: tools(vec![
                call("child", "task", json!({})),
                call("parent-later", "write_todos", json!({})),
            ]),
        })))
        .tool(
            runtime
                .gate
                .wrap_tool(Arc::new(ForegroundChild(child_agent))),
        )
        .tool(runtime.gate.wrap_tool(Arc::new(Recorder {
            name: "write_todos",
            calls: later.clone(),
        })))
        .middleware(runtime.gate.clone())
        .backend(Arc::new(runtime.prepared().filesystem().clone()))
        .build();
    assert!(matches!(
        runtime.run_turn("synthetic".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    assert_eq!(main.load(Ordering::SeqCst), 1);
    assert_eq!(child.load(Ordering::SeqCst), 1);
    assert_eq!(later.load(Ordering::SeqCst), 0);
    assert!(result(&runtime.retained_state(), "parent-later").1);
    for _role in ["main", "subagent", "compaction"] {
        let count = Arc::new(AtomicUsize::new(0));
        let provider = runtime.gate.wrap_provider(Arc::new(OneResponse {
            calls: count.clone(),
            message: Msg::assistant("must-not-send"),
        }));
        assert!(
            provider
                .complete(ModelRequest {
                    system: String::new(),
                    messages: vec![],
                    tools: vec![]
                })
                .await
                .is_err()
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
    assert!(f.requests().await.is_empty());
    assert!(!f.workspace.join("unapproved.txt").exists());
}

#[tokio::test]
async fn main_child_and_direct_compaction_use_the_same_captured_literal_auth_not_environment_reresolution()
 {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"${CAPTURED_PROVIDER_SOURCE}","model_kwargs":{"default_headers":{"x-captured-auth":"synthetic-header-value"}}}));
    let credential = "literal-${MUST_NOT_BE_LOOKED_UP}";
    let p = Prepared::prepare_with_environment_for_tests(
        f.options(Selectors::default()),
        std::collections::BTreeMap::from([("CAPTURED_PROVIDER_SOURCE".into(), credential.into())]),
    )
    .await
    .unwrap();
    assert_eq!(
        p.credentials().redactor().text(credential).unwrap(),
        "[REDACTED]"
    );
    assert_eq!(
        p.credentials()
            .redactor()
            .text("synthetic-header-value")
            .unwrap(),
        "[REDACTED]"
    );
    let runtime = Runtime::build(p).await.unwrap();
    f.script(vec![
        tools(vec![call(
            "child",
            "task",
            json!({"subagent_type":"general-purpose","description":"scoped child"}),
        )]),
        Msg::assistant("child answer"),
        Msg::assistant("main answer"),
        Msg::assistant("compaction summary"),
    ])
    .await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    runtime
        .session
        .compaction
        .compact_now(&runtime.retained_state().messages)
        .await
        .unwrap();
    let requests = f.server.received_requests().await.unwrap();
    let requests: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/chat/completions")
        .collect();
    assert_eq!(requests.len(), 4);
    for request in requests {
        assert_eq!(
            request
                .headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            format!("Bearer {credential}")
        );
        assert_eq!(
            request
                .headers
                .get("x-captured-auth")
                .unwrap()
                .to_str()
                .unwrap(),
            "synthetic-header-value"
        );
    }
    assert!(!f.config.join("threads.db").exists());
    assert!(!f.config.join("sessions-rs").exists());
}

fn shell_print(value: &str) -> String {
    if cfg!(windows) {
        format!("[Console]::Out.Write('{value}')")
    } else {
        format!("printf '%s' '{value}'")
    }
}
#[tokio::test]
async fn actual_main_shell_result_is_redacted_before_the_next_model_round() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"shell-runtime-secret","shell":{"output_format":"head_tail"}}));
    f.script(vec![
        tools(vec![call(
            "shell-secret",
            "execute",
            json!({"command":shell_print("shell-runtime-secret")}),
        )]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&["execute"])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    assert_eq!(
        result(&runtime.retained_state(), "shell-secret"),
        ("[REDACTED]", false)
    );
    let requests = f.requests().await;
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["messages"].as_array().unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m["role"] == "tool" && m["content"] == "[REDACTED]")
    );
    assert!(!messages.iter().filter(|m| m["role"] == "tool").any(|m| {
        m["content"]
            .as_str()
            .unwrap_or_default()
            .contains("shell-runtime-secret")
    }));
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}
#[tokio::test]
async fn foreground_child_uses_the_same_scoped_redacting_shell_as_the_main_builder() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"child-shell-secret"}));
    f.script(vec![
        tools(vec![call(
            "child-task",
            "task",
            json!({"description":"run scoped shell","subagent_type":"general-purpose"}),
        )]),
        tools(vec![call(
            "child-shell",
            "execute",
            json!({"command":shell_print("child-shell-secret")}),
        )]),
        Msg::assistant("child done"),
        Msg::assistant("parent done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&["execute"])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let requests = f.requests().await;
    assert_eq!(requests.len(), 4);
    assert!(
        requests
            .iter()
            .flat_map(|r| r["messages"].as_array().unwrap())
            .any(|m| m["role"] == "tool"
                && m["tool_call_id"] == "child-shell"
                && m["content"] == "[REDACTED]")
    );
    assert!(
        !requests
            .iter()
            .flat_map(|r| r["messages"].as_array().unwrap())
            .filter(|m| m["role"] == "tool")
            .any(|m| m["content"]
                .as_str()
                .unwrap_or_default()
                .contains("child-shell-secret"))
    );
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn invalid_shell_command_args_use_scoped_redaction_before_the_next_model_round() {
    // Use a non-schema-colliding seed so this still reaches the real invalid
    // execute path. §24 separately refuses a seed equal to the command key;
    // direct actual-builder shell fixtures retain the pre-formatter proof.
    for args in [
        json!({"command":""}),
        json!({}),
        json!({"command":null}),
        json!({"command":42}),
    ] {
        let f = Fixture::new().await;
        f.configure(json!({"api_key":"invalid-command-runtime-secret"}));
        f.script(vec![
            tools(vec![call("invalid-shell", "execute", args)]),
            Msg::assistant("done"),
        ])
        .await;
        let runtime = f.runtime(allowed(&["execute"])).await;
        runtime.run_turn("synthetic".into()).await.unwrap();
        assert_eq!(
            result(&runtime.retained_state(), "invalid-shell"),
            ("[REDACTED]", true)
        );
        let requests = f.requests().await;
        assert_eq!(requests.len(), 2);
        let tool_result = requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == "invalid-shell")
            .unwrap();
        assert_eq!(tool_result["content"], "[REDACTED]");
        assert!(!tool_result["content"].as_str().unwrap().contains("command"));
        assert_eq!(std::fs::read_dir(&f.workspace).unwrap().count(), 0);
        assert!(!f.config.join("threads.db").exists());
        assert!(!f.config.join("sessions-rs").exists());
    }
}

#[tokio::test]
async fn all_actual_main_filesystem_tool_results_are_redacted_before_the_next_model_round() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"fs-runtime-secret"}));
    write(
        &f.workspace,
        "fs-runtime-secret.txt",
        "before fs-runtime-secret after",
    );
    f.script(vec![tools(vec![
        call("fs-read","read_file",json!({"file_path":"fs-runtime-secret.txt"})),
        call("fs-list","ls",json!({"path":"."})),
        call("fs-glob","glob",json!({"pattern":"fs-runtime-secret*"})),
        call("fs-grep","grep",json!({"pattern":"fs-runtime-secret","path":"fs-runtime-secret.txt"})),
        call("fs-write","write_file",json!({"file_path":"fs-runtime-secret-new.txt","content":"fs-runtime-secret"})),
        call("fs-edit","edit_file",json!({"file_path":"fs-runtime-secret-new.txt","old_string":"fs-runtime-secret","new_string":"original changed fs-runtime-secret"})),
    ]), Msg::assistant("done")]).await;
    let runtime = f.runtime(allowed(&["write_file", "edit_file"])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let requests = f.requests().await;
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["messages"].as_array().unwrap();
    for id in [
        "fs-read", "fs-list", "fs-glob", "fs-grep", "fs-write", "fs-edit",
    ] {
        let message = messages
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
            .unwrap();
        let text = message["content"].as_str().unwrap();
        assert!(
            text.contains("[REDACTED]")
                && !text.contains("fs-runtime-secret")
                && text.len() <= 100_000,
            "{id}: {text}"
        );
        assert!(!result(&runtime.retained_state(), id).1);
    }
    assert_eq!(
        std::fs::read(f.workspace.join("fs-runtime-secret-new.txt")).unwrap(),
        b"original changed fs-runtime-secret"
    );
    assert!(!f.workspace.join("[REDACTED]-new.txt").exists());
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn actual_foreground_child_filesystem_results_use_the_same_captured_views() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"child-fs-secret"}));
    write(&f.workspace, "child-fs-secret.txt", "child-fs-secret");
    f.script(vec![
        tools(vec![call(
            "parent-task",
            "task",
            json!({"description":"inspect original files","subagent_type":"general-purpose"}),
        )]),
        tools(vec![
            call(
                "child-fs-read",
                "read_file",
                json!({"file_path":"child-fs-secret.txt"}),
            ),
            call(
                "child-fs-grep",
                "grep",
                json!({"pattern":"child-fs-secret"}),
            ),
            call("child-fs-list", "ls", json!({"path":"."})),
        ]),
        Msg::assistant("child done"),
        Msg::assistant("parent done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&[])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let requests = f.requests().await;
    assert_eq!(requests.len(), 4);
    let messages = requests[2]["messages"].as_array().unwrap();
    for id in ["child-fs-read", "child-fs-grep", "child-fs-list"] {
        let text = messages
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
            .unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(text.contains("[REDACTED]") && !text.contains("child-fs-secret"));
    }
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn captured_gate_fixed_denials_are_redacted_before_the_scoped_terminal_is_entered() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"denied"}));
    let mut selectors = allowed(&[]);
    selectors.deny.push("tool:read_file".into());
    f.script(vec![
        tools(vec![call(
            "fixed-denial",
            "read_file",
            json!({"file_path":"missing"}),
        )]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = f.runtime(selectors).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let state = runtime.retained_state();
    let (text, error) = result(&state, "fixed-denial");
    assert!(error && !text.contains("denied") && text.contains("[REDACTED]"));
    assert!(!runtime.gate.approval_required());
    assert_eq!(f.requests().await.len(), 2);
}

#[tokio::test]
async fn actual_typed_protected_filesystem_failure_seals_later_same_response_dispatch() {
    let f = Fixture::new().await;
    write(
        &f.workspace,
        "data.txt",
        "NeedsApproval is just source text",
    );
    let mut selectors = allowed(&["write_file"]);
    selectors.on_unapproved = OnUnapproved::Fail;
    f.script(vec![tools(vec![
        call(
            "ordinary-data",
            "read_file",
            json!({"file_path":"data.txt"}),
        ),
        call(
            "protected-write",
            "write_file",
            json!({"file_path":"build.rs","content":"must not run"}),
        ),
        call(
            "later-fs",
            "write_file",
            json!({"file_path":"later/file.txt","content":"no"}),
        ),
    ])])
    .await;
    let runtime = f.runtime(selectors).await;
    assert!(matches!(
        runtime.run_turn("synthetic".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    let state = runtime.retained_state();
    assert!(!result(&state, "ordinary-data").1);
    assert!(
        result(&state, "ordinary-data")
            .0
            .contains("NeedsApproval is just source text")
    );
    assert!(result(&state, "protected-write").1 && result(&state, "later-fs").1);
    assert!(!f.workspace.join("build.rs").exists() && !f.workspace.join("later").exists());
    assert_eq!(f.requests().await.len(), 1);
}

#[tokio::test]
async fn actual_main_fetch_and_search_results_are_redacted_before_next_model_round() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"web-runtime-secret","search_api_key":"search-runtime-secret","_test_web_search_endpoint":format!("{}/search",f.server.uri())}));
    Mock::given(method("GET"))
        .and(path("/web-runtime-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            b"before web<span>-runtime</span>-s&#101;cret after".as_slice(),
            "text/html",
        ))
        .mount(&f.server)
        .await;
    Mock::given(method("GET")).and(path("/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"web":{"results":[{"title":"search-runtime-secret","url":"https://example.com/web-runtime-secret","description":"search-runtime-secret description"}]}}))).mount(&f.server).await;
    f.script(vec![
        tools(vec![
            call(
                "main-web-fetch",
                "web_fetch",
                json!({"url":format!("{}/web-runtime-secret",f.server.uri())}),
            ),
            call(
                "main-web-search",
                "web_search",
                json!({"query":"web-runtime-secret query"}),
            ),
        ]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&["web_fetch", "web_search"])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let requests = f.requests().await;
    assert_eq!(requests.len(), 2);
    for id in ["main-web-fetch", "main-web-search"] {
        let message = requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
            .unwrap();
        let text = message["content"].as_str().unwrap();
        assert!(
            text.contains("[REDACTED]")
                && !text.contains("web-runtime-secret")
                && !text.contains("search-runtime-secret")
        );
        assert!(!result(&runtime.retained_state(), id).1);
    }
    let network = f.server.received_requests().await.unwrap();
    let fetch = network
        .iter()
        .find(|r| r.url.path() == "/web-runtime-secret")
        .unwrap();
    assert!(!fetch.headers.contains_key("authorization"));
    let search = network.iter().find(|r| r.url.path() == "/search").unwrap();
    assert_eq!(
        search
            .headers
            .get("X-Subscription-Token")
            .unwrap()
            .to_str()
            .unwrap(),
        "search-runtime-secret"
    );
    assert_eq!(
        search.url.query_pairs().find(|(k, _)| k == "q").unwrap().1,
        "web-runtime-secret query"
    );
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn actual_foreground_child_web_results_share_scoped_auth_and_redacted_views() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"child-web-secret","search_api_key":"child-search-secret","_test_web_search_endpoint":format!("{}/search",f.server.uri())}));
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string("child-web-secret"))
        .mount(&f.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"web":{"results":[{"title":"child-search-secret"}]}})),
        )
        .mount(&f.server)
        .await;
    f.script(vec![
        tools(vec![call(
            "web-task",
            "task",
            json!({"subagent_type":"general-purpose","description":"fetch original sources"}),
        )]),
        tools(vec![
            call(
                "child-web-fetch",
                "web_fetch",
                json!({"url":format!("{}/page",f.server.uri())}),
            ),
            call(
                "child-web-search",
                "web_search",
                json!({"query":"child-web-secret"}),
            ),
        ]),
        Msg::assistant("child done"),
        Msg::assistant("parent done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&["web_fetch", "web_search"])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let requests = f.requests().await;
    assert_eq!(requests.len(), 4);
    for id in ["child-web-fetch", "child-web-search"] {
        let message = requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
            .unwrap();
        let text = message["content"].as_str().unwrap();
        assert!(
            text.contains("[REDACTED]")
                && !text.contains("child-web-secret")
                && !text.contains("child-search-secret")
        );
    }
    let network = f.server.received_requests().await.unwrap();
    let search = network.iter().find(|r| r.url.path() == "/search").unwrap();
    assert_eq!(
        search
            .headers
            .get("X-Subscription-Token")
            .unwrap()
            .to_str()
            .unwrap(),
        "child-search-secret"
    );
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn actual_framework_todos_keep_original_typed_state_and_safe_next_round_views() {
    let f = Fixture::new().await;
    let escaped = "quoted\"\\\n秘密-runtime";
    f.configure(json!({"api_key":"todos-runtime-secret", "search_api_key":escaped}));
    let original = format!("todos-runtime-secret {escaped} step");
    f.script(vec![
        tools(vec![call(
            "runtime-todos",
            "write_todos",
            json!({"todos":[{"content":original,"status":"pending"}]}),
        )]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&["write_todos"])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let state = runtime.retained_state();
    assert_eq!(state.todos[0].content, original);
    assert_eq!(
        state.todos[0].status,
        craftsman_harness::state::TodoStatus::Pending
    );
    let text = result(&state, "runtime-todos").0;
    let decoded: Value =
        serde_json::from_str(text.strip_prefix("Updated todo list to ").unwrap()).unwrap();
    assert_eq!(
        decoded,
        json!([{"content":"[REDACTED] [REDACTED] step", "status":"pending"}])
    );
    let requests = f.requests().await;
    assert_eq!(requests.len(), 2);
    let todo_context = system(&requests[1])
        .split("Current todo list:")
        .nth(1)
        .unwrap();
    assert!(
        todo_context.contains("[REDACTED] [REDACTED] step")
            && !todo_context.contains("todos-runtime-secret")
            && !todo_context.contains("秘密-runtime")
    );
    // §24 selects copied request views, not ORIGINAL state/dispatch args.
    assert!(
        !requests[1]["messages"]
            .to_string()
            .contains("todos-runtime-secret")
    );
    assert!(state.messages.iter().any(|m| matches!(m, Msg::Assistant {tool_calls,..} if tool_calls.iter().any(|c| c.args.to_string().contains("todos-runtime-secret")))));
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}
#[tokio::test]
async fn actual_framework_foreground_task_text_is_redacted_before_framework_budget_cut() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"credential-runtime-value"}));
    let original = format!("{}credential-runtime-value tail", "q".repeat(98_971));
    f.script(vec![tools(vec![call("runtime-task", "task", json!({"subagent_type":"general-purpose", "description":"ORIGINAL credential-runtime-value task"}))]), Msg::assistant(&original), Msg::assistant("parent done")]).await;
    let runtime = f.runtime(allowed(&["task"])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let state = runtime.retained_state();
    let (text, failed) = result(&state, "runtime-task");
    assert!(!failed && text.len() <= 100_000 && text.contains("[output truncated"));
    assert!(!text.contains("cred") && !text.contains("tail"));
    assert_eq!(
        runtime.session.registry.status("general-purpose#0"),
        Some(craftsman_harness::AgentStatus::Done)
    );
    let requests = f.requests().await;
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "user" && m["content"] == "ORIGINAL [REDACTED] task")
    );
    let tool = requests[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == "runtime-task")
        .unwrap();
    assert_eq!(tool["content"], text);
    assert!(
        !tool["content"]
            .as_str()
            .unwrap()
            .contains("credential-runtime-value")
    );
    assert!(state.messages.iter().any(|m| matches!(m, Msg::Assistant {tool_calls,..} if tool_calls.iter().any(|c| c.args["description"] == "ORIGINAL credential-runtime-value task"))));
    let network = f.server.received_requests().await.unwrap();
    assert_eq!(network.len(), 3);
    assert!(network.iter().all(
        |r| r.headers.get("authorization").unwrap().to_str().unwrap()
            == "Bearer credential-runtime-value"
    ));
}
#[tokio::test]
async fn actual_framework_message_ack_is_safe_but_original_notification_payload_stays_private() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"message-runtime-secret"}));
    f.script(vec![
        tools(vec![call(
            "runtime-message",
            "send_message",
            json!({"to":"main", "message":"ORIGINAL message-runtime-secret payload"}),
        )]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&["send_message"])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let state = runtime.retained_state();
    let (text, failed) = result(&state, "runtime-message");
    assert!(!failed && text == "message sent to main");
    let requests = f.requests().await;
    assert_eq!(requests.len(), 2);
    // ORIGINAL queue/injection state stays private; only its outgoing view is selected.
    assert!(state.messages.iter().any(|m| matches!(m, Msg::User {content} if content.contains("ORIGINAL message-runtime-secret payload"))));
    assert!(requests[1]["messages"].as_array().unwrap().iter().any(|m| {
        m["role"] == "user"
            && m["content"]
                .as_str()
                .is_some_and(|s| s.contains("ORIGINAL [REDACTED] payload"))
    }));
    assert!(
        !requests[1]["messages"]
            .to_string()
            .contains("message-runtime-secret")
    );
    assert!(runtime.session.registry.list().is_empty());
}
#[tokio::test]
async fn actual_framework_child_typed_failure_seals_task_publication_and_later_same_response_tools()
{
    let f = Fixture::new().await;
    f.script(vec![
        tools(vec![
            call(
                "stopped-task",
                "task",
                json!({"subagent_type":"general-purpose", "description":"original task"}),
            ),
            call(
                "later-parent",
                "write_file",
                json!({"file_path":"later-parent", "content":"must not happen"}),
            ),
            call(
                "later-message",
                "send_message",
                json!({"to":"main", "message":"must not happen"}),
            ),
        ]),
        tools(vec![
            call(
                "protected-child",
                "write_file",
                json!({"file_path":"build.rs", "content":"must not happen"}),
            ),
            call(
                "later-child",
                "write_file",
                json!({"file_path":"later-child", "content":"must not happen"}),
            ),
        ]),
    ])
    .await;
    let mut selectors = allowed(&["task", "write_file", "send_message"]);
    selectors.on_unapproved = OnUnapproved::Fail;
    let runtime = f.runtime(selectors).await;
    assert!(matches!(
        runtime.run_turn("synthetic".into()).await,
        Err(RuntimeError::ApprovalRequired)
    ));
    let state = runtime.retained_state();
    let (text, failed) = result(&state, "stopped-task");
    assert!(failed);
    assert_eq!(text, "[REDACTED]");
    assert_eq!(
        runtime.session.registry.status("general-purpose#0"),
        Some(craftsman_harness::AgentStatus::Failed)
    );
    assert_eq!(f.requests().await.len(), 2);
    assert!(
        !f.workspace.join("build.rs").exists()
            && !f.workspace.join("later-parent").exists()
            && !f.workspace.join("later-child").exists()
    );
}
#[tokio::test]
async fn actual_framework_invalid_todos_return_fixed_error_before_next_model_without_raw_validation()
 {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"validation-runtime-secret"}));
    f.script(vec![
        tools(vec![call(
            "invalid-todos",
            "write_todos",
            json!({"todos":[{"content":"step", "status":"validation-runtime-secret"}]}),
        )]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&["write_todos"])).await;
    runtime.run_turn("synthetic".into()).await.unwrap();
    let state = runtime.retained_state();
    assert!(state.todos.is_empty());
    let (text, failed) = result(&state, "invalid-todos");
    assert!(failed);
    assert_eq!(text, "[REDACTED]");
    assert!(!runtime.gate.approval_required());
    assert_eq!(f.requests().await.len(), 2);
}
#[tokio::test]
async fn actual_provider_request_status_schema_collision_refuses_before_traffic_and_state_mutation()
{
    let f = Fixture::new().await;
    f.configure(json!({"search_api_key":"pending"}));
    f.script(vec![
        tools(vec![call(
            "status-todos",
            "write_todos",
            json!({"todos":[{"content":"step", "status":"pending"}]}),
        )]),
        Msg::assistant("done"),
    ])
    .await;
    let runtime = f.runtime(allowed(&["write_todos"])).await;
    // The request schema itself contains pending as an authoritative enum.
    // Refuse BEFORE admitting a model call; direct §23 producer fixtures still
    // prove original assignment is not rolled back after a view-only failure.
    assert!(matches!(
        runtime.run_turn("synthetic".into()).await,
        Err(RuntimeError::Turn(AgentError::Model(_)))
    ));
    assert!(runtime.retained_state().todos.is_empty());
    assert!(!runtime.gate.approval_required());
    assert!(f.requests().await.is_empty());
    assert_eq!(*runtime.session.usage.lock().unwrap(), (0, 0));
}

#[tokio::test]
async fn actual_provider_request_command_identity_collision_never_admits_model_or_shell_effects() {
    let f = Fixture::new().await;
    f.configure(json!({"api_key":"command"}));
    f.script(vec![tools(vec![call(
        "must-not-dispatch",
        "execute",
        json!({"command":"must not run"}),
    )])])
    .await;
    let runtime = f.runtime(allowed(&["execute"])).await;
    assert!(matches!(
        runtime.run_turn("synthetic".into()).await,
        Err(RuntimeError::Turn(AgentError::Model(_)))
    ));
    assert!(f.requests().await.is_empty());
    assert_eq!(*runtime.session.usage.lock().unwrap(), (0, 0));
    assert!(
        runtime
            .retained_state()
            .messages
            .iter()
            .all(|m| !matches!(m, Msg::Assistant { .. } | Msg::Tool { .. }))
    );
    assert_eq!(std::fs::read_dir(&f.workspace).unwrap().count(), 0);
}
#[tokio::test]
async fn actual_provider_request_views_keep_original_tool_dispatch_files_and_response_state() {
    let f = Fixture::new().await;
    let seed = "provider-runtime-secret";
    f.configure(json!({"api_key":seed}));
    let file = format!("{seed}-original.txt");
    let content = format!("ORIGINAL {seed} file");
    f.script(vec![
        tools(vec![call(
            "provider-write",
            "write_file",
            json!({"file_path":file,"content":content}),
        )]),
        Msg::assistant(format!("ORIGINAL {seed} response")),
    ])
    .await;
    let runtime = f.runtime(allowed(&["write_file"])).await;
    runtime
        .run_turn(format!("ORIGINAL {seed} user"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.workspace.join(&file)).unwrap(),
        content
    );
    assert!(!f.workspace.join("[REDACTED]-original.txt").exists());
    let state = runtime.retained_state();
    assert!(state.messages.iter().any(|m| matches!(m,Msg::Assistant {tool_calls,..} if tool_calls.iter().any(|c| c.args["file_path"]==file && c.args["content"]==content))));
    assert!(
        state
            .messages
            .iter()
            .any(|m| matches!(m,Msg::Assistant {content,..} if content.contains(seed)))
    ); // response is original control/state, not claimed published
    let requests = f.requests().await;
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| !r.to_string().contains(seed)));
    let call = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "assistant")
        .flat_map(|m| m["tool_calls"].as_array().unwrap())
        .find(|c| c["id"] == "provider-write")
        .unwrap();
    let args: Value =
        serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(args["file_path"], "[REDACTED]-original.txt");
    assert_eq!(args["content"], "ORIGINAL [REDACTED] file");
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}
#[tokio::test]
async fn actual_provider_main_child_and_direct_compaction_send_checked_views_with_original_auth() {
    let f = Fixture::new().await;
    let seed = "all-role-request-secret";
    f.configure(json!({"api_key":seed,"search_api_key":"request-search-secret","model_kwargs":{"metadata":{"context":"request-search-secret"},"default_headers":{"x-source-auth":"request-header-secret"}}}));
    write(
        &f.config,
        "agents/craftsman/AGENTS.md",
        &format!("ORIGINAL {seed} instruction"),
    );
    f.script(vec![tools(vec![call("request-task","task",json!({"subagent_type":"general-purpose","description":format!("ORIGINAL {seed} task")}))]),Msg::assistant(format!("ORIGINAL {seed} child")),Msg::assistant(format!("ORIGINAL {seed} parent")),Msg::assistant(format!("ORIGINAL {seed} summary"))]).await;
    let runtime = f.runtime(allowed(&["task"])).await;
    runtime
        .run_turn(format!("ORIGINAL {seed} user"))
        .await
        .unwrap();
    let original = runtime.retained_state();
    assert!(
        original
            .messages
            .iter()
            .any(|m| matches!(m,Msg::User {content} if content.contains(seed)))
    );
    assert!(
        original
            .messages
            .iter()
            .any(|m| matches!(m,Msg::Assistant {content,..} if content.contains(seed)))
    );
    let summary = runtime
        .session
        .compaction
        .compact_now(&original.messages)
        .await
        .unwrap();
    assert!(summary.unwrap().iter().any(|m| matches!(m, Msg::System {content} | Msg::User {content} | Msg::Assistant {content,..} | Msg::Tool {content,..} if content.contains(seed)))); // ORIGINAL private response/state; public/storage adapter remains open
    let requests = f.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    for request in requests {
        let text = std::str::from_utf8(&request.body).unwrap();
        assert!(
            !text.contains(seed)
                && !text.contains("request-search-secret")
                && !text.contains("request-header-secret")
        );
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["metadata"]["context"], "[REDACTED]");
        assert_eq!(
            request
                .headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            format!("Bearer {seed}")
        );
        assert_eq!(
            request
                .headers
                .get("x-source-auth")
                .unwrap()
                .to_str()
                .unwrap(),
            "request-header-secret"
        );
    }
    assert!(!f.config.join("threads.db").exists());
}
#[tokio::test]
async fn actual_provider_request_refusal_does_not_invent_usage_receipt_or_retry() {
    let f = Fixture::new().await;
    f.configure(json!({"model_kwargs":{"temperature":42},"search_api_key":"42"}));
    f.script(vec![Msg::assistant("unused")]).await;
    let runtime = f.runtime(Selectors::default()).await;
    assert!(matches!(
        runtime.run_turn("original".into()).await,
        Err(RuntimeError::Turn(AgentError::Model(_)))
    ));
    assert!(f.requests().await.is_empty());
    assert_eq!(*runtime.session.usage.lock().unwrap(), (0, 0));
    assert!(runtime.retained_state().last_usage.is_none());
}

#[tokio::test]
async fn actual_provider_http_diagnostic_main_failure_keeps_no_fake_usage_or_raw_store() {
    let f = Fixture::new().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_raw(
            b"synthetic-provider-key ORIGINAL diagnostic".to_vec(),
            "text/plain",
        ))
        .mount(&f.server)
        .await;
    let runtime = f.runtime(Selectors::default()).await;
    let error = runtime.run_turn("ORIGINAL input".into()).await.unwrap_err();
    assert!(matches!(error,RuntimeError::Turn(AgentError::Model(ref text)) if text=="[REDACTED]"));
    assert_eq!(f.requests().await.len(), 1);
    assert_eq!(*runtime.session.usage.lock().unwrap(), (0, 0));
    assert!(runtime.retained_state().last_usage.is_none());
    assert!(runtime.session.registry.list().is_empty());
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn actual_provider_http_diagnostic_foreground_child_failure_preserves_real_main_usage() {
    let f = Fixture::new().await;
    let seed = "provider-child-error-key";
    f.configure(json!({"api_key":seed}));
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let script = Script(Mutex::new(
        vec![tools(vec![call(
            "http-error-task",
            "task",
            json!({"subagent_type":"general-purpose","description":"ORIGINAL child job"}),
        )])]
        .into(),
    ));
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                script.respond(request)
            } else {
                ResponseTemplate::new(400).set_body_raw(seed.as_bytes().to_vec(), "text/plain")
            }
        })
        .mount(&f.server)
        .await;
    let runtime = f.runtime(allowed(&["task"])).await;
    let error = runtime
        .run_turn("ORIGINAL parent".into())
        .await
        .unwrap_err();
    assert!(matches!(error,RuntimeError::Turn(AgentError::Model(ref text)) if text=="[REDACTED]"));
    assert_eq!(count.load(Ordering::SeqCst), 3); // genuine main, child error, parent error
    let state = runtime.retained_state();
    let (text, is_error) = result(&state, "http-error-task");
    assert!(is_error && !text.contains(seed));
    assert_eq!(*runtime.session.usage.lock().unwrap(), (20, 5)); // ORIGINAL Script usage, not error/view estimates
    assert_eq!(
        state.last_usage,
        Some(Usage {
            input_tokens: 20,
            output_tokens: 5
        })
    );
    let children = runtime.session.registry.list();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].1, craftsman_harness::AgentStatus::Failed);
    assert!(!f.config.join("threads.db").exists());
    for request in f.server.received_requests().await.unwrap() {
        assert_eq!(
            request
                .headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            format!("Bearer {seed}")
        );
    }
}

#[tokio::test]
async fn actual_provider_http_diagnostic_direct_compaction_failure_is_not_completion_or_usage() {
    let f = Fixture::new().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_raw(
            b"synthetic-provider-key compaction diagnostic".to_vec(),
            "text/plain",
        ))
        .mount(&f.server)
        .await;
    let runtime = f.runtime(Selectors::default()).await;
    let messages = vec![
        Msg::user("ORIGINAL retained input"),
        Msg::assistant("ORIGINAL retained answer"),
    ];
    let error = runtime
        .session
        .compaction
        .compact_now(&messages)
        .await
        .unwrap_err();
    assert!(matches!(error,AgentError::Model(ref text) if text=="[REDACTED]"));
    assert_eq!(f.requests().await.len(), 1);
    assert_eq!(*runtime.session.usage.lock().unwrap(), (0, 0));
    assert!(matches!(&messages[0],Msg::User {content} if content=="ORIGINAL retained input"));
    assert!(
        matches!(&messages[1],Msg::Assistant {content,..} if content=="ORIGINAL retained answer")
    );
    assert!(!f.config.join("threads.db").exists());
}
