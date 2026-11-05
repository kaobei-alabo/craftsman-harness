use super::*;
use crate::exec::input::{OriginKind, Run};
use crate::exec::meter::Role;
use crate::exec::output::{self, Format};
use crate::exec::runtime::RuntimeError;
use craftsman_harness::{Msg, ToolCall};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn argv(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn defaults() -> Defaults {
    #[cfg(unix)]
    let (workspace, config) = ("/explicit-workspace", "/explicit-config");
    #[cfg(windows)]
    let (workspace, config) = (r"C:\explicit-workspace", r"C:\explicit-config");
    Defaults {
        workspace: workspace.into(),
        config_dir: config.into(),
    }
}
fn file_options() -> FileOptions {
    FileOptions {
        format: Format::AsciiFragment,
        max_bytes: output::CAP,
    }
}
#[cfg(unix)]
fn invalid_native() -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(vec![0xff])
}
#[cfg(windows)]
fn invalid_native() -> OsString {
    use std::os::windows::ffi::OsStringExt;
    OsString::from_wide(&[0xd800])
}

#[test]
fn native_overhead_does_not_raise_old_source_or_control_limits() {
    assert_eq!(bounds(), (1432, 1048608, 11598352));
}
#[test]
fn native_count_precedes_invalid_utf8_and_projection() {
    let mut args = vec![OsString::from("x"); bounds().0 + 1];
    args[0] = invalid_native();
    assert_eq!(
        Selected::select(&defaults(), &args).err(),
        Some(SelectionError::Count)
    );
}
#[test]
fn native_token_and_total_bounds_precede_decoding() {
    for mut args in [
        vec![OsString::from("x".repeat(bounds().1 + 1))],
        vec![OsString::from("x".repeat(super::super::INSTRUCTION_BYTES)); 12],
    ] {
        args.push(invalid_native());
        assert_eq!(
            Selected::select(&defaults(), &args).err(),
            Some(SelectionError::Resource)
        );
    }
}
#[test]
fn default_scope_native_bounds_precede_decoding() {
    let mut d = defaults();
    let mut path = invalid_native();
    path.push("x".repeat(super::super::PATH_BYTES + 1));
    d.workspace = path.into();
    assert_eq!(
        Selected::select(&d, &[invalid_native()]).err(),
        Some(SelectionError::Resource)
    );
}
#[test]
fn whole_original_utf8_precedes_extraction_of_a_valid_limit() {
    let mut args = argv(&["--max-rounds=2", "goal"]);
    args.push(invalid_native());
    assert_eq!(
        Selected::select(&defaults(), &args).err(),
        Some(SelectionError::Utf8)
    );
}
#[test]
fn default_split_joined_and_boundary_values_select_the_accepted_cap() {
    for (values, expected) in [
        (vec!["goal"], 50),
        (vec!["--max-rounds", "2", "goal"], 2),
        (vec!["--max-rounds=2", "goal"], 2),
        (vec!["goal", "--max-rounds=1"], 1),
        (vec!["--max-rounds=50", "goal"], 50),
        (vec!["--max-rounds=10000", "goal"], 10000),
    ] {
        let selected = Selected::select(&defaults(), &argv(&values)).unwrap();
        assert_eq!(selected.limit.maximum(), expected);
        assert_eq!(selected.arguments, argv(&["goal"]));
    }
}
#[test]
fn duplicate_split_and_joined_limits_refuse_instead_of_last_wins() {
    for values in [
        vec!["--max-rounds=1", "--max-rounds=2", "goal"],
        vec!["--max-rounds", "1", "goal", "--max-rounds", "2"],
        vec!["--max-rounds=1", "--max-rounds", "2", "goal"],
    ] {
        assert_eq!(
            Selected::select(&defaults(), &argv(&values)).err(),
            Some(SelectionError::Duplicate)
        );
    }
}
#[test]
fn malformed_empty_negative_noninteger_overflow_and_out_of_range_limits_refuse() {
    for value in [
        "",
        "0",
        "-1",
        "+2",
        " 2",
        "2 ",
        "2.0",
        "2e1",
        "two",
        "２",
        "10001",
        "18446744073709551616",
        "2\n::warning::secret",
        "2\0",
    ] {
        for args in [
            argv(&[&format!("--max-rounds={value}"), "goal"]),
            argv(&["--max-rounds", value, "goal"]),
        ] {
            let error = Selected::select(&defaults(), &args).err().unwrap();
            assert_eq!(error, SelectionError::Value);
            assert_eq!(error.to_string(), "[REDACTED]");
            assert!(!format!("{error:?}").contains("secret"));
        }
    }
    assert_eq!(
        Selected::select(&defaults(), &argv(&["--max-rounds"])).err(),
        Some(SelectionError::Value)
    );
}
#[test]
fn end_of_options_preserves_literal_budget_text_and_the_default() {
    let original = argv(&["--", "--max-rounds=2"]);
    let selected = Selected::select(&defaults(), &original).unwrap();
    assert_eq!(selected.limit.maximum(), 50);
    assert_eq!(selected.arguments, original);
}
#[test]
fn joined_source_values_are_not_rescanned_as_budget_controls() {
    let original = argv(&["--prompt-file=--max-rounds=2", "--file=--max-rounds=3"]);
    let selected = Selected::select(&defaults(), &original).unwrap();
    assert_eq!(selected.limit.maximum(), 50);
    assert_eq!(selected.arguments, original);
}
#[test]
fn every_old_split_flag_keeps_its_option_looking_value_as_data() {
    for flag in [
        "--cwd",
        "--config-dir",
        "--output-file",
        "--permission-mode",
        "--on-unapproved",
        "--session",
        "--allow",
        "--deny",
        "--env-pass",
        "--prompt-file",
        "--file",
    ] {
        let original = argv(&[flag, "--max-rounds=2", "goal"]);
        let selected = Selected::select(&defaults(), &original).unwrap();
        assert_eq!(selected.limit.maximum(), 50);
        assert_eq!(selected.arguments, original);
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    output: PathBuf,
    server: MockServer,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let workspace = base.join("workspace");
        let config = base.join("config");
        let output = base.join("output");
        for directory in [&workspace, &config, &output] {
            std::fs::create_dir(directory).unwrap();
        }
        let server = MockServer::start().await;
        let fixture = Self {
            _root: root,
            workspace,
            config,
            output,
            server,
        };
        fixture.configure(None);
        std::fs::write(fixture.workspace.join("context.txt"), "ORIGINAL_CONTEXT").unwrap();
        fixture
    }
    fn configure(&self, context_window: Option<u64>) {
        let mut config = json!({
            "model":"synthetic-budget-model", "api_key":"synthetic-budget-key",
            "base_url":self.server.uri(),
        });
        if let Some(window) = context_window {
            config["model_profiles"] = json!({"synthetic-budget-model":{"context_window":window}});
        }
        std::fs::write(self.config.join("config.json"), config.to_string()).unwrap();
    }
    fn defaults(&self) -> Defaults {
        Defaults {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
        }
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
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }
    fn no_store(&self) {
        assert!(!self.config.join("threads.db").exists());
        assert!(!self.workspace.join("threads.db").exists());
    }
    async fn run(&self, values: &[&str]) -> Run {
        let mut args = argv(&["--ephemeral"]);
        args.extend(argv(values));
        Invocation::prepare(self.defaults(), &args)
            .await
            .unwrap()
            .run()
            .await
    }
}
struct Script(Mutex<VecDeque<Msg>>);
impl Respond for Script {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let Some(Msg::Assistant {
            content,
            tool_calls,
        }) = self.0.lock().unwrap().pop_front()
        else {
            return ResponseTemplate::new(400).set_body_string("unexpected synthetic request");
        };
        let calls: Vec<_> = tool_calls
            .iter()
            .enumerate()
            .map(|(index, call)| {
                json!({
                    "index":index,"id":call.id,"type":"function",
                    "function":{"name":call.name,"arguments":call.args.to_string()}
                })
            })
            .collect();
        let stop = if calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(format!(
                "data: {}\n\ndata: {}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":20,\"completion_tokens\":5}}}}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"delta":{"content":content,"tool_calls":calls}}]}),
                json!({"choices":[{"delta":{},"finish_reason":stop}]})
            ))
    }
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
        content: "ORIGINAL_PARTIAL_ANSWER".into(),
        tool_calls: calls,
    }
}
fn read() -> ToolCall {
    call("read", "read_file", json!({"file_path":"context.txt"}))
}
fn write() -> ToolCall {
    call(
        "write",
        "write_file",
        json!({"file_path":"later.txt","content":"must not write"}),
    )
}
fn assert_counts(run: &Run, maximum: u64, main: u64, child: u64, compact: u64) {
    let counts = run.request_counts().unwrap();
    assert_eq!(counts.maximum(), maximum);
    assert_eq!(counts.admitted(), main + child + compact);
    for (role, admitted) in [
        (Role::Main, main),
        (Role::Subagent, child),
        (Role::Compaction, compact),
    ] {
        let row = counts.role(role);
        assert_eq!(row[0], admitted);
        assert_eq!(row[0], row[1..].iter().sum::<u64>());
        assert_eq!(row[1], 0);
    }
    assert!(run.token_reservations().unwrap().is_none());
    assert!(run.admission_receipts().unwrap().is_none());
}
fn grammar_error(error: Error) -> bool {
    matches!(
        error,
        Error::Selection(_) | Error::Preparation(scopes::Error::Selection(_))
    )
}

#[tokio::test]
async fn malformed_and_unsupported_controls_refuse_before_bad_config_input_or_store() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("config.json"), "INVALID_JSON").unwrap();
    for values in [
        vec!["--max-rounds=0", "-"],
        vec!["--max-rounds=2", "--max-requests=2", "-"],
        vec!["--max-rounds=2", "--max-tokens=100", "-"],
        vec!["--max-rounds=2", "--timeout=1", "-"],
        vec!["--max-rounds=2", "--trace=result", "-"],
        vec!["--max-rounds=2", "--format=json", "-"],
        vec!["--max-rounds=2", "--allow-gadget=unknown", "-"],
        vec!["--max-rounds=2", "--allow=tool:unknown", "-"],
        vec!["--max-rounds=2", "--session=bad/id", "-"],
        vec!["--max-rounds=2", "--ephemeral", "--continue", "-"],
        vec!["--max-rounds=2", "--cwd=../escape", "-"],
        vec!["--max-rounds=2", "--prompt-file=missing", "another source"],
        vec!["--max-rounds=2"],
    ] {
        let error = Invocation::prepare(f.defaults(), &argv(&values))
            .await
            .err()
            .unwrap();
        assert!(grammar_error(error), "{error:?}");
        assert_eq!(error.to_string(), "[REDACTED]");
        f.no_store();
        assert!(f.requests().await.is_empty());
    }
}
#[tokio::test]
async fn extracting_a_cap_cannot_repair_an_invalid_original_split_value() {
    let f = Fixture::new().await;
    for flag in [
        "--cwd",
        "--config-dir",
        "--session",
        "--allow",
        "--env-pass",
        "--prompt-file",
        "--file",
    ] {
        let error = Invocation::prepare(
            f.defaults(),
            &argv(&[flag, "--max-rounds=2", "--max-rounds=3", "goal"]),
        )
        .await
        .err()
        .unwrap();
        assert!(grammar_error(error), "{error:?}");
        f.no_store();
        assert!(f.requests().await.is_empty());
    }
}
#[tokio::test]
async fn older_frontend_routes_continue_to_reject_the_new_control() {
    let f = Fixture::new().await;
    let args = argv(&["--max-rounds=2", "--ephemeral", "goal"]);
    assert!(
        super::super::ControlInvocation::prepare(f.defaults(), &args)
            .await
            .is_err()
    );
    assert!(
        super::super::ScopedInvocation::prepare(f.defaults(), &args)
            .await
            .is_err()
    );
    f.no_store();
    assert!(f.requests().await.is_empty());
}
#[tokio::test]
async fn old_instruction_and_attachment_bounds_survive_budget_projection() {
    let f = Fixture::new().await;
    let mut args = argv(&["--max-rounds=2"]);
    args.push(OsString::from(
        "x".repeat(super::super::INSTRUCTION_BYTES + 1),
    ));
    assert!(grammar_error(
        Invocation::prepare(f.defaults(), &args)
            .await
            .err()
            .unwrap()
    ));
    let mut args = argv(&["--max-rounds=2", "goal"]);
    args.extend((0..129).map(|_| OsString::from("--file=context.txt")));
    assert!(grammar_error(
        Invocation::prepare(f.defaults(), &args)
            .await
            .err()
            .unwrap()
    ));
    f.no_store();
    assert!(f.requests().await.is_empty());
}
#[tokio::test]
async fn exact_final_response_can_complete_at_the_selected_cap() {
    let f = Fixture::new().await;
    f.script(vec![Msg::assistant("EXACT_FINAL")]).await;
    let run = f.run(&["--max-rounds=1", "goal"]).await;
    assert_eq!(run.outcome().as_ref().unwrap().final_text(), "EXACT_FINAL");
    assert_counts(&run, 1, 1, 0, 0);
    assert!(run.request_counts().unwrap().refusal().is_none());
    assert_eq!(f.requests().await.len(), 1);
    f.no_store();
}
#[tokio::test]
async fn following_main_request_refuses_without_entering_another_provider() {
    let f = Fixture::new().await;
    f.script(vec![
        tools(vec![read()]),
        Msg::assistant("must not request"),
    ])
    .await;
    let run = f.run(&["--max-rounds", "1", "goal"]).await;
    assert!(matches!(run.outcome(), Err(RuntimeError::RequestLimit)));
    assert_counts(&run, 1, 1, 0, 0);
    assert_eq!(
        run.request_counts().unwrap().refusal(),
        Some((Role::Main, 1, 1))
    );
    assert_eq!(f.requests().await.len(), 1);
    assert!(
        run.retained_state()
            .messages
            .iter()
            .any(|message| matches!(message,
                Msg::Assistant { content, .. } if content == "ORIGINAL_PARTIAL_ANSWER"
            ))
    );
    f.no_store();
}
#[tokio::test]
async fn child_refusal_shares_the_cap_and_prevents_later_parent_tool_dispatch() {
    let f = Fixture::new().await;
    f.script(vec![tools(vec![call("child", "task", json!({
        "subagent_type":"general-purpose","description":"Synthetic child","prompt":"Read context"
    })), write()]), tools(vec![read()]), Msg::assistant("must not request")]).await;
    let run = f
        .run(&["--max-rounds=2", "--allow=tool:write_file", "goal"])
        .await;
    assert!(matches!(run.outcome(), Err(RuntimeError::RequestLimit)));
    assert_counts(&run, 2, 1, 1, 0);
    assert_eq!(
        run.request_counts().unwrap().refusal(),
        Some((Role::Subagent, 2, 2))
    );
    assert_eq!(f.requests().await.len(), 2);
    assert!(!f.workspace.join("later.txt").exists());
}
#[tokio::test]
async fn automatic_compaction_and_main_use_one_selected_actual_builder_meter() {
    let f = Fixture::new().await;
    f.configure(Some(1));
    f.script(vec![
        tools(vec![read()]),
        Msg::assistant("ACTUAL_SUMMARY"),
        Msg::assistant("EXACT_FINAL"),
    ])
    .await;
    let run = f.run(&["--max-rounds=3", "goal"]).await;
    assert_eq!(run.outcome().as_ref().unwrap().final_text(), "EXACT_FINAL");
    assert_counts(&run, 3, 2, 0, 1);
    assert!(run.request_counts().unwrap().refusal().is_none());
    let requests = f.requests().await;
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|request| request["model"] == "synthetic-budget-model")
    );
}
#[tokio::test]
async fn caught_compaction_refusal_cannot_become_completion() {
    let f = Fixture::new().await;
    f.configure(Some(1));
    f.script(vec![
        tools(vec![read()]),
        Msg::assistant("must not request"),
    ])
    .await;
    let run = f.run(&["--max-rounds=1", "goal"]).await;
    assert!(matches!(run.outcome(), Err(RuntimeError::RequestLimit)));
    assert_counts(&run, 1, 1, 0, 0);
    assert_eq!(
        run.request_counts().unwrap().refusal(),
        Some((Role::Compaction, 1, 1))
    );
    assert_eq!(f.requests().await.len(), 1);
}
#[tokio::test]
async fn real_approval_stop_dominates_reaching_the_request_cap() {
    let f = Fixture::new().await;
    f.script(vec![tools(vec![write(), read()])]).await;
    let run = f
        .run(&["--max-rounds=1", "--on-unapproved=fail", "goal"])
        .await;
    assert!(matches!(run.outcome(), Err(RuntimeError::ApprovalRequired)));
    assert_counts(&run, 1, 1, 0, 0);
    assert!(run.request_counts().unwrap().refusal().is_none());
    assert!(!f.workspace.join("later.txt").exists());
    assert_eq!(f.requests().await.len(), 1);
}

#[tokio::test]
async fn original_option_like_file_bytes_labels_and_duplicates_are_preserved() {
    let f = Fixture::new().await;
    std::fs::write(
        f.workspace.join("--max-rounds=2"),
        b"\xef\xbb\xbfORIGINAL\r\nFILE",
    )
    .unwrap();
    f.script(vec![Msg::assistant("done")]).await;
    let run = f
        .run(&[
            "--prompt-file=--max-rounds=2",
            "--file=--max-rounds=2",
            "--file=--max-rounds=2",
        ])
        .await;
    assert!(run.outcome().is_ok());
    assert_counts(&run, 50, 1, 0, 0);
    assert_eq!(run.facts().instruction().path(), Some("--max-rounds=2"));
    assert!(run.facts().instruction().bom_removed());
    assert_eq!(run.facts().attachments().len(), 2);
    assert!(
        run.facts()
            .attachments()
            .iter()
            .all(|source| source.path() == Some("--max-rounds=2"))
    );
    let requests = f.requests().await;
    let input = requests[0]["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(input.starts_with("ORIGINAL\r\nFILE"));
    assert_eq!(input.matches("\u{feff}ORIGINAL\r\nFILE").count(), 2);
    f.no_store();
}
#[tokio::test]
async fn the_selected_cap_and_source_cannot_change_after_preparation() {
    let f = Fixture::new().await;
    std::fs::write(f.workspace.join("goal.txt"), "ORIGINAL_GOAL").unwrap();
    f.script(vec![
        tools(vec![read()]),
        Msg::assistant("must not request"),
    ])
    .await;
    let mut args = argv(&["--ephemeral", "--max-rounds=1", "--prompt-file=goal.txt"]);
    let invocation = Invocation::prepare(f.defaults(), &args).await.unwrap();
    args[1] = "--max-rounds=10000".into();
    args[2] = "--prompt-file=other.txt".into();
    std::fs::write(f.workspace.join("goal.txt"), "CHANGED_GOAL").unwrap();
    let run = invocation.run().await;
    assert!(matches!(run.outcome(), Err(RuntimeError::RequestLimit)));
    assert_counts(&run, 1, 1, 0, 0);
    let requests = f.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["messages"].as_array().unwrap().last().unwrap()["content"],
        "ORIGINAL_GOAL"
    );
}
#[tokio::test]
async fn file_preview_keeps_the_actual_run_limit_and_one_guarded_attempt() {
    let f = Fixture::new().await;
    f.script(vec![tools(vec![read()]), Msg::assistant("EXACT_FINAL")])
        .await;
    let destination = f.output.join("result.out");
    let args = argv(&[
        "--ephemeral",
        "--max-rounds=2",
        "--output-file",
        destination.to_str().unwrap(),
        "goal",
    ]);
    let run = FileInvocation::prepare(f.defaults(), &args, file_options())
        .await
        .unwrap()
        .run()
        .await;
    assert_eq!(
        run.run_for_tests().outcome().as_ref().unwrap().final_text(),
        "EXACT_FINAL"
    );
    assert_counts(run.run_for_tests(), 2, 2, 0, 0);
    let mut terminal = run.terminal();
    terminal.publish().unwrap();
    let original = std::fs::read(&destination).unwrap();
    assert!(
        std::str::from_utf8(&original)
            .unwrap()
            .contains("EXACT_FINAL")
    );
    assert_eq!(
        terminal.publish().err().unwrap().kind(),
        output::Error::AlreadyAttempted
    );
    assert_eq!(std::fs::read(destination).unwrap(), original);
    assert_eq!(f.requests().await.len(), 2);
    f.no_store();
}
#[tokio::test]
async fn budget_control_does_not_enable_public_formats_or_weaken_destination_grammar() {
    let f = Fixture::new().await;
    for destination in [f.workspace.join("result.out"), f.config.join("result.out")] {
        let args = argv(&[
            "--max-rounds=2",
            "--ephemeral",
            "--output-file",
            destination.to_str().unwrap(),
            "-",
        ]);
        assert!(
            FileInvocation::prepare(f.defaults(), &args, file_options())
                .await
                .is_err()
        );
        assert!(!destination.exists());
    }
    let destination = f.output.join("result.out");
    let args = argv(&[
        "--max-rounds=2",
        "--format=json",
        "--output-file",
        destination.to_str().unwrap(),
        "-",
    ]);
    assert!(grammar_error(
        FileInvocation::prepare(f.defaults(), &args, file_options())
            .await
            .err()
            .unwrap()
    ));
    assert!(!destination.exists());
    f.no_store();
    assert!(f.requests().await.is_empty());
}

const HELPER: &str = "exec::input::request_budget::tests::native_request_budget_helper";
const MARKER: &str = "__craftsman_request_budget_fixture__:";
const PAYLOAD: &[u8] = b"ORIGINAL_NATIVE_BUDGET_INPUT\r\n";
fn native_case(arguments: impl Iterator<Item = String>) -> Option<String> {
    let mut cases =
        arguments.filter_map(|argument| argument.strip_prefix(MARKER).map(str::to_owned));
    let case = cases.next()?;
    if cases.next().is_some() || !matches!(case.as_str(), "memory" | "file" | "invalid" | "literal")
    {
        return None;
    }
    Some(case)
}
#[tokio::test]
#[ignore = "explicit owned native-pipe fixture; returns before IO without a registered marker"]
async fn native_request_budget_helper() {
    let Some(case) = native_case(std::env::args()) else {
        return;
    };
    let f = Fixture::new().await;
    match case.as_str() {
        "memory" | "file" => {
            f.script(vec![tools(vec![read()]), Msg::assistant("EXACT_FINAL")])
                .await;
            if case == "memory" {
                let run = f.run(&["--max-rounds=2", "-"]).await;
                assert_eq!(run.outcome().as_ref().unwrap().final_text(), "EXACT_FINAL");
                assert_counts(&run, 2, 2, 0, 0);
                assert_eq!(run.facts().instruction().kind(), OriginKind::Stdin);
                assert_eq!(run.facts().instruction().bytes(), PAYLOAD.len());
            } else {
                let destination = f.output.join("result.out");
                let args = argv(&[
                    "--ephemeral",
                    "--max-rounds",
                    "2",
                    "--output-file",
                    destination.to_str().unwrap(),
                    "-",
                ]);
                let run = FileInvocation::prepare(f.defaults(), &args, file_options())
                    .await
                    .unwrap()
                    .run()
                    .await;
                assert_counts(run.run_for_tests(), 2, 2, 0, 0);
                assert_eq!(
                    run.run_for_tests().facts().instruction().kind(),
                    OriginKind::Stdin
                );
                assert_eq!(
                    run.run_for_tests().facts().instruction().bytes(),
                    PAYLOAD.len()
                );
                let mut terminal = run.terminal();
                terminal.publish().unwrap();
                assert!(
                    std::fs::read_to_string(destination)
                        .unwrap()
                        .contains("EXACT_FINAL")
                );
                assert_eq!(
                    terminal.publish().err().unwrap().kind(),
                    output::Error::AlreadyAttempted
                );
            }
            let requests = f.requests().await;
            assert_eq!(requests.len(), 2);
            assert_eq!(
                requests[0]["messages"].as_array().unwrap().last().unwrap()["content"],
                std::str::from_utf8(PAYLOAD).unwrap()
            );
        }
        "invalid" => {
            assert_eq!(
                Invocation::prepare(f.defaults(), &argv(&["--max-rounds=0", "-"]))
                    .await
                    .err(),
                Some(Error::Selection(SelectionError::Value))
            );
            f.no_store();
            assert!(f.requests().await.is_empty());
            let mut input = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin(), &mut input).unwrap();
            assert_eq!(
                input, PAYLOAD,
                "syntax failure must not consume native stdin"
            );
        }
        "literal" => {
            f.script(vec![
                Msg::assistant("LITERAL_DONE"),
                tools(vec![read()]),
                Msg::assistant("PIPE_DONE"),
            ])
            .await;
            let literal = f.run(&["--", "--max-rounds=2"]).await;
            assert_eq!(
                literal.outcome().as_ref().unwrap().final_text(),
                "LITERAL_DONE"
            );
            assert_counts(&literal, 50, 1, 0, 0);
            let pipe = f.run(&["--max-rounds=2", "-"]).await;
            assert_eq!(pipe.outcome().as_ref().unwrap().final_text(), "PIPE_DONE");
            assert_counts(&pipe, 2, 2, 0, 0);
            let requests = f.requests().await;
            assert_eq!(requests.len(), 3);
            assert_eq!(
                requests[0]["messages"].as_array().unwrap().last().unwrap()["content"],
                "--max-rounds=2"
            );
            assert_eq!(
                requests[1]["messages"].as_array().unwrap().last().unwrap()["content"],
                std::str::from_utf8(PAYLOAD).unwrap()
            );
        }
        _ => unreachable!(),
    }
    eprintln!("CRAFTSMAN_BUDGET_FIXTURE_DONE:{case}");
}
async fn subprocess(markers: &[&str], write: bool) -> (String, String) {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        HELPER,
        "--test-threads=1",
        "--nocapture",
    ]);
    for marker in markers {
        // Marker is a nonmatching skip VALUE, not a free libtest test filter.
        command.arg("--skip").arg(format!("{MARKER}{marker}"));
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    const CAP: u64 = 64 * 1024;
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        if write {
            input.write_all(PAYLOAD).await?;
            drop(input);
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_pipe = stdout.take(CAP + 1);
        let mut err_pipe = stderr.take(CAP + 1);
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            out_pipe.read_to_end(&mut out),
            err_pipe.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    })
    .await;
    let (status, stdout, stderr) = match result {
        Ok(Ok(output)) => output,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned request-budget fixture capture/exit failed within its bounded limit");
        }
    };
    assert!(
        stdout.len() as u64 <= CAP && stderr.len() as u64 <= CAP,
        "whole fixture output overbound"
    );
    let stdout = String::from_utf8(stdout).unwrap();
    let stderr = String::from_utf8(stderr).unwrap();
    assert!(
        status.success(),
        "native fixture assertions failed\n{stdout}\n{stderr}"
    );
    (stdout, stderr)
}
async fn assert_native(case: &str) {
    let (_, stderr) = subprocess(&[case], true).await;
    assert_eq!(
        stderr
            .lines()
            .filter(|line| *line == format!("CRAFTSMAN_BUDGET_FIXTURE_DONE:{case}"))
            .count(),
        1
    );
}
#[tokio::test]
async fn actual_native_stdin_selects_the_requested_shared_cap() {
    assert_native("memory").await;
}
#[tokio::test]
async fn actual_native_stdin_file_preview_retains_the_same_cap_and_pairing() {
    assert_native("file").await;
}
#[tokio::test]
async fn invalid_budget_leaves_original_native_stdin_unread() {
    assert_native("invalid").await;
}
#[tokio::test]
async fn literal_budget_prompt_preserves_the_following_native_pipe_and_both_histories() {
    assert_native("literal").await;
}
#[tokio::test]
async fn unmarked_unknown_and_duplicate_helpers_return_before_open_pipe_or_fixture_io() {
    for markers in [vec![], vec!["unknown"], vec!["memory", "file"]] {
        let (_, stderr) = subprocess(&markers, false).await;
        assert!(!stderr.contains("CRAFTSMAN_BUDGET_FIXTURE_DONE"));
    }
}
