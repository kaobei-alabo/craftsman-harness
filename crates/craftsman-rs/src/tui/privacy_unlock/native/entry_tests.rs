//! Actual marked private terminal, captured user policy/key, source-built WASM,
//! shared Chat builder, human question/opaque review, file and checkpoint IO.
//! No fixture policy or reduced component set is used by this entry.
use super::*;
use crate::gadgets::{GadgetRuntime, RuntimeOptions};
use craftsman_harness::{Checkpointer, Msg};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::method};

const TYPED: &str = "  Handle alice@example.invalid and +886 912 345 678.  ";
const FOLLOW: &str = "  Follow up for alice@example.invalid.  ";
const CONTACT_FILE: &str = "Contact delivered alice@example.invalid";

fn actual_alias(request: &Request) -> String {
    let value: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_absent(&request.body);
    eprintln!(
        "PRIVATE-ENTRY-ACTUAL-WIRE:{}",
        base64::engine::general_purpose::STANDARD.encode(&request.body)
    );
    let user = value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("@privacy.invalid"))
        })
        .unwrap()["content"]
        .as_str()
        .unwrap();
    // Borrow the exact issued alias from the actual request. This is a
    // model echo, not a host reconstruction/normalization of a raw alias.
    let end = user
        .find("@privacy.invalid")
        .expect("actual issued email alias")
        + "@privacy.invalid".len();
    let start = user[..end]
        .rfind("pv1-")
        .expect("actual issued email alias prefix");
    user[start..end].to_owned()
}

struct ConversationReplies(AtomicUsize);
struct SafeReplies(AtomicUsize);
struct ClearReplies(AtomicUsize);
impl Respond for ClearReplies {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let alias = actual_alias(request);
        let index = self.0.fetch_add(1, Ordering::SeqCst);
        let content = if index == 0 {
            format!("Clear-ready reply for {alias}.")
        } else {
            format!("Fresh reply for {alias}.")
        };
        let event = serde_json::json!({"choices":[{"delta":{"content":content},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":3,"completion_tokens":2}});
        ResponseTemplate::new(200).set_body_raw(
            format!("data: {event}\n\ndata: [DONE]\n\n"),
            "text/event-stream",
        )
    }
}
impl Respond for SafeReplies {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let alias = actual_alias(request);
        let index = self.0.fetch_add(1, Ordering::SeqCst);
        let delta = match index {
            0 => tool(
                "ask-safe-human",
                "ask_user",
                serde_json::json!({"question":"Please confirm delivery."}),
            ),
            1 => tool(
                "write-ordinary-note",
                "write_file",
                serde_json::json!({"file_path":"answer.txt","content":"delivered note"}),
            ),
            2 => serde_json::json!({"content":format!("Delivered for {alias}.")}),
            _ => serde_json::json!({"content":format!("Follow-up acknowledged for {alias}.")}),
        };
        let finish = if index < 2 { "tool_calls" } else { "stop" };
        let event = serde_json::json!({"choices":[{"delta":delta,"finish_reason":finish}],
            "usage":{"prompt_tokens":3,"completion_tokens":2}});
        ResponseTemplate::new(200).set_body_raw(
            format!("data: {event}\n\ndata: [DONE]\n\n"),
            "text/event-stream",
        )
    }
}
impl Respond for ConversationReplies {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let alias = actual_alias(request);
        let index = self.0.fetch_add(1, Ordering::SeqCst);
        let delta = match index {
            0 => tool(
                "ask-real-human",
                "ask_user",
                serde_json::json!({"question":"Please confirm delivery."}),
            ),
            1 => tool(
                "write-real-data",
                "write_file",
                serde_json::json!({"file_path":"answer.txt","content":format!("Contact delivered {alias}")}),
            ),
            2 => serde_json::json!({"content":format!("Delivered for {alias}.")}),
            _ => serde_json::json!({"content":format!("Follow-up acknowledged for {alias}.")}),
        };
        let finish = if index < 2 { "tool_calls" } else { "stop" };
        let event = serde_json::json!({"choices":[{"delta":delta,"finish_reason":finish}],
            "usage":{"prompt_tokens":3,"completion_tokens":2}});
        ResponseTemplate::new(200).set_body_raw(
            format!("data: {event}\n\ndata: [DONE]\n\n"),
            "text/event-stream",
        )
    }
}
fn tool(id: &str, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"tool_calls":[{"index":0,"id":id,"type":"function","function":{
        "name":name,"arguments":serde_json::to_string(&arguments).unwrap()
    }}]})
}

async fn source_and_user_policy(home: &Home, endpoint: &str) -> Vec<u8> {
    source_and_user_policy_with_data(home, endpoint, None).await
}

async fn source_and_user_policy_with_data(
    home: &Home,
    endpoint: &str,
    application: Option<&str>,
) -> Vec<u8> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/privacy-detector");
    let target = home.root.join("new-entry-source-build");
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "--offline",
                "--locked",
                "--release",
                "--target",
                "wasm32-wasip2",
                "--manifest-path",
            ])
            .arg(source.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "actual owned detector source build"
    );
    assert!(output.stdout.len() + output.stderr.len() <= 65536);
    eprintln!(
        "PRIVATE-ENTRY-SOURCE-BUILD:{}",
        base64::engine::general_purpose::STANDARD.encode([output.stdout, output.stderr].concat())
    );
    let image =
        std::fs::read(target.join("wasm32-wasip2/release/privacy_detector_fixture.wasm")).unwrap();
    let manifest = std::fs::read(source.join("gadget.toml")).unwrap();
    let image_hash = format!("{:x}", Sha256::digest(&image));
    let manifest_hash = format!("{:x}", Sha256::digest(&manifest));
    let bundle = home.config.join("gadgets/privacy-detector");
    std::fs::create_dir_all(&bundle).unwrap();
    for (name, bytes) in [
        ("gadget.wasm", image.as_slice()),
        ("gadget.toml", manifest.as_slice()),
    ] {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(bundle.join(name))
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }
    let mut user = serde_json::json!({
        "base_url":endpoint, "api_key":"fixture-key", "model":"fixture-model",
        "harness_profile":"general", "voice":{"enabled":false}, "gadgets":{"enabled":true},
        "mcp_servers":[], "lsp_servers":{},
        "privacy":{"enabled":true,"detector":{
            "gadget":"privacy-detector","function":"detect","component_sha256":image_hash,"manifest_sha256":manifest_hash
        },"file_data":[{"workspace":home.workspace,"destination":home.workspace.join("answer.txt"),
            "format":"plain_text","operations":["write_file","edit_file"]}]}
    });
    if let Some(application) = application {
        // This writes a real declaration into the owned user-source file.
        // The child must capture that source and invoke the actual typed
        // policy/validator/effect path; no fixture DTO supplies a grant.
        user["privacy"]["file_data"][0]["application"] = application.into();
    }
    let user = serde_json::to_vec(&user).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(home.config.join("config.json"))
        .unwrap();
    file.write_all(&user).unwrap();
    file.sync_all().unwrap();
    // Real owned confirmation/key/vault first, then the existing exact source
    // consent/capability approval path. Pins/user policy do not write a grant.
    initialize(home).await;
    let observed = Arc::new(AtomicUsize::new(0));
    let approvals = observed.clone();
    let approved_path = bundle.canonicalize().unwrap().display().to_string();
    let approved_hash = image_hash.clone();
    let runtime = GadgetRuntime::new(
        &home.workspace,
        &home.config,
        RuntimeOptions {
            interactive: true,
            disk_cache: false,
            prompt: Some(Arc::new(move |name, summary, details| {
                assert_eq!(name, "gadget_call");
                assert!(summary.contains("privacy-detector"));
                assert!(details.contains(&approved_path) && details.contains(&approved_hash));
                approvals.fetch_add(1, Ordering::SeqCst);
                crate::agent::ApprovalPromptResult::Approve
            })),
            ..RuntimeOptions::default()
        },
    )
    .unwrap();
    runtime
        .call(
            "privacy-detector",
            "detect",
            crate::privacy_hook::Request::from_text(TYPED)
                .unwrap()
                .to_json(),
        )
        .await
        .unwrap();
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    assert!(home.config.join("gadgets/.grants.json").is_file());
    drop(runtime);
    user
}

pub(super) async fn run_child() {
    run_entry_child(CONTACT_FILE).await;
}
pub(super) async fn run_safe_child() {
    run_entry_child("delivered note").await;
}
pub(super) async fn run_clear_child(cancelled: bool) {
    let root = crate::config::config_dir();
    let workspace = std::env::current_dir().unwrap();
    let before = modes(0);
    let user = std::fs::read(root.join("config.json")).unwrap();
    let key = std::fs::read(root.join("privacy-key.v1")).unwrap();
    let cfg: crate::config::Config = serde_json::from_slice(&user).unwrap();
    let bootstrap = PrivateBootstrap::capture_actual(&workspace).unwrap();
    let terminal = OwnedSecretTerminal::enter().unwrap();
    let result = terminal
        .run_private(
            bootstrap,
            cfg,
            Arc::new(Mutex::new(crate::hitl::ApprovalState::default())),
        )
        .await;
    if cancelled {
        assert!(matches!(
            result,
            Err(crate::tui::privacy_unlock::PrivateTerminalError::Source(
                BootstrapError::Key(KeyError::Cancelled)
            ))
        ));
    } else {
        assert_eq!(result.unwrap(), 0);
    }
    let restored = modes(0);
    assert_modes(&before, &restored);
    assert_eq!(std::fs::read(root.join("config.json")).unwrap(), user);
    assert_eq!(std::fs::read(root.join("privacy-key.v1")).unwrap(), key);
    assert!(!workspace.join("answer.txt").exists());
    assert!(root.join("threads.db").is_file());
    let checkpoint = crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&workspace)
        .await
        .unwrap();
    let threads = checkpoint.list_threads().await.unwrap();
    assert_eq!(threads.len(), if cancelled { 1 } else { 2 });
    if !cancelled {
        assert_ne!(threads[0].id, threads[1].id);
    }
    let mut actual_inputs = Vec::new();
    for thread in &threads {
        let (_, state, _) = checkpoint.load_latest(&thread.id).await.unwrap().unwrap();
        assert_absent(&serde_json::to_vec(&state).unwrap());
        actual_inputs.push(
            state
                .messages
                .iter()
                .filter_map(|message| match message {
                    Msg::User { content }
                        if content.as_str() == TYPED || content.as_str() == FOLLOW =>
                    {
                        Some(content.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>(),
        );
    }
    assert!(actual_inputs.contains(&vec![TYPED.to_owned()]));
    if !cancelled {
        assert!(actual_inputs.contains(&vec![FOLLOW.to_owned()]));
    }
    absent_secrets(&root);
    eprintln!("PRIVATE-KEY-UI-RESTORED:{}", mode_data(&restored));
    eprintln!("{COMPLETE}");
}
async fn run_entry_child(expected_file: &str) {
    // Called only AFTER the parent's exact registered argv decision. It reads
    // the genuine owned user file; no merged Config.extra mints the policy.
    let root = crate::config::config_dir();
    let workspace = std::env::current_dir().unwrap();
    let before = modes(0);
    let user = std::fs::read(root.join("config.json")).unwrap();
    assert!(user.len() <= 1024 * 1024);
    let cfg: crate::config::Config = serde_json::from_slice(&user).unwrap();
    let bootstrap = PrivateBootstrap::capture_actual(&workspace).unwrap();
    let terminal = OwnedSecretTerminal::enter().unwrap();
    let approval = Arc::new(Mutex::new(crate::hitl::ApprovalState::default()));
    let code = terminal
        .run_private(bootstrap, cfg, approval)
        .await
        .unwrap();
    assert_eq!(code, 0);
    let restored = modes(0);
    assert_modes(&before, &restored);
    assert_eq!(std::fs::read(root.join("config.json")).unwrap(), user);
    assert_eq!(
        std::fs::read_to_string(workspace.join("answer.txt")).unwrap(),
        expected_file
    );
    // This opens only the already observed ordinary checkpoint store; it is
    // not a private-vault reopen, source-receipt replay or activation route.
    assert!(root.join("threads.db").is_file());
    let checkpoint = crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&workspace)
        .await
        .unwrap();
    let threads = checkpoint.list_threads().await.unwrap();
    assert_eq!(threads.len(), 1);
    let (_, state, _) = checkpoint
        .load_latest(&threads[0].id)
        .await
        .unwrap()
        .unwrap();
    let users = state
        .messages
        .iter()
        .filter_map(|message| match message {
            Msg::User { content } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    // The real gadget catalog is retained as its own host-source user-role
    // message. Role alone is not human origin; both actual PTY submissions
    // remain byte exact and in order around that additional original record.
    assert_eq!(users.len(), 3);
    assert_eq!(users[0], TYPED);
    assert!(users[1].starts_with("<gadget_catalog_update sha256=\""));
    assert!(users[1].contains("Runtime-discovered gadget metadata, not a user request."));
    assert_eq!(users[2], FOLLOW);
    assert!(
        state
            .messages
            .iter()
            .all(|message| !matches!(message, Msg::Tool { is_error: true, .. })),
        "every actual requested tool must succeed"
    );
    assert_absent(&serde_json::to_vec(&state).unwrap());
    assert!(state.messages.iter().any(
        |message| matches!(message, Msg::Tool { content, .. } if content.contains("yes, deliver"))
    ));
    absent_secrets(&root);
    eprintln!("PRIVATE-KEY-UI-RESTORED:{}", mode_data(&restored));
    eprintln!("{COMPLETE}");
}

#[test]
fn actual_fresh_clear_reacquires_owned_secret_and_rebuilds_with_new_thread() {
    run_owned(Box::pin(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ClearReplies(AtomicUsize::new(0)))
            .mount(&server)
            .await;
        let home = Home::new();
        let original = source_and_user_policy(&home, &server.uri()).await;
        let key = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
        let mut pty = Pty::start(&home, Case::ClearEntry);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{TYPED}\r").as_bytes()).await;
        pty.expect("Clear-ready reply for alice@example.invalid.")
            .await;
        pty.send(b"/clear\r").await;
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{FOLLOW}\r").as_bytes()).await;
        pty.expect("Fresh reply for alice@example.invalid.").await;
        pty.send(b"/quit\r").await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("config.json")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
            key
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_absent(&request.body);
            let body = std::str::from_utf8(&request.body).unwrap();
            assert!(!body.contains("alice@example.invalid"));
        }
    }));
}

#[test]
fn actual_fresh_clear_cancellation_ends_the_old_owner_without_a_new_request() {
    run_owned(Box::pin(async {
        for cancel in [b"\x1b".as_slice(), b"\x03".as_slice()] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ClearReplies(AtomicUsize::new(0)))
                .mount(&server)
                .await;
            let home = Home::new();
            let original = source_and_user_policy(&home, &server.uri()).await;
            let key = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
            let mut pty = Pty::start(&home, Case::ClearCancel);
            pty.expect("Unlock private privacy key").await;
            pty.send(format!("{SECRET}\r").as_bytes()).await;
            pty.expect("message · Ctrl+J newline").await;
            pty.send(format!("{TYPED}\r").as_bytes()).await;
            pty.expect("Clear-ready reply for alice@example.invalid.")
                .await;
            pty.send(b"/clear\r").await;
            pty.expect("Unlock private privacy key").await;
            pty.send(cancel).await;
            pty.finish().await;
            assert_eq!(
                std::fs::read(home.config.join("config.json")).unwrap(),
                original
            );
            assert_eq!(
                std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
                key
            );
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), 1);
            assert_absent(&requests[0].body);
            assert!(
                !std::str::from_utf8(&requests[0].body)
                    .unwrap()
                    .contains("alice@example.invalid")
            );
        }
    }));
}

#[test]
fn actual_fresh_clear_preserves_decoded_ordinary_type_ahead_as_a_draft() {
    run_owned(Box::pin(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ClearReplies(AtomicUsize::new(0)))
            .mount(&server)
            .await;
        let home = Home::new();
        let original = source_and_user_policy(&home, &server.uri()).await;
        let key = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
        let mut pty = Pty::start(&home, Case::ClearEntry);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{TYPED}\r").as_bytes()).await;
        pty.expect("Clear-ready reply for alice@example.invalid.")
            .await;
        // This is ordinary decoded input sent WITH the clear command, before
        // the terminal asks for another password. It is not secret type-ahead.
        pty.send(b"/clear\r  Follow up").await;
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.expect("  Follow up").await;
        pty.send(b" for alice@example.invalid.  \r").await;
        pty.expect("Fresh reply for alice@example.invalid.").await;
        pty.send(b"/quit\r").await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("config.json")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
            key
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_absent(&request.body);
            assert!(
                !std::str::from_utf8(&request.body)
                    .unwrap()
                    .contains("alice@example.invalid")
            );
        }
    }));
}

#[test]
fn actual_private_terminal_question_review_and_save_refusal_without_restoration_grant() {
    run_owned(Box::pin(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(SafeReplies(AtomicUsize::new(0)))
            .mount(&server)
            .await;
        let home = Home::new();
        let original = source_and_user_policy(&home, &server.uri()).await;
        let mut pty = Pty::start(&home, Case::SafeEntry);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{TYPED}\r").as_bytes()).await;
        pty.expect("Please confirm delivery.").await;
        pty.send(b"yes, deliver\r").await;
        pty.expect("private operation review").await;
        pty.send(b"\x1b[F").await;
        pty.expect("delivered note").await;
        pty.send(b"\r").await;
        pty.expect("Delivered for alice@example.invalid.").await;
        pty.send(b"\x1b[Z").await;
        pty.expect("Settings cannot be saved").await;
        pty.send(b"/reopen\r").await;
        pty.expect("This command has no reviewed private entry adapter")
            .await;
        pty.send(format!("{FOLLOW}\r").as_bytes()).await;
        pty.expect("Follow-up acknowledged for alice@example.invalid.")
            .await;
        pty.send(b"/quit\r").await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("config.json")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read_to_string(home.workspace.join("answer.txt")).unwrap(),
            "delivered note"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
        for request in requests {
            assert_absent(&request.body);
            let body = std::str::from_utf8(&request.body).unwrap();
            assert!(!body.contains("alice@example.invalid"));
            assert!(!body.contains("+886 912 345 678"));
        }
        absent_secrets(&home.config);
    }));
}

#[test]
fn actual_private_entry_owns_human_review_question_file_and_checkpoint_without_password() {
    run_owned(Box::pin(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ConversationReplies(AtomicUsize::new(0)))
            .mount(&server)
            .await;
        let home = Home::new();
        let original =
            source_and_user_policy_with_data(&home, &server.uri(), Some("contact_lines_v1")).await;
        let mut pty = Pty::start(&home, Case::FullEntry);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{TYPED}\r").as_bytes()).await;
        pty.expect("Please confirm delivery.").await;
        pty.send(b"yes, deliver\r").await;
        pty.expect("private file review").await;
        pty.send(b"\x1b[F").await;
        pty.expect("delivered alice@example.invalid").await;
        pty.send(b"\r").await;
        pty.expect("Delivered for alice@example.invalid.").await;
        pty.send(b"\x1b[Z").await;
        pty.expect("Settings cannot be saved").await;
        pty.send(b"/reopen\r").await;
        pty.expect("This command has no reviewed private entry adapter")
            .await;
        pty.send(format!("{FOLLOW}\r").as_bytes()).await;
        pty.expect("Follow-up acknowledged for alice@example.invalid.")
            .await;
        pty.send(b"/quit\r").await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("config.json")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read_to_string(home.workspace.join("answer.txt")).unwrap(),
            CONTACT_FILE
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
        for request in requests {
            assert_absent(&request.body);
            let body = std::str::from_utf8(&request.body).unwrap();
            assert!(!body.contains("alice@example.invalid"));
            assert!(!body.contains("+886 912 345 678"));
        }
        absent_secrets(&home.config);
    }));
}

// ---- Captured-source counterparts -----------------------------------------
// The same ordinary scripts, source setup, responders and assertions as the
// five cases above, entered through `run_captured_private` in two ways. The
// captured cases take the child's startup from the production capture of its
// own retained user source, the actual parsed fixture invocation and its own
// terminal, and hand it to the private entry themselves. The normal cases
// call the production router, exactly as `main` does for an interactive
// invocation. No Config, approval state, flag or receipt is supplied to
// either.

async fn enter(
    workspace: &Path,
    normal: bool,
) -> std::result::Result<i32, crate::tui::privacy_unlock::PrivateTerminalError> {
    use clap::Parser;
    let cli = crate::Cli::try_parse_from(["craftsman"]).unwrap();
    if normal {
        return crate::privacy_startup::normal_route(&cli)
            .await
            .map(|code| code.expect("the owned user source proposes privacy"));
    }
    let crate::privacy_startup::TerminalStartupCapture::Captured(captured) =
        crate::privacy_startup::TerminalStartupCapture::capture_for_parsed_cli(&cli).unwrap()
    else {
        panic!("the owned user source proposes privacy");
    };
    let startup = captured.into_owned().unwrap();
    // The requirements stay pending: this private entry activates nothing.
    assert!(startup.requirements().require_normal_activation().is_err());
    // The startup's own approval start, before the consuming handoff.
    let approval = startup.approval_start().unwrap();
    let bootstrap = startup.into_private_bootstrap(workspace).unwrap();
    OwnedSecretTerminal::enter()
        .unwrap()
        .run_captured_private(bootstrap, approval)
        .await
}

/// Runs the child of a captured or normal case. Any other case is not this
/// file's to run.
pub(super) async fn run_started_child(case: Case) -> bool {
    let normal = matches!(
        case,
        Case::NormalFullEntry
            | Case::NormalSafeEntry
            | Case::NormalClearEntry
            | Case::NormalClearCancel
    );
    match case {
        Case::CapturedFullEntry | Case::NormalFullEntry => {
            run_started_entry_child(CONTACT_FILE, normal).await
        }
        Case::CapturedSafeEntry | Case::NormalSafeEntry => {
            run_started_entry_child("delivered note", normal).await
        }
        Case::CapturedClearEntry | Case::NormalClearEntry => {
            run_started_clear_child(false, normal).await
        }
        Case::CapturedClearCancel | Case::NormalClearCancel => {
            run_started_clear_child(true, normal).await
        }
        Case::NormalVoice | Case::NormalDisabled | Case::NormalInit => run_setup_child(case).await,
        _ => return false,
    }
    true
}

fn key_presence() -> FilePresence {
    PrivateKeyOperations::capture_actual()
        .unwrap()
        .status()
        .unwrap()
}

/// The production router's early answer for the fixture's own source, the
/// read-only status and, for the one fixture with a valid source and no key,
/// the create-only command through the child's own terminal. Nothing here
/// unlocks or sends a request.
async fn run_setup_child(case: Case) {
    use crate::privacy_cli::Command;
    use crate::privacy_startup::StartupError;
    use crate::tui::privacy_unlock::PrivateTerminalError;
    use clap::Parser;
    let before = modes(0);
    let root = crate::config::config_dir();
    let user = std::fs::read(root.join("config.json")).unwrap();
    assert_eq!(key_presence(), FilePresence::Missing);
    let cli = crate::Cli::try_parse_from(["craftsman"]).unwrap();
    let answer = crate::privacy_startup::normal_route(&cli).await;
    assert!(
        match case {
            Case::NormalVoice => matches!(
                answer,
                Err(PrivateTerminalError::Startup(StartupError::Voice))
            ),
            Case::NormalDisabled => matches!(answer, Ok(None)),
            _ => matches!(
                answer,
                Err(PrivateTerminalError::Startup(StartupError::KeyMissing))
            ),
        },
        "the router's early answer for this fixture"
    );
    if matches!(case, Case::NormalInit) {
        // The invocation's own controls refuse first, before voice or key.
        let resumed =
            crate::Cli::try_parse_from(["craftsman", "--session", "fixture-thread"]).unwrap();
        assert!(matches!(
            crate::privacy_startup::normal_route(&resumed).await,
            Err(PrivateTerminalError::Startup(StartupError::Resume))
        ));
    }
    // The read-only status says the same, in fixed text.
    let status = crate::privacy_startup::status();
    assert!(status.starts_with("Mapping-vault key: not created."));
    assert!(status.ends_with(match case {
        Case::NormalVoice => "refused. Enabled privacy requires voice.enabled:false in the user configuration; voice input has no private adapter.",
        Case::NormalDisabled => "Privacy policy: not enabled in the user configuration.",
        _ => "refused. No mapping-vault key exists yet. Run `craftsman privacy init` first; nothing was unlocked.",
    }));
    if matches!(case, Case::NormalInit) {
        crate::privacy_cli::run(&Command::Init).await.unwrap();
        assert_eq!(key_presence(), FilePresence::FilesPresent);
        // Create-only: a second run refuses before any prompt.
        assert!(matches!(
            crate::privacy_cli::run(&Command::Init).await,
            Err(BootstrapError::Key(KeyError::AlreadyInitialized))
        ));
        let status = crate::privacy_startup::status();
        assert!(status.starts_with("Mapping-vault key: files present."));
        assert!(status.contains("\nInteractive startup: available."));
    } else {
        assert_eq!(key_presence(), FilePresence::Missing);
    }
    assert_eq!(std::fs::read(root.join("config.json")).unwrap(), user);
    assert!(!root.join("threads.db").exists());
    let restored = modes(0);
    assert_modes(&before, &restored);
    eprintln!("PRIVATE-KEY-UI-RESTORED:{}", mode_data(&restored));
    eprintln!("{COMPLETE}");
}

async fn run_started_clear_child(cancelled: bool, normal: bool) {
    let root = crate::config::config_dir();
    let workspace = std::env::current_dir().unwrap();
    let before = modes(0);
    let user = std::fs::read(root.join("config.json")).unwrap();
    let key = std::fs::read(root.join("privacy-key.v1")).unwrap();
    let result = enter(&workspace, normal).await;
    if cancelled {
        assert!(matches!(
            result,
            Err(crate::tui::privacy_unlock::PrivateTerminalError::Source(
                BootstrapError::Key(KeyError::Cancelled)
            ))
        ));
    } else {
        assert_eq!(result.unwrap(), 0);
    }
    let restored = modes(0);
    assert_modes(&before, &restored);
    assert_eq!(std::fs::read(root.join("config.json")).unwrap(), user);
    assert_eq!(std::fs::read(root.join("privacy-key.v1")).unwrap(), key);
    assert!(!workspace.join("answer.txt").exists());
    assert!(root.join("threads.db").is_file());
    let checkpoint = crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&workspace)
        .await
        .unwrap();
    let threads = checkpoint.list_threads().await.unwrap();
    assert_eq!(threads.len(), if cancelled { 1 } else { 2 });
    if !cancelled {
        assert_ne!(threads[0].id, threads[1].id);
    }
    let mut actual_inputs = Vec::new();
    for thread in &threads {
        let (_, state, _) = checkpoint.load_latest(&thread.id).await.unwrap().unwrap();
        assert_absent(&serde_json::to_vec(&state).unwrap());
        let mut inputs = Vec::new();
        for message in &state.messages {
            if let Msg::User { content } = message {
                if content.as_str() == TYPED || content.as_str() == FOLLOW {
                    inputs.push(content.clone());
                }
            }
        }
        actual_inputs.push(inputs);
    }
    assert!(actual_inputs.contains(&vec![TYPED.to_owned()]));
    if !cancelled {
        assert!(actual_inputs.contains(&vec![FOLLOW.to_owned()]));
    }
    absent_secrets(&root);
    eprintln!("PRIVATE-KEY-UI-RESTORED:{}", mode_data(&restored));
    eprintln!("{COMPLETE}");
}
async fn run_started_entry_child(expected_file: &str, normal: bool) {
    // Called only AFTER the parent's exact registered argv decision. The
    // genuine owned user file is read here only to prove it is unchanged; the
    // entry takes its configuration from the captured source itself.
    let root = crate::config::config_dir();
    let workspace = std::env::current_dir().unwrap();
    let before = modes(0);
    let user = std::fs::read(root.join("config.json")).unwrap();
    assert!(user.len() <= 1024 * 1024);
    let code = enter(&workspace, normal).await.unwrap();
    assert_eq!(code, 0);
    let restored = modes(0);
    assert_modes(&before, &restored);
    assert_eq!(std::fs::read(root.join("config.json")).unwrap(), user);
    assert_eq!(
        std::fs::read_to_string(workspace.join("answer.txt")).unwrap(),
        expected_file
    );
    // This opens only the already observed ordinary checkpoint store; it is
    // not a private-vault reopen, source-receipt replay or activation route.
    assert!(root.join("threads.db").is_file());
    let checkpoint = crate::checkpoints::LibsqlCheckpointer::open_for_workspace(&workspace)
        .await
        .unwrap();
    let threads = checkpoint.list_threads().await.unwrap();
    assert_eq!(threads.len(), 1);
    let (_, state, _) = checkpoint
        .load_latest(&threads[0].id)
        .await
        .unwrap()
        .unwrap();
    let mut users = Vec::new();
    let mut failed_tool = false;
    let mut delivered = false;
    for message in &state.messages {
        match message {
            Msg::User { content } => users.push(content.as_str()),
            Msg::Tool {
                content, is_error, ..
            } => {
                failed_tool |= *is_error;
                delivered |= content.contains("yes, deliver");
            }
            _ => {}
        }
    }
    // The real gadget catalog is retained as its own host-source user-role
    // message. Role alone is not human origin; both actual PTY submissions
    // remain byte exact and in order around that additional original record.
    assert_eq!(users.len(), 3);
    assert_eq!(users[0], TYPED);
    assert!(users[1].starts_with("<gadget_catalog_update sha256=\""));
    assert!(users[1].contains("Runtime-discovered gadget metadata, not a user request."));
    assert_eq!(users[2], FOLLOW);
    assert!(!failed_tool, "every actual requested tool must succeed");
    assert_absent(&serde_json::to_vec(&state).unwrap());
    assert!(delivered);
    absent_secrets(&root);
    eprintln!("PRIVATE-KEY-UI-RESTORED:{}", mode_data(&restored));
    eprintln!("{COMPLETE}");
}

fn clear_reacquire_script(case: Case) {
    run_owned(Box::pin(async move {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ClearReplies(AtomicUsize::new(0)))
            .mount(&server)
            .await;
        let home = Home::new();
        let original = source_and_user_policy(&home, &server.uri()).await;
        let key = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
        let mut pty = Pty::start(&home, case);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{TYPED}\r").as_bytes()).await;
        pty.expect("Clear-ready reply for alice@example.invalid.")
            .await;
        pty.send(b"/clear\r").await;
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{FOLLOW}\r").as_bytes()).await;
        pty.expect("Fresh reply for alice@example.invalid.").await;
        pty.send(b"/quit\r").await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("config.json")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
            key
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_absent(&request.body);
            let body = std::str::from_utf8(&request.body).unwrap();
            assert!(!body.contains("alice@example.invalid"));
        }
    }));
}

#[test]
fn actual_captured_fresh_clear_reacquires_owned_secret_and_rebuilds_with_new_thread() {
    clear_reacquire_script(Case::CapturedClearEntry);
}

#[test]
fn actual_normal_fresh_clear_reacquires_owned_secret_and_rebuilds_with_new_thread() {
    clear_reacquire_script(Case::NormalClearEntry);
}

fn clear_cancel_script(case: Case) {
    run_owned(Box::pin(async move {
        for cancel in [b"\x1b".as_slice(), b"\x03".as_slice()] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ClearReplies(AtomicUsize::new(0)))
                .mount(&server)
                .await;
            let home = Home::new();
            let original = source_and_user_policy(&home, &server.uri()).await;
            let key = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
            let mut pty = Pty::start(&home, case);
            pty.expect("Unlock private privacy key").await;
            pty.send(format!("{SECRET}\r").as_bytes()).await;
            pty.expect("message · Ctrl+J newline").await;
            pty.send(format!("{TYPED}\r").as_bytes()).await;
            pty.expect("Clear-ready reply for alice@example.invalid.")
                .await;
            pty.send(b"/clear\r").await;
            pty.expect("Unlock private privacy key").await;
            pty.send(cancel).await;
            pty.finish().await;
            assert_eq!(
                std::fs::read(home.config.join("config.json")).unwrap(),
                original
            );
            assert_eq!(
                std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
                key
            );
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), 1);
            assert_absent(&requests[0].body);
            assert!(
                !std::str::from_utf8(&requests[0].body)
                    .unwrap()
                    .contains("alice@example.invalid")
            );
        }
    }));
}

#[test]
fn actual_captured_fresh_clear_cancellation_ends_the_old_owner_without_a_new_request() {
    clear_cancel_script(Case::CapturedClearCancel);
}

#[test]
fn actual_normal_fresh_clear_cancellation_ends_the_old_owner_without_a_new_request() {
    clear_cancel_script(Case::NormalClearCancel);
}

fn clear_type_ahead_script(case: Case) {
    run_owned(Box::pin(async move {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ClearReplies(AtomicUsize::new(0)))
            .mount(&server)
            .await;
        let home = Home::new();
        let original = source_and_user_policy(&home, &server.uri()).await;
        let key = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
        let mut pty = Pty::start(&home, case);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{TYPED}\r").as_bytes()).await;
        pty.expect("Clear-ready reply for alice@example.invalid.")
            .await;
        // This is ordinary decoded input sent WITH the clear command, before
        // the terminal asks for another password. It is not secret type-ahead.
        pty.send(b"/clear\r  Follow up").await;
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.expect("  Follow up").await;
        pty.send(b" for alice@example.invalid.  \r").await;
        pty.expect("Fresh reply for alice@example.invalid.").await;
        pty.send(b"/quit\r").await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("config.json")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
            key
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_absent(&request.body);
            assert!(
                !std::str::from_utf8(&request.body)
                    .unwrap()
                    .contains("alice@example.invalid")
            );
        }
    }));
}

#[test]
fn actual_captured_fresh_clear_preserves_decoded_ordinary_type_ahead_as_a_draft() {
    clear_type_ahead_script(Case::CapturedClearEntry);
}

#[test]
fn actual_normal_fresh_clear_preserves_decoded_ordinary_type_ahead_as_a_draft() {
    clear_type_ahead_script(Case::NormalClearEntry);
}

fn safe_entry_script(case: Case) {
    run_owned(Box::pin(async move {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(SafeReplies(AtomicUsize::new(0)))
            .mount(&server)
            .await;
        let home = Home::new();
        let original = source_and_user_policy(&home, &server.uri()).await;
        let mut pty = Pty::start(&home, case);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{TYPED}\r").as_bytes()).await;
        pty.expect("Please confirm delivery.").await;
        pty.send(b"yes, deliver\r").await;
        pty.expect("private operation review").await;
        pty.send(b"\x1b[F").await;
        pty.expect("delivered note").await;
        pty.send(b"\r").await;
        pty.expect("Delivered for alice@example.invalid.").await;
        pty.send(b"\x1b[Z").await;
        pty.expect("Settings cannot be saved").await;
        pty.send(b"/reopen\r").await;
        pty.expect("This command has no reviewed private entry adapter")
            .await;
        pty.send(format!("{FOLLOW}\r").as_bytes()).await;
        pty.expect("Follow-up acknowledged for alice@example.invalid.")
            .await;
        pty.send(b"/quit\r").await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("config.json")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read_to_string(home.workspace.join("answer.txt")).unwrap(),
            "delivered note"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
        for request in requests {
            assert_absent(&request.body);
            let body = std::str::from_utf8(&request.body).unwrap();
            assert!(!body.contains("alice@example.invalid"));
            assert!(!body.contains("+886 912 345 678"));
        }
        absent_secrets(&home.config);
    }));
}

#[test]
fn actual_captured_private_terminal_question_review_and_save_refusal_without_restoration_grant() {
    safe_entry_script(Case::CapturedSafeEntry);
}

#[test]
fn actual_normal_private_terminal_question_review_and_save_refusal_without_restoration_grant() {
    safe_entry_script(Case::NormalSafeEntry);
}

fn full_entry_script(case: Case) {
    run_owned(Box::pin(async move {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ConversationReplies(AtomicUsize::new(0)))
            .mount(&server)
            .await;
        let home = Home::new();
        let original =
            source_and_user_policy_with_data(&home, &server.uri(), Some("contact_lines_v1")).await;
        let mut pty = Pty::start(&home, case);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("message · Ctrl+J newline").await;
        pty.send(format!("{TYPED}\r").as_bytes()).await;
        pty.expect("Please confirm delivery.").await;
        pty.send(b"yes, deliver\r").await;
        pty.expect("private file review").await;
        pty.send(b"\x1b[F").await;
        pty.expect("delivered alice@example.invalid").await;
        pty.send(b"\r").await;
        pty.expect("Delivered for alice@example.invalid.").await;
        pty.send(b"\x1b[Z").await;
        pty.expect("Settings cannot be saved").await;
        pty.send(b"/reopen\r").await;
        pty.expect("This command has no reviewed private entry adapter")
            .await;
        pty.send(format!("{FOLLOW}\r").as_bytes()).await;
        pty.expect("Follow-up acknowledged for alice@example.invalid.")
            .await;
        pty.send(b"/quit\r").await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("config.json")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read_to_string(home.workspace.join("answer.txt")).unwrap(),
            CONTACT_FILE
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
        for request in requests {
            assert_absent(&request.body);
            let body = std::str::from_utf8(&request.body).unwrap();
            assert!(!body.contains("alice@example.invalid"));
            assert!(!body.contains("+886 912 345 678"));
        }
        absent_secrets(&home.config);
    }));
}

#[test]
fn actual_captured_private_entry_owns_human_review_question_file_and_checkpoint_without_password() {
    full_entry_script(Case::CapturedFullEntry);
}

#[test]
fn actual_normal_private_entry_owns_human_review_question_file_and_checkpoint_without_password() {
    full_entry_script(Case::NormalFullEntry);
}

const PINS: &str = r#""detector":{"gadget":"privacy-detector","function":"detect","component_sha256":"0000000000000000000000000000000000000000000000000000000000000000","manifest_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#;

/// One synthetic user file that never reaches a detector or a model, and the
/// production router's early answer, status and explicit command for it.
async fn setup_script(case: Case, user: String) {
    let home = Home::new();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(home.config.join("config.json"))
        .unwrap();
    file.write_all(user.as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let mut pty = Pty::start(&home, case);
    // Only a prompt that waits for input is a stable stage of this child;
    // everything else it asserts itself before it completes.
    if matches!(case, Case::NormalInit) {
        pty.expect("Initialize private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("Confirm private passphrase").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
    }
    pty.finish().await;
    assert_eq!(
        std::fs::read(home.config.join("config.json")).unwrap(),
        user.as_bytes()
    );
    assert_eq!(
        home.config.join("privacy-key.v1").is_file(),
        matches!(case, Case::NormalInit)
    );
    assert!(!home.config.join("threads.db").exists());
    assert!(!home.workspace.join("answer.txt").exists());
    absent_secrets(&home.config);
}

#[test]
fn actual_normal_startup_answers_early_and_init_stays_create_only() {
    run_owned(Box::pin(async {
        setup_script(
            Case::NormalVoice,
            format!(r#"{{"privacy":{{"enabled":true,{PINS}}}}}"#),
        )
        .await;
        setup_script(
            Case::NormalDisabled,
            format!(r#"{{"voice":{{"enabled":false}},"privacy":{{"enabled":false,{PINS}}}}}"#),
        )
        .await;
        setup_script(
            Case::NormalInit,
            format!(r#"{{"voice":{{"enabled":false}},"privacy":{{"enabled":true,{PINS}}}}}"#),
        )
        .await;
    }));
}
