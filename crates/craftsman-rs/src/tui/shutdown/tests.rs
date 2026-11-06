//! Real newly owned PTYs, TUI key input and OS signals. No real terminal/config.
use crate::tools::owned_cancellation_tests::{command, marker_directory, ready};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::unix::AsyncFd;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned PTY fixture; refuses unmarked startup before config/terminal access"]
async fn terminal_child() {
    let Some(directory) = marker_directory() else {
        return;
    };
    if directory.join("panic").exists() {
        let mut entry = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(0, &mut entry) }, 0);
        std::fs::write(directory.join("entry.json"), serde_json::json!({"local":entry.c_lflag as u64,"input":entry.c_iflag as u64,"output":entry.c_oflag as u64,"control":entry.c_cflag as u64}).to_string()).unwrap();
        let shutdown = super::Shutdown::install().unwrap();
        let guard = crate::tui::terminal::TerminalGuard::enter(false).unwrap();
        let hook = guard.panic_hook(shutdown.cancellation.clone());
        let failed =
            std::panic::catch_unwind(|| panic!("secret-panic-fixture\u{1b}]52;c;unsafe\u{7}"));
        assert!(failed.is_err());
        assert!(hook.triggered());
        assert!(shutdown.cancellation.is_cancelled());
        drop(guard);
        drop(hook);
        record_restored(&directory);
        return;
    }
    let code = crate::tui::app::run(crate::config::Config::default(), false, false, false, None)
        .await
        .unwrap();
    record_restored(&directory);
    std::process::exit(code);
}

fn record_restored(directory: &std::path::Path) {
    let mut value = unsafe { std::mem::zeroed::<libc::termios>() };
    assert_eq!(unsafe { libc::tcgetattr(0, &mut value) }, 0);
    std::fs::write(directory.join("restored.json"), serde_json::json!({"local":value.c_lflag as u64,"input":value.c_iflag as u64,"output":value.c_oflag as u64,"control":value.c_cflag as u64}).to_string()).unwrap();
}

struct Pty {
    child: tokio::process::Child,
    master: Arc<AsyncFd<std::fs::File>>,
    slave: std::fs::File,
    reader: tokio::task::JoinHandle<()>,
    output: Arc<Mutex<Vec<u8>>>,
    original: libc::termios,
    directory: std::path::PathBuf,
}
impl Drop for Pty {
    fn drop(&mut self) {
        self.reader.abort();
        let _ = self.child.start_kill();
    }
}

impl Pty {
    fn start(directory: &std::path::Path) -> Self {
        let (mut master, mut slave) = (-1, -1);
        let mut size = libc::winsize {
            ws_row: 24,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: both output descriptors and the window size are owned locals.
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
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut original) },
            0
        );
        let mut process = tokio::process::Command::new(std::env::current_exe().unwrap());
        process
            .args([
                "--ignored",
                "--exact",
                "tui::shutdown::tests::terminal_child",
                "--nocapture",
                "--skip",
            ])
            .arg(format!(
                "craftsman-owned-native-v1={}",
                directory.to_str().unwrap()
            ))
            .current_dir(directory)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("TERM", "xterm-256color")
            .env("CRAFTSMAN_CONFIG_DIR", directory.join("config"))
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()))
            .kill_on_drop(true);
        // SAFETY: only async-signal-safe syscalls run between fork and exec.
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
            let mut buffer = [0u8; 8192];
            loop {
                let mut readable = stream.readable().await.unwrap();
                let result = readable.try_io(|fd| {
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
                match result {
                    Ok(Ok(0)) | Ok(Err(_)) => return,
                    Ok(Ok(count)) => {
                        let mut bytes = captured.lock().unwrap();
                        assert!(bytes.len() + count <= 1024 * 1024);
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    Err(_) => {}
                }
            }
        });
        Self {
            child,
            master,
            slave,
            reader,
            output,
            original,
            directory: directory.into(),
        }
    }
    async fn write(&self, data: &[u8]) {
        let mut written = 0;
        while written < data.len() {
            let mut writable = self.master.writable().await.unwrap();
            match writable.try_io(|fd| {
                let count = unsafe {
                    libc::write(
                        fd.get_ref().as_raw_fd(),
                        data[written..].as_ptr().cast(),
                        data.len() - written,
                    )
                };
                if count < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(count as usize)
                }
            }) {
                Ok(result) => written += result.unwrap(),
                Err(_) => {}
            }
        }
    }
    async fn wait_text(&self, text: &str) {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if String::from_utf8_lossy(&self.output.lock().unwrap()).contains(text) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "PTY did not render {text:?}: {}",
                String::from_utf8_lossy(&self.output.lock().unwrap())
            )
        });
    }
    fn termios(&self) -> libc::termios {
        let mut value = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(self.slave.as_raw_fd(), &mut value) },
            0
        );
        value
    }
    fn restored(&self) {
        let current: serde_json::Value =
            serde_json::from_slice(&std::fs::read(self.directory.join("restored.json")).unwrap())
                .unwrap();
        // Darwin sets PENDIN when canonical input is restored. It is kernel
        // reprocessing state, not a configured terminal mode. Keep raw facts;
        // never consume/flush input merely to force that transient bit to zero.
        #[cfg(target_os = "macos")]
        let transient = libc::PENDIN as u64;
        #[cfg(not(target_os = "macos"))]
        let transient = 0;
        if self.directory.join("entry.json").exists() {
            let entry = std::fs::read_to_string(self.directory.join("entry.json")).unwrap();
            assert_eq!(
                current["local"].as_u64().unwrap() & !transient,
                (self.original.c_lflag as u64) & !transient,
                "actual child entry flags: {entry}"
            );
        }
        assert_eq!(
            current["local"].as_u64().unwrap() & !transient,
            (self.original.c_lflag as u64) & !transient
        );
        assert_eq!(
            current["input"].as_u64(),
            Some(self.original.c_iflag as u64)
        );
        assert_eq!(
            current["output"].as_u64(),
            Some(self.original.c_oflag as u64)
        );
        assert_eq!(
            current["control"].as_u64(),
            Some(self.original.c_cflag as u64)
        );
    }
}

async fn scenario(signal: Option<i32>) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("plan"), b"owned-native-v1").unwrap();
    let mut pty = Pty::start(directory.path());
    // Wait for the real TUI before entering a literal user shell submission.
    pty.wait_text("Setup required").await;
    let line = format!("!{}\r", command(directory.path()));
    pty.write(line.as_bytes()).await;
    ready(directory.path()).await;
    assert_eq!(pty.termios().c_lflag & libc::ICANON, 0);
    if let Some(signal) = signal {
        // The still-owned child has not been waited/reaped; never a saved PID.
        assert_eq!(
            unsafe { libc::kill(pty.child.id().unwrap() as i32, signal) },
            0
        );
    } else {
        pty.write(b"\x03").await;
        pty.wait_text("command cancelled").await;
        assert!(!directory.path().join("late").exists());
        assert_eq!(pty.termios().c_lflag & libc::ICANON, 0);
        pty.write(b"\x04").await;
    }
    let status = tokio::time::timeout(Duration::from_secs(10), pty.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(signal.map_or(0, |value| 128 + value)));
    assert!(!directory.path().join("late").exists());
    pty.restored();
}

#[tokio::test]
async fn ctrl_c_stops_the_actual_tui_shell_without_exiting_and_quit_restores_the_pty() {
    scenario(None).await;
}
#[tokio::test]
async fn sigterm_routes_through_actual_tui_cleanup_and_restores_the_pty() {
    scenario(Some(libc::SIGTERM)).await;
}
#[tokio::test]
async fn sighup_routes_through_actual_tui_cleanup_and_restores_the_pty() {
    scenario(Some(libc::SIGHUP)).await;
}

#[tokio::test]
async fn panic_restores_the_owned_pty_cancels_shutdown_and_hides_untrusted_payloads() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("plan"), b"owned-native-v1").unwrap();
    std::fs::write(directory.path().join("panic"), b"owned panic fixture").unwrap();
    let mut pty = Pty::start(directory.path());
    pty.wait_text("Craftsman encountered an internal error")
        .await;
    let status = tokio::time::timeout(Duration::from_secs(10), pty.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    pty.restored();
    assert!(!String::from_utf8_lossy(&pty.output.lock().unwrap()).contains("secret-panic-fixture"));
}
