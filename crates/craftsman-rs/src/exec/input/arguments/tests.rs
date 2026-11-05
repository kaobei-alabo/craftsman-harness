use super::*;
use crate::exec::construction::{PreparationError, SessionSelection};
use crate::exec::input::OriginKind;
use crate::exec::output::{Error as OutputError, Format};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::runtime::RuntimeError;
use craftsman_harness::Msg;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn argv(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn rejected(values: &[&str], expected: SelectionError) {
    assert_eq!(Selected::select(&argv(values)).err(), Some(expected));
}
fn original(values: &[&str], prompt: Option<&str>, file: Option<&str>, files: &[&str]) {
    let arguments = argv(values);
    let selected = Selected::select(&arguments).unwrap();
    let sources = selected.sources();
    assert_eq!(sources.prompt, prompt.map(str::as_bytes));
    assert_eq!(sources.prompt_file, file);
    assert_eq!(sources.files, files);
}
#[test]
fn exactly_one_original_positional_instruction_is_preserved() {
    original(
        &["original goal\r\n中文 🦀"],
        Some("original goal\r\n中文 🦀"),
        None,
        &[],
    );
}
#[test]
fn exact_positional_minus_selects_native_stdin_without_reading_it() {
    let arguments = argv(&["-"]);
    let selected = Selected::select(&arguments).unwrap();
    assert!(selected.is_stdin());
    assert_eq!(selected.sources().prompt, Some(b"-".as_slice()));
}
#[test]
fn spaced_minus_is_original_literal_data_not_native_stdin() {
    let arguments = argv(&[" - "]);
    let selected = Selected::select(&arguments).unwrap();
    assert!(!selected.is_stdin());
    assert_eq!(selected.sources().prompt, Some(b" - ".as_slice()));
}
#[test]
fn split_and_joined_prompt_file_select_identical_original_paths() {
    for arguments in [
        ["--prompt-file", "goal.txt"].as_slice(),
        ["--prompt-file=goal.txt"].as_slice(),
    ] {
        original(arguments, None, Some("goal.txt"), &[]);
    }
}
#[test]
fn minus_prompt_file_is_a_workspace_filename_not_stdin() {
    for arguments in [
        ["--prompt-file", "-"].as_slice(),
        ["--prompt-file=-"].as_slice(),
    ] {
        let owned = argv(arguments);
        let selected = Selected::select(&owned).unwrap();
        assert!(!selected.is_stdin());
        assert_eq!(selected.sources().prompt_file, Some("-"));
    }
}
#[test]
fn repeated_split_and_joined_files_preserve_order_and_duplicates() {
    original(
        &["--file", "a", "goal", "--file=b", "--file", "a"],
        Some("goal"),
        None,
        &["a", "b", "a"],
    );
}
#[test]
fn one_split_file_value_never_consumes_later_positional_instruction() {
    original(
        &["--file", "data.txt", "goal"],
        Some("goal"),
        None,
        &["data.txt"],
    );
}
#[test]
fn equals_spaces_quotes_and_backslashes_are_not_shell_split_or_normalized() {
    original(
        &[
            "--prompt-file=original=a b\\c\".txt",
            "--file=quoted=\"original\"",
        ],
        None,
        Some("original=a b\\c\".txt"),
        &["quoted=\"original\""],
    );
}
#[test]
fn native_unicode_and_original_dot_parent_path_spellings_remain_data() {
    original(
        &["--prompt-file=資料/./../goal", "--file", "資料/first.txt"],
        None,
        Some("資料/./../goal"),
        &["資料/first.txt"],
    );
}
#[test]
fn shell_and_slash_looking_prompts_are_model_data_not_command_routes() {
    for text in [
        "!touch forbidden",
        "/dangerous --flag",
        "a; $(echo not-a-route)",
        "::warning::\n##vso[task.x]",
    ] {
        original(&[text], Some(text), None, &[]);
    }
}
#[test]
fn end_of_options_preserves_option_looking_literal_instructions() {
    for text in ["--file=not-an-attachment", "--timeout", "-m", "--"] {
        original(&["--", text], Some(text), None, &[]);
    }
}
#[test]
fn end_of_options_does_not_turn_an_exact_minus_into_literal_data() {
    let arguments = argv(&["--", "-"]);
    assert!(Selected::select(&arguments).unwrap().is_stdin());
}
#[test]
fn joined_option_looking_file_values_are_explicit_filenames() {
    original(
        &["--prompt-file=--goal", "--file=--timeout"],
        None,
        Some("--goal"),
        &["--timeout"],
    );
}
#[test]
fn split_minus_attachment_is_an_explicit_filename() {
    original(&["goal", "--file", "-"], Some("goal"), None, &["-"]);
}
#[test]
fn empty_or_attachment_only_argument_lists_have_no_instruction_source() {
    for arguments in [
        vec![],
        vec!["--"],
        vec!["--file", "data.txt"],
        vec!["--file=data.txt"],
    ] {
        rejected(&arguments, SelectionError::Input(InputError::MissingSource));
    }
}
#[test]
fn two_positionals_or_prompt_file_plus_positional_refuse_whole_selection() {
    for arguments in [
        vec!["one", "two"],
        vec!["--prompt-file=a", "goal"],
        vec!["-", "--prompt-file=a"],
        vec!["--prompt-file", "a", "--", "-"],
        vec!["goal", "--", "other"],
    ] {
        rejected(
            &arguments,
            SelectionError::Input(InputError::ConflictingSources),
        );
    }
}
#[test]
fn repeated_prompt_file_is_a_source_conflict_even_for_identical_paths() {
    for arguments in [
        vec!["--prompt-file=a", "--prompt-file=a"],
        vec!["--prompt-file", "a", "--prompt-file", "a"],
    ] {
        rejected(
            &arguments,
            SelectionError::Input(InputError::ConflictingSources),
        );
    }
}
#[test]
fn empty_and_whitespace_literal_instructions_reuse_old_admission() {
    for prompt in ["", " \r\n\t", "\u{2003}"] {
        rejected(&[prompt], SelectionError::Input(InputError::Empty));
    }
}
#[test]
fn missing_split_values_refuse_instead_of_becoming_implicit_stdin() {
    for arguments in [vec!["--prompt-file"], vec!["goal", "--file"]] {
        rejected(&arguments, SelectionError::Value);
    }
}
#[test]
fn split_option_looking_values_must_use_unambiguous_joined_form() {
    for arguments in [
        vec!["--prompt-file", "--file=a"],
        vec!["goal", "--file", "--timeout"],
        vec!["--prompt-file", "-name"],
    ] {
        rejected(&arguments, SelectionError::Value);
    }
}
#[test]
fn empty_and_nul_paths_reuse_old_path_rejection() {
    for arguments in [
        vec!["--prompt-file="],
        vec!["--prompt-file", ""],
        vec!["goal", "--file="],
        vec!["goal", "--file", "a\0b"],
        vec!["--prompt-file=a\0b"],
    ] {
        rejected(&arguments, SelectionError::Input(InputError::Selection));
    }
}
#[test]
fn unknown_abbreviated_and_legacy_short_options_are_never_ignored() {
    for flag in [
        "--unknown",
        "--fil=a",
        "--prompt=a",
        "--files=a",
        "-m",
        "-f",
        "-",
        "---",
    ] {
        if flag != "-" {
            rejected(&["goal", flag], SelectionError::Flag);
        }
    }
}
#[test]
fn full_frontend_scope_permission_and_session_flags_are_unimplemented_here() {
    for flag in [
        "--cwd",
        "--config-dir",
        "--trust-workspace",
        "--permission-mode",
        "--on-unapproved",
        "--allow",
        "--deny",
        "--env-pass",
        "--auto-approve",
        "--session",
        "--continue",
        "--ephemeral",
    ] {
        rejected(&["goal", flag], SelectionError::Flag);
    }
}
#[test]
fn unimplemented_output_trace_usage_deadline_and_gadget_flags_refuse_before_run() {
    for flag in [
        "--output-file",
        "--output-format=json",
        "--trace-file",
        "--max-rounds=1",
        "--token-budget=1",
        "--timeout=1",
        "--allow-gadget=x",
        "--help",
        "--version",
    ] {
        rejected(&["-", flag], SelectionError::Flag);
    }
}
#[test]
fn exact_instruction_byte_ceiling_passes_and_one_more_byte_refuses() {
    let exact = "x".repeat(super::super::INSTRUCTION_BYTES);
    let exact_args = argv(&[&exact]);
    assert!(Selected::select(&exact_args).is_ok());
    let over = format!("{exact}x");
    rejected(
        &[&over],
        SelectionError::Input(InputError::InstructionLimit),
    );
}
#[test]
fn original_multibyte_instruction_bytes_not_scalar_count_control_admission() {
    let exact = "🦀".repeat(super::super::INSTRUCTION_BYTES / 4);
    assert!(Selected::select(&argv(&[&exact])).is_ok());
    rejected(
        &[&format!("{exact}🦀")],
        SelectionError::Input(InputError::InstructionLimit),
    );
}
#[test]
fn token_ceiling_refuses_before_utf8_or_source_copy() {
    rejected(&[&"x".repeat(TOKEN_BYTES + 1)], SelectionError::Resource);
}
#[test]
fn total_native_encoded_argv_ceiling_is_checked_before_syntax_and_copy() {
    let text = "x".repeat(super::super::INSTRUCTION_BYTES);
    rejected(&[&text, &text, &text, &text], SelectionError::Resource);
}
#[test]
fn native_argument_count_ceiling_precedes_parsing_or_borrowed_retention() {
    let values = vec![OsString::from("x"); ARGUMENTS + 1];
    assert_eq!(Selected::select(&values).err(), Some(SelectionError::Count));
}
#[test]
fn exactly_128_split_attachments_are_admitted_without_reading_any_file() {
    let mut arguments = vec![OsString::from("goal")];
    for _ in 0..super::super::FILES {
        arguments.extend(argv(&["--file", "not-read.txt"]));
    }
    let selected = Selected::select(&arguments).unwrap();
    assert_eq!(selected.files.len(), 128);
    assert!(selected.files.iter().all(|p| *p == "not-read.txt"));
}
#[test]
fn attachment_129_refuses_for_argument_file_and_stdin_sources() {
    for source in [vec!["goal"], vec!["--prompt-file=goal"], vec!["-"]] {
        let mut arguments = argv(&source);
        arguments.extend((0..129).map(|_| OsString::from("--file=x")));
        assert_eq!(
            Selected::select(&arguments).err(),
            Some(SelectionError::Input(InputError::SelectionLimit))
        );
    }
}
#[test]
fn exact_original_path_scalar_and_utf8_byte_bounds_pass_without_io() {
    let path = "🦀".repeat(super::super::PATH_SCALARS);
    original(
        &["goal", &format!("--file={path}")],
        Some("goal"),
        None,
        &[&path],
    );
}
#[test]
fn overlong_original_paths_are_not_shortened_or_normalized_into_selection() {
    let path = "x".repeat(super::super::PATH_SCALARS + 1);
    for arguments in [
        vec![format!("--prompt-file={path}")],
        vec!["goal".into(), format!("--file={path}")],
    ] {
        let native = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            Selected::select(&native).err(),
            Some(SelectionError::Input(InputError::SelectionLimit))
        );
    }
}
#[test]
fn native_non_utf8_argument_is_rejected_without_lossy_conversion() {
    #[cfg(unix)]
    let invalid = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![0xff])
    };
    #[cfg(windows)]
    let invalid = {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0xd800])
    };
    assert_eq!(
        Selected::select(&[invalid]).err(),
        Some(SelectionError::Utf8)
    );
}
#[test]
fn error_display_is_fixed_even_when_arguments_contain_secret_control_text() {
    let arguments = argv(&["goal", "--unknown=secret\u{001b}::warning::"]);
    let error = Selected::select(&arguments).err().unwrap();
    assert_eq!(error.to_string(), "[REDACTED]");
    assert!(!format!("{error:?}").contains("secret"));
    assert_eq!(Error::Arguments(error).to_string(), "[REDACTED]");
}

const KEY: &str = "argv-routing-captured-synthetic-credential";
const CAP: usize = 1024 * 1024;
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
        for directory in [&workspace, &config, &output] {
            std::fs::create_dir(directory).unwrap();
        }
        let f = Self {
            _root: root,
            workspace,
            config,
            output,
            server: MockServer::start().await,
        };
        f.configure();
        f
    }
    fn configure(&self) {
        std::fs::write(
            self.config.join("config.json"),
            json!({"model":"synthetic-model", "api_key":KEY, "base_url":self.server.uri()})
                .to_string(),
        )
        .unwrap();
    }
    fn options(&self, session: SessionSelection) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session,
            selectors: Selectors {
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
        }
    }
    fn destination(&self, format: Format) -> Destination {
        Destination {
            directory: self.output.clone(),
            file_name: "result.out".into(),
            format,
            max_bytes: CAP,
        }
    }
    fn file(&self, name: &str, content: impl AsRef<[u8]>) {
        let path = self.workspace.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    fn no_store(&self) {
        for name in ["threads.db", "sessions-rs", "gadgets/.sdk"] {
            assert!(!self.config.join(name).exists());
        }
    }
    async fn no_effects(&self) {
        self.no_store();
        assert!(std::fs::read_dir(&self.output).unwrap().next().is_none());
        assert!(self.server.received_requests().await.unwrap().is_empty());
    }
    async fn respond(&self, output: &str) {
        let event = json!({"choices":[{"delta":{"content":output},"finish_reason":"stop"}]});
        self.event(event).await;
    }
    async fn event(&self, event: Value) {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {event}\n\ndata: [DONE]\n\n"),
                "text/event-stream",
            ))
            .mount(&self.server)
            .await;
    }
    async fn inputs(&self) -> Vec<String> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| {
                let value: Value = serde_json::from_slice(&request.body).unwrap();
                value["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["role"] == "user")
                    .unwrap()["content"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }
    async fn input(&self) -> String {
        let inputs = self.inputs().await;
        assert_eq!(inputs.len(), 1);
        inputs.into_iter().next().unwrap()
    }
    fn output_value(&self) -> Value {
        let bytes = std::fs::read(self.output.join("result.out")).unwrap();
        assert!(bytes.is_ascii());
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
        serde_json::from_slice(&bytes).unwrap()
    }
}
#[tokio::test]
async fn invalid_argument_grammar_precedes_config_scope_or_storage_inspection() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("config.json"), b"invalid config").unwrap();
    let mut options = f.options(SessionSelection::Fresh);
    options.workspace = f.workspace.join("missing");
    assert_eq!(
        Invocation::prepare(options, &argv(&["-", "--timeout=1"]))
            .await
            .err(),
        Some(Error::Arguments(SelectionError::Flag))
    );
    f.no_effects().await;
}
#[tokio::test]
async fn valid_argument_grammar_still_requires_actual_bootstrap_before_any_source_or_session() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("config.json"), b"invalid config").unwrap();
    for input in ["goal", "--prompt-file=missing.txt", "-"] {
        let arguments = argv(&[input]);
        let memory = Invocation::prepare(f.options(SessionSelection::Fresh), &arguments)
            .await
            .err()
            .unwrap();
        let file = FileInvocation::prepare(
            f.options(SessionSelection::Fresh),
            &arguments,
            f.destination(Format::AsciiFragment),
        )
        .await
        .err()
        .unwrap();
        if input == "-" {
            assert_eq!(
                memory,
                Error::Stdin(StdinPreparationError::Preparation(PreparationError::Config))
            );
            assert_eq!(
                file,
                Error::StdinOutput(StdinOutputPreparationError::Stdin(
                    StdinPreparationError::Preparation(PreparationError::Config)
                ))
            );
        } else {
            assert_eq!(
                memory,
                Error::Input(InputPreparationError::Preparation(PreparationError::Config))
            );
            assert_eq!(
                file,
                Error::Output(OutputPreparationError::Input(
                    InputPreparationError::Preparation(PreparationError::Config)
                ))
            );
        }
        f.no_effects().await;
    }
}

#[tokio::test]
async fn file_argument_conflict_precedes_invalid_destination_and_bootstrap() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("config.json"), b"invalid config").unwrap();
    let mut destination = f.destination(Format::AsciiFragment);
    destination.directory = PathBuf::from("relative");
    assert_eq!(
        FileInvocation::prepare(
            f.options(SessionSelection::Fresh),
            &argv(&["-", "--prompt-file=goal"]),
            destination
        )
        .await
        .err(),
        Some(Error::Arguments(SelectionError::Input(
            InputError::ConflictingSources
        )))
    );
    f.no_effects().await;
}
#[tokio::test]
async fn invalid_flags_leave_an_existing_bad_store_and_foreign_output_untouched() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("threads.db"), b"existing bad store").unwrap();
    std::fs::write(f.output.join("result.out"), b"foreign output").unwrap();
    assert_eq!(
        FileInvocation::prepare(
            f.options(SessionSelection::Fresh),
            &argv(&["goal", "--output-format=json"]),
            f.destination(Format::AsciiFragment)
        )
        .await
        .err(),
        Some(Error::Arguments(SelectionError::Flag))
    );
    assert_eq!(
        std::fs::read(f.config.join("threads.db")).unwrap(),
        b"existing bad store"
    );
    assert_eq!(
        std::fs::read(f.output.join("result.out")).unwrap(),
        b"foreign output"
    );
    assert!(f.server.received_requests().await.unwrap().is_empty());
    assert!(!f.config.join("sessions-rs").exists() && !f.config.join("gadgets/.sdk").exists());
}
#[tokio::test]
async fn actual_literal_argv_uses_original_input_and_the_same_runtime_builder() {
    let f = Fixture::new().await;
    let prompt = "!touch forbidden\r\n中文 ::warning::";
    f.respond("done").await;
    let invocation = Invocation::prepare(f.options(SessionSelection::Ephemeral), &argv(&[prompt]))
        .await
        .unwrap();
    f.no_effects().await;
    let run = invocation.run().await;
    assert!(run.outcome().is_ok());
    assert_eq!(run.facts().instruction().kind(), OriginKind::Argument);
    assert_eq!(run.facts().instruction().bytes(), prompt.len());
    assert_eq!(f.input().await, prompt);
    assert!(
        run.retained_state()
            .messages
            .iter()
            .any(|m| matches!(m, Msg::User { content } if content == prompt))
    );
    assert!(!f.workspace.join("forbidden").exists());
    f.no_store();
}
#[tokio::test]
async fn split_prompt_file_uses_actual_confined_original_bytes_and_one_bom() {
    let f = Fixture::new().await;
    let raw = "\u{feff}original file goal\r\n";
    f.file("goal.txt", raw);
    f.respond("done").await;
    let run = Invocation::prepare(
        f.options(SessionSelection::Ephemeral),
        &argv(&["--prompt-file", "goal.txt"]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(run.outcome().is_ok());
    assert_eq!(run.facts().instruction().kind(), OriginKind::WorkspaceFile);
    assert_eq!(run.facts().instruction().path(), Some("goal.txt"));
    assert_eq!(run.facts().instruction().bytes(), raw.len());
    assert!(run.facts().instruction().bom_removed());
    assert_eq!(f.input().await, "original file goal\r\n");
    f.no_store();
}
#[tokio::test]
async fn joined_minus_prompt_file_reads_the_filename_not_process_stdin() {
    let f = Fixture::new().await;
    f.file("-", "file literally named minus");
    f.respond("done").await;
    let run = Invocation::prepare(
        f.options(SessionSelection::Ephemeral),
        &argv(&["--prompt-file=-"]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(run.facts().instruction().kind(), OriginKind::WorkspaceFile);
    assert_eq!(run.facts().instruction().path(), Some("-"));
    assert_eq!(f.input().await, "file literally named minus");
    f.no_store();
}
#[tokio::test]
async fn mixed_argv_attachments_preserve_labels_duplicates_bom_crlf_and_original_facts() {
    let f = Fixture::new().await;
    f.file("資料/a.txt", "\u{feff}original data\r\n");
    f.file("-", "minus attachment");
    f.respond("done").await;
    let run = Invocation::prepare(
        f.options(SessionSelection::Ephemeral),
        &argv(&[
            "--file",
            "資料/a.txt",
            "goal",
            "--file=-",
            "--file=資料/a.txt",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(run.outcome().is_ok());
    let facts = run.facts();
    assert_eq!(facts.attachments().len(), 3);
    assert_eq!(facts.attachments()[0].path(), Some("資料/a.txt"));
    assert_eq!(facts.attachments()[1].path(), Some("-"));
    assert_eq!(facts.attachments()[2].path(), Some("資料/a.txt"));
    assert_eq!(
        facts.attachment_bytes(),
        2 * "\u{feff}original data\r\n".len() + "minus attachment".len()
    );
    assert!(!facts.attachments()[0].bom_removed());
    let input = f.input().await;
    assert!(input.starts_with("goal\n\n<craftsman_input_attachments>"));
    assert_eq!(input.matches("\u{feff}original data\r\n").count(), 2);
    f.no_store();
}
#[tokio::test]
async fn missing_or_invalid_utf8_instruction_file_refuses_before_session_or_provider() {
    let f = Fixture::new().await;
    for file in ["missing.txt", "invalid.txt"] {
        if file == "invalid.txt" {
            f.file(file, [0xff]);
        }
        let error = Invocation::prepare(
            f.options(SessionSelection::Fresh),
            &argv(&[&format!("--prompt-file={file}")]),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(
            error,
            Error::Input(InputPreparationError::Input(
                InputError::Source | InputError::Utf8
            ))
        ));
        f.no_effects().await;
    }
}
#[tokio::test]
async fn actual_protected_attachment_denial_is_not_bypassed_by_argument_routing() {
    let f = Fixture::new().await;
    f.file(".env", "protected source");
    assert_eq!(
        Invocation::prepare(
            f.options(SessionSelection::Fresh),
            &argv(&["goal", "--file=.env"])
        )
        .await
        .err(),
        Some(Error::Input(InputPreparationError::Input(
            InputError::Source
        )))
    );
    f.no_effects().await;
}
#[tokio::test]
async fn actual_file_argv_keeps_the_existing_ascii_run_terminal_record_and_publisher() {
    let f = Fixture::new().await;
    f.respond(&format!("中文 🦀 {KEY}")).await;
    let invocation = FileInvocation::prepare(
        f.options(SessionSelection::Ephemeral),
        &argv(&["goal"]),
        f.destination(Format::AsciiFragment),
    )
    .await
    .unwrap();
    f.no_effects().await;
    let run = invocation.run().await;
    assert!(run.run_for_tests().outcome().is_ok());
    assert_eq!(
        run.run_for_tests().facts().instruction().kind(),
        OriginKind::Argument
    );
    let mut terminal = run.terminal();
    assert!(std::fs::read_dir(&f.output).unwrap().next().is_none());
    terminal.publish().unwrap();
    let value = f.output_value();
    assert_eq!(value["final_output"], "中文 🦀 [REDACTED]");
    assert!(value.get("status").is_none() && value.get("usage").is_none());
    assert_eq!(
        terminal.publish().unwrap_err().kind(),
        OutputError::AlreadyAttempted
    );
    f.no_store();
}
#[tokio::test]
async fn instruction_file_argv_keeps_original_filename_through_guarded_human_preview() {
    let f = Fixture::new().await;
    f.file("goal.txt", "original file goal");
    f.respond("::warning:: 中文").await;
    let run = FileInvocation::prepare(
        f.options(SessionSelection::Ephemeral),
        &argv(&["--prompt-file=goal.txt"]),
        f.destination(Format::HumanPreview),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(
        run.run_for_tests().facts().instruction().kind(),
        OriginKind::WorkspaceFile
    );
    assert_eq!(
        run.run_for_tests().facts().instruction().path(),
        Some("goal.txt")
    );
    run.terminal().publish().unwrap();
    let bytes = std::fs::read(f.output.join("result.out")).unwrap();
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .starts_with(":\\u{003a}warning:\\u{003a} 中文\n")
    );
    f.no_store();
}
#[tokio::test]
async fn actual_typed_approval_failed_partial_run_is_not_changed_into_completion() {
    let f = Fixture::new().await;
    f.event(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"protected","type":"function","function":{"name":"write_file","arguments":"{\"file_path\":\"build.rs\",\"content\":\"data\"}"}}]},"finish_reason":"tool_calls"}]})).await;
    let mut options = f.options(SessionSelection::Ephemeral);
    options.selectors.allow = vec!["tool:write_file".into()];
    let run = FileInvocation::prepare(
        options,
        &argv(&["goal"]),
        f.destination(Format::AsciiFragment),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(matches!(
        run.run_for_tests().outcome(),
        Err(RuntimeError::ApprovalRequired)
    ));
    assert!(!f.workspace.join("build.rs").exists());
    run.terminal().publish().unwrap();
    let value = f.output_value();
    assert_eq!(value["run_return"], "approval_required");
    assert_eq!(value["approval_required"], true);
    assert!(!value["messages"].as_array().unwrap().is_empty());
    f.no_store();
}
#[tokio::test]
async fn actual_provider_failure_keeps_original_partial_state_and_fixed_file_diagnostic() {
    let f = Fixture::new().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_raw(format!("raw {KEY}\u{001b} ::warning::"), "text/plain"),
        )
        .mount(&f.server)
        .await;
    let run = FileInvocation::prepare(
        f.options(SessionSelection::Ephemeral),
        &argv(&["goal"]),
        f.destination(Format::HumanPreview),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(matches!(
        run.run_for_tests().outcome(),
        Err(RuntimeError::Turn(_))
    ));
    assert!(
        run.run_for_tests()
            .retained_state()
            .messages
            .iter()
            .any(|m| matches!(m, Msg::User { content } if content == "goal"))
    );
    run.terminal().publish().unwrap();
    assert_eq!(
        std::fs::read(f.output.join("result.out")).unwrap(),
        b"craftsman: private run return=turn_failed; approval_required=false; final_answer=absent\n"
    );
    f.no_store();
}
#[tokio::test]
async fn existing_final_file_is_preserved_without_runtime_or_session_effects() {
    let f = Fixture::new().await;
    std::fs::write(f.output.join("result.out"), b"foreign final").unwrap();
    assert_eq!(
        FileInvocation::prepare(
            f.options(SessionSelection::Fresh),
            &argv(&["goal"]),
            f.destination(Format::AsciiFragment)
        )
        .await
        .err(),
        Some(Error::Output(OutputPreparationError::Output(
            OutputError::Exists
        )))
    );
    assert_eq!(
        std::fs::read(f.output.join("result.out")).unwrap(),
        b"foreign final"
    );
    assert!(f.server.received_requests().await.unwrap().is_empty());
    f.no_store();
}
#[tokio::test]
async fn borrowed_argument_storage_is_not_reloaded_after_preparation() {
    let f = Fixture::new().await;
    f.file("goal.txt", "original captured goal");
    f.respond("done").await;
    let mut arguments = argv(&["--prompt-file=goal.txt"]);
    let invocation = Invocation::prepare(f.options(SessionSelection::Ephemeral), &arguments)
        .await
        .unwrap();
    arguments[0] = OsString::from("--prompt-file=other.txt");
    f.file("goal.txt", "later source replacement");
    f.file("other.txt", "other source");
    let run = invocation.run().await;
    assert!(run.outcome().is_ok());
    assert_eq!(f.input().await, "original captured goal");
    assert_eq!(run.facts().instruction().path(), Some("goal.txt"));
    f.no_store();
}

const HELPER: &str = "exec::input::arguments::tests::native_arguments_helper";
const MARKER: &str = "--craftsman-private-input-arguments-fixture";
const NATIVE: &[u8] = b"original native argv goal\r\n";
#[tokio::test]
#[ignore = "native subprocess only; argv marker required before any stdin/config inspection"]
async fn native_arguments_helper() {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(index) = args.iter().position(|a| a == MARKER) else {
        return;
    };
    let Some(scenario) = args.get(index + 1) else {
        return;
    };
    if !matches!(
        scenario.as_str(),
        "memory" | "file" | "invalid-flag" | "conflict" | "bad-target" | "literal"
    ) {
        return;
    }
    let f = Fixture::new().await;
    if scenario == "invalid-flag" {
        assert_eq!(
            Invocation::prepare(
                f.options(SessionSelection::Fresh),
                &argv(&["-", "--timeout=1"])
            )
            .await
            .err(),
            Some(Error::Arguments(SelectionError::Flag))
        );
        f.no_effects().await;
    } else if scenario == "conflict" {
        assert_eq!(
            FileInvocation::prepare(
                f.options(SessionSelection::Fresh),
                &argv(&["-", "--prompt-file=goal"]),
                f.destination(Format::AsciiFragment)
            )
            .await
            .err(),
            Some(Error::Arguments(SelectionError::Input(
                InputError::ConflictingSources
            )))
        );
        f.no_effects().await;
    } else if scenario == "bad-target" {
        let mut destination = f.destination(Format::AsciiFragment);
        destination.file_name = ".env".into();
        assert_eq!(
            FileInvocation::prepare(
                f.options(SessionSelection::Fresh),
                &argv(&["-"]),
                destination
            )
            .await
            .err(),
            Some(Error::StdinOutput(StdinOutputPreparationError::Output(
                OutputError::Path
            )))
        );
        f.no_effects().await;
    } else if scenario == "literal" {
        f.respond("done").await;
        let run = Invocation::prepare(
            f.options(SessionSelection::Ephemeral),
            &argv(&["original literal goal"]),
        )
        .await
        .unwrap()
        .run()
        .await;
        assert_eq!(run.facts().instruction().kind(), OriginKind::Argument);
        assert_eq!(f.input().await, "original literal goal");
    }
    if scenario != "literal" {
        f.respond("done").await;
    }
    // Preserve EVERY real provider request. The literal case has one actual
    // earlier call; native preparation must not add a request, then its run
    // adds exactly one. No mock/request-history reset or invented zero count.
    let prior_inputs = f.inputs().await;
    assert_eq!(prior_inputs.len(), usize::from(scenario == "literal"));
    if scenario == "literal" {
        assert_eq!(prior_inputs, ["original literal goal"]);
    }
    if scenario == "file" {
        let invocation = FileInvocation::prepare(
            f.options(SessionSelection::Ephemeral),
            &argv(&["--", "-"]),
            f.destination(Format::AsciiFragment),
        )
        .await
        .unwrap();
        f.no_effects().await;
        let run = invocation.run().await;
        assert!(run.run_for_tests().outcome().is_ok());
        assert_eq!(
            run.run_for_tests().facts().instruction().kind(),
            OriginKind::Stdin
        );
        assert_eq!(
            run.run_for_tests().facts().instruction().bytes(),
            NATIVE.len()
        );
        assert_eq!(f.input().await, std::str::from_utf8(NATIVE).unwrap());
        run.terminal().publish().unwrap();
        assert_eq!(f.output_value()["final_output"], "done");
    } else {
        let invocation = Invocation::prepare(f.options(SessionSelection::Ephemeral), &argv(&["-"]))
            .await
            .unwrap();
        f.no_store();
        assert!(std::fs::read_dir(&f.output).unwrap().next().is_none());
        assert_eq!(f.inputs().await, prior_inputs);
        let run = invocation.run().await;
        assert!(run.outcome().is_ok());
        assert_eq!(run.facts().instruction().kind(), OriginKind::Stdin);
        assert_eq!(run.facts().instruction().bytes(), NATIVE.len());
        let mut expected = prior_inputs;
        expected.push(std::str::from_utf8(NATIVE).unwrap().to_owned());
        assert_eq!(f.inputs().await, expected);
    }
    f.no_store();
    println!("ARGV_NATIVE_CAPTURE_COMPLETE");
}
async fn subprocess(scenario: &str, marker: bool, write: bool) -> std::process::Output {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        HELPER,
        "--nocapture",
        "--test-threads=1",
    ]);
    if marker {
        command.args(["--", MARKER, scenario]);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let mut input = Some(child.stdin.take().unwrap());
    if write {
        use tokio::io::AsyncWriteExt;
        input.as_mut().unwrap().write_all(NATIVE).await.unwrap();
        drop(input.take());
    }
    let output = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output())
        .await
        .expect("native helper setup/capture bound, not a widened production deadline")
        .unwrap();
    drop(input);
    assert!(
        output.status.success(),
        "native helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
#[tokio::test]
async fn actual_process_stdin_argv_reaches_original_memory_invocation_chain() {
    let output = subprocess("memory", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("ARGV_NATIVE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn actual_process_stdin_argv_reaches_original_guarded_file_invocation_chain() {
    let output = subprocess("file", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("ARGV_NATIVE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn invalid_full_frontend_flag_does_not_consume_real_native_stdin() {
    let output = subprocess("invalid-flag", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("ARGV_NATIVE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn source_conflict_does_not_consume_real_native_stdin_or_capture_output() {
    let output = subprocess("conflict", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("ARGV_NATIVE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn guarded_destination_refusal_leaves_real_native_stdin_for_later_admission() {
    let output = subprocess("bad-target", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("ARGV_NATIVE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn unnamed_pipe_is_unread_by_literal_argv_then_available_for_named_stdin() {
    let output = subprocess("literal", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("ARGV_NATIVE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn unmarked_ignored_argument_helper_never_inspects_stdin_or_config() {
    let output = subprocess("memory", false, false).await;
    assert!(!String::from_utf8_lossy(&output.stdout).contains("ARGV_NATIVE_CAPTURE_COMPLETE"));
}
