//! Synthetic concrete producers use an actual builder's opaque lease/unique
//! gate. These fixture facts are not live model-quality/usage/OS evidence.
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::framework::Framework;
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::runtime::InvocationGate;
use craftsman_harness::builtin::{SubAgentMiddleware, TodoListMiddleware};
use craftsman_harness::state::{Todo, TodoStatus};
use craftsman_harness::testing::ScriptedModel;
use craftsman_harness::{
    AgentRegistry, AgentStatus, Backend, HarnessProfile, ModelProvider, Msg, NotificationKind,
    NotificationQueue, SubAgentSpec, ToolErrorKind, ToolMeta, ToolOutcome,
};
use std::time::Duration;

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
}
struct Harness {
    _session: Session,
    framework: Arc<Framework>,
    filesystem: Arc<crate::exec::confined_fs::TextFs>,
    gate: Arc<InvocationGate>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let config = root.path().join("selected");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        Self {
            _root: root,
            workspace,
            config,
        }
    }
    async fn harness(&self, extra: Value, selectors: Selectors) -> Harness {
        let mut config = json!({"model":"synthetic", "api_key":"synthetic-secret", "search_api_key":"search-secret", "base_url":"http://127.0.0.1:9"});
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
        let (session, gate) = build_prepared_session(&prepared).await.unwrap();
        Harness {
            _session: session,
            framework: Arc::new(Framework::from_scope(FrameworkScope {
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
            "tool:write_todos".into(),
            "tool:task".into(),
            "tool:send_message".into(),
            "tool:write_file".into(),
        ],
        ..Default::default()
    }
}
impl Harness {
    fn todos(&self) -> Arc<dyn AgentMiddleware> {
        self.gate
            .wrap_middleware(self.framework.todo_factory().materialize())
    }
    fn subagents(&self, mut specs: Vec<SubAgentSpec>) -> Arc<dyn AgentMiddleware> {
        for spec in &mut specs {
            spec.tools = self.gate.wrap_tools(std::mem::take(&mut spec.tools));
            spec.model = spec.model.take().map(|m| self.gate.wrap_provider(m));
        }
        let original = SubAgentMiddleware::new(specs)
            .for_agent("main")
            .child_middleware(self.gate.clone());
        self.gate.wrap_middleware(
            self.framework
                .subagents(SubagentOutputSource {
                    original,
                    gate: self.gate.clone(),
                })
                .unwrap(),
        )
    }
    async fn call(
        &self,
        middleware: &Arc<dyn AgentMiddleware>,
        name: &str,
        args: Value,
        state: &mut AgentState,
        registry: Arc<AgentRegistry>,
        notifications: NotificationQueue,
    ) -> ToolOutcome {
        let tool = middleware
            .tools()
            .into_iter()
            .find(|(def, _)| def.name == name)
            .unwrap()
            .1;
        tool.call(
            args,
            &mut ToolCtx {
                state,
                backend: self.filesystem.clone() as Arc<dyn Backend>,
                registry,
                notifications,
            },
        )
        .await
        .unwrap()
    }
    async fn todo(&self, args: Value, state: &mut AgentState) -> ToolOutcome {
        self.call(
            &self.todos(),
            "write_todos",
            args,
            state,
            Arc::new(AgentRegistry::default()),
            NotificationQueue::default(),
        )
        .await
    }
}
fn parsed_view(text: &str) -> Value {
    serde_json::from_str(text.strip_prefix("Updated todo list to ").unwrap()).unwrap()
}
fn spec(
    name: &str,
    model: Arc<dyn ModelProvider>,
    profile: Option<HarnessProfile>,
) -> SubAgentSpec {
    SubAgentSpec {
        name: name.into(),
        description: "synthetic declared role".into(),
        system_prompt: "synthetic context".into(),
        tools: vec![],
        model: Some(model),
        harness_profile: profile,
        interrupt_on: vec![],
        max_depth: 1,
    }
}
fn original_todo(content: &str) -> Todo {
    Todo {
        content: content.into(),
        status: TodoStatus::Pending,
    }
}

#[tokio::test]
async fn original_todo_fields_and_statuses_stay_in_state_but_views_are_redacted() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let mut state = AgentState::default();
    let out = h.todo(json!({"todos":[{"content":"synthetic-secret step","status":"pending","ignored":"unchanged parser"}]}), &mut state).await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(state.todos, vec![original_todo("synthetic-secret step")]);
    assert_eq!(
        parsed_view(&out.content),
        json!([{"content":"[REDACTED] step","status":"pending"}])
    );
    assert_eq!(out.meta.bytes, None);
    assert_eq!(out.meta.network_calls, 0);
    assert!(!out.meta.truncated);
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}
#[tokio::test]
async fn complete_todo_fields_are_redacted_before_json_escaping() {
    let f = Fixture::new();
    let seed = "quote\"\\\n秘密";
    let h = f.harness(json!({"search_api_key":seed}), allow()).await;
    let mut state = AgentState::default();
    let out = h
        .todo(
            json!({"todos":[{"content":format!("before {seed} after"),"status":"completed"}]}),
            &mut state,
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(
        parsed_view(&out.content)[0]["content"],
        "before [REDACTED] after"
    );
    assert_eq!(state.todos[0].content, format!("before {seed} after"));
    assert_eq!(state.todos[0].status, TodoStatus::Completed);
    assert!(!out.content.contains("quote") && !out.content.contains("秘密"));
}
#[tokio::test]
async fn malformed_todo_args_are_fixed_typed_errors_without_raw_validation_or_mutation() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    for args in [
        json!({}),
        json!({"todos":null}),
        json!({"todos":7}),
        json!({"todos":[{"content":"x","status":"synthetic-secret"}]}),
        json!({"todos":[{"content":3,"status":"pending"}]}),
    ] {
        let mut state = AgentState {
            todos: vec![original_todo("original")],
            ..Default::default()
        };
        let out = h.todo(args, &mut state).await;
        assert!(out.is_error);
        assert_eq!(out.meta.error_kind, Some(ToolErrorKind::InvalidArgs));
        assert_eq!(out.content, "[REDACTED]");
        assert_eq!(state.todos, vec![original_todo("original")]);
        assert_eq!(out.meta.bytes, None);
        assert_eq!(out.meta.network_calls, 0);
        assert!(!h.gate.approval_required());
    }
}
#[tokio::test]
async fn original_todo_input_admission_fails_before_parse_copy_or_state_effects() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let mut state = AgentState {
        todos: vec![original_todo("original")],
        ..Default::default()
    };
    let out = h
        .todo(
            json!({"todos":[{"content":"q".repeat(1024*1024),"status":"pending"}]}),
            &mut state,
        )
        .await;
    assert!(out.is_error && out.meta.truncated);
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::OutputLimit));
    assert_eq!(out.content, "[REDACTED]");
    assert_eq!(state.todos, vec![original_todo("original")]);
    assert_eq!(out.meta.bytes, None);
}
#[tokio::test]
async fn whole_todo_output_does_not_publish_or_repair_a_json_prefix_on_model_budget_cut() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let original = format!("{}synthetic-secret tail", "q".repeat(120_000));
    let mut state = AgentState::default();
    let out = h
        .todo(
            json!({"todos":[{"content":original,"status":"pending"}]}),
            &mut state,
        )
        .await;
    assert!(out.is_error && out.meta.truncated);
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::OutputLimit));
    assert_eq!(out.content, "[REDACTED]");
    assert_eq!(state.todos[0].content, original);
    assert_eq!(out.meta.bytes, None);
    assert_eq!(out.meta.network_calls, 0);
}
#[tokio::test]
async fn fixed_todo_schema_and_enum_collisions_fail_not_rename_or_change_status_authority() {
    for seed in ["content", "status", "pending"] {
        let f = Fixture::new();
        let h = f.harness(json!({"search_api_key":seed}), allow()).await;
        let mut state = AgentState::default();
        let out = h
            .todo(
                json!({"todos":[{"content":"step","status":"pending"}]}),
                &mut state,
            )
            .await;
        assert!(out.is_error && out.meta.truncated);
        assert_eq!(out.meta.error_kind, Some(ToolErrorKind::Failed));
        assert_eq!(out.content, "[REDACTED]");
        assert_eq!(state.todos, vec![original_todo("step")]);
        assert!(!h.gate.approval_required());
    }
}
#[tokio::test]
async fn replacement_and_framing_json_collisions_are_fixed_failures_not_syntax_corruption() {
    for extra in [
        json!({"api_key":"secret", "search_api_key":"x[REDACTED]"}),
        json!({"search_api_key":"to ["}),
    ] {
        let f = Fixture::new();
        let h = f.harness(extra, allow()).await;
        let mut state = AgentState::default();
        let out = h
            .todo(
                json!({"todos":[{"content":"xsecret","status":"pending"}]}),
                &mut state,
            )
            .await;
        assert!(out.is_error);
        assert_eq!(out.meta.error_kind, Some(ToolErrorKind::Failed));
        assert_eq!(out.content, "[REDACTED]");
        assert_eq!(state.todos, vec![original_todo("xsecret")]);
    }
}
#[tokio::test]
async fn known_todo_context_views_are_checked_before_retention_without_changing_original_state() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let original = format!(
        "synthetic-secret {} synthetic-secret tail",
        "q".repeat(120_000)
    );
    let mut state = AgentState {
        todos: vec![original_todo(&original)],
        system_extras: vec!["OTHER ORIGINAL CONTEXT".into()],
        ..Default::default()
    };
    h.todos().prepare_model(&mut state).await.unwrap();
    assert_eq!(state.todos, vec![original_todo(&original)]);
    assert_eq!(state.system_extras[0], "OTHER ORIGINAL CONTEXT");
    let view = &state.system_extras[1];
    assert!(view.len() <= 100_000 && view.contains("[output truncated"));
    assert!(!view.contains("synthetic-secret") && !view.contains("tail"));
}
#[tokio::test]
async fn captured_framework_seeds_do_not_reopen_replaced_config() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    std::fs::write(f.config.join("config.json"), json!({"model":"synthetic","api_key":"replacement-secret","search_api_key":"replacement-secret"}).to_string()).unwrap();
    let mut state = AgentState::default();
    let out = h
        .todo(
            json!({"todos":[{"content":"search-secret replacement-secret","status":"pending"}]}),
            &mut state,
        )
        .await;
    assert_eq!(
        parsed_view(&out.content)[0]["content"],
        "[REDACTED] replacement-secret"
    );
}
#[tokio::test]
async fn concrete_foreground_task_keeps_original_description_registry_and_file_authority() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant(
        "synthetic-secret child result",
    )]));
    let middleware = h.subagents(vec![spec("synthetic-secret-agent", model.clone(), None)]);
    let mut state = AgentState::default();
    state.files.insert(
        "synthetic-secret.txt".into(),
        craftsman_harness::state::FileData::text("ORIGINAL source data"),
    );
    let registry = Arc::new(AgentRegistry::default());
    let out = h.call(&middleware, "task", json!({"subagent_type":"synthetic-secret-agent","description":"ORIGINAL synthetic-secret task"}), &mut state, registry.clone(), NotificationQueue::default()).await;
    assert!(!out.is_error);
    assert_eq!(out.content, "[REDACTED] child result");
    assert_eq!(out.meta.bytes, None);
    assert_eq!(out.meta.network_calls, 0);
    assert_eq!(
        registry.status("synthetic-secret-agent#0"),
        Some(AgentStatus::Done)
    );
    assert!(
        state.files.contains_key("synthetic-secret.txt")
            && !state.files.contains_key("[REDACTED].txt")
    );
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].messages.iter().any(
        |m| matches!(m, Msg::User { content } if content == "ORIGINAL synthetic-secret task")
    ));
}
#[tokio::test]
async fn concrete_unknown_task_result_is_safe_without_inventing_a_spawn_or_changing_error_class() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
    let middleware = h.subagents(vec![spec("role", model.clone(), None)]);
    let registry = Arc::new(AgentRegistry::default());
    let mut state = AgentState::default();
    let out = h
        .call(
            &middleware,
            "task",
            json!({"subagent_type":"synthetic-secret-unknown","description":"original"}),
            &mut state,
            registry.clone(),
            NotificationQueue::default(),
        )
        .await;
    assert!(!out.is_error); // Preserve the concrete legacy unknown-role outcome, not fake success/spawn.
    assert!(!out.content.contains("synthetic-secret"));
    assert!(out.content.contains("cannot invoke"));
    assert!(registry.list().is_empty() && model.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn concrete_message_views_preserve_original_recipient_payload_and_real_inbox_delivery() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let middleware = h.subagents(vec![]);
    let registry = Arc::new(AgentRegistry::default());
    let (sender, mut inbox) = tokio::sync::mpsc::unbounded_channel();
    registry.insert_entry(
        "synthetic-secret",
        craftsman_harness::subagent::AgentEntry {
            status: AgentStatus::Running,
            progress: "original".into(),
            join: None,
            inbox: Some(sender),
        },
    );
    let (queue, mut notifications) = craftsman_harness::notification_channel();
    let mut state = AgentState::default();
    let out = h
        .call(
            &middleware,
            "send_message",
            json!({"to":"synthetic-secret","message":"ORIGINAL search-secret payload"}),
            &mut state,
            registry.clone(),
            queue.clone(),
        )
        .await;
    assert_eq!(
        out.content,
        "message sent to [REDACTED] (delivered to inbox)"
    );
    assert!(!out.is_error);
    assert_eq!(inbox.try_recv().unwrap(), "ORIGINAL search-secret payload");
    let observed = notifications.drain();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].to, "synthetic-secret");
    assert!(
        matches!(&observed[0].kind, NotificationKind::Message(text) if text == "ORIGINAL search-secret payload")
    );
    registry.set_status("synthetic-secret", AgentStatus::Done);
    let out = h
        .call(
            &middleware,
            "send_message",
            json!({"to":"synthetic-secret","message":"later original"}),
            &mut state,
            registry,
            queue,
        )
        .await;
    assert_eq!(out.content, "message sent to [REDACTED]");
    assert!(!out.content.contains("delivered"));
    assert!(inbox.try_recv().is_err());
    assert_eq!(out.meta.bytes, None);
}
#[tokio::test]
async fn concrete_profile_todo_factories_share_frozen_scope_but_not_context_or_streams() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let profile = HarnessProfile {
        extra_middleware: vec![h.framework.todo_factory()],
        ..Default::default()
    };
    let model = Arc::new(ScriptedModel::new(vec![
        Msg::Assistant {
            content: "".into(),
            tool_calls: vec![craftsman_harness::ToolCall {
                id: "child-todo".into(),
                name: "write_todos".into(),
                args: json!({"todos":[{"content":"synthetic-secret child step","status":"pending"}]}),
            }],
        },
        Msg::assistant("search-secret child done"),
    ]));
    let middleware = h.subagents(vec![spec("role", model.clone(), Some(profile.clone()))]);
    let mut state = AgentState {
        todos: vec![original_todo("parent original")],
        ..Default::default()
    };
    let out = h
        .call(
            &middleware,
            "task",
            json!({"subagent_type":"role","description":"original"}),
            &mut state,
            Arc::new(AgentRegistry::default()),
            NotificationQueue::default(),
        )
        .await;
    assert_eq!(out.content, "[REDACTED] child done");
    assert_eq!(state.todos, vec![original_todo("parent original")]);
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let text = requests[1]
            .messages
            .iter()
            .find_map(|m| match m {
                Msg::Tool {
                    tool_call_id,
                    content,
                    ..
                } if tool_call_id == "child-todo" => Some(content),
                _ => None,
            })
            .unwrap();
        assert_eq!(parsed_view(text)[0]["content"], "[REDACTED] child step");
        assert!(
            requests[1].system.contains("[REDACTED] child step")
                && !requests[1].system.contains("synthetic-secret child step")
        );
    }
    let factories = profile.materialize_extra_middleware();
    let mut other = AgentState::default();
    let out = h
        .call(
            &factories[0],
            "write_todos",
            json!({"todos":[{"content":"search-secret other","status":"completed"}]}),
            &mut other,
            Arc::new(AgentRegistry::default()),
            NotificationQueue::default(),
        )
        .await;
    assert_eq!(parsed_view(&out.content)[0]["content"], "[REDACTED] other");
    assert_eq!(state.todos, vec![original_todo("parent original")]);
    assert_eq!(other.todos[0].status, TodoStatus::Completed);
}
#[tokio::test]
async fn subagent_source_from_a_different_actual_invocation_is_rejected_before_compilation() {
    let f = Fixture::new();
    let one = f.harness(json!({}), allow()).await;
    let two = f.harness(json!({}), allow()).await;
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
    let source = SubagentOutputSource {
        original: SubAgentMiddleware::new(vec![spec("role", model.clone(), None)])
            .child_middleware(two.gate.clone()),
        gate: two.gate.clone(),
    };
    assert!(one.framework.subagents(source).is_none());
    assert!(model.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn original_typed_error_facts_survive_view_failures_and_success_metadata_never_latches() {
    let f = Fixture::new();
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let h = f
        .harness(
            json!({"api_key":"secret","search_api_key":"x[REDACTED]"}),
            selectors,
        )
        .await;
    // Explicit SYNTHETIC codec metadata, not claimed network receipts/usage.
    let mut meta = ToolMeta::new();
    meta.bytes = Some(27);
    meta.network_calls = 2;
    meta.duration = Some(Duration::from_millis(17));
    meta.exit_code = Some(9);
    meta.error_kind = Some(ToolErrorKind::Timeout);
    let out = h
        .framework
        .publish_for_tests(ToolOutcome::err("xsecret").with_meta(meta.clone()));
    assert_eq!(out.content, "[REDACTED]");
    assert!(out.is_error && out.meta.truncated);
    assert_eq!(out.meta.bytes, meta.bytes);
    assert_eq!(out.meta.network_calls, meta.network_calls);
    assert_eq!(out.meta.duration, meta.duration);
    assert_eq!(out.meta.exit_code, meta.exit_code);
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::Timeout));
    assert!(!h.gate.approval_required());
    meta.error_kind = Some(ToolErrorKind::NeedsApproval);
    let out = h
        .framework
        .publish_for_tests(ToolOutcome::ok("safe NeedsApproval text").with_meta(meta.clone()));
    assert!(!out.is_error && !h.gate.approval_required());
    let out = h
        .framework
        .publish_for_tests(ToolOutcome::ok("xsecret").with_meta(meta));
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::Failed));
    assert!(!h.gate.approval_required());
}
#[tokio::test]
async fn already_truncated_framework_source_is_not_finished_or_repaired_as_a_complete_record() {
    let f = Fixture::new();
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let h = f.harness(json!({}), selectors).await;
    for kind in [None, Some(ToolErrorKind::NeedsApproval)] {
        let mut meta = ToolMeta::new();
        meta.truncated = true;
        meta.error_kind = kind;
        let out = h
            .framework
            .publish_for_tests(ToolOutcome::ok("synthetic-se").with_meta(meta));
        assert_eq!(out.content, "[REDACTED]");
        assert!(out.is_error && out.meta.truncated);
        assert_eq!(out.meta.error_kind, Some(ToolErrorKind::OutputLimit));
        assert_eq!(out.meta.bytes, None);
        assert!(!h.gate.approval_required());
    }
}
#[tokio::test]
async fn actual_shared_typed_stop_blocks_todo_message_and_task_effects_and_late_publication() {
    let f = Fixture::new();
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let h = f.harness(json!({}), selectors).await;
    let mut state = AgentState::default();
    let assembled = h
        .todo(
            json!({"todos":[{"content":"synthetic-secret original","status":"pending"}]}),
            &mut state,
        )
        .await;
    assert!(!assembled.is_error);
    let metadata = assembled.meta.clone();
    let denied = h
        .filesystem
        .call("write_file", json!({"file_path":"build.rs","content":"no"}))
        .await;
    assert_eq!(denied.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert!(h.gate.approval_required());
    let out = h.framework.recheck_for_tests(assembled);
    assert_eq!(out.content, "[REDACTED]");
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert_eq!(out.meta.bytes, metadata.bytes);
    assert_eq!(out.meta.duration, metadata.duration);
    assert_eq!(out.meta.network_calls, metadata.network_calls);
    let out = h
        .todo(
            json!({"todos":[{"content":"later","status":"completed"}]}),
            &mut state,
        )
        .await;
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert_eq!(
        state.todos,
        vec![original_todo("synthetic-secret original")]
    );
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("must not run")]));
    let middleware = h.subagents(vec![spec("role", model.clone(), None)]);
    let registry = Arc::new(AgentRegistry::default());
    let (queue, mut notifications) = craftsman_harness::notification_channel();
    for (name, args) in [
        ("send_message", json!({"to":"original","message":"no"})),
        ("task", json!({"subagent_type":"role","description":"no"})),
    ] {
        let out = h
            .call(
                &middleware,
                name,
                args,
                &mut state,
                registry.clone(),
                queue.clone(),
            )
            .await;
        assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
        assert_eq!(out.meta.bytes, None);
    }
    assert!(
        registry.list().is_empty()
            && notifications.drain().is_empty()
            && model.requests.lock().unwrap().is_empty()
    );
    assert!(!f.workspace.join("build.rs").exists());
}
#[tokio::test]
async fn builtin_definitions_and_concrete_wrapper_exclusions_preserve_the_original_contract() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let scoped = h.framework.todo_factory().materialize();
    let original = TodoListMiddleware::new();
    assert_eq!(
        serde_json::to_value(scoped.tools()[0].0.clone()).unwrap(),
        serde_json::to_value(original.tools()[0].0.clone()).unwrap()
    );
    let profile = HarnessProfile::from_config(craftsman_harness::HarnessProfileConfig {
        excluded_middleware: BTreeSet::from(["todos".into()]),
        ..Default::default()
    });
    assert!(
        craftsman_harness::apply_middleware_profile(vec![scoped], &profile, &BTreeSet::new())
            .unwrap()
            .is_empty()
    );
    // No separately excludable scrub middleware can leave the raw todo producer.
    let profile = HarnessProfile::from_config(craftsman_harness::HarnessProfileConfig {
        excluded_middleware: BTreeSet::from(["framework_outputs".into()]),
        ..Default::default()
    });
    assert!(
        craftsman_harness::apply_middleware_profile(vec![h.todos()], &profile, &BTreeSet::new())
            .is_err()
    );
}

#[tokio::test]
async fn trusted_framework_approval_kind_seals_before_unsafe_view_but_failure_words_do_not() {
    let f = Fixture::new();
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let h = f
        .harness(
            json!({"api_key":"secret","search_api_key":"x[REDACTED]"}),
            selectors,
        )
        .await;
    let out = h.framework.publish_for_tests(ToolOutcome::err(
        "NeedsApproval is just untrusted failure text",
    ));
    assert!(out.is_error && !h.gate.approval_required());
    let mut meta = ToolMeta::new();
    meta.error_kind = Some(ToolErrorKind::NeedsApproval);
    let out = h
        .framework
        .publish_for_tests(ToolOutcome::err("xsecret").with_meta(meta));
    assert_eq!(out.content, "[REDACTED]");
    assert!(h.gate.approval_required());
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert_eq!(out.meta.bytes, None);
    assert_eq!(out.meta.network_calls, 0);
    let mut state = AgentState::default();
    assert!(h.todos().prepare_model(&mut state).await.is_err());
    assert!(state.todos.is_empty() && state.system_extras.is_empty());
}

#[tokio::test]
async fn original_or_restored_todo_context_is_bounded_before_formatter_allocation() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    for todos in [
        vec![original_todo(&"q".repeat(1024 * 1024 + 1))],
        vec![original_todo("q"); 34_000],
    ] {
        let mut state = AgentState {
            todos,
            system_extras: vec!["ORIGINAL other context".into()],
            ..Default::default()
        };
        let count = state.todos.len();
        assert!(h.todos().prepare_model(&mut state).await.is_err());
        assert_eq!(state.todos.len(), count);
        assert_eq!(state.system_extras, vec!["ORIGINAL other context"]);
        assert!(!h.gate.approval_required());
    }
}
