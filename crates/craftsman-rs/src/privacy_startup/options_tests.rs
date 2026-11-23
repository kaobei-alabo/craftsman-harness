use super::*;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
use std::process::Command;

const CHILD: &str = "privacy_startup::options_tests::source_options_child";
const MARKER: &str = "captured_options_approval_v1";

fn source_options_fixture(cli: &crate::Cli) -> OwnedTerminalStartup {
    // Test-only source adapter: genuine retained synthetic user source, the
    // SAME parsed-options constructor and into-owned derivation, no TTY claim.
    let source = match capture_user_source(&UserSourceOrigin::Actual)
        .expect("isolated actual user source")
    {
        SourceSelection::Proposed(source) => source,
        _ => panic!("synthetic user source did not propose privacy"),
    };
    let mut captured = finish_source(source).expect("isolated source ownership");
    captured.options = Some(CapturedTerminalOptions::capture(cli));
    captured
        .into_owned()
        .expect("same consuming source derivation")
}

#[test]
#[ignore = "exact marked isolated source/options subprocess only"]
fn source_options_child() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 8
        || args[0] != "--exact"
        || args[1] != CHILD
        || args[2] != "--ignored"
        || args[3] != "--nocapture"
        || args[4] != "--skip"
        || args[5] != MARKER
        || args[6] != "--skip"
        || !matches!(args[7].to_str(), Some("missing" | "unicode" | "nonunicode"))
    {
        // Unknown/duplicate/unmarked argv returns before env/config/stdin.
        return;
    }
    let kind = args[7].to_str().expect("registered case");
    let root = crate::config::config_dir();
    let original = fs::read(root.join("config.json")).expect("synthetic source snapshot");
    let expected_shell = match kind {
        "unicode" => vec!["user-tool", "env-one", "env-two", "env-one"],
        _ => vec!["user-tool"],
    };
    let cases: &[(&[&str], PermissionMode, bool, bool)] = &[
        (&[], PermissionMode::Manual, false, true),
        (
            &["--permission-mode", "plan"],
            PermissionMode::Plan,
            false,
            true,
        ),
        (
            &["--permission-mode", "accept-edits", "--no-mouse"],
            PermissionMode::AcceptEdits,
            false,
            false,
        ),
        (
            &["--permission-mode", "auto"],
            PermissionMode::Auto,
            false,
            true,
        ),
        (
            &["--permission-mode", "plan", "--auto-approve"],
            PermissionMode::BypassPermissions,
            true,
            true,
        ),
    ];
    for (flags, expected_mode, expected_auto, expected_mouse) in cases {
        let cli =
            crate::Cli::try_parse_from(std::iter::once("craftsman").chain(flags.iter().copied()))
                .expect("actual CLI parser for immutable fixture options");
        let startup = source_options_fixture(&cli);
        let options = startup
            .requirements()
            .terminal_options()
            .expect("actual parsed option owner");
        assert!(options.auto_approve_override() == *expected_auto);
        assert!(options.mouse_capture() == *expected_mouse);
        let issued = startup
            .approval_start()
            .expect("source-checked approval issue");
        let state = issued
            .state_for(startup.requirements())
            .expect("actual startup binding");
        let guard = state.lock().expect("live state");
        assert!(guard.mode == *expected_mode);
        assert!(guard.auto_approve == *expected_auto);
        assert!(
            guard.auto_approve_network,
            "actual user web auto source was lost"
        );
        assert!(
            guard
                .shell_allow_list
                .iter()
                .map(String::as_str)
                .eq(expected_shell.iter().copied())
        );
        drop(guard);
        let repeated = startup.approval_start().expect("same cached issue");
        assert!(Arc::ptr_eq(
            &state,
            &repeated
                .state_for(startup.requirements())
                .expect("same live binding")
        ));
        let other = source_options_fixture(&cli);
        assert!(matches!(
            issued.state_for(other.requirements()),
            Err(StartupError::ApprovalSource)
        ));
        let (_, policy, source, requirements) = startup
            .into_configuration_policy()
            .expect("four owners move");
        let boxed_requirements = Box::new(requirements);
        assert!(policy.recheck().is_ok() && source.recheck().is_ok());
        assert!(Arc::ptr_eq(
            &state,
            &issued
                .state_for(&boxed_requirements)
                .expect("identity survives moves")
        ));
        assert!(boxed_requirements.require_normal_activation().is_err());
        assert!(
            !format!("{issued:?} {options_kind:?}", options_kind = "fixed").contains("user-tool")
        );
    }
    // Actual immutable user-auto source overrides even explicit Plan. Web
    // false/non-boolean keeps the original accessor behavior (false).
    let mut value: serde_json::Value = serde_json::from_slice(&original).expect("synthetic source");
    value["auto_approve"] = serde_json::Value::Bool(true);
    value["auto_approve_web"] = serde_json::Value::String("not-a-boolean".into());
    fs::write(
        root.join("config.json"),
        serde_json::to_vec(&value).expect("synthetic encoding"),
    )
    .expect("isolated user-source update before capture");
    let cli = crate::Cli::try_parse_from(["craftsman", "--permission-mode", "plan"])
        .expect("actual Plan parser");
    let startup = source_options_fixture(&cli);
    let issued = startup.approval_start().expect("source-bound user auto");
    let state = issued
        .state_for(startup.requirements())
        .expect("same source binding");
    {
        let guard = state.lock().expect("actual live state");
        assert!(
            guard.mode == PermissionMode::BypassPermissions
                && guard.auto_approve
                && guard.auto_approve_network
        );
    }
    // A fresh root capture with matching config cannot bind the issued state.
    let other = source_options_fixture(&cli);
    assert!(matches!(
        issued.state_for(other.requirements()),
        Err(StartupError::ApprovalSource)
    ));
    // Use the original class-specific effect routine on the derived live
    // state; this is component retention proof, not a UI decision receipt.
    value["auto_approve"] = serde_json::Value::Bool(false);
    value["auto_approve_web"] = serde_json::Value::Bool(false);
    fs::write(
        root.join("config.json"),
        serde_json::to_vec(&value).expect("isolated effect source"),
    )
    .expect("isolated source before effect capture");
    let manual = crate::Cli::try_parse_from(["craftsman"]).expect("actual Manual parser");
    let effects = source_options_fixture(&manual);
    let start = effects.approval_start().expect("actual effect state");
    let live = start
        .state_for(effects.requirements())
        .expect("bound effect state");
    {
        let mut guard = live.lock().expect("live effect state");
        assert!(!guard.auto_approve && !guard.auto_approve_network);
        crate::hitl::apply_always_to_captured_options_fixture(&mut guard, "write_file");
        assert!(guard.auto_approve && !guard.auto_approve_network);
    }
    let repeated = effects
        .approval_start()
        .expect("no reset after local effect");
    assert!(Arc::ptr_eq(
        &live,
        &repeated
            .state_for(effects.requirements())
            .expect("same effect state"),
    ));
    {
        let mut guard = live.lock().expect("same local effect");
        assert!(guard.auto_approve && !guard.auto_approve_network);
        crate::hitl::apply_always_to_captured_options_fixture(&mut guard, "web_fetch");
        assert!(guard.auto_approve && guard.auto_approve_network);
    }
    let (_, _, source, moved_requirements) = effects
        .into_configuration_policy()
        .expect("retained effect requirements");
    assert!(source.recheck().is_ok());
    let moved = start
        .state_for(&moved_requirements)
        .expect("live state after move");
    assert!(Arc::ptr_eq(&live, &moved));
    assert!(
        moved
            .lock()
            .expect("actual retained effects")
            .auto_approve_network
    );
    let before = fs::read(root.join("config.json")).expect("held synthetic source");
    fs::write(root.join("config.json"), original.clone()).expect("isolated source replacement");
    assert!(matches!(
        startup.approval_start(),
        Err(StartupError::Source)
    ));
    assert!(before != original);
    println!("CAPTURED_OPTIONS_SOURCE_PASS");
}

fn synthetic_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().expect("owned temporary directory");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonical temporary root")
        .join("user");
    fs::create_dir(&root).expect("synthetic user root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root mode");
    let body = serde_json::json!({
        "permission_mode": "manual", "auto_approve": false, "auto_approve_web": true,
        "shell_allow_list": ["user-tool"],
        "privacy": {"enabled": true, "detector": {
            "gadget": "synthetic-detector", "function": "detect",
            "component_sha256": "ab".repeat(32), "manifest_sha256": "cd".repeat(32)
        }}
    });
    fs::write(
        root.join("config.json"),
        serde_json::to_vec(&body).expect("synthetic encoding"),
    )
    .expect("synthetic source write");
    fs::set_permissions(root.join("config.json"), fs::Permissions::from_mode(0o600))
        .expect("source mode");
    (temp, root)
}

fn child(
    root: &OsStr,
    marker: Option<&str>,
    kind: &str,
    allow: Option<OsString>,
) -> std::process::Output {
    let mut cmd = Command::new(std::env::current_exe().expect("current test executable"));
    cmd.args(["--exact", CHILD, "--ignored", "--nocapture"]);
    if let Some(marker) = marker {
        cmd.args(["--skip", marker, "--skip", kind]);
    }
    cmd.env("CRAFTSMAN_CONFIG_DIR", root);
    cmd.env_remove("CRAFTSMAN_CLI_SHELL_ALLOW_LIST");
    if let Some(value) = allow {
        cmd.env("CRAFTSMAN_CLI_SHELL_ALLOW_LIST", value);
    }
    cmd.output().expect("isolated immutable child environment")
}

#[test]
fn options_source_actual_child_environment_and_cli_precedence() {
    for (kind, value) in [
        ("missing", None),
        (
            "unicode",
            Some(OsString::from(" env-one , ,env-two,env-one ")),
        ),
        ("nonunicode", Some(OsString::from_vec(vec![0xff]))),
    ] {
        let (_temp, root) = synthetic_root();
        let output = child(root.as_os_str(), Some(MARKER), kind, value);
        assert!(
            output.status.success(),
            "marked isolated component child failed"
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("CAPTURED_OPTIONS_SOURCE_PASS"));
        let names = fs::read_dir(&root)
            .expect("owned root contents")
            .map(|entry| entry.expect("owned entry").file_name())
            .collect::<Vec<_>>();
        assert!(
            names.len() == 1 && names[0] == "config.json",
            "component created unrelated files"
        );
    }
}

#[test]
fn options_source_unmarked_unknown_duplicate_return_before_user_sources() {
    let temp = tempfile::tempdir().expect("owned missing-root fixture");
    let root = temp.path().join("does-not-exist");
    for marker in [None, Some("unknown_options_marker")] {
        let output = child(root.as_os_str(), marker, "missing", None);
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("CAPTURED_OPTIONS_SOURCE_PASS"));
        assert!(!root.exists());
    }
    let mut cmd = Command::new(std::env::current_exe().expect("test executable"));
    cmd.args([
        "--exact",
        CHILD,
        "--ignored",
        "--nocapture",
        "--skip",
        MARKER,
        "--skip",
        "missing",
        "--skip",
        MARKER,
    ]);
    cmd.env("CRAFTSMAN_CONFIG_DIR", &root)
        .env_remove("CRAFTSMAN_CLI_SHELL_ALLOW_LIST");
    let output = cmd.output().expect("duplicate-marker child");
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CAPTURED_OPTIONS_SOURCE_PASS"));
    assert!(!root.exists());
}

#[test]
fn options_source_requires_actual_options_and_retains_pending_gate() {
    let (_temp, root) = synthetic_root();
    let source = match capture_user_source(&UserSourceOrigin::OwnedFixture(root))
        .expect("real synthetic source")
    {
        SourceSelection::Proposed(source) => source,
        _ => panic!("expected actual source"),
    };
    let startup = finish_source(source)
        .expect("held source")
        .into_owned()
        .expect("owned source");
    assert!(matches!(
        startup.approval_start(),
        Err(StartupError::ApprovalSource)
    ));
    assert!(startup.requirements().require_normal_activation().is_err());
}
