//! Golden frames for the frozen protocols.
//!
//! Protocol v4 (`craftsman desktop-serve`) and the Craftsman ACP profile (`craftsman
//! acp-serve`, `craftsman acp-bridge`) have consumers outside this repository: the
//! desktop application and the workspace controller. These tests drive the
//! real `craftsman` binary against a scripted OpenAI-compatible model, record every
//! frame in both directions, and compare the recording with
//! `tests/golden/*.json`, so that a later phase cannot change a frame without a
//! reviewer seeing the change in the diff of a golden.
//!
//! The tests are hermetic: the program runs with a scrubbed environment and an
//! isolated HOME (`common/process.rs`), talks to loopback only and binds
//! ephemeral ports, so they run in parallel with the rest of the suite. (`craftsman
//! acp-bridge` logs the port it was asked for, not the one it bound, so its
//! port is reserved just before the launch and the launch is retried if
//! someone else takes it first.)
//!
//! # Scenarios
//!
//! * `acp_stdio_permission_turn`: `initialize` (with `_meta.craftsman.extensions`),
//!   `session/new` from a stock client, a prompt whose model turn calls
//!   `write_file`, the `session/request_permission` request, the client's
//!   approval, the `tool_call` / `tool_call_update` stream and `end_turn`.
//! * `acp_stdio_cancel`: `session/cancel` during a model call (`cancelled`),
//!   with a client-chosen thread id (`_meta.craftsman.threadId`), and an
//!   `initialize` that asks for protocol version 99 and is answered with 1.
//! * `desktop_v4_permission_turn`: the protocol v4 handshake (token-guarded
//!   WebSocket upgrade, `hello`, `state_snapshot`), a user message, the
//!   approval request and answer, the streamed frames and `done`, plus
//!   `/healthz` before and after the turn.
//! * `acp_bridge_http_and_relay`: `/healthz`, `/stats` (with and without the
//!   proxy secret), the WebSocket upgrade (refused without the secret) and an
//!   ACP `initialize` relayed over it.
//!
//! # What is replaced before comparing
//!
//! Only values that differ from run to run or from machine to machine are
//! replaced, in `common/golden.rs`. A rule that matches nothing fails the test,
//! so a rule cannot go stale unnoticed.
//!
//! * the workspace and HOME temp directories, as given and as canonicalized:
//!   `<WORKSPACE>`, `<HOME>`;
//! * the port after `127.0.0.1:`, `localhost:`, `[::1]:` or `0.0.0.0:`: `<PORT>`;
//! * minted conversation ids (`<unix seconds>-<pid>-<counter>`):
//!   `<thread-id-N>`, numbered by first appearance so that the identity of an
//!   id across frames is kept;
//! * `agentInfo.version` of `initialize`, when it is the crate version:
//!   `<crate-version>`;
//! * the bridge's `uptime_s` (a float) and its byte counts `disk.used_bytes`,
//!   `disk.total_bytes` and `home.used_bytes` (unsigned integers): `<float>`,
//!   `<uint>`; a value of another type stays visible;
//! * the values of the v4 snapshot's `voice` block, which depend on the build
//!   features. The block's own keys are kept and each value becomes its JSON
//!   type (`<bool>`, `<string>`, `<uint>`, ...), but an array becomes
//!   `<array>`: the entries of `voice.models` are not in the golden (see "Not
//!   covered").
//!
//! Everything else is compared exactly: the ids the protocols mint themselves
//! (`turn-1`, `permission-1`, `perm-1`, `call_1`, `msg-1`), the capability
//! lists, token counts, content types and the order of the frames. The
//! recording must also contain neither the model's API key nor any path of the
//! machine that made it (checkout, binary directory, temp directory, home), so
//! a golden recorded on a developer's machine cannot carry their paths.
//!
//! # Not covered
//!
//! * Protocol v4: 35 of its 37 request kinds (see `chat_session` in
//!   `desktop_serve.rs`), among them `ask_user_response`, `ui_action`,
//!   `steer`, `cancel`, the model, thread, workspace, checkpoint, task,
//!   diagnostics and voice operations, `configure`; the matching server
//!   frames (`ask_user`, `activity`, A2UI, `turn_error`, `operation_error`);
//!   rejecting or always-approving a permission; the proxy-secret
//!   authorization (only the desktop token is exercised); the single-client
//!   rule; the `initializing: true` first snapshot and the snapshot sent when
//!   startup ends (startup timing, which the dial waits out).
//! * ACP: `session/load`, the `_craftsman/*` methods (including the
//!   `_craftsman/session/request_input` request), permission outcomes other than
//!   `allow-once`, error responses, `max_turn_requests`, a second prompt
//!   superseding the first, A2UI resource blocks, and cancelling while a
//!   permission request is pending (the rejected tool's final
//!   `tool_call_update` and the `cancelled` response come from different tasks
//!   and nothing orders them).
//! * `acp-bridge`: the identity headers, a relayed turn, the close codes, a
//!   harness that fails to start, and a bridge without a proxy secret.
//! * The entries of the v4 snapshot's `voice.models` array (`name`, `bytes`,
//!   `downloaded`, which the desktop renders in its voice model picker). The
//!   golden cannot show them: CI builds with `--no-default-features`, where
//!   the list is empty (`voice_stub.rs`), while the shipped default build lists
//!   three models, and one golden must hold on both. So `shape` leaves the
//!   array as `<array>` on purpose, and renaming a key of an entry in
//!   `desktop_serve.rs` changes no golden. On a build with voice the scenario
//!   also checks each entry's keys and types (`assert_voice_model_entry`; the
//!   rule itself is unit tested on a literal), but no CI job runs that build
//!   (it needs CMake and, on Linux, ALSA headers), so CI cannot catch a renamed
//!   key. Closing the gap in CI needs the entry built by a function that a
//!   unit test in `desktop_serve.rs` can call, which is a product change;
//!   until then, run this test on the default build before touching that code.
//! * `craftsman serve` (protocol v2): the plan names only protocol v4 and ACP.
//!
//! # Two quirks frozen as they are
//!
//! `acp_stdio_permission_turn` records two behaviours that look like product
//! bugs. The `session/request_permission` request names the tool call
//! `toolCallId: "perm-1"`, which is the request's own id, while the `tool_call`
//! and `tool_call_update` notifications call the same tool `call_1`, so a stock
//! ACP client cannot link the prompt to the tool call. And every
//! `agent_message_chunk`, before and after the tool call, has `messageId:
//! "msg-1"`. The goldens record what the program does, not what it should do,
//! so both are frozen as they are. Fixing either is a product change that needs
//! the desktop and workspace owners' sign-off and a re-recording of the golden
//! (below), not a change to this test.
//!
//! # Changing a golden on purpose
//!
//! A golden is recorded by the same code that replays it, so a regenerated
//! golden always passes. What stops a protocol change from getting through is
//! a person reading the diff of the golden.
//!
//! 1. Check that the change is allowed. Protocol v4 takes no new frames; ACP
//!    additions ride `_meta.craftsman.*` or `_craftsman/*` and are listed in
//!    `_meta.craftsman.extensions` (AGENTS.md).
//! 2. Regenerate, on the same feature set CI uses; only files whose contents
//!    change are rewritten:
//!
//!    ```text
//!    UPDATE_GOLDEN=1 cargo test --locked --no-default-features -p craftsman-rs --test golden_frames
//!    ```
//!
//!    Only the value `1` rewrites, and only where `CI` is unset; any other
//!    value of `UPDATE_GOLDEN`, or `UPDATE_GOLDEN` together with `CI`, fails
//!    the test with a message instead of being ignored. The CI workflow also
//!    ends with `git diff --exit-code -- crates/craftsman-rs/tests/golden`, so a
//!    run that changed a golden cannot pass there.
//!
//! 3. Read `git diff crates/craftsman-rs/tests/golden` line by line. Every changed
//!    line is something a program outside this repository parses: a frame that
//!    moved or vanished, a renamed key or a changed value breaks it.
//! 4. Say in the pull request which golden changed and why, and get the
//!    desktop and workspace owners to sign off on the diff.
//!
//! Do not regenerate to turn a red build green; first find out why a frame
//! changed. A mismatch fails with a frame-by-frame diff that names the frames
//! that changed, were added or were removed.

mod common;

use common::golden::{Case, Kind, Normalizer, Transcript, check};
use common::mock_llm::{MockModel, Round};
use common::process::{MODEL_API_KEY, Proc, STEP_TIMEOUT, Sandbox, StderrLog, model_config};
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
const DESKTOP_TOKEN_ENV: &str = "CRAFTSMAN_DESKTOP_TOKEN";
const DESKTOP_TOKEN: &str = "golden-desktop-token";

/// How long a WebSocket must stay silent to count as quiet: after a
/// scenario's last frame (nothing may follow it), and after a v4 server has
/// started (see [`wait_for_desktop_startup`]).
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

// ── Protocol v4 over WebSocket ──────────────────────────────────────────────

/// The frame a v4 server answers with when another client is attached.
fn is_single_client_refusal(frame: &Value) -> bool {
    frame["type"] == "operation_error" && frame["code"] == "client_already_connected"
}

/// Why a first frame that is neither `hello` nor the single-client refusal
/// ends the test: it is a change of the handshake, not a reason to dial again.
fn not_a_handshake(first: &Value) -> String {
    format!(
        "the first frame of a protocol v4 connection is neither hello nor the \
         client_already_connected refusal: {first}"
    )
}

/// Read the `state_snapshot` that follows `hello`; returns it and whether the
/// server has finished starting up (`initializing: false`).
async fn read_handshake_snapshot(socket: &mut WsRpc) -> (Value, bool) {
    let snapshot = socket.recv("state_snapshot").await;
    if snapshot["type"] != "state_snapshot" {
        socket.fail_with(&format!(
            "the frame after hello is not a state_snapshot: {snapshot}"
        ));
    }
    match snapshot["state"]["initializing"].as_bool() {
        Some(initializing) => (snapshot, !initializing),
        None => socket.fail_with(&format!(
            "state.initializing of the snapshot is not a boolean: {snapshot}"
        )),
    }
}

/// Let `desktop-serve` finish starting up, so that the recorded dial sees
/// neither the startup nor its aftermath.
///
/// The server prints its address and then finishes starting in a task of its
/// own: a client that attaches before that is told `initializing: true`, and
/// when the task is done it sends whoever is attached one more
/// `state_snapshot`. Either would put startup timing into the golden. So an
/// unrecorded probe dials until the server reports `initializing: false`, and
/// stays attached until the socket has been quiet for [`QUIET_PERIOD`], which
/// absorbs that last snapshot even if the task was slow to send it.
///
/// Only the two things that really do pass are waited out: a server that is
/// still starting, and a slot that the previous probe has not yet given up.
/// Any other first frame is a change of the handshake and fails the test at
/// once, with the frame.
async fn wait_for_desktop_startup(address: SocketAddr, path: &str, stderr: &StderrLog) {
    let unrecorded = Transcript::default();
    let deadline = tokio::time::Instant::now() + STEP_TIMEOUT;
    loop {
        let mut probe =
            WsRpc::connect(address, path, &[], unrecorded.clone(), stderr.clone()).await;
        let first = probe.recv("hello").await;
        let started = if first["type"] == "hello" {
            read_handshake_snapshot(&mut probe).await.1
        } else if is_single_client_refusal(&first) {
            false
        } else {
            probe.fail_with(&not_a_handshake(&first))
        };
        if started {
            probe.drain_quiet(QUIET_PERIOD).await;
        }
        probe.close().await;
        if started {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "desktop-serve never finished starting up\nstderr:\n{}",
            stderr.tail(40)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Dial the v4 socket after startup and read its handshake.
///
/// The server serves one client at a time and notices a closed connection a
/// moment later. A dial that arrives in between is turned away with an
/// `operation_error` (`client_already_connected`); such an attempt is rewound
/// out of `transcript` and the dial is repeated. Any other first frame fails
/// the test at once. The socket that is returned records into `transcript`
/// from then on.
async fn dial_desktop(
    address: SocketAddr,
    path: &str,
    transcript: &Transcript,
    stderr: &StderrLog,
) -> (WsRpc, Value, Value) {
    wait_for_desktop_startup(address, path, stderr).await;
    let deadline = tokio::time::Instant::now() + STEP_TIMEOUT;
    loop {
        let mark = transcript.mark();
        let mut socket =
            WsRpc::connect(address, path, &[], transcript.clone(), stderr.clone()).await;
        let first = socket.recv("hello").await;
        if first["type"] == "hello" {
            let (snapshot, started) = read_handshake_snapshot(&mut socket).await;
            if !started {
                socket.fail_with(&format!(
                    "the server reported that it had started, then that it had not: {snapshot}"
                ));
            }
            return (socket, first, snapshot);
        }
        if !is_single_client_refusal(&first) {
            socket.fail_with(&not_a_handshake(&first));
        }
        socket.close().await;
        transcript.rewind_to(mark);
        assert!(
            tokio::time::Instant::now() < deadline,
            "desktop-serve kept turning the dial away: {first}\nstderr:\n{}",
            stderr.tail(40)
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// A frame the golden holds none of: the server is refusing something the
/// test just did, so waiting for the frame the test hoped for is pointless.
fn operation_error_is_fatal(frame: &Value) -> Option<&'static str> {
    (frame["type"] == "operation_error")
        .then_some("the server answered with an operation_error, which no golden holds")
}

/// The schema of one entry of `voice.models` in the v4 `state_snapshot`:
/// exactly `name`, `bytes` and `downloaded`, which the desktop reads.
fn assert_voice_model_entry(entry: &Value) {
    assert_eq!(
        sorted_keys(entry),
        ["bytes", "downloaded", "name"],
        "voice.models entry: {entry}"
    );
    assert!(entry["name"].is_string(), "voice.models entry: {entry}");
    assert!(entry["bytes"].is_u64(), "voice.models entry: {entry}");
    assert!(
        entry["downloaded"].is_boolean(),
        "voice.models entry: {entry}"
    );
}

#[tokio::test]
async fn desktop_v4_permission_turn() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let model = MockModel::start(write_file_turn()).await;
        let sandbox = Sandbox::new(Some(model_config(&model.url())));
        // The desktop launcher sets CRAFTSMAN_DESKTOP_TOKEN and dials /chat?token=.
        let mut command = sandbox.craftsman(&["desktop-serve", "--host", "127.0.0.1", "--port", "0"]);
        command.env(DESKTOP_TOKEN_ENV, DESKTOP_TOKEN);
        let mut server = Proc::spawn_server("craftsman desktop-serve", command);
        // Port 0 asks the OS for a free port; the server reports the real one.
        let listening = server
            .stderr
            .wait_for("desktop-serve listening on", STEP_TIMEOUT)
            .await
            .unwrap_or_else(|reason| panic!("desktop-serve did not start: {reason}"));
        let address: SocketAddr = listening
            .rsplit(' ')
            .next()
            .and_then(|address| address.parse().ok())
            .unwrap_or_else(|| panic!("no address in {listening:?}"));

        let normalizer = Normalizer::new()
            .directory(sandbox.workspace.path(), "<WORKSPACE>")
            .directory(sandbox.home.path(), "<HOME>")
            .forbid(MODEL_API_KEY)
            // Whether voice input is available, and its defaults, depend on
            // the build features, so the values are reduced to their types
            // and the block's own keys are the contract. Arrays are reduced
            // to `<array>`: see "Not covered" for `voice.models`.
            .shape("/recv/state/voice");
        let transcript = Transcript::default();
        transcript.expect_golden("desktop_v4_permission_turn", normalizer.clone());
        let http = Http::new(address, transcript.clone(), server.stderr.clone());
        let (status, health) = http.get("/healthz", &[]).await;
        assert_eq!(status, 200);
        assert_eq!(health["protocol"], 4);
        assert_eq!(health["agent_ready"], false, "no turn has run yet");

        // The socket is guarded by the token: a dial without it is refused.
        let refused =
            WsRpc::connect_refused(address, "/chat", &[], &transcript, &server.stderr).await;
        assert_eq!(refused, 403);

        let (mut socket, hello, snapshot) = dial_desktop(
            address,
            &format!("/chat?token={DESKTOP_TOKEN}"),
            &transcript,
            &server.stderr,
        )
        .await;
        assert_eq!(hello["protocol"], 4);
        assert_eq!(snapshot["type"], "state_snapshot");
        // Empty without voice (CI); on a voice build, the entries are checked
        // here because the golden reduces the array to `<array>`.
        for entry in snapshot["state"]["voice"]["models"]
            .as_array()
            .unwrap_or_else(|| panic!("voice.models is not an array: {snapshot}"))
        {
            assert_voice_model_entry(entry);
        }

        socket
            .send(json!({"type": "message", "content": "create golden.txt"}))
            .await;
        let request = socket
            .recv_until_or_fail(
                "permission_request",
                |frame| frame["type"] == "permission_request",
                operation_error_is_fatal,
            )
            .await;
        assert!(
            !sandbox.path_in_workspace("golden.txt").exists(),
            "the write must wait for the client's answer"
        );

        let request_id = socket.str_field(&request, "/request_id");
        socket
            .send(json!({
                "type": "permission_response",
                "request_id": request_id,
                "decision": "approve_once",
            }))
            .await;
        let done = socket
            .recv_until_or_fail(
                "done",
                |frame| frame["type"] == "done",
                operation_error_is_fatal,
            )
            .await;
        assert_eq!(done["thread_id"], hello["thread_id"]);

        assert_eq!(
            std::fs::read_to_string(sandbox.path_in_workspace("golden.txt")).unwrap(),
            "hello golden\n"
        );
        assert_write_file_turn_reached_the_model(&model).await;

        // `done` is the last frame of the turn; nothing may follow it.
        socket.drain_quiet(QUIET_PERIOD).await;
        let (_, health) = http.get("/healthz", &[]).await;
        assert_eq!(health["agent_ready"], true, "the turn built the session");
        socket.close().await;
        server.kill().await;

        for kind in [
            "hello",
            "state_snapshot",
            "turn_started",
            "token",
            "tool_started",
            "permission_request",
            "tool_finished",
            "done",
        ] {
            transcript.require(kind, |step| step["recv"]["type"] == kind);
        }
        transcript.require("permission_response", |step| {
            step["send"]["type"] == "permission_response"
        });
        transcript.require("the /healthz after the turn", |step| {
            step["http"]["request"] == "GET /healthz" && step["http"]["body"]["agent_ready"] == true
        });

        check(
            Case {
                name: "desktop_v4_permission_turn",
                description: "Protocol v4 (`craftsman desktop-serve` with CRAFTSMAN_DESKTOP_TOKEN set, \
                    JSON text frames over WebSocket /chat). /healthz before and after the turn; \
                    the WebSocket upgrade refused without the token and accepted with \
                    ?token=; the hello and state_snapshot handshake; a user message; \
                    turn_started, token, tool_started and the permission_request for \
                    write_file; the client's approve_once; tool_finished, the remaining tokens \
                    and done.",
            },
            &transcript,
            normalizer,
        );
    })
    .await
    .expect("scenario finished within its time limit");
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

/// The check that runs against the real `voice.models` entries on a voice
/// build is exercised here on literals, so that it has teeth in every build.
#[cfg(test)]
mod voice_model_entry {
    use super::*;

    #[test]
    fn accepts_the_documented_entry() {
        assert_voice_model_entry(
            &json!({"name": "large-v3-turbo-q5_0", "bytes": 574_041_195_u64, "downloaded": true}),
        );
    }

    #[test]
    #[should_panic(expected = "voice.models entry")]
    fn rejects_a_renamed_key() {
        assert_voice_model_entry(&json!({"name": "base-q5_1", "size": 1, "downloaded": false}));
    }

    #[test]
    #[should_panic(expected = "voice.models entry")]
    fn rejects_a_dropped_key() {
        assert_voice_model_entry(&json!({"name": "base-q5_1", "bytes": 1}));
    }

    #[test]
    #[should_panic(expected = "voice.models entry")]
    fn rejects_an_added_key() {
        assert_voice_model_entry(
            &json!({"name": "base-q5_1", "bytes": 1, "downloaded": false, "sha256": "00"}),
        );
    }

    #[test]
    #[should_panic(expected = "voice.models entry")]
    fn rejects_a_value_of_another_type() {
        assert_voice_model_entry(&json!({"name": "base-q5_1", "bytes": "1", "downloaded": false}));
    }
}
