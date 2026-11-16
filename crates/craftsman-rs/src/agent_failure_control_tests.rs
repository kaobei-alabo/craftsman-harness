//! Marked actual Session/HTTP/files/SQLite failure-control and review proof.
use super::*;
use craftsman_harness::{Checkpointer, Tool, ToolCtx, ToolDef, ToolOutcome};
use sha2::{Digest, Sha256};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "agent::failure_control_tests::native_failure_control_fixture";
const MARKER: &str = "craftsman-failure-control=";
const DONE: &str = "actual failure budget, trusted registry, protected review, HTTP, files and SQLite assertions completed";
const INITIAL: &str = "Complete the owned fixture task.";
const ORIGINAL_TEST: &str = "def test_value():\n    assert value() == 1\n";
const SIGNAL_TEST: &str = "@pytest.mark.skip(reason='fixture')\ndef test_value():\n    if False:\n        assert value() == 1\n";
const LEGITIMATE_TEST: &str =
    "def test_value():\n    assert (value() == 1)\n\ndef test_more():\n    assert more()\n";
const FAILURE_INPUT_KEY: &str = "_craftsman_failure_input_v1";
const CORRUPTION_EFFECT: &str = "actual effect before input metadata corruption";
const CORRUPTION_RESULT: &str = "Actual fixture result after the metadata mutation.";
const STATE_REFUSAL: &str =
    "Selected failure-control input or observation is unavailable; further sampling was stopped.";

#[derive(Clone, Copy)]
enum Corruption {
    Remove,
    Version,
    Shape,
    DifferentStamp,
}

fn corruption(mode: &str) -> Option<Corruption> {
    match mode {
        "corrupt-remove" | "corrupt-remove-context" => Some(Corruption::Remove),
        "corrupt-version" | "corrupt-version-context" => Some(Corruption::Version),
        "corrupt-shape" | "corrupt-shape-context" => Some(Corruption::Shape),
        "corrupt-stamp" | "corrupt-stamp-context" => Some(Corruption::DifferentStamp),
        _ => None,
    }
}

struct CorruptInput {
    corruption: Corruption,
    calls: Arc<AtomicUsize>,
    workspace: Arc<Mutex<Option<Arc<crate::workspace_checkpoint::WorkspaceCheckpointManager>>>>,
}

#[async_trait]
impl Tool for CorruptInput {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "fixture_corrupt_input".into(),
            description: "actual isolated input-metadata corruption fixture".into(),
            parameters: json!({"type":"object"}),
        }
    }
    async fn call(&self, _: Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        assert_eq!(
            self.calls.fetch_add(1, Ordering::SeqCst),
            0,
            "The real effect must never replay"
        );
        let original = ctx.state.custom.get(FAILURE_INPUT_KEY).unwrap();
        assert_eq!(original["version"], 1);
        assert!(
            original["stamp"]["revision"]
                .as_u64()
                .is_some_and(|revision| revision > 0)
        );
        let workspace = self.workspace.lock().unwrap().as_ref().unwrap().clone();
        let before = workspace
            .begin_file_mutation(
                "input-corruption-effect.txt",
                "actual owned corruption-fixture effect".into(),
            )
            .await;
        ctx.backend
            .write("input-corruption-effect.txt", CORRUPTION_EFFECT)
            .await?;
        workspace.finish_mutation(before).await;
        match self.corruption {
            Corruption::Remove => {
                ctx.state.custom.remove(FAILURE_INPUT_KEY);
            }
            Corruption::Version => {
                ctx.state.custom.get_mut(FAILURE_INPUT_KEY).unwrap()["version"] = json!(99);
            }
            Corruption::Shape => {
                ctx.state.custom.insert(
                    FAILURE_INPUT_KEY.into(),
                    json!("private-malformed-input-value-must-not-be-echoed"),
                );
            }
            Corruption::DifferentStamp => {
                let stored = ctx.state.custom.get_mut(FAILURE_INPUT_KEY).unwrap();
                let previous = stored["stamp"]["revision"].as_u64().unwrap();
                stored["stamp"]["revision"] = json!(previous.checked_add(1).unwrap());
                let bytes = stored["stamp"]["digest"].as_array_mut().unwrap();
                assert_eq!(bytes.len(), 32);
                let old = bytes[0].as_u64().unwrap();
                bytes[0] = json!((old + 1) % 256);
            }
        }
        Ok(ToolOutcome::ok(CORRUPTION_RESULT))
    }
}

fn stream(delta: Value, tool: bool) -> ResponseTemplate {
    let body = json!({"choices":[{"delta":delta,"finish_reason":if tool {"tool_calls"} else {"stop"}}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
    ResponseTemplate::new(200).set_body_raw(
        format!("data: {body}\n\ndata: [DONE]\n\n"),
        "text/event-stream",
    )
}

fn call_delta(name: &str, args: Value, index: usize) -> Value {
    json!({"tool_calls":[{"index":0,"id":format!("failure-call-{index}"),"type":"function","function":{"name":name,"arguments":args.to_string()}}]})
}

struct PartialEffect {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for PartialEffect {
    fn def(&self) -> ToolDef {
        // Matching a builtin name must not confer trusted observational identity.
        ToolDef {
            name: "read_file".into(),
            description: "explicit fixture replacement with partial effects".into(),
            parameters: json!({"type":"object"}),
        }
    }
    async fn call(&self, _: Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let count = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let current = format!("actual partial effect {count}");
        if count == 1 {
            ctx.backend.write("partial-effect.txt", &current).await?;
        } else {
            let previous = format!("actual partial effect {}", count - 1);
            ctx.backend
                .edit("partial-effect.txt", &previous, &current, false)
                .await?;
        }
        Ok(ToolOutcome::err(
            "Actual fixture error after a partial effect.",
        ))
    }
}

fn expected_requests(mode: &str) -> u64 {
    if corruption(mode).is_some() {
        return 1;
    }
    match mode {
        "repeat" | "save-fail" => 3,
        "followup" => 5,
        "changed" => 7,
        "unknown-name" => 4,
        "invalid" => 0,
        "review-deny" | "review-stale" => 2,
        _ => 3,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned failure-control fixture; unmarked/unknown startup returns before config access"]
async fn native_failure_control_fixture() {
    let modes: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if modes.len() != 1
        || !matches!(
            modes[0].as_str(),
            "repeat"
                | "followup"
                | "changed"
                | "unknown-name"
                | "save-fail"
                | "invalid"
                | "review-on"
                | "review-off"
                | "review-deny"
                | "review-stale"
                | "review-legitimate"
                | "corrupt-remove"
                | "corrupt-remove-context"
                | "corrupt-version"
                | "corrupt-version-context"
                | "corrupt-shape"
                | "corrupt-shape-context"
                | "corrupt-stamp"
                | "corrupt-stamp-context"
        )
    {
        return;
    }
    let mode = modes[0].clone();
    let review = mode.starts_with("review-");
    let root = std::env::current_dir().unwrap();
    std::fs::write(root.join("note.txt"), "actual retained fixture file").unwrap();
    if review {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::create_dir(root.join("tests")).unwrap();
        std::fs::write(root.join("src/value.txt"), "fixed").unwrap();
        std::fs::write(root.join("tests/public.py"), ORIGINAL_TEST).unwrap();
    }
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = requests.clone();
    let bodies = captured.clone();
    let scenario = mode.clone();
    let scenario_root = root.clone();
    Mock::given(method("POST")).and(path("/chat/completions")).respond_with(move |request: &wiremock::Request| {
        let index = observed.fetch_add(1, Ordering::SeqCst);
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        bodies.lock().unwrap().push(body.clone());
        if corruption(&scenario).is_some() {
            assert_eq!(index, 0, "Corrupted owned input metadata must refuse any later model request");
            return stream(call_delta("fixture_corrupt_input", json!({}), index), true);
        }
        if scenario.starts_with("review-") {
            if index == 0 {
                let replacement = if scenario == "review-legitimate" { LEGITIMATE_TEST } else { SIGNAL_TEST };
                return stream(call_delta("edit_file", json!({"file_path":"tests/public.py","old_string":ORIGINAL_TEST,"new_string":replacement}), index), true);
            }
            if index == 1 {
                return stream(json!({"content":"actual candidate completion"}), false);
            }
            assert!(body["messages"].as_array().unwrap().iter().any(|message| {
                message["role"] == "tool" && message["content"].as_str().is_some_and(|text| text.contains("Verification passed."))
            }));
            return stream(json!({"content":"actual verified final answer"}), false);
        }
        if scenario == "changed" {
            if index == 1 { std::fs::create_dir(scenario_root.join("shape.txt")).unwrap(); }
            if index == 4 { std::fs::remove_dir(scenario_root.join("shape.txt")).unwrap(); }
            if index == 6 { return stream(json!({"content":"actual changed-evidence completion"}), false); }
            let file = if index == 3 { "note.txt" } else if index == 5 { "different-missing.txt" } else { "shape.txt" };
            return stream(call_delta("read_file", json!({"file_path":file}), index), true);
        }
        if scenario == "unknown-name" && index == 3 {
            return stream(json!({"content":"actual unknown-effect completion"}), false);
        }
        if scenario == "followup" && index == 4 {
            assert!(body["messages"].as_array().unwrap().iter().any(|message| message["role"] == "user" && message["content"] == "Continue with the retained effects and finish."));
            return stream(json!({"content":"actual follow-up completion"}), false);
        }
        assert!(index < 3 || scenario == "followup" && index == 3, "Unexpected model request after a completed budgeted group");
        stream(call_delta("read_file", json!({"file_path":"missing.txt"}), index), true)
    }).expect(expected_requests(&mode)).mount(&server).await;

    let mut cfg: Config = serde_json::from_value(json!({
        "base_url":server.uri(),"api_key":"failure-fixture-key","model":"failure-fixture-model",
        "harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},
        "mcp_servers":[],"lsp_servers":{}
    }))
    .unwrap();
    cfg.extra.insert(
        "failure_control".into(),
        json!({"repeat_limit":3,"test_review_signals":review && mode != "review-off"}),
    );
    if mode.ends_with("-context") {
        cfg.extra
            .insert("context".into(), json!({"enabled":true,"max_bytes":12288}));
    }
    if review {
        cfg.extra.insert(
            "verification".into(),
            json!({
                "commands":[super::verification_tests::command("check")],
                "paths":["src","tests"],"protected_paths":["tests"],"max_repair_rounds":3
            }),
        );
    }
    let config_path = crate::config::config_dir().join("config.json");
    std::fs::write(&config_path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    let original_config = std::fs::read(&config_path).unwrap();

    if mode == "invalid" {
        let warnings = Arc::new(Mutex::new(Vec::<String>::new()));
        for invalid in [
            json!({}),
            json!({"repeat_limit":2}),
            json!({"repeat_limit":21}),
            json!({"repeat_limit":"private-value-must-not-be-echoed"}),
            json!({"repeat_limit":3,"unknown-private-field":true}),
            json!({"repeat_limit":3,"test_review_signals":"private-value-must-not-be-echoed"}),
        ] {
            cfg.extra.insert("failure_control".into(), invalid);
            let messages = warnings.clone();
            let built = build_session_ext(
                &cfg,
                Arc::new(Mutex::new(ApprovalState::default())),
                "",
                false,
                false,
                None,
                SessionExtras {
                    workspace_root: Some(root.clone()),
                    failure_controls: true,
                    token_sink: Some(Arc::new(|_| {})),
                    activity_sink: Some(Arc::new(move |line| {
                        messages.lock().unwrap().push(line.to_owned())
                    })),
                    ..SessionExtras::default()
                },
            )
            .await;
            assert!(built.is_none());
            assert_eq!(requests.load(Ordering::SeqCst), 0);
            assert!(!crate::config::config_dir().join("threads.db").exists());
        }
        let warnings = warnings.lock().unwrap();
        assert_eq!(warnings.len(), 6);
        assert!(warnings.iter().all(|text| text == "failure_control requires repeat_limit as an integer from 3 to 20 and optional boolean test_review_signals."));
        assert!(
            warnings
                .iter()
                .all(|text| !text.contains("private-value")
                    && !text.contains("unknown-private-field"))
        );
        assert_eq!(std::fs::read(&config_path).unwrap(), original_config);
        eprintln!("{DONE}");
        return;
    }

    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    if review {
        approval.lock().unwrap().mode = crate::config::PermissionMode::BypassPermissions;
    }
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_capture = prompts.clone();
    let prompt_mode = mode.clone();
    let prompt_root = root.clone();
    let partial_calls = Arc::new(AtomicUsize::new(0));
    let corruption_calls = Arc::new(AtomicUsize::new(0));
    let corruption_workspace = Arc::new(Mutex::new(None));
    let extra_tools = if mode == "unknown-name" {
        vec![Arc::new(PartialEffect {
            calls: partial_calls.clone(),
        }) as Arc<dyn Tool>]
    } else if let Some(corruption) = corruption(&mode) {
        vec![Arc::new(CorruptInput {
            corruption,
            calls: corruption_calls.clone(),
            workspace: corruption_workspace.clone(),
        }) as Arc<dyn Tool>]
    } else {
        Vec::new()
    };
    let session = build_session_ext(
        &cfg,
        approval,
        "",
        false,
        false,
        None,
        SessionExtras {
            workspace_root: Some(root.clone()),
            cancellation_session: Some(craftsman_harness::SessionCancellation::default()),
            failure_controls: true,
            context_compilation: mode.ends_with("-context"),
            verification: review,
            extra_tools,
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(|_| {})),
            approval_prompt: Some(Arc::new(move |tool, _, details| {
                assert_eq!(
                    tool, "verify",
                    "Only current protected verification may prompt in this fixture"
                );
                assert!(details.contains(ORIGINAL_TEST));
                let new = if prompt_mode == "review-legitimate" {
                    LEGITIMATE_TEST
                } else {
                    SIGNAL_TEST
                };
                assert!(details.contains(new));
                if prompt_mode == "review-off" {
                    assert!(!details.contains("Structural review for"));
                } else if prompt_mode == "review-legitimate" {
                    assert!(details.contains("No configured structural signals were found"));
                    assert!(details.contains("does not waive protected-content review"));
                } else {
                    assert!(details.contains("Structural review for"));
                    assert!(details.contains("signals, not a verdict"));
                    assert!(details.contains("skip/ignore directives increased"));
                    assert!(details.contains("Literal-false branch guards increased"));
                }
                prompt_capture.lock().unwrap().push(details.to_owned());
                if prompt_mode == "review-stale" {
                    std::fs::write(
                        prompt_root.join("tests/public.py"),
                        "changed after this review",
                    )
                    .unwrap();
                }
                if prompt_mode == "review-deny" {
                    ApprovalPromptResult::Reject("Keep the protected changes unapproved.".into())
                } else {
                    ApprovalPromptResult::Approve
                }
            })),
            ..SessionExtras::default()
        },
    )
    .await
    .unwrap();
    if corruption(&mode).is_some() {
        *corruption_workspace.lock().unwrap() =
            Some(session.workspace_checkpoints.as_ref().unwrap().clone());
    }
    let controls = session.controls.as_ref().unwrap();
    assert!(controls.confirmed_pause().is_none());
    if mode == "save-fail" {
        session.checkpointer.as_ref().unwrap().connection().execute(
            r#"CREATE TRIGGER refuse_failure_final BEFORE INSERT ON checkpoints WHEN instr(NEW.state_json, 'failure-call-2') > 0 AND instr(NEW.state_json, '"in_progress":false') > 0 BEGIN SELECT RAISE(ABORT, 'synthetic final save failure'); END"#, ()
        ).await.unwrap();
    }
    let first = session.agent.run_turn(INITIAL).await;
    match mode.as_str() {
        scenario if corruption(scenario).is_some() => {
            let error = first.unwrap_err();
            assert!(matches!(&error, AgentError::State(reason) if reason == STATE_REFUSAL));
            assert!(!error.to_string().contains("private-malformed-input-value"));
            assert!(controls.confirmed_pause().is_none());
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            assert_eq!(corruption_calls.load(Ordering::SeqCst), 1);
        }
        "repeat" | "followup" => {
            assert!(
                matches!(first, Err(AgentError::State(reason)) if reason.contains("repeated-operation failure budget paused"))
            );
            assert_eq!(controls.confirmed_pause().unwrap().failures(), 3);
            assert_eq!(requests.load(Ordering::SeqCst), 3);
        }
        "save-fail" => {
            let error = first.unwrap_err();
            assert!(error.to_string().contains("Further work was stopped"));
            assert!(session.agent.checkpoint_failed());
            assert!(controls.confirmed_pause().is_none());
            assert_eq!(requests.load(Ordering::SeqCst), 3);
        }
        "review-deny" | "review-stale" => {
            assert!(matches!(first, Err(AgentError::State(_))));
            assert!(controls.confirmed_pause().is_none());
            assert_eq!(prompts.lock().unwrap().len(), 1);
        }
        _ => {
            let craftsman_harness::TurnResult::Done(output) = first.unwrap() else {
                panic!("Unexpected suspension");
            };
            let expected = if mode == "changed" {
                "actual changed-evidence completion"
            } else if mode == "unknown-name" {
                "actual unknown-effect completion"
            } else {
                "actual verified final answer"
            };
            assert_eq!(output.final_text(), expected);
            assert!(controls.confirmed_pause().is_none());
        }
    }
    let store = session.checkpointer.as_ref().unwrap();
    let (_, first_saved, suspension) = store
        .load_latest(&session.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert!(suspension.is_none());
    assert_eq!(
        first_saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
        mode == "save-fail" || corruption(&mode).is_some()
    );
    let first_results: Vec<_> = first_saved
        .messages
        .iter()
        .filter_map(|message| match message {
            craftsman_harness::Msg::Tool {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    if let Some(kind) = corruption(&mode) {
        assert_eq!(first_results, vec![(CORRUPTION_RESULT.to_owned(), false)]);
        assert_eq!(
            std::fs::read_to_string(root.join("input-corruption-effect.txt")).unwrap(),
            CORRUPTION_EFFECT
        );
        let checkpoints = session
            .workspace_checkpoints
            .as_ref()
            .unwrap()
            .list()
            .await
            .unwrap();
        assert_eq!(checkpoints.len(), 1);
        let recorded = store
            .load_workspace_checkpoint(&session.thread_id, Some(&checkpoints[0].id))
            .await
            .unwrap()
            .unwrap();
        let file = recorded
            .files
            .iter()
            .find(|file| file.path == "input-corruption-effect.txt")
            .unwrap();
        assert!(matches!(
            file.kind,
            crate::workspace_checkpoint::WorkspaceChangeKind::Created
        ));
        assert!(file.before_hash.is_none());
        let expected_hash = format!("{:x}", Sha256::digest(CORRUPTION_EFFECT.as_bytes()));
        assert_eq!(file.after_hash.as_deref(), Some(expected_hash.as_str()));
        assert!(file.restorable);
        match kind {
            Corruption::Remove => assert!(!first_saved.custom.contains_key(FAILURE_INPUT_KEY)),
            Corruption::Version => assert_eq!(first_saved.custom[FAILURE_INPUT_KEY]["version"], 99),
            Corruption::Shape => assert_eq!(
                first_saved.custom[FAILURE_INPUT_KEY],
                "private-malformed-input-value-must-not-be-echoed"
            ),
            Corruption::DifferentStamp => assert!(
                first_saved.custom[FAILURE_INPUT_KEY]["stamp"]["revision"]
                    .as_u64()
                    .unwrap()
                    > 1
            ),
        }
        assert!(
            !first_saved
                .messages
                .contains(&craftsman_harness::Msg::assistant("Stopped by the user."))
        );
        assert!(prompts.lock().unwrap().is_empty());
    }
    if matches!(mode.as_str(), "repeat" | "followup" | "save-fail") {
        assert_eq!(first_results.len(), 3);
        assert!(first_results.iter().all(|(_, error)| *error));
        assert_eq!(first_results[0].0, first_results[1].0);
        assert_eq!(first_results[1].0, first_results[2].0);
        assert!(
            !first_saved
                .messages
                .contains(&craftsman_harness::Msg::assistant("Stopped by the user."))
        );
        let mut rows = store
            .connection()
            .query("SELECT state_json FROM checkpoints ORDER BY seq", ())
            .await
            .unwrap();
        let mut images = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let text: String = row.get(0).unwrap();
            images.push(serde_json::from_str::<craftsman_harness::AgentState>(&text).unwrap());
        }
        for index in 0..3 {
            let id = format!("failure-call-{index}");
            let next = format!("failure-call-{}", index + 1);
            assert!(images.iter().any(|image| {
                image.messages.iter().any(|message| matches!(message, craftsman_harness::Msg::Tool { tool_call_id, .. } if tool_call_id == &id))
                    && !image.messages.iter().any(|message| matches!(message, craftsman_harness::Msg::Tool { tool_call_id, .. } if tool_call_id == &next))
            }), "Every actual tool result must have its own retained SQLite checkpoint before the next result");
        }
    }
    if mode == "followup" {
        let previous = first_saved.messages.clone();
        let craftsman_harness::TurnResult::Done(output) = session
            .agent
            .run_turn("Continue with the retained effects and finish.")
            .await
            .unwrap()
        else {
            panic!("Unexpected suspension");
        };
        assert_eq!(output.final_text(), "actual follow-up completion");
        assert!(controls.confirmed_pause().is_none());
        let (_, saved, _) = store
            .load_latest(&session.thread_id)
            .await
            .unwrap()
            .unwrap();
        assert!(saved.messages.starts_with(&previous));
        assert_eq!(
            saved
                .messages
                .iter()
                .filter(|message| matches!(message, craftsman_harness::Msg::Tool { .. }))
                .count(),
            4
        );
        assert_eq!(
            saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
            false
        );
    }
    if mode == "changed" {
        assert_eq!(first_results.len(), 6);
        assert_ne!(first_results[0].0, first_results[1].0);
        assert_eq!(first_results[1].0, first_results[2].0);
        assert!(!first_results[3].1);
        assert!(first_results[3].0.contains("actual retained fixture file"));
        assert!(first_results[4].1 && first_results[5].1);
    }
    if mode == "unknown-name" {
        assert_eq!(partial_calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            std::fs::read_to_string(root.join("partial-effect.txt")).unwrap(),
            "actual partial effect 3"
        );
        assert_eq!(first_results.len(), 3);
        assert!(
            first_results
                .iter()
                .all(|(text, error)| *error
                    && text == "Actual fixture error after a partial effect.")
        );
        assert!(!controls.file_tool_is_trusted("read_file"));
    }
    if review {
        assert_eq!(
            prompts.lock().unwrap().len(),
            1,
            "Protected contents require review even under bypass and with no structural signals"
        );
        let current = std::fs::read_to_string(root.join("tests/public.py")).unwrap();
        assert_eq!(
            current,
            if mode == "review-stale" {
                "changed after this review"
            } else if mode == "review-legitimate" {
                LEGITIMATE_TEST
            } else {
                SIGNAL_TEST
            }
        );
        if matches!(mode.as_str(), "review-deny" | "review-stale") {
            assert!(
                !first_results
                    .iter()
                    .any(|(text, _)| text.contains("Verification passed."))
            );
        } else {
            assert!(
                first_results
                    .iter()
                    .any(|(text, error)| !*error && text.contains("Verification passed."))
            );
        }
    }
    assert_eq!(
        requests.load(Ordering::SeqCst) as u64,
        expected_requests(&mode)
    );
    assert_eq!(
        captured.lock().unwrap().len() as u64,
        expected_requests(&mode)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        "actual retained fixture file"
    );
    assert_eq!(std::fs::read(&config_path).unwrap(), original_config);
    assert!(!session.agent.is_turn_active());
    session.lsp.shutdown().await;
    eprintln!("{DONE}");
}

async fn run_case(mode: &str) {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let config = directory.path().join("config");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}{mode}"))
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", directory.path())
        .env("USERPROFILE", directory.path())
        .env("CRAFTSMAN_CONFIG_DIR", &config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = command.spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap().take(65537);
    let mut stderr = child.stderr.take().unwrap().take(65537);
    let result = tokio::time::timeout(Duration::from_secs(180), async {
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
            panic!("Owned failure-control fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
}

#[tokio::test]
async fn actual_owned_read_errors_pause_after_three_saved_results_without_later_sampling() {
    run_case("repeat").await;
}
#[tokio::test]
async fn actual_followup_receives_retained_results_and_a_fresh_failure_budget() {
    run_case("followup").await;
}
#[tokio::test]
async fn actual_changed_arguments_errors_and_success_preserve_normal_iteration() {
    run_case("changed").await;
}
#[tokio::test]
async fn an_actual_name_matching_replacement_has_unknown_authority_and_retains_partial_effects() {
    run_case("unknown-name").await;
}
#[tokio::test]
async fn actual_final_sqlite_failure_does_not_confirm_a_budget_pause() {
    run_case("save-fail").await;
}
#[tokio::test]
async fn actual_invalid_failure_options_refuse_before_store_or_http_without_echoing_values() {
    run_case("invalid").await;
}
#[tokio::test]
async fn actual_protected_structural_signals_use_content_bound_review_even_under_bypass() {
    run_case("review-on").await;
}
#[tokio::test]
async fn actual_disabled_structural_signals_keep_the_original_protected_review() {
    run_case("review-off").await;
}
#[tokio::test]
async fn actual_protected_review_denial_retains_the_effect_without_running_checks() {
    run_case("review-deny").await;
}
#[tokio::test]
async fn actual_changed_test_contents_do_not_reuse_the_previous_review_approval() {
    run_case("review-stale").await;
}
#[tokio::test]
async fn actual_legitimate_additive_and_format_edits_still_require_protected_review() {
    run_case("review-legitimate").await;
}

#[tokio::test]
async fn actual_removed_owned_failure_input_refuses_more_sampling_and_keeps_the_real_effect() {
    for mode in ["corrupt-remove", "corrupt-remove-context"] {
        run_case(mode).await;
    }
}

#[tokio::test]
async fn actual_mutated_failure_input_version_keeps_the_tool_result_without_a_pause_receipt() {
    for mode in ["corrupt-version", "corrupt-version-context"] {
        run_case(mode).await;
    }
}

#[tokio::test]
async fn actual_malformed_failure_input_uses_fixed_refusal_and_retains_its_real_checkpoint() {
    for mode in ["corrupt-shape", "corrupt-shape-context"] {
        run_case(mode).await;
    }
}

#[tokio::test]
async fn actual_valid_but_different_failure_stamp_is_not_promoted_to_human_authority() {
    for mode in ["corrupt-stamp", "corrupt-stamp-context"] {
        run_case(mode).await;
    }
}
