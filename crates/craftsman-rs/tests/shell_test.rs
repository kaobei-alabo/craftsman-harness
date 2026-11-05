//! End-to-end tests of the `execute` tool's shell settings (harness plan P2b)
//! against a mock OpenAI-compatible server, through the real `craftsman -m`
//! binary: round 1 asks for one `execute` call, round 2 ends the turn, and
//! the test reads the tool result the binary sent back in round 2.
//!
//! Process tests assert outcomes with generous bounds (30 s), never latencies.

use std::time::{Duration, Instant};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const BOUND: Duration = Duration::from_secs(30);

fn set_test_home(command: &mut tokio::process::Command, home: &std::path::Path) {
    command.env("HOME", home);
    command.env("CRAFTSMAN_CONFIG_DIR", home.join(".craftsman"));
    #[cfg(windows)]
    command.env("USERPROFILE", home);
}

fn sse(events: &[String]) -> String {
    let mut out = String::new();
    for e in events {
        out.push_str("data: ");
        out.push_str(e);
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

/// Round 1: one `execute` call with `args`; round 2: "done".
struct OneExecute {
    args: serde_json::Value,
}

impl Respond for OneExecute {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let has_tool_result = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool");
        let events = if has_tool_result {
            vec![
                r#"{"choices":[{"delta":{"content":"done"}}]}"#.to_string(),
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#.to_string(),
            ]
        } else {
            let call = serde_json::json!({"choices":[{"delta":{"tool_calls":[{
                "index": 0, "id": "call_1",
                "function": {"name": "execute", "arguments": self.args.to_string()}
            }]}}]});
            vec![
                call.to_string(),
                r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#.to_string(),
            ]
        };
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse(&events))
    }
}

struct Run {
    server: MockServer,
    home: tempfile::TempDir,
}

impl Run {
    async fn new(args: serde_json::Value, shell: serde_json::Value) -> Self {
        Self::with_config(args, serde_json::json!({ "shell": shell })).await
    }

    /// `extra` is merged over the base config (mock route, model).
    async fn with_config(args: serde_json::Value, extra: serde_json::Value) -> Self {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(OneExecute { args })
            .mount(&server)
            .await;
        let home = tempfile::tempdir().unwrap();
        let craftsman_dir = home.path().join(".craftsman");
        std::fs::create_dir_all(&craftsman_dir).unwrap();
        let mut config = serde_json::json!({
            "base_url": server.uri(),
            "api_key": "test-key",
            "model": "mock-model",
        });
        for (key, value) in extra.as_object().unwrap() {
            config[key] = value.clone();
        }
        std::fs::write(craftsman_dir.join("config.json"), config.to_string()).unwrap();
        Self { server, home }
    }

    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_craftsman"));
        command
            .args(["-m", "run it", "--auto-approve"])
            .current_dir(self.home.path())
            .stdin(std::process::Stdio::null());
        set_test_home(&mut command, self.home.path());
        command
    }

    /// The first request's tool list.
    async fn tools(&self) -> serde_json::Value {
        let requests = self.server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        body["tools"].clone()
    }

    /// The tool result the binary sent back in round 2.
    async fn tool_result(&self) -> String {
        let requests = self.server.received_requests().await.unwrap();
        for request in requests.iter().rev() {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            if let Some(tool) = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "tool")
            {
                return tool["content"].as_str().unwrap().to_string();
            }
        }
        panic!("no tool result was sent back");
    }
}

/// Run `craftsman -m` to completion and return the tool result.
async fn run_to_end(args: serde_json::Value, shell: serde_json::Value) -> (Run, String) {
    finish(Run::new(args, shell).await, &[]).await
}

/// Run `craftsman -m` to completion with `env` set for it (only for it), and
/// return the tool result.
async fn finish(run: Run, env: &[(&str, &std::ffi::OsStr)]) -> (Run, String) {
    let mut command = run.command();
    for (name, value) in env {
        command.env(name, value);
    }
    let out = tokio::time::timeout(Duration::from_secs(120), command.output())
        .await
        .expect("craftsman -m finishes")
        .unwrap();
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let result = run.tool_result().await;
    (run, result)
}

#[cfg(unix)]
fn nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{}{:09}", std::process::id(), nanos)
}

#[cfg(unix)]
fn pgrep(pattern: &str) -> Vec<u32> {
    let out = std::process::Command::new("pgrep")
        .args(["-f", pattern])
        .output()
        .expect("pgrep is available");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|pid| pid.parse().ok())
        .collect()
}

async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < BOUND, "{what}: not within {BOUND:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// An interrupted `craftsman -m` (SIGINT, as Ctrl-C in a cooked terminal) leaves
/// no process of its command when `shell.process_group` is on: the command
/// is in a group of its own, which the terminal's SIGINT would not reach,
/// and the one-shot signal hook kills it before exiting with 130.
#[cfg(unix)]
#[tokio::test]
async fn interrupted_one_shot_leaves_no_process_of_its_command() {
    let n = nonce();
    let run = Run::new(
        serde_json::json!({"command": format!("sleep 60.{n}1 & sleep 60.{n}2")}),
        serde_json::json!({"process_group": true}),
    )
    .await;
    let mut child = run.command().spawn().unwrap();
    let sleepers = format!("^sleep [0-9]+\\.{n}");
    wait_until("both sleepers started", || pgrep(&sleepers).len() == 2).await;
    let pid = child.id().unwrap().to_string();
    let sent = std::process::Command::new("kill")
        .args(["-INT", &pid])
        .status()
        .unwrap();
    assert!(sent.success());
    let status = tokio::time::timeout(BOUND, child.wait())
        .await
        .expect("craftsman exits after SIGINT")
        .unwrap();
    assert_eq!(status.code(), Some(130));
    wait_until("every process of the command gone", || pgrep(&n).is_empty()).await;
}

/// All defaults: the execute schema has only `command` and a short command's
/// result is today's text.
#[tokio::test]
async fn defaults_keep_the_schema_and_the_result_text() {
    #[cfg(unix)]
    let command = "echo hi; echo err >&2; exit 3";
    #[cfg(windows)]
    let command = "Write-Output 'hi'; [Console]::Error.WriteLine('err'); exit 3";
    let (run, result) = run_to_end(
        serde_json::json!({ "command": command }),
        serde_json::json!({}),
    )
    .await;
    let tools = run.tools().await;
    let execute = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["function"]["name"] == "execute")
        .unwrap();
    assert_eq!(
        execute["function"]["parameters"],
        serde_json::json!({
            "type": "object",
            "properties": {"command": {"type": "string", "description": "Shell command to run"}},
            "required": ["command"]
        })
    );
    #[cfg(unix)]
    assert_eq!(result, "hi\n\n--- stderr ---\nerr\n\n[exit code 3]");
    #[cfg(windows)]
    assert!(
        result.contains("hi") && result.contains("[exit code 3]"),
        "{result}"
    );
}

#[cfg(unix)]
const SLEEP_3S: &str = "sleep 3";
#[cfg(windows)]
const SLEEP_3S: &str = "Start-Sleep -Seconds 3";

/// `shell.timeout_param` on: the schema gains `timeout`, and the model's
/// per-call value is the limit (with the default legacy wording).
#[tokio::test]
async fn timeout_param_flag_exposes_and_honours_the_argument() {
    let (run, result) = run_to_end(
        serde_json::json!({"command": SLEEP_3S, "timeout": 2}),
        serde_json::json!({"timeout_param": true}),
    )
    .await;
    let tools = run.tools().await;
    let execute = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["function"]["name"] == "execute")
        .unwrap();
    assert_eq!(
        execute["function"]["parameters"]["properties"]["timeout"]["type"],
        "integer"
    );
    assert_eq!(
        result,
        "tool 'execute' error: Error: command timed out after 2s"
    );

    let (_, result) = run_to_end(
        serde_json::json!({"command": SLEEP_3S, "timeout": 10}),
        serde_json::json!({"timeout_param": true}),
    )
    .await;
    assert_eq!(result, "");
}

/// Flag off: a `timeout` argument from the model is ignored (the default
/// 120 s applies), so the 3 s sleep completes.
#[tokio::test]
async fn timeout_argument_is_ignored_with_the_flag_off() {
    let (_, result) = run_to_end(
        serde_json::json!({"command": SLEEP_3S, "timeout": 1}),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(result, "");
}

#[cfg(unix)]
const BIG_OUTPUT: &str = "printf 'HEAD-MARK\\n'; head -c 200000 /dev/zero | tr '\\0' x; printf '\\nTAIL-MARK\\n'; echo oops >&2; exit 5";
#[cfg(windows)]
const BIG_OUTPUT: &str = "Write-Output 'HEAD-MARK'; [Console]::Out.Write('x' * 200000); Write-Output ''; Write-Output 'TAIL-MARK'; [Console]::Error.WriteLine('oops'); exit 5";

/// `shell.output_format = "head_tail"` through the binary: 200 kB keeps head,
/// tail, stderr and the exit code and names the spill file.
#[tokio::test]
async fn head_tail_output_format_through_the_binary() {
    let (_, result) = run_to_end(
        serde_json::json!({ "command": BIG_OUTPUT }),
        serde_json::json!({"output_format": "head_tail"}),
    )
    .await;
    assert!(result.starts_with("HEAD-MARK"));
    assert!(result.contains("TAIL-MARK"));
    assert!(result.contains("--- stderr ---\noops"));
    assert!(result.ends_with("[exit code 5]"));
    let marker = result
        .lines()
        .find(|l| l.contains("omitted;"))
        .expect("a cut marker");
    eprintln!("marker: {marker}");
    assert!(marker.contains("the full stdout is in "), "{marker}");
}

/// Default format through the binary: today's front cut.
#[tokio::test]
async fn legacy_output_format_through_the_binary() {
    let (_, result) = run_to_end(
        serde_json::json!({ "command": BIG_OUTPUT }),
        serde_json::json!({}),
    )
    .await;
    assert!(result.ends_with("\n… [output truncated]"));
    assert!(!result.contains("TAIL-MARK"));
}

const PROBED: &[&str] = &[
    "CRAFTSMAN_TEST_FAKE_API_KEY",
    "CRAFTSMAN_TEST_FAKE_TOKEN",
    "CRAFTSMAN_TEST_LLM_VAR",
    "CRAFTSMAN_TEST_PASSED_KEY",
    "PATH",
    "HOME",
    "CARGO_HOME",
    "SSH_AUTH_SOCK",
];

/// A command that prints, for each probed name, only whether it is present
/// (never a value), and the three hints.
fn env_probe() -> String {
    #[cfg(unix)]
    {
        format!(
            "for v in {}; do if printenv \"$v\" >/dev/null; then echo \"$v=present\"; else echo \"$v=absent\"; fi; done; echo \"hints=$NO_COLOR,$PAGER,$GIT_TERMINAL_PROMPT\"",
            PROBED.join(" ")
        )
    }
    #[cfg(windows)]
    {
        let names = PROBED
            .iter()
            .map(|n| format!("'{n}'"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "foreach ($v in {names}) {{ if (Test-Path \"env:$v\") {{ \"$v=present\" }} else {{ \"$v=absent\" }} }}; \"hints=$env:NO_COLOR,$env:PAGER,$env:GIT_TERMINAL_PROMPT\""
        )
    }
}

/// Synthetic variables, set only for the craftsman child under test.
fn synthetic_env(dir: &std::path::Path) -> Vec<(&'static str, std::ffi::OsString)> {
    vec![
        ("CRAFTSMAN_TEST_FAKE_API_KEY", "not-a-secret".into()),
        ("CRAFTSMAN_TEST_FAKE_TOKEN", "not-a-secret".into()),
        ("CRAFTSMAN_TEST_LLM_VAR", "not-a-secret-either".into()),
        ("CRAFTSMAN_TEST_PASSED_KEY", "passed".into()),
        ("CARGO_HOME", dir.join("cargo-home").into_os_string()),
        ("SSH_AUTH_SOCK", dir.join("agent.sock").into_os_string()),
    ]
}

fn as_refs<'a>(
    env: &'a [(&'static str, std::ffi::OsString)],
) -> Vec<(&'static str, &'a std::ffi::OsStr)> {
    env.iter().map(|(k, v)| (*k, v.as_os_str())).collect()
}

/// Exit criterion: a `*_KEY` (and `*_TOKEN`) variable and the variable
/// `api_key` is read from are absent from the command; PATH, HOME,
/// CARGO_HOME and SSH_AUTH_SOCK are present; `shell.env_pass` lets a name
/// through; the hints are set. With `shell.env_scrub = false` everything is
/// inherited as before.
#[tokio::test]
async fn the_scrub_removes_secret_names_and_keeps_the_toolchain() {
    let dir = tempfile::tempdir().unwrap();
    let env = synthetic_env(dir.path());
    let run = Run::with_config(
        serde_json::json!({ "command": env_probe() }),
        serde_json::json!({
            "api_key": "${CRAFTSMAN_TEST_LLM_VAR}",
            "shell": {"env_pass": ["CRAFTSMAN_TEST_PASSED_KEY"]}
        }),
    )
    .await;
    let (_, result) = finish(run, &as_refs(&env)).await;
    for line in [
        "CRAFTSMAN_TEST_FAKE_API_KEY=absent",
        "CRAFTSMAN_TEST_FAKE_TOKEN=absent",
        "CRAFTSMAN_TEST_LLM_VAR=absent",
        "CRAFTSMAN_TEST_PASSED_KEY=present",
        "PATH=present",
        "HOME=present",
        "CARGO_HOME=present",
        "SSH_AUTH_SOCK=present",
        "hints=1,cat,0",
    ] {
        assert!(
            result.lines().any(|l| l.trim() == line),
            "{line} in:\n{result}"
        );
    }

    let run = Run::with_config(
        serde_json::json!({ "command": env_probe() }),
        serde_json::json!({
            "api_key": "${CRAFTSMAN_TEST_LLM_VAR}",
            "shell": {"env_scrub": false}
        }),
    )
    .await;
    let (_, result) = finish(run, &as_refs(&env)).await;
    for line in [
        "CRAFTSMAN_TEST_FAKE_API_KEY=present",
        "CRAFTSMAN_TEST_FAKE_TOKEN=present",
        "CRAFTSMAN_TEST_LLM_VAR=present",
    ] {
        assert!(
            result.lines().any(|l| l.trim() == line),
            "{line} in:\n{result}"
        );
    }
}

/// The real toolchain directories of this machine (rustup's proxies find
/// their toolchain through them; the test's HOME is a temporary directory).
fn toolchain_env() -> Vec<(&'static str, std::ffi::OsString)> {
    let home = dirs::home_dir().expect("a home directory");
    vec![
        (
            "CARGO_HOME",
            std::env::var_os("CARGO_HOME").unwrap_or_else(|| home.join(".cargo").into()),
        ),
        (
            "RUSTUP_HOME",
            std::env::var_os("RUSTUP_HOME").unwrap_or_else(|| home.join(".rustup").into()),
        ),
    ]
}

/// Exit criterion: `cargo --version` and `npm --version` still work in a
/// scrubbed environment (npm is skipped when it is not installed).
#[tokio::test]
async fn cargo_and_npm_still_work_in_the_scrubbed_environment() {
    let env = toolchain_env();
    #[cfg(unix)]
    let command = "cargo --version; if command -v npm >/dev/null; then npm --version && echo npm-ok; else echo npm-absent; fi";
    #[cfg(windows)]
    let command = "cargo --version; if (Get-Command npm -ErrorAction SilentlyContinue) { npm --version; if ($LASTEXITCODE -eq 0) { 'npm-ok' } } else { 'npm-absent' }";
    let run = Run::with_config(
        serde_json::json!({ "command": command }),
        serde_json::json!({}),
    )
    .await;
    let (_, result) = finish(run, &as_refs(&env)).await;
    assert!(result.contains("cargo 1."), "{result}");
    assert!(!result.contains("[exit code"), "{result}");
    if result.contains("npm-absent") {
        eprintln!("npm is not installed here: the npm half of this test was skipped");
    } else {
        assert!(result.contains("npm-ok"), "{result}");
    }
}

/// Exit criterion: SSH_AUTH_SOCK survives and `ssh-add -l` reaches a test
/// agent (exit 1, "no identities", rather than 2, "cannot connect").
/// Skipped with a message when ssh-agent is not available.
#[cfg(unix)]
#[tokio::test]
async fn ssh_add_reaches_a_test_agent_through_the_scrub() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("agent.sock");
    let started = std::process::Command::new("ssh-agent")
        .arg("-a")
        .arg(&sock)
        .arg("-s")
        .output();
    let Some(started) = started.ok().filter(|o| o.status.success()) else {
        eprintln!("ssh-agent is not available: skipping the ssh-add check");
        return;
    };
    let out = String::from_utf8_lossy(&started.stdout).into_owned();
    let agent_pid = out
        .split("SSH_AGENT_PID=")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        .expect("ssh-agent prints its pid");
    let run = Run::with_config(
        serde_json::json!({"command": "ssh-add -l >/dev/null 2>&1; echo \"rc=$?\""}),
        serde_json::json!({}),
    )
    .await;
    let (_, result) = finish(run, &[("SSH_AUTH_SOCK", sock.as_os_str())]).await;
    let _ = std::process::Command::new("kill")
        .arg(agent_pid.to_string())
        .status();
    assert_eq!(result.trim(), "rc=1", "ssh-add could not reach the agent");
}
