use super::*;
use crate::checkpoints::LibsqlCheckpointer;
use crate::exec::construction::{
    InputPreparationError, OutputPreparationError, PreparationError, StdinOutputPreparationError,
    StdinPreparationError,
};
use crate::exec::input::{OriginKind, arguments};
use crate::exec::output::{Error as OutputError, Format};
use crate::exec::permissions::ToolDecision;
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
    let (workspace, config) = ("/explicit-workspace", "/explicit-config");
    #[cfg(windows)]
    let (workspace, config) = (r"C:\explicit-workspace", r"C:\explicit-config");
    Defaults {
        workspace: PathBuf::from(workspace),
        config_dir: PathBuf::from(config),
    }
}
fn rejection(values: &[&str], expected: SelectionError) {
    assert_eq!(
        Selected::select(defaults(), &argv(values)).err(),
        Some(expected)
    );
}
#[test]
fn default_control_policy_is_fresh_manual_deny_and_untrusted() {
    let selected = Selected::select(defaults(), &argv(&["goal"])).unwrap();
    assert_eq!(selected.options.workspace, defaults().workspace);
    assert_eq!(selected.options.config_dir, defaults().config_dir);
    assert_eq!(selected.options.session, SessionSelection::Fresh);
    assert_eq!(selected.options.selectors.mode, Mode::Manual);
    assert_eq!(selected.options.selectors.on_unapproved, OnUnapproved::Deny);
    assert!(!selected.options.trust_workspace);
    assert!(
        selected.options.selectors.allow.is_empty()
            && selected.options.selectors.deny.is_empty()
            && selected.options.selectors.env_pass.is_empty()
    );
    assert_eq!(selected.source_arguments, argv(&["goal"]));
}
#[test]
fn manual_plan_and_deny_fail_use_exact_existing_enum_grammar() {
    for (mode, parsed_mode) in [("manual", Mode::Manual), ("plan", Mode::Plan)] {
        for (action, parsed_action) in [("deny", OnUnapproved::Deny), ("fail", OnUnapproved::Fail)]
        {
            let selected = Selected::select(
                defaults(),
                &argv(&[
                    "--permission-mode",
                    mode,
                    "goal",
                    &format!("--on-unapproved={action}"),
                ]),
            )
            .unwrap();
            assert_eq!(selected.options.selectors.mode, parsed_mode);
            assert_eq!(selected.options.selectors.on_unapproved, parsed_action);
            assert_eq!(selected.source_arguments, argv(&["goal"]));
        }
    }
}
#[test]
fn old_broad_permission_modes_and_auto_approve_alias_are_rejected() {
    for mode in [
        "auto",
        "accept-edits",
        "bypass-permissions",
        "Manual",
        " plan ",
        "",
    ] {
        rejection(
            &["goal", &format!("--permission-mode={mode}")],
            SelectionError::Permissions(UsageError::Mode),
        );
    }
    rejection(&["goal", "--auto-approve"], SelectionError::Flag);
}
#[test]
fn unsupported_unapproved_actions_never_become_defaults() {
    for action in ["approve", "ask", "Fail", " fail ", ""] {
        rejection(
            &["goal", &format!("--on-unapproved={action}")],
            SelectionError::Permissions(UsageError::OnUnapproved),
        );
    }
}
#[test]
fn repeated_control_values_and_booleans_refuse_instead_of_last_wins() {
    for args in [
        vec![
            "goal",
            "--permission-mode=manual",
            "--permission-mode=manual",
        ],
        vec!["goal", "--on-unapproved=deny", "--on-unapproved=fail"],
        vec!["goal", "--trust-workspace", "--trust-workspace"],
        vec!["goal", "--ephemeral", "--ephemeral"],
        vec!["goal", "--continue", "--continue"],
    ] {
        rejection(&args, SelectionError::Duplicate);
    }
}
#[test]
fn session_modes_are_mutually_exclusive_in_all_orders() {
    for first in ["--ephemeral", "--continue", "--session=thread"] {
        for second in ["--ephemeral", "--continue", "--session=thread"] {
            rejection(&["goal", first, second], SelectionError::Duplicate);
        }
    }
}
#[test]
fn exact_session_identifier_grammar_reuses_old_store_selection_validator() {
    for id in ["thread-123", "A_z.7", ".", "..", "a"] {
        let selected = Selected::select(defaults(), &argv(&["--session", id, "goal"])).unwrap();
        assert_eq!(
            selected.options.session,
            SessionSelection::Resume(id.into())
        );
    }
    for id in ["", "a/b", "a\\b", "a b", "字", "a\0b", "a:1"] {
        rejection(
            &["goal", &format!("--session={id}")],
            SelectionError::Session,
        );
    }
    rejection(
        &["goal", &format!("--session={}", "x".repeat(129))],
        SelectionError::Session,
    );
}
#[test]
fn exact_session_id_length_boundary_remains_128_ascii_bytes() {
    let id = "x".repeat(128);
    let selected =
        Selected::select(defaults(), &argv(&["goal", &format!("--session={id}")])).unwrap();
    assert_eq!(selected.options.session, SessionSelection::Resume(id));
}
#[test]
fn standalone_session_selectors_are_typed_before_any_store_inspection() {
    for (flag, mode) in [
        ("--ephemeral", SessionSelection::Ephemeral),
        ("--continue", SessionSelection::Continue),
    ] {
        let selected = Selected::select(defaults(), &argv(&[flag, "goal"])).unwrap();
        assert_eq!(selected.options.session, mode);
    }
}
#[test]
fn valued_forms_of_boolean_flags_are_unsupported_not_truthy() {
    for flag in [
        "--ephemeral=true",
        "--continue=false",
        "--trust-workspace=1",
    ] {
        rejection(&["goal", flag], SelectionError::Flag);
    }
}
#[test]
fn allow_deny_env_names_preserve_original_order_and_are_not_source_tokens() {
    let selected = Selected::select(
        defaults(),
        &argv(&[
            "--allow",
            "tool:write_file",
            "goal",
            "--deny=path:protected.txt",
            "--allow=tool:read_file",
            "--env-pass=CI_JOB_ID",
            "--env-pass",
            "BUILD_NUMBER",
        ]),
    )
    .unwrap();
    assert_eq!(
        selected.options.selectors.allow,
        ["tool:write_file", "tool:read_file"]
    );
    assert_eq!(selected.options.selectors.deny, ["path:protected.txt"]);
    assert_eq!(
        selected.options.selectors.env_pass,
        ["CI_JOB_ID", "BUILD_NUMBER"]
    );
    assert_eq!(selected.source_arguments, argv(&["goal"]));
}
#[test]
fn ordinary_deny_and_allow_conflict_is_preserved_for_deny_wins() {
    let selected = Selected::select(
        defaults(),
        &argv(&["goal", "--allow=tool:write_file", "--deny=tool:write_file"]),
    )
    .unwrap();
    let policy = InvocationPermissions::from_explicit(selected.options.selectors, &[]).unwrap();
    assert_eq!(
        policy.tool_decision("write_file", &json!({"file_path":"x","content":"y"})),
        ToolDecision::Deny
    );
}
#[test]
fn malformed_unknown_or_protected_allow_rules_use_existing_policy_errors() {
    for rule in ["unknown", "tool:unknown", "path:../bad", "exec:[]"] {
        rejection(
            &["goal", &format!("--allow={rule}")],
            SelectionError::Permissions(UsageError::Rule),
        );
    }
    for rule in ["tool:ask_user", "path:.env", "path:.craftsman/x"] {
        rejection(
            &["goal", &format!("--allow={rule}")],
            SelectionError::Permissions(UsageError::ForbiddenAllow),
        );
    }
}
#[test]
fn duplicated_permission_rules_are_not_silently_deduplicated() {
    rejection(
        &["goal", "--allow=tool:write_file", "--allow=tool:write_file"],
        SelectionError::Permissions(UsageError::DuplicateRule),
    );
    rejection(
        &["goal", "--deny=tool:write_file", "--deny=tool:write_file"],
        SelectionError::Permissions(UsageError::DuplicateRule),
    );
}
#[test]
fn command_prefix_rules_keep_native_platform_support_without_reinterpretation() {
    let args = argv(&["goal", "--allow=exec:[\"git\",\"status\"]"]);
    let result = Selected::select(defaults(), &args);
    if crate::platform_os::shell_allow_list_supported() {
        assert!(result.is_ok());
    } else {
        assert_eq!(
            result.err(),
            Some(SelectionError::Permissions(UsageError::UnsupportedPrefix))
        );
    }
}
#[test]
fn environment_identifier_duplicate_and_conventional_credential_guards_are_reused() {
    rejection(
        &["goal", "--env-pass=bad-name"],
        SelectionError::Permissions(UsageError::EnvironmentName),
    );
    rejection(
        &["goal", "--env-pass=CI_JOB_ID", "--env-pass=ci_job_id"],
        SelectionError::Permissions(UsageError::DuplicateEnvironmentName),
    );
    for name in [
        "OPENAI_API_KEY",
        "APP_TOKEN",
        "SVC_SECRET",
        "DATABASE_PASSWORD",
        "CRAFTSMAN_PLATFORM_TOKEN",
    ] {
        rejection(
            &["goal", &format!("--env-pass={name}")],
            SelectionError::Permissions(UsageError::CredentialEnvironmentName),
        );
    }
}
#[test]
fn provisional_env_grammar_does_not_claim_configured_credential_authority() {
    let selected =
        Selected::select(defaults(), &argv(&["goal", "--env-pass=HARMLESS_ROUTE"])).unwrap();
    assert_eq!(selected.options.selectors.env_pass, ["HARMLESS_ROUTE"]);
}
#[test]
fn permission_and_environment_native_resource_limits_precede_policy_copies() {
    let (rules, rule_bytes, envs, env_bytes) = permissions::frontend_argument_bounds();
    assert_eq!((rules, rule_bytes, envs, env_bytes), (256, 16384, 64, 128));
    for flag in ["--allow", "--deny"] {
        let mut args = argv(&["goal"]);
        args.extend((0..rules + 1).map(|i| OsString::from(format!("{flag}=path:file{i}"))));
        assert_eq!(
            Selected::select(defaults(), &args).err(),
            Some(SelectionError::Resource)
        );
    }
    let mut args = argv(&["goal"]);
    args.extend((0..envs + 1).map(|i| OsString::from(format!("--env-pass=FIELD_{i}"))));
    assert_eq!(
        Selected::select(defaults(), &args).err(),
        Some(SelectionError::Resource)
    );
    rejection(
        &["goal", &format!("--allow={}", "x".repeat(rule_bytes + 1))],
        SelectionError::Resource,
    );
    rejection(
        &["goal", &format!("--env-pass={}", "X".repeat(env_bytes + 1))],
        SelectionError::Resource,
    );
}
#[test]
fn bounded_full_control_counts_and_native_encoded_work_reject_upfront() {
    let (rules, _, envs, _) = permissions::frontend_argument_bounds();
    let count = 2 * super::super::FILES + 4 + 4 * rules + 2 * envs + 16;
    let args = vec![OsString::from("x"); count + 1];
    assert_eq!(
        Selected::select(defaults(), &args).err(),
        Some(SelectionError::Count)
    );
    rejection(
        &[&"x".repeat(super::super::INSTRUCTION_BYTES + 33)],
        SelectionError::Resource,
    );
    let args = vec![OsString::from("x".repeat(super::super::INSTRUCTION_BYTES)); 12];
    assert_eq!(
        Selected::select(defaults(), &args).err(),
        Some(SelectionError::Resource)
    );
}
#[test]
fn current_source_instruction_and_attachment_limits_are_not_raised_by_full_argv() {
    let exact = "x".repeat(super::super::INSTRUCTION_BYTES);
    assert!(Selected::select(defaults(), &argv(&["--ephemeral", &exact])).is_ok());
    rejection(
        &["--ephemeral", &format!("{exact}x")],
        SelectionError::Input(InputError::InstructionLimit),
    );
    let mut args = argv(&["--ephemeral", "-"]);
    args.extend((0..129).map(|_| OsString::from("--file=x")));
    assert_eq!(
        Selected::select(defaults(), &args).err(),
        Some(SelectionError::Input(InputError::SelectionLimit))
    );
}
#[test]
fn native_invalid_utf8_is_never_lossily_decoded_for_controls_or_sources() {
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
    for flag in ["--cwd", "--allow", "--session"] {
        assert_eq!(
            Selected::select(
                defaults(),
                &[
                    OsString::from("goal"),
                    OsString::from(flag),
                    invalid.clone()
                ]
            )
            .err(),
            Some(SelectionError::Utf8)
        );
    }
}
#[test]
fn absolute_scope_flags_preserve_original_native_paths_without_ambient_resolution() {
    let work = defaults().workspace.join("child");
    let config = defaults().config_dir.join("selected");
    let args = vec![
        OsString::from("--cwd"),
        work.as_os_str().to_owned(),
        OsString::from(format!("--config-dir={}", config.display())),
        OsString::from("goal"),
    ];
    let selected = Selected::select(defaults(), &args).unwrap();
    assert_eq!(selected.options.workspace, work);
    assert_eq!(selected.options.config_dir, config);
}
#[test]
fn relative_empty_parent_dot_and_control_scope_spellings_refuse_without_io() {
    for scope in [
        "relative",
        ".",
        "..",
        "",
        "/bad/../path",
        "/bad/./path",
        "/bad\0path",
        "/bad\u{001b}path",
    ] {
        rejection(&["goal", &format!("--cwd={scope}")], SelectionError::Scope);
        rejection(
            &["goal", &format!("--config-dir={scope}")],
            SelectionError::Scope,
        );
    }
}
#[test]
fn windows_verbatim_scope_dot_parent_spellings_are_not_normalized_into_selection() {
    for segment in ["..", "."] {
        let mut path = defaults().workspace.as_os_str().to_os_string();
        path.push(std::path::MAIN_SEPARATOR_STR);
        path.push(segment);
        let mut value = OsString::from("--cwd=");
        value.push(path);
        assert_eq!(
            Selected::select(defaults(), &[OsString::from("goal"), value]).err(),
            Some(SelectionError::Scope)
        );
    }
}
#[test]
fn scope_depth_scalar_and_native_byte_ceilings_are_conservative() {
    #[cfg(unix)]
    let root = "/";
    #[cfg(windows)]
    let root = r"C:\";
    let deep = format!(
        "{root}{}",
        vec!["x"; 129].join(std::path::MAIN_SEPARATOR_STR)
    );
    rejection(&["goal", &format!("--cwd={deep}")], SelectionError::Scope);
    let big = format!("{root}{}", "x".repeat(super::super::PATH_SCALARS + 1));
    rejection(
        &["goal", &format!("--config-dir={big}")],
        SelectionError::Scope,
    );
}
#[test]
fn default_scope_values_are_checked_only_when_selected_but_never_guessed() {
    let bad = Defaults {
        workspace: PathBuf::from("relative"),
        config_dir: defaults().config_dir,
    };
    assert_eq!(
        Selected::select(bad, &argv(&["goal"])).err(),
        Some(SelectionError::Scope)
    );
    let override_value = format!("--cwd={}", defaults().workspace.display());
    let bad = Defaults {
        workspace: PathBuf::from("relative"),
        config_dir: defaults().config_dir,
    };
    assert!(Selected::select(bad, &argv(&["goal", &override_value])).is_ok());
}
#[test]
fn duplicate_scope_overrides_refuse_even_when_they_name_the_same_path() {
    let work = format!("--cwd={}", defaults().workspace.display());
    let config = format!("--config-dir={}", defaults().config_dir.display());
    rejection(&["goal", &work, &work], SelectionError::Duplicate);
    rejection(&["goal", &config, &config], SelectionError::Duplicate);
}
#[test]
fn original_source_tokens_keep_order_duplicates_equals_unicode_and_minus_filenames() {
    let args = argv(&[
        "--permission-mode=plan",
        "--file=資料=a",
        "--ephemeral",
        "--prompt-file",
        "-",
        "--env-pass=CI_JOB_ID",
        "--file",
        "-",
        "--file=資料=a",
    ]);
    let selected = Selected::select(defaults(), &args).unwrap();
    assert_eq!(
        selected.source_arguments,
        argv(&[
            "--file=資料=a",
            "--prompt-file",
            "-",
            "--file",
            "-",
            "--file=資料=a"
        ])
    );
}
#[test]
fn end_of_options_is_original_source_data_not_a_permission_or_scope_escape() {
    let selected = Selected::select(
        defaults(),
        &argv(&["--ephemeral", "--", "--allow=tool:execute"]),
    )
    .unwrap();
    assert!(selected.options.selectors.allow.is_empty());
    assert_eq!(
        selected.source_arguments,
        argv(&["--", "--allow=tool:execute"])
    );
    rejection(
        &["goal", "--", "--cwd=x"],
        SelectionError::Input(InputError::ConflictingSources),
    );
}
#[test]
fn exact_minus_still_selects_stdin_after_control_flags_and_end_of_options() {
    let selected = Selected::select(
        defaults(),
        &argv(&["--ephemeral", "--on-unapproved=fail", "--", "-"]),
    )
    .unwrap();
    assert_eq!(selected.source_arguments, argv(&["--", "-"]));
    let selected = Selected::select(defaults(), &argv(&["--ephemeral", " - "])).unwrap();
    assert_eq!(selected.source_arguments, argv(&[" - "]));
}
#[test]
fn missing_sources_or_conflicting_instruction_forms_are_fixed_usage_errors() {
    rejection(
        &["--ephemeral"],
        SelectionError::Input(InputError::MissingSource),
    );
    rejection(
        &["--ephemeral", "goal", "--prompt-file=x"],
        SelectionError::Input(InputError::ConflictingSources),
    );
    rejection(
        &["--permission-mode=plan", "goal", "another"],
        SelectionError::Input(InputError::ConflictingSources),
    );
    rejection(
        &["--ephemeral", ""],
        SelectionError::Input(InputError::Empty),
    );
}
#[test]
fn split_option_looking_values_require_joined_form_except_file_minus() {
    for flag in [
        "--cwd",
        "--config-dir",
        "--allow",
        "--session",
        "--file",
        "--prompt-file",
    ] {
        rejection(&["goal", flag, "--ephemeral"], SelectionError::Value);
    }
    rejection(&["goal", "--on-unapproved"], SelectionError::Value);
    assert!(Selected::select(defaults(), &argv(&["--prompt-file", "-", "--ephemeral"])).is_ok());
}
#[test]
fn unimplemented_gadget_output_trace_ledger_and_wall_options_refuse_before_run() {
    for flag in [
        "--allow-gadget=demo",
        "--output-format=text",
        "--output-file=x",
        "--trace-file=x",
        "--max-rounds=1",
        "--token-budget=1",
        "--timeout=1",
        "--help",
        "--version",
        "--auto-approve",
        "-m",
        "--unknown",
    ] {
        rejection(&["--ephemeral", "-", flag], SelectionError::Flag);
    }
}
#[test]
fn selection_errors_never_echo_source_rules_paths_or_credential_names() {
    let err = Selected::select(
        defaults(),
        &argv(&["goal", "--allow=SECRET\u{001b}::warning::"]),
    )
    .err()
    .unwrap();
    assert_eq!(err.to_string(), "[REDACTED]");
    assert!(!format!("{err:?}").contains("SECRET"));
    assert_eq!(Error::Selection(err).to_string(), "[REDACTED]");
}

const KEY: &str = "control-argv-fixture-captured-secret";
const CAP: usize = 1024 * 1024;
struct Fixture {
    root: tempfile::TempDir,
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
        let fixture = Self {
            root,
            workspace,
            config,
            output,
            server: MockServer::start().await,
        };
        fixture.configure();
        fixture
    }
    fn configure(&self) {
        std::fs::write(self.config.join("config.json"),json!({"model":"synthetic-model","api_key":KEY,"base_url":self.server.uri(),"auto_approve":true,"permission_mode":"bypass-permissions","shell_allow_list":["*"]}).to_string()).unwrap();
    }
    fn defaults(&self) -> Defaults {
        Defaults {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
        }
    }
    fn destination(&self) -> Destination {
        Destination {
            directory: self.output.clone(),
            file_name: "result.out".into(),
            format: Format::AsciiFragment,
            max_bytes: CAP,
        }
    }
    fn file(&self, name: &str, data: impl AsRef<[u8]>) {
        let path = self.workspace.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, data).unwrap();
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
    async fn respond(&self, text: &str) {
        self.event(json!({"choices":[{"delta":{"content":text},"finish_reason":"stop"}]}))
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
async fn invalid_control_or_permission_flags_precede_bad_config_store_source_or_target_io() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("config.json"), b"invalid config").unwrap();
    for (args, error) in [
        (
            vec!["-", "--allow=tool:unknown"],
            SelectionError::Permissions(UsageError::Rule),
        ),
        (vec!["-", "--session=bad/id"], SelectionError::Session),
        (vec!["-", "--cwd=relative"], SelectionError::Scope),
        (vec!["-", "--timeout=1"], SelectionError::Flag),
    ] {
        assert_eq!(
            Invocation::prepare(f.defaults(), &argv(&args)).await.err(),
            Some(Error::Selection(error))
        );
        assert_eq!(
            FileInvocation::prepare(f.defaults(), &argv(&args), f.destination())
                .await
                .err(),
            Some(Error::Selection(error))
        );
        f.no_effects().await;
    }
}
#[tokio::test]
async fn supported_controls_preserve_same_bootstrap_defaults_and_ignore_ambient_broad_grants() {
    let f = Fixture::new().await;
    f.respond("done").await;
    let invocation = Invocation::prepare(f.defaults(), &argv(&["--ephemeral", "goal"]))
        .await
        .unwrap();
    f.no_effects().await;
    let run = invocation.run().await;
    assert!(run.outcome().is_ok());
    assert!(run.prepared().ignored_ambient_settings());
    assert_eq!(run.prepared().permissions().mode(), Mode::Manual);
    assert_eq!(
        run.prepared().permissions().on_unapproved(),
        OnUnapproved::Deny
    );
    assert!(run.prepared().permissions().allow_rules().is_empty());
    assert!(!run.prepared().config().auto_approve);
    assert_eq!(run.facts().instruction().kind(), OriginKind::Argument);
    assert_eq!(f.inputs().await, ["goal"]);
    f.no_store();
}
#[tokio::test]
async fn explicit_plan_and_env_pass_are_bound_to_actual_runtime_not_provisional_policy() {
    let f = Fixture::new().await;
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &argv(&[
            "--ephemeral",
            "--permission-mode=plan",
            "--allow=tool:write_file",
            "--env-pass=CI_JOB_ID",
            "goal",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(run.prepared().permissions().mode(), Mode::Plan);
    assert_eq!(run.prepared().permissions().env_pass(), ["CI_JOB_ID"]);
    assert_eq!(run.prepared().shell().env_pass, ["CI_JOB_ID"]);
    assert!(run.prepared().shell().env_scrub);
    assert_eq!(
        run.prepared().permissions().tool_decision(
            "write_file",
            &json!({"file_path":"new.txt","content":"data"})
        ),
        ToolDecision::Deny
    );
    assert!(!f.workspace.join("new.txt").exists());
    f.no_store();
}
#[tokio::test]
async fn custom_configured_credential_sources_are_rejected_before_input_or_session_effects() {
    let f = Fixture::new().await;
    std::fs::write(
        f.config.join("config.json"),
        json!({"model":"synthetic-model","api_key":"${HARMLESS_ROUTE}","base_url":f.server.uri()})
            .to_string(),
    )
    .unwrap();
    assert_eq!(
        Invocation::prepare(
            f.defaults(),
            &argv(&["-", "--env-pass=HARMLESS_ROUTE", "--ephemeral"])
        )
        .await
        .err(),
        Some(Error::Preparation(arguments::Error::Stdin(
            StdinPreparationError::Preparation(PreparationError::Permissions)
        )))
    );
    f.no_effects().await;
}
#[tokio::test]
async fn selected_workspace_is_resolved_by_same_retained_bootstrap_before_source_loading() {
    let f = Fixture::new().await;
    let other = f
        .root
        .path()
        .canonicalize()
        .unwrap()
        .join("actual-selected-work");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("goal.txt"), b"original selected workspace goal").unwrap();
    f.file("goal.txt", b"wrong default workspace goal");
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &argv(&[
            "--ephemeral",
            &format!("--cwd={}", other.display()),
            "--prompt-file=goal.txt",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(run.prepared().workspace(), other);
    assert_eq!(f.inputs().await, ["original selected workspace goal"]);
    assert_eq!(run.facts().instruction().path(), Some("goal.txt"));
    f.no_store();
}
#[tokio::test]
async fn selected_config_dir_governs_config_sources_and_store_consistently() {
    let f = Fixture::new().await;
    let config = f
        .root
        .path()
        .canonicalize()
        .unwrap()
        .join("actual-selected-config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(
        config.join("config.json"),
        json!({"model":"synthetic-model","api_key":KEY,"base_url":f.server.uri()}).to_string(),
    )
    .unwrap();
    std::fs::write(f.config.join("config.json"), b"invalid unused config").unwrap();
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &argv(&[
            "--ephemeral",
            &format!("--config-dir={}", config.display()),
            "goal",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(run.outcome().is_ok());
    assert_eq!(run.prepared().config_dir(), config);
    assert!(!config.join("threads.db").exists());
    f.no_store();
}
#[tokio::test]
async fn actual_session_resume_uses_existing_id_without_ambient_permissions() {
    let f = Fixture::new().await;
    let store = f.seeded_store().await;
    assert!(
        store
            .create_headless_thread("existing-thread")
            .await
            .unwrap()
    );
    drop(store);
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &argv(&[
            "--session=existing-thread",
            "--permission-mode=manual",
            "goal",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(
        run.prepared().persistence().thread_id(),
        Some("existing-thread")
    );
    assert_eq!(
        run.prepared().persistence().mode(),
        &SessionSelection::Resume("existing-thread".into())
    );
    assert!(run.prepared().permissions().allow_rules().is_empty());
}
#[tokio::test]
async fn actual_continue_uses_existing_updated_desc_id_asc_and_scope_filter() {
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
    let run = Invocation::prepare(f.defaults(), &argv(&["--continue", "goal"]))
        .await
        .unwrap()
        .run()
        .await;
    assert_eq!(run.prepared().persistence().thread_id(), Some("a-existing"));
    assert_eq!(
        run.prepared().persistence().mode(),
        &SessionSelection::Continue
    );
}
#[tokio::test]
async fn no_session_flag_keeps_old_fresh_durable_selection() {
    let f = Fixture::new().await;
    f.respond("done").await;
    let run = Invocation::prepare(f.defaults(), &argv(&["goal"]))
        .await
        .unwrap()
        .run()
        .await;
    assert!(run.prepared().persistence().thread_id().is_some());
    assert_eq!(
        run.prepared().persistence().mode(),
        &SessionSelection::Fresh
    );
    assert!(f.config.join("threads.db").exists());
}
#[tokio::test]
async fn missing_and_foreign_sessions_refuse_without_provider_or_metadata_retirement() {
    let f = Fixture::new().await;
    let store = f.seeded_store().await;
    store.create_headless_thread("valid").await.unwrap();
    store.connection().execute("INSERT INTO threads(id,created,updated,title,workspace) VALUES('foreign',1,99,'legacy','wrong')",()).await.unwrap();
    drop(store);
    for (id, error) in [
        ("missing", PreparationError::MissingSession),
        ("foreign", PreparationError::WorkspaceMismatch),
    ] {
        assert_eq!(
            Invocation::prepare(f.defaults(), &argv(&[&format!("--session={id}"), "goal"]))
                .await
                .err(),
            Some(Error::Preparation(arguments::Error::Input(
                InputPreparationError::Preparation(error)
            )))
        );
    }
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn trust_workspace_only_enables_existing_project_discovery_not_execution_permission() {
    let f = Fixture::new().await;
    f.file(".craftsman/AGENTS.md", "UNIQUE_PROJECT_CONTEXT");
    f.respond("done").await;
    let untrusted = Invocation::prepare(f.defaults(), &argv(&["--ephemeral", "goal"]))
        .await
        .unwrap()
        .run()
        .await;
    assert!(
        !format!("{:?}", untrusted.prepared().discovery().instructions)
            .contains("UNIQUE_PROJECT_CONTEXT")
    );
    let trusted = Invocation::prepare(
        f.defaults(),
        &argv(&["--ephemeral", "--trust-workspace", "goal"]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(
        format!("{:?}", trusted.prepared().discovery().instructions)
            .contains("UNIQUE_PROJECT_CONTEXT")
    );
    assert!(trusted.prepared().permissions().allow_rules().is_empty());
    f.no_store();
}
#[tokio::test]
async fn control_and_source_owned_inputs_are_not_reloaded_after_preparation() {
    let f = Fixture::new().await;
    f.file("goal.txt", "original captured goal");
    f.respond("done").await;
    let mut args = argv(&[
        "--ephemeral",
        "--permission-mode=plan",
        "--prompt-file=goal.txt",
    ]);
    let invocation = Invocation::prepare(f.defaults(), &args).await.unwrap();
    args[1] = OsString::from("--permission-mode=manual");
    f.file("goal.txt", "later replacement");
    let run = invocation.run().await;
    assert_eq!(run.prepared().permissions().mode(), Mode::Plan);
    assert_eq!(f.inputs().await, ["original captured goal"]);
    f.no_store();
}
#[tokio::test]
async fn original_attachment_labels_bom_crlf_and_duplicates_survive_control_filtering() {
    let f = Fixture::new().await;
    f.file("data.txt", "\u{feff}original data\r\n");
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &argv(&[
            "--ephemeral",
            "--file=data.txt",
            "--permission-mode=manual",
            "goal",
            "--file",
            "data.txt",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(run.facts().attachments().len(), 2);
    assert_eq!(
        run.facts().attachment_bytes(),
        2 * "\u{feff}original data\r\n".len()
    );
    assert_eq!(run.facts().attachments()[0].path(), Some("data.txt"));
    assert!(!run.facts().attachments()[0].bom_removed());
    f.no_store();
}
#[tokio::test]
async fn actual_fail_permission_controls_share_existing_typed_latch_and_file_owner() {
    let f = Fixture::new().await;
    f.event(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"protected","type":"function","function":{"name":"write_file","arguments":"{\"file_path\":\"build.rs\",\"content\":\"data\"}"}}]},"finish_reason":"tool_calls"}]})).await;
    let run = FileInvocation::prepare(
        f.defaults(),
        &argv(&[
            "--ephemeral",
            "--allow=tool:write_file",
            "--on-unapproved=fail",
            "goal",
        ]),
        f.destination(),
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
#[tokio::test]
async fn explicit_allow_and_deny_bind_real_runtime_policy_with_deny_winning() {
    let f = Fixture::new().await;
    f.respond("done").await;
    let run = Invocation::prepare(
        f.defaults(),
        &argv(&[
            "--ephemeral",
            "--allow=tool:write_file",
            "--deny=tool:write_file",
            "goal",
        ]),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(
        run.prepared()
            .permissions()
            .tool_decision("write_file", &json!({"file_path":"x","content":"y"})),
        ToolDecision::Deny
    );
    f.no_store();
}
#[tokio::test]
async fn controls_use_existing_guarded_ascii_record_and_exact_one_attempt() {
    let f = Fixture::new().await;
    f.respond(&format!("中文 🦀 {KEY}")).await;
    let invocation = FileInvocation::prepare(
        f.defaults(),
        &argv(&["--ephemeral", "--permission-mode=plan", "goal"]),
        f.destination(),
    )
    .await
    .unwrap();
    f.no_effects().await;
    let run = invocation.run().await;
    assert_eq!(
        run.run_for_tests().prepared().permissions().mode(),
        Mode::Plan
    );
    let mut terminal = run.terminal();
    terminal.publish().unwrap();
    let bytes = std::fs::read(f.output.join("result.out")).unwrap();
    assert!(bytes.is_ascii());
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["final_output"], "中文 🦀 [REDACTED]");
    assert!(value.get("usage").is_none() && value.get("status").is_none());
    assert_eq!(
        terminal.publish().unwrap_err().kind(),
        OutputError::AlreadyAttempted
    );
    f.no_store();
}
#[tokio::test]
async fn missing_or_denied_source_and_existing_output_keep_old_prep_failures() {
    let f = Fixture::new().await;
    f.file(".env", "protected");
    assert_eq!(
        Invocation::prepare(f.defaults(), &argv(&["--ephemeral", "--prompt-file=.env"]))
            .await
            .err(),
        Some(Error::Preparation(arguments::Error::Input(
            InputPreparationError::Input(InputError::Source)
        )))
    );
    f.no_effects().await;
    std::fs::write(f.output.join("result.out"), b"foreign").unwrap();
    assert_eq!(
        FileInvocation::prepare(
            f.defaults(),
            &argv(&["--ephemeral", "goal"]),
            f.destination()
        )
        .await
        .err(),
        Some(Error::Preparation(arguments::Error::Output(
            OutputPreparationError::Output(OutputError::Exists)
        )))
    );
    assert_eq!(
        std::fs::read(f.output.join("result.out")).unwrap(),
        b"foreign"
    );
    f.no_store();
}

const HELPER: &str = "exec::input::frontend::tests::native_control_arguments_helper";
const MARKER: &str = "--craftsman-private-control-arguments-fixture";
const NATIVE: &[u8] = b"original native control argv goal\r\n";
#[tokio::test]
#[ignore = "native subprocess only; argv marker required before any stdin/config work"]
async fn native_control_arguments_helper() {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(index) = args.iter().position(|arg| arg == MARKER) else {
        return;
    };
    let Some(scenario) = args.get(index + 1) else {
        return;
    };
    if !matches!(
        scenario.as_str(),
        "memory" | "file" | "policy" | "scope" | "session" | "credential" | "literal" | "target"
    ) {
        return;
    }
    let f = Fixture::new().await;
    if scenario == "policy" {
        assert_eq!(
            Invocation::prepare(f.defaults(), &argv(&["-", "--allow=tool:unknown"]))
                .await
                .err(),
            Some(Error::Selection(SelectionError::Permissions(
                UsageError::Rule
            )))
        );
        f.no_effects().await;
    } else if scenario == "scope" {
        assert_eq!(
            Invocation::prepare(f.defaults(), &argv(&["-", "--cwd=relative"]))
                .await
                .err(),
            Some(Error::Selection(SelectionError::Scope))
        );
        f.no_effects().await;
    } else if scenario == "session" {
        assert_eq!(
            Invocation::prepare(f.defaults(), &argv(&["-", "--session=bad/id"]))
                .await
                .err(),
            Some(Error::Selection(SelectionError::Session))
        );
        f.no_effects().await;
    } else if scenario == "credential" {
        std::fs::write(f.config.join("config.json"),json!({"model":"synthetic-model","api_key":"${HARMLESS_ROUTE}","base_url":f.server.uri()}).to_string()).unwrap();
        assert_eq!(
            Invocation::prepare(
                f.defaults(),
                &argv(&["-", "--env-pass=HARMLESS_ROUTE", "--ephemeral"])
            )
            .await
            .err(),
            Some(Error::Preparation(arguments::Error::Stdin(
                StdinPreparationError::Preparation(PreparationError::Permissions)
            )))
        );
        f.no_effects().await;
        f.configure();
    } else if scenario == "target" {
        let mut destination = f.destination();
        destination.file_name = ".env".into();
        assert_eq!(
            FileInvocation::prepare(f.defaults(), &argv(&["--ephemeral", "-"]), destination)
                .await
                .err(),
            Some(Error::Preparation(arguments::Error::StdinOutput(
                StdinOutputPreparationError::Output(OutputError::Path)
            )))
        );
        f.no_effects().await;
    } else if scenario == "literal" {
        f.respond("done").await;
        let run = Invocation::prepare(
            f.defaults(),
            &argv(&["--ephemeral", "--", "--allow=tool:execute"]),
        )
        .await
        .unwrap()
        .run()
        .await;
        assert_eq!(run.facts().instruction().kind(), OriginKind::Argument);
        assert!(run.prepared().permissions().allow_rules().is_empty());
        assert_eq!(f.inputs().await, ["--allow=tool:execute"]);
    }
    if scenario != "literal" {
        f.respond("done").await;
    }
    let prior = f.inputs().await;
    assert_eq!(prior.len(), usize::from(scenario == "literal"));
    if scenario == "file" {
        let invocation = FileInvocation::prepare(
            f.defaults(),
            &argv(&[
                "--ephemeral",
                "--permission-mode=manual",
                "--on-unapproved=fail",
                "-",
            ]),
            f.destination(),
        )
        .await
        .unwrap();
        f.no_effects().await;
        let run = invocation.run().await;
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
        let invocation = Invocation::prepare(
            f.defaults(),
            &argv(&[
                "--ephemeral",
                "--permission-mode=manual",
                "--on-unapproved=deny",
                "--",
                "-",
            ]),
        )
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
    f.no_store();
    println!("CONTROL_ARGV_NATIVE_COMPLETE");
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
        .expect("fixture startup/capture bound, not production read deadline")
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
async fn real_native_stdin_supported_controls_reach_old_memory_chain() {
    let output = subprocess("memory", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("CONTROL_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn real_native_stdin_supported_controls_reach_old_guarded_file_chain() {
    let output = subprocess("file", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("CONTROL_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn real_native_invalid_policy_scope_and_session_controls_do_not_consume_stdin() {
    for scenario in ["policy", "scope", "session"] {
        let output = subprocess(scenario, true, true).await;
        assert!(String::from_utf8_lossy(&output.stdout).contains("CONTROL_ARGV_NATIVE_COMPLETE"));
    }
}
#[tokio::test]
async fn real_native_configured_credential_env_source_refuses_before_input() {
    let output = subprocess("credential", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("CONTROL_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn real_native_invalid_guarded_target_preserves_input_after_valid_controls() {
    let output = subprocess("target", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("CONTROL_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn real_literal_option_looking_prompt_cannot_add_policy_or_consume_pipe() {
    let output = subprocess("literal", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("CONTROL_ARGV_NATIVE_COMPLETE"));
}
#[tokio::test]
async fn unmarked_ignored_control_helper_returns_before_source_or_config_inspection() {
    let output = subprocess("memory", false, false).await;
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CONTROL_ARGV_NATIVE_COMPLETE"));
}
