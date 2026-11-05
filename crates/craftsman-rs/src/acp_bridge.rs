//! `craftsman acp-bridge` — WebSocket ⇄ child-process stdio ACP bridge.
//!
//! ACP is a stdio protocol; browsers speak WebSocket. This subcommand is the
//! only translation layer between the two, and it is deliberately dumb: every
//! text frame becomes one newline-delimited JSON line on the child's stdin, and
//! every line the child writes to stdout becomes one text frame. No ACP method
//! is parsed here, so a new harness needs no bridge change.
//!
//! It lives in `craftsman-rs` (not in the workspace addon) so that every harness
//! image can invoke a bridge that is already inside it — the binary never has to
//! be copied across images with mismatched libc.
//!
//! One child per WebSocket connection: the socket's lifetime IS the child's
//! lifetime. Reconnecting clients recover context with ACP `session/load`, not
//! by keeping a child alive across disconnects.
//!
//! The HTTP surface matches `craftsman serve` because the platform depends on it:
//! `/healthz` (StatefulSet readiness probe) and `/stats` (the controller's disk
//! poll). Both are served by the bridge process itself, not by the harness.

use crate::serve::{PROXY_HEADER, authorized, dir_used_bytes, disk_usage, home_dir};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Identity headers the platform's WS proxy asserts on the pod dial. They are
/// forwarded to the harness as env so a multi-tenant harness can attribute the
/// session; the browser never sets them (the platform terminates its JWT).
const USER_HEADER: &str = "x-craftsman-workspace-user";
const TENANT_HEADER: &str = "x-craftsman-workspace-tenant";

/// 60s cache for the `/stats` home walk, matching `craftsman serve`.
const HOME_CACHE_TTL_SECS: u64 = 60;

/// How long a child gets to exit after its stdin closes before it is killed.
const TEARDOWN_GRACE: Duration = Duration::from_secs(3);

/// Hard cap on one JSON-RPC line from the child. A frame this large means the
/// harness is malfunctioning; the session is closed instead of relaying it.
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// The harness command line: `argv[0]` plus its arguments.
#[derive(Clone, Debug)]
pub struct Harness {
    pub command: String,
    pub args: Vec<String>,
}

impl Harness {
    /// Split a `--`-terminated argv tail into a harness command line.
    ///
    /// An empty tail falls back to this binary's own ACP server, which makes
    /// `craftsman acp-bridge` useful with no arguments at all.
    #[must_use]
    pub fn from_argv(argv: Vec<String>) -> Harness {
        let mut iter = argv.into_iter();
        match iter.next() {
            Some(command) => Harness {
                command,
                args: iter.collect(),
            },
            None => Harness {
                command: std::env::current_exe()
                    .ok()
                    .and_then(|p| p.to_str().map(str::to_owned))
                    .unwrap_or_else(|| "craftsman-rs".into()),
                args: vec!["acp-serve".into()],
            },
        }
    }
}

#[derive(Clone)]
struct AppState {
    harness: Arc<Harness>,
    started: Instant,
    home_cache: Arc<Mutex<Option<(Instant, u64)>>>,
}

/// Build the bridge router. Exposed for tests.
pub fn build_router(harness: Harness) -> Router {
    let state = AppState {
        harness: Arc::new(harness),
        started: Instant::now(),
        home_cache: Arc::new(Mutex::new(None)),
    };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/stats", get(stats))
        .route("/chat", get(chat_upgrade))
        .route("/", get(chat_upgrade))
        .with_state(state)
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "harness": state.harness.command,
        "uptime_s": state.started.elapsed().as_secs_f64(),
    }))
}

/// Same payload shape as `craftsman serve`'s `/stats`: the controller reads only
/// the `home` block (see the addon's `pod_stats.rs`).
async fn stats(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return (StatusCode::FORBIDDEN, Json(json!({"detail": "forbidden"}))).into_response();
    }
    let home = home_dir();
    let (disk_used, disk_total) = disk_usage(&home);
    let cached = {
        let cache = state.home_cache.lock().unwrap();
        cache.and_then(|(at, v)| (at.elapsed().as_secs() < HOME_CACHE_TTL_SECS).then_some(v))
    };
    let home_used = match cached {
        Some(v) => v,
        None => {
            let walk_root = home.clone();
            let v = tokio::task::spawn_blocking(move || dir_used_bytes(&walk_root))
                .await
                .unwrap_or(0);
            *state.home_cache.lock().unwrap() = Some((Instant::now(), v));
            v
        }
    };
    Json(json!({
        "disk": {"used_bytes": disk_used, "total_bytes": disk_total},
        "home": {"used_bytes": home_used},
        "harness": state.harness.command,
        "uptime_s": state.started.elapsed().as_secs_f64(),
    }))
    .into_response()
}

async fn chat_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !authorized(&headers) {
        eprintln!("[acp-bridge] rejected dial: missing or wrong {PROXY_HEADER}");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"detail": "unauthorized"})),
        )
            .into_response();
    }
    let mut env = HashMap::new();
    if let Some(user) = header_str(&headers, USER_HEADER) {
        env.insert("CRAFTSMAN_WORKSPACE_USER_ID".to_string(), user);
    }
    if let Some(tenant) = header_str(&headers, TENANT_HEADER) {
        env.insert("CRAFTSMAN_WORKSPACE_TENANT_ID".to_string(), tenant);
    }
    let harness = state.harness.clone();
    ws.on_upgrade(move |socket| bridge_session(socket, harness, env))
        .into_response()
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

/// Relay one WebSocket ⇄ one child process until either side ends.
pub async fn bridge_session(ws: WebSocket, harness: Arc<Harness>, env: HashMap<String, String>) {
    let mut cmd = Command::new(&harness.command);
    cmd.args(&harness.args)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // The harness logs to stderr; it belongs in the pod log, not the wire.
        .stderr(Stdio::inherit())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!(
                "[acp-bridge] failed to spawn harness `{}`: {e}",
                harness.command
            );
            close_with(ws, 1011, "harness failed to start").await;
            return;
        }
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        eprintln!("[acp-bridge] harness stdio was not captured");
        let _ = child.kill().await;
        close_with(ws, 1011, "harness stdio unavailable").await;
        return;
    };

    let mut lines = BufReader::new(stdout).lines();
    let (mut tx, mut rx) = ws.split();
    let mut close: Option<(u16, String)> = None;

    loop {
        tokio::select! {
            inbound = rx.next() => match inbound {
                Some(Ok(WsMessage::Text(text))) => {
                    if write_frame(&mut stdin, text.as_bytes()).await.is_err() { break }
                }
                Some(Ok(WsMessage::Binary(bytes))) => {
                    if write_frame(&mut stdin, &bytes).await.is_err() { break }
                }
                Some(Ok(WsMessage::Ping(payload))) => {
                    if tx.send(WsMessage::Pong(payload)).await.is_err() { break }
                }
                Some(Ok(WsMessage::Pong(_))) => {}
                Some(Ok(WsMessage::Close(_))) | None => break,
                Some(Err(e)) => {
                    eprintln!("[acp-bridge] websocket read failed: {e}");
                    break;
                }
            },

            // `Lines::next_line` keeps its partial buffer in the struct, so it
            // is cancel-safe inside `select!`.
            outbound = lines.next_line() => match outbound {
                Ok(Some(line)) => {
                    let frame = line.trim_end_matches('\r');
                    if frame.len() > MAX_FRAME_BYTES {
                        close = Some((1009, "harness frame exceeds size limit".into()));
                        break;
                    }
                    if frame.is_empty() { continue }
                    if tx.send(WsMessage::Text(frame.to_string().into())).await.is_err() { break }
                }
                Ok(None) => {
                    close = Some((1000, "harness closed its output".into()));
                    break;
                }
                Err(e) => {
                    eprintln!("[acp-bridge] harness stdout read failed: {e}");
                    close = Some((1011, "harness output unreadable".into()));
                    break;
                }
            },

            exited = child.wait() => {
                let (code, reason) = match &exited {
                    Ok(status) if status.success() => (1000, format!("harness exited with {status}")),
                    Ok(status) => (1011, format!("harness exited with {status}")),
                    Err(e) => (1011, format!("harness wait failed: {e}")),
                };
                // Drain whatever the child wrote before exiting so the client
                // still sees the final JSON-RPC error/response.
                while let Ok(Some(line)) = lines.next_line().await {
                    let frame = line.trim_end_matches('\r');
                    if !frame.is_empty()
                        && tx.send(WsMessage::Text(frame.to_string().into())).await.is_err() {
                        break;
                    }
                }
                let _ = tx.send(WsMessage::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                return;
            }
        }
    }

    if let Some((code, reason)) = close {
        let _ = tx
            .send(WsMessage::Close(Some(CloseFrame {
                code,
                reason: reason.into(),
            })))
            .await;
    }
    teardown(child, stdin).await;
}

async fn write_frame(
    stdin: &mut tokio::process::ChildStdin,
    payload: &[u8],
) -> Result<(), std::io::Error> {
    stdin.write_all(payload).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}

/// Closing stdin is the graceful stop for a stdio agent: it observes EOF and
/// exits. Only a child that ignores EOF is killed.
async fn teardown(mut child: tokio::process::Child, stdin: tokio::process::ChildStdin) {
    drop(stdin);
    match tokio::time::timeout(TEARDOWN_GRACE, child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => eprintln!("[acp-bridge] harness wait failed during teardown: {e}"),
        Err(_) => {
            eprintln!("[acp-bridge] harness ignored stdin EOF; killing");
            let _ = child.kill().await;
        }
    }
}

async fn close_with(ws: WebSocket, code: u16, reason: &str) {
    let (mut tx, _rx) = ws.split();
    let _ = tx
        .send(WsMessage::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
}

/// Entrypoint for `craftsman acp-bridge`.
///
/// Besides the HTTP/WS surface this also owns the workspace inbox poller: the
/// bridge is the pod's only long-lived process, so the scheduled/offline
/// message worker has to hang off it. The poller spawns its own harness child
/// per message rather than sharing one with a browser session.
pub async fn run(host: &str, port: u16, harness: Harness) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    eprintln!(
        "craftsman acp-bridge listening on {host}:{port} → {} {}",
        harness.command,
        harness.args.join(" ")
    );
    let _poller = crate::inbox_poller::spawn(Arc::new(harness.clone()));
    axum::serve(listener, build_router(harness)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    // Health identity only; process IO tests use the native fixture below so
    // Windows does not depend on a cold Git-Bash `cat`/`sh` startup.
    fn echo_harness() -> Harness {
        Harness {
            command: "cat".into(),
            args: vec![],
        }
    }

    const FIXTURE_MARKER: &str = "__craftsman_bridge_stdio_fixture__";
    const FIXTURE_READY: &str = r#"{"jsonrpc":"2.0","method":"test_fixture/ready"}"#;
    const FIXTURE_EXIT: &str = r#"{"jsonrpc":"2.0","method":"test_fixture/exit"}"#;
    const FIXTURE_BYE: &str = r#"{"jsonrpc":"2.0","method":"bye"}"#;

    fn stdio_harness() -> Harness {
        Harness {
            command: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: vec![
                "--ignored".into(),
                "--exact".into(),
                "acp_bridge::tests::stdio_harness_fixture".into(),
                "--nocapture".into(),
                "--quiet".into(),
                // A libtest skip value is also an explicit child-only marker;
                // no process-global environment is mutated by the parent.
                "--skip".into(),
                FIXTURE_MARKER.into(),
            ],
        }
    }

    #[test]
    #[ignore = "native stdio subprocess fixture; invoked by bridge IO tests"]
    fn stdio_harness_fixture() {
        // Running all ignored tests manually must not read/hang on user stdin.
        if !std::env::args().any(|arg| arg == FIXTURE_MARKER) {
            return;
        }
        use std::io::{BufRead, Write};
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "\n{FIXTURE_READY}").unwrap();
        stdout.flush().unwrap();
        for line in std::io::stdin().lock().lines() {
            let line = line.unwrap();
            if line == FIXTURE_EXIT {
                writeln!(stdout, "{FIXTURE_BYE}").unwrap();
                stdout.flush().unwrap();
                // No libtest summary after bye: the next wire event must close.
                std::process::exit(0);
            }
            writeln!(stdout, "{line}").unwrap();
            stdout.flush().unwrap();
        }
        std::process::exit(0);
    }

    async fn await_fixture_ready(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) {
        // Startup gets its own bound. The lifecycle assertion starts only after
        // this real stdout/stdin handshake, not after a WS upgrade/process spawn.
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let message = ws
                    .next()
                    .await
                    .expect("fixture socket ended before ready")
                    .expect("fixture socket failed before ready");
                let ClientMessage::Text(text) = message else {
                    panic!("unexpected fixture startup frame: {message:?}")
                };
                if text == FIXTURE_READY {
                    break;
                }
                assert_eq!(
                    text.trim(),
                    "running 1 test",
                    "unexpected native fixture preamble"
                );
            }
        })
        .await
        .expect("native fixture did not become ready");
    }

    async fn serve(harness: Harness) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = build_router(harness);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("{addr}")
    }

    #[tokio::test]
    async fn healthz_reports_the_configured_harness() {
        let addr = serve(echo_harness()).await;
        let body: Value = reqwest::get(format!("http://{addr}/healthz"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["harness"], "cat");
    }

    // `/stats` is deliberately not exercised end to end here: the 200 path
    // walks the real home directory, which costs a minute on a dev machine.
    // Its pieces (`disk_usage`, `dir_used_bytes`, `authorized`) are unit-tested
    // in `serve`.

    #[tokio::test]
    async fn frames_round_trip_verbatim_in_both_directions() {
        let addr = serve(stdio_harness()).await;
        let (mut ws, _) = connect_async(format!("ws://{addr}/chat")).await.unwrap();
        await_fixture_ready(&mut ws).await;
        for id in 1..=3 {
            let frame = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"session/prompt"}}"#);
            ws.send(ClientMessage::Text(frame.clone().into()))
                .await
                .unwrap();
            let echoed = tokio::time::timeout(Duration::from_secs(3), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(echoed.into_text().unwrap().as_str(), frame);
        }
    }

    #[tokio::test]
    async fn harness_exit_closes_the_socket() {
        let addr = serve(stdio_harness()).await;
        let (mut ws, _) = connect_async(format!("ws://{addr}/chat")).await.unwrap();
        await_fixture_ready(&mut ws).await;
        ws.send(ClientMessage::Text(FIXTURE_EXIT.into()))
            .await
            .unwrap();

        let first = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("ready fixture did not send its final frame")
            .expect("fixture closed before its final frame")
            .unwrap();
        assert_eq!(first.into_text().unwrap().as_str(), FIXTURE_BYE);

        // Unlike the old conditional check, a timeout/error is never success.
        match tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("exited fixture did not close the socket")
        {
            Some(Ok(ClientMessage::Close(_))) | None => {}
            other => panic!("expected close after the final frame, got {other:?}"),
        }
    }

    // The proxy-secret gate is `serve::authorized`, covered by that module's
    // test; mutating the shared env from a second test would race it.

    #[test]
    fn empty_argv_falls_back_to_this_binarys_acp_server() {
        let harness = Harness::from_argv(vec![]);
        assert_eq!(harness.args, vec!["acp-serve"]);
    }

    #[test]
    fn argv_tail_becomes_the_harness_command_line() {
        let harness = Harness::from_argv(vec!["goose".into(), "acp".into(), "--quiet".into()]);
        assert_eq!(harness.command, "goose");
        assert_eq!(harness.args, vec!["acp", "--quiet"]);
    }
}
