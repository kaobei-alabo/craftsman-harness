//! Marked native fixture: one privacy conversation per model wire, built by
//! the shared session builder, with the source-built detector approved and
//! pinned at its user-level source, a real vault and guard, the checkpoint
//! store and a loopback server. Chat, Responses, Claude Messages and Gemini
//! each run a tool round, so the second request carries the first reply back,
//! with its signed or opaque provider items on the native wires.
//!
//! Unmarked startup returns before any build, path or configuration access.

use super::super::grants::GrantStore;
use super::super::manifest::Capabilities;
use super::{build_fixture, hash, root};
use crate::agent::{Session, SessionExtras, build_session_ext};
use crate::config::Config;
use crate::hitl::ApprovalState;
use crate::privacy_hook::{Detector, DetectorPin, PinnedDetector};
use crate::privacy_session::wire_fixtures::{config, reply};
use crate::privacy_session::{
    FreshConversationBinding, PrivacySession, PrivacyViewSelection, RawOrigin,
};
use crate::privacy_vault::{HostKey, HostKeySource, Limits, Vault, VaultLocation};
use craftsman_harness::{Checkpointer, Msg, TurnResult};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};
use zeroize::Zeroizing;

const ID: &str = "privacy-detector";
const HELPER: &str = "gadgets::privacy_detector_tests::view_wires::view_wires_native_fixture";
const MARKER: &str = "craftsman-privacy-view-wires=";
const EMAIL: &str = "alice@example.invalid";
const PHONE: &str = "+886 912 345 678";
const WIRES: [&str; 4] = ["chat", "responses", "claude", "gemini"];

fn selected(arguments: impl IntoIterator<Item = OsString>) -> bool {
    let values: Vec<_> = arguments
        .into_iter()
        .filter_map(|argument| {
            argument
                .to_str()
                .and_then(|argument| argument.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    matches!(values.as_slice(), [value] if value == "suite")
}

struct SyntheticKey;

impl HostKeySource for SyntheticKey {
    fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([11; 32])))
    }
}

/// The first reply's provider item, exactly as it must come back.
fn replayed_exactly(wire: &str, body: &Value, args: &Value) -> bool {
    match wire {
        "responses" => {
            let input = body["input"].as_array().unwrap();
            input.contains(&json!({"type":"reasoning","id":"wire-reasoning","encrypted_content":"wire-original-opaque","summary":[]}))
                && input.contains(&json!({"type":"function_call","id":"wire-function-0","call_id":"wire-call-0","name":"read_file","arguments":args.to_string(),"status":"completed"}))
        }
        "claude" => body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "assistant")
            .flat_map(|message| message["content"].as_array().unwrap())
            .any(|part| {
                part == &json!({"type":"thinking","thinking":"wire fixture reasoning","signature":"wire-original-signature"})
            }),
        "gemini" => body["contents"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "model")
            .flat_map(|message| message["parts"].as_array().unwrap())
            .any(|part| {
                part == &json!({"functionCall":{"id":"wire-call-0","name":"read_file","args":args},"thoughtSignature":"wire-original-signature"})
            }),
        _ => body["messages"][2]["tool_calls"][0]["id"] == "wire-call-0",
    }
}

/// The system text as the wire carries it.
fn system_text<'a>(wire: &str, body: &'a Value) -> &'a str {
    match wire {
        "responses" => body["instructions"].as_str(),
        "claude" => body["system"].as_str(),
        "gemini" => body["systemInstruction"]["parts"][0]["text"].as_str(),
        _ => body["messages"][0]["content"].as_str(),
    }
    .unwrap()
}

async fn build(
    cfg: &Config,
    workspace: &Path,
    owner: &Arc<PrivacySession>,
    activity: &Arc<Mutex<Vec<String>>>,
) -> Option<Session> {
    let activity = activity.clone();
    build_session_ext(
        cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        None,
        SessionExtras {
            privacy_view: Some(PrivacyViewSelection::fixture_limited_set(owner.clone())),
            // The owned terminal's durable checkpoints, which idle compaction
            // needs.
            cancellation_session: Some(craftsman_harness::SessionCancellation::default()),
            workspace_root: Some(workspace.to_path_buf()),
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(move |line: &str| {
                activity.lock().unwrap().push(line.to_owned())
            })),
            ..Default::default()
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "four-wire private view fixture; exactly one argv marker required before build/config access"]
async fn view_wires_native_fixture() {
    if !selected(std::env::args_os()) {
        println!("PRIVACY-VIEW-WIRES:NOT-SELECTED");
        return;
    }
    // The detector: built from source, installed at the user-level source,
    // approved for exactly these bytes and pinned.
    let root = root();
    let (image, _) = build_fixture(&root);
    let manifest =
        std::fs::read(root.join("crates/craftsman-rs/tests/fixtures/privacy-detector/gadget.toml"))
            .unwrap();
    let workspace = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    let config_root = std::fs::canonicalize(PathBuf::from(
        std::env::var_os("CRAFTSMAN_CONFIG_DIR").expect("the isolated configuration root"),
    ))
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let bundle = config_root.join("gadgets").join(ID);
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("gadget.wasm"), &image).unwrap();
    std::fs::write(bundle.join("gadget.toml"), &manifest).unwrap();
    let component = hash(&image);
    GrantStore::new(&config_root)
        .approve(
            ID,
            &bundle.canonicalize().unwrap(),
            &component,
            &Capabilities::default(),
        )
        .unwrap();
    let detector = || {
        Detector::pinned(
            PinnedDetector::new(
                &workspace,
                &config_root,
                DetectorPin::supplied_by_host(ID, "detect", &component, &hash(&manifest)).unwrap(),
            )
            .unwrap(),
        )
    };
    let vault = Vault::open(
        VaultLocation::under_host_config_root(&config_root).unwrap(),
        &SyntheticKey,
        Limits::default(),
    )
    .await
    .unwrap();
    let notes = workspace.join("notes.txt");
    std::fs::write(&notes, format!("Their number is {PHONE}.\n")).unwrap();
    let args = json!({"file_path": notes});
    let typed = format!("Who handles {EMAIL}? The notes are in notes.txt.");
    let activity = Arc::new(Mutex::new(Vec::new()));

    let mut threads = Vec::new();
    for wire in WIRES {
        let server = MockServer::start().await;
        for (priority, template) in [
            reply(wire, 0, Some(("wire-call-0", "read_file", &args)), ""),
            reply(wire, 1, None, "fixture answer"),
        ]
        .into_iter()
        .enumerate()
        {
            Mock::given(method("POST"))
                .respond_with(template)
                .up_to_n_times(1)
                .with_priority(priority as u8 + 1)
                .mount(&server)
                .await;
        }
        Mock::given(method("POST"))
            .respond_with(reply(wire, 2, None, "second answer"))
            .mount(&server)
            .await;
        let cfg = config(wire, &server);
        let owner = Arc::new(
            PrivacySession::open(
                &vault,
                FreshConversationBinding::reserve(&workspace).unwrap(),
                detector(),
            )
            .await
            .unwrap(),
        );
        let session = build(&cfg, &workspace, &owner, &activity)
            .await
            .unwrap_or_else(|| panic!("{wire}: {:?}", activity.lock().unwrap()));

        // One reserved id for the agent, the checkpoint store and the owner.
        assert_eq!(session.thread_id, owner.thread_id(), "{wire}");
        assert!(!threads.contains(&session.thread_id), "{wire}");
        threads.push(session.thread_id.clone());
        let view = session.privacy_view.as_ref().unwrap();
        let TurnResult::Done(output) = view
            .turn(&session.agent, &typed)
            .await
            .unwrap_or_else(|error| panic!("{wire}: {error}"))
        else {
            panic!("{wire}: the turn finishes");
        };
        assert_eq!(output.final_text(), "fixture answer", "{wire}");

        // The record: what was typed, the call, what the tool returned and
        // the answer, as they are.
        let messages = session.agent.state.lock().unwrap().messages.clone();
        assert_eq!(messages.len(), 4, "{wire}");
        assert_eq!(messages[0], Msg::user(typed.clone()), "{wire}");
        assert!(
            matches!(&messages[1], Msg::Assistant { tool_calls, .. }
            if tool_calls.len() == 1 && tool_calls[0].name == "read_file" && tool_calls[0].args == args),
            "{wire}"
        );
        assert!(
            matches!(&messages[2], Msg::Tool { content, is_error: false, .. }
            if content.contains(PHONE)),
            "{wire}"
        );
        assert_eq!(messages[3], Msg::assistant("fixture answer"), "{wire}");
        // The real usage of both replies.
        assert_eq!(*session.usage.lock().unwrap(), (6, 4), "{wire}");
        // No provider history was written into the checkpointed state: the
        // items live in this conversation's receipts.
        assert!(
            !session
                .agent
                .state
                .lock()
                .unwrap()
                .custom
                .contains_key("_craftsman_native_model_history"),
            "{wire}"
        );

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2, "{wire}");
        let bodies: Vec<Value> = requests
            .iter()
            .map(|request| serde_json::from_slice(&request.body).unwrap())
            .collect();
        for (request, body) in requests.iter().zip(&bodies) {
            // No original in the final bytes of either request.
            let raw = String::from_utf8(request.body.clone()).unwrap();
            assert!(!raw.contains(EMAIL) && !raw.contains(PHONE), "{wire}");
            assert!(raw.contains("Who handles "), "{wire}");
            // The system text carries the prompt's sections and the sections
            // the todo and memory middleware add, each admitted as its own
            // part through the detector.
            let system = system_text(wire, body);
            assert!(system.contains("## `write_todos`"), "{wire}");
            assert!(system.contains("<agent_memory>"), "{wire}");
            assert!(raw.contains("read_file"), "{wire}");
        }
        // The second request carries the first reply back exactly, signed
        // and opaque items included, and the tool result as its view.
        let second = String::from_utf8(requests[1].body.clone()).unwrap();
        assert!(
            replayed_exactly(wire, &bodies[1], &args),
            "{wire}: {second}"
        );
        assert!(second.contains("Their number is "), "{wire}");

        // The committed replies know the exact request they answered.
        let sources = session.agent.recorded_sources().unwrap();
        for (reply_at, request) in [(1, &requests[0]), (3, &requests[1])] {
            let node = owner
                .committed_model(sources[reply_at].as_ref().unwrap())
                .unwrap();
            assert_eq!(node.message().unwrap(), &messages[reply_at], "{wire}");
            assert_eq!(
                node.received_message().unwrap(),
                &messages[reply_at],
                "{wire}"
            );
            assert!(node.edits().unwrap().is_empty(), "{wire}");
            // Every wire: the reply knows the exact bytes it answered.
            let sent = <[u8; 32]>::from(Sha256::digest(&request.body));
            assert_eq!(node.request_sha256().unwrap(), sent, "{wire}");
            assert_eq!(node.has_provider_items().unwrap(), wire != "chat", "{wire}");
        }
        let result = owner.actual_result(sources[2].as_ref().unwrap()).unwrap();
        assert_eq!(result.tool(), "read_file", "{wire}");
        assert_eq!(result.reached().len(), 1, "{wire}");
        assert_eq!(
            owner
                .admitted(sources[2].as_ref().unwrap())
                .unwrap()
                .origin(),
            RawOrigin::ToolResult,
            "{wire}"
        );

        // A second turn sends the whole conversation again: still exact
        // items, still no original.
        let TurnResult::Done(output) = view.turn(&session.agent, "thanks").await.unwrap() else {
            panic!("{wire}: the conversation continues");
        };
        assert_eq!(output.final_text(), "second answer", "{wire}");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 3, "{wire}");
        let third = String::from_utf8(requests[2].body.clone()).unwrap();
        assert!(!third.contains(EMAIL) && !third.contains(PHONE), "{wire}");
        let body: Value = serde_json::from_str(&third).unwrap();
        assert!(replayed_exactly(wire, &body, &args), "{wire}: {third}");
        assert_eq!(*session.usage.lock().unwrap(), (9, 6), "{wire}");

        // Manual compaction on this wire, through the builder's summarizer
        // lane. After one more turn the first four messages are summarized,
        // and they include the first reply with its provider item.
        view.turn(&session.agent, "and then?").await.unwrap();
        let before = session.agent.state.lock().unwrap().messages.clone();
        assert_eq!(before.len(), 8, "{wire}");
        let counts = view
            .compact(&session.agent, &session.compaction)
            .await
            .unwrap_or_else(|error| panic!("{wire}: {error}"));
        assert_eq!(counts, Some((8, 5)), "{wire}");
        let after = session.agent.state.lock().unwrap().messages.clone();
        assert_eq!(
            after[0],
            Msg::user("<conversation_summary>\nsecond answer\n</conversation_summary>"),
            "{wire}"
        );
        assert_eq!(after[1..], before[4..], "{wire}");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 5, "{wire}");
        let asked = String::from_utf8(requests[4].body.clone()).unwrap();
        // The summarizer saw views, and the first reply as its call without
        // the provider's item: its own client never received that reply.
        assert!(!asked.contains(EMAIL) && !asked.contains(PHONE), "{wire}");
        assert!(
            asked.contains("You are compacting an ongoing agent conversation"),
            "{wire}"
        );
        assert!(
            asked.contains("read_file") && asked.contains("Their number is "),
            "{wire}"
        );
        assert!(!asked.contains("wire-original-"), "{wire}: {asked}");
        // The conversation goes on from the summary. Replies that the
        // compaction moved go back as their text.
        let TurnResult::Done(output) = view.turn(&session.agent, "go on").await.unwrap() else {
            panic!("{wire}: the conversation continues after compaction");
        };
        assert_eq!(output.final_text(), "second answer", "{wire}");
        let requests = server.received_requests().await.unwrap();
        let next = String::from_utf8(requests[5].body.clone()).unwrap();
        assert!(next.contains("<conversation_summary>"), "{wire}");
        assert!(!next.contains(EMAIL) && !next.contains(PHONE), "{wire}");
        assert!(!next.contains("wire-original-"), "{wire}");
        assert!(
            session
                .agent
                .recorded_sources()
                .unwrap()
                .iter()
                .all(Option::is_some),
            "{wire}"
        );

        // The real record is in the checkpoint store under the reserved id.
        let listed = session
            .checkpointer
            .as_ref()
            .expect("the store opened")
            .list_threads()
            .await
            .unwrap();
        assert!(
            listed.iter().any(|thread| thread.id == owner.thread_id()),
            "{wire}"
        );
        assert!(owner.check().is_ok(), "{wire}");
        session.lsp.shutdown().await;
    }
    println!(
        "PRIVACY-VIEW-WIRES:COMPLETE wires={} conversations={}",
        WIRES.len(),
        threads.len()
    );
}

async fn run_helper(
    marker: Option<&str>,
    home: &Path,
) -> (std::process::ExitStatus, String, String) {
    let selected = marker == Some("suite");
    if selected {
        // The selected run owns a workspace and a configuration root of its
        // own under the isolated home.
        std::fs::create_dir(home.join("workspace")).unwrap();
        std::fs::create_dir(home.join("config")).unwrap();
    }
    super::pinned_hook::run_marked_helper(
        HELPER,
        marker.map(|marker| format!("{MARKER}{marker}")),
        home,
        "view wires fixture",
        |command| {
            if selected {
                command
                    .current_dir(home.join("workspace"))
                    .env("CRAFTSMAN_CONFIG_DIR", home.join("config"));
            } else {
                command.current_dir(home);
            }
        },
    )
    .await
}

#[tokio::test]
async fn an_unmarked_view_wires_fixture_returns_before_any_build_or_configuration() {
    let home = tempfile::tempdir().unwrap();
    for marker in [None, Some("collect")] {
        let (status, stdout, _) = run_helper(marker, home.path()).await;
        assert!(status.success());
        assert!(stdout.contains("PRIVACY-VIEW-WIRES:NOT-SELECTED"));
        assert!(!stdout.contains("COMPLETE"));
    }
    // Nothing was created in the isolated home.
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn four_wires_carry_a_privacy_conversation_through_the_actual_builder_and_detector() {
    let home = tempfile::tempdir().unwrap();
    let (status, stdout, stderr) = run_helper(Some("suite"), home.path()).await;
    assert!(
        status.success(),
        "view wires fixture failed: {stdout} {stderr}"
    );
    assert!(stdout.contains("PRIVACY-VIEW-WIRES:COMPLETE wires=4 conversations=4"));
    assert!(!stdout.contains("NOT-SELECTED"));
}
