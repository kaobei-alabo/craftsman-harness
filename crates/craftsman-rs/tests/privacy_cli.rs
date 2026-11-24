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
                return;
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
    assert_eq!(
        reports,
        [
            "state=working:app=craftsman:msg=V29ya2luZw==",
            "state=clear:app=craftsman:id=craftsman/tasks",
            "state=error:app=craftsman:msg=V29yayBzdG9wcGVkIGFmdGVyIGFuIGVycm9y",
        ]
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
