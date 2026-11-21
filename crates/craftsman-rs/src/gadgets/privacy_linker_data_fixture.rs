// Private cfg-test data operation. These bytes are neither gadget source nor
// executable instructions. The selected LINK result still needs real exit0,
// the unchanged banner gate, full PE validation and owned cleanup.
use super::*;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};

const CHILD: &str = "owned-msvc-linker";
const INPUT: &str = "LinkerProbe.obj";
const OUTPUT: &str = "LinkerProbe.dll";
const MARKER: &[u8; 16] = b"MSVC-CMD:BUILTIN";
const API_LIMIT: usize = 80;

const X64_OBJECT: [u8; 98] = [
    0x64, 0x86, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4c, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x2e, 0x72, 0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x10, 0x40, 0x4d, 0x53, 0x56, 0x43,
    0x2d, 0x43, 0x4d, 0x44, 0x3a, 0x42, 0x55, 0x49, 0x4c, 0x54, 0x49, 0x4e, 0x63, 0x66, 0x70, 0x72,
    0x6f, 0x62, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x04, 0x00,
    0x00, 0x00,
];
const X86_OBJECT: [u8; 98] = [
    0x4c, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4c, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x2e, 0x72, 0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x10, 0x40, 0x4d, 0x53, 0x56, 0x43,
    0x2d, 0x43, 0x4d, 0x44, 0x3a, 0x42, 0x55, 0x49, 0x4c, 0x54, 0x49, 0x4e, 0x5f, 0x63, 0x66, 0x70,
    0x72, 0x6f, 0x62, 0x65, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x04, 0x00,
    0x00, 0x00,
];
const ARM64_OBJECT: [u8; 98] = [
    0x64, 0xaa, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4c, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x2e, 0x72, 0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x10, 0x40, 0x4d, 0x53, 0x56, 0x43,
    0x2d, 0x43, 0x4d, 0x44, 0x3a, 0x42, 0x55, 0x49, 0x4c, 0x54, 0x49, 0x4e, 0x63, 0x66, 0x70, 0x72,
    0x6f, 0x62, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x04, 0x00,
    0x00, 0x00,
];

fn object(host: Host) -> &'static [u8; 98] {
    match host {
        Host::X64 => &X64_OBJECT,
        Host::X86 => &X86_OBJECT,
        Host::Arm64 => &ARM64_OBJECT,
    }
}
fn arguments(host: Host) -> [&'static str; 10] {
    let (symbol, machine) = match host {
        Host::X64 => ("/INCLUDE:cfprobe", "/MACHINE:X64"),
        Host::X86 => ("/INCLUDE:_cfprobe", "/MACHINE:X86"),
        Host::Arm64 => ("/INCLUDE:cfprobe", "/MACHINE:ARM64"),
    };
    [
        "/DLL",
        "/NOENTRY",
        "/NODEFAULTLIB",
        "/INCREMENTAL:NO",
        "/MANIFEST:NO",
        symbol,
        machine,
        "/OUT:LinkerProbe.dll",
        INPUT,
        "/NOCOFFGRPINFO",
    ]
}
fn failure(kind: Kind, step: &'static str) -> Failure {
    Failure::new(Stage::LinkerBanner, kind).expected(step)
}
fn io_failure(kind: Kind, step: &'static str, error: &std::io::Error) -> Failure {
    Failure::io(Stage::LinkerBanner, kind, error).expected(step)
}

#[derive(Clone, Copy)]
enum Phase {
    Input,
    Validation,
    Cleanup,
}
impl Phase {
    fn seconds(self) -> u64 {
        match self {
            Self::Input => 5,
            Self::Validation => 10,
            Self::Cleanup => 15,
        }
    }
}
struct Clock {
    // One stamp, taken immediately before the first new native file operation.
    started: Instant,
    calls: usize,
    handles: usize,
}
impl Clock {
    fn check(&self, phase: Phase) -> SetupResult<()> {
        if self.started.elapsed() >= Duration::from_secs(phase.seconds()) {
            return Err(failure(Kind::Timeout, "probe.deadline"));
        }
        Ok(())
    }
    fn admit(&mut self, phase: Phase, opening: bool) -> SetupResult<()> {
        self.check(phase)?;
        // Reserve one release admission for every held/new handle. A refusal
        // cannot consume the calls needed to release already owned handles.
        if self.calls + 1 + self.handles + usize::from(opening) > API_LIMIT {
            return Err(failure(Kind::Bounds, "probe.calls"));
        }
        self.calls += 1;
        Ok(())
    }
    fn after<T>(&self, phase: Phase, value: SetupResult<T>) -> SetupResult<T> {
        // An actual OS failure stays primary even when the same call is late.
        let time = self.check(phase);
        match value {
            Err(error) => Err(error),
            Ok(value) => time.map(|()| value),
        }
    }
    fn poll_seconds(&self) -> SetupResult<u64> {
        self.check(Phase::Input)?;
        let seconds = Duration::from_secs(5)
            .saturating_sub(self.started.elapsed())
            .as_secs();
        if seconds == 0 {
            return Err(failure(Kind::Timeout, "probe.deadline"));
        }
        Ok(seconds)
    }
}

struct Paths {
    root: PathBuf,
    child: PathBuf,
    input: PathBuf,
    output: PathBuf,
}
impl Paths {
    fn capture(command: &Command) -> SetupResult<Self> {
        // These are the original registered owner's already cleared command
        // values. No ambient lookup or supplied root/label is accepted here.
        let mut root = None;
        for name in ["HOME", "USERPROFILE", "TMPDIR", "TEMP", "TMP"] {
            let values: Vec<_> = command
                .get_envs()
                .filter(|(key, _)| *key == OsStr::new(name))
                .collect();
            let [(_, Some(value))] = values.as_slice() else {
                return Err(failure(Kind::Environment, "probe.aliases"));
            };
            match &root {
                Some(previous) if previous != value => {
                    return Err(failure(Kind::Environment, "probe.aliases"));
                }
                None => root = Some(*value),
                _ => {}
            }
        }
        let root = checked_path(root.unwrap().to_owned())
            .map_err(|error| error.at(Stage::LinkerBanner).expected("probe.root"))?;
        let child = root.join(CHILD);
        let input = child.join(INPUT);
        let output = child.join(OUTPUT);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            if output.as_os_str().encode_wide().count() >= 260 {
                return Err(failure(Kind::Bounds, "probe.output.path"));
            }
        }
        Ok(Self {
            root,
            child,
            input,
            output,
        })
    }
    fn path(&self, slot: Slot) -> &Path {
        match slot {
            Slot::Root => &self.root,
            Slot::Child => &self.child,
            Slot::Writer | Slot::Input => &self.input,
            Slot::Output => &self.output,
            Slot::Check => unreachable!("check handle uses a fixed target slot"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
enum Slot {
    Root,
    Child,
    Writer,
    Input,
    Output,
    Check,
}
const SLOTS: [Slot; 6] = [
    Slot::Check,
    Slot::Output,
    Slot::Input,
    Slot::Writer,
    Slot::Child,
    Slot::Root,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    volume: u64,
    index: u64,
    directory: bool,
    reparse: bool,
    links: u64,
    size: u64,
    write_time: (u64, u64),
}
impl Identity {
    fn regular(self) -> SetupResult<Self> {
        if self.directory || self.reparse || self.links != 1 || self.index == 0 {
            return Err(failure(Kind::NotFile, "probe.file"));
        }
        Ok(self)
    }
    fn same(self, other: Self) -> bool {
        self.volume == other.volume
            && self.index == other.index
            && self.directory == other.directory
            && !self.reparse
            && !other.reparse
            && (self.directory
                || (self.links == other.links
                    && self.links == 1
                    && self.size == other.size
                    && self.write_time == other.write_time))
    }
    fn same_created_input(self, other: Self) -> bool {
        // Windows finalizes a creator's last-write timestamp when it closes.
        // Bind that transition by native file identity and complete98 bytes;
        // the read-only seal then retains the actual post-close full metadata.
        self.volume == other.volume
            && self.index == other.index
            && !self.directory
            && !other.directory
            && !self.reparse
            && !other.reparse
            && self.links == 1
            && other.links == 1
            && self.size == 98
            && other.size == 98
    }
}

fn native_open(path: &Path, directory: bool, create: bool) -> std::io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING,
        };
        // The existing windows-sys feature set need not expose the optional
        // SECURITY_ATTRIBUTES type. This fixed Win32 prototype passes NULL;
        // no dependency/SDK feature or handle inheritance setting is changed.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateFileW(
                name: *const u16,
                access: u32,
                share: u32,
                security: *mut std::ffi::c_void,
                disposition: u32,
                flags: u32,
                template: *mut std::ffi::c_void,
            ) -> *mut std::ffi::c_void;
        }
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let access = if directory {
            FILE_READ_ATTRIBUTES
        } else if create {
            GENERIC_READ | GENERIC_WRITE
        } else {
            GENERIC_READ
        };
        let share = if directory {
            FILE_SHARE_READ | FILE_SHARE_WRITE
        } else {
            FILE_SHARE_READ
        };
        let flags = FILE_ATTRIBUTE_NORMAL
            | FILE_FLAG_OPEN_REPARSE_POINT
            | if directory {
                FILE_FLAG_BACKUP_SEMANTICS
            } else {
                0
            };
        // NULL security attributes make this actual handle noninheritable.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                access,
                share,
                std::ptr::null_mut(),
                if create { CREATE_NEW } else { OPEN_EXISTING },
                flags,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_handle(handle) })
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).custom_flags(
            libc::O_NOFOLLOW | libc::O_CLOEXEC | if directory { libc::O_DIRECTORY } else { 0 },
        );
        if create {
            options.write(true).create_new(true).mode(0o600);
        }
        options.open(path)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (path, directory, create);
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}
fn native_close(file: File) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::IntoRawHandle;
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(file.into_raw_handle()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::os::fd::IntoRawFd;
        if unsafe { libc::close(file.into_raw_fd()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(windows, unix)))]
    {
        drop(file);
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

struct Owned {
    paths: Paths,
    clock: Clock,
    files: [Option<File>; 6],
    root: Option<Identity>,
    child: Option<Identity>,
    input: Option<Identity>,
    output: Option<Identity>,
}
impl Owned {
    fn new(command: &Command) -> SetupResult<Self> {
        let paths = Paths::capture(command)?;
        Ok(Self {
            paths,
            clock: Clock {
                started: Instant::now(),
                calls: 0,
                handles: 0,
            },
            files: std::array::from_fn(|_| None),
            root: None,
            child: None,
            input: None,
            output: None,
        })
    }
    fn file(&mut self, slot: Slot) -> SetupResult<&mut File> {
        self.files[slot as usize]
            .as_mut()
            .ok_or_else(|| failure(Kind::Shape, "probe.handle"))
    }
    fn open(&mut self, slot: Slot, target: Slot, phase: Phase) -> SetupResult<()> {
        if self.files[slot as usize].is_some() {
            return Err(failure(Kind::Shape, "probe.handle"));
        }
        self.clock.admit(phase, true)?;
        let value = native_open(
            self.paths.path(target),
            matches!(target, Slot::Root | Slot::Child),
            slot == Slot::Writer,
        );
        let result = match value {
            Ok(file) => {
                // Store before a late-return check; even a late acquired handle
                // has one explicit counted release, never a hidden File drop.
                self.files[slot as usize] = Some(file);
                self.clock.handles += 1;
                Ok(())
            }
            Err(error) => Err(io_failure(Kind::Metadata, "probe.open", &error)),
        };
        self.clock.after(phase, result)
    }
    fn info(&mut self, slot: Slot, phase: Phase) -> SetupResult<Identity> {
        self.clock.admit(phase, false)?;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
                FILE_TYPE_DISK, GetFileInformationByHandle, GetFileType,
            };
            let handle = self.file(slot)?.as_raw_handle();
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            let result = if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
                Err(io_failure(
                    Kind::Metadata,
                    "probe.info",
                    &std::io::Error::last_os_error(),
                ))
            } else {
                Ok(())
            };
            self.clock.after(phase, result)?;
            self.clock.admit(phase, false)?;
            let disk = unsafe { GetFileType(handle) };
            self.clock.after(
                phase,
                if disk == FILE_TYPE_DISK {
                    Ok(())
                } else {
                    Err(failure(Kind::NotFile, "probe.disk"))
                },
            )?;
            Ok(Identity {
                volume: u64::from(info.dwVolumeSerialNumber),
                index: u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
                directory: info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                reparse: info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
                links: u64::from(info.nNumberOfLinks),
                size: u64::from(info.nFileSizeHigh) << 32 | u64::from(info.nFileSizeLow),
                write_time: (
                    u64::from(info.ftLastWriteTime.dwHighDateTime),
                    u64::from(info.ftLastWriteTime.dwLowDateTime),
                ),
            })
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let value = self.file(slot)?.metadata();
            let metadata = self.clock.after(
                phase,
                value.map_err(|error| io_failure(Kind::Metadata, "probe.info", &error)),
            )?;
            if !metadata.is_dir() && !metadata.is_file() {
                return Err(failure(Kind::NotFile, "probe.disk"));
            }
            Ok(Identity {
                volume: metadata.dev(),
                index: metadata.ino(),
                directory: metadata.is_dir(),
                reparse: metadata.file_type().is_symlink(),
                links: metadata.nlink(),
                size: metadata.len(),
                write_time: (metadata.mtime() as u64, metadata.mtime_nsec() as u64),
            })
        }
        #[cfg(not(any(windows, unix)))]
        Err(failure(Kind::Host, "probe.platform"))
    }
    fn close(&mut self, slot: Slot, phase: Phase) -> SetupResult<()> {
        let Some(file) = self.files[slot as usize].take() else {
            return Ok(());
        };
        // Mandatory release is attempted once, also on an earlier error/late
        // return. Its before/after checks cannot turn late cleanup into success.
        // No namespace I/O is admitted after its deadline, and no clock resets.
        let before = self.clock.check(phase);
        self.clock.calls += 1;
        self.clock.handles -= 1;
        assert!(self.clock.calls + self.clock.handles <= API_LIMIT);
        let result =
            native_close(file).map_err(|error| io_failure(Kind::Metadata, "probe.close", &error));
        let after = self.clock.after(phase, result);
        match after {
            Err(error) => Err(error),
            Ok(()) => before,
        }
    }
    fn close_all(&mut self) -> SetupResult<()> {
        let mut first = None;
        for slot in SLOTS {
            if let Err(error) = self.close(slot, Phase::Cleanup) {
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        match first {
            Some(error) => Err(error),
            None => self.clock.check(Phase::Cleanup),
        }
    }
    fn same_path(&mut self, target: Slot, identity: Identity, phase: Phase) -> SetupResult<()> {
        self.open(Slot::Check, target, phase)?;
        let current = self.info(Slot::Check, phase)?;
        let same = identity.same(current);
        self.close(Slot::Check, phase)?;
        if !same {
            return Err(failure(Kind::Containment, "probe.fileid"));
        }
        Ok(())
    }
    fn roots(&mut self, phase: Phase) -> SetupResult<()> {
        let root = self
            .root
            .ok_or_else(|| failure(Kind::Shape, "probe.root"))?;
        let child = self
            .child
            .ok_or_else(|| failure(Kind::Shape, "probe.child"))?;
        self.same_path(Slot::Root, root, phase)?;
        self.same_path(Slot::Child, child, phase)
    }
    fn absent_output(&mut self, phase: Phase) -> SetupResult<()> {
        self.clock.admit(phase, false)?;
        let result = match std::fs::symlink_metadata(&self.paths.output) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_failure(Kind::Metadata, "probe.output.absent", &error)),
            Ok(_) => Err(failure(Kind::Containment, "probe.output.absent")),
        };
        self.clock.after(phase, result)
    }
    fn seek(&mut self, slot: Slot, phase: Phase) -> SetupResult<()> {
        self.clock.admit(phase, false)?;
        let result = self
            .file(slot)?
            .seek(SeekFrom::Start(0))
            .map_err(|error| io_failure(Kind::Read, "probe.seek", &error));
        self.clock.after(phase, result).map(|_| ())
    }
    fn read(
        &mut self,
        slot: Slot,
        phase: Phase,
        sentinel: usize,
        max_reads: usize,
    ) -> SetupResult<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut attempted = false;
        let result = (|| {
            for _ in 0..max_reads {
                self.clock.admit(phase, false)?;
                let mut scratch = [0u8; 4096];
                let request = (sentinel - bytes.len()).min(scratch.len());
                if request == 0 {
                    return Err(failure(Kind::Bounds, "probe.read"));
                }
                let file = self.file(slot)?;
                attempted = true;
                let result = file
                    .read(&mut scratch[..request])
                    .map_err(|error| io_failure(Kind::Read, "probe.read", &error));
                let count = match result {
                    Ok(count) => {
                        bytes.extend_from_slice(&scratch[..count]);
                        self.clock.check(phase)?;
                        count
                    }
                    Err(error) => {
                        let _ = self.clock.check(phase);
                        return Err(error);
                    }
                };
                if count == 0 {
                    return Ok(()); // actual EOF, never inferred from size/status
                }
                if bytes.len() == sentinel {
                    return Err(failure(Kind::Bounds, "probe.read"));
                }
            }
            Err(failure(Kind::Read, "probe.read"))
        })();
        match result {
            Ok(()) => Ok(bytes),
            Err(mut error) => {
                if attempted {
                    error.observed_read_bytes = Some(bytes.len());
                    error.observed_read_prefix_sha256 = Some(hash(&bytes));
                }
                Err(error)
            }
        }
    }
    fn input_pass(&mut self, host: Host, phase: Phase) -> SetupResult<()> {
        self.seek(Slot::Input, phase)?;
        let bytes = self.read(Slot::Input, phase, 99, 4)?;
        if bytes.as_slice() != object(host).as_slice() {
            let mut error = failure(Kind::Shape, "probe.input");
            error.observed_read_bytes = Some(bytes.len());
            error.observed_read_prefix_sha256 = Some(hash(&bytes));
            return Err(error);
        }
        Ok(())
    }
    fn prepare_input(&mut self, host: Host) -> SetupResult<()> {
        self.open(Slot::Root, Slot::Root, Phase::Input)?;
        let root = self.info(Slot::Root, Phase::Input)?;
        if !root.directory || root.reparse || root.index == 0 {
            return Err(failure(Kind::NotDirectory, "probe.root"));
        }
        self.root = Some(root);
        self.clock.admit(Phase::Input, false)?;
        let result = std::fs::create_dir(&self.paths.child)
            .map_err(|error| io_failure(Kind::Metadata, "probe.child", &error));
        self.clock.after(Phase::Input, result)?;
        self.open(Slot::Child, Slot::Child, Phase::Input)?;
        let child = self.info(Slot::Child, Phase::Input)?;
        if !child.directory || child.reparse || child.index == 0 || child.volume != root.volume {
            return Err(failure(Kind::NotDirectory, "probe.child"));
        }
        self.child = Some(child);
        self.open(Slot::Writer, Slot::Writer, Phase::Input)?;
        let mut written = 0;
        for _ in 0..4 {
            if written == object(host).len() {
                break;
            }
            self.clock.admit(Phase::Input, false)?;
            let result = self
                .file(Slot::Writer)?
                .write(&object(host)[written..])
                .map_err(|error| io_failure(Kind::Read, "probe.write", &error));
            let count = self.clock.after(Phase::Input, result)?;
            if count == 0 {
                return Err(failure(Kind::Read, "probe.write"));
            }
            written += count;
        }
        if written != 98 {
            return Err(failure(Kind::Read, "probe.write"));
        }
        self.clock.admit(Phase::Input, false)?;
        let result = self
            .file(Slot::Writer)?
            .sync_all()
            .map_err(|error| io_failure(Kind::Read, "probe.flush", &error));
        self.clock.after(Phase::Input, result)?;
        let created = self.info(Slot::Writer, Phase::Input)?.regular()?;
        self.seek(Slot::Writer, Phase::Input)?;
        let bytes = self.read(Slot::Writer, Phase::Input, 99, 4)?;
        if bytes.as_slice() != object(host).as_slice()
            || created.size != 98
            || created.volume != root.volume
        {
            let mut error = failure(Kind::Shape, "probe.input");
            error.observed_read_bytes = Some(bytes.len());
            error.observed_read_prefix_sha256 = Some(hash(&bytes));
            return Err(error);
        }
        self.close(Slot::Writer, Phase::Input)?;
        self.open(Slot::Input, Slot::Input, Phase::Input)?;
        let input = self.info(Slot::Input, Phase::Input)?.regular()?;
        if !created.same_created_input(input) {
            return Err(failure(Kind::Containment, "probe.input"));
        }
        self.input = Some(input);
        self.input_pass(host, Phase::Input)?;
        self.same_path(Slot::Input, input, Phase::Input)?;
        self.roots(Phase::Input)
    }
    fn validate_output(&mut self, host: Host) -> SetupResult<()> {
        // Identity starts at this real post-link open. No empty file reservation
        // or assumed continuity with an inode that LINK might replace.
        self.open(Slot::Output, Slot::Output, Phase::Validation)?;
        let output = self.info(Slot::Output, Phase::Validation)?.regular()?;
        let child = self
            .child
            .ok_or_else(|| failure(Kind::Shape, "probe.child"))?;
        if output.volume != child.volume || output.size > CAPTURE_BYTES as u64 {
            return Err(failure(Kind::Bounds, "probe.output"));
        }
        self.output = Some(output);
        let bytes = self.read(Slot::Output, Phase::Validation, CAPTURE_BYTES + 1, 33)?;
        if bytes.len() as u64 != output.size {
            let mut error = failure(Kind::Containment, "probe.output");
            error.observed_read_bytes = Some(bytes.len());
            error.observed_read_prefix_sha256 = Some(hash(&bytes));
            return Err(error);
        }
        self.clock.check(Phase::Validation)?;
        validate_pe(&bytes, host)?;
        self.clock.check(Phase::Validation)?;
        self.same_path(Slot::Output, output, Phase::Validation)?;
        let input = self
            .input
            .ok_or_else(|| failure(Kind::Shape, "probe.input"))?;
        self.input_pass(host, Phase::Validation)?;
        self.same_path(Slot::Input, input, Phase::Validation)?;
        self.roots(Phase::Validation)
    }
    fn cleanup_files(&mut self) -> SetupResult<()> {
        self.close(Slot::Output, Phase::Cleanup)?;
        self.close(Slot::Input, Phase::Cleanup)?;
        for (slot, identity) in [(Slot::Output, self.output), (Slot::Input, self.input)] {
            let identity = identity.ok_or_else(|| failure(Kind::Shape, "probe.cleanup"))?;
            self.same_path(slot, identity, Phase::Cleanup)?;
            self.clock.admit(Phase::Cleanup, false)?;
            let result = std::fs::remove_file(self.paths.path(slot))
                .map_err(|error| io_failure(Kind::Metadata, "probe.cleanup", &error));
            self.clock.after(Phase::Cleanup, result)?;
        }
        let child = self
            .child
            .ok_or_else(|| failure(Kind::Shape, "probe.cleanup"))?;
        self.same_path(Slot::Child, child, Phase::Cleanup)?;
        self.close(Slot::Child, Phase::Cleanup)?;
        self.clock.admit(Phase::Cleanup, false)?;
        let result = std::fs::remove_dir(&self.paths.child)
            .map_err(|error| io_failure(Kind::Metadata, "probe.cleanup", &error));
        self.clock.after(Phase::Cleanup, result)
    }
}

#[cfg(windows)]
pub(super) fn run(command: &mut Command, host: Host) -> SetupResult<()> {
    let mut owned = Owned::new(command)?;
    let mut native = None;
    let result = (|| {
        owned.prepare_input(host)?;
        command
            .args(arguments(host))
            .current_dir(&owned.paths.child)
            .stdin(Stdio::null());
        // This absence observation is immediately before actual dispatch.
        owned.absent_output(Phase::Input)?;
        owned.clock.admit(Phase::Input, false)?;
        let seconds = owned.clock.poll_seconds()?;
        let output = bounded_at(command, seconds, Stage::LinkerBanner)?;
        native = Some(output);
        let output = native.as_ref().unwrap();
        linker_banner(output)?; // FIRST acceptance after the unchanged helper
        owned.clock.check(Phase::Validation)?;
        owned.validate_output(host)?;
        owned.cleanup_files()
    })();
    let closed = owned.close_all();
    let result = match result {
        Err(error) => Err(error), // retain the original primary failure exactly
        Ok(()) => closed,
    };
    result.map_err(|error| {
        if error.process.is_none() {
            match native.as_ref() {
                Some(output) => error.output(output, false),
                None => error,
            }
        } else {
            error
        }
    })
}

fn word(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn dword(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn aligned(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment.checked_sub(1)?)?
        .checked_div(alignment)?
        .checked_mul(alignment)
}
// Fixed first-refusal groups for the original acceptance checks. These labels
// identify checked data, not a native cause or permission to accept that data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeRejection {
    File,
    Signature,
    Coff,
    OptionalHeader,
    OptionalExtent,
    EntryReserved,
    Directories,
    DirectoryRva(usize),
    DirectorySize(usize),
    AlignmentSection,
    ImageRawMarker,
}
const DIRECTORY_RVA_REJECTIONS: [&str; 16] = [
    "pe.d00.rva",
    "pe.d01.rva",
    "pe.d02.rva",
    "pe.d03.rva",
    "pe.d04.rva",
    "pe.d05.rva",
    "pe.d06.rva",
    "pe.d07.rva",
    "pe.d08.rva",
    "pe.d09.rva",
    "pe.d10.rva",
    "pe.d11.rva",
    "pe.d12.rva",
    "pe.d13.rva",
    "pe.d14.rva",
    "pe.d15.rva",
];
const DIRECTORY_SIZE_REJECTIONS: [&str; 16] = [
    "pe.d00.size",
    "pe.d01.size",
    "pe.d02.size",
    "pe.d03.size",
    "pe.d04.size",
    "pe.d05.size",
    "pe.d06.size",
    "pe.d07.size",
    "pe.d08.size",
    "pe.d09.size",
    "pe.d10.size",
    "pe.d11.size",
    "pe.d12.size",
    "pe.d13.size",
    "pe.d14.size",
    "pe.d15.size",
];
impl PeRejection {
    fn expected(self) -> &'static str {
        match self {
            Self::File => "pe.file",
            Self::Signature => "pe.signature",
            Self::Coff => "pe.coff",
            Self::OptionalHeader => "pe.optional.header",
            Self::OptionalExtent => "pe.optional.extent",
            Self::EntryReserved => "pe.entry.reserved",
            Self::Directories => "pe.directories",
            Self::DirectoryRva(ordinal) => DIRECTORY_RVA_REJECTIONS
                .get(ordinal)
                .copied()
                .unwrap_or("pe.directories"),
            Self::DirectorySize(ordinal) => DIRECTORY_SIZE_REJECTIONS
                .get(ordinal)
                .copied()
                .unwrap_or("pe.directories"),
            Self::AlignmentSection => "pe.align.section",
            Self::ImageRawMarker => "pe.image.raw.marker",
        }
    }
}
fn pe_shape(bytes: &[u8], host: Host) -> Result<(), PeRejection> {
    let mut reason = PeRejection::File;
    let accepted = (|| {
        if bytes.len() > CAPTURE_BYTES || bytes.get(..2)? != b"MZ" {
            return None;
        }
        reason = PeRejection::Signature;
        let pe = usize::try_from(dword(bytes, 60)?).ok()?;
        if pe < 64 || bytes.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
            return None;
        }
        reason = PeRejection::Coff;
        let header = pe.checked_add(4)?;
        let machine = match host {
            Host::X64 => 0x8664,
            Host::X86 => 0x14c,
            Host::Arm64 => 0xaa64,
        };
        if word(bytes, header)? != machine
            || word(bytes, header.checked_add(2)?)? != 1
            || dword(bytes, header.checked_add(8)?)? != 0
            || dword(bytes, header.checked_add(12)?)? != 0
            || word(bytes, header.checked_add(18)?)? & 0x2002 != 0x2002
        {
            return None;
        }
        reason = PeRejection::OptionalHeader;
        let optional = header.checked_add(20)?;
        let optional_size = usize::from(word(bytes, header.checked_add(16)?)?);
        let (magic, directory_offset, count_offset) = match host {
            Host::X86 => (0x10b, 96usize, 92usize),
            Host::X64 | Host::Arm64 => (0x20b, 112, 108),
        };
        if word(bytes, optional)? != magic || optional_size < directory_offset {
            return None;
        }
        reason = PeRejection::OptionalExtent;
        let count = usize::try_from(dword(bytes, optional.checked_add(count_offset)?)?).ok()?;
        if count > 16 || optional_size != directory_offset.checked_add(count.checked_mul(8)?)? {
            return None;
        }
        let section = optional.checked_add(optional_size)?;
        let section_end = section.checked_add(40)?;
        bytes.get(optional..section_end)?;
        reason = PeRejection::EntryReserved;
        if dword(bytes, optional.checked_add(4)?)? != 0
            || dword(bytes, optional.checked_add(12)?)? != 0
            || dword(bytes, optional.checked_add(16)?)? != 0
            || dword(bytes, optional.checked_add(52)?)? != 0
            || dword(
                bytes,
                optional.checked_add(if host == Host::X86 { 88 } else { 104 })?,
            )? != 0
        {
            return None;
        }
        reason = PeRejection::Directories;
        // This fixed data-only input has no image directories. In particular no
        // imports/exports/TLS/bound or delay imports, code, loader data or signatures.
        for ordinal in 0..count {
            let offset = optional
                .checked_add(directory_offset)?
                .checked_add(ordinal.checked_mul(8)?)?;
            if {
                reason = PeRejection::DirectoryRva(ordinal);
                dword(bytes, offset)? != 0
            } || {
                reason = PeRejection::DirectorySize(ordinal);
                dword(bytes, offset.checked_add(4)?)? != 0
            } {
                return None;
            }
        }
        reason = PeRejection::AlignmentSection;
        let file_alignment = usize::try_from(dword(bytes, optional.checked_add(36)?)?).ok()?;
        let section_alignment = usize::try_from(dword(bytes, optional.checked_add(32)?)?).ok()?;
        let headers = usize::try_from(dword(bytes, optional.checked_add(60)?)?).ok()?;
        let image = usize::try_from(dword(bytes, optional.checked_add(56)?)?).ok()?;
        if !(512..=65536).contains(&file_alignment)
            || !file_alignment.is_power_of_two()
            || !section_alignment.is_power_of_two()
            || section_alignment < 4096
            || section_alignment < file_alignment
            || headers < section_end
            || headers > bytes.len()
            || headers % file_alignment != 0
            || bytes.get(section..section.checked_add(8)?)? != b".rdata\0\0"
            || dword(bytes, section.checked_add(8)?)? != 16
            || dword(bytes, section.checked_add(24)?)? != 0
            || dword(bytes, section.checked_add(28)?)? != 0
            || word(bytes, section.checked_add(32)?)? != 0
            || word(bytes, section.checked_add(34)?)? != 0
            || dword(bytes, section.checked_add(36)?)? != 0x40000040
        {
            return None;
        }
        reason = PeRejection::ImageRawMarker;
        let virtual_address = usize::try_from(dword(bytes, section.checked_add(12)?)?).ok()?;
        let raw_size = usize::try_from(dword(bytes, section.checked_add(16)?)?).ok()?;
        let raw = usize::try_from(dword(bytes, section.checked_add(20)?)?).ok()?;
        let end = raw.checked_add(raw_size)?;
        if virtual_address < aligned(headers, section_alignment)?
            || virtual_address % section_alignment != 0
            || image != aligned(virtual_address.checked_add(16)?, section_alignment)?
            || raw_size < MARKER.len()
            || raw_size % file_alignment != 0
            || raw < headers
            || raw % file_alignment != 0
            || end != bytes.len()
            || dword(bytes, optional.checked_add(8)?)? as usize != raw_size
            || bytes.get(raw..raw.checked_add(MARKER.len())?)? != MARKER
        {
            return None;
        }
        // Vendor timestamps, DOS stub and alignment padding are retained verbatim.
        // No image/DLL is ever loaded or executed by this helper.
        Some(())
    })();
    accepted.ok_or(reason)
}
fn validate_pe(bytes: &[u8], host: Host) -> SetupResult<()> {
    let reason = match pe_shape(bytes, host) {
        Ok(()) => return Ok(()),
        Err(reason) => reason,
    };
    let mut error = failure(Kind::Pe, reason.expected());
    error.observed_read_bytes = Some(bytes.len());
    error.observed_read_prefix_sha256 = Some(hash(bytes));
    if bytes.get(..2) == Some(b"MZ") {
        if let Some(offset) = dword(bytes, 60).and_then(|offset| usize::try_from(offset).ok()) {
            if offset
                .checked_add(4)
                .is_some_and(|end| bytes.get(offset..end) == Some(b"PE\0\0"))
            {
                error.observed_pe_machine =
                    offset.checked_add(4).and_then(|offset| word(bytes, offset));
            }
        }
    }
    Err(error)
}

// Independent ordinary PE fixtures: full files made of headers and data only.
// They are never passed to LINK, loaded, executed or used as Prepared/source.
#[cfg(test)]
fn sample_pe(host: Host) -> Vec<u8> {
    let mut bytes = vec![0xa5; 1024];
    bytes[..64].fill(0);
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&128u32.to_le_bytes());
    bytes[128..132].copy_from_slice(b"PE\0\0");
    let machine: u16 = match host {
        Host::X64 => 0x8664,
        Host::X86 => 0x14c,
        Host::Arm64 => 0xaa64,
    };
    let optional_size: u16 = if host == Host::X86 { 224 } else { 240 };
    bytes[132..152].fill(0);
    bytes[132..134].copy_from_slice(&machine.to_le_bytes());
    bytes[134..136].copy_from_slice(&1u16.to_le_bytes());
    bytes[136..140].copy_from_slice(&0x67f012abu32.to_le_bytes());
    bytes[148..150].copy_from_slice(&optional_size.to_le_bytes());
    bytes[150..152].copy_from_slice(&0x2002u16.to_le_bytes());
    let optional = 152usize;
    let end = optional + usize::from(optional_size);
    bytes[optional..end + 40].fill(0);
    let magic: u16 = if host == Host::X86 { 0x10b } else { 0x20b };
    bytes[optional..optional + 2].copy_from_slice(&magic.to_le_bytes());
    bytes[optional + 8..optional + 12].copy_from_slice(&512u32.to_le_bytes());
    bytes[optional + 32..optional + 36].copy_from_slice(&4096u32.to_le_bytes());
    bytes[optional + 36..optional + 40].copy_from_slice(&512u32.to_le_bytes());
    bytes[optional + 56..optional + 60].copy_from_slice(&8192u32.to_le_bytes());
    bytes[optional + 60..optional + 64].copy_from_slice(&512u32.to_le_bytes());
    let count = optional + if host == Host::X86 { 92 } else { 108 };
    bytes[count..count + 4].copy_from_slice(&16u32.to_le_bytes());
    bytes[end..end + 8].copy_from_slice(b".rdata\0\0");
    bytes[end + 8..end + 12].copy_from_slice(&16u32.to_le_bytes());
    bytes[end + 12..end + 16].copy_from_slice(&4096u32.to_le_bytes());
    bytes[end + 16..end + 20].copy_from_slice(&512u32.to_le_bytes());
    bytes[end + 20..end + 24].copy_from_slice(&512u32.to_le_bytes());
    bytes[end + 36..end + 40].copy_from_slice(&0x40000040u32.to_le_bytes());
    bytes[512..528].copy_from_slice(MARKER);
    bytes
}

#[test]
fn complete_data_pe_retains_vendor_padding_timestamp_and_all_three_machine_formats() {
    for host in [Host::X64, Host::X86, Host::Arm64] {
        let bytes = sample_pe(host);
        let original = bytes.clone();
        validate_pe(&bytes, host).unwrap();
        assert_eq!(bytes, original);
        assert!(bytes[528..].iter().all(|byte| *byte == 0xa5));
        for other in [Host::X64, Host::X86, Host::Arm64] {
            if host != other {
                let error = validate_pe(&bytes, other).unwrap_err();
                assert_eq!(error.stage, Stage::LinkerBanner);
                assert_eq!(error.kind, Kind::Pe);
                assert!(error.observed_pe_machine.is_some());
                assert_eq!(error.observed_read_bytes, Some(bytes.len()));
            }
        }
    }
}

#[test]
fn data_pe_refuses_code_entry_loader_directories_and_section_overlap() {
    let original = sample_pe(Host::X64);
    let optional = 152usize;
    let section = optional + 240;
    for (offset, value) in [
        (optional + 4, 1u32),
        (optional + 16, 4096),
        (section + 36, 0x60000040),
        (section + 36, 0xc0000040),
        (section + 8, 17),
        (section + 20, 0),
        (section + 16, 1024),
        (optional + 60, 1024),
    ] {
        let mut bytes = original.clone();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(validate_pe(&bytes, Host::X64).is_err());
    }
    for ordinal in 0..16 {
        let mut bytes = original.clone();
        let offset = optional + 112 + ordinal * 8;
        bytes[offset..offset + 4].copy_from_slice(&4096u32.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&16u32.to_le_bytes());
        assert!(validate_pe(&bytes, Host::X64).is_err());
    }
    let mut marker = original.clone();
    marker[512] = b'X';
    assert!(validate_pe(&marker, Host::X64).is_err());
}

#[test]
fn data_pe_refuses_incomplete_or_overbound_files_and_checked_header_offsets() {
    let original = sample_pe(Host::X64);
    for end in [0, 2, 63, 132, 151, 391, 431, 527, 1023] {
        let error = validate_pe(&original[..end], Host::X64).unwrap_err();
        assert_eq!(error.observed_read_bytes, Some(end));
        assert_eq!(
            error.observed_read_prefix_sha256,
            Some(hash(&original[..end]))
        );
    }
    let mut overflow = original.clone();
    overflow[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(validate_pe(&overflow, Host::X64).is_err());
    let mut overlay = original.clone();
    overlay.push(0);
    assert!(validate_pe(&overlay, Host::X64).is_err());
    let mut too_many = original.clone();
    too_many[260..264].copy_from_slice(&17u32.to_le_bytes());
    assert!(validate_pe(&too_many, Host::X64).is_err());
    assert!(validate_pe(&vec![0; CAPTURE_BYTES + 1], Host::X64).is_err());
}

#[cfg(test)]
fn owner_command(home: &Path) -> Command {
    let mut command = Command::new("fixed-unused-data-format-control");
    command.env_clear();
    for name in ["HOME", "USERPROFILE", "TMPDIR", "TEMP", "TMP"] {
        command.env(name, home);
    }
    command
}

#[test]
fn owned_input_is_complete_create_only_and_output_remains_absent_without_native_link() {
    let home = tempfile::tempdir().unwrap();
    let command = owner_command(home.path());
    let mut owned = Owned::new(&command).unwrap();
    let stamp = owned.clock.started;
    owned.prepare_input(Host::X64).unwrap();
    owned.absent_output(Phase::Input).unwrap();
    assert!((1..=5).contains(&owned.clock.poll_seconds().unwrap()));
    assert_eq!(owned.clock.started, stamp);
    let identity = owned.input.unwrap();
    assert_eq!(identity.size, 98);
    assert_eq!(identity.links, 1);
    assert!(!identity.reparse);
    assert_eq!(
        std::fs::read(&owned.paths.input).unwrap().as_slice(),
        object(Host::X64).as_slice()
    );
    assert!(native_open(&owned.paths.input, false, true).is_err());
    owned.close_all().unwrap();
    assert_eq!(owned.clock.handles, 0);
    assert!(owned.clock.calls <= API_LIMIT);
    assert!(!owned.paths.output.exists());
    assert!(owned.paths.input.exists()); // no fake successful native cleanup
    let mut second = Owned::new(&command).unwrap();
    let failure = second.prepare_input(Host::X64).unwrap_err();
    assert_eq!(failure.expected, Some("probe.child"));
    assert!(failure.raw_os_error.is_some());
    second.close_all().unwrap();
    assert_eq!(
        std::fs::read(&owned.paths.input).unwrap().as_slice(),
        object(Host::X64).as_slice()
    );
    // Plan inspection is pure: this command is never spawned on this host.
    for host in [Host::X64, Host::X86, Host::Arm64] {
        let args = arguments(host);
        assert!(!args.contains(&"/?") && !args.contains(&"/NOLOGO"));
        assert_eq!(args[8], INPUT);
    }
}

#[test]
fn owned_alias_mismatch_and_existing_output_refuse_before_native_dispatch() {
    let home = tempfile::tempdir().unwrap();
    let foreign = tempfile::tempdir().unwrap();
    let mut command = owner_command(home.path());
    command.env("TMP", foreign.path());
    let error = Paths::capture(&command).err().unwrap();
    assert_eq!(error.expected, Some("probe.aliases"));
    assert!(!home.path().join(CHILD).exists());
    let command = owner_command(home.path());
    let mut owned = Owned::new(&command).unwrap();
    owned.prepare_input(Host::X64).unwrap();
    std::fs::write(&owned.paths.output, b"owned-existing-data").unwrap();
    let error = owned.absent_output(Phase::Input).unwrap_err();
    assert_eq!(error.expected, Some("probe.output.absent"));
    assert!(error.process.is_none());
    owned.close_all().unwrap();
    assert_eq!(
        std::fs::read(&owned.paths.output).unwrap(),
        b"owned-existing-data"
    );
}

#[test]
fn owned_output_full_eof_and_residual_directory_failure_keep_real_file_effects() {
    let home = tempfile::tempdir().unwrap();
    let command = owner_command(home.path());
    let mut owned = Owned::new(&command).unwrap();
    owned.prepare_input(Host::X64).unwrap();
    owned.absent_output(Phase::Input).unwrap();
    // Ordinary file/parser control only. It does not invoke run, forge a native
    // exit or supply Prepared. The data file is never loaded or executed.
    let bytes = sample_pe(Host::X64);
    std::fs::write(&owned.paths.output, &bytes).unwrap();
    owned.validate_output(Host::X64).unwrap();
    let residual = owned.paths.child.join("owned-residual-data");
    std::fs::write(&residual, b"ordinary-owned-control").unwrap();
    let error = owned.cleanup_files().unwrap_err();
    assert_eq!(error.expected, Some("probe.cleanup"));
    assert!(error.raw_os_error.is_some());
    owned.close_all().unwrap();
    assert!(owned.clock.calls <= API_LIMIT);
    assert!(!owned.paths.input.exists() && !owned.paths.output.exists());
    assert_eq!(std::fs::read(residual).unwrap(), b"ordinary-owned-control");
    assert!(owned.paths.child.exists());
}

#[test]
fn owned_sentinel_capture_refuses_overflow_with_actual_prefix_facts() {
    let home = tempfile::tempdir().unwrap();
    let command = owner_command(home.path());
    let mut owned = Owned::new(&command).unwrap();
    owned.prepare_input(Host::X64).unwrap();
    let bytes = vec![b'x'; CAPTURE_BYTES + 1];
    std::fs::write(&owned.paths.output, &bytes).unwrap();
    owned
        .open(Slot::Output, Slot::Output, Phase::Validation)
        .unwrap();
    let error = owned
        .read(Slot::Output, Phase::Validation, CAPTURE_BYTES + 1, 33)
        .unwrap_err();
    assert_eq!(error.observed_read_bytes, Some(CAPTURE_BYTES + 1));
    assert_eq!(error.observed_read_prefix_sha256, Some(hash(&bytes)));
    assert_eq!(error.kind, Kind::Bounds);
    assert!(error.process.is_none());
    owned.close_all().unwrap();
    assert!(owned.clock.calls <= API_LIMIT);
    assert_eq!(std::fs::read(&owned.paths.output).unwrap(), bytes);
}

#[test]
fn later_linker_failure_codec_uses_original_fields_and_fits_the_reviewed_envelope() {
    // Source/serialization extrema, never native status/EOF observations.
    let raw = vec![0xff; CAPTURE_BYTES + 1];
    for complete in [false, true] {
        for process_present in [false, true] {
            let mut error = failure(Kind::CaptureReader, "probe.output.absent");
            error.raw_os_error = Some(i32::MIN);
            error.observed_read_bytes = Some(usize::MAX);
            error.observed_read_prefix_sha256 = Some("f".repeat(64));
            error.observed_pe_machine = Some(u16::MAX);
            if process_present {
                error.process = Some(facts::ProcessFacts {
                    spawn_returned_child: false,
                    exit_code: Some(i32::MIN),
                    exit_success: Some(false),
                    stdout: Some(facts::stream(Some(&raw), complete, !complete, true, 2048)),
                    stderr: Some(facts::stream(Some(&raw), complete, !complete, true, 2048)),
                    cleanup: Some(facts::CleanupFacts {
                        kill_returned_ok: false,
                        kill_raw_os_error: Some(i32::MIN),
                        try_wait_returned_status: false,
                        try_wait_raw_os_error: Some(i32::MIN),
                        exit_code_after_cleanup: Some(i32::MIN),
                        bound_reached_without_status: false,
                    }),
                });
            }
            assert!(error.library_observations.is_empty());
            assert!(error.discovery_comparisons.is_none() && error.command_comparisons.is_none());
            let encoded = format!("{error:?}");
            let nested = format!(
                "MSVC-HELPER-FAILURE:{}",
                facts::helper_failure_record(b"", encoded.as_bytes())
            );
            assert!(encoded.is_ascii() && nested.is_ascii());
            assert!(encoded.len() <= 12768);
            assert!(nested.len() + 4096 <= 34192);
            assert!(nested.len() + 4096 < CAPTURE_BYTES);
            println!(
                "SYNTHETIC-LINKER-DATA-BOUND complete={complete} process={process_present} raw={} nested={} framing=4096 cap={CAPTURE_BYTES}",
                encoded.len(),
                nested.len()
            );
        }
    }
}

#[test]
fn private_probe_refuses_unsupported_subpage_raw_and_rva_layouts() {
    let original = sample_pe(Host::X86);
    validate_pe(&original, Host::X86).unwrap();
    for (section_alignment, image_size) in [(1024u32, 2048u32), (512, 1536)] {
        let mut bytes = original.clone();
        let optional = 152usize;
        let section = optional + 224;
        bytes[optional + 32..optional + 36].copy_from_slice(&section_alignment.to_le_bytes());
        bytes[optional + 56..optional + 60].copy_from_slice(&image_size.to_le_bytes());
        bytes[section + 12..section + 16].copy_from_slice(&1024u32.to_le_bytes());
        // These reviewed data-only shapes satisfy the old independent raw/RVA
        // guards, but are unsupported by this private regular-alignment probe.
        let error = validate_pe(&bytes, Host::X86).unwrap_err();
        assert_eq!(error.kind, Kind::Pe);
        assert_eq!(error.observed_read_bytes, Some(bytes.len()));
    }
    validate_pe(&original, Host::X86).unwrap();
}

#[test]
fn reserved_win32_version_and_loader_flags_refuse_on_all_typed_hosts() {
    for host in [Host::X64, Host::X86, Host::Arm64] {
        let original = sample_pe(host);
        let retained = original.clone();
        validate_pe(&original, host).unwrap();
        for offset in [52usize, if host == Host::X86 { 88 } else { 104 }] {
            let mut bytes = original.clone();
            bytes[152 + offset..156 + offset].copy_from_slice(&1u32.to_le_bytes());
            let error = validate_pe(&bytes, host).unwrap_err();
            assert_eq!(error.kind, Kind::Pe);
            assert_eq!(error.observed_read_bytes, Some(bytes.len()));
            assert_eq!(error.observed_read_prefix_sha256, Some(hash(&bytes)));
        }
        validate_pe(&original, host).unwrap();
        assert_eq!(original, retained);
    }
}

#[test]
fn pe_refusal_labels_identify_the_first_failed_header_or_data_group() {
    for host in [Host::X64, Host::X86, Host::Arm64] {
        let original = sample_pe(host);
        let retained = original.clone();
        validate_pe(&original, host).unwrap();
        let directory = 152 + if host == Host::X86 { 96 } else { 112 };
        let count = 152 + if host == Host::X86 { 92 } else { 108 };
        for (offset, value, expected) in [
            (0, b'X', "pe.file"),
            (128, b'X', "pe.signature"),
            (132, original[132] ^ 1, "pe.coff"),
            (152, original[152] ^ 1, "pe.optional.header"),
            (count, 17, "pe.optional.extent"),
            (152 + 52, 1, "pe.entry.reserved"),
            (directory, 1, "pe.d00.rva"),
            (152 + 32, 1, "pe.align.section"),
            (512, b'X', "pe.image.raw.marker"),
        ] {
            let mut bytes = original.clone();
            bytes[offset] = value;
            // A later invalid marker must not replace the first refusal.
            bytes[512] = b'X';
            let error = validate_pe(&bytes, host).unwrap_err();
            assert_eq!(error.stage, Stage::LinkerBanner);
            assert_eq!(error.kind, Kind::Pe);
            assert_eq!(error.expected, Some(expected));
            assert!(expected.is_ascii() && expected.len() <= 19);
            assert_eq!(error.observed_read_bytes, Some(bytes.len()));
            assert_eq!(error.observed_read_prefix_sha256, Some(hash(&bytes)));
        }
        validate_pe(&original, host).unwrap();
        assert_eq!(original, retained);
    }
}

#[test]
fn missing_pe_fields_refuse_in_their_first_checked_group_without_value_defaults() {
    let original = sample_pe(Host::X64);
    for (end, expected) in [
        (0, "pe.file"),
        (1, "pe.file"),
        (2, "pe.signature"),
        (63, "pe.signature"),
        (132, "pe.coff"),
        (151, "pe.coff"),
        (152, "pe.optional.header"),
        (154, "pe.optional.extent"),
        (391, "pe.optional.extent"),
        (432, "pe.align.section"),
        (512, "pe.image.raw.marker"),
        (1023, "pe.image.raw.marker"),
    ] {
        let bytes = &original[..end];
        let error = validate_pe(bytes, Host::X64).unwrap_err();
        assert_eq!(error.kind, Kind::Pe);
        assert_eq!(error.expected, Some(expected));
        assert_eq!(error.observed_read_bytes, Some(end));
        assert_eq!(error.observed_read_prefix_sha256, Some(hash(bytes)));
        assert_eq!(error.observed_pe_machine, (end >= 134).then_some(0x8664));
    }
}

#[test]
fn every_pe_directory_rva_and_size_identifies_its_first_nonzero_operand() {
    for host in [Host::X64, Host::X86, Host::Arm64] {
        let original = sample_pe(host);
        let retained = original.clone();
        let directories = 152 + if host == Host::X86 { 96 } else { 112 };
        validate_pe(&original, host).unwrap();
        for ordinal in 0..16 {
            let offset = directories + ordinal * 8;
            for (rva, size, operand) in [(4096u32, 0u32, "rva"), (0, 16, "size"), (4096, 16, "rva")]
            {
                let mut bytes = original.clone();
                bytes[offset..offset + 4].copy_from_slice(&rva.to_le_bytes());
                bytes[offset + 4..offset + 8].copy_from_slice(&size.to_le_bytes());
                let error = validate_pe(&bytes, host).unwrap_err();
                let expected = format!("pe.d{ordinal:02}.{operand}");
                assert_eq!(error.stage, Stage::LinkerBanner);
                assert_eq!(error.kind, Kind::Pe);
                assert_eq!(error.expected, Some(expected.as_str()));
                assert_eq!(error.observed_read_bytes, Some(bytes.len()));
                assert_eq!(error.observed_read_prefix_sha256, Some(hash(&bytes)));
                assert_eq!(
                    error.observed_pe_machine,
                    Some(if host == Host::X86 {
                        0x14c
                    } else if host == Host::X64 {
                        0x8664
                    } else {
                        0xaa64
                    })
                );
            }
        }
        validate_pe(&original, host).unwrap();
        assert_eq!(original, retained);
    }
}

#[test]
fn directory_order_and_incomplete_headers_keep_the_first_refusal_and_actual_read_facts() {
    for host in [Host::X64, Host::X86, Host::Arm64] {
        let original = sample_pe(host);
        let directories = 152 + if host == Host::X86 { 96 } else { 112 };
        let mut bytes = original.clone();
        bytes[directories + 4..directories + 8].copy_from_slice(&16u32.to_le_bytes());
        bytes[directories + 15 * 8..directories + 15 * 8 + 4]
            .copy_from_slice(&4096u32.to_le_bytes());
        assert_eq!(
            validate_pe(&bytes, host).unwrap_err().expected,
            Some("pe.d00.size")
        );
        bytes[132] ^= 1;
        assert_eq!(
            validate_pe(&bytes, host).unwrap_err().expected,
            Some("pe.coff")
        );
        for ordinal in 0..16 {
            for end in [directories + ordinal * 8 + 3, directories + ordinal * 8 + 7] {
                let bytes = &original[..end];
                let error = validate_pe(bytes, host).unwrap_err();
                assert_eq!(error.kind, Kind::Pe);
                assert_eq!(error.expected, Some("pe.optional.extent"));
                assert_eq!(error.observed_read_bytes, Some(end));
                assert_eq!(error.observed_read_prefix_sha256, Some(hash(bytes)));
            }
        }
        validate_pe(&original, host).unwrap();
    }
}
