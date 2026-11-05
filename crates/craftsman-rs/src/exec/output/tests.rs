use super::write::{Event, Signals};
use super::*;
use crate::exec::construction::{
    InputPreparationError, Options, OutputPreparationError, SessionSelection,
};
use crate::exec::input::{FileInvocation, FileRun, Sources};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::provider::request::Fault;
use crate::exec::runtime::{InvocationGate, RuntimeError};
use crate::exec::terminal::Error as TerminalError;
use craftsman_harness::{Msg, ToolCall, ToolErrorKind};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEED: &str = "guarded-source-secret";
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
        let config = base.join("selected-config");
        let output = base.join("output");
        for dir in [&workspace, &config, &output] {
            std::fs::create_dir(dir).unwrap();
        }
        let f = Self {
            _root: root,
            workspace,
            config,
            output,
            server: MockServer::start().await,
        };
        f.configure(&[]);
        f
    }
    fn configure(&self, extra: &[&str]) {
        let headers: serde_json::Map<String, Value> = extra
            .iter()
            .enumerate()
            .map(|(i, s)| (format!("X-Fixture-{i}"), json!(s)))
            .collect();
        std::fs::write(
            self.config.join("config.json"),
            json!({"model":"synthetic-model","api_key":SEED,"base_url":self.server.uri(),
                "mcp_servers":[{"name":"disabled","url":"http://127.0.0.1:9","headers":headers}]})
            .to_string(),
        )
        .unwrap();
    }
    fn options(&self, session: SessionSelection, allow: &[&str]) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session,
            selectors: Selectors {
                on_unapproved: OnUnapproved::Fail,
                allow: allow.iter().map(|s| format!("tool:{s}")).collect(),
                ..Default::default()
            },
        }
    }
    fn destination(&self, format: Format, maximum: usize) -> Destination {
        Destination {
            directory: self.output.clone(),
            file_name: "result.out".into(),
            format,
            max_bytes: maximum,
        }
    }
    async fn response(&self, content: &str) {
        let data = json!({"choices":[{"delta":{"content":content},"finish_reason":"stop"}]});
        self.mount(
            200,
            format!("data: {data}\n\ndata: [DONE]\n\n"),
            "text/event-stream",
        )
        .await;
    }
    async fn mount(&self, status: u16, body: String, mime: &str) {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, mime))
            .mount(&self.server)
            .await;
    }
    async fn prepare(&self, format: Format, maximum: usize) -> FileInvocation {
        FileInvocation::prepare(
            self.options(SessionSelection::Ephemeral, &[]),
            sources(),
            self.destination(format, maximum),
        )
        .await
        .unwrap()
    }
    async fn run(&self, format: Format, content: &str) -> FileRun {
        self.response(content).await;
        let run = self.prepare(format, CAP).await.run().await;
        assert!(run.run_for_tests().outcome().is_ok());
        run
    }
    fn no_store(&self) {
        assert!(!self.config.join("threads.db").exists());
        assert!(!self.config.join("sessions-rs").exists());
        assert!(!self.config.join("gadgets/.sdk").exists());
    }
    fn entries(&self) -> Vec<String> {
        let mut names = std::fs::read_dir(&self.output)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        names
    }
    fn absent(&self) {
        assert!(self.entries().is_empty());
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.output.join("result.out")).unwrap()
    }
    fn value(&self) -> Value {
        let bytes = self.bytes();
        assert!(bytes.is_ascii());
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
        serde_json::from_slice(&bytes).unwrap()
    }
}
// Metadata probes release their temporary handles before the actual rename.
// Only the real prepared/runtime directory authority remains retained.
fn directory_identity(path: &Path) -> super::path::Identity {
    let directory = cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority()).unwrap();
    super::path::identity(&directory.dir_metadata().unwrap())
}
fn attempt_directory_move(original: &Path, moved: &Path, expected: super::path::Identity) -> bool {
    match std::fs::rename(original, moved) {
        Ok(()) => {
            assert!(!original.exists());
            assert!(directory_identity(moved) == expected);
            true
        }
        #[cfg(windows)]
        Err(error) => {
            // Exact native access/sharing denials, NOT arbitrary Uncategorized
            // errors. cap-std intentionally retains directory handles without
            // FILE_SHARE_DELETE. Never weaken that authority to force movement.
            assert!(
                matches!(error.raw_os_error(), Some(5 | 32)),
                "actual directory rename failed unexpectedly: {error:?}"
            );
            assert!(original.is_dir() && !moved.exists());
            assert!(directory_identity(original) == expected);
            false
        }
        #[cfg(not(windows))]
        Err(error) => panic!("actual directory rename failed: {error:?}"),
    }
}
fn sources() -> Sources<'static> {
    Sources {
        prompt: Some(b"synthetic instruction"),
        prompt_file: None,
        files: &[],
    }
}
fn gate(run: &FileRun) -> Arc<InvocationGate> {
    run.run_for_tests().runtime_for_tests().gate_for_tests()
}
fn failure(result: Result<(), Failure>, kind: Error, linked: bool) {
    let error = result.unwrap_err();
    assert_eq!(error.kind(), kind);
    assert_eq!(error.link_returned_success(), linked);
    assert_eq!(error.to_string(), "[REDACTED]");
}
async fn rejected(f: &Fixture, destination: Destination, expected: Error) {
    let e = FileInvocation::prepare(
        f.options(SessionSelection::Fresh, &[]),
        sources(),
        destination,
    )
    .await
    .err()
    .unwrap();
    assert_eq!(e, OutputPreparationError::Output(expected));
    f.no_store();
    f.absent();
    assert!(f.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn actual_ascii_file_is_checked_lf_terminated_private_fragment_not_public_result() {
    let f = Fixture::new().await;
    let run = f
        .run(
            Format::AsciiFragment,
            &format!("中文 🦀 {SEED}\u{001b}\n::warning::data"),
        )
        .await;
    let g = gate(&run);
    let mut t = run.terminal();
    t.publish().unwrap();
    let value = f.value();
    assert_eq!(
        value["final_output"],
        "中文 🦀 [REDACTED]\u{001b}\n::warning::data"
    );
    assert_eq!(value["run_return"], "returned");
    assert_eq!(value["approval_required"], false);
    assert!(value.get("usage").is_none() && value.get("status").is_none());
    assert_eq!(f.entries(), vec!["result.out"]);
    assert!(g.execution_closed());
    f.no_store();
}
#[tokio::test]
async fn human_preview_preserves_whole_answer_and_fixed_private_facts_not_stdout_policy() {
    let f = Fixture::new().await;
    let run = f
        .run(
            Format::HumanPreview,
            &format!("中文 {SEED}\r\n##vso[task.x] ::warning::"),
        )
        .await;
    let mut t = run.terminal();
    t.publish().unwrap();
    assert_eq!(
        String::from_utf8(f.bytes()).unwrap(),
        "中文 [REDACTED]\\u{000d}\n#\\u{0023}vso[task.x] :\\u{003a}warning:\\u{003a}\ncraftsman: private run return=returned; approval_required=false; final_answer=present\n"
    );
    assert_eq!(f.entries(), vec!["result.out"]);
    f.no_store();
}
#[tokio::test]
async fn output_is_read_only_and_absent_until_actual_run_returns_and_is_consumed() {
    let f = Fixture::new().await;
    f.response("done").await;
    let inv = f.prepare(Format::AsciiFragment, CAP).await;
    f.absent();
    assert!(f.server.received_requests().await.unwrap().is_empty());
    let run = inv.run().await;
    f.absent();
    let mut t = run.terminal();
    f.absent();
    t.publish().unwrap();
    assert_eq!(f.value()["final_output"], "done");
}
#[tokio::test]
async fn only_one_publication_attempt_is_permitted_even_after_success() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    t.publish().unwrap();
    let before = f.bytes();
    failure(t.publish(), Error::AlreadyAttempted, false);
    assert_eq!(f.bytes(), before);
}
#[tokio::test]
async fn actual_failed_turn_writes_only_fixed_private_human_diagnostic_no_raw_error() {
    let f = Fixture::new().await;
    f.mount(
        400,
        format!("raw failure {SEED}\u{001b} ##vso[task.complete]"),
        "text/plain",
    )
    .await;
    let run = f.prepare(Format::HumanPreview, CAP).await.run().await;
    assert!(matches!(
        run.run_for_tests().outcome(),
        Err(RuntimeError::Turn(_))
    ));
    let mut t = run.terminal();
    t.publish().unwrap();
    assert_eq!(
        String::from_utf8(f.bytes()).unwrap(),
        "craftsman: private run return=turn_failed; approval_required=false; final_answer=absent\n"
    );
    f.no_store();
}
#[tokio::test]
async fn genuine_empty_answer_remains_present_with_its_framing_lf() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::HumanPreview, "").await.terminal();
    t.publish().unwrap();
    assert_eq!(
        String::from_utf8(f.bytes()).unwrap(),
        "\ncraftsman: private run return=returned; approval_required=false; final_answer=present\n"
    );
}
#[tokio::test]
async fn final_words_do_not_latch_or_invent_public_completion() {
    let f = Fixture::new().await;
    let run = f
        .run(
            Format::AsciiFragment,
            "NeedsApproval refused failed success exit=0",
        )
        .await;
    let g = gate(&run);
    let mut t = run.terminal();
    t.publish().unwrap();
    assert!(!g.approval_required());
    assert_eq!(f.value()["approval_required"], false);
    assert!(f.value().get("exit_code").is_none());
}
#[tokio::test]
async fn configuration_reload_never_changes_frozen_seeds_or_original_destination() {
    let f = Fixture::new().await;
    f.configure(&["first-secret"]);
    let run = f.run(Format::AsciiFragment, "first-secret later-key").await;
    f.configure(&["later-key"]);
    let mut t = run.terminal();
    t.publish().unwrap();
    assert_eq!(f.value()["final_output"], "[REDACTED] later-key");
}
#[tokio::test]
async fn original_destination_name_containing_a_seed_is_not_replaced_by_an_alias() {
    let f = Fixture::new().await;
    f.response(SEED).await;
    let mut d = f.destination(Format::AsciiFragment, CAP);
    d.file_name = format!("{SEED}.out");
    let run = FileInvocation::prepare(f.options(SessionSelection::Ephemeral, &[]), sources(), d)
        .await
        .unwrap()
        .run()
        .await;
    run.terminal().publish().unwrap();
    let original = f.output.join(format!("{SEED}.out"));
    assert!(original.exists());
    assert!(!f.output.join("[REDACTED].out").exists());
    assert!(!std::fs::read_to_string(original).unwrap().contains(SEED));
}
#[tokio::test]
async fn original_final_answer_is_not_replaced_by_later_retained_state() {
    let f = Fixture::new().await;
    let run = f.run(Format::HumanPreview, "actual original").await;
    run.run_for_tests()
        .runtime_for_tests()
        .state_for_tests()
        .lock()
        .unwrap()
        .messages = vec![Msg::assistant("different raw last")];
    run.terminal().publish().unwrap();
    assert!(
        String::from_utf8(f.bytes())
            .unwrap()
            .starts_with("actual original\n")
    );
}
#[tokio::test]
async fn unadmitted_retained_state_cannot_be_rescued_by_file_publication() {
    let f = Fixture::new().await;
    let run = f.run(Format::AsciiFragment, "done").await;
    run.run_for_tests()
        .runtime_for_tests()
        .state_for_tests()
        .lock()
        .unwrap()
        .messages = vec![Msg::user("old"); 257];
    failure(
        run.terminal().publish(),
        Error::Terminal(TerminalError::Codec(Fault::Limit)),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn unsafe_original_argument_keys_still_refuse_before_staging() {
    let f = Fixture::new().await;
    let run = f.run(Format::AsciiFragment, "done").await;
    run.run_for_tests()
        .runtime_for_tests()
        .state_for_tests()
        .lock()
        .unwrap()
        .messages = vec![Msg::Assistant {
        content: "".into(),
        tool_calls: vec![ToolCall {
            id: "id".into(),
            name: "example".into(),
            args: json!({SEED:"value"}),
        }],
    }];
    failure(
        run.terminal().publish(),
        Error::Terminal(TerminalError::Codec(Fault::Collision)),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn ascii_generated_escape_collision_is_not_rescued_by_the_file_layer() {
    let f = Fixture::new().await;
    f.configure(&[r"\u4e2d"]);
    let run = f.run(Format::AsciiFragment, "中").await;
    failure(
        run.terminal().publish(),
        Error::Terminal(TerminalError::Codec(Fault::Collision)),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn human_generated_escape_collision_refuses_whole_no_diagnostic_only_fallback() {
    let f = Fixture::new().await;
    f.configure(&[r"\u{001b}"]);
    let run = f.run(Format::HumanPreview, "\u{001b}").await;
    failure(
        run.terminal().publish(),
        Error::Terminal(TerminalError::Codec(Fault::Collision)),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn new_ascii_record_lf_collision_refuses_before_any_staging_effect() {
    let f = Fixture::new().await;
    f.configure(&["}\n"]);
    let run = f.run(Format::AsciiFragment, "done").await;
    failure(
        run.terminal().publish(),
        Error::Terminal(TerminalError::Codec(Fault::Collision)),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn exact_ascii_budget_includes_the_new_physical_lf() {
    let f = Fixture::new().await;
    let run = f.run(Format::AsciiFragment, "done").await;
    run.terminal().publish().unwrap();
    let size = f.bytes().len();
    let f = Fixture::new().await;
    f.response("done").await;
    let mut t = f
        .prepare(Format::AsciiFragment, size)
        .await
        .run()
        .await
        .terminal();
    t.publish().unwrap();
    assert_eq!(f.bytes().len(), size);
}
#[tokio::test]
async fn one_byte_below_minimal_ascii_header_refuses_without_cutting_answer() {
    let f = Fixture::new().await;
    let run = f.run(Format::AsciiFragment, "done").await;
    run.run_for_tests()
        .runtime_for_tests()
        .state_for_tests()
        .lock()
        .unwrap()
        .messages
        .clear();
    run.terminal().publish().unwrap();
    let minimum = f.bytes().len();
    let f = Fixture::new().await;
    f.response("done").await;
    let run = f
        .prepare(Format::AsciiFragment, minimum - 1)
        .await
        .run()
        .await;
    run.run_for_tests()
        .runtime_for_tests()
        .state_for_tests()
        .lock()
        .unwrap()
        .messages
        .clear();
    failure(
        run.terminal().publish(),
        Error::Terminal(TerminalError::Codec(Fault::Limit)),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn too_small_human_budget_is_not_answer_truncation_or_fact_omission() {
    let f = Fixture::new().await;
    f.response("whole answer").await;
    let run = f.prepare(Format::HumanPreview, 20).await.run().await;
    failure(
        run.terminal().publish(),
        Error::Terminal(TerminalError::Codec(Fault::Limit)),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn codec_failure_consumes_attempt_and_cannot_switch_formats_or_retry_layouts() {
    let f = Fixture::new().await;
    f.response("done").await;
    let mut t = f
        .prepare(Format::AsciiFragment, 1)
        .await
        .run()
        .await
        .terminal();
    failure(
        t.publish(),
        Error::Terminal(TerminalError::Codec(Fault::Limit)),
        false,
    );
    failure(t.publish(), Error::AlreadyAttempted, false);
    f.absent();
}
#[tokio::test]
async fn invalid_budgets_fail_before_session_ids_store_or_traffic() {
    let f = Fixture::new().await;
    for max in [0, CAP + 1, usize::MAX] {
        rejected(&f, f.destination(Format::AsciiFragment, max), Error::Limit).await;
    }
}
#[tokio::test]
async fn invalid_single_names_fail_before_session_or_effects() {
    let f = Fixture::new().await;
    for name in [
        "",
        ".",
        "..",
        "a/b",
        "a\\b",
        "x:y",
        "nul",
        "COM1.out",
        "a.",
        "a ",
        "a\u{001b}",
        "a\u{009b}",
    ] {
        let mut d = f.destination(Format::AsciiFragment, CAP);
        d.file_name = name.into();
        rejected(&f, d, Error::Path).await;
    }
}
#[tokio::test]
async fn oversized_name_and_directory_are_rejected_without_discovery_or_store() {
    let f = Fixture::new().await;
    let mut d = f.destination(Format::AsciiFragment, CAP);
    d.file_name = "a".repeat(256);
    rejected(&f, d, Error::Path).await;
    let mut d = f.destination(Format::AsciiFragment, CAP);
    d.directory = PathBuf::from(format!("/{}", "a".repeat(16 * 1024 + 1)));
    rejected(&f, d, Error::Path).await;
}
#[tokio::test]
async fn relative_parent_and_dot_directory_spellings_are_not_normalized_into_grants() {
    let f = Fixture::new().await;
    // Preserve original spellings: Windows verbatim PathBuf::push resolves
    // dot/parent segments before the parser ever sees them.
    let original_segment = |segment: &str| {
        let mut raw = f.output.as_os_str().to_os_string();
        raw.push(std::path::MAIN_SEPARATOR_STR);
        raw.push(segment);
        PathBuf::from(raw)
    };
    for directory in [
        PathBuf::from("relative"),
        original_segment(".."),
        original_segment("."),
    ] {
        let mut d = f.destination(Format::AsciiFragment, CAP);
        d.directory = directory;
        rejected(&f, d, Error::Path).await;
    }
}
#[tokio::test]
async fn runtime_credential_and_build_publication_names_are_hard_refused() {
    let f = Fixture::new().await;
    for name in [
        ".env",
        ".ENV.prod",
        ".git",
        ".craftsman",
        ".craftsman-exec-tmp-x",
        "Cargo.toml",
        "build.rs",
        "install.ps1",
        "Makefile",
    ] {
        let mut d = f.destination(Format::AsciiFragment, CAP);
        d.file_name = name.into();
        rejected(&f, d, Error::Path).await;
    }
}
#[tokio::test]
async fn workspace_and_config_output_roots_do_not_widen_agent_or_config_authority() {
    let f = Fixture::new().await;
    for directory in [&f.workspace, &f.config] {
        let mut d = f.destination(Format::AsciiFragment, CAP);
        d.directory = directory.clone();
        rejected(&f, d, Error::Scope).await;
    }
}
#[tokio::test]
async fn nested_workspace_or_config_output_roots_are_also_refused() {
    let f = Fixture::new().await;
    for directory in [f.workspace.join("nested"), f.config.join("nested")] {
        std::fs::create_dir(&directory).unwrap();
        let mut d = f.destination(Format::AsciiFragment, CAP);
        d.directory = directory;
        rejected(&f, d, Error::Scope).await;
    }
}
#[tokio::test]
async fn existing_unrelated_output_is_preserved_and_prevents_thread_creation() {
    let f = Fixture::new().await;
    std::fs::write(f.output.join("result.out"), b"unrelated").unwrap();
    let e = FileInvocation::prepare(
        f.options(SessionSelection::Fresh, &[]),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(e, OutputPreparationError::Output(Error::Exists));
    assert_eq!(f.bytes(), b"unrelated");
    f.no_store();
}
#[tokio::test]
async fn existing_output_directory_is_not_opened_as_a_file_or_replaced() {
    let f = Fixture::new().await;
    std::fs::create_dir(f.output.join("result.out")).unwrap();
    let e = FileInvocation::prepare(
        f.options(SessionSelection::Fresh, &[]),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(e, OutputPreparationError::Output(Error::Exists));
    assert!(f.output.join("result.out").is_dir());
    f.no_store();
}
#[tokio::test]
async fn existing_hard_link_is_not_overwritten_or_unlinked() {
    let f = Fixture::new().await;
    let outside = f.output.join("original");
    std::fs::write(&outside, b"keep").unwrap();
    std::fs::hard_link(&outside, f.output.join("result.out")).unwrap();
    let e = FileInvocation::prepare(
        f.options(SessionSelection::Fresh, &[]),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(e, OutputPreparationError::Output(Error::Exists));
    assert_eq!(f.bytes(), b"keep");
    assert_eq!(std::fs::read(outside).unwrap(), b"keep");
    f.no_store();
}
#[tokio::test]
async fn absent_destination_parent_is_not_created_during_preparation() {
    let f = Fixture::new().await;
    let mut d = f.destination(Format::AsciiFragment, CAP);
    d.directory = f.output.join("missing");
    rejected(&f, d, Error::Io(std::io::ErrorKind::NotFound)).await;
    assert!(!f.output.join("missing").exists());
}
#[tokio::test]
async fn invalid_output_precedes_legacy_store_schema_inspection_or_writer_open() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("threads.db"), b"invalid legacy sentinel").unwrap();
    let mut d = f.destination(Format::AsciiFragment, CAP);
    d.file_name = "../bad".into();
    let e = FileInvocation::prepare(f.options(SessionSelection::Fresh, &[]), sources(), d)
        .await
        .err()
        .unwrap();
    assert_eq!(e, OutputPreparationError::Output(Error::Path));
    assert_eq!(
        std::fs::read(f.config.join("threads.db")).unwrap(),
        b"invalid legacy sentinel"
    );
    f.absent();
}
#[tokio::test]
async fn bad_input_never_creates_output_staging_or_thread_store() {
    let f = Fixture::new().await;
    let input = Sources {
        prompt: Some(b"\xff"),
        prompt_file: None,
        files: &[],
    };
    let e = FileInvocation::prepare(
        f.options(SessionSelection::Fresh, &[]),
        input,
        f.destination(Format::AsciiFragment, CAP),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(
        e,
        OutputPreparationError::Input(InputPreparationError::Input(
            crate::exec::input::InputError::Utf8
        ))
    );
    f.no_store();
    f.absent();
}
#[tokio::test]
async fn late_existing_output_is_preserved_and_no_staging_is_retained() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    std::fs::write(f.output.join("result.out"), b"late unrelated").unwrap();
    failure(t.publish(), Error::Exists, false);
    assert_eq!(f.bytes(), b"late unrelated");
    assert_eq!(f.entries(), vec!["result.out"]);
}
#[tokio::test]
async fn planted_destination_between_staging_and_link_is_never_replaced() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    let target = f.output.join("result.out");
    let mut hook = |event, _: &Terminal, _: &cap_std::fs::Dir| {
        if event == Event::BeforeLink {
            std::fs::write(&target, b"raced unrelated").unwrap();
        }
    };
    failure(
        t.publish_for_tests(&mut Signals {
            hook: Some(&mut hook),
            ..Default::default()
        }),
        Error::Exists,
        false,
    );
    assert_eq!(f.bytes(), b"raced unrelated");
    assert_eq!(f.entries(), vec!["result.out"]);
}
#[tokio::test]
async fn output_parent_rebinding_is_refused_by_identity_or_native_retained_handles() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    let original = directory_identity(&f.output);
    let old = f.output.with_file_name("old-output");
    if attempt_directory_move(&f.output, &old, original) {
        std::fs::create_dir(&f.output).unwrap();
        failure(t.publish(), Error::Changed, false);
        f.absent();
        assert!(std::fs::read_dir(old).unwrap().next().is_none());
    } else {
        // Native refusal is not a simulated rebind or skipped assertion. The
        // only publication must still use the unchanged original directory.
        t.publish().unwrap();
        assert!(directory_identity(&f.output) == original);
        assert_eq!(f.value()["final_output"], "done");
        assert_eq!(f.entries(), vec!["result.out"]);
        assert!(!old.exists());
    }
    failure(t.publish(), Error::AlreadyAttempted, false);
    f.no_store();
}
#[tokio::test]
async fn staged_parent_rebinding_keeps_original_authority_or_preserves_replacement() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    let original = directory_identity(&f.output);
    let old = f.output.with_file_name("old-output");
    let mut moved = None;
    let mut hook = |event, _: &Terminal, _: &cap_std::fs::Dir| {
        if event == Event::BeforeLink {
            assert!(moved.is_none());
            let movement = attempt_directory_move(&f.output, &old, original);
            moved = Some(movement);
            if movement {
                std::fs::create_dir(&f.output).unwrap();
            }
            std::fs::write(f.output.join("keep"), b"keep").unwrap();
        }
    };
    let result = t.publish_for_tests(&mut Signals {
        hook: Some(&mut hook),
        ..Default::default()
    });
    if moved.expect("a real rename must be attempted before link") {
        failure(result, Error::Changed, false);
        assert_eq!(f.entries(), vec!["keep"]);
        assert!(std::fs::read_dir(old).unwrap().next().is_none());
    } else {
        result.unwrap();
        assert!(directory_identity(&f.output) == original);
        assert_eq!(f.value()["final_output"], "done");
        assert_eq!(f.entries(), vec!["keep", "result.out"]);
        assert!(!old.exists());
    }
    assert_eq!(std::fs::read(f.output.join("keep")).unwrap(), b"keep");
    failure(t.publish(), Error::AlreadyAttempted, false);
    f.no_store();
}
#[tokio::test]
async fn same_length_staging_content_change_before_link_is_refused_not_repaired() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    let mut hook = |event, _: &Terminal, dir: &cap_std::fs::Dir| {
        if event == Event::BeforeLink {
            let bytes = dir.read("record").unwrap();
            dir.write("record", vec![b'x'; bytes.len()]).unwrap();
        }
    };
    failure(
        t.publish_for_tests(&mut Signals {
            hook: Some(&mut hook),
            ..Default::default()
        }),
        Error::Changed,
        false,
    );
    f.absent();
}
#[tokio::test]
async fn rebound_staging_entry_is_not_linked_as_a_checked_source() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    let mut hook = |event, _: &Terminal, dir: &cap_std::fs::Dir| {
        if event == Event::BeforeLink {
            dir.rename("record", dir, "original").unwrap();
            dir.write("record", b"unrelated").unwrap();
        }
    };
    failure(
        t.publish_for_tests(&mut Signals {
            hook: Some(&mut hook),
            ..Default::default()
        }),
        Error::Changed,
        false,
    );
    assert!(!f.output.join("result.out").exists());
    assert_eq!(f.entries().len(), 1); // foreign staging entries are not recursively deleted
}
#[tokio::test]
async fn partial_native_writes_are_advanced_exactly_and_flushed_before_link() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    t.publish_for_tests(&mut Signals {
        max_write: Some(3),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(f.value()["final_output"], "done");
}
#[tokio::test]
async fn interrupted_write_results_retry_only_under_the_same_ready_guard() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    t.publish_for_tests(&mut Signals {
        interrupted: 3,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(f.value()["final_output"], "done");
}
#[tokio::test]
async fn zero_length_write_is_failure_not_completed_publication() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    failure(
        t.publish_for_tests(&mut Signals {
            max_write: Some(0),
            ..Default::default()
        }),
        Error::Io(std::io::ErrorKind::WriteZero),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn simulated_broken_pipe_keeps_only_kind_and_never_publishes_or_retries() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    failure(
        t.publish_for_tests(&mut Signals {
            fail_write: Some(std::io::ErrorKind::BrokenPipe),
            ..Default::default()
        }),
        Error::Io(std::io::ErrorKind::BrokenPipe),
        false,
    );
    failure(t.publish(), Error::AlreadyAttempted, false);
    f.absent();
}
#[tokio::test]
async fn flush_failure_does_not_claim_complete_delivery_or_link_the_record() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    failure(
        t.publish_for_tests(&mut Signals {
            fail_flush: true,
            ..Default::default()
        }),
        Error::Io(std::io::ErrorKind::BrokenPipe),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn sync_failure_does_not_link_or_retry_the_staged_prefix() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    failure(
        t.publish_for_tests(&mut Signals {
            fail_sync: true,
            ..Default::default()
        }),
        Error::Io(std::io::ErrorKind::Other),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn interrupted_livelock_is_bounded_without_widening_native_or_source_limits() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    failure(
        t.publish_for_tests(&mut Signals {
            interrupted: 4096,
            ..Default::default()
        }),
        Error::Limit,
        false,
    );
    f.absent();
}
#[tokio::test]
async fn publication_revoked_before_attempt_causes_no_staging_or_output() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    t.close();
    failure(t.publish(), Error::Terminal(TerminalError::Closed), false);
    f.absent();
}
#[tokio::test]
async fn late_actual_approval_before_capture_keeps_original_return_plus_real_stop() {
    let f = Fixture::new().await;
    let run = f.run(Format::AsciiFragment, "done").await;
    let g = gate(&run);
    g.observe_error_kind(ToolErrorKind::NeedsApproval);
    run.terminal().publish().unwrap();
    assert_eq!(f.value()["run_return"], "returned");
    assert_eq!(f.value()["approval_required"], true);
    assert!(g.execution_closed());
}
const EVENTS: &[Event] = &[
    Event::BeforeCreate,
    Event::AfterCreate,
    Event::BeforeOpen,
    Event::AfterOpen,
    Event::BeforeWrite,
    Event::AfterWrite,
    Event::BeforeFlush,
    Event::AfterFlush,
    Event::BeforeSync,
    Event::AfterSync,
    Event::BeforeCheck,
    Event::AfterCheck,
    Event::BeforeLink,
    Event::AfterLink,
    Event::BeforeCleanup,
    Event::AfterCleanup,
    Event::Finished,
];
fn linked_at(event: Event) -> bool {
    matches!(
        event,
        Event::AfterLink | Event::BeforeCleanup | Event::AfterCleanup | Event::Finished
    )
}
#[tokio::test]
async fn actual_approval_change_at_every_io_stage_hides_later_work_and_keeps_link_observation() {
    for stage in EVENTS {
        let f = Fixture::new().await;
        let run = f.run(Format::AsciiFragment, "done").await;
        let g = gate(&run);
        let mut t = run.terminal();
        let mut events = Vec::new();
        let mut hook = |event, _: &Terminal, _: &cap_std::fs::Dir| {
            events.push(event);
            if event == *stage {
                g.observe_error_kind(ToolErrorKind::NeedsApproval);
            }
        };
        failure(
            t.publish_for_tests(&mut Signals {
                hook: Some(&mut hook),
                ..Default::default()
            }),
            Error::Terminal(TerminalError::Superseded),
            linked_at(*stage),
        );
        assert_eq!(events.last(), Some(stage));
        assert!(g.execution_closed() && g.approval_required());
        if linked_at(*stage) {
            assert_eq!(f.value()["final_output"], "done");
            assert_eq!(f.entries(), vec!["result.out"]);
        } else {
            f.absent();
        }
    }
}
#[tokio::test]
async fn actual_publication_revocation_at_every_io_stage_prevents_any_later_data_operation() {
    for stage in EVENTS {
        let f = Fixture::new().await;
        let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
        let mut events = Vec::new();
        let mut hook = |event, terminal: &Terminal, _: &cap_std::fs::Dir| {
            events.push(event);
            if event == *stage {
                terminal.close();
            }
        };
        failure(
            t.publish_for_tests(&mut Signals {
                hook: Some(&mut hook),
                ..Default::default()
            }),
            Error::Terminal(TerminalError::Closed),
            linked_at(*stage),
        );
        assert_eq!(events.last(), Some(stage));
        if linked_at(*stage) {
            assert_eq!(f.entries(), vec!["result.out"]);
        } else {
            f.absent();
        }
    }
}
#[tokio::test]
async fn same_typed_epoch_overrides_an_actual_local_write_error_without_fake_success() {
    let f = Fixture::new().await;
    let run = f.run(Format::AsciiFragment, "done").await;
    let g = gate(&run);
    let mut t = run.terminal();
    let mut hook = |event, _: &Terminal, _: &cap_std::fs::Dir| {
        if event == Event::AfterWrite {
            g.observe_error_kind(ToolErrorKind::NeedsApproval);
        }
    };
    failure(
        t.publish_for_tests(&mut Signals {
            hook: Some(&mut hook),
            fail_write: Some(std::io::ErrorKind::BrokenPipe),
            ..Default::default()
        }),
        Error::Terminal(TerminalError::Superseded),
        false,
    );
    f.absent();
}
#[tokio::test]
async fn mutation_after_link_is_reported_as_failure_without_deleting_final_or_faking_absence() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    let mut hook = |event, _: &Terminal, dir: &cap_std::fs::Dir| {
        if event == Event::AfterLink {
            let bytes = dir.read("record").unwrap();
            dir.write("record", vec![b'x'; bytes.len()]).unwrap();
        }
    };
    failure(
        t.publish_for_tests(&mut Signals {
            hook: Some(&mut hook),
            ..Default::default()
        }),
        Error::Changed,
        true,
    );
    assert!(f.output.join("result.out").exists());
    assert_eq!(f.entries(), vec!["result.out"]);
}
#[tokio::test]
async fn final_name_rebinding_after_link_preserves_the_unrelated_replacement() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    let target = f.output.join("result.out");
    let mut hook = |event, _: &Terminal, _: &cap_std::fs::Dir| {
        if event == Event::AfterCleanup {
            std::fs::remove_file(&target).unwrap();
            std::fs::write(&target, b"unrelated replacement").unwrap();
        }
    };
    failure(
        t.publish_for_tests(&mut Signals {
            hook: Some(&mut hook),
            ..Default::default()
        }),
        Error::Changed,
        true,
    );
    assert_eq!(f.bytes(), b"unrelated replacement");
}
#[tokio::test]
async fn changed_same_inode_content_after_cleanup_is_not_reported_as_success() {
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    let target = f.output.join("result.out");
    let mut hook = |event, _: &Terminal, _: &cap_std::fs::Dir| {
        if event == Event::AfterCleanup {
            let bytes = std::fs::read(&target).unwrap();
            std::fs::write(&target, vec![b'x'; bytes.len()]).unwrap();
        }
    };
    failure(
        t.publish_for_tests(&mut Signals {
            hook: Some(&mut hook),
            ..Default::default()
        }),
        Error::Changed,
        true,
    );
    assert_eq!(f.entries(), vec!["result.out"]);
}
#[tokio::test]
async fn two_equal_actual_invocations_have_independent_files_and_revocation() {
    let f = Fixture::new().await;
    f.response("same").await;
    let one = f.prepare(Format::AsciiFragment, CAP).await.run().await;
    let mut d = f.destination(Format::AsciiFragment, CAP);
    d.file_name = "second.out".into();
    let two = FileInvocation::prepare(f.options(SessionSelection::Ephemeral, &[]), sources(), d)
        .await
        .unwrap()
        .run()
        .await;
    let mut one = one.terminal();
    let mut two = two.terminal();
    one.close();
    failure(one.publish(), Error::Terminal(TerminalError::Closed), false);
    two.publish().unwrap();
    assert_eq!(f.entries(), vec!["second.out"]);
}
#[tokio::test]
async fn private_file_operations_do_not_reopen_compaction_or_add_provider_calls() {
    let f = Fixture::new().await;
    let run = f.run(Format::AsciiFragment, "done").await;
    let g = gate(&run);
    run.terminal().publish().unwrap();
    assert!(g.execution_closed());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}
#[cfg(unix)]
fn symlink(old: &Path, new: &Path) {
    std::os::unix::fs::symlink(old, new).unwrap();
}
#[cfg(windows)]
fn symlink_file(old: &Path, new: &Path) {
    std::os::windows::fs::symlink_file(old, new).unwrap();
}
#[tokio::test]
async fn existing_final_symlink_is_not_followed_or_overwritten() {
    let f = Fixture::new().await;
    let original = f.output.join("original");
    std::fs::write(&original, b"keep").unwrap();
    #[cfg(unix)]
    symlink(&original, &f.output.join("result.out"));
    #[cfg(windows)]
    symlink_file(&original, &f.output.join("result.out"));
    let e = FileInvocation::prepare(
        f.options(SessionSelection::Fresh, &[]),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(e, OutputPreparationError::Output(Error::Exists));
    assert_eq!(std::fs::read(original).unwrap(), b"keep");
    f.no_store();
}
#[tokio::test]
async fn broken_final_symlink_is_not_a_create_only_hole() {
    let f = Fixture::new().await;
    let missing = f.output.join("missing");
    #[cfg(unix)]
    symlink(&missing, &f.output.join("result.out"));
    #[cfg(windows)]
    symlink_file(&missing, &f.output.join("result.out"));
    let e = FileInvocation::prepare(
        f.options(SessionSelection::Fresh, &[]),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(e, OutputPreparationError::Output(Error::Exists));
    assert!(
        std::fs::symlink_metadata(f.output.join("result.out"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    f.no_store();
}
#[cfg(unix)]
#[tokio::test]
async fn planted_symlink_in_destination_directory_walk_is_rejected_before_session() {
    let f = Fixture::new().await;
    let alias = f.output.with_file_name("alias");
    symlink(&f.output, &alias);
    let mut d = f.destination(Format::AsciiFragment, CAP);
    d.directory = alias;
    rejected(&f, d, Error::Path).await;
}
#[cfg(unix)]
#[tokio::test]
async fn new_files_and_staging_directories_are_private_from_creation() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let mut t = f.run(Format::AsciiFragment, "done").await.terminal();
    let mut hook = |event, _: &Terminal, dir: &cap_std::fs::Dir| {
        if event == Event::AfterOpen {
            use cap_std::fs::PermissionsExt as _;
            assert_eq!(
                dir.dir_metadata().unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                dir.metadata("record").unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    };
    t.publish_for_tests(&mut Signals {
        hook: Some(&mut hook),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        std::fs::metadata(f.output.join("result.out"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

async fn approval_run(f: &Fixture, format: Format) -> FileRun {
    let data = json!({"choices":[{"delta":{"content":format!("partial {SEED}"),"tool_calls":[
        {"index":0,"id":"stop","type":"function","function":{"name":"execute","arguments":json!({"command":"must-not-run"}).to_string()}},
        {"index":1,"id":"later","type":"function","function":{"name":"write_file","arguments":json!({"file_path":"later.txt","content":SEED}).to_string()}}
    ]},"finish_reason":"tool_calls"}]});
    f.mount(
        200,
        format!("data: {data}\n\ndata: [DONE]\n\n"),
        "text/event-stream",
    )
    .await;
    FileInvocation::prepare(
        f.options(SessionSelection::Ephemeral, &["write_file"]),
        sources(),
        f.destination(format, CAP),
    )
    .await
    .unwrap()
    .run()
    .await
}
#[tokio::test]
async fn actual_approval_required_ascii_keeps_original_return_and_partial_retained_state() {
    let f = Fixture::new().await;
    let run = approval_run(&f, Format::AsciiFragment).await;
    assert!(matches!(
        run.run_for_tests().outcome(),
        Err(RuntimeError::ApprovalRequired)
    ));
    let g = gate(&run);
    run.terminal().publish().unwrap();
    let value = f.value();
    assert_eq!(value["run_return"], "approval_required");
    assert_eq!(value["approval_required"], true);
    assert!(value["final_output"].is_null());
    assert!(!String::from_utf8(f.bytes()).unwrap().contains(SEED));
    assert!(!f.workspace.join("later.txt").exists());
    assert!(g.execution_closed() && g.approval_required());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}
#[tokio::test]
async fn actual_approval_required_human_has_no_synthetic_partial_answer_or_resume() {
    let f = Fixture::new().await;
    let run = approval_run(&f, Format::HumanPreview).await;
    assert!(matches!(
        run.run_for_tests().outcome(),
        Err(RuntimeError::ApprovalRequired)
    ));
    run.terminal().publish().unwrap();
    assert_eq!(
        String::from_utf8(f.bytes()).unwrap(),
        "craftsman: private run return=approval_required; approval_required=true; final_answer=absent\n"
    );
    assert!(!f.workspace.join("later.txt").exists());
    f.no_store();
}
#[tokio::test]
async fn actual_selected_config_identity_stays_denied_when_root_rename_is_attempted() {
    let f = Fixture::new().await;
    let prepared =
        crate::exec::construction::Prepared::prepare(f.options(SessionSelection::Ephemeral, &[]))
            .await
            .unwrap();
    let moved = f.config.with_file_name("moved-config");
    let original = directory_identity(&f.config);
    let selected = if attempt_directory_move(&f.config, &moved, original) {
        moved
    } else {
        f.config.clone()
    };
    let mut destination = f.destination(Format::AsciiFragment, CAP);
    destination.directory = selected;
    assert_eq!(
        Selected::select(destination)
            .unwrap()
            .capture(prepared.source_roots())
            .err(),
        Some(Error::Scope)
    );
    f.absent();
}
#[tokio::test]
async fn exact_one_mib_private_human_record_is_written_without_expanding_any_source_cap() {
    let diagnostic =
        "craftsman: private run return=returned; approval_required=false; final_answer=present\n";
    let count = (CAP - diagnostic.len() + 6) / 8;
    let tail = CAP - diagnostic.len() - (8 * count - 6);
    let content = ":".repeat(count) + &"x".repeat(tail);
    let f = Fixture::new().await;
    f.run(Format::HumanPreview, &content)
        .await
        .terminal()
        .publish()
        .unwrap();
    assert_eq!(f.bytes().len(), CAP);
    assert_eq!(f.entries(), vec!["result.out"]);
}
#[tokio::test]
async fn oversized_original_output_work_fails_before_copying_to_a_staging_file() {
    let f = Fixture::new().await;
    let run = f.run(Format::HumanPreview, &"x".repeat(400_000)).await;
    failure(
        run.terminal().publish(),
        Error::Terminal(TerminalError::Codec(Fault::Limit)),
        false,
    );
    f.absent();
}

#[tokio::test]
async fn content_pass_seek_payload_and_eof_share_the_same_native_call_ceiling() {
    let diagnostic =
        "craftsman: private run return=returned; approval_required=false; final_answer=present\n";
    for (file_bytes, allowed) in [(4094usize, true), (4095, false)] {
        let f = Fixture::new().await;
        let answer = "x".repeat(file_bytes - diagnostic.len() - 1);
        let mut t = f.run(Format::HumanPreview, &answer).await.terminal();
        let mut staged_length = None;
        let mut hook = |event, _: &Terminal, dir: &cap_std::fs::Dir| {
            if event == Event::BeforeCheck {
                assert!(staged_length.is_none());
                staged_length = Some(dir.metadata("record").unwrap().len());
            }
        };
        let mut signals = Signals {
            hook: Some(&mut hook),
            max_read: Some(1),
            ..Default::default()
        };
        let result = t.publish_for_tests(&mut signals);
        if allowed {
            result.unwrap();
            // Five real passes: one seek + 4094 payload reads + one EOF probe
            // each. The fixture counter observes actual native read attempts
            // only; it is never a production byte/usage/receipt field.
            assert_eq!(signals.read_calls, 5 * (file_bytes + 1));
            assert_eq!(f.bytes().len(), file_bytes);
            assert_eq!(f.bytes(), (answer + "\n" + diagnostic).as_bytes());
            assert_eq!(f.entries(), vec!["result.out"]);
        } else {
            failure(result, Error::Limit, false);
            // The first pass consumed its one seek and 4095 native reads:
            // no extra EOF probe, link, later pass or publication retry.
            assert_eq!(signals.read_calls, file_bytes);
            f.absent();
        }
        assert_eq!(staged_length, Some(file_bytes as u64));
        failure(t.publish(), Error::AlreadyAttempted, false);
        f.no_store();
    }
}

#[tokio::test]
async fn failed_owned_cleanup_rechecks_actual_stop_and_preserves_foreign_entries() {
    for stop in [1u8, 2, 0] {
        let f = Fixture::new().await;
        let run = f.run(Format::AsciiFragment, "done").await;
        let g = gate(&run);
        let mut t = run.terminal();
        let mut cleanup_returned = false;
        let mut hook = |event, terminal: &Terminal, dir: &cap_std::fs::Dir| {
            if event == Event::BeforeCleanup {
                // Real native identity conflict, not an injected writer/error.
                // Our final link already exists; the foreign entry must survive.
                dir.rename("record", dir, "original").unwrap();
                dir.write("record", b"foreign cleanup entry").unwrap();
            } else if event == Event::AfterCleanup {
                cleanup_returned = true;
                if stop == 1 {
                    g.observe_error_kind(ToolErrorKind::NeedsApproval);
                } else if stop == 2 {
                    terminal.close();
                }
            }
        };
        failure(
            t.publish_for_tests(&mut Signals {
                hook: Some(&mut hook),
                ..Default::default()
            }),
            match stop {
                1 => Error::Terminal(TerminalError::Superseded),
                2 => Error::Terminal(TerminalError::Closed),
                _ => Error::Changed,
            },
            true,
        );
        assert!(cleanup_returned);
        assert_eq!(g.approval_required(), stop == 1);
        assert!(g.execution_closed());
        assert_eq!(f.value()["final_output"], "done");
        let entries = f.entries();
        assert_eq!(entries.len(), 2);
        let staging = entries
            .iter()
            .find(|name| name.starts_with(".craftsman-exec-tmp-"))
            .unwrap();
        assert_eq!(
            std::fs::read(f.output.join(staging).join("record")).unwrap(),
            b"foreign cleanup entry"
        );
        assert_eq!(
            std::fs::read(f.output.join(staging).join("original")).unwrap(),
            f.bytes()
        );
        failure(t.publish(), Error::AlreadyAttempted, false);
        f.no_store();
    }
}
