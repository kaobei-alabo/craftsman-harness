//! Golden model requests: the exact JSON body the real `craftsman` binary sends to the
//! model, once per harness profile.
//!
//! Phase P1 of `docs/craftsman-harness-plan.md` renames the framework crate, its types
//! and its hooks. "No behavior change" has to be provable, and what a harness does is,
//! observably, the request its model receives: the system prompt, the tool list and
//! the remaining request fields. These files pin the whole body. A rename that leaves
//! them byte-identical has not changed what the model sees.
//!
//! To accept an intended change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p craftsman-rs --test golden_model_request` and review the
//! diff of `tests/golden/model_request/*.json` like any other change.
//!
//! The one-shot (`-m`) path is driven with the default permission mode. Two lines of
//! the system prompt describe the host and differ per operating system, so they are
//! replaced by placeholders before comparing: `Operating system:` and `Command shell:`.

use std::path::Path;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn set_test_home(command: &mut tokio::process::Command, home: &Path) {
    command.env("HOME", home);
    command.env("CRAFTSMAN_CONFIG_DIR", home.join(".craftsman"));
    #[cfg(windows)]
    command.env("USERPROFILE", home);
}

struct PlainReply;

impl Respond for PlainReply {
    fn respond(&self, _req: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":1}}\n\n",
                "data: [DONE]\n\n",
            ))
    }
}

/// Replace the two host-dependent lines of the system prompt with fixed text.
fn mask_host_lines(system: &str) -> String {
    system
        .split('\n')
        .map(|line| {
            if line.starts_with("Operating system: ") {
                "Operating system: <os>"
            } else if line.starts_with("Command shell: ") {
                "Command shell: <shell>"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The request body as stored in the golden file: pretty JSON, host lines masked,
/// one trailing newline.
fn render(mut body: serde_json::Value) -> String {
    assert_eq!(
        body["messages"][0]["role"], "system",
        "first message is not the system prompt"
    );
    let system = body["messages"][0]["content"]
        .as_str()
        .expect("the system prompt is a string")
        .to_string();
    body["messages"][0]["content"] = serde_json::Value::String(mask_host_lines(&system));
    let mut text = serde_json::to_string_pretty(&body).unwrap();
    text.push('\n');
    text
}

/// A short stretch of `line` around character `at`.
fn window(line: &str, at: usize) -> String {
    line.chars().skip(at.saturating_sub(40)).take(120).collect()
}

/// Where two renderings first differ. The system prompt is one long JSON line, so the
/// position is given as line and column, with a window of text on each side.
fn first_difference(expected: &str, actual: &str) -> String {
    for (index, (e, a)) in expected.lines().zip(actual.lines()).enumerate() {
        if e != a {
            let at = e.chars().zip(a.chars()).take_while(|(x, y)| x == y).count();
            return format!(
                "line {}, column {}:\n  golden: ...{}...\n  actual: ...{}...",
                index + 1,
                at + 1,
                window(e, at),
                window(a, at)
            );
        }
    }
    format!(
        "one side is longer: the golden has {} lines, the request {}",
        expected.lines().count(),
        actual.lines().count()
    )
}

fn compare(profile: &str, actual: &str) {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/model_request")
        .join(format!("{profile}.json"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&golden)
        .unwrap_or_else(|error| {
            panic!(
                "cannot read {}: {error}\nrecord it with UPDATE_GOLDEN=1",
                golden.display()
            )
        })
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "the model request for the `{profile}` harness profile changed ({})\nfirst difference: {}\n\
         If the change is intended, regenerate with UPDATE_GOLDEN=1 and review the diff.",
        golden.display(),
        first_difference(&expected, actual)
    );
}

async fn check(profile: &str) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(PlainReply)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let craftsman_dir = home.path().join(".craftsman");
    std::fs::create_dir_all(&craftsman_dir).unwrap();
    std::fs::write(
        craftsman_dir.join("config.json"),
        serde_json::json!({
            "base_url": server.uri(),
            "api_key": "test-key",
            "model": "golden-model",
            "harness_profile": profile,
        })
        .to_string(),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_craftsman"));
    command
        .args(["-m", "golden request"])
        .current_dir(workspace.path())
        .stdin(std::process::Stdio::null());
    set_test_home(&mut command, home.path());
    let out = command.output().await.unwrap();
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let requests = server
        .received_requests()
        .await
        .expect("request recording is on");
    let first = requests
        .iter()
        .find(|request| request.url.path() == "/chat/completions")
        .expect("the binary sent no model request");
    let body: serde_json::Value = serde_json::from_slice(&first.body).unwrap();
    compare(profile, &render(body));
}

#[tokio::test]
async fn general_profile_request_is_pinned() {
    check("general").await;
}

#[tokio::test]
async fn openai_profile_request_is_pinned() {
    check("openai").await;
}

#[tokio::test]
async fn claude_profile_request_is_pinned() {
    check("claude").await;
}

#[tokio::test]
async fn nemotron_profile_request_is_pinned() {
    check("nemotron").await;
}

#[test]
fn host_lines_are_masked_and_nothing_else() {
    let masked = mask_host_lines(
        "intro\n# Host environment\nOperating system: macos\nCommand shell: POSIX sh\nUse host-appropriate syntax.",
    );
    assert_eq!(
        masked,
        "intro\n# Host environment\nOperating system: <os>\nCommand shell: <shell>\nUse host-appropriate syntax."
    );
}

#[test]
fn first_difference_names_the_line() {
    assert_eq!(
        first_difference("a\nb\nc\n", "a\nx\nc\n"),
        "line 2, column 1:\n  golden: ...b...\n  actual: ...x..."
    );
    let long_golden = format!("{}X{}", "a".repeat(500), "b".repeat(500));
    let long_actual = format!("{}Y{}", "a".repeat(500), "b".repeat(500));
    let report = first_difference(&long_golden, &long_actual);
    assert!(report.starts_with("line 1, column 501:"), "{report}");
    assert!(
        report.len() < 400,
        "the report must stay short, got {} bytes",
        report.len()
    );
    assert!(first_difference("a\n", "a\nb\n").starts_with("one side is longer"));
}
