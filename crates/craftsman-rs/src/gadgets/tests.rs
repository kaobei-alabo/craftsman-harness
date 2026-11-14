use super::grants::GrantStore;
use super::manifest::{Capabilities, Manifest, valid_id};
use super::*;
use crate::agent::ApprovalPromptResult;
use crate::config::PermissionMode;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[path = "../../tests/common/gadget_fixture.rs"]
mod gadget_fixture;
use gadget_fixture::fixtures;

struct TestHome {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    gadget: PathBuf,
}
impl TestHome {
    fn new(capabilities: Capabilities) -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("home/.craftsman");
        let gadget = config.join("gadgets/demo");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("Cargo.toml"), "must not be visible via WASI").unwrap();
        std::fs::create_dir_all(&gadget).unwrap();
        let home = Self {
            _root: root,
            workspace,
            config,
            gadget,
        };
        home.manifest(&home.gadget, capabilities);
        std::fs::write(home.gadget.join("gadget.wasm"), &fixtures().0).unwrap();
        home
    }
    fn manifest(&self, path: &Path, capabilities: Capabilities) {
        std::fs::create_dir_all(path).unwrap();
        let manifest = Manifest {
            id: "demo".into(),
            version: "0.1.0".into(),
            description: "A fixture gadget.".into(),
            capabilities,
            prompt: None,
            commands: vec![],
            agents: vec![],
            migration: None,
        };
        std::fs::write(
            path.join("gadget.toml"),
            toml::to_string(&manifest).unwrap(),
        )
        .unwrap();
    }
    fn options(&self) -> RuntimeOptions {
        RuntimeOptions {
            allowed_gadgets: BTreeSet::from(["demo".into()]),
            disk_cache: false,
            ..RuntimeOptions::default()
        }
    }
    fn runtime(&self, options: RuntimeOptions) -> Arc<GadgetRuntime> {
        Arc::new(GadgetRuntime::new(&self.workspace, &self.config, options).unwrap())
    }
}
fn capabilities(read: bool, write: bool, http: &[&str]) -> Capabilities {
    Capabilities {
        read: if read {
            BTreeSet::from(["workspace".into()])
        } else {
            BTreeSet::new()
        },
        write: if write {
            BTreeSet::from(["workspace".into()])
        } else {
            BTreeSet::new()
        },
        http: http.iter().map(|h| h.to_string()).collect(),
    }
}

#[tokio::test]
async fn source_built_sdk_exports_and_dispatches_without_an_export_macro() {
    let home = TestHome::new(Capabilities::default());
    let runtime = home.runtime(home.options());
    let list = runtime.list().await.unwrap();
    assert_eq!(list[0].status, "ready");
    assert!(
        list[0]
            .functions
            .iter()
            .any(|f| f.name == "echo" && f.input_schema["properties"]["text"]["type"] == "string")
    );
    let result = runtime
        .call("demo", "echo", json!({"text":"hello"}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.json).unwrap(),
        json!({"text":"hello", "version":"0.1.0"})
    );
    assert_eq!(
        runtime
            .call("demo", "missing", json!({}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::NotFound
    );
    assert_eq!(
        runtime
            .call("demo", "echo", json!({"text": 3}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgs
    );
}

#[tokio::test]
async fn unapproved_gadgets_never_execute_even_describe() {
    let home = TestHome::new(Capabilities::default());
    // If list executed or compiled this component, it would report a trap.
    std::fs::write(home.gadget.join("gadget.wasm"), b"not a component").unwrap();
    let mut options = home.options();
    options.allowed_gadgets.clear();
    let runtime = home.runtime(options);
    let list = runtime.list().await.unwrap();
    assert_eq!(list[0].status, "needs-approval");
    assert!(list[0].functions.is_empty());
    assert_eq!(
        runtime
            .call("demo", "ping", json!({}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::NeedsApproval
    );
    assert!(runtime.compiled_count() == 0);
}

#[tokio::test]
async fn approval_is_source_bound_and_only_wider_capabilities_ask_again() {
    let home = TestHome::new(Capabilities::default());
    let prompts = Arc::new(AtomicUsize::new(0));
    let probe = prompts.clone();
    let mut options = home.options();
    options.allowed_gadgets.clear();
    options.interactive = true;
    options.prompt = Some(Arc::new(move |tool, _, details| {
        assert_eq!(tool, "gadget_call");
        assert!(details.contains("SHA-256"));
        probe.fetch_add(1, Ordering::Relaxed);
        ApprovalPromptResult::ApproveAlways
    }));
    let runtime = home.runtime(options.clone());
    assert_eq!(runtime.list().await.unwrap()[0].status, "needs-approval");
    assert_eq!(prompts.load(Ordering::Relaxed), 0);
    runtime.call("demo", "ping", json!({})).await.unwrap();
    assert_eq!(prompts.load(Ordering::Relaxed), 1);
    assert!(
        !options.approval.lock().unwrap().auto_approve,
        "first-load approval must not become global bypass"
    );
    std::fs::write(home.gadget.join("gadget.wasm"), &fixtures().1).unwrap();
    let result = runtime
        .call("demo", "echo", json!({"text":"rebuilt"}))
        .await
        .unwrap();
    assert!(result.json.contains("0.2.0"));
    assert_eq!(prompts.load(Ordering::Relaxed), 1);
    home.manifest(&home.gadget, capabilities(true, false, &[]));
    assert_eq!(runtime.list().await.unwrap()[0].status, "needs-approval");
    runtime.call("demo", "ping", json!({})).await.unwrap();
    assert_eq!(prompts.load(Ordering::Relaxed), 2);
    home.manifest(&home.gadget, Capabilities::default());
    runtime.call("demo", "ping", json!({})).await.unwrap();
    assert_eq!(prompts.load(Ordering::Relaxed), 2);
    let project = home.workspace.join(".craftsman/gadgets/demo");
    home.manifest(&project, Capabilities::default());
    std::fs::write(project.join("gadget.wasm"), &fixtures().0).unwrap();
    runtime.call("demo", "ping", json!({})).await.unwrap();
    assert_eq!(
        prompts.load(Ordering::Relaxed),
        3,
        "project shadow must not inherit user gadget's grant"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(home.config.join("gadgets/.grants.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn project_headless_runs_need_both_explicit_allow_and_workspace_trust() {
    let home = TestHome::new(Capabilities::default());
    let project = home.workspace.join(".craftsman/gadgets/demo");
    home.manifest(&project, Capabilities::default());
    std::fs::write(project.join("gadget.wasm"), &fixtures().0).unwrap();
    let runtime = home.runtime(home.options());
    assert_eq!(
        runtime
            .call("demo", "ping", json!({}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::NeedsApproval
    );
    let mut options = home.options();
    options.trust_workspace = true;
    home.runtime(options)
        .call("demo", "ping", json!({}))
        .await
        .unwrap();
}

#[tokio::test]
async fn escape_suite_labels_each_denial_mechanism() {
    let home = TestHome::new(capabilities(true, true, &["example.com"]));
    std::fs::write(home.workspace.join("readme.txt"), "inside").unwrap();
    let options = home.options();
    options.approval.lock().unwrap().mode = PermissionMode::AcceptEdits;
    let runtime = home.runtime(options);
    assert!(
        runtime
            .call("demo", "read", json!({"path":"readme.txt"}))
            .await
            .unwrap()
            .json
            .contains("inside")
    );
    assert!(
        runtime
            .call("demo", "list", json!({"path":"."}))
            .await
            .is_ok()
    );
    for path in ["../outside", home.config.to_str().unwrap()] {
        let error = runtime
            .call("demo", "read", json!({"path":path}))
            .await
            .unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::DeniedCapability,
            "workspace-confinement: {path}: {error}"
        );
    }
    for path in [
        ".git/config",
        ".craftsman/gadgets/.grants.json",
        ".github/workflows/ci.yml",
        ".env",
        ".env.local",
        ".GIT/new",
        ".gitlab-ci.yml",
        ".git./config",
        ".git /config",
        "NUL",
        "CON.txt",
    ] {
        let error = runtime
            .call(
                "demo",
                "write",
                json!({"path":path,"text":"must not write"}),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::DeniedCapability,
            "protected-path: {path}: {error}"
        );
    }
    for url in [
        "https://unlisted.example/",
        "http://example.com/",
        "https://example.com:8443/",
        "https://user:pass@example.com/",
    ] {
        let error = runtime
            .call("demo", "http", json!({"url":url}))
            .await
            .unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::DeniedCapability,
            "https-exact-host: {url}: {error}"
        );
    }
    let ambient = runtime.call("demo", "ambient", json!({})).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&ambient.json).unwrap(),
        json!({"env":null,"file":false,"socket":false}),
        "no-ambient-wasi"
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&home.config, home.workspace.join("escape")).unwrap();
        assert_eq!(
            runtime
                .call(
                    "demo",
                    "read",
                    json!({"path":"escape/gadgets/demo/gadget.toml"})
                )
                .await
                .unwrap_err()
                .kind,
            ErrorKind::DeniedCapability,
            "scoped-symlink"
        );
        std::fs::create_dir(home.workspace.join(".git")).unwrap();
        std::os::unix::fs::symlink(".git", home.workspace.join("alias")).unwrap();
        assert_eq!(
            runtime
                .call("demo", "write", json!({"path":"alias/config","text":"bad"}))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::DeniedCapability,
            "nofollow-write-parent"
        );
        assert!(!home.workspace.join(".git/config").exists());
    }
}

#[tokio::test]
async fn capabilities_and_file_write_approval_are_independent() {
    let home = TestHome::new(capabilities(false, true, &[]));
    let options = home.options();
    let runtime = home.runtime(options.clone());
    assert_eq!(
        runtime
            .call("demo", "read", json!({"path":"readme.txt"}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::DeniedCapability
    );
    assert_eq!(
        runtime
            .call("demo", "write", json!({"path":"new.txt","text":"hello"}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::DeniedCapability
    );
    assert!(!home.workspace.join("new.txt").exists());
    options.approval.lock().unwrap().mode = PermissionMode::AcceptEdits;
    runtime
        .call(
            "demo",
            "write",
            json!({"path":"nested/new.txt","text":"hello"}),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(home.workspace.join("nested/new.txt")).unwrap(),
        "hello"
    );
    assert!(
        runtime
            .call(
                "demo",
                "write",
                json!({"path":"nested/new.txt","text":"overwrite"})
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn approved_gadget_write_enrolls_its_file_checkpoint_and_rewinds() {
    let home = TestHome::new(capabilities(false, true, &[]));
    let store = Arc::new(
        crate::checkpoints::LibsqlCheckpointer::open_at(home.config.join("threads.db"))
            .await
            .unwrap(),
    );
    let checkpoints = Arc::new(
        crate::workspace_checkpoint::WorkspaceCheckpointManager::new(
            home.workspace.clone(),
            store,
            "gadget-checkpoint",
        )
        .with_warning_sink(Some(Arc::new(|_| {}))),
    );
    let mut state = craftsman_harness::AgentState::default();
    state
        .messages
        .push(craftsman_harness::Msg::user("Create a gadget output file."));
    craftsman_harness::AgentMiddleware::turn_start(checkpoints.as_ref(), &mut state)
        .await
        .unwrap();
    let mut options = home.options();
    options.approval.lock().unwrap().mode = PermissionMode::AcceptEdits;
    options.workspace_checkpoints = Some(checkpoints.clone());
    let runtime = home.runtime(options);
    runtime
        .call(
            "demo",
            "write",
            json!({"path":"nested/created.txt","text":"gadget output"}),
        )
        .await
        .unwrap();
    craftsman_harness::AgentMiddleware::turn_end(checkpoints.as_ref(), &mut state)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(home.workspace.join("nested/created.txt")).unwrap(),
        b"gadget output"
    );
    let list = checkpoints.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].file_count, 1);
    assert_eq!(
        checkpoints.rewind(None, false).await.unwrap().restored,
        ["nested/created.txt"]
    );
    assert!(!home.workspace.join("nested/created.txt").exists());
}

#[tokio::test]
async fn denied_gadget_write_does_not_start_file_checkpoint_work() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let home = TestHome::new(capabilities(false, true, &[]));
    std::fs::create_dir(home.workspace.join("already-a-directory")).unwrap();
    let store = Arc::new(
        crate::checkpoints::LibsqlCheckpointer::open_at(home.config.join("threads.db"))
            .await
            .unwrap(),
    );
    let notices = Arc::new(AtomicUsize::new(0));
    let observed = notices.clone();
    let checkpoints = Arc::new(
        crate::workspace_checkpoint::WorkspaceCheckpointManager::new(
            home.workspace.clone(),
            store,
            "denied-gadget-checkpoint",
        )
        .with_warning_sink(Some(Arc::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        }))),
    );
    let mut state = craftsman_harness::AgentState::default();
    state
        .messages
        .push(craftsman_harness::Msg::user("Do not approve the write."));
    craftsman_harness::AgentMiddleware::turn_start(checkpoints.as_ref(), &mut state)
        .await
        .unwrap();
    let mut options = home.options();
    options.workspace_checkpoints = Some(checkpoints.clone());
    let runtime = home.runtime(options);
    assert_eq!(
        runtime
            .call(
                "demo",
                "write",
                json!({"path":"already-a-directory","text":"blocked"})
            )
            .await
            .unwrap_err()
            .kind,
        ErrorKind::DeniedCapability
    );
    craftsman_harness::AgentMiddleware::turn_end(checkpoints.as_ref(), &mut state)
        .await
        .unwrap();
    assert_eq!(notices.load(Ordering::SeqCst), 0);
    assert!(checkpoints.list().await.unwrap().is_empty());
    assert!(home.workspace.join("already-a-directory").is_dir());
}

#[tokio::test]
async fn timeout_memory_result_and_log_limits_leave_next_call_healthy() {
    let home = TestHome::new(Capabilities::default());
    let mut options = home.options();
    // Bound timeout behavior, not throughput of the stdout stress fixture.
    // Shared CI runners must not turn that independent check into a latency gate.
    options.timeout = Duration::from_secs(1);
    let runtime = home.runtime(options);
    runtime.call("demo", "ping", json!({})).await.unwrap(); // warm the compiler
    let started = Instant::now();
    assert_eq!(
        runtime
            .call("demo", "spin", json!({}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Timeout
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        runtime
            .call("demo", "sleep", json!({}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Timeout
    );
    assert_eq!(
        runtime.call("demo", "ping", json!({})).await.unwrap().json,
        r#"{"pong":true}"#
    );
    // Memory/result/output limits have their own generous wall-clock budget.
    let runtime = home.runtime(home.options());
    assert_eq!(
        runtime
            .call("demo", "allocate", json!({}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::OutOfMemory
    );
    assert_eq!(
        runtime
            .call("demo", "enormous", json!({}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::OutputLimit
    );
    let output = runtime.call("demo", "output", json!({})).await.unwrap();
    assert!(output.logs_truncated);
    assert!(output.logs.len() <= super::host::MAX_LOG_BYTES);
    assert_eq!(
        runtime.call("demo", "ping", json!({})).await.unwrap().json,
        r#"{"pong":true}"#
    );
}

#[test]
fn manifest_bounds_names_and_capability_validation() {
    for id in ["ab", "a-1", &"a".repeat(64)] {
        assert!(valid_id(id));
    }
    for id in ["a", "A-b", "-ab", "ab-", "../ab", &"a".repeat(65)] {
        assert!(!valid_id(id));
    }
    for host in [
        "EXAMPLE.com",
        "*.example.com",
        "https://example.com",
        "example.com:443",
        "example.com/path",
        "user@example.com",
        "example.com.",
    ] {
        assert!(
            capabilities(false, false, &[host]).validate().is_err(),
            "{host}"
        );
    }
    assert!(
        Capabilities {
            read: BTreeSet::from(["home".into()]),
            ..Capabilities::default()
        }
        .validate()
        .is_err()
    );
    assert!(Manifest::parse("id='ab'\nversion='0.1.0'\nunknown=true", "ab").is_err());
}

#[test]
fn malformed_and_non_private_grants_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("gadgets");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join(".grants.json");
    std::fs::write(&path, "invalid json").unwrap();
    let grants = GrantStore::new(root.path());
    assert!(
        grants
            .approved(
                "demo",
                root.path(),
                &"0".repeat(64),
                &Capabilities::default()
            )
            .is_err()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&path, r#"{"version":1,"entries":[]}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            grants
                .approved(
                    "demo",
                    root.path(),
                    &"0".repeat(64),
                    &Capabilities::default()
                )
                .is_err()
        );
    }
}

#[tokio::test]
async fn mock_agent_observes_mid_session_rebuild_with_a_fixed_tool_list() {
    use craftsman_harness::{
        Agent, AgentError, ModelProvider, ModelRequest, ModelResponse, Msg, ToolCall, Usage,
    };
    struct Model {
        round: AtomicUsize,
        gadget: PathBuf,
        definitions: std::sync::Mutex<Option<Vec<craftsman_harness::ToolDef>>>,
    }
    #[async_trait::async_trait]
    impl ModelProvider for Model {
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, AgentError> {
            let mut definitions = self.definitions.lock().unwrap();
            match definitions.as_ref() {
                Some(original) => assert_eq!(
                    original, &request.tools,
                    "hot reload must not change tool definitions"
                ),
                None => *definitions = Some(request.tools.clone()),
            }
            drop(definitions);
            let round = self.round.fetch_add(1, Ordering::Relaxed);
            if round > 0 {
                let tool = request
                    .messages
                    .iter()
                    .rev()
                    .find_map(|m| match m {
                        Msg::Tool { content, .. } => Some(content),
                        _ => None,
                    })
                    .unwrap();
                let json: Value = serde_json::from_str(tool).unwrap();
                assert_eq!(json["version"], if round == 1 { "0.1.0" } else { "0.2.0" });
                assert!(
                    json.get("meta").is_none(),
                    "harness facts must not reach the model"
                );
            }
            if round == 1 {
                std::fs::write(self.gadget.join("gadget.wasm"), &fixtures().1).unwrap();
            }
            Ok(ModelResponse {
                message: Msg::Assistant {
                    content: if round == 2 {
                        "rebuilt successfully".into()
                    } else {
                        String::new()
                    },
                    tool_calls: if round == 2 {
                        vec![]
                    } else {
                        vec![ToolCall {
                            id: format!("call-{round}"),
                            name: "gadget_call".into(),
                            args: json!({"gadget":"demo", "function":"echo", "args":{"text":"from agent"}}),
                        }]
                    },
                },
                usage: Usage::default(),
            })
        }
    }
    let home = TestHome::new(Capabilities::default());
    let runtime = home.runtime(home.options());
    let model = Arc::new(Model {
        round: AtomicUsize::new(0),
        gadget: home.gadget.clone(),
        definitions: std::sync::Mutex::new(None),
    });
    let mut builder = Agent::builder(model.clone());
    for tool in runtime.tools() {
        builder = builder.tool(tool);
    }
    let agent = builder.build();
    assert!(matches!(
        agent.run_turn("test gadgets").await.unwrap(),
        craftsman_harness::TurnResult::Done(_)
    ));
    assert_eq!(model.round.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn deterministic_catalog_detects_install_rebuild_remove_without_list_polling() {
    use craftsman_harness::{AgentMiddleware, AgentState};
    let home = TestHome::new(Capabilities::default());
    std::fs::remove_dir_all(&home.gadget).unwrap();
    let runtime = home.runtime(home.options());
    let definitions: Vec<_> = runtime.tools().iter().map(|t| t.def()).collect();
    let middleware = GadgetCatalogMiddleware::new(runtime.clone());
    let mut state = AgentState::default();
    middleware.prepare_model(&mut state).await.unwrap();
    assert_eq!(super::catalogue::notifications(&state), 1);
    middleware.prepare_model(&mut state).await.unwrap();
    assert_eq!(
        super::catalogue::notifications(&state),
        1,
        "unchanged catalog must add nothing"
    );
    home.manifest(&home.gadget, Capabilities::default());
    std::fs::write(home.gadget.join("gadget.wasm"), &fixtures().0).unwrap();
    middleware.prepare_model(&mut state).await.unwrap();
    assert_eq!(
        super::catalogue::notifications(&state),
        2,
        "installation must be noticed deterministically"
    );
    let notice = match state.messages.last().unwrap() {
        craftsman_harness::Msg::User { content } => content,
        _ => panic!("missing notice"),
    };
    assert!(
        notice.contains("\"id\":\"demo\"")
            && notice.contains("\"name\":\"echo\"")
            && notice.contains("input_schema")
    );
    std::fs::write(home.gadget.join("gadget.wasm"), &fixtures().1).unwrap();
    middleware.prepare_model(&mut state).await.unwrap();
    assert_eq!(
        super::catalogue::notifications(&state),
        3,
        "binary-only rebuild must be noticed"
    );
    middleware.prepare_model(&mut state).await.unwrap();
    assert_eq!(super::catalogue::notifications(&state), 3);
    std::fs::remove_dir_all(&home.gadget).unwrap();
    middleware.prepare_model(&mut state).await.unwrap();
    assert_eq!(
        super::catalogue::notifications(&state),
        4,
        "removal must append a new notice even when it returns to the initial empty catalog"
    );
    assert_eq!(
        definitions,
        runtime.tools().iter().map(|t| t.def()).collect::<Vec<_>>()
    );
    state.messages.clear(); // Simulate compaction removing the last catalog.
    middleware.prepare_model(&mut state).await.unwrap();
    assert_eq!(
        super::catalogue::notifications(&state),
        1,
        "current catalog must be restored after compaction"
    );
}

#[tokio::test]
async fn automatic_discovery_asks_once_and_declined_code_never_executes() {
    use craftsman_harness::{AgentMiddleware, AgentState};
    let home = TestHome::new(Capabilities::default());
    let prompts = Arc::new(AtomicUsize::new(0));
    let probe = prompts.clone();
    let mut options = home.options();
    options.allowed_gadgets.clear();
    options.interactive = true;
    options.prompt = Some(Arc::new(move |_, _, _| {
        probe.fetch_add(1, Ordering::Relaxed);
        ApprovalPromptResult::Reject("not now".into())
    }));
    let runtime = home.runtime(options);
    let middleware = GadgetCatalogMiddleware::new(runtime.clone());
    let mut state = AgentState::default();
    middleware.prepare_model(&mut state).await.unwrap();
    middleware.prepare_model(&mut state).await.unwrap();
    assert_eq!(prompts.load(Ordering::Relaxed), 1);
    assert_eq!(runtime.compiled_count(), 0);
    assert_eq!(super::catalogue::notifications(&state), 1);
    std::fs::write(home.gadget.join("gadget.wasm"), &fixtures().1).unwrap();
    middleware.prepare_model(&mut state).await.unwrap();
    assert_eq!(
        prompts.load(Ordering::Relaxed),
        2,
        "a new identity may ask again"
    );
    assert_eq!(runtime.compiled_count(), 0);
}

#[tokio::test]
async fn live_install_is_announced_then_called_directly_in_the_mock_agent_loop() {
    use craftsman_harness::{
        Agent, AgentError, ModelProvider, ModelRequest, ModelResponse, Msg, Tool, ToolCall,
        ToolCtx, ToolDef, ToolOutcome, Usage,
    };
    struct Install(PathBuf);
    #[async_trait::async_trait]
    impl Tool for Install {
        fn def(&self) -> ToolDef {
            ToolDef {
                name: "install_fixture".into(),
                description: "Install a source-built fixture.".into(),
                parameters: json!({"type":"object","properties":{}}),
            }
        }
        async fn call(&self, _: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
            std::fs::create_dir_all(&self.0).unwrap();
            std::fs::write(self.0.join("gadget.toml"), "id='demo'\nversion='0.1.0'\n").unwrap();
            std::fs::write(self.0.join("gadget.wasm"), &fixtures().0).unwrap();
            Ok(ToolOutcome::ok("installed"))
        }
    }
    struct Model {
        round: AtomicUsize,
        definitions: std::sync::Mutex<Option<Vec<ToolDef>>>,
    }
    #[async_trait::async_trait]
    impl ModelProvider for Model {
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, AgentError> {
            assert_eq!(request.system, "stable system prefix");
            let mut defs = self.definitions.lock().unwrap();
            if let Some(original) = defs.as_ref() {
                assert_eq!(original, &request.tools);
            } else {
                *defs = Some(request.tools.clone());
            }
            drop(defs);
            let round = self.round.fetch_add(1, Ordering::Relaxed);
            let notices: Vec<_> = request
                .messages
                .iter()
                .filter_map(|m| match m {
                    Msg::User { content } if content.starts_with("<gadget_catalog_update") => {
                        Some(content)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(notices.len(), if round == 0 { 1 } else { 2 });
            if round > 0 {
                assert!(
                    notices.last().unwrap().contains("\"name\":\"echo\""),
                    "agent must know the new function without asking gadget_list"
                );
            }
            let calls = match round {
                0 => vec![ToolCall {
                    id: "install".into(),
                    name: "install_fixture".into(),
                    args: json!({}),
                }],
                1 => vec![ToolCall {
                    id: "use".into(),
                    name: "gadget_call".into(),
                    args: json!({"gadget":"demo","function":"echo","args":{"text":"new capability"}}),
                }],
                _ => {
                    assert!(request.messages.iter().any(|m| matches!(m, Msg::Tool { content, .. } if content.contains("new capability"))));
                    vec![]
                }
            };
            Ok(ModelResponse {
                message: Msg::Assistant {
                    content: if calls.is_empty() {
                        "done".into()
                    } else {
                        String::new()
                    },
                    tool_calls: calls,
                },
                usage: Usage::default(),
            })
        }
    }
    let home = TestHome::new(Capabilities::default());
    std::fs::remove_dir_all(&home.gadget).unwrap();
    let runtime = home.runtime(home.options());
    let model = Arc::new(Model {
        round: AtomicUsize::new(0),
        definitions: std::sync::Mutex::new(None),
    });
    let mut builder = Agent::builder(model.clone())
        .system_prompt("stable system prefix")
        .middleware(Arc::new(GadgetCatalogMiddleware::new(runtime.clone())))
        .tool(Arc::new(Install(home.gadget.clone())));
    for tool in runtime.tools() {
        builder = builder.tool(tool);
    }
    assert!(matches!(
        builder
            .build()
            .run_turn("install a capability and use it")
            .await
            .unwrap(),
        craftsman_harness::TurnResult::Done(_)
    ));
    assert_eq!(model.round.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn aliased_user_and_project_roots_do_not_cache_away_workspace_trust() {
    let home = TestHome::new(Capabilities::default());
    let config = home.workspace.join(".craftsman");
    let project = config.join("gadgets/demo");
    home.manifest(&project, Capabilities::default());
    std::fs::write(project.join("gadget.wasm"), &fixtures().0).unwrap();
    let runtime = GadgetRuntime::new(&home.workspace, &config, home.options()).unwrap();
    assert_eq!(
        runtime
            .call("demo", "ping", json!({}))
            .await
            .unwrap_err()
            .kind,
        ErrorKind::NeedsApproval
    );
    assert!(runtime.list().await.unwrap()[0].project);
}

#[tokio::test]
async fn over_limit_roots_fail_deterministically_instead_of_loading_an_arbitrary_subset() {
    let home = TestHome::new(Capabilities::default());
    for i in 0..64 {
        let id = format!("extra-{i:02}");
        let path = home.config.join("gadgets").join(&id);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(
            path.join("gadget.toml"),
            format!("id='{id}'\nversion='0.1.0'\n"),
        )
        .unwrap();
    }
    let runtime = home.runtime(home.options());
    for _ in 0..2 {
        let list = runtime.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "<limit>");
        assert_eq!(list[0].status, "broken");
    }
    assert_eq!(runtime.compiled_count(), 0);
}

#[cfg(unix)]
#[test]
fn native_code_cache_is_never_loaded_from_project_shared_or_symlink_roots() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let config = root.path().join("config");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&config).unwrap();
    assert!(super::runtime::trusted_cache_directory(&config, &workspace).is_some());
    assert!(super::runtime::trusted_cache_directory(&workspace, &workspace).is_none());
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(super::runtime::trusted_cache_directory(&config, &workspace).is_none());
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir_all(config.join("cache")).unwrap();
    std::os::unix::fs::symlink(&workspace, config.join("cache")).unwrap();
    assert!(super::runtime::trusted_cache_directory(&config, &workspace).is_none());
}

#[tokio::test]
async fn delegated_agents_receive_the_catalog_without_a_list_tool_call() {
    use craftsman_harness::builtin::SubAgentMiddleware;
    use craftsman_harness::{
        Agent, AgentError, ModelProvider, ModelRequest, ModelResponse, Msg, SubAgentSpec, ToolCall,
        Usage,
    };
    struct Child(AtomicUsize);
    #[async_trait::async_trait]
    impl ModelProvider for Child {
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, AgentError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            assert_eq!(request.system, "child stable prefix");
            assert!(request.messages.iter().any(|m| matches!(m, Msg::User { content } if content.starts_with("<gadget_catalog_update") && content.contains("\"name\":\"echo\""))));
            Ok(ModelResponse {
                message: Msg::assistant("child discovered gadgets without polling"),
                usage: Usage::default(),
            })
        }
    }
    struct Parent(AtomicUsize);
    #[async_trait::async_trait]
    impl ModelProvider for Parent {
        async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
            let round = self.0.fetch_add(1, Ordering::Relaxed);
            Ok(ModelResponse {
                message: if round == 0 {
                    Msg::Assistant {
                        content: String::new(),
                        tool_calls: vec![ToolCall {
                            id: "delegate".into(),
                            name: "task".into(),
                            args: json!({"description":"inspect available gadgets", "subagent_type":"catalog-child"}),
                        }],
                    }
                } else {
                    Msg::assistant("done")
                },
                usage: Usage::default(),
            })
        }
    }
    let home = TestHome::new(Capabilities::default());
    let runtime = home.runtime(home.options());
    let child = Arc::new(Child(AtomicUsize::new(0)));
    let specification = SubAgentSpec {
        name: "catalog-child".into(),
        description: "Catalog probe.".into(),
        system_prompt: "child stable prefix".into(),
        tools: runtime.tools(),
        model: Some(child.clone()),
        harness_profile: None,
        interrupt_on: vec![],
        max_depth: 1,
    };
    let subagents = SubAgentMiddleware::new(vec![specification])
        .child_middleware(Arc::new(GadgetCatalogMiddleware::new(runtime.clone())));
    let agent = Agent::builder(Arc::new(Parent(AtomicUsize::new(0))))
        .middleware(Arc::new(GadgetCatalogMiddleware::new(runtime)))
        .middleware(Arc::new(subagents))
        .build();
    assert!(matches!(
        agent.run_turn("delegate catalog inspection").await.unwrap(),
        craftsman_harness::TurnResult::Done(_)
    ));
    assert_eq!(child.0.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn hidden_entries_cannot_make_catalog_enumeration_unbounded() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let config = root.path().join("config");
    let gadgets = config.join("gadgets");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&gadgets).unwrap();
    for index in 0..=crate::extensions::MAX_DIR_ENTRIES {
        std::fs::write(gadgets.join(format!(".junk-{index}")), b"").unwrap();
    }
    let runtime = GadgetRuntime::new(
        &workspace,
        &config,
        RuntimeOptions {
            disk_cache: false,
            ..Default::default()
        },
    )
    .unwrap();
    let listed = runtime.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].status, "broken");
    assert!(listed[0].error.as_deref().unwrap().contains("1024"));
    assert_eq!(runtime.compiled_count(), 0);
    std::fs::create_dir_all(config.join("plugins")).unwrap();
    std::fs::write(
        config.join("plugins/review.json"),
        serde_json::json!({"version":1,"name":"review-flow","prompt":"p"}).to_string(),
    )
    .unwrap();
    let report = super::migration::migrate(&config, &workspace, false, false).unwrap();
    assert!(!report.complete);
    assert!(!gadgets.join("review-flow").exists());
}

#[path = "benchmark.rs"]
mod benchmark;
