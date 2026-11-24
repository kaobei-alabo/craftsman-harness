//! The explicit `craftsman privacy` commands and the interactive privacy
//! route, driven through the real `craftsman` binary and its own `main`.
//!
//! Every case uses a private temporary directory as the user configuration
//! root and workspace, an empty environment and synthetic user files. No real
//! user file, key, password or environment is read or changed, no model is
//! reached and no detector guest runs: the cases stop at what the binary
//! decides, shows and writes locally.
//!
//! A terminal case gives the binary a real pseudo-terminal. A stage is found
//! on the screen that the terminal's own output produces: the renderer writes
//! only the cells that changed, so the raw stream alone does not hold the
//! text of a redrawn line. The raw bytes stay available for the
//! control-sequence checks. The binary is not made a session leader, so that
//! this test can still read the terminal's modes after it has exited.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The bound of each stage and of each process, as in the unit-test driver.
const LIMIT: Duration = Duration::from_secs(60);
/// A synthetic passphrase with a combining mark and outer spaces.
const SECRET: &str = "  synthetic-vault-e\u{301}-passphrase  ";
const PINS: &str = r#""detector":{"gadget":"privacy-detector","function":"detect","component_sha256":"0000000000000000000000000000000000000000000000000000000000000000","manifest_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
const NOT_YET: &str = "Enabled privacy is not yet available in this version.";
/// The status line of a passed read-only preflight. It names what a start
/// still has to check; it asserts no unlock, approval, support or readiness.
#[cfg(feature = "gadgets")]
const PREFLIGHT_PASSED: &str = "Interactive startup: read-only preflight passed (policy, voice setting, key files, stored detector approval). Not checked until `craftsman` starts: the vault unlock, the detector's own call check, the model connection, native-view support for the configured model wire, and support for every requested session feature. A passed preflight is not an unlock, an approval or readiness.";

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
    /// Writes the synthetic user file, user-only, and returns its bytes.
    fn user(&self, text: &str) -> Vec<u8> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(self.config.join("config.json"))
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
        file.sync_all().unwrap();
        text.as_bytes().to_vec()
    }
    fn user_bytes(&self) -> Vec<u8> {
        std::fs::read(self.config.join("config.json")).unwrap()
    }
    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_craftsman"));
        command
            .args(arguments)
            .current_dir(&self.workspace)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("TERM", "xterm-256color")
            .env("HOME", &self.root)
            .env("CRAFTSMAN_CONFIG_DIR", &self.config);
        command
    }
    /// Without a terminal: stdin is closed and both outputs are pipes.
    fn run(&self, arguments: &[&str]) -> (String, i32) {
        let output = self
            .command(arguments)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let mut text = String::from_utf8(output.stdout).unwrap();
        text.push_str(&String::from_utf8(output.stderr).unwrap());
        (text, output.status.code().unwrap())
    }
    /// Every entry of the synthetic configuration root, by name.
    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.config)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

#[cfg(feature = "gadgets")]
/// No file under `directory` holds the passphrase.
fn absent_secret(directory: &std::path::Path) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            absent_secret(&path);
        } else {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(SECRET.len())
                    .any(|window| window == SECRET.as_bytes()),
                "a stored file holds the passphrase"
            );
        }
    }
}

/// The terminal's output without control sequences and without blank cells.
fn plain(bytes: &[u8]) -> String {
    let mut text = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte != 0x1b {
            if !byte.is_ascii_whitespace() {
                text.push(byte);
            }
            index += 1;
            continue;
        }
        index = after_escape(bytes, index).0;
    }
    String::from_utf8_lossy(&text).into_owned()
}

/// The index after the escape sequence that starts at `index`, and, for a
/// control sequence, its parameter bytes and final byte.
fn after_escape(bytes: &[u8], index: usize) -> (usize, Option<(&[u8], u8)>) {
    match bytes.get(index + 1) {
        // A control sequence ends with its final byte.
        Some(b'[') => {
            let start = index + 2;
            let mut end = start;
            while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                end += 1;
            }
            match bytes.get(end) {
                Some(last) => (end + 1, Some((&bytes[start..end], *last))),
                None => (end, None),
            }
        }
        // An operating-system command ends with BEL or with ESC backslash.
        Some(b']') => {
            let mut end = index + 2;
            while end < bytes.len() && bytes[end] != 0x07 && bytes[end] != 0x1b {
                end += 1;
            }
            (
                end + if bytes.get(end) == Some(&0x1b) { 2 } else { 1 },
                None,
            )
        }
        Some(b'(' | b')') => (index + 3, None),
        _ => (index + 2, None),
    }
}

const ROWS: usize = 40;
const COLUMNS: usize = 120;

#[cfg(feature = "gadgets")]
/// The screen that `bytes` leave on a terminal of this test's size: cursor
/// positioning, erasing, line feeds and the alternate screen. Colours and
/// modes are ignored, and every character takes one cell; the renderer
/// positions the cursor again after anything wider.
fn screen(bytes: &[u8]) -> String {
    let blank = || vec![vec![' '; COLUMNS]; ROWS];
    let (mut grid, mut saved) = (blank(), blank());
    let (mut row, mut column) = (0usize, 0usize);
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == 0x1b {
            let (next, control) = after_escape(bytes, index);
            index = next;
            let Some((parameters, last)) = control else {
                continue;
            };
            let text = std::str::from_utf8(parameters).unwrap_or_default();
            let mut numbers = text
                .trim_start_matches('?')
                .split(';')
                .map(|number| number.parse::<usize>().unwrap_or(0));
            let first = numbers.next().unwrap_or(0);
            let second = numbers.next().unwrap_or(0);
            match last {
                b'H' | b'f' => {
                    row = first.max(1).min(ROWS) - 1;
                    column = second.max(1).min(COLUMNS) - 1;
                }
                b'A' => row = row.saturating_sub(first.max(1)),
                b'B' => row = (row + first.max(1)).min(ROWS - 1),
                b'C' => column = (column + first.max(1)).min(COLUMNS - 1),
                b'D' => column = column.saturating_sub(first.max(1)),
                b'G' => column = first.max(1).min(COLUMNS) - 1,
                b'd' => row = first.max(1).min(ROWS) - 1,
                b'J' => {
                    for (line, cells) in grid.iter_mut().enumerate() {
                        for (cell, character) in cells.iter_mut().enumerate() {
                            let after = (line, cell) >= (row, column);
                            if first >= 2 || (first == 0 && after) || (first == 1 && !after) {
                                *character = ' ';
                            }
                        }
                    }
                }
                b'K' => {
                    for (cell, character) in grid[row].iter_mut().enumerate() {
                        if first >= 2
                            || (first == 0 && cell >= column)
                            || (first == 1 && cell <= column)
                        {
                            *character = ' ';
                        }
                    }
                }
                b'h' if text == "?1049" => {
                    saved = std::mem::replace(&mut grid, blank());
                    (row, column) = (0, 0);
                }
                b'l' if text == "?1049" => grid = std::mem::replace(&mut saved, blank()),
                _ => {}
            }
            continue;
        }
        let length = match byte {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            _ => 4,
        };
        let character = std::str::from_utf8(&bytes[index..(index + length).min(bytes.len())])
            .ok()
            .and_then(|text| text.chars().next());
        index += length;
        match (byte, character) {
            (b'\r', _) => column = 0,
            (0x08, _) => column = column.saturating_sub(1),
            (b'\n', _) => {
                if row + 1 == ROWS {
                    grid.remove(0);
                    grid.push(vec![' '; COLUMNS]);
                } else {
                    row += 1;
                }
            }
            (_, Some(character)) if !character.is_control() => {
                if column == COLUMNS {
                    column = 0;
                    if row + 1 == ROWS {
                        grid.remove(0);
                        grid.push(vec![' '; COLUMNS]);
                    } else {
                        row += 1;
                    }
                }
                grid[row][column] = character;
                column += 1;
            }
            _ => {}
        }
    }
    grid.iter()
        .map(|cells| cells.iter().collect::<String>().trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}
fn squeezed(text: &str) -> String {
    text.split_whitespace().collect()
}

fn modes(descriptor: i32) -> libc::termios {
    let mut mode = unsafe { std::mem::zeroed::<libc::termios>() };
    assert_eq!(unsafe { libc::tcgetattr(descriptor, &mut mode) }, 0);
    mode
}

/// The real binary on a real pseudo-terminal that this test owns.
struct Terminal {
    child: std::process::Child,
    master: std::fs::File,
    slave: std::fs::File,
    output: Vec<u8>,
    original: libc::termios,
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Terminal {
    fn start(home: &Home, arguments: &[&str]) -> Self {
        let (mut master, mut slave) = (-1, -1);
        let mut size = libc::winsize {
            ws_row: ROWS as u16,
            ws_col: COLUMNS as u16,
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
        let original = modes(slave.as_raw_fd());
        let mut command = home.command(arguments);
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()));
        Self {
            child: command.spawn().unwrap(),
            master,
            slave,
            output: Vec::new(),
            original,
        }
    }
    /// Reads what the terminal has written, waiting at most `wait` for more.
    fn pump(&mut self, wait: Duration) {
        let mut poll = libc::pollfd {
            fd: self.master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, wait.as_millis() as libc::c_int) } <= 0 {
            return;
        }
        let mut buffer = [0; 4096];
        loop {
            match self.master.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => self.output.extend_from_slice(&buffer[..count]),
            }
        }
    }
    #[cfg(feature = "gadgets")]
    /// Waits until the terminal's screen shows `text`.
    fn wait_for(&mut self, text: &str) {
        let deadline = Instant::now() + LIMIT;
        loop {
            if screen(&self.output).contains(text) {
                // A stage holds on two samples: a line that the next write
                // erases, such as the old composer under a new popup, is not
                // a stage.
                self.pump(Duration::from_millis(30));
                if screen(&self.output).contains(text) {
                    return;
                }
            }
            let ended = self.child.try_wait().unwrap().is_some();
            self.pump(Duration::from_millis(50));
            if (ended && !screen(&self.output).contains(text)) || Instant::now() > deadline {
                self.absent_secret();
                panic!(
                    "the terminal did not reach its stage {text:?}; its screen was:\n{}",
                    screen(&self.output)
                );
            }
        }
    }
    #[cfg(feature = "gadgets")]
    /// Waits until the screen shows one of `texts` and returns which.
    fn wait_for_any(&mut self, texts: &[&str]) -> usize {
        let deadline = Instant::now() + LIMIT;
        loop {
            let shown = screen(&self.output);
            if let Some(index) = texts.iter().position(|text| shown.contains(text)) {
                return index;
            }
            let ended = self.child.try_wait().unwrap().is_some();
            self.pump(Duration::from_millis(50));
            if ended || Instant::now() > deadline {
                let shown = screen(&self.output);
                if let Some(index) = texts.iter().position(|text| shown.contains(text)) {
                    return index;
                }
                self.absent_secret();
                panic!(
                    "the terminal reached none of its stages {texts:?}; its screen was:\n{shown}"
                );
            }
        }
    }
    #[cfg(feature = "gadgets")]
    /// Answers the vault password prompt and waits for the idle composer of
    /// the session that is built after it. The prompt must be gone first:
    /// while it is up, input belongs to it.
    fn unlock(&mut self) {
        self.wait_for("Unlock private privacy key");
        self.send(format!("{SECRET}\r").as_bytes());
        self.wait_until_gone("Unlock private privacy key");
        self.wait_for("message · Ctrl+J newline");
    }
    #[cfg(feature = "gadgets")]
    /// Waits until the screen no longer shows `text`.
    fn wait_until_gone(&mut self, text: &str) {
        let deadline = Instant::now() + LIMIT;
        while screen(&self.output).contains(text) {
            self.pump(Duration::from_millis(50));
            assert!(
                Instant::now() < deadline,
                "the terminal still shows {text:?}; its screen was:\n{}",
                screen(&self.output)
            );
        }
    }
    #[cfg(feature = "gadgets")]
    fn send(&mut self, bytes: &[u8]) {
        let mut remaining = bytes;
        while !remaining.is_empty() {
            match self.master.write(remaining) {
                Ok(count) => remaining = &remaining[count..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    self.pump(Duration::from_millis(5))
                }
                Err(error) => panic!("terminal write failed: {error}"),
            }
        }
    }
    fn absent_secret(&self) {
        assert!(
            !self
                .output
                .windows(SECRET.len())
                .any(|window| window == SECRET.as_bytes()),
            "the terminal echoed the passphrase"
        );
    }
    /// Waits for the process, then returns its whole raw terminal output and
    /// exit code. The terminal's modes are what they were before it started.
    fn finish(mut self) -> (Vec<u8>, i32) {
        let deadline = Instant::now() + LIMIT;
        let status = loop {
            self.pump(Duration::from_millis(50));
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "the process exceeded its bound");
        };
        self.pump(Duration::from_millis(100));
        self.absent_secret();
        let restored = modes(self.slave.as_raw_fd());
        #[cfg(target_os = "macos")]
        let local = !(libc::PENDIN as libc::tcflag_t);
        #[cfg(not(target_os = "macos"))]
        let local = !0;
        assert_eq!(restored.c_iflag, self.original.c_iflag);
        assert_eq!(restored.c_oflag, self.original.c_oflag);
        assert_eq!(restored.c_cflag, self.original.c_cflag);
        assert_eq!(restored.c_lflag & local, self.original.c_lflag & local);
        assert_eq!(restored.c_cc, self.original.c_cc);
        (std::mem::take(&mut self.output), status.code().unwrap())
    }
}

fn enabled(extra: &str) -> String {
    format!(r#"{{{extra}"privacy":{{"enabled":true,{PINS}}}}}"#)
}
const VOICE_OFF: &str = r#""voice":{"enabled":false},"#;

#[cfg(feature = "gadgets")]
/// `craftsman privacy init` through the owned popup, with real key derivation.
fn initialize(home: &Home) {
    let mut terminal = Terminal::start(home, &["privacy", "init"]);
    terminal.wait_for("Initialize private privacy key");
    terminal.send(format!("{SECRET}\r").as_bytes());
    terminal.wait_for("Confirm private passphrase");
    terminal.send(format!("{SECRET}\r").as_bytes());
    let (output, code) = terminal.finish();
    assert_eq!(code, 0);
    assert!(plain(&output).contains(&squeezed(
        "The mapping-vault key and vault were created. Privacy is not enabled by this command."
    )));
    assert!(home.config.join("privacy-key.v1").is_file());
}

#[test]
fn privacy_status_reports_the_local_state_and_only_a_preflight() {
    let home = Home::new();
    let missing = "Mapping-vault key: not created. `craftsman privacy init` creates it.";
    let not_enabled = "Privacy policy: not enabled in the user configuration.";
    assert_eq!(
        home.run(&["privacy", "status"]),
        (format!("{missing}\n{not_enabled}\n"), 0)
    );
    home.user(&format!(
        r#"{{{VOICE_OFF}"privacy":{{"enabled":false,{PINS}}}}}"#
    ));
    assert_eq!(
        home.run(&["privacy", "status"]),
        (format!("{missing}\n{not_enabled}\n"), 0)
    );
    // A malformed policy is reported as refused, never as not enabled.
    home.user(r#"{"privacy":{"enabled":true,"detector":{"gadget":"privacy-detector"}}}"#);
    assert_eq!(
        home.run(&["privacy", "status"]),
        (
            format!(
                "{missing}\nPrivacy policy: refused. The captured user configuration is invalid.\n"
            ),
            0
        )
    );
    let enabled_line = "Privacy policy: enabled in the user configuration.";
    #[cfg(feature = "gadgets")]
    let refusals = [
        (
            enabled(""),
            "Enabled privacy requires voice.enabled:false in the user configuration; voice input has no private adapter.",
        ),
        (
            enabled(VOICE_OFF),
            "No mapping-vault key exists yet. Run `craftsman privacy init` first; nothing was unlocked.",
        ),
    ];
    #[cfg(not(feature = "gadgets"))]
    let refusals = [(
        enabled(VOICE_OFF),
        "The private terminal requires the source-built detector feature.",
    )];
    for (user, refusal) in refusals {
        home.user(&user);
        assert_eq!(
            home.run(&["privacy", "status"]),
            (
                format!("{missing}\n{enabled_line}\nInteractive startup: refused. {refusal}\n"),
                0
            )
        );
    }
    // Read-only: nothing but the synthetic user file is there.
    assert_eq!(home.entries(), ["config.json"]);
}

#[test]
fn only_the_eligible_interactive_invocation_takes_the_privacy_capture() {
    let home = Home::new();
    // Without a user file every invocation keeps its own answer.
    let (output, code) = home.run(&[]);
    assert_eq!(
        (output.as_str(), code),
        ("Error: interactive mode requires a terminal\n", 1)
    );
    // With an enabled policy, everything but the owned interactive terminal
    // keeps the loader's own rejection: a one-shot message, a resumed
    // session, other frontends, and an invocation without a terminal.
    let user = home.user(&enabled(VOICE_OFF));
    let mut invocations: Vec<&[&str]> = vec![
        &[],
        &["-m", "hello"],
        &["--session", "synthetic-thread"],
        &["setup"],
        &["acp-serve"],
    ];
    if cfg!(feature = "gadgets") {
        invocations.push(&["gadgets", "list"]);
    }
    for arguments in invocations {
        let (output, code) = home.run(arguments);
        assert_eq!(
            (output.as_str(), code),
            (format!("Error: {NOT_YET}\n").as_str(), 1),
            "{arguments:?}"
        );
    }
    // A resumed session keeps that rejection on a terminal too.
    let (output, code) = Terminal::start(&home, &["--session", "synthetic-thread"]).finish();
    assert!(plain(&output).contains(&squeezed(NOT_YET)));
    assert_eq!(code, 1);
    // An unparsable user file keeps the loader's own text off a terminal.
    home.user(r#"{"voice": "#);
    let (output, code) = home.run(&[]);
    assert!(output.starts_with("Error: could not parse "));
    assert!(output.ends_with("invalid configuration at line 1, column 10\n"));
    assert_eq!(code, 1);
    home.user(std::str::from_utf8(&user).unwrap());
    assert_eq!(home.entries(), ["config.json"]);
}

/// The interactive router's early answers on a real terminal, and the
/// create-only `privacy init`. This case was `entry_tests`'
/// `actual_normal_startup_answers_early_and_init_stays_create_only`; it now
/// drives the real binary instead of calling the router in a test child.
#[cfg(feature = "gadgets")]
#[test]
fn actual_normal_startup_answers_early_and_init_stays_create_only() {
    // A refused capture is final: the terminal never starts as privacy-off.
    for (user, refusal) in [
        (
            enabled(""),
            "Enabled privacy requires voice.enabled:false in the user configuration; voice input has no private adapter.",
        ),
        (
            format!(
                r#"{{{VOICE_OFF}"privacy":{{"enabled":true,{PINS},"identifiers":["synthetic"]}}}}"#
            ),
            "The private terminal cannot omit unsupported declared identifiers.",
        ),
        (
            r#"{"voice":{"enabled":false},"privacy":{"enabled":true}}"#.to_owned(),
            "The captured user configuration is invalid.",
        ),
        (
            enabled(VOICE_OFF),
            "No mapping-vault key exists yet. Run `craftsman privacy init` first; nothing was unlocked.",
        ),
    ] {
        let home = Home::new();
        let user = home.user(&user);
        let (output, code) = Terminal::start(&home, &[]).finish();
        assert!(
            plain(&output).contains(&squeezed(&format!("Error: {refusal}"))),
            "{refusal}: {}",
            plain(&output)
        );
        assert_eq!(code, 1);
        // No alternate screen was entered and nothing was created.
        assert!(!output.windows(8).any(|window| window == b"\x1b[?1049h"));
        assert_eq!(home.user_bytes(), user);
        assert_eq!(home.entries(), ["config.json"]);
    }

    // A disabled policy is the capture's own legacy answer: the ordinary
    // terminal starts, here without a model connection.
    let home = Home::new();
    let user = home.user(&format!(
        r#"{{{VOICE_OFF}"privacy":{{"enabled":false,{PINS}}}}}"#
    ));
    let mut terminal = Terminal::start(&home, &[]);
    terminal.wait_for("Setup required");
    terminal.send(b"/quit\r");
    let (_, code) = terminal.finish();
    assert_eq!(code, 0);
    assert_eq!(home.user_bytes(), user);
    assert!(!home.config.join("privacy-key.v1").exists());

    // A valid policy without a key: the create-only command, twice.
    let home = Home::new();
    let user = home.user(&enabled(VOICE_OFF));
    initialize(&home);
    let key = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
    let (output, code) = home.run(&["privacy", "init"]);
    assert_eq!(
        (output.as_str(), code),
        (
            "Error: The private terminal bootstrap stopped: The private bootstrap key operation stopped: The private key initialization requires both files and sidecars to be absent.\n",
            1
        )
    );
    assert_eq!(
        std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
        key
    );
    // The status now names the next thing that is missing: the pinned
    // detector of this synthetic policy is not installed.
    let (status, _) = home.run(&["privacy", "status"]);
    assert_eq!(
        status,
        "Mapping-vault key: files present. Presence is not an unlock.\nPrivacy policy: enabled in the user configuration.\nInteractive startup: refused. The installed detector gadget does not match the policy's pins or has no stored source approval; nothing was unlocked.\n"
    );
    assert_eq!(home.user_bytes(), user);
    assert!(!home.config.join("threads.db").exists());
    absent_secret(&home.config);

    // A setting of the captured source that does not parse is refused by
    // the status and by the route, there before any terminal effect: no
    // alternate screen and no password prompt.
    let user = home.user(&format!(
        r#"{{"terminal":{{"osc7501":"yes"}},{VOICE_OFF}"privacy":{{"enabled":true,{PINS}}}}}"#
    ));
    let invalid = "The captured user configuration is invalid.";
    let (status, _) = home.run(&["privacy", "status"]);
    assert!(
        status.ends_with(&format!("Interactive startup: refused. {invalid}\n")),
        "{status}"
    );
    let (output, code) = Terminal::start(&home, &[]).finish();
    assert_eq!(code, 1);
    assert!(plain(&output).contains(&squeezed(&format!("Error: {invalid}"))));
    assert!(!output.windows(8).any(|window| window == b"\x1b[?1049h"));
    assert!(!plain(&output).contains("Unlockprivateprivacykey"));
    assert_eq!(home.user_bytes(), user);
    assert!(!home.config.join("threads.db").exists());
}

/// The real binary through the unlock and into its private terminal, on a
/// synthetic policy whose detector is not installed. Every private build
/// admits the system text through the detector, so this build is refused: an
/// ordinary refusal after the unlock, not a conversation. The case checks what
/// the terminal does around it: the fixed status reports, the invocation's
/// mouse choice switched on after the password prompt and off again, the
/// fixed error, restored modes and unchanged files. It is not a witness of a
/// cancelled or overrunning construction.
#[cfg(feature = "gadgets")]
#[test]
fn the_private_terminal_reports_status_and_restores_the_mouse_when_its_build_is_refused() {
    let home = Home::new();
    let user = home.user(&format!(
        r#"{{"base_url":"http://127.0.0.1:9","api_key":"synthetic-key","model":"synthetic-model","harness_profile":"general","gadgets":{{"enabled":true}},"mcp_servers":[],"lsp_servers":{{}},{VOICE_OFF}"privacy":{{"enabled":true,{PINS}}}}}"#
    ));
    initialize(&home);
    let key = std::fs::read(home.config.join("privacy-key.v1")).unwrap();

    let mut terminal = Terminal::start(&home, &[]);
    terminal.wait_for("Unlock private privacy key");
    terminal.send(format!("{SECRET}\r").as_bytes());
    let (output, code) = terminal.finish();
    assert_eq!(code, 1);
    assert!(plain(&output).contains(&squeezed(
        "Error: The private terminal session could not be built: The full private terminal session was refused. `craftsman privacy status` reports what a private start still needs."
    )));

    // Fixed status reports only, ending with the error verdict.
    let mut reports = Vec::new();
    let mut rest = output.as_slice();
    while let Some(start) = rest.windows(7).position(|window| window == b"\x1b]7501;") {
        let body = &rest[start + 7..];
        let end = body
            .windows(2)
            .position(|window| window == b"\x1b\\")
            .unwrap();
        reports.push(String::from_utf8(body[..end].to_vec()).unwrap());
        rest = &body[end + 2..];
    }
    // The reporter is sampled when the terminal draws. A build that is
    // refused before the first draw reports no "working" first; it was there
    // in every run alone and missing once under four parallel cases.
    let working = "state=working:app=craftsman:msg=V29ya2luZw==";
    let ending = [
        "state=clear:app=craftsman:id=craftsman/tasks",
        "state=error:app=craftsman:msg=V29yayBzdG9wcGVkIGFmdGVyIGFuIGVycm9y",
    ];
    assert!(
        reports == ending
            || (reports.len() == 3 && reports[0] == working && reports[1..] == ending),
        "{reports:?}"
    );

    // The mouse is captured only after the password prompt, and released.
    let text = String::from_utf8_lossy(&output)
        .replace("\x1b[?1000h", "<mouse-on>")
        .replace("\x1b[?1000l", "<mouse-off>");
    let text = plain(text.as_bytes());
    let prompt = text.find("privacykey").unwrap();
    let on = text.find("<mouse-on>").unwrap();
    let off = text.find("<mouse-off>").unwrap();
    assert!(prompt < on && on < off);
    assert_eq!(text.matches("<mouse-on>").count(), 1);
    assert_eq!(text.matches("<mouse-off>").count(), 1);

    assert_eq!(home.user_bytes(), user);
    assert_eq!(
        std::fs::read(home.config.join("privacy-key.v1")).unwrap(),
        key
    );
    absent_secret(&home.config);
}

#[test]
fn privacy_enable_refuses_without_changing_anything() {
    let enable = ["privacy", "enable", "--detector", "privacy-detector"];
    let refused = |home: &Home, refusal: &str| {
        assert_eq!(
            home.run(&enable),
            (format!("Error: {refusal}\n"), 1),
            "{refusal}"
        );
    };
    let home = Home::new();
    refused(
        &home,
        "No user configuration file exists; configure a model connection first. Nothing was changed.",
    );
    assert!(home.entries().is_empty());

    // It only adds: an existing policy, enabled or not, is left alone.
    for user in [
        enabled(VOICE_OFF),
        format!(r#"{{{VOICE_OFF}"privacy":{{"enabled":false,{PINS}}}}}"#),
    ] {
        let user = home.user(&user);
        refused(
            &home,
            "The user configuration already has a privacy policy; this command only adds one. Nothing was changed.",
        );
        assert_eq!(home.user_bytes(), user);
    }

    // Voice is never turned off to make the candidate pass.
    let user = home.user(r#"{"base_url":"http://127.0.0.1:9","model":"synthetic-model"}"#);
    refused(
        &home,
        "Enabled privacy requires voice.enabled:false in the user configuration; voice input has no private adapter.",
    );
    assert_eq!(home.user_bytes(), user);

    let user = home.user(
        r#"{"base_url":"http://127.0.0.1:9","model":"synthetic-model","voice":{"enabled":false},"unknown":{"kept":[1,2,3]}}"#,
    );
    #[cfg(not(feature = "gadgets"))]
    refused(
        &home,
        "The private terminal requires the source-built detector feature.",
    );
    #[cfg(feature = "gadgets")]
    {
        let not_approved = "The detector gadget is not installed under the user configuration or has no stored source approval. Nothing was changed.";
        // Not installed.
        refused(&home, not_approved);
        assert_eq!(home.entries(), ["config.json"]);
        // Installed, with the fixture's real manifest and synthetic component
        // bytes, but never approved: the pin is not written.
        let bundle = home.config.join("gadgets/privacy-detector");
        std::fs::create_dir_all(&bundle).unwrap();
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/privacy-detector/gadget.toml");
        for (name, bytes) in [
            ("gadget.toml", std::fs::read(manifest).unwrap()),
            ("gadget.wasm", b"synthetic component bytes".to_vec()),
        ] {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(bundle.join(name))
                .unwrap();
            file.write_all(&bytes).unwrap();
        }
        refused(&home, not_approved);
        assert_eq!(home.entries(), ["config.json", "gadgets"]);
        // No grant was written and no staged file was left.
        assert!(!home.config.join("gadgets/.grants.json").exists());
    }
    assert_eq!(home.user_bytes(), user);
    assert!(!home.config.join("config.json.privacy-enable").exists());
}

/// What the person types in the private conversation, and after its fresh
/// clear. Both carry synthetic contact values with outer spaces.
#[cfg(feature = "gadgets")]
const TYPED: &str = "  Handle alice@example.invalid and +886 912 345 678.  ";
#[cfg(feature = "gadgets")]
const FOLLOW: &str = "  Follow up for alice@example.invalid.  ";
/// The text of the detector call that the synthetic model asks for in the
/// ordinary session, where the detector's first use is approved.
#[cfg(feature = "gadgets")]
const CHECKED: &str = "Synthetic check for bob@example.invalid.";

#[cfg(feature = "gadgets")]
fn sha256(parts: &[&[u8]]) -> String {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    for part in parts {
        digest.update(part);
    }
    format!("{:x}", digest.finalize())
}

/// The detector's one-node request document for `text`, with the bindings of
/// the detector protocol, as a model that calls the detector would send it.
#[cfg(feature = "gadgets")]
fn detector_request(text: &str) -> serde_json::Value {
    let id = "t/0";
    let node = sha256(&[
        b"craftsman/privacy-detector/node/v1\0",
        &(id.len() as u64).to_le_bytes(),
        id.as_bytes(),
        &(text.len() as u64).to_le_bytes(),
        text.as_bytes(),
    ]);
    let batch = sha256(&[
        b"craftsman/privacy-detector/batch/v1\0",
        &1u64.to_le_bytes(),
        node.as_bytes(),
    ]);
    serde_json::json!({"version":1,"input_binding":batch,
        "nodes":[{"id":id,"text":text,"input_binding":node}]})
}

/// What the synthetic model answers in the private conversation of a case.
#[cfg(feature = "gadgets")]
#[derive(Clone, Copy)]
enum Script {
    /// A reply, and after the fresh clear another.
    Clear,
    /// A question, an ordinary note written to a file, a reply, a follow-up.
    Safe,
    /// A question, a contact written to a data file, a reply, a follow-up.
    Full,
}

/// The synthetic model. It answers by what the request actually carries:
/// - a private request, which holds an issued alias: the next step of the
///   case's script, echoing that alias as it is in the request and never
///   rebuilt here;
/// - an ordinary request after a tool result: a closing sentence;
/// - an ordinary first request: one call of the detector gadget.
#[cfg(feature = "gadgets")]
struct Model {
    script: Script,
    private: std::sync::atomic::AtomicUsize,
}
#[cfg(feature = "gadgets")]
fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"tool_calls":[{"index":0,"id":id,"type":"function",
        "function":{"name":name,"arguments":arguments.to_string()}}]})
}
#[cfg(feature = "gadgets")]
impl wiremock::Respond for Model {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let value: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let messages = value["messages"].as_array().unwrap();
        let aliased = messages.iter().rev().find_map(|message| {
            let content = message["content"].as_str()?;
            (message["role"] == "user" && content.contains("@privacy.invalid")).then_some(content)
        });
        let delta = if let Some(user) = aliased {
            let end = user.find("@privacy.invalid").unwrap() + "@privacy.invalid".len();
            let alias = &user[user[..end].rfind("pv1-").unwrap()..end];
            let index = self
                .private
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let say = |content: String| serde_json::json!({ "content": content });
            match (self.script, index) {
                (Script::Clear, 0) => say(format!("Clear-ready reply for {alias}.")),
                (Script::Clear, _) => say(format!("Fresh reply for {alias}.")),
                (_, 0) => tool_call(
                    "ask-human",
                    "ask_user",
                    serde_json::json!({"question":"Please confirm delivery."}),
                ),
                (Script::Safe, 1) => tool_call(
                    "write-ordinary-note",
                    "write_file",
                    serde_json::json!({"file_path":"answer.txt","content":"delivered note"}),
                ),
                (_, 1) => tool_call(
                    "write-real-data",
                    "write_file",
                    serde_json::json!({"file_path":"answer.txt","content":format!("Contact delivered {alias}")}),
                ),
                (_, 2) => say(format!("Delivered for {alias}.")),
                _ => say(format!("Follow-up acknowledged for {alias}.")),
            }
        } else if messages.iter().any(|message| message["role"] == "tool") {
            serde_json::json!({"content":"Detector checked."})
        } else {
            tool_call(
                "call-detector",
                "gadget_call",
                serde_json::json!({"gadget":"privacy-detector","function":"detect",
                    "args":detector_request(CHECKED)}),
            )
        };
        let finish = match delta.get("tool_calls") {
            Some(_) => "tool_calls",
            None => "stop",
        };
        let event = serde_json::json!({"choices":[{"delta":delta,"finish_reason":finish}],
            "usage":{"prompt_tokens":3,"completion_tokens":2}});
        wiremock::ResponseTemplate::new(200).set_body_raw(
            format!("data: {event}\n\ndata: [DONE]\n\n"),
            "text/event-stream",
        )
    }
}

/// Builds the detector fixture from its source into a directory of this
/// case, installs it under the synthetic user root and returns the SHA-256 of
/// its component and of its manifest. No grant is written here.
#[cfg(feature = "gadgets")]
fn install_detector(home: &Home) -> (String, String) {
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/privacy-detector");
    let target = home.root.join("detector-source-build");
    let mut build = Command::new(env!("CARGO"))
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
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = build.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = build.kill();
            let _ = build.wait();
            panic!("the detector source build exceeded its bound");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.success(), "the detector source build failed");
    let component =
        std::fs::read(target.join("wasm32-wasip2/release/privacy_detector_fixture.wasm")).unwrap();
    let manifest = std::fs::read(source.join("gadget.toml")).unwrap();
    let bundle = home.config.join("gadgets/privacy-detector");
    std::fs::create_dir_all(&bundle).unwrap();
    for (name, bytes) in [("gadget.wasm", &component), ("gadget.toml", &manifest)] {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(bundle.join(name))
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }
    (sha256(&[&component]), sha256(&[&manifest]))
}

/// A synthetic home whose detector was approved at the real prompt.
#[cfg(feature = "gadgets")]
struct Approved {
    runtime: tokio::runtime::Runtime,
    server: wiremock::MockServer,
    home: Home,
    /// SHA-256 of the installed component and of its manifest.
    component: String,
    manifest: String,
    /// The user file of the ordinary session, without a privacy policy.
    user: Vec<u8>,
    /// The grant file as the runtime stored it.
    stored: Vec<u8>,
}

/// Starts this case's loopback model, installs the detector, and runs the
/// ordinary terminal once: the synthetic model calls the detector gadget and
/// the person approves its first use at the real prompt, which names the
/// gadget and the hash of its installed bytes. The runtime stores that
/// approval itself. Nothing here writes a grant file or supplies an approval
/// state, and the only endpoint, key and model are this case's own.
#[cfg(feature = "gadgets")]
fn approved(script: Script) -> Approved {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(async {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(Model {
                script,
                private: std::sync::atomic::AtomicUsize::new(0),
            })
            .mount(&server)
            .await;
        server
    });
    let home = Home::new();
    let (component, manifest) = install_detector(&home);
    let user = home.user(&format!(
        r#"{{"base_url":"{}","api_key":"fixture-key","model":"fixture-model","harness_profile":"general","voice":{{"enabled":false}},"gadgets":{{"enabled":true}},"mcp_servers":[],"lsp_servers":{{}},"unknown":{{"kept":[1,2,3]}}}}"#,
        server.uri()
    ));
    let grants = home.config.join("gadgets/.grants.json");
    assert!(!grants.exists());
    let mut terminal = Terminal::start(&home, &[]);
    terminal.wait_for("message · Ctrl+J newline");
    terminal.send(b"Check the detector.\r");
    let mut approvals = 0;
    while terminal.wait_for_any(&["Detector checked.", "Approve once"]) == 1 {
        let prompt = screen(&terminal.output);
        assert!(prompt.contains("approval required"));
        assert!(prompt.contains("Approve gadget privacy-detector (0.1.0)"));
        assert!(prompt.contains(&format!("Component SHA-256: {component}")));
        approvals += 1;
        terminal.send(b"\r");
        terminal.wait_until_gone("Approve once");
    }
    assert_eq!(approvals, 1, "one first-use prompt, answered once");
    terminal.send(b"/quit\r");
    let (_, code) = terminal.finish();
    assert_eq!(code, 0);
    // The stored approval is of this gadget, at its installed source, with
    // exactly the bytes that were shown.
    let stored = std::fs::read(&grants).unwrap();
    let grant: serde_json::Value = serde_json::from_slice(&stored).unwrap();
    let bundle = home
        .config
        .join("gadgets/privacy-detector")
        .canonicalize()
        .unwrap();
    assert_eq!(grant["entries"].as_array().unwrap().len(), 1);
    assert_eq!(grant["entries"][0]["id"], "privacy-detector");
    assert_eq!(grant["entries"][0]["source"], bundle.to_str().unwrap());
    assert_eq!(grant["entries"][0]["component_hash"], component.as_str());
    assert_eq!(home.user_bytes(), user);
    assert_eq!(
        runtime.block_on(server.received_requests()).unwrap().len(),
        2
    );
    Approved {
        runtime,
        server,
        home,
        component,
        manifest,
        user,
        stored,
    }
}

#[cfg(feature = "gadgets")]
impl Approved {
    /// The user's own edit of the file: a policy that pins the installed
    /// detector and names one data file of the workspace, as the conversation
    /// cases need it. Then the create-only key. Returns the authored file.
    fn author_policy(&self, application: Option<&str>) -> Vec<u8> {
        let mut user: serde_json::Value = serde_json::from_slice(&self.user).unwrap();
        let workspace = self.home.workspace.to_str().unwrap();
        user["privacy"] = serde_json::json!({"enabled":true,"detector":{
            "gadget":"privacy-detector","function":"detect",
            "component_sha256":self.component,"manifest_sha256":self.manifest},
            "file_data":[{"workspace":workspace,"destination":format!("{workspace}/answer.txt"),
                "format":"plain_text","operations":["write_file","edit_file"]}]});
        if let Some(application) = application {
            user["privacy"]["file_data"][0]["application"] = application.into();
        }
        let user = self.home.user(&user.to_string());
        initialize(&self.home);
        user
    }
    /// The requests of the private conversation: everything after the two
    /// of the ordinary session.
    fn private_requests(&self) -> Vec<wiremock::Request> {
        let mut requests = self
            .runtime
            .block_on(self.server.received_requests())
            .unwrap();
        requests.drain(..2);
        assert_masked(&requests);
        requests
    }
    /// The recorded states of the private threads: every thread but the
    /// ordinary session's.
    fn private_threads(&self) -> Vec<serde_json::Value> {
        let mut threads = thread_states(&self.runtime, &self.home);
        let ordinary = threads
            .iter()
            .position(|state| typed_inputs(state) == ["Check the detector."])
            .unwrap();
        threads.remove(ordinary);
        threads
    }
    /// The user file, the key and the stored approval are what they were.
    fn assert_unchanged(&self, user: &[u8], key: &[u8]) {
        assert_eq!(self.home.user_bytes(), user);
        assert_eq!(
            std::fs::read(self.home.config.join("privacy-key.v1")).unwrap(),
            key
        );
        assert_eq!(
            std::fs::read(self.home.config.join("gadgets/.grants.json")).unwrap(),
            self.stored
        );
        absent_secret(&self.home.config);
    }
    fn key(&self) -> Vec<u8> {
        std::fs::read(self.home.config.join("privacy-key.v1")).unwrap()
    }
}

/// The whole explicit setup through the real binary, then a private
/// conversation with the policy that `enable` wrote:
///
/// 1. the ordinary terminal and the real first-use prompt (`approved`);
/// 2. `craftsman privacy enable`, whose candidate keeps every original byte
///    and pins exactly the installed component and manifest;
/// 3. `craftsman privacy status` before and after `craftsman privacy init`;
/// 4. the private terminal: unlock, a masked turn, a fresh clear with a new
///    unlock and thread, a second masked turn.
#[cfg(feature = "gadgets")]
#[test]
fn privacy_enable_writes_the_policy_after_a_real_approval_and_the_private_terminal_runs() {
    let setup = approved(Script::Clear);
    let Approved {
        runtime,
        home,
        component,
        manifest,
        user,
        stored,
        ..
    } = &setup;
    let grants = home.config.join("gadgets/.grants.json");

    // The explicit enable command. The key does not exist yet.
    let enable = ["privacy", "enable", "--detector", "privacy-detector"];
    // A leftover with the staging name is not this command's file: it is
    // neither adopted nor overwritten nor removed, and nothing is written.
    let leftover = home.config.join("config.json.privacy-enable");
    std::fs::write(&leftover, b"foreign leftover").unwrap();
    let (output, code) = home.run(&enable);
    assert_eq!(code, 1);
    assert!(
        output.ends_with("The private key operation did not complete its IO.\n"),
        "{output}"
    );
    assert_eq!(std::fs::read(&leftover).unwrap(), b"foreign leftover");
    assert_eq!(&home.user_bytes(), user);
    std::fs::remove_file(&leftover).unwrap();
    assert_eq!(
        home.run(&enable),
        (
            "A privacy policy was added to your user configuration. `craftsman privacy status` shows what a start still needs.\n"
                .to_owned(),
            0
        )
    );
    let written = home.user_bytes();
    // Every original byte is where it was; one member follows the brace.
    assert!(written.starts_with(b"{\"privacy\":"));
    assert!(written.ends_with(&user[1..]));
    let member = &written[b"{\"privacy\":".len()..written.len() - user.len()];
    let policy: serde_json::Value = serde_json::from_slice(member).unwrap();
    assert_eq!(written[written.len() - user.len()], b',');
    assert_eq!(
        policy,
        serde_json::json!({"enabled":true,"identifiers":[],"detector":{
            "gadget":"privacy-detector","function":"detect",
            "component_sha256":component.as_str(),"manifest_sha256":manifest.as_str()}})
    );
    // Every other member has its value: endpoint, key, model and the
    // member this program does not know.
    let before: serde_json::Value = serde_json::from_slice(user).unwrap();
    let mut after: serde_json::Value = serde_json::from_slice(&written).unwrap();
    after.as_object_mut().unwrap().remove("privacy").unwrap();
    assert_eq!(after, before);
    assert_eq!(after["unknown"], serde_json::json!({"kept":[1,2,3]}));
    // The command wrote no grant, left no staged file and kept the mode.
    assert_eq!(&std::fs::read(&grants).unwrap(), stored);
    assert!(!home.config.join("config.json.privacy-enable").exists());
    assert_eq!(
        std::fs::metadata(home.config.join("config.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    // It only adds: a second run refuses and changes nothing.
    assert_eq!(
        home.run(&enable),
        (
            "Error: The user configuration already has a privacy policy; this command only adds one. Nothing was changed.\n"
                .to_owned(),
            1
        )
    );
    assert_eq!(home.user_bytes(), written);

    // The status before and after the key exists.
    let enabled_line = "Privacy policy: enabled in the user configuration.";
    assert_eq!(
        home.run(&["privacy", "status"]),
        (
            format!(
                "Mapping-vault key: not created. `craftsman privacy init` creates it.\n{enabled_line}\nInteractive startup: refused. No mapping-vault key exists yet. Run `craftsman privacy init` first; nothing was unlocked.\n"
            ),
            0
        )
    );
    initialize(home);
    let key = std::fs::read(home.config.join("privacy-key.v1")).unwrap();
    assert_eq!(
        home.run(&["privacy", "status"]),
        (
            format!(
                "Mapping-vault key: files present. Presence is not an unlock.\n{enabled_line}\n{PREFLIGHT_PASSED}\n"
            ),
            0
        )
    );

    // The private terminal with the written policy.
    let mut terminal = Terminal::start(home, &[]);
    terminal.unlock();
    terminal.send(format!("{TYPED}\r").as_bytes());
    terminal.wait_for("Clear-ready reply for alice@example.invalid.");
    terminal.send(b"/clear\r");
    terminal.unlock();
    terminal.send(format!("{FOLLOW}\r").as_bytes());
    terminal.wait_for("Fresh reply for alice@example.invalid.");
    terminal.send(b"/quit\r");
    let (output, code) = terminal.finish();
    assert_eq!(code, 0);
    assert_fixed_reports(&output);
    setup.assert_unchanged(&written, &key);
    assert_eq!(setup.private_requests().len(), 2);
    // Three threads: the ordinary session, the private conversation and
    // the fresh one after the clear. Each keeps what the person typed, byte
    // for byte: the local records are plaintext, only the vault is not.
    let threads = thread_states(runtime, home);
    assert_eq!(threads.len(), 3);
    let typed: Vec<Vec<String>> = threads.iter().map(typed_inputs).collect();
    assert!(typed.contains(&vec!["Check the detector.".to_owned()]));
    assert!(typed.contains(&vec![TYPED.to_owned()]));
    assert!(typed.contains(&vec![FOLLOW.to_owned()]));
}

/// Every status report in `output` is built of the reporter's fixed fields
/// only: a state, the program's name, for a wait its kind, and either one of
/// the fixed messages or the owned task namespace. The first says working and
/// the last is the final verdict after that namespace was cleared. No name,
/// prompt or model text is in any of them.
#[cfg(feature = "gadgets")]
fn assert_fixed_reports(output: &[u8]) {
    let mut reports = Vec::new();
    let mut rest = output;
    while let Some(start) = rest.windows(7).position(|window| window == b"\x1b]7501;") {
        let body = &rest[start + 7..];
        let end = body
            .windows(2)
            .position(|window| window == b"\x1b\\")
            .unwrap();
        reports.push(String::from_utf8(body[..end].to_vec()).unwrap());
        rest = &body[end + 2..];
    }
    // "Working", "Ready", "Response ready", "Waiting for approval" and
    // "Waiting for an answer", in the reporter's encoding.
    let fields = [
        "state=working",
        "state=idle",
        "state=done",
        "state=blocked",
        "state=clear",
        "app=craftsman",
        "kind=permission",
        "kind=question",
        "msg=V29ya2luZw==",
        "msg=UmVhZHk=",
        "msg=UmVzcG9uc2UgcmVhZHk=",
        "msg=V2FpdGluZyBmb3IgYXBwcm92YWw=",
        "msg=V2FpdGluZyBmb3IgYW4gYW5zd2Vy",
        "id=craftsman/tasks",
    ];
    for report in &reports {
        assert!(
            report.split(':').all(|field| fields.contains(&field)),
            "{report}"
        );
    }
    assert_eq!(
        reports.first().map(String::as_str),
        Some("state=working:app=craftsman:msg=V29ya2luZw==")
    );
    assert!(reports.len() >= 4);
    assert_eq!(
        reports[reports.len() - 2],
        "state=clear:app=craftsman:id=craftsman/tasks"
    );
    assert_eq!(
        reports[reports.len() - 1],
        "state=done:app=craftsman:msg=UmVzcG9uc2UgcmVhZHk="
    );
}

/// Each private request carries an issued alias and none of the values the
/// person typed, and never the passphrase.
#[cfg(feature = "gadgets")]
fn assert_masked(requests: &[wiremock::Request]) {
    for request in requests {
        let body = std::str::from_utf8(&request.body).unwrap();
        assert!(body.contains("@privacy.invalid"));
        assert!(!body.contains("alice@example.invalid"));
        assert!(!body.contains("+886 912 345 678"));
        assert!(!body.contains(SECRET));
    }
}

/// The latest recorded state of every thread in the synthetic home's store,
/// read with the store's own engine and nothing of the program.
#[cfg(feature = "gadgets")]
fn thread_states(runtime: &tokio::runtime::Runtime, home: &Home) -> Vec<serde_json::Value> {
    runtime.block_on(async {
        let database = libsql::Builder::new_local(home.config.join("threads.db"))
            .build()
            .await
            .unwrap();
        let mut rows = database
            .connect()
            .unwrap()
            .query(
                "SELECT (SELECT c.state_json FROM checkpoints c WHERE c.thread_id = t.id ORDER BY c.seq DESC LIMIT 1) FROM threads t ORDER BY t.id",
                (),
            )
            .await
            .unwrap();
        let mut states = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let state = row.get::<String>(0).unwrap();
            assert!(!state.contains(SECRET));
            states.push(serde_json::from_str(&state).unwrap());
        }
        states
    })
}

/// What the person submitted in a recorded state, in order: its user-role
/// messages without the runtime's own catalog notes.
#[cfg(feature = "gadgets")]
fn typed_inputs(state: &serde_json::Value) -> Vec<String> {
    state["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_str())
        .filter(|content| !content.starts_with("<gadget_catalog_update sha256=\""))
        .map(str::to_owned)
        .collect()
}

// The five conversation cases of the interactive route. They were cases of
// the unit-test binary, where a test child called the router; under the same
// names they now drive the real binary through its own `main`, after the
// detector was approved at the real prompt. Each keeps its stages, its
// request count and masking checks, and what its child checked afterwards:
// the exit code, restored terminal modes, unchanged user file and key, the
// written file, and the recorded threads and inputs.

/// A conversation with one fresh clear in it, up to the second password
/// prompt. Returns the terminal there.
#[cfg(feature = "gadgets")]
fn conversation_until_clear(setup: &Approved, clear: &[u8]) -> Terminal {
    let mut terminal = Terminal::start(&setup.home, &[]);
    terminal.unlock();
    terminal.send(format!("{TYPED}\r").as_bytes());
    terminal.wait_for("Clear-ready reply for alice@example.invalid.");
    terminal.send(clear);
    terminal.wait_for("Unlock private privacy key");
    terminal
}

#[cfg(feature = "gadgets")]
#[test]
fn actual_normal_fresh_clear_reacquires_owned_secret_and_rebuilds_with_new_thread() {
    let setup = approved(Script::Clear);
    let user = setup.author_policy(None);
    let key = setup.key();
    let mut terminal = conversation_until_clear(&setup, b"/clear\r");
    terminal.unlock();
    terminal.send(format!("{FOLLOW}\r").as_bytes());
    terminal.wait_for("Fresh reply for alice@example.invalid.");
    terminal.send(b"/quit\r");
    let (output, code) = terminal.finish();
    assert_eq!(code, 0);
    assert_fixed_reports(&output);
    setup.assert_unchanged(&user, &key);
    assert_eq!(setup.private_requests().len(), 2);
    assert!(!setup.home.workspace.join("answer.txt").exists());
    // Two threads, the old one and the fresh one, each with its own input.
    let typed: Vec<Vec<String>> = setup.private_threads().iter().map(typed_inputs).collect();
    assert_eq!(typed.len(), 2);
    assert!(typed.contains(&vec![TYPED.to_owned()]));
    assert!(typed.contains(&vec![FOLLOW.to_owned()]));
}

#[cfg(feature = "gadgets")]
#[test]
fn actual_normal_fresh_clear_cancellation_ends_the_old_owner_without_a_new_request() {
    for cancel in [b"\x1b".as_slice(), b"\x03".as_slice()] {
        let setup = approved(Script::Clear);
        let user = setup.author_policy(None);
        let key = setup.key();
        let mut terminal = conversation_until_clear(&setup, b"/clear\r");
        terminal.send(cancel);
        let (output, code) = terminal.finish();
        assert_eq!(code, 1);
        assert!(plain(&output).contains(&squeezed(
            "Error: The private terminal bootstrap stopped: The private bootstrap key operation stopped: The private key operation was cancelled."
        )));
        setup.assert_unchanged(&user, &key);
        assert_eq!(setup.private_requests().len(), 1);
        assert!(!setup.home.workspace.join("answer.txt").exists());
        let typed: Vec<Vec<String>> = setup.private_threads().iter().map(typed_inputs).collect();
        assert_eq!(typed, [vec![TYPED.to_owned()]]);
    }
}

#[cfg(feature = "gadgets")]
#[test]
fn actual_normal_fresh_clear_preserves_decoded_ordinary_type_ahead_as_a_draft() {
    let setup = approved(Script::Clear);
    let user = setup.author_policy(None);
    let key = setup.key();
    // Ordinary decoded input sent with the clear command, before the
    // terminal asks for another password. It is not secret type-ahead.
    let mut terminal = conversation_until_clear(&setup, b"/clear\r  Follow up");
    terminal.unlock();
    terminal.wait_for("  Follow up");
    terminal.send(b" for alice@example.invalid.  \r");
    terminal.wait_for("Fresh reply for alice@example.invalid.");
    terminal.send(b"/quit\r");
    let (output, code) = terminal.finish();
    assert_eq!(code, 0);
    assert_fixed_reports(&output);
    setup.assert_unchanged(&user, &key);
    assert_eq!(setup.private_requests().len(), 2);
    let typed: Vec<Vec<String>> = setup.private_threads().iter().map(typed_inputs).collect();
    assert_eq!(typed.len(), 2);
    assert!(typed.contains(&vec![TYPED.to_owned()]));
    assert!(typed.contains(&vec![FOLLOW.to_owned()]));
}

/// A question, a reviewed file write, a refused settings change and a refused
/// command, then a follow-up; and afterwards what the entry's child checked.
#[cfg(feature = "gadgets")]
fn entry_conversation(script: Script, application: Option<&str>, review: &str, file: &str) {
    let setup = approved(script);
    let user = setup.author_policy(application);
    let key = setup.key();
    let mut terminal = Terminal::start(&setup.home, &[]);
    terminal.unlock();
    terminal.send(format!("{TYPED}\r").as_bytes());
    terminal.wait_for("Please confirm delivery.");
    terminal.send(b"yes, deliver\r");
    terminal.wait_for(review);
    terminal.send(b"\x1b[F");
    terminal.wait_for(match script {
        Script::Full => "delivered alice@example.invalid",
        _ => "delivered note",
    });
    terminal.send(b"\r");
    terminal.wait_for("Delivered for alice@example.invalid.");
    terminal.send(b"\x1b[Z");
    terminal.wait_for("Settings cannot be saved");
    terminal.send(b"/reopen\r");
    terminal.wait_for("This command has no reviewed private entry adapter");
    if matches!(script, Script::Safe) {
        // Read-only local commands answer through their ordinary owners.
        // Leading spaces do not make one a prompt, and the unchanged record
        // and request counts below show that neither reached the model.
        terminal.send(b"/version\r");
        terminal.wait_for(concat!("craftsman ", env!("CARGO_PKG_VERSION")));
        terminal.send(b"   /status\r");
        terminal.wait_for("model: fixture-model");
        // Inspection of this session and its registry, and a cancellation
        // that finds no such running task: the ordinary lines, no request.
        terminal.send(b"/mcp\r");
        terminal.wait_for("(no MCP servers)");
        terminal.send(b"/tasks\r");
        terminal.wait_for("(no subagent tasks)");
        terminal.send(b"/tasks cancel nothing\r");
        terminal.wait_for("Task nothing is not a cancellable running background task");
        terminal.send(b"/gadgets\r");
        terminal.wait_for("privacy-detector 0.1.0");
        terminal.send(b"/checkpoints\r");
        terminal.wait_for("1 file(s)");
        terminal.send(b"/skills\r");
        terminal.wait_for("(no local skills found)");
        // Listings that wait for their owner arrive as returned work.
        terminal.send(b"/memories\r");
        terminal.wait_for("(no memories yet)");
        terminal.send(b"/lsp\r");
        terminal.wait_for("gopls");
        terminal.send(b"/diff\r");
        terminal.wait_for("workspace diff");
        // The agent picker is shown; choosing in it would be a settings
        // change. Escape closes it.
        terminal.send(b"/agents\r");
        terminal.wait_for("Select agent");
        terminal.send(b"\x1b");
        terminal.wait_until_gone("Select agent");
    }
    terminal.send(format!("{FOLLOW}\r").as_bytes());
    terminal.wait_for("Follow-up acknowledged for alice@example.invalid.");
    terminal.send(b"/quit\r");
    let (output, code) = terminal.finish();
    assert_eq!(code, 0);
    assert_fixed_reports(&output);
    setup.assert_unchanged(&user, &key);
    assert_eq!(setup.private_requests().len(), 4);
    assert_eq!(
        std::fs::read_to_string(setup.home.workspace.join("answer.txt")).unwrap(),
        file
    );
    // One private thread. The runtime's catalog note is its own user-role
    // record; both submissions are byte exact and in order around it.
    let threads = setup.private_threads();
    assert_eq!(threads.len(), 1);
    let messages = threads[0]["messages"].as_array().unwrap();
    let users: Vec<&str> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| message["content"].as_str().unwrap())
        .collect();
    assert_eq!(users.len(), 3);
    assert_eq!(users[0], TYPED);
    assert!(users[1].starts_with("<gadget_catalog_update sha256=\""));
    assert!(users[1].contains("Runtime-discovered gadget metadata, not a user request."));
    assert_eq!(users[2], FOLLOW);
    let tools: Vec<&serde_json::Value> = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect();
    assert_eq!(tools.len(), 2);
    assert!(
        tools.iter().all(|message| message["is_error"] == false),
        "every actual requested tool must succeed"
    );
    assert!(tools.iter().any(|message| {
        message["content"]
            .as_str()
            .is_some_and(|content| content.contains("yes, deliver"))
    }));
}

#[cfg(feature = "gadgets")]
#[test]
fn actual_normal_private_terminal_question_review_and_save_refusal_without_restoration_grant() {
    entry_conversation(
        Script::Safe,
        None,
        "private operation review",
        "delivered note",
    );
}

#[cfg(feature = "gadgets")]
#[test]
fn actual_normal_private_entry_owns_human_review_question_file_and_checkpoint_without_password() {
    entry_conversation(
        Script::Full,
        Some("contact_lines_v1"),
        "private file review",
        "Contact delivered alice@example.invalid",
    );
}
