use super::*;
use crate::checkpoints::LibsqlCheckpointer;
use crate::exec::construction::{
    InputPreparationError, OutputPreparationError, PreparationError, SessionSelection,
    StdinOutputPreparationError, StdinPreparationError,
};
use crate::exec::input::{InputError, OriginKind, arguments};
use crate::exec::permissions::{Mode, OnUnapproved, ToolDecision, UsageError};
use crate::exec::runtime::RuntimeError;
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn argv(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn defaults() -> Defaults {
    #[cfg(unix)]
    let (workspace, config) = ("/explicit-launch", "/explicit-config");
    #[cfg(windows)]
    let (workspace, config) = (r"C:\explicit-launch", r"C:\explicit-config");
    Defaults {
        workspace: workspace.into(),
        config_dir: config.into(),
    }
}
fn options() -> FileOptions {
    FileOptions {
        format: Format::AsciiFragment,
        max_bytes: output::CAP,
    }
}
fn output_path() -> &'static str {
    #[cfg(unix)]
    {
        "/external-output/result.out"
    }
    #[cfg(windows)]
    {
        r"C:\external-output\result.out"
    }
}
fn file_argv(values: &[&str]) -> Vec<OsString> {
    let mut args = argv(values);
    args.extend(argv(&["--output-file", output_path()]));
    args
}
fn reject(values: &[&str], expected: SelectionError) {
    assert_eq!(
        Selected::select(defaults(), &argv(values), None).err(),
        Some(expected)
    );
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
fn native_argument_limits_add_only_one_output_path_to_original_bounds() {
    assert_eq!(bounds(), (1430, 1048608, 11598320));
}
#[test]
fn native_count_is_checked_before_utf8_decoding_or_copies() {
    let mut args = vec![OsString::from("x"); bounds().0 + 1];
    args[0] = invalid_native();
    assert_eq!(
        Selected::select(defaults(), &args, None).err(),
        Some(SelectionError::Count)
    );
}
#[test]
fn native_token_and_aggregate_byte_limits_precede_grammar() {
    for args in [
        vec![OsString::from("x".repeat(bounds().1 + 1))],
        vec![OsString::from("x".repeat(super::super::INSTRUCTION_BYTES)); 12],
    ] {
        assert_eq!(
            Selected::select(defaults(), &args, None).err(),
            Some(SelectionError::Resource)
        );
    }
}
#[test]
fn whole_original_native_utf8_admission_precedes_path_projection() {
    let mut args = argv(&["--cwd=repo", "goal"]);
    args.push(invalid_native());
    assert_eq!(
        Selected::select(defaults(), &args, None).err(),
        Some(SelectionError::Utf8)
    );
}
#[test]
fn explicit_default_native_bounds_precede_utf8_inspection() {
    let mut d = defaults();
    let mut raw = invalid_native();
    raw.push("x".repeat(super::super::PATH_BYTES + 1));
    d.workspace = raw.into();
    assert_eq!(
        Selected::select(d, &argv(&["goal"]), None).err(),
        Some(SelectionError::Resource)
    );
    let mut d = defaults();
    d.config_dir = invalid_native().into();
    assert_eq!(
        Selected::select(d, &argv(&["goal"]), None).err(),
        Some(SelectionError::Utf8)
    );
}
#[test]
fn absent_scope_flags_keep_original_absolute_defaults_and_arguments() {
    let selected = Selected::select(defaults(), &argv(&["--ephemeral", "goal"]), None).unwrap();
    assert_eq!(selected.defaults.workspace, defaults().workspace);
    assert_eq!(selected.defaults.config_dir, defaults().config_dir);
    assert_eq!(selected.arguments, argv(&["--ephemeral", "goal"]));
    assert!(selected.destination.is_none());
}
#[test]
fn relative_scopes_resolve_against_original_explicit_launch_base() {
    let selected = Selected::select(
        defaults(),
        &argv(&["--cwd=repo", "--config-dir", "config", "goal"]),
        None,
    )
    .unwrap();
    assert_eq!(
        selected.arguments,
        vec![
            OsString::from(format!(
                "--cwd={}",
                defaults().workspace.join("repo").display()
            )),
            OsString::from("--config-dir"),
            defaults().workspace.join("config").into_os_string(),
            OsString::from("goal")
        ]
    );
    assert_eq!(selected.defaults.workspace, defaults().workspace);
    assert_eq!(selected.defaults.config_dir, defaults().config_dir);
}
#[test]
fn config_resolution_is_independent_of_cwd_option_order() {
    for args in [
        argv(&["--cwd=repo", "--config-dir=config", "goal"]),
        argv(&["--config-dir=config", "--cwd=repo", "goal"]),
    ] {
        let selected = Selected::select(defaults(), &args, None).unwrap();
        assert!(selected.arguments.contains(&OsString::from(format!(
            "--config-dir={}",
            defaults().workspace.join("config").display()
        ))));
        assert!(!selected.arguments.contains(&OsString::from(format!(
            "--config-dir={}",
            defaults().workspace.join("repo").join("config").display()
        ))));
    }
}
#[test]
fn native_append_preserves_relative_spelling_and_joined_or_split_form() {
    let relative = format!("repo{}nested", std::path::MAIN_SEPARATOR);
    let selected =
        Selected::select(defaults(), &argv(&["--cwd", &relative, "goal"]), None).unwrap();
    let mut expected = defaults().workspace.into_os_string();
    expected.push(std::path::MAIN_SEPARATOR_STR);
    expected.push(&relative);
    assert_eq!(
        selected.arguments,
        vec![OsString::from("--cwd"), expected, OsString::from("goal")]
    );
}
#[test]
fn existing_base_separator_is_not_rewritten_or_doubled() {
    let mut d = defaults();
    let mut raw = d.workspace.into_os_string();
    raw.push(std::path::MAIN_SEPARATOR_STR);
    d.workspace = raw.clone().into();
    let selected = Selected::select(d, &argv(&["--cwd", "repo", "goal"]), None).unwrap();
    raw.push("repo");
    assert_eq!(selected.arguments[1], raw);
}
#[test]
fn absolute_overrides_remain_original_data_not_relative_children() {
    let absolute = defaults().config_dir;
    let selected = Selected::select(
        defaults(),
        &argv(&["goal", &format!("--cwd={}", absolute.display())]),
        None,
    )
    .unwrap();
    assert_eq!(
        selected.arguments[1],
        OsString::from(format!("--cwd={}", absolute.display()))
    );
}
#[test]
fn dots_parents_empty_and_control_scopes_are_refused_not_normalized() {
    for value in [
        "",
        ".",
        "..",
        "repo/.",
        "repo/../other",
        "./repo",
        "repo/../../elsewhere",
        "repo\0child",
        "repo\nchild",
    ] {
        reject(&["goal", &format!("--cwd={value}")], SelectionError::Scope);
        reject(
            &["goal", &format!("--config-dir={value}")],
            SelectionError::Scope,
        );
    }
}
#[test]
fn foreign_drive_root_only_unc_and_device_spellings_never_use_ambient_roots() {
    for value in [
        "C:relative",
        r"\root-only",
        r"\\server\share",
        r"\\.\device",
        "//server/share",
    ] {
        reject(&["goal", &format!("--cwd={value}")], SelectionError::Scope);
    }
}
#[test]
fn original_and_expanded_scope_scalar_byte_and_depth_bounds_are_independent() {
    for value in [
        "a".repeat(super::super::PATH_SCALARS),
        "🦀".repeat(super::super::PATH_SCALARS),
        std::iter::repeat_n("x", 129)
            .collect::<Vec<_>>()
            .join(std::path::MAIN_SEPARATOR_STR),
    ] {
        reject(&["goal", &format!("--cwd={value}")], SelectionError::Scope);
    }
    let mut d = defaults();
    d.workspace = PathBuf::from("relative");
    assert_eq!(
        Selected::select(d, &argv(&["goal"]), None).err(),
        Some(SelectionError::Scope)
    );
}
#[test]
fn unicode_space_quote_equal_and_option_looking_relative_names_stay_exact() {
    let name = "repo 中文 = \"quoted\"";
    let selected =
        Selected::select(defaults(), &argv(&[&format!("--cwd={name}"), "goal"]), None).unwrap();
    assert_eq!(
        selected.arguments[0],
        OsString::from(format!(
            "--cwd={}",
            defaults().workspace.join(name).display()
        ))
    );
    let selected = Selected::select(
        defaults(),
        &argv(&["--cwd=--config-dir=literal", "goal"]),
        None,
    )
    .unwrap();
    assert_eq!(
        selected.arguments[0],
        OsString::from(format!(
            "--cwd={}",
            defaults().workspace.join("--config-dir=literal").display()
        ))
    );
}
#[test]
fn repeated_scope_or_destination_flags_refuse_instead_of_last_wins() {
    reject(&["goal", "--cwd=a", "--cwd=b"], SelectionError::Duplicate);
    reject(
        &["goal", "--config-dir=a", "--config-dir=a"],
        SelectionError::Duplicate,
    );
    let args = file_argv(&["goal", "--output-file=second"]);
    assert_eq!(
        Selected::select(defaults(), &args, Some(options())).err(),
        Some(SelectionError::Duplicate)
    );
}
#[test]
fn missing_or_option_looking_scope_and_output_split_values_refuse() {
    for flag in ["--cwd", "--config-dir", "--output-file"] {
        reject(&["goal", flag], SelectionError::Value);
        reject(&["goal", flag, "--ephemeral"], SelectionError::Value);
    }
}
#[test]
fn memory_route_never_accepts_or_ignores_a_file_destination() {
    reject(
        &["goal", "--output-file=anything"],
        SelectionError::UnexpectedDestination,
    );
}
#[test]
fn file_route_requires_an_explicit_destination_before_preparation() {
    assert_eq!(
        Selected::select(defaults(), &argv(&["goal"]), Some(options())).err(),
        Some(SelectionError::MissingDestination)
    );
}
#[test]
fn output_file_flag_selects_private_destination_not_public_format_or_data() {
    let selected = Selected::select(defaults(), &file_argv(&["goal"]), Some(options())).unwrap();
    let d = selected.destination.unwrap();
    assert_eq!(d.directory, Path::new(output_path()).parent().unwrap());
    assert_eq!(d.file_name, "result.out");
    assert_eq!(d.format, Format::AsciiFragment);
    assert_eq!(d.max_bytes, output::CAP);
    assert_eq!(selected.arguments, argv(&["goal"]));
}
#[test]
fn joined_and_split_output_file_paths_have_identical_original_destination() {
    let a = Selected::select(defaults(), &file_argv(&["goal"]), Some(options()))
        .unwrap()
        .destination
        .unwrap();
    let b = Selected::select(
        defaults(),
        &argv(&["goal", &format!("--output-file={}", output_path())]),
        Some(options()),
    )
    .unwrap()
    .destination
    .unwrap();
    assert_eq!(a.directory, b.directory);
    assert_eq!(a.file_name, b.file_name);
}
#[test]
fn relative_output_paths_use_selected_workspace_not_original_launch_base() {
    for args in [
        argv(&["--cwd=repo", "goal", "--output-file=reports/result.out"]),
        argv(&["--output-file=reports/result.out", "goal", "--cwd=repo"]),
    ] {
        let d = Selected::select(defaults(), &args, Some(options()))
            .unwrap()
            .destination
            .unwrap();
        assert_eq!(
            d.directory,
            defaults().workspace.join("repo").join("reports")
        );
        assert_eq!(d.file_name, "result.out");
    }
}
#[test]
fn one_name_and_minus_output_values_are_filenames_never_stdout_selection() {
    for name in ["result.out", "-"] {
        let d = Selected::select(
            defaults(),
            &argv(&["--cwd=repo", "goal", "--output-file", name]),
            Some(options()),
        )
        .unwrap()
        .destination
        .unwrap();
        assert_eq!(d.directory, defaults().workspace.join("repo"));
        assert_eq!(d.file_name, name);
    }
}
#[test]
fn empty_directory_only_dot_parent_and_control_output_paths_refuse() {
    for path in [
        "",
        "reports/",
        ".",
        "..",
        "reports/../result",
        "reports/./result",
        "result\0x",
        "result\n",
    ] {
        let args = argv(&["goal", &format!("--output-file={path}")]);
        assert!(matches!(
            Selected::select(defaults(), &args, Some(options())).err(),
            Some(SelectionError::Output(output::Error::Path))
        ));
    }
}
#[test]
fn original_output_filename_and_private_budget_limits_reuse_old_grammar() {
    for options in [
        FileOptions {
            format: Format::AsciiFragment,
            max_bytes: 0,
        },
        FileOptions {
            format: Format::HumanPreview,
            max_bytes: output::CAP + 1,
        },
    ] {
        assert_eq!(
            Selected::select(defaults(), &file_argv(&["goal"]), Some(options)).err(),
            Some(SelectionError::Output(output::Error::Limit))
        );
    }
    let raw = format!(
        "{}{}",
        Path::new(output_path()).parent().unwrap().display(),
        std::path::MAIN_SEPARATOR
    );
    let accepted = argv(&["goal", &format!("--output-file={raw}{}", "x".repeat(255))]);
    assert!(Selected::select(defaults(), &accepted, Some(options())).is_ok());
    let refused = argv(&["goal", &format!("--output-file={raw}{}", "x".repeat(256))]);
    assert_eq!(
        Selected::select(defaults(), &refused, Some(options())).err(),
        Some(SelectionError::Output(output::Error::Path))
    );
}
#[test]
fn sensitive_output_names_are_not_rescued_by_scope_resolution() {
    for name in [
        ".env",
        ".craftsman",
        "build.rs",
        "Cargo.toml",
        ".craftsman-exec-tmp-foreign",
    ] {
        let raw = format!(
            "{}{}{}",
            Path::new(output_path()).parent().unwrap().display(),
            std::path::MAIN_SEPARATOR,
            name
        );
        assert_eq!(
            Selected::select(
                defaults(),
                &argv(&["goal", &format!("--output-file={raw}")]),
                Some(options())
            )
            .err(),
            Some(SelectionError::Output(output::Error::Path))
        );
    }
}
#[test]
fn public_formats_and_trace_usage_gadget_wall_legacy_flags_remain_unsupported() {
    for flag in [
        "--output-format=text",
        "--output-format=json",
        "--output-format=jsonl",
        "--trace-file=trace",
        "--max-rounds=1",
        "--token-budget=1",
        "--timeout=1",
        "--allow-gadget=demo",
        "--auto-approve",
        "-m",
        "--help",
        "--unknown",
    ] {
        let args = file_argv(&["--cwd=repo", "goal", flag]);
        assert_eq!(
            Selected::select(defaults(), &args, Some(options())).err(),
            Some(SelectionError::Controls(frontend::SelectionError::Flag))
        );
    }
}
#[test]
fn end_of_options_keeps_scope_and_output_looking_prompts_as_original_data() {
    for data in [
        "--cwd=outside",
        "--config-dir=outside",
        "--output-file=result",
        "--output-format=json",
    ] {
        let selected =
            Selected::select(defaults(), &argv(&["--cwd=repo", "--", data]), None).unwrap();
        assert_eq!(selected.arguments.last().unwrap(), data);
        assert!(selected.destination.is_none());
    }
}
#[test]
fn older_split_values_and_joined_filename_controls_are_never_rescanned() {
    for flag in [
        "--permission-mode",
        "--on-unapproved",
        "--session",
        "--allow",
        "--deny",
        "--env-pass",
        "--prompt-file",
        "--file",
    ] {
        reject(
            &["goal", flag, "--cwd=hidden"],
            SelectionError::Controls(frontend::SelectionError::Value),
        );
    }
    let selected = Selected::select(
        defaults(),
        &argv(&[
            "--cwd=repo",
            "--prompt-file=--output-file=literal",
            "--file=--cwd=literal",
        ]),
        None,
    )
    .unwrap();
    assert_eq!(selected.arguments[1], "--prompt-file=--output-file=literal");
    assert_eq!(selected.arguments[2], "--file=--cwd=literal");
}
#[test]
fn old_source_instruction_and_attachment_caps_are_not_raised_by_native_overhead() {
    reject(
        &[&"x".repeat(super::super::INSTRUCTION_BYTES + 1)],
        SelectionError::Controls(frontend::SelectionError::Input(
            InputError::InstructionLimit,
        )),
    );
    let mut args = argv(&["goal"]);
    for _ in 0..129 {
        args.push(OsString::from("--file=x"));
    }
    assert_eq!(
        Selected::select(defaults(), &args, None).err(),
        Some(SelectionError::Controls(frontend::SelectionError::Input(
            InputError::SelectionLimit
        )))
    );
}
#[test]
fn old_permission_session_and_native_source_grammar_still_precede_effects() {
    reject(
        &["--cwd=repo", "-", "--allow=tool:unknown"],
        SelectionError::Controls(frontend::SelectionError::Permissions(UsageError::Rule)),
    );
    reject(
        &["--cwd=repo", "-", "--session=bad/id"],
        SelectionError::Controls(frontend::SelectionError::Session),
    );
    reject(
        &["--cwd=repo", "goal", "--ephemeral", "--continue"],
        SelectionError::Controls(frontend::SelectionError::Duplicate),
    );
    for id in [".", ".."] {
        assert!(
            Selected::select(
                defaults(),
                &argv(&["--cwd=repo", "goal", &format!("--session={id}")]),
                None
            )
            .is_ok()
        );
    }
}
#[test]
fn selected_errors_never_echo_original_paths_values_or_credentials() {
    let error = Selected::select(
        defaults(),
        &argv(&["goal", "--cwd=SECRET\n::warning::"]),
        None,
    )
    .err()
    .unwrap();
    assert_eq!(error.to_string(), "[REDACTED]");
    assert!(!format!("{error:?}").contains("SECRET"));
    assert_eq!(Error::Selection(error).to_string(), "[REDACTED]");
}

const KEY: &str = "scope-output-argv-fixture-captured-secret";
struct Fixture {
    root: tempfile::TempDir,
    launch: PathBuf,
    workspace: PathBuf,
    config: PathBuf,
    output: PathBuf,
    server: MockServer,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let launch = root.path().canonicalize().unwrap();
        let workspace = launch.join("workspace");
        let config = launch.join("config");
        let output = launch.join("output");
        for d in [&workspace, &config, &output] {
            std::fs::create_dir(d).unwrap();
        }
        let f = Self {
            root,
            launch,
            workspace,
            config,
            output,
            server: MockServer::start().await,
        };
        f.configure();
        f
    }
    fn configure(&self) {
        std::fs::write(self.config.join("config.json"),json!({"model":"synthetic-model","api_key":KEY,"base_url":self.server.uri(),"auto_approve":true,"permission_mode":"bypass-permissions","shell_allow_list":["*"]}).to_string()).unwrap();
    }
    fn defaults(&self) -> Defaults {
        Defaults {
            workspace: self.launch.clone(),
            config_dir: self.config.clone(),
        }
    }
    fn memory(&self, values: &[&str]) -> Vec<OsString> {
        let mut args = argv(&["--cwd=workspace", "--config-dir=config", "--ephemeral"]);
        args.extend(argv(values));
        args
    }
    fn file_args(&self, values: &[&str]) -> Vec<OsString> {
        let mut args = self.memory(values);
        args.extend(argv(&[
            "--output-file",
            self.output.join("result.out").to_str().unwrap(),
        ]));
        args
    }
    fn file(&self, name: &str, data: impl AsRef<[u8]>) {
        let path = self.workspace.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, data).unwrap();
    }
    fn no_store(&self) {
        assert!(self.root.path().exists());
        for base in [&self.launch, &self.config, &self.workspace] {
            for name in ["threads.db", "sessions-rs", "gadgets/.sdk"] {
                assert!(!base.join(name).exists());
            }
        }
    }
    async fn no_effects(&self) {
        self.no_store();
        assert!(std::fs::read_dir(&self.output).unwrap().next().is_none());
        assert!(self.server.received_requests().await.unwrap().is_empty());
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
    async fn respond(&self, data: &str) {
        self.event(json!({"choices":[{"delta":{"content":data},"finish_reason":"stop"}]}))
            .await;
    }
    async fn inputs(&self) -> Vec<String> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| {
                let value: Value = serde_json::from_slice(&r.body).unwrap();
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
    async fn seeded_store(&self) -> LibsqlCheckpointer {
        LibsqlCheckpointer::open_at_for_headless(
            self.config.join("threads.db"),
            self.workspace.to_str().unwrap().to_owned(),
        )
        .await
        .unwrap()
    }
}
#[tokio::test]
async fn actual_relative_workspace_and_config_use_same_original_bootstrap() {
    let f = Fixture::new().await;
    f.respond("done").await;
    let invocation = Invocation::prepare(f.defaults(), &f.memory(&["goal"]))
        .await
        .unwrap();
    f.no_effects().await;
    let run = invocation.run().await;
    assert!(run.outcome().is_ok());
    assert_eq!(run.prepared().workspace(), f.workspace);
    assert_eq!(run.prepared().config_dir(), f.config);
    assert_eq!(f.inputs().await, ["goal"]);
    assert_eq!(run.prepared().permissions().mode(), Mode::Manual);
    assert_eq!(
        run.prepared().permissions().on_unapproved(),
        OnUnapproved::Deny
    );
    assert!(run.prepared().ignored_ambient_settings());
    assert!(run.prepared().permissions().allow_rules().is_empty());
    f.no_store();
}
#[tokio::test]
async fn relative_config_is_launch_scoped_even_when_option_order_is_reversed() {
    let f = Fixture::new().await;
    std::fs::create_dir(f.workspace.join("config")).unwrap();
    std::fs::write(
        f.workspace.join("config/config.json"),
        b"invalid distractor",
    )
    .unwrap();
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &argv(&[
            "--config-dir=config",
            "--ephemeral",
            "--cwd=workspace",
            "goal",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(run.prepared().config_dir(), f.config);
    assert_eq!(f.inputs().await, ["goal"]);
    f.no_store();
}
#[tokio::test]
async fn instruction_and_attachment_paths_use_selected_workspace_not_launch() {
    let f = Fixture::new().await;
    f.file("goal.txt", "\u{feff}selected original goal\r\n");
    f.file("data.txt", "\u{feff}selected original data\r\n");
    std::fs::write(f.launch.join("goal.txt"), b"WRONG_LAUNCH_GOAL").unwrap();
    std::fs::write(f.launch.join("data.txt"), b"WRONG_LAUNCH_DATA").unwrap();
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &f.memory(&[
            "--prompt-file=goal.txt",
            "--file=data.txt",
            "--file",
            "data.txt",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(run.facts().instruction().kind(), OriginKind::WorkspaceFile);
    assert!(run.facts().instruction().bom_removed());
    assert_eq!(run.facts().attachments().len(), 2);
    assert_eq!(run.facts().attachments()[0].path(), Some("data.txt"));
    assert!(!run.facts().attachments()[0].bom_removed());
    let inputs = f.inputs().await;
    assert!(inputs[0].starts_with("selected original goal\r\n"));
    assert_eq!(inputs[0].matches("selected original data\r\n").count(), 2);
    assert!(!inputs[0].contains("WRONG_LAUNCH"));
    f.no_store();
}
#[tokio::test]
async fn scope_output_selection_does_not_rescan_original_filename_data() {
    let f = Fixture::new().await;
    f.file("--output-file=literal", "original option-looking goal");
    f.file("--cwd=literal", "original attachment");
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &f.memory(&[
            "--prompt-file=--output-file=literal",
            "--file=--cwd=literal",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(
        run.facts().instruction().path(),
        Some("--output-file=literal")
    );
    assert_eq!(run.facts().attachments()[0].path(), Some("--cwd=literal"));
    assert!(f.inputs().await[0].starts_with("original option-looking goal"));
    f.no_store();
}
#[tokio::test]
async fn old_permission_policy_not_provisional_scope_validation_controls_runtime() {
    let f = Fixture::new().await;
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &f.memory(&[
            "--permission-mode=plan",
            "--on-unapproved=fail",
            "--allow=tool:write_file",
            "--deny=tool:write_file",
            "--env-pass=CI_JOB_ID",
            "goal",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(run.prepared().permissions().mode(), Mode::Plan);
    assert_eq!(
        run.prepared().permissions().on_unapproved(),
        OnUnapproved::Fail
    );
    assert_eq!(run.prepared().permissions().env_pass(), ["CI_JOB_ID"]);
    assert_eq!(
        run.prepared()
            .permissions()
            .tool_decision("write_file", &json!({"file_path":"x","content":"y"})),
        ToolDecision::Deny
    );
    f.no_store();
}
#[tokio::test]
async fn invalid_full_flags_permissions_and_session_ids_precede_bad_config_and_io() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("config.json"), b"invalid config").unwrap();
    for (flag, expected) in [
        ("--output-format=json", frontend::SelectionError::Flag),
        (
            "--allow=tool:unknown",
            frontend::SelectionError::Permissions(UsageError::Rule),
        ),
        ("--session=bad/id", frontend::SelectionError::Session),
    ] {
        assert_eq!(
            FileInvocation::prepare(f.defaults(), &f.file_args(&["-", flag]), options())
                .await
                .err(),
            Some(Error::Selection(SelectionError::Controls(expected)))
        );
        f.no_effects().await;
    }
}
#[tokio::test]
async fn old_configured_credential_source_denial_remains_authoritative_before_input() {
    let f = Fixture::new().await;
    std::fs::write(
        f.config.join("config.json"),
        json!({"model":"synthetic-model","api_key":"${HARMLESS_ROUTE}","base_url":f.server.uri()})
            .to_string(),
    )
    .unwrap();
    assert_eq!(
        Invocation::prepare(f.defaults(), &f.memory(&["--env-pass=HARMLESS_ROUTE", "-"]))
            .await
            .err(),
        Some(Error::Preparation(frontend::Error::Preparation(
            arguments::Error::Stdin(StdinPreparationError::Preparation(
                PreparationError::Permissions
            ))
        )))
    );
    f.no_effects().await;
}
#[tokio::test]
async fn trust_only_changes_selected_project_discovery_not_grants() {
    let f = Fixture::new().await;
    f.file(".craftsman/AGENTS.md", "SCOPED_PROJECT_CONTEXT");
    f.respond("done").await;
    let untrusted = Invocation::prepare(f.defaults(), &f.memory(&["goal"]))
        .await
        .unwrap()
        .run()
        .await;
    assert!(
        !format!("{:?}", untrusted.prepared().discovery().instructions)
            .contains("SCOPED_PROJECT_CONTEXT")
    );
    let trusted = Invocation::prepare(f.defaults(), &f.memory(&["--trust-workspace", "goal"]))
        .await
        .unwrap()
        .run()
        .await;
    assert!(
        format!("{:?}", trusted.prepared().discovery().instructions)
            .contains("SCOPED_PROJECT_CONTEXT")
    );
    assert!(trusted.prepared().permissions().allow_rules().is_empty());
    assert_eq!(f.inputs().await, ["goal", "goal"]);
    f.no_store();
}
#[tokio::test]
async fn actual_resume_and_continue_use_selected_config_and_canonical_workspace() {
    let f = Fixture::new().await;
    let store = f.seeded_store().await;
    store.create_headless_thread("z-existing").await.unwrap();
    store.create_headless_thread("a-existing").await.unwrap();
    store
        .connection()
        .execute("UPDATE threads SET updated=100", ())
        .await
        .unwrap();
    drop(store);
    f.respond("done").await;
    for selector in ["--session=a-existing", "--continue"] {
        let run = Invocation::prepare(
            f.defaults(),
            &argv(&["--cwd=workspace", "--config-dir=config", selector, "goal"]),
        )
        .await
        .unwrap()
        .run()
        .await;
        assert_eq!(run.prepared().persistence().thread_id(), Some("a-existing"));
        assert_eq!(run.prepared().workspace(), f.workspace);
        assert!(run.prepared().permissions().allow_rules().is_empty());
    }
    assert_eq!(f.inputs().await, ["goal", "goal"]);
    assert!(!f.launch.join("threads.db").exists());
}
#[tokio::test]
async fn absent_session_flag_keeps_fresh_writer_after_complete_input_admission() {
    let f = Fixture::new().await;
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &argv(&["--cwd=workspace", "--config-dir=config", "goal"]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(
        run.prepared().persistence().mode(),
        &SessionSelection::Fresh
    );
    assert!(run.prepared().persistence().thread_id().is_some());
    assert!(f.config.join("threads.db").exists());
    assert!(!f.launch.join("threads.db").exists());
}
#[tokio::test]
async fn real_output_file_flags_keep_same_guarded_ascii_record_and_one_attempt() {
    let f = Fixture::new().await;
    f.respond(&format!("中文 🦀 {KEY}")).await;
    let invocation = FileInvocation::prepare(f.defaults(), &f.file_args(&["goal"]), options())
        .await
        .unwrap();
    f.no_effects().await;
    let run = invocation.run().await;
    let mut terminal = run.terminal();
    terminal.publish().unwrap();
    let bytes = std::fs::read(f.output.join("result.out")).unwrap();
    assert!(bytes.is_ascii());
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["final_output"], "中文 🦀 [REDACTED]");
    assert!(
        value.get("status").is_none()
            && value.get("usage").is_none()
            && value.get("schema").is_none()
    );
    assert_eq!(
        terminal.publish().unwrap_err().kind(),
        output::Error::AlreadyAttempted
    );
    f.no_store();
}
#[tokio::test]
async fn typed_human_preview_is_not_a_public_text_output_format() {
    let f = Fixture::new().await;
    f.respond(&format!("::notice::中文 {KEY}")).await;
    let run = FileInvocation::prepare(
        f.defaults(),
        &f.file_args(&["goal"]),
        FileOptions {
            format: Format::HumanPreview,
            max_bytes: output::CAP,
        },
    )
    .await
    .unwrap()
    .run()
    .await;
    run.terminal().publish().unwrap();
    let text = std::fs::read_to_string(f.output.join("result.out")).unwrap();
    assert!(text.contains("中文 [REDACTED]"));
    assert!(!text.contains(KEY) && !text.contains("::"));
    f.no_store();
}
#[tokio::test]
async fn relative_workspace_output_is_physically_denied_before_stdin_or_store() {
    let f = Fixture::new().await;
    assert_eq!(
        FileInvocation::prepare(
            f.defaults(),
            &f.memory(&["-", "--output-file=result.out"]),
            options()
        )
        .await
        .err(),
        Some(Error::Preparation(frontend::Error::Preparation(
            arguments::Error::StdinOutput(StdinOutputPreparationError::Output(
                output::Error::Scope
            ))
        )))
    );
    f.no_effects().await;
    assert!(!f.workspace.join("result.out").exists());
}
#[tokio::test]
async fn selected_config_output_scope_is_still_physically_denied() {
    let f = Fixture::new().await;
    let args = f.memory(&[
        "goal",
        "--output-file",
        f.config.join("result.out").to_str().unwrap(),
    ]);
    assert_eq!(
        FileInvocation::prepare(f.defaults(), &args, options())
            .await
            .err(),
        Some(Error::Preparation(frontend::Error::Preparation(
            arguments::Error::Output(OutputPreparationError::Output(output::Error::Scope))
        )))
    );
    f.no_effects().await;
}
#[tokio::test]
async fn existing_unrelated_output_file_is_never_overwritten_or_used_as_resume() {
    let f = Fixture::new().await;
    std::fs::write(f.output.join("result.out"), b"FOREIGN_ORIGINAL").unwrap();
    assert_eq!(
        FileInvocation::prepare(f.defaults(), &f.file_args(&["goal"]), options())
            .await
            .err(),
        Some(Error::Preparation(frontend::Error::Preparation(
            arguments::Error::Output(OutputPreparationError::Output(output::Error::Exists))
        )))
    );
    assert_eq!(
        std::fs::read(f.output.join("result.out")).unwrap(),
        b"FOREIGN_ORIGINAL"
    );
    assert!(f.inputs().await.is_empty());
    f.no_store();
}
#[tokio::test]
async fn prepared_scopes_source_config_and_output_are_not_reloaded_from_mutated_argv() {
    let f = Fixture::new().await;
    f.file("goal.txt", "ORIGINAL_CAPTURED_GOAL");
    f.respond("done").await;
    let mut args = f.file_args(&["--prompt-file=goal.txt"]);
    let invocation = FileInvocation::prepare(f.defaults(), &args, options())
        .await
        .unwrap();
    args[0] = OsString::from("--cwd=elsewhere");
    args[1] = OsString::from("--config-dir=elsewhere");
    *args.last_mut().unwrap() = OsString::from(f.output.join("changed.out"));
    f.file("goal.txt", "LATER_REPLACED_GOAL");
    std::fs::write(f.config.join("config.json"), b"invalid later config").unwrap();
    let run = invocation.run().await;
    assert_eq!(f.inputs().await, ["ORIGINAL_CAPTURED_GOAL"]);
    run.terminal().publish().unwrap();
    assert!(f.output.join("result.out").exists());
    assert!(!f.output.join("changed.out").exists());
    f.no_store();
}
#[tokio::test]
async fn same_actual_fail_latch_and_partial_approval_facts_reach_old_publisher() {
    let f = Fixture::new().await;
    f.event(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"protected","type":"function","function":{"name":"write_file","arguments":"{\"file_path\":\"build.rs\",\"content\":\"data\"}"}}]},"finish_reason":"tool_calls"}]})).await;
    let run = FileInvocation::prepare(
        f.defaults(),
        &f.file_args(&["--allow=tool:write_file", "--on-unapproved=fail", "goal"]),
        options(),
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
    let value: Value =
        serde_json::from_slice(&std::fs::read(f.output.join("result.out")).unwrap()).unwrap();
    assert_eq!(value["approval_required"], true);
    assert_eq!(value["run_return"], "approval_required");
    f.no_store();
}

const HELPER: &str = "exec::input::scopes::tests::native_scope_output_arguments_helper";
const MARKER: &str = "--craftsman-private-scope-output-arguments-fixture";
const NATIVE: &[u8] = b"original named native scoped output goal\r\n";
#[tokio::test]
#[ignore = "native subprocess only; argv marker required before any stdin/config work"]
async fn native_scope_output_arguments_helper() {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(index) = args.iter().position(|a| a == MARKER) else {
        return;
    };
    let Some(scenario) = args.get(index + 1) else {
        return;
    };
    if !matches!(
        scenario.as_str(),
        "memory"
            | "file"
            | "scope"
            | "policy"
            | "session"
            | "credential"
            | "target"
            | "relative-target"
            | "existing-target"
            | "format"
            | "literal"
            | "duplicate"
    ) {
        return;
    }
    let f = Fixture::new().await;
    match scenario.as_str() {
        "scope" => {
            assert_eq!(
                Invocation::prepare(f.defaults(), &argv(&["--cwd=workspace/../elsewhere", "-"]))
                    .await
                    .err(),
                Some(Error::Selection(SelectionError::Scope))
            );
            f.no_effects().await;
        }
        "policy" => {
            assert_eq!(
                Invocation::prepare(f.defaults(), &f.memory(&["-", "--allow=tool:unknown"]))
                    .await
                    .err(),
                Some(Error::Selection(SelectionError::Controls(
                    frontend::SelectionError::Permissions(UsageError::Rule)
                )))
            );
            f.no_effects().await;
        }
        "session" => {
            assert_eq!(
                Invocation::prepare(f.defaults(), &f.memory(&["-", "--session=bad/id"]))
                    .await
                    .err(),
                Some(Error::Selection(SelectionError::Controls(
                    frontend::SelectionError::Session
                )))
            );
            f.no_effects().await;
        }
        "credential" => {
            std::fs::write(f.config.join("config.json"),json!({"model":"synthetic-model","api_key":"${HARMLESS_ROUTE}","base_url":f.server.uri()}).to_string()).unwrap();
            assert_eq!(
                Invocation::prepare(f.defaults(), &f.memory(&["-", "--env-pass=HARMLESS_ROUTE"]))
                    .await
                    .err(),
                Some(Error::Preparation(frontend::Error::Preparation(
                    arguments::Error::Stdin(StdinPreparationError::Preparation(
                        PreparationError::Permissions
                    ))
                )))
            );
            f.no_effects().await;
            f.configure();
        }
        "target" => {
            let args = f.memory(&[
                "-",
                "--output-file",
                f.output.join(".env").to_str().unwrap(),
            ]);
            assert_eq!(
                FileInvocation::prepare(f.defaults(), &args, options())
                    .await
                    .err(),
                Some(Error::Selection(SelectionError::Output(
                    output::Error::Path
                )))
            );
            f.no_effects().await;
        }
        "relative-target" => {
            assert_eq!(
                FileInvocation::prepare(
                    f.defaults(),
                    &f.memory(&["-", "--output-file=result.out"]),
                    options()
                )
                .await
                .err(),
                Some(Error::Preparation(frontend::Error::Preparation(
                    arguments::Error::StdinOutput(StdinOutputPreparationError::Output(
                        output::Error::Scope
                    ))
                )))
            );
            f.no_effects().await;
        }
        "existing-target" => {
            std::fs::write(f.output.join("result.out"), b"ORIGINAL_FOREIGN_FILE").unwrap();
            assert_eq!(
                FileInvocation::prepare(f.defaults(), &f.file_args(&["-"]), options())
                    .await
                    .err(),
                Some(Error::Preparation(frontend::Error::Preparation(
                    arguments::Error::StdinOutput(StdinOutputPreparationError::Output(
                        output::Error::Exists
                    ))
                )))
            );
            assert_eq!(
                std::fs::read(f.output.join("result.out")).unwrap(),
                b"ORIGINAL_FOREIGN_FILE"
            );
            assert!(f.inputs().await.is_empty());
            f.no_store();
        }
        "format" => {
            assert_eq!(
                FileInvocation::prepare(
                    f.defaults(),
                    &f.file_args(&["-", "--output-format=json"]),
                    options()
                )
                .await
                .err(),
                Some(Error::Selection(SelectionError::Controls(
                    frontend::SelectionError::Flag
                )))
            );
            f.no_effects().await;
        }
        "duplicate" => {
            assert_eq!(
                FileInvocation::prepare(
                    f.defaults(),
                    &f.file_args(&["-", "--output-file=second"]),
                    options()
                )
                .await
                .err(),
                Some(Error::Selection(SelectionError::Duplicate))
            );
            f.no_effects().await;
        }
        _ => {}
    }
    f.respond("done").await;
    if scenario == "literal" {
        let run = Invocation::prepare(f.defaults(), &f.memory(&["--", "--output-file=literal"]))
            .await
            .unwrap()
            .run()
            .await;
        assert_eq!(run.facts().instruction().kind(), OriginKind::Argument);
        assert!(run.prepared().permissions().allow_rules().is_empty());
        assert_eq!(f.inputs().await, ["--output-file=literal"]);
        assert!(std::fs::read_dir(&f.output).unwrap().next().is_none());
    }
    let prior = f.inputs().await;
    assert_eq!(prior.len(), usize::from(scenario == "literal"));
    if scenario == "file" {
        let invocation = FileInvocation::prepare(
            f.defaults(),
            &f.file_args(&["--on-unapproved=fail", "-"]),
            options(),
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
        run.terminal().publish().unwrap();
        assert!(
            std::fs::read(f.output.join("result.out"))
                .unwrap()
                .is_ascii()
        );
    } else {
        let invocation = Invocation::prepare(f.defaults(), &f.memory(&["--", "-"]))
            .await
            .unwrap();
        f.no_store();
        assert_eq!(f.inputs().await, prior);
        let run = invocation.run().await;
        assert!(run.outcome().is_ok());
        assert_eq!(run.facts().instruction().kind(), OriginKind::Stdin);
        assert_eq!(run.facts().instruction().bytes(), NATIVE.len());
    }
    let mut expected = prior;
    expected.push(std::str::from_utf8(NATIVE).unwrap().into());
    assert_eq!(f.inputs().await, expected);
    if scenario == "existing-target" {
        assert_eq!(
            std::fs::read(f.output.join("result.out")).unwrap(),
            b"ORIGINAL_FOREIGN_FILE"
        );
    }
    f.no_store();
    println!("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE");
}
async fn subprocess(scenario: &str, marked: bool, write: bool) -> std::process::Output {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        HELPER,
        "--nocapture",
        "--test-threads=1",
    ]);
    if marked {
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
        .expect("fixture setup/process bound, not a production stdin clock")
        .unwrap();
    drop(input);
    assert!(
        output.status.success(),
        "native fixture failure: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
#[tokio::test]
async fn real_native_relative_controls_reach_same_memory_stdin_chain() {
    let output = subprocess("memory", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn real_native_scoped_output_file_reaches_same_composed_stdin_chain() {
    let output = subprocess("file", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn real_native_bad_scope_policy_session_and_duplicate_leave_input_unread() {
    for scenario in ["scope", "policy", "session", "duplicate"] {
        let output = subprocess(scenario, true, true).await;
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE")
        );
    }
}
#[tokio::test]
async fn real_native_configured_credential_source_refuses_before_consumption() {
    let output = subprocess("credential", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn real_native_invalid_relative_and_existing_targets_leave_input_unread() {
    for scenario in ["target", "relative-target", "existing-target"] {
        let output = subprocess(scenario, true, true).await;
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE")
        );
    }
}
#[tokio::test]
async fn real_native_public_output_format_refuses_instead_of_using_private_preview() {
    let output = subprocess("format", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn real_literal_output_control_preserves_both_actual_requests_and_native_pipe() {
    let output = subprocess("literal", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn unmarked_helper_returns_before_any_config_or_open_pipe_inspection() {
    let output = subprocess("memory", false, false).await;
    assert!(!String::from_utf8_lossy(&output.stdout).contains("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE"));
}

#[tokio::test]
async fn missing_and_denied_sources_refuse_before_fresh_store_or_provider_effects() {
    let f = Fixture::new().await;
    f.file(".env", "original protected source");
    for source in ["missing.txt", ".env"] {
        let args = argv(&[
            "--cwd=workspace",
            "--config-dir=config",
            &format!("--prompt-file={source}"),
        ]);
        assert_eq!(
            Invocation::prepare(f.defaults(), &args).await.err(),
            Some(Error::Preparation(frontend::Error::Preparation(
                arguments::Error::Input(InputPreparationError::Input(InputError::Source))
            )))
        );
        f.no_effects().await;
    }
}
#[test]
fn native_verbatim_or_plain_bases_cannot_elide_original_dot_parent_components() {
    let mut d = defaults();
    #[cfg(windows)]
    {
        d.workspace = PathBuf::from(r"\\?\C:\explicit-launch");
    }
    #[cfg(unix)]
    {
        d.workspace = PathBuf::from("/explicit-launch");
    }
    let mut expected = d.workspace.as_os_str().to_os_string();
    expected.push(std::path::MAIN_SEPARATOR_STR);
    expected.push("repo");
    assert_eq!(resolve(&d.workspace, "repo").unwrap().as_os_str(), expected);
    for raw in ["repo/..", "repo/../other", r"repo\..", ".", ".."] {
        assert_eq!(
            resolve(&d.workspace, raw).err(),
            Some(SelectionError::Scope)
        );
    }
}
#[tokio::test]
async fn unknown_marker_scenario_returns_before_fixture_config_or_native_input() {
    let output = subprocess("unknown-fixture-scenario", true, false).await;
    assert!(!String::from_utf8_lossy(&output.stdout).contains("SCOPE_OUTPUT_ARGV_NATIVE_COMPLETE"));
}
