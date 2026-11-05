//! The accepted P2a contract's three legacy `-m` fixes, against the real binary
//! and a synthetic local provider. No hosted model, real keys, installed CLI or
//! user config is used. These tests do not claim that `craftsman exec` is shipped.

use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn command(home: &Path, workspace: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_craftsman"));
    command
        .env("HOME", home)
        .env("CRAFTSMAN_CONFIG_DIR", home.join(".craftsman"))
        .current_dir(workspace)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.env("USERPROFILE", home);
    command
}

fn config(home: &Path, base_url: &str, extra: Value) {
    let directory = home.join(".craftsman");
    std::fs::create_dir_all(&directory).unwrap();
    let mut cfg =
        json!({"base_url":base_url, "api_key":"synthetic-test-key", "model":"mock-model"});
    for (key, value) in extra.as_object().unwrap() {
        cfg[key] = value.clone();
    }
    std::fs::write(directory.join("config.json"), cfg.to_string()).unwrap();
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

fn text(value: &str) -> ResponseTemplate {
    sse(&[
        json!({"choices":[{"delta":{"content":value}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
        json!({"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2}}),
    ])
}

fn tool(id: &str, name: &str, args: Value) -> Value {
    json!({"choices":[{"delta":{"tool_calls":[{
        "index":0,"id":id,"function":{"name":name,"arguments":args.to_string()}
    }]}}]})
}

struct DenialAware {
    target: String,
}

impl Respond for DenialAware {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        if let Some(result) = messages.iter().find(|m| m["role"] == "tool") {
            assert!(result["content"].as_str().unwrap().contains("denied"));
            text("denied-without-reading-stdin")
        } else {
            sse(&[
                tool(
                    "write-1",
                    "write_file",
                    json!({"file_path":self.target,"content":"must not be written"}),
                ),
                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            ])
        }
    }
}

#[tokio::test]
async fn non_terminal_approval_bytes_and_an_idle_open_pipe_never_approve_or_block() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("must-not-exist.txt");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(DenialAware {
            target: target.to_string_lossy().into_owned(),
        })
        .mount(&server)
        .await;
    config(home.path(), &server.uri(), json!({}));

    for payload in [&b""[..], &b"y\n"[..], &b"\n"[..], &b"a\n"[..]] {
        let mut command = command(home.path(), workspace.path());
        command.args(["-m", "try the write"]).stdin(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(payload).await.unwrap();
        // Keep the writer open even when empty or after approval-like bytes.
        // An EOF-only deny fix would still hang, and y/Enter must not approve.
        let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
            .await
            .expect("an open non-terminal stdin pipe must not block approval")
            .unwrap();
        drop(stdin);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "denied-without-reading-stdin\n"
        );
        assert!(!target.exists(), "piped bytes are not permission");
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 8);
}

struct CompactionAware {
    main_calls: Arc<AtomicUsize>,
    compaction_calls: Arc<AtomicUsize>,
}

impl Respond for CompactionAware {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let no_tools = body
            .get("tools")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty);
        if no_tools {
            self.compaction_calls.fetch_add(1, Ordering::SeqCst);
            return text("INTERNAL-COMPACTION-DELTA-4ad1");
        }
        match self.main_calls.fetch_add(1, Ordering::SeqCst) {
            0 => sse(&[
                tool("read-1", "read_file", json!({"file_path":"source.txt"})),
                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
                json!({"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2}}),
            ]),
            1 => {
                assert!(
                    body["messages"].as_array().unwrap().iter().any(|m| {
                        m["content"]
                            .as_str()
                            .is_some_and(|s| s.contains("INTERNAL-COMPACTION-DELTA-4ad1"))
                    }),
                    "the summary must still reach the model, just not stdout"
                );
                text("main-after-compaction")
            }
            _ => panic!("unexpected extra main request"),
        }
    }
}

#[tokio::test]
async fn compaction_deltas_stay_off_stdout_but_the_summary_is_retained() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("source.txt"), "source evidence").unwrap();
    let main_calls = Arc::new(AtomicUsize::new(0));
    let compaction_calls = Arc::new(AtomicUsize::new(0));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(CompactionAware {
            main_calls: main_calls.clone(),
            compaction_calls: compaction_calls.clone(),
        })
        .mount(&server)
        .await;
    config(
        home.path(),
        &server.uri(),
        json!({"model_profiles":{"mock-model":{"context_window":1}}}),
    );
    let mut command = command(home.path(), workspace.path());
    command.args(["-m", "read the source"]);
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "main-after-compaction\n"
    );
    assert_eq!(main_calls.load(Ordering::SeqCst), 2);
    assert_eq!(compaction_calls.load(Ordering::SeqCst), 1);
}

struct BrokenPipeAware {
    target: String,
}

impl Respond for BrokenPipeAware {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        // One HTTP/SSE body may stream content and finish a tool-call response
        // without yielding. The post-response guard must prevent the write.
        sse(&[
            json!({"choices":[{"delta":{"content":"consumer-already-closed"}}]}),
            tool(
                "write-after-close",
                "write_file",
                json!({"file_path":self.target,"content":"must not run"}),
            ),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        ])
    }
}

#[tokio::test]
async fn broken_stdout_pipe_exits_quietly_and_never_dispatches_the_following_tool() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("must-not-be-dispatched.txt");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(BrokenPipeAware {
            target: target.to_string_lossy().into_owned(),
        })
        .mount(&server)
        .await;
    config(home.path(), &server.uri(), json!({}));
    let mut command = command(home.path(), workspace.path());
    command.args(["-m", "respond then write", "--auto-approve"]);
    let mut child = command.spawn().unwrap();
    // Drop the sole reader before the binary finishes startup.
    drop(child.stdout.take().unwrap());
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
        "{stderr}"
    );
    assert!(
        !target.exists(),
        "output closure must be checked before dispatch"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_broken_pipe_on_the_final_newline_is_also_a_quiet_exit() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(text(""))
        .mount(&server)
        .await;
    config(home.path(), &server.uri(), json!({}));
    let mut command = command(home.path(), workspace.path());
    command.args(["-m", "return an empty answer"]);
    let mut child = command.spawn().unwrap();
    drop(child.stdout.take().unwrap());
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn broken_pipe_stops_an_in_flight_sse_response_that_never_finishes() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buffer).await.unwrap();
            assert!(
                n > 0 && request.len() + n <= 64 * 1024,
                "bounded HTTP headers expected"
            );
            request.extend_from_slice(&buffer[..n]);
        }
        let chunk =
            "data: {\"choices\":[{\"delta\":{\"content\":\"pipe-closed-mid-stream\"}}]}\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n{:X}\r\n{chunk}\r\n",
            chunk.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        // Deliberately keep the response open: no final choice, DONE or zero chunk.
        // The caller must cancel without waiting for the provider to return.
        std::future::pending::<()>().await;
        drop(stream);
    });
    config(home.path(), &format!("http://{address}"), json!({}));
    let mut command = command(home.path(), workspace.path());
    command.args(["-m", "stream indefinitely"]);
    let mut child = command.spawn().unwrap();
    drop(child.stdout.take().unwrap());
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("a broken stdout pipe must stop even an unfinished provider stream")
        .unwrap();
    assert!(
        !server.is_finished(),
        "the mock provider must still be unfinished"
    );
    server.abort();
    let _ = server.await;
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct RoundCapModel {
    requests: Arc<AtomicUsize>,
}

impl Respond for RoundCapModel {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let round = self.requests.fetch_add(1, Ordering::SeqCst);
        sse(&[
            tool(
                &format!("read-{round}"),
                "read_file",
                json!({"file_path":"source.txt"}),
            ),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        ])
    }
}

#[tokio::test]
async fn legacy_round_cap_keeps_exit_two_instead_of_the_new_exec_limit_code() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("source.txt"), "source").unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(RoundCapModel {
            requests: requests.clone(),
        })
        .mount(&server)
        .await;
    config(home.path(), &server.uri(), json!({}));
    let mut command = command(home.path(), workspace.path());
    command.args(["-m", "keep reading"]);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(requests.load(Ordering::SeqCst), 50);
    assert_eq!(output.stdout, b"\n");
}
