//! Newly owned PTYs exercise the production TUI, status writer and restoration.
use super::*;
use std::os::fd::{AsRawFd, FromRawFd};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::unix::AsyncFd;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "tui::program_status::tests::native::native_status_terminal_fixture";
const MARKER: &str = "craftsman-osc7501-terminal-v1=";

fn terminal_modes() -> Value {
    let mut modes = unsafe { std::mem::zeroed::<libc::termios>() };
    assert_eq!(unsafe { libc::tcgetattr(0, &mut modes) }, 0);
    json!({"local":modes.c_lflag as u64,"input":modes.c_iflag as u64,
        "output":modes.c_oflag as u64,"control":modes.c_cflag as u64})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned status PTY; unmarked/unknown startup returns before terminal/config access"]
async fn native_status_terminal_fixture() {
    let modes: Vec<_> = std::env::args_os()
        .filter_map(|argument| {
            argument
                .to_str()
                .and_then(|argument| argument.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if modes.len() != 1
        || !matches!(
            modes[0].as_str(),
            "terminal" | "disabled" | "error" | "cancel" | "panic"
        )
    {
        return;
    }
    let mode = &modes[0];
    let entry = terminal_modes();
    if mode == "panic" {
        let cancellation = craftsman_harness::SessionCancellation::new();
        let guard = crate::tui::terminal::TerminalGuard::enter(false).unwrap();
        let hook = guard.panic_hook(cancellation.clone());
        guard.enable_program_status(true);
        guard
            .report_program_status(Status::Working, None, false)
            .unwrap();
        let panicked = std::panic::catch_unwind(|| {
            panic!("private-panic-payload\x1b]7501;state=done:app=forged\x1b\\")
        });
        assert!(panicked.is_err());
        assert!(hook.triggered());
        assert!(cancellation.is_cancelled());
        // Restoration sealed the owner: a late update cannot revive working.
        guard
            .report_program_status(Status::Working, None, false)
            .unwrap();
        drop(guard);
        drop(hook);
        std::fs::write(
            "modes.json",
            serde_json::to_vec(&json!({"before":entry,"after":terminal_modes()})).unwrap(),
        )
        .unwrap();
        return;
    }
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let response_mode = mode.clone();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            let round = counter.fetch_add(1, Ordering::SeqCst);
            std::fs::write("request-started", round.to_string()).unwrap();
            if response_mode == "error" {
                return ResponseTemplate::new(500).set_body_string("synthetic failed response");
            }
            let delta = if response_mode == "terminal" {
                match round {
                    0 => json!({"tool_calls":[{"index":0,"id":"pty-edit","type":"function","function":{"name":"edit_file","arguments":json!({"file_path":"note.txt","old_string":"before","new_string":"after"}).to_string()}}]}),
                    1 => json!({"tool_calls":[{"index":0,"id":"pty-ask","type":"function","function":{"name":"ask_user","arguments":json!({"question":"Native fixture question"}).to_string()}}]}),
                    2 => json!({"content":"Native fixture response ready."}),
                    _ => panic!("unexpected additional model request"),
                }
            } else {
                assert_eq!(round, 0);
                json!({"content":"Native fixture response ready.\u{1b}]7501;state=error:app=forged\u{1b}\\"})
            };
            let body = json!({"choices":[{"delta":delta,"finish_reason":if response_mode == "terminal" && round < 2 {"tool_calls"} else {"stop"}}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
            let response = ResponseTemplate::new(200).set_body_raw(format!("data: {body}\n\ndata: [DONE]\n\n"), "text/event-stream");
            if response_mode == "cancel" { response.set_delay(Duration::from_secs(60)) } else { response }
        })
        .mount(&server)
        .await;
    std::fs::write("note.txt", "before").unwrap();
    let value = json!({"base_url":format!("{}/v1",server.uri()),"api_key":"synthetic-pty-key",
        "model":"status-fixture","permission_mode":"manual","harness_profile":"general",
        "voice":{"enabled":false},"gadgets":{"enabled":false},"lsp_servers":{},
        "terminal":{"max_rounds":8,"osc7501":mode != "disabled"}});
    let config_path = crate::config::config_dir().join("config.json");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let config_bytes = serde_json::to_vec_pretty(&value).unwrap();
    std::fs::write(&config_path, &config_bytes).unwrap();
    let cfg: Config = serde_json::from_value(value).unwrap();
    let code = crate::tui::app::run(cfg, false, false, false, None)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&config_path).unwrap(), config_bytes);
    let database = libsql::Builder::new_local(crate::config::config_dir().join("threads.db"))
        .build()
        .await
        .unwrap();
    let connection = database.connect().unwrap();
    let mut rows = connection
        .query(
            "SELECT state_json FROM checkpoints ORDER BY seq DESC LIMIT 1",
            (),
        )
        .await
        .unwrap();
    if let Some(row) = rows.next().await.unwrap() {
        std::fs::write("checkpoint.json", row.get::<String>(0).unwrap()).unwrap();
    }
    std::fs::write(
        "modes.json",
        serde_json::to_vec(&json!({"before":entry,"after":terminal_modes()})).unwrap(),
    )
    .unwrap();
    std::fs::write("calls.json", calls.load(Ordering::SeqCst).to_string()).unwrap();
    std::process::exit(code);
}

struct Pty {
    child: tokio::process::Child,
    master: Arc<AsyncFd<std::fs::File>>,
    _slave: std::fs::File,
    output: Arc<Mutex<Vec<u8>>>,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.reader.abort();
        let _ = self.child.start_kill();
    }
}

impl Pty {
    fn start(directory: &std::path::Path, mode: &str) -> Self {
        let (mut master, mut slave) = (-1, -1);
        let mut size = libc::winsize {
            ws_row: 32,
            ws_col: 100,
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
        for descriptor in [master.as_raw_fd(), slave.as_raw_fd()] {
            assert_eq!(
                unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        assert_eq!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) },
            0
        );
        let mut process = tokio::process::Command::new(std::env::current_exe().unwrap());
        process
            .args(["--ignored", "--exact", HELPER, "--nocapture", "--skip"])
            .arg(format!("{MARKER}{mode}"))
            .current_dir(directory)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("TERM", "xterm-256color")
            .env("CRAFTSMAN_CONFIG_DIR", directory.join("config"))
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()))
            .kill_on_drop(true);
        unsafe {
            process.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = process.spawn().unwrap();
        let master = Arc::new(AsyncFd::new(master).unwrap());
        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = output.clone();
        let stream = master.clone();
        let reader = tokio::spawn(async move {
            let mut buffer = [0; 8192];
            loop {
                let mut readiness = stream.readable().await.unwrap();
                match readiness.try_io(|descriptor| {
                    let count = unsafe {
                        libc::read(
                            descriptor.get_ref().as_raw_fd(),
                            buffer.as_mut_ptr().cast(),
                            buffer.len(),
                        )
                    };
                    if count < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(count as usize)
                    }
                }) {
                    Ok(Ok(0)) | Ok(Err(_)) => return,
                    Ok(Ok(count)) => {
                        let mut bytes = captured.lock().unwrap();
                        assert!(bytes.len() + count <= 2 * 1024 * 1024);
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    Err(_) => {}
                }
            }
        });
        Self {
            child,
            master,
            _slave: slave,
            output,
            reader,
        }
    }

    async fn write(&self, bytes: &[u8]) {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut offset = 0;
            while offset < bytes.len() {
                let mut readiness = self.master.writable().await.unwrap();
                if let Ok(result) = readiness.try_io(|descriptor| {
                    let count = unsafe {
                        libc::write(
                            descriptor.get_ref().as_raw_fd(),
                            bytes[offset..].as_ptr().cast(),
                            bytes.len() - offset,
                        )
                    };
                    if count < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(count as usize)
                    }
                }) {
                    offset += result.unwrap();
                }
            }
        })
        .await
        .unwrap();
    }

    async fn wait_root(&self, state: &str, kind: Option<&str>, after: usize) -> usize {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let bytes = self.output.lock().unwrap().clone();
                let text = String::from_utf8_lossy(&bytes);
                for (offset, _) in text.match_indices("\x1b]7501;") {
                    if offset < after {
                        continue;
                    }
                    let Some((body, _)) = text[offset + 7..].split_once("\x1b\\") else {
                        continue;
                    };
                    let report: BTreeMap<_, _> = body
                        .split(':')
                        .filter_map(|pair| pair.split_once('='))
                        .collect();
                    if !report.contains_key("id")
                        && report.get("state") == Some(&state)
                        && report.get("kind").copied() == kind
                    {
                        return offset + body.len() + 9;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "missing root status {state}/{kind:?}: {}",
                String::from_utf8_lossy(&self.output.lock().unwrap())
            )
        })
    }

    async fn wait_text(&self, needle: &str) {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if String::from_utf8_lossy(&self.output.lock().unwrap()).contains(needle) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "missing terminal text {needle}: {}",
                String::from_utf8_lossy(&self.output.lock().unwrap())
            )
        })
    }

    async fn finish(&mut self, directory: &std::path::Path) {
        let status = tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success());
        let modes: Value =
            serde_json::from_slice(&std::fs::read(directory.join("modes.json")).unwrap()).unwrap();
        #[cfg(target_os = "macos")]
        let transient = libc::PENDIN as u64;
        #[cfg(not(target_os = "macos"))]
        let transient = 0;
        assert_eq!(
            modes["before"]["local"].as_u64().unwrap() & !transient,
            modes["after"]["local"].as_u64().unwrap() & !transient
        );
        for key in ["input", "output", "control"] {
            assert_eq!(modes["before"][key], modes["after"][key]);
        }
    }
}

#[tokio::test]
async fn actual_owned_pty_reports_approval_question_completion_and_restores_modes() {
    let directory = tempfile::tempdir().unwrap();
    let mut pty = Pty::start(directory.path(), "terminal");
    let mut offset = pty.wait_root("idle", None, 0).await;
    pty.write(b"Edit note.txt, ask the question, then finish.\r")
        .await;
    offset = pty.wait_root("working", None, offset).await;
    offset = pty.wait_root("blocked", Some("permission"), offset).await;
    assert_eq!(
        std::fs::read_to_string(directory.path().join("note.txt")).unwrap(),
        "before"
    );
    pty.write(b"\r").await;
    offset = pty.wait_root("working", None, offset).await;
    offset = pty.wait_root("blocked", Some("question"), offset).await;
    assert_eq!(
        std::fs::read_to_string(directory.path().join("note.txt")).unwrap(),
        "after"
    );
    pty.write(b"native answer\r").await;
    offset = pty.wait_root("working", None, offset).await;
    pty.wait_root("done", None, offset).await;
    pty.write(b"\x04").await;
    pty.finish(directory.path()).await;
    assert_eq!(
        std::fs::read_to_string(directory.path().join("calls.json")).unwrap(),
        "3"
    );
}

#[tokio::test]
async fn actual_owned_pty_opt_out_emits_no_status_even_for_control_text_in_reply() {
    let directory = tempfile::tempdir().unwrap();
    let mut pty = Pty::start(directory.path(), "disabled");
    pty.wait_text("AUTH").await;
    pty.write(b"Give the fixture response.\r").await;
    // Diff rendering places the separate words with cursor-position commands.
    // Observe the real escaped suffix, then check the whole committed answer.
    pty.wait_text("ready.␛]7501;state=error:app=forged␛\\")
        .await;
    pty.write(b"\x04").await;
    pty.finish(directory.path()).await;
    assert!(!String::from_utf8_lossy(&pty.output.lock().unwrap()).contains("\x1b]7501;"));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("calls.json")).unwrap(),
        "1"
    );
    let state: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("checkpoint.json")).unwrap())
            .unwrap();
    assert_eq!(
        state["custom"]["_craftsman_durable_turn_v1"]["in_progress"],
        false
    );
    assert!(
        state["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "assistant"
                && message["content"]
                    == "Native fixture response ready.\x1b]7501;state=error:app=forged\x1b\\")
    );
}

#[tokio::test]
async fn actual_owned_pty_reports_returned_model_error() {
    let directory = tempfile::tempdir().unwrap();
    let mut pty = Pty::start(directory.path(), "error");
    let offset = pty.wait_root("idle", None, 0).await;
    pty.write(b"Give the fixture response.\r").await;
    pty.wait_root("error", None, offset).await;
    pty.write(b"\x04").await;
    pty.finish(directory.path()).await;
    let count: usize = std::fs::read_to_string(directory.path().join("calls.json"))
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=3).contains(&count));
}

#[tokio::test]
async fn actual_owned_pty_cancellation_returns_to_idle_after_the_turn_returns() {
    let directory = tempfile::tempdir().unwrap();
    let mut pty = Pty::start(directory.path(), "cancel");
    let mut offset = pty.wait_root("idle", None, 0).await;
    pty.write(b"Wait for the fixture response.\r").await;
    offset = pty.wait_root("working", None, offset).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !directory.path().join("request-started").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    pty.write(b"\x03").await;
    offset = pty.wait_root("working", None, offset).await;
    pty.wait_root("idle", None, offset).await;
    pty.write(b"\x04").await;
    pty.finish(directory.path()).await;
    assert_eq!(
        std::fs::read_to_string(directory.path().join("calls.json")).unwrap(),
        "1"
    );
}

#[tokio::test]
async fn actual_owned_pty_panic_reports_error_seals_status_and_hides_payload() {
    let directory = tempfile::tempdir().unwrap();
    let mut pty = Pty::start(directory.path(), "panic");
    let offset = pty.wait_root("working", None, 0).await;
    let error_end = pty.wait_root("error", None, offset).await;
    pty.finish(directory.path()).await;
    let bytes = pty.output.lock().unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("private-panic-payload"));
    assert!(!text[error_end..].contains("\x1b]7501;state=working"));
    assert!(!text.contains("app=forged"));
}
