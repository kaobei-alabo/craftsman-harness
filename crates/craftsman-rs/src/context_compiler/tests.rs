use super::*;
use craftsman_harness::builtin::CompactionTrigger;
use craftsman_harness::testing::ScriptedModel;
use craftsman_harness::{
    Agent, Checkpointer, Msg, Todo, TodoStatus, Tool, ToolDef, ToolErrorKind, ToolMeta, Usage,
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn workspace() -> &'static Path {
    #[cfg(windows)]
    {
        Path::new(r"C:\context-fixture")
    }
    #[cfg(not(windows))]
    {
        Path::new("/context-fixture")
    }
}

fn controller(max_bytes: usize) -> Arc<ContextController> {
    Arc::new(
        ContextController::new(
            ContextOptions {
                enabled: true,
                max_bytes,
            },
            workspace(),
        )
        .unwrap(),
    )
}

fn call(name: &str, path: &str) -> ToolCall {
    ToolCall {
        id: "reused-id".into(),
        name: name.into(),
        args: json!({"file_path":path}),
    }
}

fn payload(compiled: &CompiledContext) -> Value {
    serde_json::from_str(compiled.text.split_once('\n').unwrap().1).unwrap()
}

fn control_outcome(kind: ToolErrorKind) -> ToolOutcome {
    let mut meta = ToolMeta::new();
    meta.error_kind = Some(kind);
    ToolOutcome::err("private control detail").with_meta(meta)
}

#[test]
fn missing_and_false_settings_preserve_disabled_default() {
    assert_eq!(
        ContextOptions::parse(None).unwrap(),
        ContextOptions::default()
    );
    assert_eq!(
        ContextOptions::parse(Some(&json!({"enabled":false}))).unwrap(),
        ContextOptions::default()
    );
    assert!(
        !ContextOptions::parse(Some(&json!({"max_bytes":2048})))
            .unwrap()
            .enabled
    );
    for bytes in [2048, 12288, 32768] {
        assert_eq!(
            ContextOptions::parse(Some(&json!({"enabled":true,"max_bytes":bytes})))
                .unwrap()
                .max_bytes,
            bytes
        );
    }
}

#[test]
fn malformed_selected_settings_fail_fixed_without_value_diagnostics() {
    for value in [
        Value::Null,
        json!("private-setting"),
        json!({"enabled":1}),
        json!({"enabled":true,"max_bytes":2047}),
        json!({"max_bytes":32769}),
        json!({"max_bytes":-1}),
        json!({"max_bytes":2048.0}),
        json!({"enabled":false,"private-setting":"private-key"}),
    ] {
        let error = ContextOptions::parse(Some(&value)).unwrap_err().to_string();
        assert!(error.contains(CONFIG_ERROR));
        assert!(!error.contains("private-setting") && !error.contains("private-key"));
    }
}

#[test]
fn disabled_compiler_does_not_read_or_mutate_private_context_records() {
    let context = ContextController::new(ContextOptions::default(), workspace()).unwrap();
    let mut state = AgentState::default();
    state
        .custom
        .insert(state::KEY.into(), json!("malformed-private-context"));
    let before = serde_json::to_vec(&state).unwrap();
    assert!(context.compile(&state).unwrap().is_none());
    assert!(context.append_context(&mut state).unwrap().is_none());
    context
        .observe_tool(
            &mut state,
            &call("edit_file", "x"),
            &ToolOutcome::ok("done"),
        )
        .unwrap();
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    assert!(
        context
            .render(&state, None)
            .unwrap()
            .contains("\"context_enabled\": false")
    );
}

#[test]
fn explicit_inputs_keep_identical_accepted_occasions_distinct() {
    let context = controller(12288);
    let mut state = AgentState::default();
    let first = context.record_input(&mut state, "continue").unwrap();
    let second = context.record_input(&mut state, "continue").unwrap();
    assert_eq!(first.digest, second.digest);
    assert!(second.revision > first.revision);
    assert_eq!(context.input_stamp(&state).unwrap(), Some(second));
    let data = payload(&context.compile(&state).unwrap().unwrap());
    assert_eq!(data["initial_request"]["text"], "continue");
    assert_eq!(data["recent_user_inputs"][0]["text"], "continue");
}

#[test]
fn generic_user_growth_does_not_promote_uncommitted_input() {
    let context = controller(12288);
    let mut state = AgentState::default();
    state.messages.push(Msg::user("accepted but unsaved"));
    state.messages.push(Msg::user("peer notification"));
    state.messages.push(Msg::user(
        "<conversation_summary>generated</conversation_summary>",
    ));
    let data = payload(&context.compile(&state).unwrap().unwrap());
    assert!(data["initial_request"].is_null());
    assert_eq!(data["recent_user_inputs"], json!([]));
    assert!(context.input_stamp(&state).unwrap().is_none());
    let stamp = context
        .record_input(&mut state, "accepted but unsaved")
        .unwrap();
    assert_eq!(context.input_stamp(&state).unwrap(), Some(stamp));
}

#[test]
fn a_user_typed_summary_prefix_remains_a_real_explicit_input() {
    let context = controller(12288);
    let mut state = AgentState::default();
    let input = "<conversation_summary>this is a user request</conversation_summary>";
    context.record_input(&mut state, input).unwrap();
    assert_eq!(
        payload(&context.compile(&state).unwrap().unwrap())["initial_request"]["text"],
        input
    );
}

#[test]
fn large_inputs_are_wholly_omitted_with_original_size_and_digest() {
    let context = controller(12288);
    let mut state = AgentState::default();
    let input = "模型".repeat(1000);
    context.record_input(&mut state, &input).unwrap();
    let data = payload(&context.compile(&state).unwrap().unwrap());
    assert!(data["initial_request"]["text"].is_null());
    assert_eq!(data["initial_request"]["original_bytes"], input.len());
    assert_eq!(
        data["initial_request"]["digest"],
        state::digest(input.as_bytes())
    );
    assert_eq!(data["initial_request"]["omission"], "too_large");
    assert!(!serde_json::to_string(&data).unwrap().contains("模型"));
}

#[test]
fn observed_input_hash_budget_accepts_exact_limit_and_refuses_larger_before_state_changes() {
    let context = controller(12288);
    let exact = "x".repeat(MAX_OBSERVED_INPUT_BYTES);
    let stamp = InputStamp::for_input(1, &exact).unwrap();
    let expected: [u8; 32] = Sha256::digest(exact.as_bytes()).into();
    assert_eq!(stamp.digest, expected);
    let mut state = AgentState::default();
    context.record_input(&mut state, &exact).unwrap();
    let record = state::Stored::load(&state, &context.scope).unwrap();
    let initial = record.initial_input.unwrap();
    assert_eq!(initial.original_bytes, MAX_OBSERVED_INPUT_BYTES as u64);
    assert!(initial.text.is_none());
    assert_eq!(initial.omission, Some(state::InputOmission::TooLarge));
    let too_large = "p".repeat(MAX_OBSERVED_INPUT_BYTES + 1);
    let before = state.custom[state::KEY].clone();
    assert!(
        InputStamp::for_input(2, &too_large)
            .unwrap_err()
            .to_string()
            .contains(STATE_ERROR)
    );
    assert!(
        context
            .record_input(&mut state, &too_large)
            .unwrap_err()
            .to_string()
            .contains(STATE_ERROR)
    );
    assert_eq!(state.custom[state::KEY], before);
    let mut fresh = state::Stored::fresh(&context.scope);
    assert!(fresh.input(&too_large).is_err());
    assert_eq!(fresh.sequence, 0);
    let disabled = ContextController::new(ContextOptions::default(), workspace()).unwrap();
    assert!(
        disabled
            .record_input(&mut AgentState::default(), &too_large)
            .is_err()
    );
    assert_eq!(disabled.runtime.lock().unwrap().passive_input_sequence, 0);
}

#[tokio::test]
async fn oversized_context_observation_leaves_actual_framework_input_checkpointed_without_sampling()
{
    let context = controller(12288);
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("must not sample")]));
    let checkpoint = Arc::new(craftsman_harness::testing::MemCheckpointer::default());
    let agent = Agent::builder(model.clone())
        .thread_id("oversized-context")
        .checkpointer(checkpoint.clone())
        .durable_turn_checkpoints(true)
        .middleware(Arc::new(Inputs(context)))
        .build();
    let input = "original input ".repeat(MAX_OBSERVED_INPUT_BYTES / 15 + 1);
    assert!(input.len() > MAX_OBSERVED_INPUT_BYTES);
    assert!(
        agent
            .run_turn(input.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains(STATE_ERROR)
    );
    assert_eq!(model.requests.lock().unwrap().len(), 0);
    assert!(
        matches!(&agent.state.lock().unwrap().messages[0],Msg::User { content } if content==&input)
    );
    let (_, saved, _) = checkpoint
        .load_latest("oversized-context")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(&saved.messages[0],Msg::User { content } if content==&input));
    assert!(!saved.custom.contains_key(state::KEY));
}

#[test]
fn update_budget_retains_whole_recent_inputs_and_counts_evictions() {
    let context = controller(12288);
    let mut state = AgentState::default();
    context.record_input(&mut state, "initial task").unwrap();
    for index in 0..8 {
        context
            .record_input(&mut state, &format!("update-{index}").repeat(300))
            .unwrap();
    }
    let record = state::Stored::load(&state, &context.scope).unwrap();
    assert!(record.updates.len() <= state::MAX_UPDATES);
    assert!(
        record
            .updates
            .iter()
            .filter_map(|input| input.text.as_ref())
            .map(String::len)
            .sum::<usize>()
            <= state::MAX_UPDATE_BYTES
    );
    assert!(record.omissions.input_events > 0);
    for input in record.updates {
        assert_eq!(input.text.unwrap().len() as u64, input.original_bytes);
    }
}

#[test]
fn stored_state_round_trips_without_touching_original_or_native_records() {
    let context = controller(12288);
    let mut state = AgentState::default();
    state.messages = vec![
        Msg::user("original"),
        Msg::assistant("real response"),
        Msg::tool("id", "real result"),
    ];
    state.custom.insert(
        "_craftsman_native_model_history".into(),
        json!({"opaque":"SIGNED ORIGINAL"}),
    );
    state
        .custom
        .insert("unrelated".into(), json!({"retained":[1,2]}));
    let messages = state.messages.clone();
    let native = state.custom["_craftsman_native_model_history"].clone();
    context.record_input(&mut state, "original").unwrap();
    context.append_context(&mut state).unwrap();
    let decoded: AgentState = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(decoded.messages, messages);
    assert_eq!(decoded.custom["_craftsman_native_model_history"], native);
    assert_eq!(decoded.custom["unrelated"], json!({"retained":[1,2]}));
    assert_eq!(
        context.input_stamp(&decoded).unwrap(),
        context.input_stamp(&state).unwrap()
    );
}

#[test]
fn malformed_version_unknown_fields_and_digest_collisions_fail_fixed() {
    let context = controller(12288);
    let mut source = AgentState::default();
    context.record_input(&mut source, "task").unwrap();
    let valid = source.custom[state::KEY].clone();
    let mut variants = vec![json!("private-body"), json!({"version":2})];
    let mut unknown = valid.clone();
    unknown["private-field"] = json!("private-body");
    variants.push(unknown);
    let mut digest = valid.clone();
    digest["initial_input"]["digest"] = json!("0".repeat(64));
    variants.push(digest);
    let mut future = valid.clone();
    future["last_input"]["revision"] = json!(99);
    variants.push(future);
    for invalid in variants {
        let mut state = AgentState::default();
        state.custom.insert(state::KEY.into(), invalid.clone());
        let error = context.compile(&state).unwrap_err().to_string();
        assert!(error.contains(STATE_ERROR) && !error.contains("private-body"));
        assert_eq!(state.custom[state::KEY], invalid);
    }
}

#[test]
fn sequence_overflow_refuses_new_input_without_replacing_saved_record() {
    let context = controller(12288);
    let mut state = AgentState::default();
    context.record_input(&mut state, "original").unwrap();
    state.custom[state::KEY]["sequence"] = json!(u64::MAX);
    let before = state.custom[state::KEY].clone();
    assert!(context.record_input(&mut state, "new input").is_err());
    assert_eq!(state.custom[state::KEY], before);
}

#[test]
fn deep_and_oversized_records_fail_before_copying_unbounded_data() {
    let context = controller(12288);
    for invalid in [
        json!({"private":"x".repeat(100_000)}),
        json!({"version":1,"files":vec![json!({});33]}),
    ] {
        let mut state = AgentState::default();
        state.custom.insert(state::KEY.into(), invalid);
        assert!(context.compile(&state).is_err());
    }
}

#[test]
fn read_then_completed_write_marks_only_the_known_old_read_stale() {
    let context = controller(12288);
    let mut state = AgentState::default();
    context
        .observe_tool(
            &mut state,
            &call("read_file", "a"),
            &ToolOutcome::ok("     1\told"),
        )
        .unwrap();
    context
        .observe_tool(
            &mut state,
            &call("read_file", "b"),
            &ToolOutcome::ok("     1\tother"),
        )
        .unwrap();
    context
        .observe_tool(
            &mut state,
            &call("edit_file", "a"),
            &ToolOutcome::ok("real completion"),
        )
        .unwrap();
    let record = state::Stored::load(&state, &context.scope).unwrap();
    assert_eq!(
        record
            .files
            .iter()
            .find(|file| file.selector == "a")
            .unwrap()
            .freshness,
        state::Freshness::Stale
    );
    assert_eq!(
        record
            .files
            .iter()
            .find(|file| file.selector == "b")
            .unwrap()
            .freshness,
        state::Freshness::Uncertain
    );
    assert_eq!(
        record
            .files
            .iter()
            .find(|file| file.selector == "a")
            .unwrap()
            .read
            .as_ref()
            .unwrap()
            .result_digest,
        state::digest(b"     1\told")
    );
}

#[test]
fn denied_and_approval_required_results_do_not_claim_execution_or_change_context() {
    let context = controller(12288);
    let mut state = AgentState::default();
    context.record_input(&mut state, "task").unwrap();
    context
        .observe_tool(
            &mut state,
            &call("read_file", "a"),
            &ToolOutcome::ok("read"),
        )
        .unwrap();
    let before = state.custom[state::KEY].clone();
    for kind in [
        ToolErrorKind::NeedsApproval,
        ToolErrorKind::DeniedCapability,
    ] {
        context
            .observe_tool(&mut state, &call("edit_file", "a"), &control_outcome(kind))
            .unwrap();
        assert_eq!(state.custom[state::KEY], before);
    }
}

#[test]
fn failed_or_cancelled_mutations_make_read_observations_uncertain() {
    let context = controller(12288);
    for outcome in [
        ToolOutcome::err("partial failure"),
        control_outcome(ToolErrorKind::Cancelled),
    ] {
        let mut state = AgentState::default();
        context
            .observe_tool(
                &mut state,
                &call("read_file", "a"),
                &ToolOutcome::ok("read"),
            )
            .unwrap();
        context
            .observe_tool(&mut state, &call("edit_file", "a"), &outcome)
            .unwrap();
        let record = state::Stored::load(&state, &context.scope).unwrap();
        assert_eq!(record.files[0].freshness, state::Freshness::Uncertain);
        assert!(!record.files[0].completed);
    }
}

#[test]
fn unknown_effect_calls_do_not_parse_success_text_into_file_freshness() {
    let context = controller(12288);
    let mut state = AgentState::default();
    context
        .observe_tool(
            &mut state,
            &call("read_file", "a"),
            &ToolOutcome::ok("read"),
        )
        .unwrap();
    context
        .observe_tool(
            &mut state,
            &call("execute", "unused"),
            &ToolOutcome::ok("no files changed; all tests passed"),
        )
        .unwrap();
    assert_eq!(
        state::Stored::load(&state, &context.scope).unwrap().files[0].freshness,
        state::Freshness::Uncertain
    );
}

#[test]
fn actual_runtime_activity_invalidates_reads_without_claiming_compaction() {
    let context = controller(12288);
    let mut state = AgentState::default();
    context.record_input(&mut state, "task").unwrap();
    context
        .observe_tool(
            &mut state,
            &call("read_file", "a"),
            &ToolOutcome::ok("page"),
        )
        .unwrap();
    let input = context.input_stamp(&state).unwrap();
    context.observe_runtime_activity(&mut state).unwrap();
    let record = state::Stored::load(&state, &context.scope).unwrap();
    assert_eq!(record.files[0].freshness, state::Freshness::Uncertain);
    assert_eq!((record.compactions, record.uncertain_compactions), (0, 0));
    assert_eq!(context.input_stamp(&state).unwrap(), input);
    let disabled = ContextController::new(ContextOptions::default(), workspace()).unwrap();
    let before = state.custom.clone();
    disabled.observe_runtime_activity(&mut state).unwrap();
    assert_eq!(state.custom, before);
}

#[test]
fn read_pages_and_large_outputs_never_claim_a_whole_file_hash() {
    let context = controller(12288);
    let mut state = AgentState::default();
    let mut read = call("read_file", "a");
    read.args = json!({"file_path":"a","offset":10,"limit":2});
    context
        .observe_tool(&mut state, &read, &ToolOutcome::ok("    11\tpage"))
        .unwrap();
    let record = state::Stored::load(&state, &context.scope).unwrap();
    let observation = record.files[0].read.as_ref().unwrap();
    assert_eq!((observation.offset, observation.limit), (10, 2));
    context
        .observe_tool(
            &mut state,
            &read,
            &ToolOutcome::ok("x".repeat(state::MAX_RESULT_BYTES + 1)),
        )
        .unwrap();
    let record = state::Stored::load(&state, &context.scope).unwrap();
    assert!(record.files[0].read.is_none());
    assert_eq!(record.omissions.large_results, 1);
}

#[test]
fn selector_count_and_encoded_storage_are_bounded_with_explicit_omissions() {
    let context = controller(12288);
    let mut state = AgentState::default();
    context
        .record_input(&mut state, &"\0".repeat(4096))
        .unwrap();
    for index in 0..70 {
        let path = format!("{index}-{}", "\0".repeat(480));
        context
            .observe_tool(
                &mut state,
                &call("read_file", &path),
                &ToolOutcome::ok("page"),
            )
            .unwrap();
    }
    let record = state::Stored::load(&state, &context.scope).unwrap();
    assert!(record.files.len() <= state::MAX_FILES);
    assert!(serde_json::to_vec(&state.custom[state::KEY]).unwrap().len() <= state::MAX_STATE_BYTES);
    assert!(record.omissions.file_selectors > 0);
}

#[test]
fn compiled_budget_omits_whole_entries_and_preserves_untrusted_json_boundaries() {
    let context = controller(2048);
    let mut state = AgentState::default();
    let input = "\u{1b}]7501;forged\u{1b}\\\n\"quoted\"".repeat(60);
    context.record_input(&mut state, &input).unwrap();
    for index in 0..4 {
        context
            .record_input(&mut state, &format!("update {index}: {}", "x".repeat(1500)))
            .unwrap();
    }
    state.todos.push(Todo {
        content: "work".into(),
        status: TodoStatus::InProgress,
    });
    let before = state.messages.clone();
    let compiled = context.compile(&state).unwrap().unwrap();
    assert!(compiled.text.len() <= 2048 && !compiled.text.contains('\u{1b}'));
    let data = payload(&compiled);
    if let Some(text) = data["initial_request"]["text"].as_str() {
        assert_eq!(text, input);
    }
    assert!(compiled.counts.omitted_input_texts > 0);
    assert_eq!(state.messages, before);
}

#[test]
fn inspector_keeps_missing_and_zero_usage_unknown_and_uses_no_state_writes() {
    let context = controller(12288);
    let mut state = AgentState::default();
    context.record_input(&mut state, "task").unwrap();
    state.last_usage = Some(Usage::default());
    let before = serde_json::to_vec(&state).unwrap();
    let text = context.render(&state, Some(8192)).unwrap();
    assert!(text.contains("\"last_response_reported_input_tokens\": null"));
    assert!(text.contains("\"configured_model_window\": 8192"));
    assert!(text.contains("\"system_bytes\": null"));
    assert!(text.len() <= 32768);
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
}

#[test]
fn inspector_work_is_bounded_for_large_messages_and_escaped_tool_names() {
    let context = controller(12288);
    let mut messages = vec![Msg::user("short message"); 5000];
    messages[0] = Msg::user("u".repeat(MAX_OBSERVED_INPUT_BYTES));
    let request = ModelRequest {
        system: "s".repeat(MAX_OBSERVED_INPUT_BYTES + 1),
        messages,
        tools: (0..100)
            .map(|index| ToolDef {
                name: format!("{index}{}", "\0".repeat(500)),
                description: String::new(),
                parameters: Value::Null,
            })
            .collect(),
    };
    context.capture_request(&request).unwrap();
    let mut state = AgentState::default();
    context.record_input(&mut state, "task").unwrap();
    let text = context.render(&state, None).unwrap();
    assert!(text.len() <= 32768 && !text.contains('\0'));
    let data: Value = serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
    assert_eq!(data["last_prepared_request"]["messages"], 5000);
    assert_eq!(data["last_prepared_request"]["sampled_messages"], 4096);
    assert!(
        data["last_prepared_request"]["omitted_tool_names"]
            .as_u64()
            .unwrap()
            > 36
    );
}

#[test]
fn rebind_and_clear_preserve_real_messages_and_unrelated_custom_data() {
    let context = controller(12288);
    let mut state = AgentState::default();
    state.messages = vec![Msg::user("original"), Msg::tool("id", "completed effects")];
    state
        .custom
        .insert("unrelated".into(), json!({"kept":true}));
    context.record_input(&mut state, "original").unwrap();
    context
        .observe_tool(
            &mut state,
            &call("read_file", "a"),
            &ToolOutcome::ok("page"),
        )
        .unwrap();
    let before = state.messages.clone();
    let stamp = context.input_stamp(&state).unwrap();
    context.rebind(&mut state).unwrap();
    assert_eq!(context.input_stamp(&state).unwrap(), stamp);
    assert_eq!(
        state::Stored::load(&state, &context.scope).unwrap().files[0].freshness,
        state::Freshness::Uncertain
    );
    context.clear(&mut state).unwrap();
    assert_eq!(state.messages, before);
    assert_eq!(state.custom["unrelated"], json!({"kept":true}));
    assert!(!state.custom.contains_key(state::KEY));
}

#[test]
fn workspace_change_keeps_input_but_invalidates_historical_file_observations() {
    let first = controller(12288);
    let mut state = AgentState::default();
    first.record_input(&mut state, "original").unwrap();
    first
        .observe_tool(
            &mut state,
            &call("read_file", "a"),
            &ToolOutcome::ok("page"),
        )
        .unwrap();
    let mut record = state::Stored::load(&state, &first.scope).unwrap();
    record.scope = state::digest(b"different captured workspace");
    record.store(&mut state).unwrap();
    first.rebind(&mut state).unwrap();
    let record = state::Stored::load(&state, &first.scope).unwrap();
    assert_eq!(record.initial_input.unwrap().text.unwrap(), "original");
    assert_eq!(record.scope, first.scope);
    assert_eq!(record.files[0].freshness, state::Freshness::Uncertain);
}

#[tokio::test]
async fn wrapped_compaction_calls_only_the_original_summary_and_retains_original_input_pin() {
    let context = controller(12288);
    let summary = Arc::new(ScriptedModel::new(vec![Msg::assistant(
        "actual model summary",
    )]));
    let original = Arc::new(CompactionMiddleware::new(
        summary.clone(),
        CompactionTrigger::MessageCount {
            threshold_msgs: 4,
            keep_tail_msgs: 2,
        },
    ));
    let changes = Arc::new(AtomicUsize::new(0));
    let count = changes.clone();
    let wrapped = context.wrap_compaction_with(
        original,
        Some(Arc::new(move |transition| {
            if transition == CompactionObservation::Changed {
                count.fetch_add(1, Ordering::SeqCst);
            }
        })),
    );
    let mut state = AgentState::default();
    state.messages = (0..4)
        .flat_map(|index| {
            [
                Msg::user(format!("request {index}")),
                Msg::assistant(format!("result {index}")),
            ]
        })
        .collect();
    context.record_input(&mut state, "request 0").unwrap();
    wrapped.prepare_model(&mut state).await.unwrap();
    assert_eq!(summary.requests.lock().unwrap().len(), 1);
    assert_eq!(changes.load(Ordering::SeqCst), 1);
    assert_eq!(state.messages.len(), 3);
    assert!(
        matches!(&state.messages[0],Msg::User { content } if content.contains("actual model summary"))
    );
    let data = payload(&context.compile(&state).unwrap().unwrap());
    assert_eq!(data["initial_request"]["text"], "request 0");
    assert_eq!(data["recent_user_inputs"], json!([]));
    assert_eq!(data["known_compactions"], 1);
}

#[tokio::test]
async fn governor_only_compaction_observes_real_change_without_context_state_or_extras() {
    let context = Arc::new(ContextController::new(ContextOptions::default(), workspace()).unwrap());
    let summary = Arc::new(ScriptedModel::new(vec![Msg::assistant("actual summary")]));
    let original = Arc::new(CompactionMiddleware::new(
        summary.clone(),
        CompactionTrigger::MessageCount {
            threshold_msgs: 4,
            keep_tail_msgs: 2,
        },
    ));
    let changes = Arc::new(AtomicUsize::new(0));
    let count = changes.clone();
    let wrapped = context.wrap_compaction_with(
        original,
        Some(Arc::new(move |transition| {
            if transition == CompactionObservation::Changed {
                count.fetch_add(1, Ordering::SeqCst);
            }
        })),
    );
    let mut state = AgentState::default();
    state.messages = (0..4)
        .flat_map(|index| {
            [
                Msg::user(format!("request {index}")),
                Msg::assistant(format!("result {index}")),
            ]
        })
        .collect();
    state
        .custom
        .insert(state::KEY.into(), json!("unused malformed context"));
    let before = state.custom.clone();
    wrapped.prepare_model(&mut state).await.unwrap();
    assert_eq!(changes.load(Ordering::SeqCst), 1);
    assert_eq!(summary.requests.lock().unwrap().len(), 1);
    assert_eq!(state.custom, before);
    assert!(state.system_extras.is_empty());
}

#[tokio::test]
async fn compilation_without_a_compaction_profile_keeps_original_messages() {
    let context = controller(12288);
    let mut state = AgentState::default();
    state.messages = vec![
        Msg::user("actual task"),
        Msg::assistant("real prior answer"),
    ];
    context.record_input(&mut state, "actual task").unwrap();
    let before = state.messages.clone();
    context.compiler().prepare_model(&mut state).await.unwrap();
    assert_eq!(state.messages, before);
    assert_eq!(state.system_extras.len(), 1);
    assert!(state.system_extras[0].contains("actual task"));
    assert_eq!(
        state::Stored::load(&state, &context.scope)
            .unwrap()
            .compactions,
        0
    );
}

#[test]
fn confirmed_manual_transition_and_unavailable_anchor_are_distinct() {
    let context = controller(12288);
    let mut state = AgentState::default();
    context.record_input(&mut state, "task").unwrap();
    context
        .observe_compaction(&mut state, CompactionObservation::Changed)
        .unwrap();
    context
        .observe_compaction(&mut state, CompactionObservation::Unknown)
        .unwrap();
    let record = state::Stored::load(&state, &context.scope).unwrap();
    assert_eq!((record.compactions, record.uncertain_compactions), (1, 1));
    state.messages = vec![Msg::user("x".repeat(MAX_ANCHOR_BYTES + 1))];
    let before = context.begin_compaction(&state).unwrap();
    assert_eq!(
        context.finish_compaction(&mut state, &before).unwrap(),
        CompactionObservation::Unknown
    );
}

#[test]
fn anchor_preflight_rejects_large_keys_and_deep_nodes_and_keeps_missing_coverage_unknown() {
    let mut state = AgentState::default();
    state.messages = vec![Msg::user("small original")];
    let complete = TranscriptAnchor::capture(&state);
    assert!(complete.digest.is_some());
    let mut map = serde_json::Map::new();
    map.insert("k".repeat(MAX_ANCHOR_KEY_BYTES + 1), json!("value"));
    let mut tool = call("fixture", "a");
    tool.args = Value::Object(map);
    state.messages = vec![Msg::Assistant {
        content: String::new(),
        tool_calls: vec![tool],
    }];
    let unavailable = TranscriptAnchor::capture(&state);
    assert!(unavailable.digest.is_none());
    assert_eq!(
        complete.compare(&unavailable),
        CompactionObservation::Unknown
    );
    let mut value = json!("leaf");
    for _ in 0..MAX_ANCHOR_DEPTH + 2 {
        value = json!({"nested":value});
    }
    let mut tool = call("fixture", "a");
    tool.args = value;
    state.messages = vec![Msg::Assistant {
        content: String::new(),
        tool_calls: vec![tool],
    }];
    assert!(TranscriptAnchor::capture(&state).digest.is_none());
    state
        .messages
        .push(Msg::assistant("different actual count"));
    assert_eq!(
        unavailable.compare(&TranscriptAnchor::capture(&state)),
        CompactionObservation::Changed
    );
}

struct Inputs(Arc<ContextController>);
#[async_trait]
impl AgentMiddleware for Inputs {
    fn name(&self) -> &str {
        "actual-context-input-fixture"
    }
    async fn turn_start(&self, state: &mut AgentState) -> Result<(), AgentError> {
        let Some(Msg::User { content }) = state.messages.last() else {
            return Err(context_error());
        };
        if content.len() > MAX_OBSERVED_INPUT_BYTES {
            return Err(context_error());
        }
        let input = content.clone();
        self.0.record_input(state, &input)?;
        Ok(())
    }
}

struct ResultTool {
    name: &'static str,
    value: ToolOutcome,
    calls: Arc<AtomicUsize>,
    corrupt: bool,
}
#[async_trait]
impl Tool for ResultTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name.into(),
            description: "actual fixture result".into(),
            parameters: json!({"type":"object"}),
        }
    }
    async fn call(&self, _: Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.corrupt {
            ctx.state
                .custom
                .insert(state::KEY.into(), json!("private-collision"));
        }
        Ok(self.value.clone())
    }
}

fn assistant_call(name: &str) -> Msg {
    Msg::Assistant {
        content: "actual assistant".into(),
        tool_calls: vec![call(name, "a")],
    }
}

#[tokio::test]
async fn actual_agent_loop_preserves_calls_results_and_request_message_groups() {
    let context = controller(12288);
    let model = Arc::new(ScriptedModel::new(vec![
        assistant_call("read_file"),
        assistant_call("edit_file"),
        Msg::assistant("done"),
    ]));
    let summaries = Arc::new(ScriptedModel::new(vec![]));
    let compaction = Arc::new(CompactionMiddleware::new(
        summaries.clone(),
        CompactionTrigger::MessageCount {
            threshold_msgs: 100,
            keep_tail_msgs: 10,
        },
    ));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut metadata = ToolMeta::new();
    metadata.exit_code = Some(0);
    metadata.bytes = Some(12);
    let agent = Agent::builder(model.clone())
        .system_prompt("stable system prefix")
        .middleware(Arc::new(Inputs(context.clone())))
        .middleware(context.wrap_compaction(compaction))
        .middleware(context.observer())
        .middleware(context.request_observer())
        .tool(Arc::new(ResultTool {
            name: "read_file",
            value: ToolOutcome::ok("actual read"),
            calls: calls.clone(),
            corrupt: false,
        }))
        .tool(Arc::new(ResultTool {
            name: "edit_file",
            value: ToolOutcome::ok("actual write").with_meta(metadata),
            calls: calls.clone(),
            corrupt: false,
        }))
        .build();
    agent.run_turn("actual initial task").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(model.requests.lock().unwrap().len(), 3);
    assert_eq!(summaries.requests.lock().unwrap().len(), 0);
    let state = agent.state.lock().unwrap();
    let results: Vec<_> = state
        .messages
        .iter()
        .filter_map(|message| {
            if let Msg::Tool { content, .. } = message {
                Some(content.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(results, vec!["actual read", "actual write"]);
    assert_eq!(state.messages.len(), 6);
    let requests = model.requests.lock().unwrap();
    assert!(requests[0].system.starts_with("stable system prefix"));
    assert_eq!(requests[0].messages, vec![Msg::user("actual initial task")]);
    assert!(requests[2].system.contains("\"freshness\":\"stale\""));
    assert_eq!(requests[2].messages.len(), 5);
}

#[tokio::test]
async fn overridden_file_tool_identity_is_unknown_instead_of_a_trusted_read() {
    let context = controller(12288);
    let mut initial = AgentState::default();
    context
        .observe_tool(
            &mut initial,
            &call("read_file", "a"),
            &ToolOutcome::ok("trusted observed page"),
        )
        .unwrap();
    let model = Arc::new(ScriptedModel::new(vec![
        assistant_call("read_file"),
        Msg::assistant("done"),
    ]));
    let calls = Arc::new(AtomicUsize::new(0));
    let state = Arc::new(Mutex::new(initial));
    let agent = Agent::builder(model.clone())
        .state(state.clone())
        .middleware(context.observer_with_trust(Arc::new(|_| false)))
        .tool(Arc::new(ResultTool {
            name: "read_file",
            value: ToolOutcome::ok("actual overridden result"),
            calls: calls.clone(),
            corrupt: false,
        }))
        .build();
    agent.run_turn("task").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    let state = state.lock().unwrap();
    let record = state::Stored::load(&state, &context.scope).unwrap();
    assert_eq!(record.files[0].freshness, state::Freshness::Uncertain);
    assert_eq!(
        record.files[0].read.as_ref().unwrap().result_digest,
        state::digest(b"trusted observed page")
    );
    assert!(state.messages.iter().any(
        |message| matches!(message,Msg::Tool { content,.. } if content=="actual overridden result")
    ));
}

#[tokio::test]
async fn post_tool_context_collision_keeps_the_actual_result_and_stops_more_sampling() {
    let context = controller(12288);
    let model = Arc::new(ScriptedModel::new(vec![
        assistant_call("fixture"),
        Msg::assistant("must not sample"),
    ]));
    let compaction = Arc::new(CompactionMiddleware::new(
        Arc::new(ScriptedModel::new(vec![])),
        CompactionTrigger::MessageCount {
            threshold_msgs: 100,
            keep_tail_msgs: 10,
        },
    ));
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model.clone())
        .middleware(Arc::new(Inputs(context.clone())))
        .middleware(context.wrap_compaction(compaction))
        .middleware(context.observer())
        .tool(Arc::new(ResultTool {
            name: "fixture",
            value: ToolOutcome::ok("completed actual effects"),
            calls: calls.clone(),
            corrupt: true,
        }))
        .build();
    let error = agent.run_turn("task").await.unwrap_err().to_string();
    assert!(error.contains(STATE_ERROR) && !error.contains("private-collision"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert!(agent.state.lock().unwrap().messages.iter().any(
        |message| matches!(message,Msg::Tool { content,.. } if content=="completed actual effects")
    ));
}

#[tokio::test]
async fn disabled_actual_loop_keeps_default_requests_and_context_key_untouched() {
    let context = Arc::new(ContextController::new(ContextOptions::default(), workspace()).unwrap());
    let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("done")]));
    let compaction = Arc::new(CompactionMiddleware::new(
        Arc::new(ScriptedModel::new(vec![])),
        CompactionTrigger::MessageCount {
            threshold_msgs: 100,
            keep_tail_msgs: 10,
        },
    ));
    let agent = Agent::builder(model.clone())
        .system_prompt("original default")
        .middleware(context.wrap_compaction(compaction))
        .middleware(context.observer())
        .middleware(context.request_observer())
        .build();
    agent.run_turn("original input").await.unwrap();
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].system, "original default");
    assert_eq!(requests[0].messages, vec![Msg::user("original input")]);
    assert!(!agent.state.lock().unwrap().custom.contains_key(state::KEY));
}

#[test]
fn sources_for_a_source_view_are_kept_in_memory_within_their_bounds() {
    let controller = controller(4096);
    let stamp = |revision| InputStamp::for_input(revision, "typed").unwrap();
    let source = |value: u64| SourceRef::new(Arc::new(value));
    // The first input and the latest ones are kept; the others go.
    let first = source(1);
    controller.retain_input_source(stamp(1), first.clone());
    let mut last = first.clone();
    for revision in 2..=40 {
        last = source(revision);
        controller.retain_input_source(stamp(revision), last.clone());
    }
    let kept = controller.retained_input_sources();
    assert_eq!(kept.len(), MAX_RETAINED_INPUTS);
    assert!(kept[0].is(&first) && kept.last().unwrap().is(&last));
    {
        let runtime = controller.runtime.lock().unwrap();
        let events: Vec<u64> = runtime.inputs.iter().map(|kept| kept.event).collect();
        assert_eq!(events[0], 1);
        assert_eq!(
            events[1..],
            (40 - MAX_RETAINED_INPUTS as u64 + 2..=40).collect::<Vec<_>>()
        );
    }

    // A path is kept for a file tool's call with an id and a bounded path:
    // the latest call per path, and a bounded number of paths.
    let named = |id: &str, name: &str, path: &str| ToolCall {
        id: id.into(),
        name: name.into(),
        args: json!({"file_path": path}),
    };
    controller.retain_path_source(&named("a", "read_file", "one.txt"));
    controller.retain_path_source(&named("b", "edit_file", "one.txt"));
    controller.retain_path_source(&named("", "read_file", "no-id.txt"));
    controller.retain_path_source(&named("c", "grep", "not-a-file-tool.txt"));
    controller.retain_path_source(&named("d", "write_file", ""));
    controller.retain_path_source(&named(&"i".repeat(513), "read_file", "long-id.txt"));
    controller.retain_path_source(&named("e", "read_file", &"p".repeat(513)));
    controller.retain_path_source(&ToolCall {
        id: "f".into(),
        name: "read_file".into(),
        args: json!({"file_path": 7}),
    });
    {
        let runtime = controller.runtime.lock().unwrap();
        assert_eq!(runtime.paths.len(), 1);
        assert_eq!(runtime.paths[0].selector, "one.txt");
        assert_eq!(runtime.paths[0].call_id, "b");
    }
    for index in 0..200 {
        controller.retain_path_source(&named("g", "read_file", &format!("file-{index}.txt")));
    }
    {
        let runtime = controller.runtime.lock().unwrap();
        assert_eq!(runtime.paths.len(), MAX_RETAINED_PATHS);
        assert_eq!(runtime.paths.last().unwrap().selector, "file-199.txt");
        assert!(runtime.paths.iter().all(|kept| kept.selector != "one.txt"));
    }

    // A fresh conversation starts without any of them, and a controller
    // that is not enabled keeps none.
    let mut state = AgentState::default();
    controller.clear(&mut state).unwrap();
    assert!(controller.retained_input_sources().is_empty());
    assert!(controller.runtime.lock().unwrap().paths.is_empty());
    let disabled = ContextController::new(ContextOptions::default(), workspace()).unwrap();
    disabled.retain_input_source(stamp(1), source(1));
    disabled.retain_path_source(&named("a", "read_file", "one.txt"));
    assert!(disabled.retained_input_sources().is_empty());
    assert!(disabled.runtime.lock().unwrap().paths.is_empty());
}
