//! Marked native fixture: restoration on each model wire, through the shared
//! session builder, with the source-built detector approved and pinned at
//! its user-level source, a real vault and guard and a loopback server.
//!
//! On Chat, Responses, Claude Messages and Gemini a person approves a file
//! write after being shown its real content, the file gets that content, and
//! the person reads the reply with the value. The model, the record and
//! every request keep the placeholder. The same holds for a file a delegated
//! agent writes behind its own gate, and a frontend that shows replies is
//! given each committed reply once.
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
pub(super) const HELPER: &str =
    "gadgets::privacy_detector_tests::restore_wires::restore_wires_native_fixture";
pub(super) const MARKER: &str = "craftsman-privacy-restore-wires=";
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

/// One concrete contact-data declaration for the owned fixture target, with
/// the actual workspace directory retained before any model proposal.
struct PlainTextFiles {
    destination: PathBuf,
    directory: std::fs::File,
}

impl PlainTextFiles {
    fn at(workspace: &Path, destination: &Path) -> Self {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        Self {
            destination: destination.to_owned(),
            directory: options.open(workspace).unwrap(),
        }
    }
    fn names(&self, destination: &Path) -> bool {
        if destination != self.destination {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let Some(parent) = destination.parent() else {
                return false;
            };
            let (Ok(named), Ok(held)) = (std::fs::metadata(parent), self.directory.metadata())
            else {
                return false;
            };
            named.dev() == held.dev() && named.ino() == held.ino()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

impl CapturedUserPolicy for PlainTextFiles {
    fn precheck_file_target(
        &self,
        request: &crate::privacy_session::FileTargetRequest,
    ) -> Option<crate::privacy_session::FileTargetPrecheck> {
        if !self.names(request.destination()) {
            return None;
        }
        request
            .check_application(crate::privacy_session::SupportedDataApplication::ContactLinesV1)
            .ok()
    }
    fn supported_file_data(
        &self,
        request: &crate::privacy_session::FileDataRequest,
    ) -> Option<crate::privacy_session::SupportedFileData> {
        if !self.names(request.destination()) {
            return None;
        }
        request
            .validate_application(crate::privacy_session::SupportedDataApplication::ContactLinesV1)
            .ok()
    }
    fn retained_file_effect(
        &self,
        request: &crate::privacy_session::FileEffectRequest,
    ) -> Option<crate::privacy_session::RetainedFileEffect> {
        self.supported_file_data(request.data())?;
        request.bind_retained_parent(&self.directory).ok()
    }
    fn enables_privacy_in(&self, _: &str) -> bool {
        true
    }

    fn requires_inspectable_provider_items(&self) -> bool {
        false
    }

    fn restorable_file_data(&self, _: &str, _: &Path) -> Option<FileDataFormat> {
        Some(FileDataFormat::PlainText)
    }
}

fn bound_fixture_ui(read: ApprovalPrompt) -> Arc<crate::privacy_entry::PrivacyApprovalPrompt> {
    let slot: Arc<Mutex<Option<Arc<crate::privacy_entry::PrivacyApprovalUi>>>> =
        Arc::new(Mutex::new(None));
    let receiver = slot.clone();
    let (prompt, ui) =
        crate::privacy_entry::register_privacy_approval_ui(Arc::new(move |request| {
            let ui = receiver.lock().unwrap().clone()?;
            let presented = ui.present(&request).ok()?;
            let view = presented.review();
            let choice = match read(&view.call.name, view.summary, view.details) {
                ApprovalPromptResult::Approve => crate::privacy_entry::HumanChoice::Approve,
                ApprovalPromptResult::ApproveAlways => {
                    crate::privacy_entry::HumanChoice::ApproveAlways
                }
                ApprovalPromptResult::Reject(feedback) => {
                    crate::privacy_entry::HumanChoice::Deny(feedback)
                }
            };
            ui.decide(presented, choice).ok()
        }));
    *slot.lock().unwrap() = Some(ui);
    prompt
}

/// A reply made from the placeholder the request carries for the address in
/// its input.
struct FromInput(Box<dyn Fn(&str) -> wiremock::ResponseTemplate + Send + Sync>);

impl wiremock::Respond for FromInput {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        (self.0)(&between(request, "Who handles ", "?"))
    }
}

/// A delegated agent's reply, made from the placeholder its task carries.
struct FromTask(Box<dyn Fn(&str) -> wiremock::ResponseTemplate + Send + Sync>);

impl wiremock::Respond for FromTask {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        (self.0)(&between(request, "Jot-down: ", " ;end"))
    }
}

/// What stands between `open` and `close` in a request.
fn between(request: &wiremock::Request, open: &str, close: &str) -> String {
    let raw = String::from_utf8_lossy(&request.body);
    raw.split(open)
        .nth(1)
        .and_then(|rest| rest.split(close).next())
        .expect("the request carries the input")
        .to_owned()
}

/// Mounts the reply to the request at `index` of a server, from the first.
async fn nth(server: &MockServer, index: u8, reply: impl wiremock::Respond + 'static) {
    Mock::given(method("POST"))
        .respond_with(reply)
        .up_to_n_times(1)
        .with_priority(index + 1)
        .mount(server)
        .await;
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

    let (mut written, mut delegated, mut shown_replies) = (0, 0, 0);
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
                    PrivacyViewSelection::from_user_policy(
                        Arc::new(PlainTextFiles::at(&workspace, &note)),
                        owner.clone(),
                    )
                    .unwrap(),
                ),
                privacy_approval_prompt: Some(bound_fixture_ui(prompt.clone())),
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

        // A second conversation on the same wire: the root agent delegates,
        // and the delegated agent writes the file behind its own gate.
        let note = workspace.join(format!("delegated-{wire}.txt"));
        let server = MockServer::start().await;
        nth(
            &server,
            0,
            FromInput(Box::new(move |placeholder| {
                let task = json!({"description": format!("Jot-down: {placeholder} ;end"),
                    "subagent_type": "general-purpose"});
                reply(wire, 0, Some(("wire-task", "task", &task)), "")
            })),
        )
        .await;
        let path = note.clone();
        nth(
            &server,
            1,
            FromTask(Box::new(move |placeholder| {
                let write =
                    json!({"file_path": path, "content": format!("Contact delegated {placeholder}.")});
                reply(wire, 1, Some(("wire-note", "write_file", &write)), "")
            })),
        )
        .await;
        nth(&server, 2, reply(wire, 1, None, "Noted.")).await;
        nth(
            &server,
            3,
            FromInput(Box::new(move |placeholder| {
                reply(wire, 1, None, &format!("Done for {placeholder}."))
            })),
        )
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
        let shown: Arc<Mutex<Vec<String>>> = Arc::default();
        let prompt: ApprovalPrompt = {
            let shown = shown.clone();
            Arc::new(move |_, _, details| {
                shown.lock().unwrap().push(details.to_owned());
                ApprovalPromptResult::Approve
            })
        };
        // What a frontend that shows replies is given, and what reached the
        // token sink it would otherwise draw from.
        let read: Arc<Mutex<Vec<String>>> = Arc::default();
        let streamed: Arc<Mutex<String>> = Arc::default();
        let (reader, stream) = (read.clone(), streamed.clone());
        let session = build_session_ext(
            &cfg,
            Arc::new(Mutex::new(ApprovalState::default())),
            "",
            false,
            false,
            None,
            SessionExtras {
                privacy_view: Some(
                    PrivacyViewSelection::from_user_policy(
                        Arc::new(PlainTextFiles::at(&workspace, &note)),
                        owner.clone(),
                    )
                    .unwrap(),
                ),
                privacy_approval_prompt: Some(bound_fixture_ui(prompt.clone())),
                approval_prompt: Some(prompt),
                workspace_root: Some(workspace.clone()),
                token_sink: Some(Arc::new(move |text| stream.lock().unwrap().push_str(text))),
                human_reply_sink: Some(Arc::new(move |reply| {
                    if let crate::privacy_session::HumanReply::Text(text) = reply {
                        reader
                            .lock()
                            .unwrap()
                            .push(text.read(str::to_owned).unwrap());
                    }
                })),
                ..Default::default()
            },
        )
        .await
        .unwrap_or_else(|| panic!("{wire}: a session with delegated agents is built"));
        let view = session.privacy_view.as_ref().unwrap();
        view.turn(&session.agent, &format!("Who handles {EMAIL}?"))
            .await
            .unwrap_or_else(|error| panic!("{wire}: {error}"));
        // The delegated agent's gate showed the real content once, and the
        // file holds it.
        let details = shown.lock().unwrap().clone();
        assert_eq!(details.len(), 1, "{wire}");
        assert!(
            details[0].contains(&format!("Contact delegated {EMAIL}."))
                && !details[0].contains("pv1-"),
            "{wire}"
        );
        assert_eq!(
            std::fs::read_to_string(&note).unwrap(),
            format!("Contact delegated {EMAIL}."),
            "{wire}"
        );
        delegated += 1;
        // The frontend was given the root agent's committed reply once, as a
        // person may read it, and nothing reached its token sink.
        assert_eq!(
            *read.lock().unwrap(),
            [format!("Done for {EMAIL}.")],
            "{wire}"
        );
        assert!(streamed.lock().unwrap().is_empty(), "{wire}");
        shown_replies += 1;
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4, "{wire}");
        for request in &requests {
            let raw = String::from_utf8(request.body.clone()).unwrap();
            assert!(!raw.contains(EMAIL), "{wire}");
        }
        assert!(owner.check().is_ok(), "{wire}");
        session.lsp.shutdown().await;
    }
    println!(
        "PRIVACY-RESTORE-WIRES:COMPLETE wires={} files={written} delegated={delegated} shown={shown_replies}",
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
    assert!(stdout.contains("PRIVACY-RESTORE-WIRES:COMPLETE wires=4 files=4 delegated=4 shown=4"));
    assert!(!stdout.contains("NOT-SELECTED"));
}
