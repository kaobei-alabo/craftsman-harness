//! Actual selected output, real builder/HTTP/tool effects, and a deliberately
//! rebound global stdout in a marked subprocess. No user config or model calls.

use super::*;
use crate::agent::{SessionExtras, build_session_ext};
use crate::config::Config;
use crate::hitl::ApprovalState;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const MARKER: &str = "--oneshot-stdout-owner-v1";
const HELPER: &str = "oneshot_io::native_tests::stdout_owner_fixture";

fn replace_stdout(file: &File) {
    // SAFETY: the marked helper alone owns its process-global stdout slot; the
    // supplied File is a newly owned pipe or an isolated foreign-file control.
    assert_eq!(
        unsafe { libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO) },
        libc::STDOUT_FILENO
    );
}

#[tokio::test]
#[ignore = "marked stdout owner fixture; unmarked/unknown modes return before paths or config"]
async fn stdout_owner_fixture() {
    let args: Vec<String> = std::env::args().collect();
    let markers: Vec<usize> = args
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (value == MARKER).then_some(index))
        .collect();
    let [marker] = markers.as_slice() else {
        return;
    };
    let [mode, endpoint, completed, forbidden, foreign] = &args[marker + 1..] else {
        return;
    };
    if !matches!(
        mode.as_str(),
        "first-response"
            | "after-real-result"
            | "after-first-write"
            | "datagram-refusal"
            | "stream-after-first-write"
    ) {
        return;
    }
    // No path/config/environment/native IO was inspected before the marker.
    if mode == "datagram-refusal" {
        let (reader, sender) = UnixDatagram::pair().unwrap();
        sender.send(b"actual datagram control").unwrap();
        let mut observed = [0; 64];
        let count = reader.recv(&mut observed).unwrap();
        assert_eq!(&observed[..count], b"actual datagram control");
        drop(reader);
        let writer = File::from(OwnedFd::from(sender));
        replace_stdout(&writer);
        let refusal = match super::stdout() {
            Ok(_) => panic!("datagram stdout was admitted before agent construction"),
            Err(error) => error,
        };
        assert_eq!(refusal.kind(), ErrorKind::Unsupported);
        std::process::exit(0);
    }
    let (reader, writer) = if mode == "stream-after-first-write" {
        let (reader, writer) = UnixStream::pair().unwrap();
        (
            File::from(OwnedFd::from(reader)),
            File::from(OwnedFd::from(writer)),
        )
    } else {
        crate::platform_os::stdin_pipe::pair_for_tests().unwrap()
    };
    replace_stdout(&writer);
    let output = super::stdout().unwrap();
    let reader = Arc::new(Mutex::new(Some(reader)));
    let foreign_file = Arc::new(File::create(foreign).unwrap());
    if mode == "first-response" {
        drop(reader.lock().unwrap().take());
        replace_stdout(&foreign_file);
    }
    let round = Arc::new(AtomicUsize::new(0));
    let close_after_result = mode == "after-real-result";
    let round_sink = {
        let round = round.clone();
        let reader = reader.clone();
        let foreign_file = foreign_file.clone();
        Arc::new(move || {
            if round.fetch_add(1, Ordering::SeqCst) == 1 && close_after_result {
                // The first actual tool has returned. Close the sole read end
                // before the second request/response and redirect only the
                // global stdout name, not the previously selected output.
                drop(reader.lock().unwrap().take());
                replace_stdout(&foreign_file);
            }
        }) as crate::agent::RoundSink
    };
    let cfg: Config = serde_json::from_value(json!({
        "model": "mock-model", "base_url": endpoint, "api_key": "synthetic-owner-key"
    }))
    .unwrap();
    let approval = Arc::new(Mutex::new(ApprovalState {
        auto_approve: true,
        ..ApprovalState::default()
    }));
    let token_sink = if matches!(
        mode.as_str(),
        "after-first-write" | "stream-after-first-write"
    ) {
        let output = output.clone();
        let reader = reader.clone();
        Arc::new(move |text: &str| {
            output.write(text.as_bytes());
            if let Some(mut reader) = reader.lock().unwrap().take() {
                let mut observed = vec![0; text.len()];
                reader.read_exact(&mut observed).unwrap();
                assert_eq!(observed, text.as_bytes());
                // The real write succeeded and the actual bytes were read.
                // Close the output peer before this same provider poll hands
                // back the following tool, without setting a synthetic flag.
                drop(reader);
            }
        }) as crate::agent::TokenSink
    } else {
        output.sink()
    };
    let session = build_session_ext(
        &cfg,
        approval,
        crate::prompt::NONINTERACTIVE_INSTRUCTIONS,
        true,
        false,
        None,
        SessionExtras {
            token_sink: Some(token_sink),
            output_closed: Some(output.closed_flag()),
            output_guard: Some(output.guard()),
            round_sink: Some(round_sink),
            ..SessionExtras::default()
        },
    )
    .await
    .unwrap();
    let result = session
        .agent
        .run_turn("exercise the selected output owner")
        .await;
    session.lsp.shutdown().await;
    assert_eq!(output.failure_kind(), Some(ErrorKind::BrokenPipe));
    assert!(matches!(result, Err(AgentError::State(_))));
    assert!(!Path::new(forbidden).exists());
    assert_eq!(std::fs::read(foreign).unwrap(), b"");
    let requests = if close_after_result { 2 } else { 1 };
    assert_eq!(round.load(Ordering::SeqCst), requests);
    assert_eq!(
        *session.usage.lock().unwrap(),
        (requests as u64 * 8, requests as u64 * 2)
    );
    if close_after_result {
        assert_eq!(std::fs::read(completed).unwrap(), b"real completed effect");
        assert!(
            session
                .agent
                .state
                .lock()
                .unwrap()
                .messages
                .iter()
                .any(|message| {
                    matches!(message, craftsman_harness::Msg::Tool { content, .. }
                if content.contains("Updated file"))
                })
        );
    }
    // Only after actual owner/usage/effect/result assertions. The test harness
    // must not write its own completion line into the rebound stdout control.
    std::process::exit(0);
}

fn sse(events: &[Value]) -> ResponseTemplate {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

fn tool(target: &str, id: &str, content: &str) -> Value {
    json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,
        "function":{"name":"write_file","arguments":json!({
            "file_path": target, "content": content
        }).to_string()}}]}}]})
}

struct Script {
    after_result: bool,
    completed: String,
    forbidden: String,
}

impl Respond for Script {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        let completed = messages.iter().any(|message| {
            message["role"] == "tool" && message["tool_call_id"] == "completed-tool"
        });
        let followed = messages.iter().any(|message| {
            message["role"] == "tool" && message["tool_call_id"] == "forbidden-tool"
        });
        if followed {
            return sse(&[
                json!({"choices":[{"delta":{"content":"unexpected follow-up"}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
                json!({"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2}}),
            ]);
        }
        let action = if self.after_result && !completed {
            tool(&self.completed, "completed-tool", "real completed effect")
        } else {
            tool(&self.forbidden, "forbidden-tool", "must not run")
        };
        let mut events = vec![];
        if !self.after_result || completed {
            events.push(json!({"choices":[{"delta":{"content":"selected output content"}}]}));
        }
        events.extend([
            action,
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2}}),
        ]);
        // One actual HTTP body contains content then the following tool.
        sse(&events)
    }
}

fn helper(home: &Path, workspace: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([HELPER, "--exact", "--ignored", "--nocapture", "--"])
        .env_clear()
        .env("HOME", home)
        .env("CRAFTSMAN_CONFIG_DIR", home.join(".craftsman"))
        .current_dir(workspace)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
}

async fn actual_owner_case(after_result: bool) {
    actual_owner_case_mode(if after_result {
        "after-real-result"
    } else {
        "first-response"
    })
    .await;
}

async fn actual_owner_case_mode(mode: &str) {
    let after_result = mode == "after-real-result";
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let completed = workspace.path().join("completed.txt");
    let forbidden = workspace.path().join("must-not-be-dispatched.txt");
    let foreign = workspace.path().join("foreign-stdout.txt");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(Script {
            after_result,
            completed: completed.to_string_lossy().into_owned(),
            forbidden: forbidden.to_string_lossy().into_owned(),
        })
        .mount(&server)
        .await;
    let mut command = helper(home.path(), workspace.path());
    command
        .args([MARKER, mode])
        .arg(server.uri())
        .arg(&completed)
        .arg(&forbidden)
        .arg(&foreign);
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .unwrap()
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "child stderr: {}; real requests: {}; forbidden file: {}; foreign bytes: {}",
        String::from_utf8_lossy(&output.stderr),
        requests.len(),
        forbidden.exists(),
        std::fs::metadata(&foreign).map_or(0, |info| info.len())
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Broken pipe"));
    assert!(
        !forbidden.exists(),
        "source closure must precede same-response dispatch"
    );
    assert_eq!(std::fs::read(foreign).unwrap(), b"");
    assert_eq!(requests.len(), if after_result { 2 } else { 1 });
    if after_result {
        assert_eq!(std::fs::read(completed).unwrap(), b"real completed effect");
        let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert!(
            second["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["role"] == "tool"
                        && message["content"]
                            .as_str()
                            .is_some_and(|content| content.contains("Updated file"))
                })
        );
    }
}

#[tokio::test]
async fn same_response_stdout_rebinding_cannot_redirect_output_or_dispatch_the_following_tool() {
    actual_owner_case(false).await;
}

#[tokio::test]
async fn late_stdout_owner_loss_preserves_actual_prior_tool_result_and_usage() {
    actual_owner_case(true).await;
}

#[tokio::test]
async fn output_peer_closing_after_a_successful_write_stops_the_same_response_tool() {
    actual_owner_case_mode("after-first-write").await;
}

#[tokio::test]
async fn stream_socket_peer_closure_preserves_same_response_dispatch_protection() {
    actual_owner_case_mode("stream-after-first-write").await;
}

#[tokio::test]
async fn datagram_stdout_is_refused_before_model_or_tool_or_storage_effects() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("must-not-be-dispatched.txt");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(Script {
            after_result: false,
            completed: workspace
                .path()
                .join("completed.txt")
                .to_string_lossy()
                .into_owned(),
            forbidden: target.to_string_lossy().into_owned(),
        })
        .mount(&server)
        .await;
    let mut command = helper(home.path(), workspace.path());
    command
        .args([MARKER, "datagram-refusal"])
        .arg(server.uri())
        .arg(workspace.path().join("completed.txt"))
        .arg(&target)
        .arg(workspace.path().join("foreign-stdout.txt"));
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn unmarked_or_unknown_stdout_owner_helpers_leave_isolated_paths_untouched() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    for marker in [None, Some("unknown-mode")] {
        let mut command = helper(home.path(), workspace.path());
        if let Some(mode) = marker {
            command.args([MARKER, mode, "unused", "unused", "unused", "unused"]);
        }
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 0);
    }
}
