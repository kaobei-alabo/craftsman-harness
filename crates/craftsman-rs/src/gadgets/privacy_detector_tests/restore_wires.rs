//! Marked native fixture: restoration on each model wire, through the shared
//! session builder, with the source-built detector approved and pinned at
//! its user-level source, a real vault and guard and a loopback server.
//!
//! On Chat, Responses, Claude Messages and Gemini a person approves a file
//! write after being shown its real content, the file gets that content, and
//! the person reads the reply with the value. The model, the record and
//! every request keep the placeholder.
//!
//! Unmarked startup returns before any build, path or configuration access.

use super::super::grants::GrantStore;
use super::super::manifest::Capabilities;
use super::{build_fixture, hash, root};
use crate::agent::{ApprovalPrompt, ApprovalPromptResult, SessionExtras, build_session_ext};
use crate::hitl::ApprovalState;
use crate::privacy_hook::{Detector, DetectorPin, PinnedDetector};
use crate::privacy_restore::FileDataFormat;
use crate::privacy_session::wire_fixtures::{config, reply};
use crate::privacy_session::{
    CapturedUserPolicy, FreshConversationBinding, PrivacySession, PrivacyViewSelection,
};
use crate::privacy_vault::{HostKey, HostKeySource, Limits, Vault, VaultLocation};
use craftsman_harness::Msg;
use serde_json::json;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};
use zeroize::Zeroizing;

const ID: &str = "privacy-detector";
const HELPER: &str = "gadgets::privacy_detector_tests::restore_wires::restore_wires_native_fixture";
const MARKER: &str = "craftsman-privacy-restore-wires=";
const EMAIL: &str = "alice@example.invalid";
const WIRES: [&str; 4] = ["chat", "responses", "claude", "gemini"];

fn selected(arguments: impl IntoIterator<Item = OsString>) -> bool {
    let mut values = Vec::new();
    for argument in arguments {
        if let Some(value) = argument
            .to_str()
            .and_then(|argument| argument.strip_prefix(MARKER))
        {
            values.push(value.to_owned());
        }
    }
    matches!(values.as_slice(), [value] if value == "suite")
}

struct SyntheticKey;

impl HostKeySource for SyntheticKey {
    fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([13; 32])))
    }
}

/// A user's policy with a rule for file content: plain text, for every file.
struct PlainTextFiles;

impl CapturedUserPolicy for PlainTextFiles {
    fn enables_privacy_in(&self, _: &str) -> bool {
        true
    }

    fn requires_inspectable_provider_items(&self) -> bool {
        false
    }

    fn restorable_file_data(&self, _: &str, _: &str) -> Option<FileDataFormat> {
        Some(FileDataFormat::PlainText)
    }
}

/// A reply made from the placeholder the request carries for the address in
/// its input.
struct FromInput(Box<dyn Fn(&str) -> wiremock::ResponseTemplate + Send + Sync>);

impl wiremock::Respond for FromInput {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let raw = String::from_utf8_lossy(&request.body);
        let placeholder = raw
            .split("Who handles ")
            .nth(1)
            .and_then(|rest| rest.split('?').next())
            .expect("the request carries the input");
        (self.0)(placeholder)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "four-wire restoration fixture; exactly one argv marker required before build/config access"]
async fn restore_wires_native_fixture() {
    if !selected(std::env::args_os()) {
        println!("PRIVACY-RESTORE-WIRES:NOT-SELECTED");
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
    let vault = Vault::open(
        VaultLocation::under_host_config_root(&config_root).unwrap(),
        &SyntheticKey,
        Limits::default(),
    )
    .await
    .unwrap();

    let mut written = 0;
    for wire in WIRES {
        let note = workspace.join(format!("restored-{wire}.txt"));
        let server = MockServer::start().await;
        let path = note.clone();
        Mock::given(method("POST"))
            .respond_with(FromInput(Box::new(move |placeholder| {
                reply(
                    wire,
                    0,
                    Some((
                        "wire-write",
                        "write_file",
                        &json!({"file_path": path, "content": format!("Contact {placeholder} today.")}),
                    )),
                    "",
                )
            })))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(FromInput(Box::new(move |placeholder| {
                reply(wire, 1, None, &format!("Saved it for {placeholder}."))
            })))
            .mount(&server)
            .await;
        let cfg = config(wire, &server);
        let detector = Detector::pinned(
            PinnedDetector::new(
                &workspace,
                &config_root,
                DetectorPin::supplied_by_host(ID, "detect", &component, &hash(&manifest)).unwrap(),
            )
            .unwrap(),
        );
        let owner = Arc::new(
            PrivacySession::open(
                &vault,
                FreshConversationBinding::reserve(&workspace).unwrap(),
                detector,
            )
            .await
            .unwrap(),
        );
        // What the approval prompt is shown before it approves.
        let shown: Arc<Mutex<Vec<String>>> = Arc::default();
        let prompt: ApprovalPrompt = {
            let shown = shown.clone();
            Arc::new(move |_, _, details| {
                shown.lock().unwrap().push(details.to_owned());
                ApprovalPromptResult::Approve
            })
        };
        let session = build_session_ext(
            &cfg,
            Arc::new(Mutex::new(ApprovalState::default())),
            "",
            false,
            false,
            None,
            SessionExtras {
                privacy_view: Some(
                    PrivacyViewSelection::from_user_policy(Arc::new(PlainTextFiles), owner.clone())
                        .unwrap(),
                ),
                approval_prompt: Some(prompt),
                workspace_root: Some(workspace.clone()),
                token_sink: Some(Arc::new(|_| {})),
                ..Default::default()
            },
        )
        .await
        .unwrap_or_else(|| panic!("{wire}: a session with a file rule is built"));
        let view = session.privacy_view.as_ref().unwrap();
        view.turn(&session.agent, &format!("Who handles {EMAIL}?"))
            .await
            .unwrap_or_else(|error| panic!("{wire}: {error}"));

        // The person was shown the real content, and it is in the file.
        let details = shown.lock().unwrap().clone();
        assert_eq!(details.len(), 1, "{wire}");
        assert!(
            details[0].contains(&format!("Contact {EMAIL} today.")) && !details[0].contains("pv1-"),
            "{wire}"
        );
        assert_eq!(
            std::fs::read_to_string(&note).unwrap(),
            format!("Contact {EMAIL} today."),
            "{wire}"
        );
        written += 1;
        // The recorded call, result and reply are the canonical ones: the
        // call and the reply hold the placeholder, and the result is the
        // tool's own text.
        let messages = session.agent.state.lock().unwrap().messages.clone();
        assert_eq!(messages.len(), 4, "{wire}");
        assert!(
            matches!(&messages[1], Msg::Assistant { tool_calls, .. }
                if tool_calls[0].args["content"].as_str().unwrap().contains("pv1-")
                    && !tool_calls[0].args.to_string().contains(EMAIL)),
            "{wire}"
        );
        assert!(
            matches!(&messages[2], Msg::Tool { content, is_error: false, .. }
                if *content == format!("Updated file {}", note.display())),
            "{wire}"
        );
        assert!(
            matches!(&messages[3], Msg::Assistant { content, .. }
                if content.contains("pv1-") && !content.contains(EMAIL)),
            "{wire}"
        );
        // No request holds the value.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2, "{wire}");
        for request in &requests {
            let raw = String::from_utf8(request.body.clone()).unwrap();
            assert!(!raw.contains(EMAIL), "{wire}");
        }
        // The person reads the reply with the value, from the committed
        // reply's own source.
        let sources = session.agent.recorded_sources().unwrap();
        let human = owner.human_reply(sources[3].as_ref().unwrap()).unwrap();
        assert_eq!(
            human.read(str::to_owned).unwrap(),
            format!("Saved it for {EMAIL}."),
            "{wire}"
        );
        assert_eq!(human.restored(), 1, "{wire}");
        assert!(owner.check().is_ok(), "{wire}");
        session.lsp.shutdown().await;
    }
    println!(
        "PRIVACY-RESTORE-WIRES:COMPLETE wires={} files={written}",
        WIRES.len()
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
        "restore wires fixture",
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
async fn an_unmarked_restore_wires_fixture_returns_before_any_build_or_configuration() {
    let home = tempfile::tempdir().unwrap();
    for marker in [None, Some("collect")] {
        let (status, stdout, _) = run_helper(marker, home.path()).await;
        assert!(status.success());
        assert!(stdout.contains("PRIVACY-RESTORE-WIRES:NOT-SELECTED"));
        assert!(!stdout.contains("COMPLETE"));
    }
    // Nothing was created in the isolated home.
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn four_wires_restore_a_reply_for_a_person_and_real_content_into_a_file() {
    let home = tempfile::tempdir().unwrap();
    let (status, stdout, stderr) = run_helper(Some("suite"), home.path()).await;
    assert!(
        status.success(),
        "restore wires fixture failed: {stdout} {stderr}"
    );
    assert!(stdout.contains("PRIVACY-RESTORE-WIRES:COMPLETE wires=4 files=4"));
    assert!(!stdout.contains("NOT-SELECTED"));
}
