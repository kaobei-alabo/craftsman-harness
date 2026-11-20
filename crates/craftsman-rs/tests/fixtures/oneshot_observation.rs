//! Exact selected legacy fixture logic; no original test definitions are included.
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use wiremock::{Request, Respond, ResponseTemplate};

pub(super) fn command(home: &Path, workspace: &Path) -> Command {
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

pub(super) fn config(home: &Path, base_url: &str, extra: Value) {
    let directory = home.join(".craftsman");
    std::fs::create_dir_all(&directory).unwrap();
    let mut cfg =
        json!({"base_url":base_url, "api_key":"synthetic-test-key", "model":"mock-model"});
    for (key, value) in extra.as_object().unwrap() {
        cfg[key] = value.clone();
    }
    std::fs::write(directory.join("config.json"), cfg.to_string()).unwrap();
}

pub(super) fn sse(events: &[Value]) -> ResponseTemplate {
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

pub(super) fn text(value: &str) -> ResponseTemplate {
    sse(&[
        json!({"choices":[{"delta":{"content":value}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
        json!({"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2}}),
    ])
}

pub(super) fn tool(id: &str, name: &str, args: Value) -> Value {
    json!({"choices":[{"delta":{"tool_calls":[{
        "index":0,"id":id,"function":{"name":name,"arguments":args.to_string()}
    }]}}]})
}

pub(super) struct CompactionAware {
    pub(super) main_calls: Arc<AtomicUsize>,
    pub(super) compaction_calls: Arc<AtomicUsize>,
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

pub(super) struct BrokenPipeAware {
    pub(super) target: String,
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

pub(super) fn unfinished(listener: super::ObservedListener) -> tokio::task::JoinHandle<()> {
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
    server
}
