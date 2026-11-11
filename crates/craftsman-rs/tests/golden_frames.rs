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
//! The owner retired desktop-v4 and authorized the stable-v1 audit repairs.
//! The ACP recording changes intentionally correct permission correlation and
//! distinct message identities. Native audit regressions below independently
//! check file effects, isolation, replay and ordering. UPDATE_GOLDEN is refused in CI.

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

// Native regressions for the stable-v1 audit; these assertions do not use goldens.
async fn audit_new(agent: &mut StdioRpc, id: u64, cwd: &std::path::Path) -> String {
    agent.send(json!({"jsonrpc":"2.0","id":id,"method":"session/new","params":{"cwd":cwd,"mcpServers":[]}})).await;
    let created = agent.recv_response(id).await;
    agent.str_field(&created, "/result/sessionId")
}

async fn audit_prompt(agent: &mut StdioRpc, id: u64, sid: &str, text: &str) {
    agent.send(json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":text}]}})).await;
}

async fn audit_answer(agent: &mut StdioRpc, request: &Value, option: &str) {
    agent.send(json!({"jsonrpc":"2.0","id":request["id"],"result":{"outcome":{"outcome":"selected","optionId":option}}})).await;
}

fn audit_updates(agent: &StdioRpc, mark: usize) -> Vec<Value> {
    agent.transcript.steps()[mark..]
        .iter()
        .filter_map(|s| {
            let recv = &s["recv"];
            (recv["method"] == "session/update").then(|| recv["params"].clone())
        })
        .collect()
}

#[tokio::test]
async fn acp_audit_directory_permissions_reused_ids_and_persisted_replay() {
    let model = MockModel::start(vec![
        Round::tool_call(
            &["first"],
            "reused",
            "write_file",
            json!({"file_path":"accepted.txt","content":"accepted"}),
            (11, 5),
        ),
        Round::text(&["allowed"], (20, 4)),
        Round::tool_call(
            &["second"],
            "reused",
            "write_file",
            json!({"file_path":"denied.txt","content":"denied"}),
            (12, 6),
        ),
        Round::text(&["denied"], (21, 3)),
    ])
    .await;
    let sandbox = Sandbox::new(Some(model_config(&model.url())));
    let selected = tempfile::tempdir().unwrap();
    let mut agent = StdioRpc::spawn(
        "ACP audit",
        sandbox.craftsman(&["acp-serve", "--permission-mode", "manual"]),
    );
    agent.send(acp_initialize(1)).await;
    assert_acp_initialize_response(&agent.recv_response(1).await);
    let sid = audit_new(&mut agent, 2, selected.path()).await;
    let mut ids = Vec::new();
    let mut messages = std::collections::HashSet::new();
    for (request_id, option) in [(3, "allow-once"), (4, "allow-unoffered-audit")] {
        let mark = agent.transcript.mark();
        audit_prompt(&mut agent, request_id, &sid, "write").await;
        let permission = agent
            .recv_until("permission", |f| {
                f["method"] == "session/request_permission"
            })
            .await;
        let started = audit_updates(&agent, mark)
            .into_iter()
            .find(|p| p["update"]["sessionUpdate"] == "tool_call")
            .unwrap();
        assert_eq!(permission["params"]["sessionId"], sid);
        assert_eq!(
            permission["params"]["toolCall"]["toolCallId"],
            started["update"]["toolCallId"]
        );
        audit_answer(&mut agent, &permission, option).await;
        assert_eq!(
            agent.recv_response(request_id).await["result"]["stopReason"],
            "end_turn"
        );
        let updates = audit_updates(&agent, mark);
        let finished = updates
            .iter()
            .find(|p| p["update"]["sessionUpdate"] == "tool_call_update")
            .unwrap();
        assert_eq!(
            finished["update"]["toolCallId"],
            started["update"]["toolCallId"]
        );
        assert_eq!(
            finished["update"]["status"],
            if request_id == 3 {
                "completed"
            } else {
                "failed"
            }
        );
        ids.push(started["update"]["toolCallId"].clone());
        let turn_ids: std::collections::HashSet<String> = updates
            .iter()
            .filter_map(|p| p["update"]["messageId"].as_str().map(str::to_owned))
            .collect();
        for id in turn_ids {
            assert!(
                messages.insert(id),
                "distinct assistant messages reused an ID"
            );
        }
    }
    assert_ne!(
        ids[0], ids[1],
        "model IDs can repeat across turns; ACP identities cannot"
    );
    assert_eq!(
        std::fs::read_to_string(selected.path().join("accepted.txt")).unwrap(),
        "accepted"
    );
    assert!(!selected.path().join("denied.txt").exists());
    assert!(!sandbox.workspace.path().join("accepted.txt").exists());
    agent.finish().await;

    // Reopen the actual SQLite checkpoint in a fresh process, with no sampling.
    let mut agent = StdioRpc::spawn("ACP load audit", sandbox.craftsman(&["acp-serve"]));
    agent.send(acp_initialize(10)).await;
    assert_acp_initialize_response(&agent.recv_response(10).await);
    let mark = agent.transcript.mark();
    agent.send(json!({"jsonrpc":"2.0","id":11,"method":"session/load","params":{"sessionId":sid,"cwd":selected.path(),"mcpServers":[]}})).await;
    assert!(agent.recv_response(11).await["result"].is_object());
    agent.send(json!({"jsonrpc":"2.0","id":14,"method":"session/new","params":{"cwd":selected.path(),"mcpServers":[],"_meta":{"craftsman":{"threadId":sid}}}})).await;
    assert!(
        agent.recv_response(14).await.get("error").is_some(),
        "session/new must not overwrite saved history"
    );
    agent.send(json!({"jsonrpc":"2.0","id":15,"method":"session/load","params":{"sessionId":sid,"cwd":sandbox.workspace.path(),"mcpServers":[]}})).await;
    assert!(
        agent.recv_response(15).await.get("error").is_some(),
        "cached session directory mismatch must be refused"
    );
    let replay = audit_updates(&agent, mark);
    let calls: Vec<_> = replay
        .iter()
        .filter(|p| p["update"]["sessionUpdate"] == "tool_call")
        .collect();
    let results: Vec<_> = replay
        .iter()
        .filter(|p| p["update"]["sessionUpdate"] == "tool_call_update")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(results.len(), 2);
    assert_ne!(
        calls[0]["update"]["toolCallId"],
        calls[1]["update"]["toolCallId"]
    );
    for (call, result) in calls.iter().zip(&results) {
        assert_eq!(call["update"]["toolCallId"], result["update"]["toolCallId"]);
    }
    assert_eq!(results[0]["update"]["status"], "completed");
    assert_eq!(results[1]["update"]["status"], "failed");
    assert!(
        results[0]["update"]["content"]
            .to_string()
            .contains("Updated file accepted.txt")
    );
    assert!(
        results[1]["update"]["content"]
            .to_string()
            .contains("Tool call denied by the user.")
    );
    agent.send(json!({"jsonrpc":"2.0","id":12,"method":"session/load","params":{"sessionId":"audit-unknown","cwd":selected.path(),"mcpServers":[]}})).await;
    assert!(agent.recv_response(12).await.get("error").is_some());
    agent.send(json!({"jsonrpc":"2.0","id":13,"method":"session/prompt","params":{"sessionId":"audit-unknown","prompt":[{"type":"text","text":"must not sample"}]}})).await;
    assert!(agent.recv_response(13).await.get("error").is_some());
    agent.finish().await;
    assert_eq!(
        model.requests().await.len(),
        4,
        "load/unknown IDs must not run the model"
    );
}

#[tokio::test]
async fn acp_audit_busy_isolation_and_cancellation_ordering() {
    let model = MockModel::start(vec![
        Round::tool_call(
            &[],
            "blocked",
            "write_file",
            json!({"file_path":"cancelled.txt","content":"no"}),
            (11, 5),
        ),
        Round::text(&["session B"], (13, 2)),
        Round::text(&["session A"], (17, 3)),
    ])
    .await;
    let sandbox = Sandbox::new(Some(model_config(&model.url())));
    let other = tempfile::tempdir().unwrap();
    let mut agent = StdioRpc::spawn(
        "ACP cancel audit",
        sandbox.craftsman(&["acp-serve", "--permission-mode", "manual"]),
    );
    agent.send(acp_initialize(1)).await;
    assert_acp_initialize_response(&agent.recv_response(1).await);
    let a = audit_new(&mut agent, 2, sandbox.workspace.path()).await;
    let mark = agent.transcript.mark();
    audit_prompt(&mut agent, 3, &a, "write").await;
    let permission = agent
        .recv_until("permission", |f| {
            f["method"] == "session/request_permission"
        })
        .await;
    agent
        .send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{}}))
        .await;
    agent.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"other-session"}})).await;
    for (id, method, params) in [
        (
            4,
            "session/new",
            json!({"cwd":other.path(),"mcpServers":[]}),
        ),
        (
            5,
            "session/load",
            json!({"sessionId":a,"cwd":sandbox.workspace.path(),"mcpServers":[]}),
        ),
        (
            6,
            "session/prompt",
            json!({"sessionId":a,"prompt":[{"type":"text","text":"second"}]}),
        ),
        (
            7,
            "_craftsman/session/set_model",
            json!({"modelId":"another"}),
        ),
    ] {
        agent
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        assert_eq!(agent.recv_response(id).await["error"]["code"], -32001);
    }
    agent
        .send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":a}}))
        .await;
    // A stale approval after cancellation cannot grant this or another turn.
    audit_answer(&mut agent, &permission, "allow-always").await;
    assert_eq!(
        agent.recv_response(3).await["result"]["stopReason"],
        "cancelled"
    );
    let updates = audit_updates(&agent, mark);
    assert!(updates.iter().all(|p| p["sessionId"] == a));
    assert!(
        updates
            .iter()
            .any(|p| p["update"]["sessionUpdate"] == "tool_call_update"
                && p["update"]["status"] == "failed")
    );
    let final_mark = agent.transcript.mark();
    let b = audit_new(&mut agent, 8, other.path()).await;
    assert!(
        audit_updates(&agent, final_mark).is_empty(),
        "old turn updates followed its final response"
    );
    audit_prompt(&mut agent, 9, &b, "B").await;
    assert_eq!(
        agent.recv_response(9).await["result"]["_meta"]["craftsman"]["usage"]["inputTokens"],
        13
    );
    audit_prompt(&mut agent, 10, &a, "A").await;
    assert_eq!(
        agent.recv_response(10).await["result"]["_meta"]["craftsman"]["usage"]["inputTokens"],
        17
    );
    let transcript = agent.finish().await;
    let steps = transcript.steps();
    let end = steps.iter().position(|s| s["recv"]["id"] == 3).unwrap();
    assert!(
        steps[end + 1..]
            .iter()
            .filter(|s| s["recv"]["method"] == "session/update")
            .all(|s| s["recv"]["params"]["update"]["sessionUpdate"] != "tool_call_update")
    );
    assert!(!sandbox.workspace.path().join("cancelled.txt").exists());
    let req = model.requests().await;
    assert_eq!(req.len(), 3);
    assert!(
        !req[1]["messages"].to_string().contains("cancelled.txt"),
        "session B inherited session A history"
    );
    assert!(
        req[2]["messages"].to_string().contains("cancelled.txt"),
        "session A lost its actual failed result"
    );
}

#[tokio::test]
async fn acp_audit_invalid_inputs_and_permission_shapes_fail_closed() {
    let model = MockModel::start(vec![]).await;
    let sandbox = Sandbox::new(Some(model_config(&model.url())));
    let mut agent = StdioRpc::spawn("ACP invalid audit", sandbox.craftsman(&["acp-serve"]));
    agent.send(acp_initialize_asking_for(1, 0)).await;
    assert_eq!(agent.recv_response(1).await["result"]["protocolVersion"], 1);
    for (id, frame, code) in [
        (
            2,
            json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}),
            -32602,
        ),
        (
            3,
            json!({"jsonrpc":"2.0","id":3,"method":"session/new","params":{"cwd":"relative","mcpServers":[]}}),
            -32602,
        ),
        (
            4,
            json!({"jsonrpc":"2.0","id":4,"method":"session/new","params":{"cwd":sandbox.workspace.path()}}),
            -32602,
        ),
        (5, json!({"jsonrpc":"2.0","id":5,"method":123}), -32600),
        (
            6,
            json!({"jsonrpc":"2.0","id":6,"method":"session/new","params":[]}),
            -32602,
        ),
        (
            7,
            json!({"jsonrpc":"2.0","id":7,"method":"session/new","params":{"cwd":sandbox.workspace.path(),"mcpServers":[],"_meta":{"craftsman":{"threadId":"../../outside"}}}}),
            -32602,
        ),
    ] {
        agent.send(frame).await;
        assert_eq!(agent.recv_response(id).await["error"]["code"], code);
    }
    agent.finish().await;
    assert!(model.requests().await.is_empty());

    // Each loose affirmative is checked against an actual manual write attempt.
    for result in [
        json!(true),
        json!("allow-once"),
        json!({"optionId":"allow-always"}),
        json!({"outcome":{"outcome":"selected","optionId":"approve-anything"}}),
    ] {
        let model = MockModel::start(vec![
            Round::tool_call(
                &[],
                "loose",
                "write_file",
                json!({"file_path":"loose.txt","content":"no"}),
                (11, 5),
            ),
            Round::text(&["refused"], (20, 4)),
        ])
        .await;
        let sandbox = Sandbox::new(Some(model_config(&model.url())));
        let mut agent = StdioRpc::spawn(
            "ACP permission audit",
            sandbox.craftsman(&["acp-serve", "--permission-mode", "manual"]),
        );
        agent.send(acp_initialize(1)).await;
        agent.recv_response(1).await;
        let sid = audit_new(&mut agent, 2, sandbox.workspace.path()).await;
        audit_prompt(&mut agent, 3, &sid, "write").await;
        let p = agent
            .recv_until("permission", |f| {
                f["method"] == "session/request_permission"
            })
            .await;
        agent
            .send(json!({"jsonrpc":"2.0","id":p["id"],"result":result}))
            .await;
        assert_eq!(
            agent.recv_response(3).await["result"]["stopReason"],
            "end_turn"
        );
        assert!(!sandbox.workspace.path().join("loose.txt").exists());
        agent.finish().await;
    }
}

#[tokio::test]
async fn acp_audit_delegated_permission_reports_the_actual_child_result() {
    let model = MockModel::start(vec![
        Round::tool_call(
            &[],
            "parent",
            "task",
            json!({"description":"write delegated.txt","subagent_type":"general-purpose"}),
            (11, 5),
        ),
        Round::tool_call(
            &[],
            "child",
            "write_file",
            json!({"file_path":"delegated.txt","content":"child"}),
            (7, 3),
        ),
        Round::text(&["child done"], (9, 2)),
        Round::text(&["parent done"], (20, 4)),
    ])
    .await;
    let sandbox = Sandbox::new(Some(model_config(&model.url())));
    let mut agent = StdioRpc::spawn(
        "ACP delegated audit",
        sandbox.craftsman(&["acp-serve", "--permission-mode", "manual"]),
    );
    agent.send(acp_initialize(1)).await;
    agent.recv_response(1).await;
    let sid = audit_new(&mut agent, 2, sandbox.workspace.path()).await;
    audit_prompt(&mut agent, 3, &sid, "delegate").await;
    let p = agent
        .recv_until("delegated permission", |f| {
            f["method"] == "session/request_permission"
        })
        .await;
    let id = p["params"]["toolCall"]["toolCallId"].clone();
    assert!(id.as_str().unwrap().starts_with("delegated-"));
    assert!(audit_updates(&agent,0).iter().any(|u|u["update"]["toolCallId"]==id && u["update"]["sessionUpdate"]=="tool_call"));
    audit_answer(&mut agent, &p, "allow-once").await;
    assert_eq!(
        agent.recv_response(3).await["result"]["stopReason"],
        "end_turn"
    );
    assert!(
        audit_updates(&agent, 0)
            .iter()
            .any(|u| u["update"]["toolCallId"] == id && u["update"]["status"] == "completed")
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.workspace.path().join("delegated.txt")).unwrap(),
        "child"
    );
    agent.finish().await;
    assert_eq!(model.requests().await.len(), 4);
}

const ACP_MCP_MARKER: &str = "__craftsman_acp_mcp_fixture__";
const ACP_MCP_RECORD: &str = "__craftsman_acp_mcp_record__:";

#[test]
#[ignore = "argv-marked native MCP child; never inspect stdin/config when unmarked"]
fn acp_audit_native_mcp_helper() {
    use std::io::{BufRead, Write};
    let args: Vec<_> = std::env::args().collect();
    if args.iter().filter(|a| a.as_str() == ACP_MCP_MARKER).count() != 1 {
        return;
    }
    let records: Vec<_> = args
        .iter()
        .filter_map(|a| a.strip_prefix(ACP_MCP_RECORD))
        .collect();
    if records.len() != 1 || !std::path::Path::new(records[0]).is_absolute() {
        return;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(records[0])
        .unwrap();
    writeln!(file,"{}",json!({"cwd":std::env::current_dir().unwrap(),"literal":std::env::var("CRAFTSMAN_ACP_MCP_LITERAL").unwrap()})).unwrap();
    file.sync_all().unwrap();
    let mut lines = std::io::stdin().lock().lines();
    for _ in 0..16 {
        let Some(line) = lines.next() else { return };
        let line = line.unwrap();
        assert!(line.len() <= 65536);
        let request: Value = serde_json::from_str(&line).unwrap();
        let result = match request["method"].as_str() {
            Some("server/discover") => {
                json!({"protocolVersion":"2026-07-28","capabilities":{"tools":{}},"serverInfo":{"name":"audit-mcp","version":"1"}})
            }
            Some("notifications/initialized") => continue,
            Some("tools/list") => {
                json!({"tools":[{"name":"audit_mcp_ping","description":"Synthetic audit ping","inputSchema":{"type":"object","properties":{}}}]})
            }
            Some("tools/call") => {
                json!({"content":[{"type":"text","text":"native MCP pong"}],"isError":false})
            }
            _ => panic!("unexpected fixture request"),
        };
        println!(
            "{}",
            json!({"jsonrpc":"2.0","id":request["id"],"result":result})
        );
        std::io::stdout().flush().unwrap();
    }
    panic!("fixture request bound exceeded");
}

#[tokio::test]
async fn acp_audit_standard_stdio_mcp_uses_session_cwd_and_literal_env() {
    let model = MockModel::start(vec![
        Round::tool_call(&[], "mcp-call", "audit_mcp_ping", json!({}), (11, 5)),
        Round::text(&["pong"], (20, 4)),
    ])
    .await;
    let sandbox = Sandbox::new(Some(model_config(&model.url())));
    let selected = tempfile::tempdir().unwrap();
    let record = selected.path().join("mcp-record.json");
    let mut agent = StdioRpc::spawn("ACP MCP audit", sandbox.craftsman(&["acp-serve"]));
    agent.send(acp_initialize(1)).await;
    agent.recv_response(1).await;
    let server = json!({"name":"native-audit","command":std::env::current_exe().unwrap(),"args":["--exact","acp_audit_native_mcp_helper","--ignored","--nocapture",ACP_MCP_MARKER,format!("{ACP_MCP_RECORD}{}",record.display())],"env":[{"name":"CRAFTSMAN_ACP_MCP_LITERAL","value":"literal-value"}]});
    agent.send(json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":selected.path(),"mcpServers":[server]}})).await;
    let created = agent.recv_response(2).await;
    let sid = agent.str_field(&created, "/result/sessionId");
    let facts: Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    assert_eq!(
        std::path::PathBuf::from(facts["cwd"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        selected.path().canonicalize().unwrap()
    );
    assert_eq!(facts["literal"], "literal-value");
    audit_prompt(&mut agent, 3, &sid, "ping").await;
    assert_eq!(
        agent.recv_response(3).await["result"]["stopReason"],
        "end_turn"
    );
    agent.finish().await;
    let requests = model.requests().await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["function"]["name"] == "audit_mcp_ping")
    );
    assert!(
        requests[1]["messages"]
            .to_string()
            .contains("native MCP pong")
    );

    let mut agent = StdioRpc::spawn("ACP bad MCP audit", sandbox.craftsman(&["acp-serve"]));
    agent.send(acp_initialize(1)).await;
    agent.recv_response(1).await;
    agent.send(json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":selected.path(),"mcpServers":[{"name":"bad","command":selected.path().join("nonexistent-command"),"args":[],"env":[]}]}})).await;
    assert!(
        agent.recv_response(2).await.get("error").is_some(),
        "requested MCP failures must not be ignored"
    );
    agent.finish().await;
}

#[tokio::test]
async fn acp_audit_eof_releases_pending_permission_and_drains_final_updates() {
    let model = MockModel::start(vec![Round::tool_call(
        &[],
        "eof-write",
        "write_file",
        json!({"file_path":"eof.txt","content":"no"}),
        (11, 5),
    )])
    .await;
    let sandbox = Sandbox::new(Some(model_config(&model.url())));
    let mut agent = StdioRpc::spawn(
        "ACP EOF audit",
        sandbox.craftsman(&["acp-serve", "--permission-mode", "manual"]),
    );
    agent.send(acp_initialize(1)).await;
    agent.recv_response(1).await;
    let sid = audit_new(&mut agent, 2, sandbox.workspace.path()).await;
    audit_prompt(&mut agent, 3, &sid, "write").await;
    agent
        .recv_until("parked permission", |f| {
            f["method"] == "session/request_permission"
        })
        .await;
    let transcript = agent.finish().await;
    let steps = transcript.steps();
    let update = steps
        .iter()
        .position(|s| s["recv"]["params"]["update"]["sessionUpdate"] == "tool_call_update")
        .unwrap();
    let done = steps
        .iter()
        .position(|s| s["recv"]["id"] == 3 && s["recv"].get("result").is_some())
        .unwrap();
    assert!(update < done);
    assert_eq!(steps[done]["recv"]["result"]["stopReason"], "cancelled");
    assert!(!sandbox.workspace.path().join("eof.txt").exists());
    assert_eq!(model.requests().await.len(), 1);
}

#[tokio::test]
async fn acp_audit_model_reconfiguration_preserves_history_approval_and_ids() {
    let model = MockModel::start(vec![
        Round::tool_call(
            &["old model"],
            "same",
            "write_file",
            json!({"file_path":"before.txt","content":"before"}),
            (11, 5),
        ),
        Round::text(&["saved"], (20, 4)),
        Round::tool_call(
            &["new model"],
            "same",
            "write_file",
            json!({"file_path":"after.txt","content":"after"}),
            (13, 6),
        ),
        Round::text(&["continued"], (22, 3)),
    ])
    .await;
    let sandbox = Sandbox::new(Some(model_config(&model.url())));
    let mut agent = StdioRpc::spawn(
        "ACP model audit",
        sandbox.craftsman(&["acp-serve", "--permission-mode", "manual"]),
    );
    agent.send(acp_initialize(1)).await;
    agent.recv_response(1).await;
    let sid = audit_new(&mut agent, 2, sandbox.workspace.path()).await;
    audit_prompt(&mut agent, 3, &sid, "before").await;
    let p = agent
        .recv_until("permission", |f| {
            f["method"] == "session/request_permission"
        })
        .await;
    audit_answer(&mut agent, &p, "allow-always").await;
    assert_eq!(
        agent.recv_response(3).await["result"]["stopReason"],
        "end_turn"
    );
    let first = audit_updates(&agent, 0);
    let mark = agent.transcript.mark();
    agent.send(json!({"jsonrpc":"2.0","id":4,"method":"_craftsman/session/set_model","params":{"sessionId":sid,"modelId":"mock-second"}})).await;
    assert_eq!(
        agent.recv_response(4).await["result"]["modelId"],
        "mock-second"
    );
    assert!(
        audit_updates(&agent, mark).is_empty(),
        "model transfer replayed effects/content"
    );
    assert_eq!(
        model.requests().await.len(),
        2,
        "model transfer must not sample"
    );
    audit_prompt(&mut agent, 5, &sid, "after").await;
    let result = agent.recv_response(5).await;
    assert_eq!(result["result"]["stopReason"], "end_turn");
    assert_eq!(
        result["result"]["_meta"]["craftsman"]["usage"]["inputTokens"],
        35
    );
    let second = audit_updates(&agent, mark);
    let first_ids: std::collections::HashSet<_> = first
        .iter()
        .filter_map(|p| p["update"]["messageId"].as_str())
        .collect();
    assert!(
        second
            .iter()
            .filter_map(|p| p["update"]["messageId"].as_str())
            .all(|id| !first_ids.contains(id))
    );
    let old = first
        .iter()
        .find(|p| p["update"]["sessionUpdate"] == "tool_call")
        .unwrap();
    let new = second
        .iter()
        .find(|p| p["update"]["sessionUpdate"] == "tool_call")
        .unwrap();
    assert_ne!(old["update"]["toolCallId"], new["update"]["toolCallId"]);
    assert_eq!(
        std::fs::read_to_string(sandbox.workspace.path().join("after.txt")).unwrap(),
        "after"
    );
    let transcript = agent.finish().await;
    assert_eq!(
        transcript
            .steps()
            .iter()
            .filter(|s| s["recv"]["method"] == "session/request_permission")
            .count(),
        1,
        "allow-always was lost during transfer"
    );
    let requests = model.requests().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[2]["model"], "mock-second");
    assert!(requests[2]["messages"].to_string().contains("before.txt"));
}
