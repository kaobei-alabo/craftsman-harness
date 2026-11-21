// Diagnostic siblings only. Real Host::command, the original guard and setup
// acceptance stay intact; synthetic fields never construct Prepared.
use super::*;

const WRAPPER: &str = "TailComparison.bat";
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum TailCase {
    NoIfZero,
    NoIfSeven,
    SeparateLinesZero,
    SeparateLinesSeven,
}
const TAILS: [TailCase; 4] = [
    TailCase::NoIfZero,
    TailCase::NoIfSeven,
    TailCase::SeparateLinesZero,
    TailCase::SeparateLinesSeven,
];
impl TailCase {
    fn seven(self) -> bool {
        matches!(self, Self::NoIfSeven | Self::SeparateLinesSeven)
    }
    fn text(self, host: Host) -> String {
        match self {
            Self::NoIfZero | Self::NoIfSeven => no_if(host),
            Self::SeparateLinesZero | Self::SeparateLinesSeven => format!("\"{WRAPPER}\""),
        }
    }
}
fn no_if(host: Host) -> String {
    host.command()
        .replacen(" & (if errorlevel 1 (exit /b 91))", "", 1)
}
fn separate_lines(host: Host) -> String {
    let original = host.command();
    let chain = &original[1..original.len() - 1];
    format!(
        "@echo off\r\n{}\r\n",
        chain.split(" & ").collect::<Vec<_>>().join("\r\n")
    )
}

// One prefix digest is enough when full=true: the existing capture guarantees
// original size==captured bytes and original digest==prefix digest. No raw output
// is published here. Failure/prefix/EOF/limit facts remain explicit.
#[derive(serde::Serialize)]
struct Stream {
    bytes: Option<usize>,
    prefix_sha256: Option<String>,
    eof: bool,
    limit: bool,
    full: bool,
    withheld: bool,
}
impl Stream {
    fn from(record: &facts::StreamFacts) -> Self {
        let full = record.original_size.is_some();
        if full {
            assert_eq!(record.original_size, record.captured_bytes);
            assert_eq!(record.original_sha256, record.captured_prefix_sha256);
        }
        Self {
            bytes: record.captured_bytes,
            prefix_sha256: record.captured_prefix_sha256.clone(),
            eof: record.actual_eof,
            limit: record.reached_capture_limit,
            full,
            withheld: true,
        }
    }
}
#[derive(serde::Serialize)]
struct Cleanup {
    kill_ok: bool,
    kill_os: Option<i32>,
    reaped: bool,
    wait_os: Option<i32>,
    exit: Option<i32>,
    expired: bool,
}
impl Cleanup {
    fn from(value: &facts::CleanupFacts) -> Self {
        Self {
            kill_ok: value.kill_returned_ok,
            kill_os: value.kill_raw_os_error,
            reaped: value.try_wait_returned_status,
            wait_os: value.try_wait_raw_os_error,
            exit: value.exit_code_after_cleanup,
            expired: value.bound_reached_without_status,
        }
    }
}
#[derive(serde::Serialize)]
struct FailureInfo {
    stage: Stage,
    kind: Kind,
    os: Option<i32>,
}
#[derive(serde::Serialize)]
struct Observed {
    out_state: TextState,
    err_state: TextState,
    out_fields: u8,
    err_fields: u8,
    out_tokens: u8,
    err_tokens: u8,
}
fn tokens(value: &ObservedText) -> u8 {
    u8::from(value.entry_token)
        | (u8::from(value.return_zero_request_token) << 1)
        | (u8::from(value.return_seven_request_token) << 2)
        | (u8::from(value.builtin_token) << 3)
}
#[derive(serde::Serialize)]
struct TailRecord {
    case: TailCase,
    // Same owned directory/base/program as the corresponding original inline
    // case already recorded above. This is source-bound input identity, not an
    // execution assertion; unstarted records still have no process/output facts.
    original_case: Case,
    spawned: Option<bool>,
    exit: Option<i32>,
    success: Option<bool>,
    stdout: Option<Stream>,
    stderr: Option<Stream>,
    cleanup: Option<Cleanup>,
    failure: Option<FailureInfo>,
    observed: Option<Observed>,
    not_started: Option<NotStarted>,
}
impl TailRecord {
    fn unstarted(case: TailCase, reason: NotStarted) -> Self {
        Self {
            case,
            original_case: if case.seven() {
                Case::OriginalInlineSeven
            } else {
                Case::OriginalInlineZero
            },
            spawned: None,
            exit: None,
            success: None,
            stdout: None,
            stderr: None,
            cleanup: None,
            failure: None,
            observed: None,
            not_started: Some(reason),
        }
    }
}
fn result(case: TailCase, output: SetupResult<std::process::Output>, host: Host) -> TailRecord {
    let mut record = TailRecord::unstarted(case, NotStarted::SetupRefused);
    record.not_started = None;
    match output {
        Ok(output) => {
            record.spawned = Some(true);
            record.exit = output.status.code();
            record.success = Some(output.status.success());
            record.stdout = Some(Stream::from(&facts::stream(
                Some(&output.stdout),
                true,
                false,
                false,
                0,
            )));
            record.stderr = Some(Stream::from(&facts::stream(
                Some(&output.stderr),
                true,
                false,
                false,
                0,
            )));
            let out = observe(&output.stdout, host);
            let err = observe(&output.stderr, host);
            record.observed = Some(Observed {
                out_state: out.state,
                err_state: err.state,
                out_fields: out.post_call_field_mask,
                err_fields: err.post_call_field_mask,
                out_tokens: tokens(&out),
                err_tokens: tokens(&err),
            });
        }
        Err(error) => {
            record.failure = Some(FailureInfo {
                stage: error.stage,
                kind: error.kind,
                os: error.raw_os_error,
            });
            if let Some(process) = error.process {
                record.spawned = Some(process.spawn_returned_child);
                record.exit = process.exit_code;
                record.success = process.exit_success;
                record.stdout = process.stdout.as_ref().map(Stream::from);
                record.stderr = process.stderr.as_ref().map(Stream::from);
                record.cleanup = process.cleanup.as_ref().map(Cleanup::from);
            }
        }
    }
    record
}

#[derive(serde::Serialize)]
pub(super) struct TailComparisons {
    evidence_only: bool,
    no_if_command_sha256: String,
    multiline_source_bytes: usize,
    multiline_source_sha256: String,
    setup_failure: Option<FailureInfo>,
    cases: Vec<TailRecord>,
}
fn empty(host: Host) -> TailComparisons {
    let source = separate_lines(host);
    TailComparisons {
        evidence_only: true,
        no_if_command_sha256: hash(no_if(host).as_bytes()),
        multiline_source_bytes: source.len(),
        multiline_source_sha256: hash(source.as_bytes()),
        setup_failure: None,
        cases: Vec::new(),
    }
}
#[cfg(windows)]
pub(super) fn collect(base: &Base, owned: &Path, host: Host, started: Instant) -> TailComparisons {
    use std::os::windows::process::CommandExt;
    let mut record = empty(host);
    for name in ["zero", "seven"] {
        if let Err(error) = create_source(&owned.join(name), name, WRAPPER, &separate_lines(host)) {
            record.setup_failure = Some(FailureInfo {
                stage: error.stage,
                kind: error.kind,
                os: error.raw_os_error,
            });
            record.cases = TAILS
                .map(|case| TailRecord::unstarted(case, NotStarted::SetupRefused))
                .into();
            return record;
        }
    }
    for case in TAILS {
        if !should_start(started.elapsed()) {
            record
                .cases
                .push(TailRecord::unstarted(case, NotStarted::ComparisonBudget));
            continue;
        }
        let directory = owned.join(if case.seven() { "seven" } else { "zero" });
        let mut command = base.command(&base.program);
        command
            .current_dir(directory)
            .args(["/d", "/u", "/v:off", "/s", "/c"])
            .raw_arg(case.text(host));
        record.cases.push(result(
            case,
            bounded_at(&mut command, 20, Stage::Control),
            host,
        ));
    }
    record
}

#[test]
fn tail_sources_preserve_original_call_sets_and_guard_on_separate_lines() {
    for host in [Host::X64, Host::X86, Host::Arm64] {
        let original = host.command();
        let diagnostic = no_if(host);
        assert_eq!(diagnostic.matches("if errorlevel").count(), 0);
        assert_eq!(diagnostic.matches(" & set ").count(), 5);
        assert_eq!(
            diagnostic.replace(
                " & set VCToolsInstallDir",
                " & (if errorlevel 1 (exit /b 91)) & set VCToolsInstallDir"
            ),
            original
        );
        let multiline = separate_lines(host);
        assert_eq!(
            multiline,
            format!(
                "@echo off\r\n{}\r\n",
                original[1..original.len() - 1].replace(" & ", "\r\n")
            )
        );
        assert!(
            multiline
                .lines()
                .any(|line| line == "(if errorlevel 1 (exit /b 91))")
        );
        assert_eq!(
            multiline
                .lines()
                .filter(|line| line.starts_with("set "))
                .count(),
            5
        );
        for case in TAILS {
            assert!(case.text(host).is_ascii());
        }
    }
}

#[test]
fn exact_observed_builtin_space_is_not_trimmed_into_success() {
    let before = utf16("MSVC-CMD:BUILTIN \r\n");
    assert_eq!(before.len(), 38);
    assert_eq!(
        hash(&before),
        "ac96d2ead6bd3b761ed7a24a18ee01ee28f67b3c8939ad2e72f54236a87b671c"
    );
    assert!(!observe(&before, Host::X64).builtin_token);
    assert_eq!(observe(&before, Host::X64).state, TextState::UnknownText);
    let after = utf16("MSVC-CMD:BUILTIN\r\n");
    assert_eq!(
        observe(&after, Host::X64).state,
        TextState::FixedControlData
    );
    assert!(observe(&after, Host::X64).builtin_token);
    assert_eq!(
        Case::BuiltinExitSeven.text(Host::X64),
        "\"echo MSVC-CMD:BUILTIN& exit /b 7\""
    );
}

#[test]
fn actual_tail_stream_and_failure_facts_are_withheld_without_invented_completion() {
    let output = bounded_at(&mut fixture_command("seven"), 5, Stage::Control).unwrap();
    let record = result(TailCase::NoIfSeven, Ok(output), Host::X64);
    assert_eq!(record.exit, Some(7));
    assert!(record.stderr.as_ref().unwrap().full && record.stderr.as_ref().unwrap().eof);
    assert!(record.stderr.as_ref().unwrap().withheld);
    assert_eq!(record.observed.as_ref().unwrap().err_tokens & 8, 8);
    let failure = bounded_at(&mut fixture_command("sleep"), 1, Stage::Control).unwrap_err();
    let observed = failure.process.as_ref().unwrap();
    let expected_exit = observed.exit_code;
    let expected_bytes = observed
        .stderr
        .as_ref()
        .and_then(|stream| stream.captured_bytes);
    let record = result(TailCase::SeparateLinesZero, Err(failure), Host::X64);
    assert_eq!(record.exit, expected_exit);
    assert_eq!(
        record.stderr.as_ref().and_then(|stream| stream.bytes),
        expected_bytes
    );
    assert!(record.observed.is_none() && record.cleanup.is_some());
    for case in TAILS {
        let unstarted = TailRecord::unstarted(case, NotStarted::ComparisonBudget);
        assert!(
            unstarted.spawned.is_none() && unstarted.stdout.is_none() && unstarted.exit.is_none()
        );
    }
}

#[test]
fn original_seven_plus_all_tail_extremes_and_nested_parent_fit_the_original_cap() {
    let home = tempfile::tempdir().unwrap();
    let (command, selected, system) = synthetic_template(home.path());
    let base = Base::capture(&command, &selected, &system, Host::X64).unwrap();
    let status = bounded_at(&mut fixture_command("seven"), 5, Stage::Control)
        .unwrap()
        .status;
    for host in [Host::X64, Host::X86, Host::Arm64] {
        for complete in [false, true] {
            let mut combined = CommandComparisons::empty();
            let mut identity = base.recorded();
            identity.program = identity_for_bound();
            identity.original_current_dir = identity_for_bound();
            for field in &mut identity.environment {
                field.value = identity_for_bound();
            }
            combined.base = Some(identity);
            combined.setup_failure = Some(FailureSummary {
                stage: Stage::CompilerEnvironment,
                kind: Kind::DuplicateField,
                expected: Some("VSCMD_ARG_HOST_ARCH"),
                raw_os_error: Some(i32::MIN),
            });
            for directory in ["zero", "seven"] {
                for _ in 0..2 {
                    combined.sources.push(SourceBytes {
                        directory,
                        basename: super::WRAPPER,
                        bytes: 4096,
                        sha256: "f".repeat(64),
                        fixed_source: "host-owned-constant-synthetic-batch",
                    });
                }
            }
            let raw = vec![
                b'x';
                if complete {
                    CAPTURE_BYTES
                } else {
                    CAPTURE_BYTES + 1
                }
            ];
            for case in CASES {
                let mut cwd = identity_for_bound();
                cwd.bytes = 65535;
                combined.cases.push(CaseFacts {
                    case,
                    command_plan: Some(CommandPlan {
                        arguments: arguments(case, host),
                        current_dir: cwd,
                        input: "null",
                    }),
                    process: Some(max_process(&raw, complete)),
                    failure: Some(FailureSummary {
                        stage: Stage::CompilerEnvironment,
                        kind: Kind::DuplicateField,
                        expected: Some("VSCMD_ARG_HOST_ARCH"),
                        raw_os_error: Some(i32::MIN),
                    }),
                    observed_complete_output: Some(Observation {
                        stdout: observation_for_bound(host),
                        stderr: observation_for_bound(host),
                    }),
                    not_started: Some(NotStarted::ComparisonBudget),
                });
            }
            let mut tails = empty(host);
            tails.setup_failure = Some(FailureInfo {
                stage: Stage::CompilerEnvironment,
                kind: Kind::DuplicateField,
                os: Some(i32::MIN),
            });
            for case in TAILS {
                let mut record =
                    result(case, Err(Failure::new(Stage::Control, Kind::Timeout)), host);
                let process = max_process(&raw, complete);
                record.spawned = Some(process.spawn_returned_child);
                record.exit = process.exit_code;
                record.success = process.exit_success;
                record.stdout = process.stdout.as_ref().map(Stream::from);
                record.stderr = process.stderr.as_ref().map(Stream::from);
                record.cleanup = process.cleanup.as_ref().map(Cleanup::from);
                record.failure = Some(FailureInfo {
                    stage: Stage::CompilerEnvironment,
                    kind: Kind::DuplicateField,
                    os: Some(i32::MIN),
                });
                record.observed = Some(Observed {
                    out_state: TextState::FixedControlData,
                    err_state: TextState::FixedControlData,
                    out_fields: 31,
                    err_fields: 31,
                    out_tokens: 15,
                    err_tokens: 15,
                });
                record.not_started = Some(NotStarted::ComparisonBudget);
                tails.cases.push(record);
            }
            combined.tails = Some(tails);
            let original = std::process::Output {
                status,
                stdout: vec![b'x'; CAPTURE_BYTES],
                stderr: vec![b'x'; CAPTURE_BYTES],
            };
            let mut error =
                Failure::new(Stage::Utf16Fields, Kind::DuplicateField).output(&original, false);
            error.expected = Some("VSCMD_ARG_HOST_ARCH");
            error.process.as_mut().unwrap().exit_code = Some(i32::MIN);
            error.command_comparisons = Some(combined);
            let encoded = format!("{error:?}");
            let parent = parent_record(b"", encoded.as_bytes(), Some(i32::MIN), false);
            let nested = format!("MSVC-COMMAND-HELPER-FAILURE:{parent}");
            assert!(encoded.len() + 4096 < CAPTURE_BYTES);
            assert!(
                nested.len() + 4096 < CAPTURE_BYTES,
                "whole old7+tail4+nested packet exceeds unchanged64KiB: {}",
                nested.len()
            );
            println!(
                "SYNTHETIC-TAIL-BOUND host={host:?} complete={complete} failure={} nested={} framing=4096 cap={CAPTURE_BYTES}",
                encoded.len(),
                nested.len()
            );
        }
    }
}
fn identity_for_bound() -> NativeIdentity {
    NativeIdentity {
        // Exercise the longer native Windows identity label on Unix too.
        encoding: "UTF-16LE-native-Windows-units",
        bytes: 65535,
        sha256: "f".repeat(64),
        contents_withheld: false,
    }
}
fn bound_text(host: Host) -> Vec<u8> {
    utf16(&format!(
        "{ENTER}\r\n{RETURN_ZERO}\r\n{RETURN_SEVEN}\r\n{BUILTIN}\r\n{}",
        fields(host)
            .map(|(key, value)| format!("{key}={value}\r\n"))
            .concat()
    ))
}
fn observation_for_bound(host: Host) -> ObservedText {
    let mut value = observe(&bound_text(host), host);
    // False is the longer JSON Boolean. This is a serialized-width control,
    // not an observation of a command or its synthetic field values.
    value.entry_token = false;
    value.return_zero_request_token = false;
    value.return_seven_request_token = false;
    value.builtin_token = false;
    value
}
#[test]
fn measured_tail_scope_changes_only_the_fixed_refusal_command_group() {
    for host in [Host::X64, Host::X86, Host::Arm64] {
        let command = host.command();
        let grouped = " & (if errorlevel 1 (exit /b 91))";
        let original = " & if errorlevel 1 (exit /b 91)";
        assert_eq!(command.matches(grouped).count(), 1);
        let before = command.replacen(grouped, original, 1);
        assert_eq!(before.len() + 2, command.len());
        assert_eq!(before.matches(" & set ").count(), 5);
        assert_eq!(command.matches(" & set ").count(), 5);
        assert_eq!(
            before.replacen(original, "", 1),
            no_if(host),
            "the same CALL and all five field commands must survive guard isolation"
        );
        assert!(command.contains("1>&2") && command.ends_with(" & exit /b 0\""));
        assert!(!command.contains("%") && !command.contains("2>nul"));
    }
}
fn max_process(raw: &[u8], complete: bool) -> facts::ProcessFacts {
    facts::ProcessFacts {
        spawn_returned_child: false,
        exit_code: Some(i32::MIN),
        exit_success: Some(false),
        stdout: Some(facts::stream(Some(raw), complete, !complete, true, EXCERPT)),
        stderr: Some(facts::stream(Some(raw), complete, !complete, true, EXCERPT)),
        cleanup: Some(facts::CleanupFacts {
            kill_returned_ok: false,
            kill_raw_os_error: Some(i32::MIN),
            try_wait_returned_status: false,
            try_wait_raw_os_error: Some(i32::MIN),
            exit_code_after_cleanup: Some(i32::MIN),
            bound_reached_without_status: false,
        }),
    }
}
