//! Small host-OS boundary for filesystem security, disk statistics, and shell
//! process construction. The agent/runtime stays platform-neutral; only this
//! module knows about Unix modes, `statvfs`, PowerShell, or Win32 APIs.

use std::path::Path;
use std::process::Stdio;
#[cfg(unix)]
use tokio::process::Command;

pub fn apply_config_permissions(dir: &Path, file: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        // Files created below the user's profile inherit its ACL. Explicit DACL
        // hardening can be layered here later without leaking Win32 details into
        // config.rs.
        let _ = (dir, file);
    }
    Ok(())
}

/// Handle-relative, no-follow file opens for the confined workspace backend.
/// Nonblocking Unix opens let the caller reject a swapped-in FIFO by fstat
/// without first hanging a blocking worker. New files are private on Unix;
/// Windows uses the directory's inherited ACL (P2d hardens this separately).
pub(crate) fn confined_file_options(write: bool, create_new: bool) -> cap_std::fs::OpenOptions {
    use cap_fs_ext::OpenOptionsFollowExt;
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .read(true)
        .write(write)
        .create_new(create_new)
        .follow(cap_fs_ext::FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_fs_ext::OpenOptionsSyncExt;
        use cap_std::fs::OpenOptionsExt;
        options.nonblock(true).mode(0o600);
    }
    options
}

/// Create a workspace parent directory relative to retained authority. Unix
/// directories are private from their creation; Windows inherits the ACL.
pub(crate) fn confined_create_dir(directory: &cap_std::fs::Dir, name: &str) -> std::io::Result<()> {
    let mut builder = cap_std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use cap_std::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    #[cfg(not(unix))]
    let _ = &mut builder;
    directory.create_dir_with(name, &builder)
}

/// Preserve ordinary executable/read/write bits on an edit, never special Unix
/// set-id bits. Other platforms retain the original file's permissions.
pub(crate) fn confined_replace_permissions(
    mut permissions: cap_std::fs::Permissions,
) -> cap_std::fs::Permissions {
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() & 0o777);
    }
    #[cfg(not(unix))]
    let _ = &mut permissions;
    permissions
}

/// How [`ProcessTree::spawn_shell`] starts a command.
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// Unix only: start the shell as the leader of a process group of its own,
    /// so a timeout, a drop or the `-m` signal hook can SIGKILL everything it
    /// forked. Config `shell.process_group`, OFF by default until P2c: with a
    /// group of its own the command leaves the terminal's foreground group,
    /// so without signal routing an interrupted `craftsman -m` (SIGINT) or a
    /// closed terminal (SIGHUP to the TUI) would leave the whole command
    /// running, both of which are clean today (docs/spikes/p0-canceltoken.md,
    /// section 4). Off means today's behaviour: the shell stays in craftsman's
    /// group and only the direct child is killed. Ignored on Windows, where
    /// the Job Object is always used: the job is closed, and every process
    /// in it killed, when craftsman exits for any reason.
    pub process_group: bool,
    /// Variables removed from the inherited environment (the scrub of
    /// `shell.rs`; empty means the full environment is inherited).
    pub env_remove: Vec<std::ffi::OsString>,
    /// Variables set for the command (the scrub's hints).
    pub env_set: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    /// Windows only (`shell.powershell_utf8`): see [`powershell_command`].
    #[cfg_attr(not(windows), allow(dead_code))]
    pub powershell_utf8: bool,
}

/// The PowerShell UTF-8 prefix. It must come first in the `-Command`
/// string, before anything prints (docs/spikes/p0-windows-jobobject-conpty.md
/// 3.6): a `CREATE_NO_WINDOW` child starts at the OEM code page.
#[cfg_attr(not(windows), allow(dead_code))]
pub const POWERSHELL_UTF8_PREFIX: &str = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; ";

/// The `-Command` string PowerShell receives for `command`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn powershell_command(command: &str, utf8: bool) -> String {
    if utf8 {
        format!("{POWERSHELL_UTF8_PREFIX}{command}")
    } else {
        command.to_string()
    }
}

/// The process tree of one shell command: the only place in Craftsman that knows
/// about process groups and Job Objects.
///
/// - **Unix**: `sh -c`, `kill_on_drop`, and with [`SpawnOptions::process_group`]
///   a group of its own (`process_group(0)`) that [`ProcessTree::kill_tree`] and
///   `Drop` SIGKILL while the group leader is not yet reaped (after the leader
///   has been reaped its pid, and so the group id, may be reused, so the guard
///   disarms then). Without the flag only the direct child is killed (today).
/// - **Windows**: `powershell.exe -NoLogo -NoProfile -NonInteractive -Command`
///   inside a Job Object with kill-on-close, through `process-wrap` (pinned).
///   The four rules of the P0 spike (docs/spikes/p0-windows-jobobject-conpty.md
///   5.1) are kept here: (1) register the `CreationFlags`, `JobObject` and
///   `KillOnDrop` shims and never set creation flags or kill-on-drop on the
///   `Command` itself (`JobObject` would overwrite them); (2) on timeout or
///   cancel call `kill()` (it terminates the job) and then read the pipes
///   with a short deadline; (3) never rely on the wrapper's `wait()` to wait
///   for the whole job: [`ProcessTree::wait`] waits for the direct child
///   only, and dropping the tree closes the job, which kills the rest;
///   (4) never call `wait()` after `try_wait()`: this type never calls
///   `try_wait()` at all.
pub struct ProcessTree {
    #[cfg(unix)]
    child: tokio::process::Child,
    /// The group to SIGKILL: `Some` only while the leader is unreaped.
    #[cfg(unix)]
    group: Option<i32>,
    #[cfg(windows)]
    child: Box<dyn process_wrap::tokio::ChildWrapper>,
}

impl ProcessTree {
    /// Start `command` through the host shell in `cwd` with stdin null and
    /// stdout/stderr piped.
    pub fn spawn_shell(command: &str, cwd: &Path, options: &SpawnOptions) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            let mut process = Command::new("sh");
            process
                .arg("-c")
                .arg(command)
                .current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            for name in &options.env_remove {
                process.env_remove(name);
            }
            process.envs(options.env_set.iter().map(|(k, v)| (k, v)));
            if options.process_group {
                process.process_group(0);
            }
            let child = process.spawn()?;
            // With process_group(0) the group id is the leader's pid. Never
            // signal group 0 or 1 (our own group, init's).
            let group = match (options.process_group, child.id()) {
                (true, Some(pid)) if pid > 1 => Some(pid as i32),
                _ => None,
            };
            if let Some(pgid) = group {
                live_groups::insert(pgid);
            }
            Ok(Self { child, group })
        }
        #[cfg(windows)]
        {
            let mut wrap = windows_command_wrap(command, cwd, options);
            let child = wrap.spawn()?;
            Ok(Self { child })
        }
    }

    pub fn take_stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        #[cfg(unix)]
        {
            self.child.stdout.take()
        }
        #[cfg(windows)]
        {
            self.child.stdout().take()
        }
    }

    pub fn take_stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        #[cfg(unix)]
        {
            self.child.stderr.take()
        }
        #[cfg(windows)]
        {
            self.child.stderr().take()
        }
    }

    /// The direct child's pid.
    #[cfg(all(unix, test))]
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// The process group this tree kills (Unix, `process_group` on).
    #[cfg(all(unix, test))]
    pub fn process_group_id(&self) -> Option<i32> {
        self.group
    }

    /// Wait for the direct child (the shell) to exit and return its status.
    /// Descendants that outlive it are not waited for: on Unix with a group
    /// they are left alone once the leader is reaped (the guard disarms,
    /// see the type docs), on Windows they die when the tree is dropped and
    /// the job is closed.
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        #[cfg(unix)]
        {
            let status = self.child.wait().await;
            if status.is_ok() {
                self.disarm();
            }
            status
        }
        #[cfg(windows)]
        {
            // Rule 3: the inner tokio child, not the job wrapper's wait().
            self.child.inner_mut().wait().await
        }
    }

    /// Kill the tree (timeout or cancel) and reap the direct child: the whole
    /// group (Unix with the flag on) or the whole job (Windows); without the
    /// flag on Unix, the direct child only, as today. Bounded: returns
    /// within a few seconds even if a process refuses to go.
    pub async fn kill_tree(&mut self) {
        const REAP: std::time::Duration = std::time::Duration::from_secs(5);
        #[cfg(unix)]
        {
            // Kill the group BEFORE reaping the leader, so the group id cannot
            // have been reused yet.
            if let Some(pgid) = self.group {
                kill_group(pgid);
            }
            let _ = self.child.start_kill();
            if let Ok(Ok(_)) = tokio::time::timeout(REAP, self.child.wait()).await {
                self.disarm();
            }
        }
        #[cfg(windows)]
        {
            // Rule 2: kill() terminates the job (start_kill) and then waits;
            // the caller reads the pipes afterwards with a short deadline.
            let _ = tokio::time::timeout(REAP, Box::into_pin(self.child.kill())).await;
        }
    }

    #[cfg(unix)]
    fn disarm(&mut self) {
        if let Some(pgid) = self.group.take() {
            live_groups::remove(pgid);
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        // Unix: the guard. A call abandoned mid-run (an aborted turn, a
        // dropped future) still kills its whole group. `kill_on_drop` then
        // takes the direct child. Windows: dropping `child` closes the job
        // handle, and kill-on-close (the `KillOnDrop` shim) kills the job.
        #[cfg(unix)]
        if let Some(pgid) = self.group.take() {
            kill_group(pgid);
            live_groups::remove(pgid);
        }
    }
}

#[cfg(unix)]
fn kill_group(pgid: i32) {
    if pgid > 1 {
        // SAFETY: a plain syscall. The group id is the pid of a leader that
        // has not been reaped, so it cannot belong to anyone else.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

/// SIGKILL every process group a live [`ProcessTree`] owns. The `-m` signal
/// hook calls this before exiting (only when `shell.process_group` is on).
#[cfg(unix)]
pub fn kill_live_process_groups() {
    for pgid in live_groups::snapshot() {
        kill_group(pgid);
    }
}

#[cfg(unix)]
mod live_groups {
    use std::sync::Mutex;

    static LIVE: Mutex<Vec<i32>> = Mutex::new(Vec::new());

    pub(super) fn insert(pgid: i32) {
        if let Ok(mut live) = LIVE.lock() {
            live.push(pgid);
        }
    }

    pub(super) fn remove(pgid: i32) {
        if let Ok(mut live) = LIVE.lock() {
            live.retain(|p| *p != pgid);
        }
    }

    pub(super) fn snapshot() -> Vec<i32> {
        LIVE.lock().map(|live| live.clone()).unwrap_or_default()
    }
}

/// The Windows command: PowerShell inside a kill-on-close Job Object. Rule 1:
/// every flag goes through a shim; nothing is set on the `Command` itself.
#[cfg(windows)]
fn windows_command_wrap(
    command: &str,
    cwd: &Path,
    options: &SpawnOptions,
) -> process_wrap::tokio::CommandWrap {
    use process_wrap::tokio::{CommandWrap, CreationFlags, JobObject, KillOnDrop};

    let command = powershell_command(command, options.powershell_utf8);
    let mut wrap = CommandWrap::with_new("powershell.exe", |c| {
        c.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &command,
        ])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        for name in &options.env_remove {
            c.env_remove(name);
        }
        c.envs(options.env_set.iter().map(|(k, v)| (k, v)));
    });
    // `CreationFlags` takes the `windows` crate's PROCESS_CREATION_FLAGS, a
    // `pub u32` newtype with Default; filling it this way avoids a direct
    // dependency on `windows` just to name the type.
    let mut no_window = CreationFlags(Default::default());
    no_window.0.0 = windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    wrap.wrap(no_window).wrap(JobObject).wrap(KillOnDrop);
    wrap
}

pub const fn shell_name() -> &'static str {
    #[cfg(windows)]
    {
        "Windows PowerShell"
    }
    #[cfg(not(windows))]
    {
        "POSIX sh"
    }
}

/// The current allow-list parser is deliberately Unix/sh-specific. Windows
/// execution remains gated unless the user enables blanket auto-approval.
pub const fn shell_allow_list_supported() -> bool {
    cfg!(not(windows))
}

#[cfg(unix)]
pub fn disk_usage(path: &Path) -> (u64, u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return (0, 0);
    };
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c_path is NUL-terminated and stat is a valid out-parameter.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return (0, 0);
    }
    let block_size = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * block_size;
    let free = stat.f_bfree as u64 * block_size;
    (total.saturating_sub(free), total)
}

#[cfg(windows)]
pub fn disk_usage(path: &Path) -> (u64, u64) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut available = 0u64;
    let mut total = 0u64;
    let mut free = 0u64;
    // SAFETY: wide is NUL-terminated and all output pointers are valid.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
    if ok == 0 {
        (0, 0)
    } else {
        (total.saturating_sub(free), total)
    }
}

pub fn system_prompt_block() -> String {
    format!(
        "# Host environment\nOperating system: {}\nCommand shell: {}\nUse host-appropriate command syntax when the execute tool is necessary.",
        std::env::consts::OS,
        shell_name()
    )
}

/// Process helpers for the tests here, in `tools.rs` and in `shell.rs`:
/// find processes by a nonce in their command line so a test can prove
/// nothing is left running. Outcomes only, with generous bounds; no latencies.
#[cfg(test)]
pub(crate) mod test_procs {
    use std::time::{Duration, Instant};

    /// Bound for every "eventually" assertion of the process tests.
    pub(crate) const BOUND: Duration = Duration::from_secs(30);

    /// A string no other process on the machine has in its command line.
    pub(crate) fn nonce() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!(
            "{}{:09}{}",
            std::process::id(),
            nanos,
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Pids of live processes whose command line matches `pattern`.
    #[cfg(unix)]
    pub(crate) fn pgrep(pattern: &str) -> Vec<u32> {
        let out = std::process::Command::new("pgrep")
            .args(["-f", pattern])
            .output()
            .expect("pgrep is available");
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok())
            .collect()
    }

    /// The `sleep <n>.<nonce>` processes themselves (not the shell running them).
    #[cfg(unix)]
    pub(crate) fn sleepers(nonce: &str) -> Vec<u32> {
        pgrep(&format!("^sleep [0-9]+\\.{nonce}"))
    }

    /// SIGKILL leftovers so a control test does not leak processes.
    #[cfg(unix)]
    pub(crate) fn kill_all(pids: &[u32]) {
        for pid in pids {
            // SAFETY: plain syscall on pids this test started.
            unsafe {
                libc::kill(*pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }

    /// Whether a pid is still running (Windows: `tasklist`).
    #[cfg(windows)]
    pub(crate) fn alive(pid: u32) -> bool {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .expect("tasklist is available");
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    }

    /// Poll until `done` holds; panic after [`BOUND`].
    pub(crate) async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < BOUND, "{what}: not within {BOUND:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_procs::*;
    use super::*;
    use std::time::Duration;

    #[cfg(unix)]
    fn pgid_of(pid: u32) -> i32 {
        // SAFETY: plain syscall.
        unsafe { libc::getpgid(pid as libc::pid_t) }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn flag_off_keeps_the_command_in_craftsmans_own_group() {
        let tree =
            ProcessTree::spawn_shell("sleep 30", Path::new("."), &SpawnOptions::default()).unwrap();
        let pid = tree.id().unwrap();
        // SAFETY: plain syscall.
        assert_eq!(pgid_of(pid), unsafe { libc::getpgid(0) });
        assert_eq!(tree.process_group_id(), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn flag_on_gives_the_command_a_group_of_its_own() {
        let options = SpawnOptions {
            process_group: true,
            ..Default::default()
        };
        let tree = ProcessTree::spawn_shell("sleep 30", Path::new("."), &options).unwrap();
        let pid = tree.id().unwrap();
        assert_eq!(pgid_of(pid), pid as i32);
        assert_eq!(tree.process_group_id(), Some(pid as i32));
        assert!(live_groups::snapshot().contains(&(pid as i32)));
        drop(tree);
        assert!(!live_groups::snapshot().contains(&(pid as i32)));
    }

    /// The guard: dropping a tree mid-run kills the background and the
    /// foreground grandchildren, not only `sh`.
    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_the_tree_kills_the_whole_group() {
        let n = nonce();
        let options = SpawnOptions {
            process_group: true,
            ..Default::default()
        };
        let tree = ProcessTree::spawn_shell(
            &format!("sleep 60.{n}1 & sleep 60.{n}2"),
            Path::new("."),
            &options,
        )
        .unwrap();
        wait_until("both sleepers started", || sleepers(&n).len() == 2).await;
        drop(tree);
        wait_until("every process of the command gone", || pgrep(&n).is_empty()).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kill_tree_kills_the_group_and_reaps_the_leader() {
        let n = nonce();
        let options = SpawnOptions {
            process_group: true,
            ..Default::default()
        };
        let mut tree =
            ProcessTree::spawn_shell(&format!("sleep 60.{n}; echo x"), Path::new("."), &options)
                .unwrap();
        wait_until("the sleeper started", || sleepers(&n).len() == 1).await;
        tokio::time::timeout(BOUND, tree.kill_tree()).await.unwrap();
        assert_eq!(tree.process_group_id(), None, "disarmed after the reap");
        wait_until("every process of the command gone", || pgrep(&n).is_empty()).await;
    }

    /// Control: with the flag off (today), dropping the tree kills `sh` only
    /// and the grandchild keeps running.
    #[cfg(unix)]
    #[tokio::test]
    async fn flag_off_drop_kills_the_direct_child_only() {
        let n = nonce();
        let tree = ProcessTree::spawn_shell(
            &format!("sleep 60.{n}; echo x"),
            Path::new("."),
            &SpawnOptions::default(),
        )
        .unwrap();
        wait_until("the sleeper started", || sleepers(&n).len() == 1).await;
        let shell = tree.id().unwrap();
        drop(tree);
        wait_until("sh killed", || !pgrep(&n).contains(&shell)).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let left = sleepers(&n);
        kill_all(&left);
        assert_eq!(left.len(), 1, "today's behaviour: the grandchild survives");
    }

    #[test]
    fn the_powershell_utf8_prefix_comes_first_and_only_with_the_flag() {
        assert_eq!(
            powershell_command("Write-Output x", false),
            "Write-Output x"
        );
        assert_eq!(
            powershell_command("Write-Output x", true),
            "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Write-Output x"
        );
    }

    /// Windows: with the prefix, text PowerShell prints arrives as UTF-8
    /// (e-acute and a CJK character survive); spike 3.6.
    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_utf8_flag_returns_utf8() {
        use tokio::io::AsyncReadExt;
        let options = SpawnOptions {
            powershell_utf8: true,
            ..Default::default()
        };
        let mut tree = ProcessTree::spawn_shell(
            "Write-Output ([string][char]0x00E9 + [string][char]0x4E2D)",
            Path::new("."),
            &options,
        )
        .unwrap();
        let mut stdout = tree.take_stdout().unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(BOUND, stdout.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&bytes).trim(), "\u{e9}\u{4e2d}");
    }

    /// Windows rule 1: all three shims registered, CREATE_NO_WINDOW carried
    /// by the `CreationFlags` shim (never set on the `Command` itself).
    #[cfg(windows)]
    #[test]
    fn rule1_all_three_shims_are_registered() {
        use process_wrap::tokio::{CreationFlags, JobObject, KillOnDrop};
        let wrap = windows_command_wrap("exit 0", Path::new("."), &SpawnOptions::default());
        assert!(wrap.has_wrap::<JobObject>());
        assert!(wrap.has_wrap::<KillOnDrop>());
        let flags = wrap
            .get_wrap::<CreationFlags>()
            .expect("CreationFlags shim");
        assert_eq!(
            flags.0.0,
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
        );
    }

    /// Start a 60 s ping from PowerShell and return its pid (read from the
    /// first stdout line) along with the tree.
    #[cfg(windows)]
    async fn ping_tree(script: &str) -> (ProcessTree, u32, tokio::process::ChildStdout, String) {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut tree =
            ProcessTree::spawn_shell(script, Path::new("."), &SpawnOptions::default()).unwrap();
        let stdout = tree.take_stdout().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let mut before = String::new();
        let pid = loop {
            before.push_str(&line);
            line.clear();
            let n = tokio::time::timeout(BOUND, reader.read_line(&mut line))
                .await
                .expect("the PID line within the bound")
                .unwrap();
            assert!(n > 0, "stdout closed before the PID line");
            if let Some(rest) = line.trim().strip_prefix("PID=") {
                break rest.parse::<u32>().unwrap();
            }
        };
        (tree, pid, reader.into_inner(), before)
    }

    #[cfg(windows)]
    const PING_GRANDCHILD: &str = "$p = Start-Process -FilePath ping.exe -ArgumentList '-n','60','127.0.0.1' -NoNewWindow -PassThru; Write-Output \"PID=$($p.Id)\"; $p.WaitForExit()";

    /// Windows rule 1 (behaviour): without `KillOnDrop` a drop leaks the
    /// whole tree (spike b2n); with it the grandchild dies.
    #[cfg(windows)]
    #[tokio::test]
    async fn rule1_drop_kills_the_grandchild() {
        let (tree, pid, _stdout, _) = ping_tree(PING_GRANDCHILD).await;
        assert!(alive(pid));
        drop(tree);
        wait_until("the ping grandchild gone", || !alive(pid)).await;
    }

    /// Windows rule 2: on timeout `kill()` terminates the job, then the pipes
    /// reach EOF and what was printed before is intact (spike
    /// b-p2b-prototype-jobobject).
    #[cfg(windows)]
    #[tokio::test]
    async fn rule2_kill_then_read_the_pipes_with_a_short_deadline() {
        use tokio::io::AsyncReadExt;
        let script = format!("Write-Output 'partial-output'; {PING_GRANDCHILD}");
        let (mut tree, pid, mut stdout, before) = ping_tree(&script).await;
        assert!(before.contains("partial-output"), "got: {before}");
        tokio::time::timeout(BOUND, tree.kill_tree()).await.unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stdout.read_to_end(&mut rest))
            .await
            .expect("the pipe reaches EOF after the job is killed")
            .unwrap();
        wait_until("the ping grandchild gone", || !alive(pid)).await;
    }

    /// Windows rule 3: `wait()` is the direct child's; the job is not waited
    /// for, and dropping the tree kills what the shell left behind.
    #[cfg(windows)]
    #[tokio::test]
    async fn rule3_wait_is_the_shells_and_the_drop_kills_the_orphan() {
        let script = "$p = Start-Process -FilePath ping.exe -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru; Write-Output \"PID=$($p.Id)\"; exit 4";
        let (mut tree, pid, _stdout, _) = ping_tree(script).await;
        let status = tokio::time::timeout(BOUND, tree.wait())
            .await
            .expect("wait() returns once the shell exits, with the orphan still running")
            .unwrap();
        assert_eq!(status.code(), Some(4));
        assert!(alive(pid), "the orphan outlives the shell until the drop");
        drop(tree);
        wait_until("the orphan gone after the drop", || !alive(pid)).await;
    }

    /// Windows rule 4: never `wait()` after `try_wait()`. The type has no
    /// `try_wait`; a child that exited long before `wait()` is still reaped.
    #[cfg(windows)]
    #[tokio::test]
    async fn rule4_wait_on_an_exited_child_returns() {
        for _ in 0..5 {
            let mut tree =
                ProcessTree::spawn_shell("exit 7", Path::new("."), &SpawnOptions::default())
                    .unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
            let status = tokio::time::timeout(BOUND, tree.wait())
                .await
                .expect("wait() returns for an exited child")
                .unwrap();
            assert_eq!(status.code(), Some(7));
        }
    }

    #[test]
    fn host_descriptions_are_non_empty() {
        assert!(!shell_name().is_empty());
        assert!(system_prompt_block().contains(std::env::consts::OS));
    }

    #[test]
    fn current_filesystem_reports_capacity() {
        let current = std::env::current_dir().unwrap();
        let (_, total) = disk_usage(&current);
        assert!(total > 0);
    }
}

// Private named-input pipe primitive; old platform/filesystem/process APIs stay exact.
#[allow(dead_code)] // Native stdin remains unwired from public frontends.
pub(crate) mod stdin_pipe;
