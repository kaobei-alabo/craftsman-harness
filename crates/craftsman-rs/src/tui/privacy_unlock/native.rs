//! Marked, newly owned PTYs. Synthetic passwords are sent only through the PTY,
//! never argv/environment. No real user config, terminal or provider is used.
use super::OwnedSecretTerminal;
#[cfg(feature = "gadgets")]
use crate::privacy_bootstrap::{BootstrapError, PrivateBootstrap};
use crate::privacy_cli::{FilePresence, PrivateKeyOperations};
use crate::privacy_key::{ENVELOPE_BYTES, KeyError};
use base64::Engine;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::unix::AsyncFd;
use tokio::sync::Notify;
#[cfg(feature = "gadgets")]
mod entry_tests;

const HELPER: &str = "tui::privacy_unlock::native::private_key_ui_fixture";
const MARKER: &str = "craftsman-private-key-ui=";
const SECRET: &str = "  owned-password-e\u{301}-🔐  ";
const CONFIRMATION: &str = "  owned-password-é-🔐  ";
const WRONG: &str = "different-owned-password";
const COMPLETE: &str = "PRIVATE-KEY-UI:COMPLETE";
const LIMIT: Duration = Duration::from_secs(60);
const CAPTURE: usize = 1024 * 1024;

// One runtime/block_on implementation for new fixture futures. Type erasure
// avoids duplicating its generic runtime scaffolding in the app test binary;
// it changes no existing test, deadline, compiler option or limit.
fn run_owned(future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("owned secret fixture runtime");
    // Match the erased type the locked Tokio test macro gives block_on.
    // Each invocation still owns its independent two-thread runtime. Keep
    // its context through future Drop, including unwind, and drop the pinned
    // adapter before this runtime; no reader or key work is detached.
    let _context = runtime.enter();
    let body = async move { future.await };
    tokio::pin!(body);
    let body: std::pin::Pin<&mut dyn std::future::Future<Output = ()>> = body;
    runtime.block_on(body);
}

#[derive(Clone, Copy)]
enum Case {
    Init,
    TailInit,
    Unlock,
    TailUnlock,
    Cancel,
    TailCancel,
    Mismatch,
    TailMismatch,
    TailInvalidUtf8,
    TailInvalidPaste,
    Wrong,
    TailWrong,
    PartialInit,
    MissingExisting,
    Builder,
    #[cfg(feature = "gadgets")]
    Conversation,
    #[cfg(feature = "gadgets")]
    Unapproved,
    #[cfg(feature = "gadgets")]
    FullEntry,
}
impl Case {
    fn name(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::TailInit => "tail-init",
            Self::Unlock => "unlock",
            Self::TailUnlock => "tail-unlock",
            Self::Cancel => "cancel",
            Self::TailCancel => "tail-cancel",
            Self::Mismatch => "mismatch",
            Self::TailMismatch => "tail-mismatch",
            Self::TailInvalidUtf8 => "tail-invalid-utf8",
            Self::TailInvalidPaste => "tail-invalid-paste",
            Self::Wrong => "wrong",
            Self::TailWrong => "tail-wrong",
            Self::PartialInit => "partial-init",
            Self::MissingExisting => "missing-existing",
            Self::Builder => "builder",
            #[cfg(feature = "gadgets")]
            Self::Conversation => "conversation",
            #[cfg(feature = "gadgets")]
            Self::Unapproved => "unapproved",
            #[cfg(feature = "gadgets")]
            Self::FullEntry => "full-entry",
        }
    }
    fn selected(arguments: impl Iterator<Item = std::ffi::OsString>) -> Option<Self> {
        let arguments: Vec<_> = arguments.collect();
        if [HELPER, "--exact", "--ignored"].iter().any(|required| {
            arguments
                .iter()
                .filter(|argument| argument.as_os_str() == std::ffi::OsStr::new(required))
                .count()
                != 1
        }) {
            return None;
        }
        let markers: Vec<_> = arguments
            .into_iter()
            .filter(|argument| argument.as_encoded_bytes().starts_with(MARKER.as_bytes()))
            .collect();
        let [argument] = markers.as_slice() else {
            return None;
        };
        Some(match argument.to_str()?.strip_prefix(MARKER)? {
            "init" => Self::Init,
            "tail-init" => Self::TailInit,
            "unlock" => Self::Unlock,
            "tail-unlock" => Self::TailUnlock,
            "cancel" => Self::Cancel,
            "tail-cancel" => Self::TailCancel,
            "mismatch" => Self::Mismatch,
            "tail-mismatch" => Self::TailMismatch,
            "tail-invalid-utf8" => Self::TailInvalidUtf8,
            "tail-invalid-paste" => Self::TailInvalidPaste,
            "wrong" => Self::Wrong,
            "tail-wrong" => Self::TailWrong,
            "partial-init" => Self::PartialInit,
            "missing-existing" => Self::MissingExisting,
            "builder" => Self::Builder,
            #[cfg(feature = "gadgets")]
            "conversation" => Self::Conversation,
            #[cfg(feature = "gadgets")]
            "unapproved" => Self::Unapproved,
            #[cfg(feature = "gadgets")]
            "full-entry" => Self::FullEntry,
            _ => return None,
        })
    }
}

#[test]
#[ignore = "owned secret PTY; exact registered argv marker required before env/config/stdin"]
fn private_key_ui_fixture() {
    let Some(case) = Case::selected(std::env::args_os()) else {
        return;
    };
    run_owned(Box::pin(private_key_ui_child(case)));
}

async fn private_key_ui_child(case: Case) {
    #[cfg(feature = "gadgets")]
    if matches!(case, Case::FullEntry) {
        entry_tests::run_child().await;
        return;
    }
    // Every environment, path, configuration and terminal access is AFTER the
    // complete marker decision. Unmarked/unknown/duplicate startup is inert.
    let config = crate::config::config_dir();
    let workspace = std::env::current_dir().unwrap();
    let before = modes(0);
    let mut app = crate::tui::app::App::new(&crate::config::Config::default());
    app.input = "unrelated composer draft".into();
    app.history_draft = Some("unrelated history draft".into());
    app.history = vec!["previous accepted human input".into()];
    app.voice.tentative = "unrelated voice draft".into();
    app.voice.committed = "unrelated committed voice".into();
    app.selection_anchor = Some(2);
    app.copy_requested = true;
    let operations = PrivateKeyOperations::capture_actual().unwrap();
    let presence = operations.status().unwrap();
    let mut terminal = OwnedSecretTerminal::enter().unwrap();
    match case {
        #[cfg(feature = "gadgets")]
        Case::FullEntry => unreachable!("full entry dispatched before ordinary popup fixture"),
        Case::Init | Case::TailInit => {
            assert_eq!(presence, FilePresence::Missing);
            let unlocked = terminal.initialize(&operations).await.unwrap();
            assert!(unlocked.key_source().acquire_privacy_key().is_ok());
            unlocked.close();
            assert_eq!(
                std::fs::read(config.join("privacy-key.v1")).unwrap().len(),
                ENVELOPE_BYTES
            );
        }
        Case::Unlock | Case::TailUnlock | Case::Builder => {
            assert_eq!(presence, FilePresence::FilesPresent);
            let unlocked = terminal.unlock(&operations).await.unwrap();
            assert!(unlocked.key_source().acquire_privacy_key().is_ok());
            if matches!(case, Case::Builder) {
                // A full ordinary builder AFTER the popup, with no privacy
                // selection, proves secret-input isolation. This is explicitly
                // NOT the unavailable full-set private Agent bootstrap.
                ordinary_builder_without_password(&workspace).await;
            }
            unlocked.close();
        }
        Case::Cancel | Case::TailCancel => {
            assert!(matches!(
                terminal.initialize(&operations).await,
                Err(KeyError::Cancelled)
            ));
            assert!(!config.join("privacy-key.v1").exists());
            assert!(!config.join("privacy-vault.db").exists());
        }
        Case::Mismatch | Case::TailMismatch | Case::TailInvalidUtf8 | Case::TailInvalidPaste => {
            assert!(matches!(
                terminal.initialize(&operations).await,
                Err(KeyError::Passphrase)
            ));
            assert!(!config.join("privacy-key.v1").exists());
            assert!(!config.join("privacy-vault.db").exists());
        }
        Case::Wrong | Case::TailWrong => {
            assert!(matches!(
                terminal.unlock(&operations).await,
                Err(KeyError::Unlock)
            ));
        }
        Case::PartialInit => {
            assert!(matches!(
                terminal.initialize(&operations).await,
                Err(KeyError::AlreadyInitialized)
            ));
            assert!(!config.join("privacy-key.v1").exists());
            assert!(!config.join("privacy-vault.db").exists());
        }
        Case::MissingExisting => {
            assert_eq!(presence, FilePresence::Partial);
            assert!(matches!(
                terminal.unlock(&operations).await,
                Err(KeyError::Vault)
            ));
            assert!(!config.join("privacy-vault.db").exists());
        }
        #[cfg(feature = "gadgets")]
        Case::Conversation | Case::Unapproved => {
            #[cfg(feature = "gadgets")]
            {
                let bootstrap = PrivateBootstrap::capture_actual(&workspace).unwrap();
                let mut conversation = bootstrap.unlock_private(&mut terminal).await.unwrap();
                let typed = "中文 typed alice@example.invalid and +886 912 345 678.";
                if matches!(case, Case::Unapproved) {
                    assert!(matches!(
                        conversation.admit_typed(typed).await,
                        Err(BootstrapError::Admission(
                            crate::privacy_session::AdmitError::Stopped(
                                crate::privacy_session::SessionStop::Detector(
                                    crate::privacy_hook::HookError::Source
                                )
                            )
                        ))
                    ));
                    assert!(conversation.check_sources().is_err());
                } else {
                    let node = conversation.admit_typed(typed).await.unwrap();
                    assert_eq!(node.replacements(), 2);
                    let view = node.model_view().unwrap();
                    assert!(!view.contains("alice@example.invalid"));
                    assert!(!view.contains("+886 912 345 678"));
                    assert_eq!(
                        typed,
                        "中文 typed alice@example.invalid and +886 912 345 678."
                    );
                    node.with_records(|scope, records| {
                        assert!(records.validate_session_binding(scope).is_ok());
                        assert_eq!(records.len().unwrap(), 2);
                    })
                    .unwrap();
                    let old_id = conversation.thread_id().to_owned();
                    conversation.clear_before_new_build().await.unwrap();
                    assert_ne!(old_id, conversation.thread_id());
                    assert!(node.model_view().is_err());
                    assert_eq!(
                        conversation
                            .admit_typed(typed)
                            .await
                            .unwrap()
                            .replacements(),
                        2
                    );
                    let conversation = Arc::new(conversation);
                    let selection = conversation.selection().unwrap();
                    assert!(selection.report().refusal().is_none());
                    use crate::privacy_session::CapturedUserPolicy;
                    assert!(conversation.requires_inspectable_provider_items());
                    assert!(!conversation.enables_privacy_in(&"0".repeat(64)));
                    let server = wiremock::MockServer::start().await;
                    let cfg=serde_json::from_value(serde_json::json!({
                        "base_url":server.uri(),"api_key":"fixture-key","model":"fixture-model",
                        "voice":{"enabled":false},"gadgets":{"enabled":false},"mcp_servers":[],"lsp_servers":{}
                    })).unwrap();
                    assert!(matches!(
                        terminal
                            .request_full_session(
                                &conversation,
                                &cfg,
                                Arc::new(Mutex::new(crate::hitl::ApprovalState::default()))
                            )
                            .await,
                        Err(crate::privacy_bootstrap::PrivateBuildError::Refused(
                            crate::privacy_session::BuildRefusal::RequestedPart
                        ))
                    ));
                    assert!(server.received_requests().await.unwrap().is_empty());
                    assert_eq!(terminal.pending_ui_events(), 0);
                    assert!(conversation.check_sources().is_ok());
                    conversation.close_before_rebind();
                    assert!(conversation.check_sources().is_err());
                }
            }
        }
    }
    drop(terminal);
    if matches!(
        case,
        Case::TailInit
            | Case::TailUnlock
            | Case::TailCancel
            | Case::TailWrong
            | Case::TailMismatch
            | Case::TailInvalidUtf8
            | Case::TailInvalidPaste
    ) {
        let mut next_reader = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&mut next_reader, 1, 0) },
            0,
            "no queued secret input may survive the owned popup for a next reader"
        );
        eprintln!("PRIVATE-KEY-TAIL-EMPTY");
    }
    let restored = modes(0);
    assert_modes(&before, &restored);
    assert_eq!(app.input, "unrelated composer draft");
    assert_eq!(
        app.history_draft.as_deref(),
        Some("unrelated history draft")
    );
    assert_eq!(app.history, ["previous accepted human input"]);
    assert_eq!(app.voice.tentative, "unrelated voice draft");
    assert_eq!(app.voice.committed, "unrelated committed voice");
    assert_eq!(app.selection_anchor, Some(2));
    assert!(app.copy_requested);
    assert!(!app.submit_requested && !app.cancel_requested && !app.running);
    assert!(
        app.transcript
            .iter()
            .all(|entry| !entry.text.contains("owned-password"))
    );
    absent_secrets(&config);
    eprintln!("PRIVATE-KEY-UI-RESTORED:{}", mode_data(&restored));
    eprintln!("{COMPLETE}");
}

async fn ordinary_builder_without_password(workspace: &Path) {
    use crate::agent::{SessionExtras, build_session_ext};
    use craftsman_harness::{Checkpointer, Msg, TurnResult};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "data: {\"choices\":[{\"delta\":{\"content\":\"owned UI isolation reply\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
            "text/event-stream",
        )).mount(&server).await;
    let cfg = serde_json::from_value(serde_json::json!({
        "base_url":server.uri(),"api_key":"fixture-key","model":"fixture-model",
        "harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},
        "mcp_servers":[],"lsp_servers":{}
    }))
    .unwrap();
    let build: std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<crate::agent::Session>> + Send + '_>,
    > = Box::pin(build_session_ext(
        &cfg,
        Arc::new(Mutex::new(crate::hitl::ApprovalState::default())),
        "",
        false,
        false,
        None,
        SessionExtras {
            workspace_root: Some(workspace.to_owned()),
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(|_| {})),
            ..Default::default()
        },
    ));
    let session = build
        .await
        .expect("the unchanged full ordinary builder constructs");
    assert!(session.privacy_view.is_none() && session.send_guard.is_none());
    let result = session
        .agent
        .run_turn("ordinary post-popup input")
        .await
        .unwrap();
    assert!(matches!(result, TurnResult::Done(_)));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_absent(&requests[0].body);
    let messages = session.agent.state.lock().unwrap().messages.clone();
    assert_eq!(messages[0], Msg::user("ordinary post-popup input"));
    assert_absent(&serde_json::to_vec(&messages).unwrap());
    let checkpoint = session
        .checkpointer
        .as_ref()
        .expect("actual checkpoint store");
    let (_, state, _) = checkpoint
        .load_latest(&session.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.messages[0], Msg::user("ordinary post-popup input"));
    assert_absent(&serde_json::to_vec(&state).unwrap());
    assert!(session.registry.list().is_empty());
    session.lsp.shutdown().await;
}

fn assert_absent(bytes: &[u8]) {
    for secret in [SECRET, CONFIRMATION, WRONG, "owned-password"] {
        assert!(
            !bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()),
            "synthetic password must remain absent from this capture"
        );
    }
}
fn absent_secrets(directory: &Path) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let metadata = entry.metadata().unwrap();
        if metadata.is_dir() {
            absent_secrets(&entry.path());
        } else if metadata.is_file() && metadata.len() <= 4 * 1024 * 1024 {
            assert_absent(&std::fs::read(entry.path()).unwrap());
        }
    }
}

fn modes(fd: i32) -> libc::termios {
    let mut mode = unsafe { std::mem::zeroed::<libc::termios>() };
    assert_eq!(unsafe { libc::tcgetattr(fd, &mut mode) }, 0);
    mode
}
fn assert_modes(before: &libc::termios, after: &libc::termios) {
    #[cfg(target_os = "macos")]
    let local_mask = !libc::PENDIN; // kernel transient, raw observations retained
    #[cfg(not(target_os = "macos"))]
    let local_mask = !0;
    assert_eq!(before.c_lflag & local_mask, after.c_lflag & local_mask);
    assert_eq!(before.c_iflag, after.c_iflag);
    assert_eq!(before.c_oflag, after.c_oflag);
    assert_eq!(before.c_cflag, after.c_cflag);
    assert_eq!(before.c_cc, after.c_cc);
    assert_eq!(unsafe { libc::cfgetispeed(before) }, unsafe {
        libc::cfgetispeed(after)
    });
    assert_eq!(unsafe { libc::cfgetospeed(before) }, unsafe {
        libc::cfgetospeed(after)
    });
}

fn mode_data(mode: &libc::termios) -> serde_json::Value {
    serde_json::json!({
        "local":mode.c_lflag as u64,"input":mode.c_iflag as u64,
        "output":mode.c_oflag as u64,"control":mode.c_cflag as u64,
        "cc":mode.c_cc.to_vec(),
        "input_speed":unsafe {libc::cfgetispeed(mode)} as u64,
        "output_speed":unsafe {libc::cfgetospeed(mode)} as u64,
    })
}

// This is a bounded observation of the owned fixture screen, not rewriting
// its capture. Ratatui's differential draws can retain part of a word and
// position subsequent letters separately, so literal stdout is not screen text.
fn screen(bytes: &[u8]) -> Option<String> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    let mut grid = [[' '; 110]; 16];
    let (mut row, mut col) = (0usize, 0usize);
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        match character {
            '\x1b' => {
                if chars.next()? != '[' {
                    return None;
                }
                let mut body = String::new();
                let command = loop {
                    let next = chars.next()?;
                    if ('@'..='~').contains(&next) {
                        break next;
                    }
                    if body.len() >= 32 {
                        return None;
                    }
                    body.push(next);
                };
                let parameters = || {
                    body.split(';')
                        .map(|n| n.parse::<usize>().unwrap_or(1))
                        .collect::<Vec<_>>()
                };
                match command {
                    'H' | 'f' => {
                        let args = parameters();
                        row = args.first().copied().unwrap_or(1).saturating_sub(1).min(15);
                        col = args.get(1).copied().unwrap_or(1).saturating_sub(1).min(109);
                    }
                    'J' if body == "2" || body == "3" => grid = [[' '; 110]; 16],
                    'h' if body == "?1049" => {
                        grid = [[' '; 110]; 16];
                        row = 0;
                        col = 0;
                    }
                    'K' => match body.as_str() {
                        "2" => grid[row] = [' '; 110],
                        "1" => grid[row][..=col].fill(' '),
                        _ => grid[row][col..].fill(' '),
                    },
                    'm' | 'h' | 'l' => {}
                    _ => return None,
                }
            }
            '\r' => col = 0,
            '\n' => {
                row = (row + 1).min(15);
                col = 0;
            }
            character if character.is_control() => {}
            character => {
                grid[row][col] = character;
                col =
                    (col + unicode_width::UnicodeWidthChar::width(character).unwrap_or(0)).min(109);
            }
        }
    }
    Some(
        grid.into_iter()
            .map(|row| row.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

struct Home {
    _owner: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    workspace: PathBuf,
}
impl Home {
    fn new() -> Self {
        let owner = tempfile::tempdir().unwrap();
        std::fs::set_permissions(owner.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = owner.path().canonicalize().unwrap();
        let config = root.join("private-config");
        let workspace = root.join("workspace");
        std::fs::create_dir(&config).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(&workspace).unwrap();
        Self {
            _owner: owner,
            root,
            config,
            workspace,
        }
    }
}

struct Pty {
    child: tokio::process::Child,
    stream: Arc<AsyncFd<std::fs::File>>,
    slave: Option<std::fs::File>,
    reader: Option<tokio::task::JoinHandle<()>>,
    reader_end: Arc<Mutex<Option<i32>>>,
    output: Arc<Mutex<Vec<u8>>>,
    changed: Arc<Notify>,
    seen: usize,
    original: libc::termios,
    case: Case,
}
impl Drop for Pty {
    fn drop(&mut self) {
        if let Some(reader) = &self.reader {
            reader.abort();
        }
        let _ = self.child.start_kill();
    }
}
impl Pty {
    fn start(home: &Home, case: Case) -> Self {
        let (mut master, mut slave) = (-1, -1);
        let mut size = libc::winsize {
            ws_row: 16,
            ws_col: 110,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut size,
                )
            },
            0
        );
        let master = unsafe { std::fs::File::from_raw_fd(master) };
        let slave = unsafe { std::fs::File::from_raw_fd(slave) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        assert_eq!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) },
            0
        );
        let original = modes(slave.as_raw_fd());
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                HELPER,
                "--test-threads=1",
                "--nocapture",
                "--skip",
            ])
            .arg(format!("{MARKER}{}", case.name()))
            .current_dir(&home.workspace)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("TERM", "xterm-256color")
            .env("CRAFTSMAN_CONFIG_DIR", &home.config)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()))
            .kill_on_drop(true);
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        let stream = Arc::new(AsyncFd::new(master).unwrap());
        let output = Arc::new(Mutex::new(Vec::new()));
        let changed = Arc::new(Notify::new());
        let read_stream = stream.clone();
        let read_output = output.clone();
        let read_changed = changed.clone();
        let reader_end = Arc::new(Mutex::new(None));
        let returned = reader_end.clone();
        let read: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            Box::pin(async move {
                let mut buffer = [0; 4096];
                let end = loop {
                    let mut ready = read_stream.readable().await.unwrap();
                    let read = ready.try_io(|fd| {
                        let count = unsafe {
                            libc::read(
                                fd.get_ref().as_raw_fd(),
                                buffer.as_mut_ptr().cast(),
                                buffer.len(),
                            )
                        };
                        if count < 0 {
                            Err(io::Error::last_os_error())
                        } else {
                            Ok(count as usize)
                        }
                    });
                    match read {
                        Ok(Ok(0)) => break 0,
                        Ok(Err(error)) => break error.raw_os_error().unwrap_or(-1),
                        Ok(Ok(count)) => {
                            let mut output = read_output.lock().unwrap();
                            assert!(
                                output
                                    .len()
                                    .checked_add(count)
                                    .is_some_and(|n| n <= CAPTURE)
                            );
                            output.extend_from_slice(&buffer[..count]);
                            drop(output);
                            read_changed.notify_one();
                        }
                        Err(_) => {}
                    }
                };
                *returned.lock().unwrap() = Some(end);
            });
        let reader = tokio::spawn(read);
        Self {
            child,
            stream,
            slave: Some(slave),
            reader: Some(reader),
            reader_end,
            output,
            changed,
            seen: 0,
            original,
            case,
        }
    }
    async fn expect(&mut self, text: &str) {
        let found = tokio::time::timeout(LIMIT, async {
            loop {
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let found = {
                    let bytes = self.output.lock().unwrap();
                    if text == COMPLETE {
                        bytes[self.seen..]
                            .windows(text.len())
                            .position(|window| window == text.as_bytes())
                            .map(|offset| self.seen + offset + text.len())
                    } else if screen(&bytes).is_some_and(|screen| screen.contains(text)) {
                        Some(bytes.len())
                    } else {
                        None
                    }
                };
                if let Some(end) = found {
                    self.seen = end;
                    return true;
                }
                tokio::select! {
                    _ = notified => {},
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {
                        if self.child.try_wait().unwrap().is_some() {
                            return false;
                        }
                    }
                }
            }
        })
        .await;
        if !matches!(found, Ok(true)) {
            let bytes = self.output.lock().unwrap().clone();
            assert_absent(&bytes);
            eprintln!(
                "PRIVATE-KEY-PTY-FIRST-STAGE-ERROR:{}",
                serde_json::json!({
                    "case":self.case.name(), "expected":text, "bytes":bytes.len(),
                    "output_base64":base64::engine::general_purpose::STANDARD.encode(&bytes),
                    "capture_complete":false
                })
            );
        }
        assert!(
            matches!(found, Ok(true)),
            "the actual owned popup must reach its expected stage"
        );
    }
    async fn send(&self, bytes: &[u8]) {
        let mode = modes(self.slave.as_ref().unwrap().as_raw_fd());
        assert_eq!(mode.c_lflag & (libc::ECHO | libc::ECHONL | libc::ICANON), 0);
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let mut ready = self.stream.writable().await.unwrap();
            match ready.try_io(|fd| {
                let count = unsafe {
                    libc::write(
                        fd.get_ref().as_raw_fd(),
                        remaining.as_ptr().cast(),
                        remaining.len(),
                    )
                };
                if count < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(count as usize)
                }
            }) {
                Ok(Ok(count)) => {
                    assert!(count > 0);
                    remaining = &remaining[count..];
                }
                Ok(Err(_)) => panic!("owned secret PTY write failed"),
                Err(_) => {}
            }
        }
    }
    async fn finish(mut self) {
        let status = tokio::time::timeout(LIMIT, self.child.wait()).await;
        if status.is_err() {
            let _ = self.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
            panic!("owned private input fixture exceeded its bound");
        }
        let status = status.unwrap().unwrap();
        // Darwin can refuse tcgetattr after the child's terminal hangs up.
        // The child captured/asserted ALL fields before last close. Retain that
        // actual record and the parent's independent entry/echo observations.
        drop(self.slave.take());
        tokio::time::timeout(Duration::from_secs(5), self.reader.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        let end = self
            .reader_end
            .lock()
            .unwrap()
            .expect("actual terminal read completion");
        let bytes = self.output.lock().unwrap().clone();
        assert_absent(&bytes);
        if !status.success()
            || !bytes
                .windows(COMPLETE.len())
                .any(|window| window == COMPLETE.as_bytes())
        {
            eprintln!(
                "PRIVATE-KEY-PTY-FIRST-CHILD-ERROR:{}",
                serde_json::json!({
                    "case":self.case.name(),"actual_child_exit":status.code(),
                    "terminal_read_end":end,"capture_complete_until_owned_terminal_close":true,
                    "bytes":bytes.len(),"output_base64":base64::engine::general_purpose::STANDARD.encode(&bytes),
                })
            );
        }
        assert!(status.success(), "owned private input assertions failed");
        assert!(
            bytes
                .windows(COMPLETE.len())
                .any(|window| window == COMPLETE.as_bytes()),
            "owned child must complete its actual assertions"
        );
        let marker = "PRIVATE-KEY-UI-RESTORED:";
        let restored = std::str::from_utf8(&bytes)
            .unwrap()
            .split(marker)
            .nth(1)
            .and_then(|tail| tail.lines().next())
            .unwrap()
            .trim_end();
        let restored: serde_json::Value = serde_json::from_str(restored).unwrap();
        let entry = mode_data(&self.original);
        for key in [
            "input",
            "output",
            "control",
            "cc",
            "input_speed",
            "output_speed",
        ] {
            assert_eq!(entry[key], restored[key]);
        }
        #[cfg(target_os = "macos")]
        let local_mask = !(libc::PENDIN as u64);
        #[cfg(not(target_os = "macos"))]
        let local_mask = !0u64;
        assert_eq!(
            entry["local"].as_u64().unwrap() & local_mask,
            restored["local"].as_u64().unwrap() & local_mask
        );
        assert!(
            end == 0 || end == libc::EIO,
            "actual terminal close must end the bounded capture"
        );
        // Full raw terminal output is encoded only after the absence check.
        // Focused --nocapture logs retain it without replaying terminal controls.
        eprintln!(
            "PRIVATE-KEY-PTY-FACTS:{}",
            serde_json::json!({
                "case":self.case.name(),"bytes":bytes.len(),
                "output_base64":base64::engine::general_purpose::STANDARD.encode(&bytes),
                "entry_modes":entry,"restored_modes":restored,
                "terminal_read_end":end,"capture_complete_until_owned_terminal_close":true,
                "mode_fields_match_under_documented_mask":true,
                "excluded_local_bits":!local_mask,"echo_before_input":false,
                "actual_child_exit":status.code(),
            })
        );
        assert!(status.success(), "owned private input assertions failed");
    }
}

async fn initialize(home: &Home) {
    let mut pty = Pty::start(home, Case::Init);
    pty.expect("Initialize private privacy key").await;
    pty.send(format!("\x1b[200~{SECRET}\x1b[201~\r").as_bytes())
        .await;
    pty.expect("Confirm private passphrase").await;
    // A real backspace removes only the erroneous final scalar.
    pty.send(format!("{CONFIRMATION}x\x7f\r").as_bytes()).await;
    pty.finish().await;
}

#[test]
fn owned_popup_errors_clear_decoder_and_queued_secret_tail() {
    run_owned(Box::pin(async {
        for case in [Case::TailInvalidUtf8, Case::TailInvalidPaste] {
            let home = Home::new();
            let mut pty = Pty::start(&home, case);
            pty.expect("Initialize private privacy key").await;
            let input = match case {
                Case::TailInvalidUtf8 => {
                    let mut input = SECRET.as_bytes().to_vec();
                    input.push(0xff);
                    input.extend_from_slice(format!("{SECRET}\rqueued-error-tail\r").as_bytes());
                    input
                }
                _ => format!("\x1b[200~{SECRET}\n\x1b[201~{SECRET}\rqueued-error-tail\r")
                    .into_bytes(),
            };
            pty.send(&input).await;
            pty.finish().await;
        }
        let home = Home::new();
        let mut pty = Pty::start(&home, Case::TailMismatch);
        pty.expect("Initialize private privacy key").await;
        pty.send(format!("{SECRET}\r{SECRET}\rqueued-first-tail\r").as_bytes())
            .await;
        pty.expect("Confirm private passphrase").await;
        pty.send(format!("{WRONG}\r{SECRET}\rqueued-mismatch-tail\r").as_bytes())
            .await;
        pty.finish().await;
    }));
}

#[test]
fn owned_popup_discards_queued_multiline_and_typeahead_before_the_next_reader() {
    run_owned(Box::pin(async {
        let home = Home::new();
        let mut pty = Pty::start(&home, Case::TailInit);
        pty.expect("Initialize private privacy key").await;
        pty.send(format!("{SECRET}\r{SECRET}\rqueued-tail\r").as_bytes())
            .await;
        pty.expect("Confirm private passphrase").await;
        pty.send(format!("{CONFIRMATION}\r{SECRET}\rqueued-confirmation-tail\r").as_bytes())
            .await;
        pty.finish().await;
        for (case, phrase) in [(Case::TailUnlock, SECRET), (Case::TailWrong, WRONG)] {
            let mut pty = Pty::start(&home, case);
            pty.expect("Unlock private privacy key").await;
            pty.send(format!("{phrase}\r{SECRET}\rqueued-unlock-tail\r").as_bytes())
                .await;
            pty.finish().await;
        }
        for cancel in [b'\x03', b'\x1b'] {
            let home = Home::new();
            let mut pty = Pty::start(&home, Case::TailCancel);
            pty.expect("Initialize private privacy key").await;
            let mut input = format!("{SECRET}").into_bytes();
            input.push(cancel);
            // Escape must be lone; Ctrl-C can have an immediately queued tail.
            if cancel == b'\x03' {
                input.extend_from_slice(format!("{SECRET}\rqueued-cancel-tail\r").as_bytes());
            }
            pty.send(&input).await;
            pty.finish().await;
        }
    }));
}

#[test]
fn owned_pty_real_confirmation_nfc_key_existing_unlock_and_shared_builder_secret_isolation() {
    run_owned(Box::pin(async {
        let home = Home::new();
        initialize(&home).await;
        let original = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
        assert_eq!(original.len(), ENVELOPE_BYTES);
        for case in [Case::Unlock, Case::Builder] {
            let mut pty = Pty::start(&home, case);
            pty.expect("Unlock private privacy key").await;
            pty.send(format!("{SECRET}\r").as_bytes()).await;
            pty.finish().await;
            assert_eq!(
                std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
                original
            );
        }
        absent_secrets(&home.config);
    }));
}

#[test]
fn owned_popup_escape_control_c_signal_and_mismatch_never_create_or_submit_a_draft() {
    run_owned(Box::pin(async {
        for input in [b"\x1b".as_slice(), b"\x03".as_slice()] {
            let home = Home::new();
            let mut pty = Pty::start(&home, Case::Cancel);
            pty.expect("Initialize private privacy key").await;
            pty.send(SECRET.as_bytes()).await;
            pty.send(input).await;
            pty.finish().await;
        }
        let home = Home::new();
        let mut pty = Pty::start(&home, Case::Cancel);
        pty.expect("Initialize private privacy key").await;
        pty.send(SECRET.as_bytes()).await;
        assert_eq!(
            unsafe { libc::kill(pty.child.id().unwrap() as i32, libc::SIGTERM) },
            0
        );
        pty.finish().await;
        let home = Home::new();
        let mut pty = Pty::start(&home, Case::Mismatch);
        pty.expect("Initialize private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("Confirm private passphrase").await;
        pty.send(format!("{WRONG}\r").as_bytes()).await;
        pty.finish().await;
    }));
}

#[test]
fn actual_wrong_password_and_create_only_partial_files_have_no_recovery_or_opt_out() {
    run_owned(Box::pin(async {
        let home = Home::new();
        initialize(&home).await;
        let before = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
        let mut pty = Pty::start(&home, Case::Wrong);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{WRONG}\r").as_bytes()).await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
            before
        );
        let home = Home::new();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(home.config.join("privacy-vault.db-journal"))
            .unwrap();
        file.write_all(b"foreign partial fixture").unwrap();
        let mut pty = Pty::start(&home, Case::PartialInit);
        pty.expect("Initialize private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.expect("Confirm private passphrase").await;
        pty.send(format!("{CONFIRMATION}\r").as_bytes()).await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("privacy-vault.db-journal")).unwrap(),
            b"foreign partial fixture"
        );
        assert!(!home.config.join("privacy-key.v1").exists());
        let home = Home::new();
        initialize(&home).await;
        let envelope = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
        std::fs::remove_file(home.config.join("privacy-vault.db")).unwrap();
        let mut pty = Pty::start(&home, Case::MissingExisting);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.finish().await;
        assert_eq!(
            std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
            envelope
        );
        assert!(!home.config.join("privacy-vault.db").exists());
    }));
}

#[test]
fn unmarked_unknown_and_duplicate_markers_return_before_poisoned_config_or_stdin() {
    run_owned(Box::pin(async {
        use tokio::io::AsyncReadExt;
        let home = Home::new();
        let bad = home.root.join("missing-config");
        for markers in [
            vec![],
            vec![std::ffi::OsString::from(format!("{MARKER}unknown"))],
            vec![std::ffi::OsString::from(format!("{MARKER}init")); 2],
        ] {
            let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--ignored",
                    "--exact",
                    HELPER,
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env_clear()
                .env("CRAFTSMAN_CONFIG_DIR", &bad)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            for marker in markers {
                command.arg("--skip").arg(marker);
            }
            let mut child = command.spawn().unwrap();
            // Keep stdin open and empty: a read would wait and fail this bound.
            let _stdin = child.stdin.take().unwrap();
            let out = child.stdout.take().unwrap();
            let err = child.stderr.take().unwrap();
            let stdout_bytes = Arc::new(Mutex::new(Vec::new()));
            let stderr_bytes = Arc::new(Mutex::new(Vec::new()));
            let captured = stdout_bytes.clone();
            let read_out: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(async move {
                    let mut b = Vec::new();
                    out.take(65537).read_to_end(&mut b).await.unwrap();
                    *captured.lock().unwrap() = b;
                });
            let captured = stderr_bytes.clone();
            let read_err: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(async move {
                    let mut b = Vec::new();
                    err.take(65537).read_to_end(&mut b).await.unwrap();
                    *captured.lock().unwrap() = b;
                });
            let stdout = tokio::spawn(read_out);
            let stderr = tokio::spawn(read_err);
            assert!(
                tokio::time::timeout(Duration::from_secs(10), child.wait())
                    .await
                    .unwrap()
                    .unwrap()
                    .success()
            );
            stdout.await.unwrap();
            stderr.await.unwrap();
            let bytes = [
                stdout_bytes.lock().unwrap().clone(),
                stderr_bytes.lock().unwrap().clone(),
            ]
            .concat();
            assert!(
                !bytes
                    .windows(COMPLETE.len())
                    .any(|w| w == COMPLETE.as_bytes())
            );
            assert!(!bad.exists());
        }
    }));
}

#[test]
fn marker_parser_counts_a_malformed_registered_companion_before_decoding() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let good = OsString::from(format!("{MARKER}init"));
    let malformed = OsString::from_vec([MARKER.as_bytes(), b"\xff"].concat());
    assert!(Case::selected([malformed.clone()].into_iter()).is_none());
    assert!(Case::selected([good, malformed].into_iter()).is_none());
}

#[cfg(feature = "gadgets")]
#[test]
fn real_password_pinned_source_policy_vault_commits_and_new_clear_owner_without_reduced_agent() {
    run_owned(Box::pin(async {
        use crate::gadgets::{GadgetRuntime, RuntimeOptions};
        use sha2::{Digest, Sha256};
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/privacy-detector");
        let home = Home::new();
        let target = home.root.join("source-build");
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
            "the owned pinned detector source build must pass"
        );
        assert!(output.stdout.len() + output.stderr.len() <= 65536);
        eprintln!(
            "PRIVATE-KEY-SOURCE-BUILD:{}",
            base64::engine::general_purpose::STANDARD
                .encode([output.stdout, output.stderr].concat())
        );
        let image =
            std::fs::read(target.join("wasm32-wasip2/release/privacy_detector_fixture.wasm"))
                .unwrap();
        let manifest = std::fs::read(source.join("gadget.toml")).unwrap();
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
        }
        let image_hash = format!("{:x}", Sha256::digest(&image));
        let manifest_hash = format!("{:x}", Sha256::digest(&manifest));
        let user=serde_json::to_vec(&serde_json::json!({"privacy":{"enabled":true,"detector":{
        "gadget":"privacy-detector","function":"detect","component_sha256":image_hash,"manifest_sha256":manifest_hash
    }}})).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(home.config.join("config.json"))
            .unwrap();
        file.write_all(&user).unwrap();
        initialize(&home).await;
        // Pin data alone does not approve anything: actual first input is refused.
        let mut pty = Pty::start(&home, Case::Unapproved);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.finish().await;
        // Use the actual existing source/capability approval path and grant writer,
        // not caller JSON, a hash label, allow-list or privacy fixture Boolean.
        let decisions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = decisions.clone();
        let approved_path = bundle.canonicalize().unwrap().display().to_string();
        let approved_hash = image_hash.clone();
        let runtime = GadgetRuntime::new(
            &home.workspace,
            &home.config,
            RuntimeOptions {
                interactive: true,
                disk_cache: false,
                prompt: Some(Arc::new(move |tool, summary, details| {
                    assert_eq!(tool, "gadget_call");
                    assert!(summary.contains("privacy-detector"));
                    assert!(details.contains(&approved_path) && details.contains(&approved_hash));
                    observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
                crate::privacy_hook::Request::from_text(
                    "中文 typed alice@example.invalid and +886 912 345 678.",
                )
                .unwrap()
                .to_json(),
            )
            .await
            .unwrap();
        assert_eq!(decisions.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(home.config.join("gadgets/.grants.json").is_file());
        drop(runtime);
        // The genuine temp user-root approval now precedes describe/execution.
        let mut pty = Pty::start(&home, Case::Conversation);
        pty.expect("Unlock private privacy key").await;
        pty.send(format!("{SECRET}\r").as_bytes()).await;
        pty.finish().await;
        absent_secrets(&home.config);
    }));
}
