//! Child processes for the golden-frame tests: an isolated environment, stderr
//! capture, and a kill on drop so a failing test never leaves a server behind.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

/// Longest any single wait on a child, a socket or an HTTP server may take.
/// The binary is a debug build and CI runners are shared and sometimes starved
/// for seconds, so this is generous; a hang still fails in well under a minute
/// rather than at the job timeout.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// An isolated configured HOME, config directory and workspace for a scenario.
/// Windows native profile APIs may still resolve the OS user's real home; the
/// bridge stats fixture separately bounds its first read-only metadata walk.
pub struct Sandbox {
    pub home: tempfile::TempDir,
    pub workspace: tempfile::TempDir,
}

/// The key the program under test is configured with. It must never appear in
/// a frame, so the scenarios forbid it in their recordings.
pub const MODEL_API_KEY: &str = "golden-model-key";

/// A direct OpenAI-compatible provider pointing at the scripted model.
pub fn model_config(base_url: &str) -> Value {
    json!({
        "base_url": base_url,
        "api_key": MODEL_API_KEY,
        "model": "mock-model",
    })
}

impl Sandbox {
    /// A fresh sandbox; `config` becomes `$HOME/.craftsman/config.json`.
    pub fn new(config: Option<Value>) -> Sandbox {
        let home = tempfile::tempdir().expect("create HOME");
        let workspace = tempfile::tempdir().expect("create workspace");
        if let Some(config) = config {
            let dir = home.path().join(".craftsman");
            std::fs::create_dir_all(&dir).expect("create config dir");
            std::fs::write(dir.join("config.json"), config.to_string()).expect("write config");
        }
        Sandbox { home, workspace }
    }

    /// `craftsman <args>` with nothing inherited from the developer's environment
    /// (no proxy secret, desktop token, platform token, shell allow-list, ...)
    /// except PATH, which gets the directory of the built binary in front so
    /// that a bridge can launch its harness as plain `craftsman`. The caller
    /// decides the stdio wiring through [`Proc::spawn_server`] or
    /// [`Proc::spawn_stdio`].
    pub fn craftsman(&self, args: &[&str]) -> Command {
        let binary = Path::new(env!("CARGO_BIN_EXE_craftsman"));
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(binary.parent().expect("binary dir").to_path_buf())
                .chain(std::env::split_paths(&inherited)),
        )
        .expect("join PATH");

        let mut command = Command::new(binary);
        command
            .args(args)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.home.path())
            .env("CRAFTSMAN_CONFIG_DIR", self.home.path().join(".craftsman"))
            .env("RUST_BACKTRACE", "1")
            .current_dir(self.workspace.path())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            command.env("USERPROFILE", self.home.path());
            for key in ["SystemRoot", "TEMP", "TMP"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
        }
        command
    }

    pub fn path_in_workspace(&self, name: &str) -> PathBuf {
        self.workspace.path().join(name)
    }
}

/// What a child has written to stderr so far. A pump task drains the pipe, so
/// the child can never block on a full buffer, and the log is available for
/// startup handshakes and for failure messages.
#[derive(Clone, Default)]
pub struct StderrLog {
    lines: Arc<Mutex<Vec<String>>>,
    closed: Arc<AtomicBool>,
}

impl StderrLog {
    fn pump(stderr: ChildStderr) -> StderrLog {
        let log = StderrLog::default();
        let writer = log.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                writer.lines.lock().unwrap().push(line);
            }
            writer.closed.store(true, Ordering::SeqCst);
        });
        log
    }

    /// The first stderr line containing `needle`, waiting up to `within`.
    pub async fn wait_for(&self, needle: &str, within: Duration) -> Result<String, String> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            // Read `closed` before the lines: the pump sets it last, so if it
            // is set here every line is already in the log.
            let closed = self.closed.load(Ordering::SeqCst);
            let found = self
                .lines
                .lock()
                .unwrap()
                .iter()
                .find(|line| line.contains(needle))
                .cloned();
            if let Some(line) = found {
                return Ok(line);
            }
            if closed {
                return Err(format!(
                    "the process closed stderr without a line containing {needle:?}"
                ));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "no stderr line containing {needle:?} within {within:?}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The last `count` lines, indented, for failure messages.
    pub fn tail(&self, count: usize) -> String {
        let lines = self.lines.lock().unwrap();
        if lines.is_empty() {
            return "    (empty)".to_string();
        }
        let from = lines.len().saturating_sub(count);
        lines[from..]
            .iter()
            .map(|line| format!("    {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A running child. Dropping it kills the child; if the thread is unwinding
/// from a failed assertion, the child's stderr is printed first.
pub struct Proc {
    name: String,
    child: Child,
    pub stderr: StderrLog,
}

impl Proc {
    /// A server that is driven over the network: stdin is closed and stdout is
    /// discarded, so neither can block it.
    pub fn spawn_server(name: &str, mut command: Command) -> Proc {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let (proc, _, _) = Proc::spawn(name, command);
        proc
    }

    /// A child driven over its stdio: returns its stdin and stdout.
    pub fn spawn_stdio(name: &str, mut command: Command) -> (Proc, ChildStdin, ChildStdout) {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (proc, stdin, stdout) = Proc::spawn(name, command);
        (
            proc,
            stdin.expect("stdin is piped"),
            stdout.expect("stdout is piped"),
        )
    }

    fn spawn(name: &str, mut command: Command) -> (Proc, Option<ChildStdin>, Option<ChildStdout>) {
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("could not start {name}: {error}"));
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = StderrLog::pump(child.stderr.take().expect("stderr is piped"));
        let proc = Proc {
            name: name.to_string(),
            child,
            stderr,
        };
        (proc, stdin, stdout)
    }

    /// Wait up to `within` for the child to exit on its own.
    pub async fn wait_exit(&mut self, within: Duration) -> Option<ExitStatus> {
        tokio::time::timeout(within, self.child.wait())
            .await
            .ok()
            .and_then(Result::ok)
    }

    /// Wait for the child to exit on its own and require a clean exit.
    pub async fn wait_success(&mut self, context: &str) {
        match self.wait_exit(STEP_TIMEOUT).await {
            Some(status) => assert!(
                status.success(),
                "{} exited with {status} {context}\nstderr:\n{}",
                self.name,
                self.stderr.tail(40)
            ),
            None => panic!(
                "{} did not exit within {STEP_TIMEOUT:?} {context}\nstderr:\n{}",
                self.name,
                self.stderr.tail(40)
            ),
        }
    }

    /// Stop the child now and reap it.
    pub async fn kill(&mut self) {
        let _ = tokio::time::timeout(STEP_TIMEOUT, self.child.kill()).await;
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "--- stderr of {} (test failed) ---\n{}",
                self.name,
                self.stderr.tail(60)
            );
        }
        let _ = self.child.start_kill();
    }
}
