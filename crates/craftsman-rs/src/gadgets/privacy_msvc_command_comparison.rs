// Included only inside the existing cfg-test compiler module. These observations
// cannot construct Prepared, adopt source, run Cargo, or authorize a guest.
use super::*;
use std::io::Seek;
use std::io::Write;

const EXCERPT: usize = 128;
const BUDGET: Duration = Duration::from_secs(120);
const RESERVE: Duration = Duration::from_secs(30); //20s polling +5s drain +5s cleanup.
const ENTER: &str = "MSVC-CMD:ENTER";
const RETURN_ZERO: &str = "MSVC-CMD:RETURN-REQUEST:0";
const RETURN_SEVEN: &str = "MSVC-CMD:RETURN-REQUEST:7";
const BUILTIN: &str = "MSVC-CMD:BUILTIN";
const WRAPPER: &str = "CommandComparison.bat";
const HELPER: &str = "gadgets::privacy_detector_tests::compiler_environment::msvc_setup::command_comparison::native_command_comparison_fixture";
const MARKER: &str = "craftsman-msvc-command-comparison=";
const CREDENTIAL: &str = "CRAFTSMAN_TEST_CMD_COMPARISON_CREDENTIAL";
const CREDENTIAL_VALUE: &str = "owned-synthetic-credential-never-output";

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Case {
    BuiltinExitSeven,
    OriginalInlineZero,
    OriginalInlineSeven,
    CompoundRedirectZero,
    CompoundRedirectSeven,
    BatchContextZero,
    BatchContextSeven,
}
const CASES: [Case; 7] = [
    Case::BuiltinExitSeven,
    Case::OriginalInlineZero,
    Case::OriginalInlineSeven,
    Case::CompoundRedirectZero,
    Case::CompoundRedirectSeven,
    Case::BatchContextZero,
    Case::BatchContextSeven,
];
impl Case {
    fn seven(self) -> bool {
        matches!(
            self,
            Self::OriginalInlineSeven | Self::CompoundRedirectSeven | Self::BatchContextSeven
        )
    }
    fn text(self, host: Host) -> String {
        match self {
            Self::BuiltinExitSeven => "\"echo MSVC-CMD:BUILTIN& exit /b 7\"".into(),
            Self::OriginalInlineZero | Self::OriginalInlineSeven => host.command().into(),
            Self::CompoundRedirectZero | Self::CompoundRedirectSeven => {
                // Both replacements operate only on the host-owned static string.
                // Same CALL/argv/exit/field commands; only the redirection is moved
                // to the compound CALL group. No discovered path enters this text.
                host.command()
                    .replacen("\"call ", "\"(call ", 1)
                    .replacen(" 1>&2", ") 1>&2", 1)
            }
            Self::BatchContextZero | Self::BatchContextSeven => format!("\"{WRAPPER}\""),
        }
    }
}

#[derive(serde::Serialize)]
struct NativeIdentity {
    encoding: &'static str,
    bytes: usize,
    sha256: String,
    contents_withheld: bool,
}
fn identity(value: &OsStr) -> NativeIdentity {
    #[cfg(windows)]
    let (bytes, encoding) = {
        use std::os::windows::ffi::OsStrExt;
        (
            value
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
            "UTF-16LE-native-Windows-units",
        )
    };
    #[cfg(unix)]
    let (bytes, encoding) = {
        use std::os::unix::ffi::OsStrExt;
        (value.as_bytes().to_vec(), "native-Unix-bytes")
    };
    NativeIdentity {
        encoding,
        bytes: bytes.len(),
        sha256: hash(&bytes),
        contents_withheld: true,
    }
}

#[derive(serde::Serialize)]
struct FieldIdentity {
    name: &'static str,
    value: NativeIdentity,
}
#[derive(serde::Serialize)]
struct BaseIdentity {
    program: NativeIdentity,
    original_current_dir: NativeIdentity,
    environment: Vec<FieldIdentity>,
    environment_cleared_for_every_control: bool,
    input: &'static str,
}
struct Base {
    values: Vec<(&'static str, OsString)>,
    program: OsString,
    cwd: PathBuf,
}
impl Base {
    fn capture(
        command: &Command,
        selected: &DiscoveryRoot,
        system: &Path,
        host: Host,
    ) -> SetupResult<Self> {
        let program = system.join("System32").join("cmd.exe");
        let arguments: Vec<_> = command.get_args().collect();
        let expected = ["/d", "/u", "/v:off", "/s", "/c", host.command()];
        if command.get_program() != program.as_os_str() || arguments != expected.map(OsStr::new) {
            return Err(Failure::new(Stage::CompilerEnvironment, Kind::Shape));
        }
        let cwd = command
            .get_current_dir()
            .ok_or_else(|| Failure::new(Stage::CompilerEnvironment, Kind::NativePath))?;
        checked_path(cwd.as_os_str().to_owned())?;
        let keys = [
            "HOME",
            "USERPROFILE",
            "TMPDIR",
            "TEMP",
            "TMP",
            "SystemRoot",
            "ComSpec",
            selected.name,
            "PATH",
        ];
        let actual: Vec<_> = command.get_envs().collect();
        if actual.len() != keys.len()
            || actual.iter().any(|(key, value)| {
                value.is_none() || !keys.iter().any(|name| *key == OsStr::new(name))
            })
        {
            return Err(Failure::new(Stage::CompilerEnvironment, Kind::Environment));
        }
        let mut values = Vec::new();
        for name in keys {
            let value = actual
                .iter()
                .find_map(|(key, value)| (*key == OsStr::new(name)).then_some(*value).flatten())
                .ok_or_else(|| {
                    Failure::new(Stage::CompilerEnvironment, Kind::Environment).expected(name)
                })?;
            values.push((name, value.to_owned()));
        }
        if values[..5].iter().any(|(_, value)| value != &values[0].1)
            || values[5].1 != system.as_os_str()
            || values[6].1 != program.as_os_str()
            || values[7].1 != selected.value
        {
            return Err(Failure::new(Stage::CompilerEnvironment, Kind::Environment));
        }
        let expected_path = std::env::join_paths([
            Path::new(env!("CARGO"))
                .parent()
                .ok_or_else(|| Failure::new(Stage::CompilerEnvironment, Kind::NativePath))?,
            system.join("System32").as_path(),
        ])
        .map_err(|_| Failure::new(Stage::CompilerEnvironment, Kind::PathList))?;
        if values[8].1 != expected_path {
            return Err(
                Failure::new(Stage::CompilerEnvironment, Kind::Environment).expected("PATH")
            );
        }
        checked_path(values[0].1.clone())?;
        Ok(Self {
            values,
            program: program.into_os_string(),
            cwd: cwd.to_owned(),
        })
    }
    fn recorded(&self) -> BaseIdentity {
        BaseIdentity {
            program: identity(&self.program),
            original_current_dir: identity(self.cwd.as_os_str()),
            environment: self
                .values
                .iter()
                .map(|(name, value)| FieldIdentity {
                    name,
                    value: identity(value),
                })
                .collect(),
            environment_cleared_for_every_control: true,
            input: "null",
        }
    }
    fn command(&self, program: &OsStr) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .envs(self.values.iter().map(|(name, value)| (name, value)));
        command
    }
}

#[derive(serde::Serialize)]
struct CommandPlan {
    arguments: Vec<String>,
    current_dir: NativeIdentity,
    input: &'static str,
}
fn arguments(case: Case, host: Host) -> Vec<String> {
    ["/d", "/u", "/v:off", "/s", "/c"]
        .map(str::to_owned)
        .into_iter()
        .chain([case.text(host)])
        .collect()
}
#[cfg(windows)]
fn native_command(base: &Base, case: Case, host: Host, cwd: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = base.command(&base.program);
    command
        .current_dir(cwd)
        .args(["/d", "/u", "/v:off", "/s", "/c"])
        .raw_arg(case.text(host));
    command
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum TextState {
    Empty,
    FixedControlData,
    UnknownText,
    OddUtf16,
    InvalidUtf16,
    Nul,
}
#[derive(serde::Serialize)]
struct ObservedText {
    state: TextState,
    entry_token: bool,
    return_zero_request_token: bool,
    return_seven_request_token: bool,
    builtin_token: bool,
    post_call_field_mask: u8,
}
fn fields(host: Host) -> [(&'static str, &'static str); 5] {
    [
        ("VCTOOLSINSTALLDIR", "MSVC-CMD:SYNTHETIC-VC"),
        ("WINDOWSSDKDIR", "MSVC-CMD:SYNTHETIC-SDK"),
        ("VSCMD_ARG_HOST_ARCH", host.architecture()),
        ("VSCMD_ARG_TGT_ARCH", host.architecture()),
        ("LIB", "MSVC-CMD:SYNTHETIC-LIB"),
    ]
}
fn observe(bytes: &[u8], host: Host) -> ObservedText {
    let mut result = ObservedText {
        state: TextState::Empty,
        entry_token: false,
        return_zero_request_token: false,
        return_seven_request_token: false,
        builtin_token: false,
        post_call_field_mask: 0,
    };
    if bytes.is_empty() {
        return result;
    }
    if bytes.len() % 2 != 0 {
        result.state = TextState::OddUtf16;
        return result;
    }
    let units: Vec<_> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    if units.contains(&0) {
        result.state = TextState::Nul;
        return result;
    }
    let Ok(text) = String::from_utf16(units.strip_prefix(&[0xfeff]).unwrap_or(&units)) else {
        result.state = TextState::InvalidUtf16;
        return result;
    };
    result.state = TextState::FixedControlData;
    for line in text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter(|line| !line.is_empty())
    {
        let slot = match line {
            ENTER => Some(&mut result.entry_token),
            RETURN_ZERO => Some(&mut result.return_zero_request_token),
            RETURN_SEVEN => Some(&mut result.return_seven_request_token),
            BUILTIN => Some(&mut result.builtin_token),
            _ => None,
        };
        if let Some(slot) = slot {
            if *slot {
                result.state = TextState::UnknownText;
            }
            *slot = true;
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            result.state = TextState::UnknownText;
            continue;
        };
        let Some(index) = fields(host)
            .iter()
            .position(|(key, expected)| name.eq_ignore_ascii_case(key) && value == *expected)
        else {
            result.state = TextState::UnknownText;
            continue;
        };
        if result.post_call_field_mask & (1 << index) != 0 {
            result.state = TextState::UnknownText;
        }
        result.post_call_field_mask |= 1 << index;
    }
    result
}

#[derive(serde::Serialize)]
struct Observation {
    stdout: ObservedText,
    stderr: ObservedText,
}
#[derive(serde::Serialize)]
struct FailureSummary {
    stage: Stage,
    kind: Kind,
    expected: Option<&'static str>,
    raw_os_error: Option<i32>,
}
fn summary(error: &Failure) -> FailureSummary {
    FailureSummary {
        stage: error.stage,
        kind: error.kind,
        expected: error.expected,
        raw_os_error: error.raw_os_error,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum NotStarted {
    SetupRefused,
    ComparisonBudget,
}
#[derive(serde::Serialize)]
struct CaseFacts {
    case: Case,
    command_plan: Option<CommandPlan>,
    process: Option<facts::ProcessFacts>,
    failure: Option<FailureSummary>,
    observed_complete_output: Option<Observation>,
    not_started: Option<NotStarted>,
}
impl CaseFacts {
    fn not_started(case: Case, why: NotStarted) -> Self {
        Self {
            case,
            command_plan: None,
            process: None,
            failure: None,
            observed_complete_output: None,
            not_started: Some(why),
        }
    }
}
fn condense(process: &mut facts::ProcessFacts) {
    for stream in [&mut process.stdout, &mut process.stderr]
        .into_iter()
        .flatten()
    {
        if let Some(hex) = stream.excerpt_hex.as_mut() {
            if hex.len() > EXCERPT * 2 {
                hex.truncate(EXCERPT * 2);
                stream.excerpt_cut = true;
            }
        }
    }
}
fn executed(
    case: Case,
    plan: CommandPlan,
    output: SetupResult<std::process::Output>,
    host: Host,
) -> CaseFacts {
    match output {
        Ok(output) => CaseFacts {
            case,
            command_plan: Some(plan),
            process: Some(facts::ProcessFacts {
                spawn_returned_child: true,
                exit_code: output.status.code(),
                exit_success: Some(output.status.success()),
                stdout: Some(facts::stream(
                    Some(&output.stdout),
                    true,
                    false,
                    true,
                    EXCERPT,
                )),
                stderr: Some(facts::stream(
                    Some(&output.stderr),
                    true,
                    false,
                    true,
                    EXCERPT,
                )),
                cleanup: None,
            }),
            failure: None,
            observed_complete_output: Some(Observation {
                stdout: observe(&output.stdout, host),
                stderr: observe(&output.stderr, host),
            }),
            not_started: None,
        },
        Err(mut error) => {
            if let Some(process) = error.process.as_mut() {
                condense(process);
            }
            let failure = summary(&error);
            CaseFacts {
                case,
                command_plan: Some(plan),
                process: error.process,
                failure: Some(failure),
                observed_complete_output: None,
                not_started: None,
            }
        }
    }
}

#[derive(serde::Serialize)]
struct SourceBytes {
    directory: &'static str,
    basename: &'static str,
    bytes: usize,
    sha256: String,
    fixed_source: &'static str,
}
#[derive(serde::Serialize)]
pub(super) struct CommandComparisons {
    source: &'static str,
    comparison_only_not_setup_or_source_acceptance: bool,
    original_field_failure_retained: bool,
    private_prestart_budget_seconds: u64,
    reserved_poll_drain_cleanup_seconds: u64,
    base: Option<BaseIdentity>,
    setup_failure: Option<FailureSummary>,
    sources: Vec<SourceBytes>,
    cases: Vec<CaseFacts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tails: Option<tail_comparison::TailComparisons>,
}
impl CommandComparisons {
    fn empty() -> Self {
        Self {
            source: "owned-fixed-command-comparison-after-utf16-fields-refusal",
            comparison_only_not_setup_or_source_acceptance: true,
            original_field_failure_retained: true,
            private_prestart_budget_seconds: BUDGET.as_secs(),
            reserved_poll_drain_cleanup_seconds: RESERVE.as_secs(),
            base: None,
            setup_failure: None,
            sources: Vec::new(),
            cases: Vec::new(),
            tails: None,
        }
    }
}
fn script(host: Host, seven: bool) -> String {
    let mut text = format!("@echo off\r\necho {ENTER}\r\n");
    for (name, value) in fields(host) {
        text.push_str(&format!("set \"{name}={value}\"\r\n"));
    }
    text.push_str(&format!(
        "echo {}\r\nexit /b {}\r\n",
        if seven { RETURN_SEVEN } else { RETURN_ZERO },
        if seven { 7 } else { 0 }
    ));
    text
}
fn wrapper(host: Host) -> String {
    tail_comparison::separate_lines(host)
}
fn create_source(
    directory: &Path,
    label: &'static str,
    basename: &'static str,
    text: &str,
) -> SetupResult<SourceBytes> {
    if text.len() > 4096 || !text.is_ascii() {
        return Err(Failure::new(Stage::Control, Kind::Bounds));
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(directory.join(basename))
        .map_err(|error| Failure::io(Stage::Control, Kind::Metadata, &error))?;
    file.write_all(text.as_bytes())
        .map_err(|error| Failure::io(Stage::Control, Kind::Read, &error))?;
    file.flush()
        .map_err(|error| Failure::io(Stage::Control, Kind::Read, &error))?;
    file.rewind()
        .map_err(|error| Failure::io(Stage::Control, Kind::Read, &error))?;
    let mut actual = Vec::new();
    (&mut file)
        .take(4097)
        .read_to_end(&mut actual)
        .map_err(|error| Failure::io(Stage::Control, Kind::Read, &error))?;
    if actual != text.as_bytes() {
        return Err(Failure::new(Stage::Control, Kind::Shape));
    }
    Ok(SourceBytes {
        directory: label,
        basename,
        bytes: text.len(),
        sha256: hash(text.as_bytes()),
        fixed_source: "host-owned-constant-synthetic-batch",
    })
}
fn should_start(elapsed: Duration) -> bool {
    BUDGET.saturating_sub(elapsed) >= RESERVE
}

#[cfg(windows)]
fn collect(
    template: &Command,
    selected: &DiscoveryRoot,
    system: &Path,
    host: Host,
) -> CommandComparisons {
    let mut record = CommandComparisons::empty();
    let started = Instant::now();
    let setup = (|| {
        let base = Base::capture(template, selected, system, host)?;
        physical_file(Path::new(&base.program))?;
        physical_directory(&base.cwd)?;
        let home = physical_directory(Path::new(&base.values[0].1))?;
        let owned = tempfile::Builder::new()
            .prefix("owned-msvc-command-")
            .tempdir_in(home)
            .map_err(|error| Failure::io(Stage::Control, Kind::Metadata, &error))?;
        for (name, seven) in [("zero", false), ("seven", true)] {
            let directory = owned.path().join(name);
            std::fs::create_dir(&directory)
                .map_err(|error| Failure::io(Stage::Control, Kind::Metadata, &error))?;
            record.sources.push(create_source(
                &directory,
                name,
                "VsDevCmd.bat",
                &script(host, seven),
            )?);
            record
                .sources
                .push(create_source(&directory, name, WRAPPER, &wrapper(host))?);
        }
        record.base = Some(base.recorded());
        Ok((base, owned))
    })();
    let (base, owned) = match setup {
        Ok(value) => value,
        Err(error) => {
            record.setup_failure = Some(summary(&error));
            record.cases = CASES
                .map(|case| CaseFacts::not_started(case, NotStarted::SetupRefused))
                .into();
            return record;
        }
    };
    for case in CASES {
        if !should_start(started.elapsed()) {
            record
                .cases
                .push(CaseFacts::not_started(case, NotStarted::ComparisonBudget));
            continue;
        }
        let cwd = if case == Case::BuiltinExitSeven {
            base.cwd.clone()
        } else {
            owned
                .path()
                .join(if case.seven() { "seven" } else { "zero" })
        };
        let mut command = native_command(&base, case, host, &cwd);
        let plan = CommandPlan {
            arguments: arguments(case, host),
            current_dir: identity(cwd.as_os_str()),
            input: "null",
        };
        record.cases.push(executed(
            case,
            plan,
            bounded_at(&mut command, 20, Stage::Control),
            host,
        ));
    }
    record.tails = Some(tail_comparison::collect(&base, owned.path(), host, started));
    record
}
#[cfg(windows)]
pub(super) fn attach(
    mut error: Failure,
    template: &Command,
    selected: &DiscoveryRoot,
    system: &Path,
    host: Host,
) -> Failure {
    if eligible(&error) {
        error.command_comparisons = Some(collect(template, selected, system, host));
    }
    error
}

fn eligible(error: &Failure) -> bool {
    error.stage == Stage::Utf16Fields
        && error.discovery_comparisons.is_none()
        && error.command_comparisons.is_none()
}

mod tail_comparison {
    include!("privacy_cmd_tail_comparison.rs");
}

fn selected(arguments: impl IntoIterator<Item = OsString>) -> Option<&'static str> {
    let values: Vec<_> = arguments
        .into_iter()
        .filter(|value| value.as_encoded_bytes().starts_with(MARKER.as_bytes()))
        .map(|value| {
            (value.as_encoded_bytes().len() <= MARKER.len() + 8)
                .then(|| {
                    value
                        .to_str()
                        .and_then(|text| text.strip_prefix(MARKER))
                        .map(str::to_owned)
                })
                .flatten()
        })
        .collect();
    let [Some(value)] = values.as_slice() else {
        return None;
    };
    match value.as_str() {
        "suite" => Some("suite"),
        "seven" => Some("seven"),
        "overflow" => Some("overflow"),
        "sleep" => Some("sleep"),
        "relay" => Some("relay"),
        _ => None,
    }
}
fn fixture_command(mode: &str) -> Command {
    assert!(matches!(mode, "seven" | "overflow" | "sleep"));
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}{mode}"));
    #[cfg(windows)]
    command.env("SystemRoot", system_root().unwrap());
    command
}

fn parent_record(stdout: &[u8], stderr: &[u8], exit_code: Option<i32>, success: bool) -> Value {
    json!({
        "source":"already-bounded-new-command-comparison-helper",
        "exit_code":exit_code,
        "exit_success":success,
        "stdout":facts::stream(Some(stdout), true, false, true, CAPTURE_BYTES),
        "stderr":facts::stream(Some(stderr), true, false, true, CAPTURE_BYTES)
    })
}
#[test]
#[ignore = "owned command comparison fixture; exactly one registered native marker required"]
fn native_command_comparison_fixture() {
    let Some(mode) = selected(std::env::args_os()) else {
        println!("MSVC-COMMAND:NOT-SELECTED");
        return;
    };
    if mode == "relay" {
        #[cfg(unix)]
        {
            assert_eq!(
                std::env::var_os(CREDENTIAL),
                Some(OsString::from(CREDENTIAL_VALUE))
            );
            let home = checked_path(std::env::var_os("HOME").unwrap()).unwrap();
            let (template, selected, system) = synthetic_template(&home);
            let base = Base::capture(&template, &selected, &system, Host::X64).unwrap();
            let mut command = base.command(&std::env::current_exe().unwrap().into_os_string());
            command
                .args(["--ignored", "--exact", HELPER, "--nocapture"])
                .arg(format!("{MARKER}seven"));
            let output = bounded_at(&mut command, 5, Stage::Control).unwrap();
            assert_eq!(output.status.code(), Some(7));
            std::io::stderr().write_all(&output.stderr).unwrap();
            std::io::stderr().flush().unwrap();
            std::process::exit(7);
        }
        #[cfg(not(unix))]
        panic!("owned relay control requires its Unix host");
    }
    if mode == "suite" {
        #[cfg(windows)]
        {
            let host = host().unwrap();
            let system = checked_path(std::env::var_os("SystemRoot").unwrap()).unwrap();
            let selected = transferred(|name| std::env::var_os(name)).unwrap().unwrap();
            let home = checked_path(std::env::var_os("HOME").unwrap()).unwrap();
            let mut command = Command::new(system.join("System32").join("cmd.exe"));
            base(&mut command, &selected, &system).unwrap();
            command
                .current_dir(home)
                .args(["/d", "/u", "/v:off", "/s", "/c"]);
            use std::os::windows::process::CommandExt;
            command.raw_arg(host.command());
            let record = collect(&command, &selected, &system, host);
            eprintln!(
                "MSVC-COMMAND-CONTROLS:{}",
                serde_json::to_string(&record).unwrap()
            );
            assert!(record.setup_failure.is_none());
            assert_eq!(record.cases.len(), CASES.len());
            for row in &record.cases {
                assert!(row.not_started.is_none() && row.failure.is_none());
                let process = row.process.as_ref().unwrap();
                let observed = row.observed_complete_output.as_ref().unwrap();
                if row.case == Case::BuiltinExitSeven {
                    assert_eq!(process.exit_code, Some(7));
                    assert!(observed.stdout.builtin_token);
                    assert_eq!(observed.stdout.state, TextState::FixedControlData);
                    assert_eq!(observed.stderr.state, TextState::Empty);
                } else {
                    assert_eq!(
                        process.exit_code,
                        Some(if row.case.seven() { 91 } else { 0 })
                    );
                    assert!(observed.stderr.entry_token);
                    assert_eq!(observed.stderr.return_zero_request_token, !row.case.seven());
                    assert_eq!(observed.stderr.return_seven_request_token, row.case.seven());
                    assert_eq!(
                        observed.stdout.post_call_field_mask,
                        if row.case.seven() { 0 } else { 31 }
                    );
                    assert_eq!(
                        observed.stdout.state,
                        if row.case.seven() {
                            TextState::Empty
                        } else {
                            TextState::FixedControlData
                        }
                    );
                    assert_eq!(observed.stderr.state, TextState::FixedControlData);
                }
                assert!(
                    process.stdout.as_ref().unwrap().actual_eof
                        && process.stderr.as_ref().unwrap().actual_eof
                );
            }
            return;
        }
        #[cfg(not(windows))]
        panic!("native command suite requires its Windows host");
    }
    assert!(
        std::env::var_os(CREDENTIAL).is_none(),
        "synthetic credential reached cleared control"
    );
    let bytes = if mode == "overflow" {
        vec![b'x'; CAPTURE_BYTES + 1]
    } else {
        format!("{BUILTIN}\r\n")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect()
    };
    if mode == "overflow" {
        std::io::stdout().write_all(&bytes).unwrap();
        std::io::stdout().flush().unwrap();
    } else {
        // Libtest's stdout preamble stays intact. The dedicated stderr stream
        // carries complete UTF16 fixture bytes without stripping framework text.
        std::io::stderr().write_all(&bytes).unwrap();
        std::io::stderr().flush().unwrap();
    }
    if mode == "sleep" {
        std::thread::sleep(Duration::from_secs(30));
    }
    std::process::exit(7);
}

#[cfg(windows)]
#[tokio::test]
async fn actual_native_command_family_records_original_redirect_and_batch_context() {
    let home = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}suite"));
    isolated_supervisor_environment(
        command.as_std_mut(),
        home.path(),
        home.path().as_os_str(),
        OsStr::new(""),
        system_root(),
        host_discovery(),
    );
    command.env(CREDENTIAL, CREDENTIAL_VALUE);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let mut out = child.stdout.take().unwrap().take(65537);
    let mut err = child.stderr.take().unwrap().take(65537);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let (a, b, status) = match tokio::time::timeout(Duration::from_secs(240), async {
        tokio::join!(
            out.read_to_end(&mut stdout),
            err.read_to_end(&mut stderr),
            child.wait()
        )
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            child.start_kill().unwrap();
            tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .unwrap()
                .unwrap();
            panic!("owned command control exceeded its unchanged supervisor bound");
        }
    };
    a.unwrap();
    b.unwrap();
    assert!(stdout.len() <= CAPTURE_BYTES && stderr.len() <= CAPTURE_BYTES);
    let output = std::process::Output {
        status: status.unwrap(),
        stdout,
        stderr,
    };
    if !output.status.success() {
        eprintln!(
            "MSVC-COMMAND-HELPER-FAILURE:{}",
            parent_record(
                &output.stdout,
                &output.stderr,
                output.status.code(),
                output.status.success()
            )
        );
    }
    assert!(
        output.status.success(),
        "actual Windows command control failed"
    );
    assert!(
        output
            .stderr
            .windows(b"MSVC-COMMAND-CONTROLS:".len())
            .any(|bytes| bytes == b"MSVC-COMMAND-CONTROLS:")
    );
}

#[test]
fn fixed_command_variants_change_only_the_declared_call_context_or_redirection() {
    assert_eq!(
        CASES.map(Case::seven),
        [false, false, true, false, true, false, true]
    );
    for host in [Host::X64, Host::X86, Host::Arm64] {
        assert_eq!(Case::OriginalInlineZero.text(host), host.command());
        assert_eq!(Case::OriginalInlineSeven.text(host), host.command());
        let outside = Case::CompoundRedirectZero.text(host);
        assert_eq!(
            outside
                .replacen("\"(call ", "\"call ", 1)
                .replacen(") 1>&2", " 1>&2", 1),
            host.command()
        );
        assert_eq!(Case::CompoundRedirectSeven.text(host), outside);
        assert_eq!(
            wrapper(host),
            format!(
                "@echo off\r\n{}\r\n",
                host.command()[1..host.command().len() - 1].replace(" & ", "\r\n")
            )
        );
        for seven in [false, true] {
            let source = script(host, seven);
            assert!(source.is_ascii() && source.len() < 4096);
            assert_eq!(
                source
                    .lines()
                    .filter(|line| line.starts_with("set "))
                    .count(),
                5
            );
            assert!(!source.contains(CREDENTIAL_VALUE) && !source.contains("%"));
        }
    }
}

#[test]
fn owned_batch_source_bytes_are_complete_and_collisions_refuse() {
    let home = tempfile::tempdir().unwrap();
    let bytes = script(Host::X64, false);
    let source = create_source(home.path(), "zero", "VsDevCmd.bat", &bytes).unwrap();
    let actual = std::fs::read(home.path().join("VsDevCmd.bat")).unwrap();
    assert_eq!(actual, bytes.as_bytes());
    assert_eq!(source.bytes, actual.len());
    assert_eq!(source.sha256, hash(&actual));
    assert!(create_source(home.path(), "zero", "VsDevCmd.bat", &bytes).is_err());
    assert!(!source.sha256.contains(CREDENTIAL_VALUE));
}

fn synthetic_template(home: &Path) -> (Command, DiscoveryRoot, PathBuf) {
    let selected = DiscoveryRoot::new("ProgramFiles", home.as_os_str().to_owned()).unwrap();
    let system = home.join("system");
    let mut command = Command::new(system.join("System32").join("cmd.exe"));
    base_values(&mut command, &selected, &system, |_| {
        Some(home.as_os_str().to_owned())
    })
    .unwrap();
    command
        .current_dir(home)
        .args(["/d", "/u", "/v:off", "/s", "/c"])
        .arg(Host::X64.command());
    (command, selected, system)
}

#[test]
fn exact_command_base_refuses_unknown_missing_colliding_and_changed_inputs() {
    let home = tempfile::tempdir().unwrap();
    let (mut command, selected, system) = synthetic_template(home.path());
    let base = Base::capture(&command, &selected, &system, Host::X64).unwrap();
    let mut cleared = base.command(OsStr::new("never-executed"));
    cleared.env_remove(CREDENTIAL);
    assert!(
        cleared
            .get_envs()
            .all(|(key, value)| key != OsStr::new(CREDENTIAL) || value.is_none())
    );
    command.env(CREDENTIAL, CREDENTIAL_VALUE);
    assert!(
        matches!(Base::capture(&command, &selected, &system, Host::X64), Err(error) if error.kind == Kind::Environment)
    );
    let (mut command, selected, system) = synthetic_template(home.path());
    command.env_remove("HOME");
    assert!(Base::capture(&command, &selected, &system, Host::X64).is_err());
    let (mut command, selected, system) = synthetic_template(home.path());
    command.env("PATH", "untrusted-path");
    assert!(
        matches!(Base::capture(&command, &selected, &system, Host::X64), Err(error) if error.expected == Some("PATH"))
    );
    let (mut command, selected, system) = synthetic_template(home.path());
    command.arg("unknown-command-input");
    assert!(
        matches!(Base::capture(&command, &selected, &system, Host::X64), Err(error) if error.kind == Kind::Shape)
    );
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}
#[test]
fn whole_control_observations_refuse_odd_invalid_nul_unknown_and_duplicate_text() {
    let text = fields(Host::X64)
        .map(|(name, value)| format!("{name}={value}\r\n"))
        .concat();
    let observed = observe(&utf16(&text), Host::X64);
    assert_eq!(observed.state, TextState::FixedControlData);
    assert_eq!(observed.post_call_field_mask, 31);
    assert_eq!(observe(&[0xff], Host::X64).state, TextState::OddUtf16);
    assert_eq!(
        observe(&0xd800u16.to_le_bytes(), Host::X64).state,
        TextState::InvalidUtf16
    );
    assert_eq!(observe(&[0, 0], Host::X64).state, TextState::Nul);
    assert_eq!(
        observe(&utf16(CREDENTIAL_VALUE), Host::X64).state,
        TextState::UnknownText
    );
    assert_eq!(
        observe(&utf16(&format!("{text}{text}")), Host::X64).state,
        TextState::UnknownText
    );
    assert_eq!(observe(&[], Host::X64).state, TextState::Empty);
}

#[test]
fn new_marker_counts_native_malformed_companions_and_unselected_returns_before_inputs() {
    let valid = OsString::from(format!("{MARKER}seven"));
    #[cfg(unix)]
    let malformed = {
        use std::os::unix::ffi::OsStringExt;
        let mut bytes = MARKER.as_bytes().to_vec();
        bytes.push(0xff);
        OsString::from_vec(bytes)
    };
    #[cfg(windows)]
    let malformed = {
        use std::os::windows::ffi::OsStringExt;
        let mut units: Vec<_> = MARKER.encode_utf16().collect();
        units.push(0xd800);
        OsString::from_wide(&units)
    };
    assert_eq!(selected([valid.clone()]), Some("seven"));
    for inputs in [
        vec![],
        vec![malformed.clone()],
        vec![valid.clone(), malformed.clone()],
        vec![malformed, valid.clone()],
        vec![valid.clone(), valid],
    ] {
        assert_eq!(selected(inputs), None);
    }
    for inputs in [
        vec![],
        vec![format!("{MARKER}unknown")],
        vec![format!("{MARKER}seven"), format!("{MARKER}seven")],
    ] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .env("HOME", "must-not-be-read")
            .env("SystemRoot", "must-not-be-read")
            .env(CREDENTIAL, CREDENTIAL_VALUE)
            .args(["--ignored", "--exact", HELPER, "--nocapture"])
            .args(inputs);
        let output = bounded_at(&mut command, 5, Stage::Control).unwrap();
        assert!(output.status.success());
        assert!(
            output
                .stdout
                .windows(b"MSVC-COMMAND:NOT-SELECTED".len())
                .any(|bytes| bytes == b"MSVC-COMMAND:NOT-SELECTED")
        );
        assert!(
            !output
                .stderr
                .windows(CREDENTIAL_VALUE.len())
                .any(|bytes| bytes == CREDENTIAL_VALUE.as_bytes())
        );
    }
}

#[test]
fn actual_nonzero_capture_and_timeout_facts_keep_unknown_status_and_exclude_credentials() {
    let mut command = fixture_command("seven");
    let output = bounded_at(&mut command, 5, Stage::Control).unwrap();
    assert_eq!(output.status.code(), Some(7));
    let plan = CommandPlan {
        arguments: vec!["owned-seven-helper".into()],
        current_dir: identity(OsStr::new("owned")),
        input: "null",
    };
    let row = executed(Case::BuiltinExitSeven, plan, Ok(output), Host::X64);
    assert!(
        row.observed_complete_output
            .as_ref()
            .unwrap()
            .stderr
            .builtin_token
    );
    assert_eq!(row.process.as_ref().unwrap().exit_code, Some(7));
    let serialized = serde_json::to_string(&row).unwrap();
    assert!(!serialized.contains(CREDENTIAL_VALUE));
    let mut command = fixture_command("overflow");
    let failure = bounded_at(&mut command, 5, Stage::Control).unwrap_err();
    assert_eq!(failure.kind, Kind::CaptureLimit);
    assert_eq!(
        failure
            .process
            .as_ref()
            .unwrap()
            .stdout
            .as_ref()
            .unwrap()
            .captured_bytes,
        Some(CAPTURE_BYTES + 1)
    );
    assert!(
        failure
            .process
            .as_ref()
            .unwrap()
            .stdout
            .as_ref()
            .unwrap()
            .original_size
            .is_none()
    );
    let mut command = fixture_command("sleep");
    let failure = bounded_at(&mut command, 1, Stage::Control).unwrap_err();
    assert_eq!(failure.kind, Kind::Timeout);
    assert_eq!(failure.process.as_ref().unwrap().exit_code, None);
    assert!(failure.process.as_ref().unwrap().cleanup.is_some());
}

#[test]
fn comparison_budget_records_unstarted_work_without_zero_exit_or_empty_streams() {
    let refused = CaseFacts::not_started(Case::BuiltinExitSeven, NotStarted::SetupRefused);
    assert!(refused.process.is_none() && refused.command_plan.is_none());
    assert_eq!(refused.not_started, Some(NotStarted::SetupRefused));
    assert!(should_start(Duration::ZERO));
    assert!(should_start(BUDGET - RESERVE));
    assert!(!should_start(BUDGET - RESERVE + Duration::from_nanos(1)));
    for case in CASES {
        let row = CaseFacts::not_started(case, NotStarted::ComparisonBudget);
        assert!(
            row.process.is_none()
                && row.observed_complete_output.is_none()
                && row.command_plan.is_none()
        );
        assert_eq!(row.not_started, Some(NotStarted::ComparisonBudget));
    }
}

#[test]
fn original_failure_routing_and_absent_optional_field_preserve_old_records() {
    let error = Failure::new(Stage::VswhereDiscovery, Kind::Empty);
    assert!(!eligible(&error));
    let json = serde_json::to_value(&error).unwrap();
    assert!(json.get("command_comparisons").is_none());
    assert_eq!(json["discovery_comparisons"], Value::Null);
    for stage in [
        Stage::VsdevcmdSetup,
        Stage::RequiredLibrary,
        Stage::PeIdentity,
        Stage::CompilerEnvironment,
    ] {
        assert!(!eligible(&Failure::new(stage, Kind::Empty)));
    }
    let mut error = Failure::new(Stage::Utf16Fields, Kind::UnknownField);
    error.expected = Some("VSCMD_ARG_HOST_ARCH");
    let before = serde_json::to_value(&error).unwrap();
    assert!(eligible(&error));
    error.command_comparisons = Some(CommandComparisons::empty());
    assert!(!eligible(&error));
    let mut after = serde_json::to_value(&error).unwrap();
    after.as_object_mut().unwrap().remove("command_comparisons");
    assert_eq!(after, before);
}

#[cfg(unix)]
#[test]
fn actual_cleared_base_excludes_seeded_parent_credentials_in_a_nested_child() {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .env("HOME", home.path())
        .env(CREDENTIAL, CREDENTIAL_VALUE)
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}relay"));
    let output = bounded_at(&mut command, 5, Stage::Control).unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(observe(&output.stderr, Host::X64).builtin_token);
    assert!(
        !output
            .stdout
            .windows(CREDENTIAL_VALUE.len())
            .any(|bytes| bytes == CREDENTIAL_VALUE.as_bytes())
    );
    assert!(
        !output
            .stderr
            .windows(CREDENTIAL_VALUE.len())
            .any(|bytes| bytes == CREDENTIAL_VALUE.as_bytes())
    );
}

#[test]
fn late_failure_and_nested_hex_packets_fit_original_bounds_with_full_extremes() {
    let home = tempfile::tempdir().unwrap();
    let (command, selected, system) = synthetic_template(home.path());
    let base = Base::capture(&command, &selected, &system, Host::X64).unwrap();
    let status = bounded_at(&mut fixture_command("seven"), 5, Stage::Control)
        .unwrap()
        .status;
    for host in [Host::X64, Host::X86, Host::Arm64] {
        for complete in [false, true] {
            let mut record = CommandComparisons::empty();
            let mut base_record = base.recorded();
            base_record.program.bytes = 65535;
            base_record.original_current_dir.bytes = 65535;
            for field in &mut base_record.environment {
                field.value.bytes = 65535;
            }
            record.base = Some(base_record);
            record.setup_failure = Some(FailureSummary {
                stage: Stage::CompilerEnvironment,
                kind: Kind::CaptureDrain,
                expected: Some("VSCMD_ARG_HOST_ARCH"),
                raw_os_error: Some(i32::MIN),
            });
            for name in ["zero", "seven"] {
                for _ in 0..2 {
                    record.sources.push(SourceBytes {
                        directory: name,
                        basename: WRAPPER,
                        bytes: 4096,
                        sha256: "f".repeat(64),
                        fixed_source: "host-owned-constant-synthetic-batch",
                    });
                }
            }
            for case in CASES {
                let raw = vec![
                    b'x';
                    if complete {
                        CAPTURE_BYTES
                    } else {
                        CAPTURE_BYTES + 1
                    }
                ];
                let text = format!(
                    "{ENTER}\r\n{RETURN_ZERO}\r\n{RETURN_SEVEN}\r\n{BUILTIN}\r\n{}",
                    fields(host)
                        .map(|(name, value)| format!("{name}={value}\r\n"))
                        .concat()
                );
                let mut cwd = identity(OsStr::new("x"));
                cwd.bytes = 65535;
                record.cases.push(CaseFacts {
                    case,
                    command_plan: Some(CommandPlan {
                        arguments: arguments(case, host),
                        current_dir: cwd,
                        input: "null",
                    }),
                    process: Some(facts::ProcessFacts {
                        spawn_returned_child: true,
                        exit_code: Some(i32::MIN),
                        exit_success: Some(false),
                        stdout: Some(facts::stream(
                            Some(&raw),
                            complete,
                            !complete,
                            true,
                            EXCERPT,
                        )),
                        stderr: Some(facts::stream(
                            Some(&raw),
                            complete,
                            !complete,
                            true,
                            EXCERPT,
                        )),
                        cleanup: Some(facts::CleanupFacts {
                            kill_returned_ok: false,
                            kill_raw_os_error: Some(i32::MIN),
                            try_wait_returned_status: false,
                            try_wait_raw_os_error: Some(i32::MIN),
                            exit_code_after_cleanup: Some(i32::MIN),
                            bound_reached_without_status: true,
                        }),
                    }),
                    failure: Some(FailureSummary {
                        stage: Stage::CompilerEnvironment,
                        kind: Kind::CaptureDrain,
                        expected: Some("VSCMD_ARG_HOST_ARCH"),
                        raw_os_error: Some(i32::MIN),
                    }),
                    observed_complete_output: Some(Observation {
                        stdout: observe(&utf16(&text), host),
                        stderr: observe(&utf16(&text), host),
                    }),
                    not_started: Some(NotStarted::ComparisonBudget),
                });
            }
            let original = std::process::Output {
                status,
                stdout: vec![b'x'; CAPTURE_BYTES],
                stderr: vec![b'x'; CAPTURE_BYTES],
            };
            let mut failure =
                Failure::new(Stage::Utf16Fields, Kind::UnknownField).output(&original, false);
            failure.expected = Some("VSCMD_ARG_HOST_ARCH");
            failure.process.as_mut().unwrap().exit_code = Some(i32::MIN);
            failure.command_comparisons = Some(record);
            let encoded = format!("{failure:?}");
            assert!(encoded.is_ascii());
            assert!(encoded.len() + 4096 < CAPTURE_BYTES);
            let parent = parent_record(b"", encoded.as_bytes(), Some(i32::MIN), false);
            let nested = format!("MSVC-COMMAND-HELPER-FAILURE:{parent}");
            assert!(
                nested.len() + 4096 < CAPTURE_BYTES,
                "new whole nested packet exceeds original capture"
            );
            println!(
                "SYNTHETIC-COMMAND-PACKET host={host:?} complete={complete} failure_bytes={} nested_bytes={} framing=4096 cap={CAPTURE_BYTES}",
                encoded.len(),
                nested.len()
            );
        }
    }
}
