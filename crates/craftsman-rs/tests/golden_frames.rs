//! Golden frames for the retained ACP stdio and WebSocket bridge protocols.
//!
//! These hermetic tests drive the real binary against a scripted loopback model
//! with scrubbed child environments and isolated workspace/home directories.
//! They compare permission turns, model-call cancellation and the bridge's
//! health/stat/auth/relay surface against the existing ACP golden files.
//!
//! Only machine-dependent paths, ports, thread IDs, crate versions and numeric
//! bridge statistics are normalized. Other frame keys, values and ordering stay
//! exact. The recordings reject model credentials and local machine paths.
//!
//! The owner retired the legacy desktop-v4 transport and its dedicated scenario;
//! retained ACP golden bytes and assertions are unchanged. Existing ACP quirks
//! (permission toolCallId and repeated messageId) remain frozen in these fixtures.
//! Fixing a retained wire behavior requires a reviewed protocol change rather
//! than regenerating a golden to conceal a failure. UPDATE_GOLDEN is refused in CI.

mod common;

use common::golden::{Case, Kind, Normalizer, Transcript, check};
use common::mock_llm::{MockModel, Round};
use common::process::{MODEL_API_KEY, Proc, STEP_TIMEOUT, Sandbox, model_config};
use common::rpc::{Http, StdioRpc, WsRpc};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;

/// Craftsman's additions to ACP, advertised at `initialize` so that a stock
/// client can feature-detect them (`acp_serve.rs`, AGENTS.md). Adding to this
/// list is a protocol change.
const CRAFTSMAN_ACP_EXTENSIONS: [&str; 3] = [
    "_craftsman/session/request_input",
    "_craftsman/session/set_model",
    "_craftsman/session/list",
];

const PROXY_HEADER: &str = "x-craftsman-workspace-proxy";
const PROXY_SECRET_ENV: &str = "CRAFTSMAN_WORKSPACE_PROXY_SECRET";
const PROXY_SECRET: &str = "golden-proxy-secret";

/// How long a WebSocket must stay silent to count as quiet: after a
/// scenario's last frame (nothing may follow it).
const QUIET_PERIOD: Duration = Duration::from_millis(200);

/// A model turn that asks to create `golden.txt`, then confirms.
fn write_file_turn() -> Vec<Round> {
    vec![
        Round::tool_call(
            &["Creating the file. "],
            "call_1",
            "write_file",
            json!({"file_path": "golden.txt", "content": "hello golden\n"}),
            (11, 5),
        ),
        Round::text(&["Created ", "golden.txt."], (20, 4)),
    ]
}

/// What the agent sent the model, checked independently of the golden: the
/// approved write ran, and its result went back to the model.
async fn assert_write_file_turn_reached_the_model(model: &MockModel) {
    let requests = model.requests().await;
    assert_eq!(requests.len(), 2, "one request per scripted round");
    let tools = requests[0]["tools"].as_array().expect("tools offered");
    assert!(
        tools
            .iter()
            .any(|tool| tool["function"]["name"] == "write_file"),
        "write_file is offered to the model"
    );
    let messages = requests[1]["messages"].as_array().expect("messages");
    let result = messages
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("the tool result is sent back to the model");
    assert_eq!(result["tool_call_id"], "call_1");
    assert!(
        result["content"]
            .as_str()
            .is_some_and(|content| content.contains("Updated file golden.txt")),
        "tool result: {result}"
    );
}

fn acp_initialize(id: u64) -> Value {
    acp_initialize_asking_for(id, 1)
}

/// An `initialize` request that asks for ACP `protocol_version`.
fn acp_initialize_asking_for(id: u64, protocol_version: u64) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "initialize",
        "params": {
            "protocolVersion": protocol_version,
            "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}},
            "clientInfo": {"name": "golden-client", "version": "1.0.0"},
        },
    })
}

/// The one rule every ACP `initialize` response needs: the agent version is
/// the crate version, which changes on every release.
fn acp_normalizer(sandbox: &Sandbox) -> Normalizer {
    Normalizer::new()
        .directory(sandbox.workspace.path(), "<WORKSPACE>")
        .directory(sandbox.home.path(), "<HOME>")
        .forbid(MODEL_API_KEY)
        .mask_exact(
            "/recv/result/agentInfo/version",
            env!("CARGO_PKG_VERSION"),
            "<crate-version>",
        )
}

fn assert_acp_initialize_response(response: &Value) {
    let result = &response["result"];
    assert_eq!(result["protocolVersion"], 1);
    assert_eq!(result["agentInfo"]["name"], "craftsman-harness");
    assert_eq!(result["agentInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        result["_meta"]["craftsman"]["extensions"],
        json!(CRAFTSMAN_ACP_EXTENSIONS),
        "Craftsman-only ACP behavior must be advertised in _meta.craftsman.extensions"
    );
}

// ── ACP over stdio ──────────────────────────────────────────────────────────

#[tokio::test]
async fn acp_stdio_permission_turn() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let model = MockModel::start(write_file_turn()).await;
        let sandbox = Sandbox::new(Some(model_config(&model.url())));
        let mut agent = StdioRpc::spawn(
            "craftsman acp-serve",
            sandbox.craftsman(&["acp-serve", "--permission-mode", "manual"]),
        );
        agent
            .transcript
            .expect_golden("acp_stdio_permission_turn", acp_normalizer(&sandbox));

        agent.send(acp_initialize(1)).await;
        assert_acp_initialize_response(&agent.recv_response(1).await);

        // A stock client: no _meta.craftsman.threadId, so the agent mints the id.
        agent
            .send(json!({
                "jsonrpc": "2.0", "id": 2, "method": "session/new",
                "params": {"cwd": sandbox.workspace.path(), "mcpServers": []},
            }))
            .await;
        let created = agent.recv_response(2).await;
        let session_id = agent.str_field(&created, "/result/sessionId");
        assert_eq!(
            created["result"]["_meta"]["craftsman"]["threadId"],
            session_id
        );

        agent
            .send(json!({
                "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [{"type": "text", "text": "create golden.txt"}],
                },
            }))
            .await;
        let request = agent
            .recv_until("session/request_permission", |frame| {
                frame["method"] == "session/request_permission"
            })
            .await;
        assert!(
            !sandbox.path_in_workspace("golden.txt").exists(),
            "the write must wait for the client's answer"
        );

        let request_id = agent.field(&request, "/id");
        agent
            .send(json!({
                "jsonrpc": "2.0", "id": request_id,
                "result": {"outcome": {"outcome": "selected", "optionId": "allow-once"}},
            }))
            .await;
        let response = agent.recv_response(3).await;
        assert_eq!(response["result"]["stopReason"], "end_turn");

        assert_eq!(
            std::fs::read_to_string(sandbox.path_in_workspace("golden.txt")).unwrap(),
            "hello golden\n"
        );
        assert_write_file_turn_reached_the_model(&model).await;

        let transcript = agent.finish().await;
        transcript.require("the permission request", |step| {
            step["recv"]["method"] == "session/request_permission"
        });
        transcript.require("the client's approval", |step| {
            step["send"]["result"]["outcome"]["optionId"] == "allow-once"
        });
        for kind in ["tool_call", "tool_call_update"] {
            transcript.require(kind, |step| {
                step["recv"]["params"]["update"]["sessionUpdate"] == kind
            });
        }
        transcript.require("the end_turn response", |step| {
            step["recv"]["result"]["stopReason"] == "end_turn"
        });
        check(
            Case {
                name: "acp_stdio_permission_turn",
                description: "ACP over stdio (`craftsman acp-serve --permission-mode manual`, \
                    newline-delimited JSON-RPC 2.0). initialize; session/new from a stock client \
                    (the agent mints the session id); session/prompt whose model turn calls \
                    write_file, which raises session/request_permission; the client allows once; \
                    tool_call and tool_call_update notifications and message chunks stream; the \
                    turn ends with stopReason end_turn.",
            },
            &transcript,
            acp_normalizer(&sandbox),
        );
    })
    .await
    .expect("scenario finished within its time limit");
}

#[tokio::test]
async fn acp_stdio_cancel() {
    tokio::time::timeout(Duration::from_secs(120), async {
        // The model holds its answer for a minute; the cancel must not wait.
        let model = MockModel::start(vec![Round::Stall(Duration::from_secs(60))]).await;
        let sandbox = Sandbox::new(Some(model_config(&model.url())));
        let mut agent = StdioRpc::spawn("craftsman acp-serve", sandbox.craftsman(&["acp-serve"]));
        agent
            .transcript
            .expect_golden("acp_stdio_cancel", acp_normalizer(&sandbox));

        agent.send(acp_initialize(1)).await;
        assert_acp_initialize_response(&agent.recv_response(1).await);

        // The Craftsman extension: the client picks the conversation id.
        agent
            .send(json!({
                "jsonrpc": "2.0", "id": 2, "method": "session/new",
                "params": {
                    "cwd": sandbox.workspace.path(), "mcpServers": [],
                    "_meta": {"craftsman": {"threadId": "golden-thread"}},
                },
            }))
            .await;
        let created = agent.recv_response(2).await;
        assert_eq!(created["result"]["sessionId"], "golden-thread");

        agent
            .send(json!({
                "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
                "params": {
                    "sessionId": "golden-thread",
                    "prompt": [{"type": "text", "text": "work slowly"}],
                },
            }))
            .await;
        wait_for_model_requests(&model, 1).await;

        // A notification: nothing may come back for the cancel itself.
        agent
            .send(json!({
                "jsonrpc": "2.0", "method": "session/cancel",
                "params": {"sessionId": "golden-thread"},
            }))
            .await;
        let response = agent.recv_response(3).await;
        assert_eq!(response["result"]["stopReason"], "cancelled");

        // Version negotiation: a client that is newer than the agent is
        // answered with the version the agent implements. Every other golden
        // client asks for 1 and gets 1 whatever the agent does, so only a
        // request for a version the agent does not implement pins the clamp
        // (`ACP_PROTOCOL_VERSION`, `negotiate_protocol_version`). It comes last
        // so that it adds frames to this golden without moving any other.
        agent.send(acp_initialize_asking_for(4, 99)).await;
        let clamped = agent.recv_response(4).await;
        assert_acp_initialize_response(&clamped);
        assert_eq!(clamped["result"]["protocolVersion"], 1);

        let transcript = agent.finish().await;
        transcript.require("the cancel notification", |step| {
            step["send"]["method"] == "session/cancel"
        });
        transcript.require("the cancelled response", |step| {
            step["recv"]["result"]["stopReason"] == "cancelled"
        });
        transcript.require("the initialize that asks for version 99", |step| {
            step["send"]["params"]["protocolVersion"] == 99
        });
        check(
            Case {
                name: "acp_stdio_cancel",
                description: "ACP over stdio. session/new adopts the client's \
                    _meta.craftsman.threadId as the session id; a session/cancel notification sent \
                    while the model call is in flight ends the pending session/prompt with \
                    stopReason cancelled. An initialize that asks for protocolVersion 99 is \
                    answered with protocolVersion 1, the version the agent implements.",
            },
            &transcript,
            acp_normalizer(&sandbox),
        );
    })
    .await
    .expect("scenario finished within its time limit");
}

/// Wait until the scripted model has received `count` requests, so a cancel
/// provably lands in the middle of a model call.
async fn wait_for_model_requests(model: &MockModel, count: usize) {
    let deadline = tokio::time::Instant::now() + STEP_TIMEOUT;
    while model.requests().await.len() < count {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the agent never called the model"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ── acp-bridge: HTTP surface and WebSocket relay ────────────────────────────

/// The keys of a JSON object, sorted.
fn sorted_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap_or_else(|| panic!("not a JSON object: {value}"))
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

/// Start `craftsman acp-bridge` on a free loopback port.
///
/// The bridge logs the port it was asked for, not the one it bound, so `--port
/// 0` cannot be used here. Instead a port is reserved and released just before
/// the launch; if another process takes it in between, the bridge fails to bind
/// and a new port is tried.
async fn start_bridge(sandbox: &Sandbox) -> (Proc, SocketAddr) {
    const ATTEMPTS: usize = 5;
    for attempt in 1..=ATTEMPTS {
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("reserve a loopback port")
            .local_addr()
            .expect("reserved address")
            .port();
        let mut command = sandbox.craftsman(&[
            "acp-bridge",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--",
            "craftsman",
            "acp-serve",
            "--permission-mode",
            "bypass-permissions",
        ]);
        command
            .env(PROXY_SECRET_ENV, PROXY_SECRET)
            // The inbox poller would only warn about a missing platform token.
            .env("CRAFTSMAN_INBOX_POLL_SECS", "3600");
        let mut bridge = Proc::spawn_server("craftsman acp-bridge", command);
        match bridge
            .stderr
            .wait_for("acp-bridge listening on", STEP_TIMEOUT)
            .await
        {
            Ok(_) => return (bridge, SocketAddr::from(([127, 0, 0, 1], port))),
            Err(reason) => {
                // A port that someone else took in the meantime makes the
                // bridge fail to bind and exit; any other failure is final.
                let exited = bridge.wait_exit(Duration::from_secs(2)).await.is_some();
                assert!(
                    exited && attempt < ATTEMPTS,
                    "acp-bridge did not start: {reason}\nstderr:\n{}",
                    bridge.stderr.tail(40)
                );
                eprintln!("the bridge could not use port {port} ({reason}); trying another");
            }
        }
    }
    unreachable!("the last attempt either returns or panics")
}

#[tokio::test]
async fn acp_bridge_http_and_relay() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let sandbox = Sandbox::new(None);
        let (mut bridge, address) = start_bridge(&sandbox).await;
        let normalizer = Normalizer::new()
            .directory(sandbox.workspace.path(), "<WORKSPACE>")
            .directory(sandbox.home.path(), "<HOME>")
            .mask("/http/body/uptime_s", Kind::Float, "<float>")
            .mask("/http/body/disk/used_bytes", Kind::UInt, "<uint>")
            .mask("/http/body/disk/total_bytes", Kind::UInt, "<uint>")
            .mask("/http/body/home/used_bytes", Kind::UInt, "<uint>")
            .mask_exact(
                "/recv/result/agentInfo/version",
                env!("CARGO_PKG_VERSION"),
                "<crate-version>",
            );
        let transcript = Transcript::default();
        transcript.expect_golden("acp_bridge_http_and_relay", normalizer.clone());
        let http = Http::new(address, transcript.clone(), bridge.stderr.clone());

        // The readiness probe is open; the stats poll needs the proxy secret.
        let (status, health) = http.get("/healthz", &[]).await;
        assert_eq!(status, 200);
        assert_eq!(sorted_keys(&health), ["harness", "status", "uptime_s"]);
        assert_eq!(health["status"], "ok");
        assert_eq!(health["harness"], "craftsman");
        assert!(health["uptime_s"].is_f64(), "{health}");

        let (status, denied) = http.get("/stats", &[]).await;
        assert_eq!(status, 403);
        assert_eq!(denied, json!({"detail": "forbidden"}));

        // The first authorized /stats computes a potentially cold native home
        // walk. Treat that as bounded setup, not the 30s HTTP contract step.
        // The unauthenticated 403 above still exercises the uncached path; every
        // original recorded auth/schema/relay assertion and 120s cap stays intact.
        let prepared = http
            .prepare_stats_cache(&[(PROXY_HEADER, PROXY_SECRET)])
            .await;
        assert_eq!(
            sorted_keys(&prepared),
            ["disk", "harness", "home", "uptime_s"]
        );
        assert_eq!(
            sorted_keys(&prepared["disk"]),
            ["total_bytes", "used_bytes"]
        );
        assert_eq!(sorted_keys(&prepared["home"]), ["used_bytes"]);
        assert!(prepared["uptime_s"].is_f64());
        for number in [
            &prepared["disk"]["total_bytes"],
            &prepared["disk"]["used_bytes"],
            &prepared["home"]["used_bytes"],
        ] {
            assert!(number.is_u64(), "{prepared}");
        }

        let (status, stats) = http.get("/stats", &[(PROXY_HEADER, PROXY_SECRET)]).await;
        assert_eq!(status, 200);
        assert_eq!(sorted_keys(&stats), ["disk", "harness", "home", "uptime_s"]);
        assert_eq!(sorted_keys(&stats["disk"]), ["total_bytes", "used_bytes"]);
        // The workspace controller reads only `home.used_bytes`.
        assert_eq!(sorted_keys(&stats["home"]), ["used_bytes"]);
        for number in [
            &stats["disk"]["total_bytes"],
            &stats["disk"]["used_bytes"],
            &stats["home"]["used_bytes"],
        ] {
            assert!(number.is_u64(), "{stats}");
        }
        assert!(stats["uptime_s"].is_f64(), "{stats}");

        // The front door: a dial without the proxy secret is refused; with it,
        // one WebSocket text frame is one stdio line.
        let refused =
            WsRpc::connect_refused(address, "/chat", &[], &transcript, &bridge.stderr).await;
        assert_eq!(refused, 401);

        let mut socket = WsRpc::connect(
            address,
            "/chat",
            &[(PROXY_HEADER, PROXY_SECRET)],
            transcript.clone(),
            bridge.stderr.clone(),
        )
        .await;
        socket.send(acp_initialize(1)).await;
        let response = socket
            .recv_until("the initialize response", |frame| frame["id"] == 1)
            .await;
        assert_acp_initialize_response(&response);
        // The response is the only frame the harness has to send.
        socket.drain_quiet(QUIET_PERIOD).await;
        socket.close().await;
        bridge.kill().await;

        transcript.require("the /stats answer", |step| {
            step["http"]["request"] == "GET /stats" && step["http"]["status"] == 200
        });
        transcript.require("the relayed initialize response", |step| {
            step["recv"]["result"]["agentInfo"]["name"] == "craftsman-harness"
        });

        check(
            Case {
                name: "acp_bridge_http_and_relay",
                description: "`craftsman acp-bridge --port <p> -- craftsman acp-serve` with \
                    CRAFTSMAN_WORKSPACE_PROXY_SECRET set, as in a workspace pod. GET /healthz \
                    (unauthenticated), GET /stats without and with the proxy secret, the \
                    WebSocket upgrade refused without the secret and accepted with it, and an \
                    ACP initialize relayed verbatim over the WebSocket to the harness.",
            },
            &transcript,
            normalizer,
        );
    })
    .await
    .expect("scenario finished within its time limit");
}

// ── The voice model entry check ─────────────────────────────────────────────
