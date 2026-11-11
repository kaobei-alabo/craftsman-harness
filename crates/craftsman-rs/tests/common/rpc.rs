//! Clients that talk to the program under test and record every frame:
//! newline-delimited JSON-RPC over a child's stdio, JSON text frames over a
//! WebSocket, and plain HTTP GETs. Every wait has a timeout, and a failure
//! prints the transcript so far (against its golden, when the scenario named
//! one) and the child's stderr.

use super::golden::Transcript;
use super::process::{Proc, STEP_TIMEOUT, StderrLog};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::TcpStream;
use tokio::process::{ChildStdin, ChildStdout};
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderName;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

fn fail(what: &str, transcript: &Transcript, stderr: &StderrLog) -> ! {
    let against_golden = transcript
        .progress_report()
        .map(|report| format!("--- against the golden ---\n{report}\n"))
        .unwrap_or_default();
    panic!(
        "{what}\n--- frames so far ---\n{}\n{against_golden}--- stderr (last lines) ---\n{}",
        transcript.outline(),
        stderr.tail(40)
    )
}

/// The value at JSON pointer `pointer` of a frame the driver has to read.
/// `Err` names the frame, so that a renamed or dropped field fails at once
/// with the frame in the message instead of making the driver send `null` and
/// wait out a timeout.
fn field_of<'a>(frame: &'a Value, pointer: &str) -> Result<&'a Value, String> {
    match frame.pointer(pointer) {
        Some(value) if !value.is_null() => Ok(value),
        _ => Err(format!(
            "the frame has no `{pointer}` that the test needs to answer it: {frame}"
        )),
    }
}

fn string_field_of(frame: &Value, pointer: &str) -> Result<String, String> {
    let value = field_of(frame, pointer)?;
    value.as_str().map(str::to_string).ok_or_else(|| {
        format!("`{pointer}` of the frame is not a string, as the test needs it to be: {frame}")
    })
}

/// A child speaking newline-delimited JSON-RPC on stdin/stdout (`acp-serve`).
pub struct StdioRpc {
    proc: Proc,
    stdin: Option<ChildStdin>,
    lines: Lines<BufReader<ChildStdout>>,
    pub transcript: Transcript,
}

impl StdioRpc {
    pub fn spawn(name: &str, command: tokio::process::Command) -> StdioRpc {
        let (proc, stdin, stdout) = Proc::spawn_stdio(name, command);
        StdioRpc {
            proc,
            stdin: Some(stdin),
            lines: BufReader::new(stdout).lines(),
            transcript: Transcript::default(),
        }
    }

    pub async fn send(&mut self, frame: Value) {
        let mut line = frame.to_string();
        line.push('\n');
        let written = {
            let stdin = self.stdin.as_mut().expect("stdin is open");
            tokio::time::timeout(STEP_TIMEOUT, async {
                stdin.write_all(line.as_bytes()).await?;
                stdin.flush().await
            })
            .await
        };
        match written {
            Ok(Ok(())) => self.transcript.send(&frame),
            Ok(Err(error)) => self.fail(&format!("could not write {frame} to the child: {error}")),
            Err(_) => self.fail(&format!("writing {frame} to the child timed out")),
        }
    }

    /// The next frame the child wrote to stdout.
    pub async fn recv(&mut self, waiting_for: &str) -> Value {
        match self.read_frame(waiting_for).await {
            Some(frame) => frame,
            None => self.fail(&format!("stdout closed while waiting for {waiting_for}")),
        }
    }

    /// The next frame, or `None` once the child has closed stdout.
    async fn read_frame(&mut self, waiting_for: &str) -> Option<Value> {
        let line = match tokio::time::timeout(STEP_TIMEOUT, self.lines.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => return None,
            Ok(Err(error)) => self.fail(&format!("reading stdout failed: {error}")),
            Err(_) => self.fail(&format!(
                "no frame within {STEP_TIMEOUT:?} while waiting for {waiting_for}"
            )),
        };
        match serde_json::from_str::<Value>(&line) {
            Ok(frame) => {
                self.transcript.recv(&frame);
                Some(frame)
            }
            // stdout carries JSON-RPC only; anything else is a protocol break.
            Err(error) => self.fail(&format!("stdout line is not JSON ({error}): {line:?}")),
        }
    }

    /// Read frames, recording each, until one satisfies `done`.
    pub async fn recv_until(
        &mut self,
        waiting_for: &str,
        mut done: impl FnMut(&Value) -> bool,
    ) -> Value {
        loop {
            let frame = self.recv(waiting_for).await;
            if done(&frame) {
                return frame;
            }
        }
    }

    /// Read frames until the response to request `id`.
    pub async fn recv_response(&mut self, id: u64) -> Value {
        self.recv_until(&format!("the response to request {id}"), |frame| {
            frame["id"] == json!(id) && frame.get("method").is_none()
        })
        .await
    }

    /// Close stdin, which is how a bridge stops a stdio agent, and require a
    /// prompt, clean exit. Whatever the child still writes before it closes
    /// stdout is recorded too, so a stray trailing frame fails the golden.
    pub async fn finish(mut self) -> Transcript {
        self.stdin = None;
        while self.read_frame("the child to exit").await.is_some() {}
        self.proc.wait_success("after stdin was closed").await;
        self.transcript.clone()
    }

    /// `frame`'s value at JSON pointer `pointer`, which the test needs for its
    /// next request; fails the test at once, naming the frame, if it is
    /// missing.
    pub fn field(&self, frame: &Value, pointer: &str) -> Value {
        field_of(frame, pointer)
            .unwrap_or_else(|reason| self.fail(&reason))
            .clone()
    }

    /// Like [`StdioRpc::field`], for a string.
    pub fn str_field(&self, frame: &Value, pointer: &str) -> String {
        string_field_of(frame, pointer).unwrap_or_else(|reason| self.fail(&reason))
    }

    fn fail(&self, what: &str) -> ! {
        fail(what, &self.transcript, &self.proc.stderr)
    }
}

/// A WebSocket client exchanging JSON text frames.
pub struct WsRpc {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    transcript: Transcript,
    stderr: StderrLog,
}

/// How a WebSocket upgrade attempt ended.
enum Upgrade {
    Accepted(Box<WebSocketStream<MaybeTlsStream<TcpStream>>>),
    Refused { status: u16 },
}

/// Attempt the upgrade of `GET <path>` and record it as an `http` step: `101`
/// when accepted, the refusing status (and JSON body, if any) otherwise.
async fn upgrade(
    address: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
    transcript: &Transcript,
    stderr: &StderrLog,
) -> Upgrade {
    let url = format!("ws://{address}{path}");
    let mut request = url
        .as_str()
        .into_client_request()
        .expect("websocket request");
    for (name, value) in headers {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            value.parse().expect("header value"),
        );
    }
    let mut exchange = json!({"request": format!("GET {path} (WebSocket upgrade)")});
    if !headers.is_empty() {
        exchange["request_headers"] = request_headers(headers);
    }
    match tokio::time::timeout(STEP_TIMEOUT, tokio_tungstenite::connect_async(request)).await {
        Ok(Ok((socket, response))) => {
            exchange["status"] = json!(response.status().as_u16());
            transcript.http(exchange);
            Upgrade::Accepted(Box::new(socket))
        }
        Ok(Err(WsError::Http(response))) => {
            let status = response.status().as_u16();
            exchange["status"] = json!(status);
            if let Some(body) = response
                .body()
                .as_ref()
                .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            {
                exchange["body"] = body;
            }
            transcript.http(exchange);
            Upgrade::Refused { status }
        }
        Ok(Err(error)) => fail(
            &format!("websocket upgrade of {url} failed: {error}"),
            transcript,
            stderr,
        ),
        Err(_) => fail(
            &format!("websocket upgrade of {url} timed out"),
            transcript,
            stderr,
        ),
    }
}

fn request_headers(headers: &[(&str, &str)]) -> Value {
    headers
        .iter()
        .map(|(name, value)| ((*name).to_string(), json!(value)))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

impl WsRpc {
    /// Upgrade `GET <path>` and require it to be accepted.
    pub async fn connect(
        address: SocketAddr,
        path: &str,
        headers: &[(&str, &str)],
        transcript: Transcript,
        stderr: StderrLog,
    ) -> WsRpc {
        match upgrade(address, path, headers, &transcript, &stderr).await {
            Upgrade::Accepted(socket) => WsRpc {
                socket: *socket,
                transcript,
                stderr,
            },
            Upgrade::Refused { status } => fail(
                &format!("websocket upgrade of {path} was refused with {status}"),
                &transcript,
                &stderr,
            ),
        }
    }

    /// Upgrade `GET <path>` and require it to be refused; returns the status.
    pub async fn connect_refused(
        address: SocketAddr,
        path: &str,
        headers: &[(&str, &str)],
        transcript: &Transcript,
        stderr: &StderrLog,
    ) -> u16 {
        match upgrade(address, path, headers, transcript, stderr).await {
            Upgrade::Refused { status } => status,
            Upgrade::Accepted(_) => fail(
                &format!("websocket upgrade of {path} was accepted but should be refused"),
                transcript,
                stderr,
            ),
        }
    }

    pub async fn send(&mut self, frame: Value) {
        let sent = tokio::time::timeout(
            STEP_TIMEOUT,
            self.socket.send(Message::Text(frame.to_string().into())),
        )
        .await;
        match sent {
            Ok(Ok(())) => self.transcript.send(&frame),
            Ok(Err(error)) => self.fail(&format!("could not send {frame}: {error}")),
            Err(_) => self.fail(&format!("sending {frame} timed out")),
        }
    }

    /// The next JSON text frame; pings and pongs are skipped.
    pub async fn recv(&mut self, waiting_for: &str) -> Value {
        loop {
            let message = match tokio::time::timeout(STEP_TIMEOUT, self.socket.next()).await {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(error))) => self.fail(&format!("websocket read failed: {error}")),
                Ok(None) => self.fail(&format!("websocket ended while waiting for {waiting_for}")),
                Err(_) => self.fail(&format!(
                    "no frame within {STEP_TIMEOUT:?} while waiting for {waiting_for}"
                )),
            };
            if let Some(frame) = self.decode(message, waiting_for) {
                return frame;
            }
        }
    }

    /// Record the JSON in a text frame; `None` for pings and pongs. Anything
    /// else is a protocol break.
    fn decode(&self, message: Message, context: &str) -> Option<Value> {
        match message {
            Message::Text(text) => match serde_json::from_str::<Value>(text.as_str()) {
                Ok(frame) => {
                    self.transcript.recv(&frame);
                    Some(frame)
                }
                Err(error) => self.fail(&format!("text frame is not JSON ({error}): {text}")),
            },
            Message::Ping(_) | Message::Pong(_) => None,
            other => self.fail(&format!(
                "unexpected websocket message while waiting for {context}: {other:?}"
            )),
        }
    }

    /// Record whatever the server still sends until it has been quiet for
    /// `quiet`, so that a stray frame after the final one fails the golden.
    pub async fn drain_quiet(&mut self, quiet: Duration) {
        // A cap, so that a server that never stops cannot hold the test open.
        for _ in 0..256 {
            let message = match tokio::time::timeout(quiet, self.socket.next()).await {
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Err(_) => return,
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(error))) => self.fail(&format!("websocket read failed: {error}")),
            };
            self.decode(message, "the connection to go quiet");
        }
        self.fail("the server kept sending frames after the final one");
    }

    /// Read frames, recording each, until one satisfies `done`.
    pub async fn recv_until(
        &mut self,
        waiting_for: &str,
        mut done: impl FnMut(&Value) -> bool,
    ) -> Value {
        loop {
            let frame = self.recv(waiting_for).await;
            if done(&frame) {
                return frame;
            }
        }
    }

    pub async fn close(mut self) {
        let _ = tokio::time::timeout(STEP_TIMEOUT, self.socket.close(None)).await;
    }

    fn fail(&self, what: &str) -> ! {
        fail(what, &self.transcript, &self.stderr)
    }
}

/// An HTTP client for one server; each GET is recorded as an `http` step.
pub struct Http {
    client: reqwest::Client,
    address: SocketAddr,
    transcript: Transcript,
    stderr: StderrLog,
}

impl Http {
    pub fn new(address: SocketAddr, transcript: Transcript, stderr: StderrLog) -> Http {
        Http {
            // Loopback only: never route these through a developer's proxy.
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(STEP_TIMEOUT)
                .build()
                .expect("http client"),
            address,
            transcript,
            stderr,
        }
    }

    /// Prepare the real bridge's cold home walk before the strict recorded
    /// /stats request. On Windows, native profile discovery is not redirected
    /// just by HOME/USERPROFILE, so the first walk can include runner caches.
    /// This setup is bounded independently, validates a REAL authorized response,
    /// and never records/retries/fabricates a golden exchange. The ordinary
    /// request timeout and scenario cap are unchanged.
    pub async fn prepare_stats_cache(&self, headers: &[(&str, &str)]) -> Value {
        const STATS_SETUP_TIMEOUT: Duration = Duration::from_secs(60);
        let mut request = self
            .client
            .get(format!("http://{}/stats", self.address))
            .timeout(STATS_SETUP_TIMEOUT);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().await.unwrap_or_else(|error| {
            fail(
                &format!("bounded cold /stats setup failed: {error:?}"),
                &self.transcript,
                &self.stderr,
            )
        });
        if response.status() != reqwest::StatusCode::OK {
            fail(
                "cold /stats setup was not authorized",
                &self.transcript,
                &self.stderr,
            );
        }
        response.json().await.unwrap_or_else(|error| {
            fail(
                &format!("cold /stats setup did not return JSON: {error:?}"),
                &self.transcript,
                &self.stderr,
            )
        })
    }

    /// GET `path`; returns the status and the parsed JSON body.
    pub async fn get(&self, path: &str, headers: &[(&str, &str)]) -> (u16, Value) {
        let mut request = self.client.get(format!("http://{}{path}", self.address));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => fail(
                &format!("GET {path} failed: {error}"),
                &self.transcript,
                &self.stderr,
            ),
        };
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body: Value = match response.json().await {
            Ok(body) => body,
            Err(error) => fail(
                &format!("GET {path} returned a body that is not JSON: {error}"),
                &self.transcript,
                &self.stderr,
            ),
        };
        let mut exchange = json!({
            "request": format!("GET {path}"),
            "status": status,
            "content_type": content_type,
            "body": body,
        });
        if !headers.is_empty() {
            exchange["request_headers"] = request_headers(headers);
        }
        self.transcript.http(exchange.clone());
        (status, exchange["body"].take())
    }
}
