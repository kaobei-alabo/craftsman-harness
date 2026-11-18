//! Captured, same-access Windows output with a synchronous-mode precondition.
//!
//! Only FileAccessInformation8/FileModeInformation16 bootstrap before the mode
//! is known. Microsoft documents that these classes are supplied directly by
//! the I/O manager, without a filesystem IRP:
//! https://learn.microsoft.com/en-us/previous-versions/windows/drivers/ifs/irp-mj-query-information
//! Driver-facing queries and File writes follow actual NONALERT eligibility.

use super::{ErrorKind, Output, Probe, StdoutWriter};
use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::null_mut;
use std::sync::Arc;

type Handle = *mut c_void;
const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;
const SAME_ACCESS: u32 = 2;
const WRITE_DATA: u32 = 2;
const APPEND_DATA: u32 = 4;
const SYNCHRONIZE: u32 = 0x0010_0000;
const SYNCHRONOUS: u32 = 0x10 | 0x20;
const NONALERT: u32 = 0x20;
const UNBUFFERED: u32 = 8;
const NAMED_PIPE_DEVICE: u32 = 0x11;
const NULL_DEVICE: u32 = 0x15;
const REMOTE_DEVICE: u32 = 0x10;

#[repr(C)]
struct IoStatusBlock {
    status: usize,
    information: usize,
}

#[repr(C)]
struct DeviceInformation {
    kind: u32,
    characteristics: u32,
}

#[repr(C)]
struct StandardInformation {
    _allocation_size: i64,
    _end_of_file: i64,
    _links: u32,
    _delete_pending: u8,
    directory: u8,
    _padding: [u8; 2],
}

#[repr(C)]
struct PipeLocalInformation {
    kind: u32,
    configuration: u32,
    _maximum_instances: u32,
    _current_instances: u32,
    _inbound_quota: u32,
    _read_available: u32,
    _outbound_quota: u32,
    _write_available: u32,
    state: u32,
    end: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn GetCurrentProcess() -> Handle;
    fn GetLastError() -> u32;
    fn SetLastError(error: u32);
    fn GetFileType(file: Handle) -> u32;
    fn GetConsoleMode(console: Handle, mode: *mut u32) -> i32;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationFile(
        file: Handle,
        status: *mut IoStatusBlock,
        information: *mut c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn NtQueryVolumeInformationFile(
        file: Handle,
        status: *mut IoStatusBlock,
        information: *mut c_void,
        length: u32,
        class: u32,
    ) -> i32;
}

fn unsupported() -> io::Error {
    io::Error::new(ErrorKind::Unsupported, "unsupported one-shot output")
}

fn native_error(error: u32) -> io::Error {
    if error == 0 {
        unsupported()
    } else {
        io::Error::from_raw_os_error(error as i32)
    }
}

fn invalid(handle: Handle) -> bool {
    handle.is_null() || handle as usize == usize::MAX
}

fn duplicate_stdout() -> io::Result<OwnedHandle> {
    // These Win32 handle-table operations do not start file IO. Invalid/null
    // values are rejected before Rust's special BorrowedHandle interpretation.
    let (source, error) = unsafe {
        SetLastError(0);
        let source = GetStdHandle(STD_OUTPUT_HANDLE);
        (source, GetLastError())
    };
    if invalid(source) {
        return Err(native_error(error));
    }
    let mut duplicate = null_mut();
    let result = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            source,
            GetCurrentProcess(),
            &mut duplicate,
            0,
            0,
            SAME_ACCESS,
        )
    };
    if result == 0 {
        return Err(native_error(unsafe { GetLastError() }));
    }
    if invalid(duplicate) {
        return Err(unsupported());
    }
    // SAFETY: successful same-access, non-inheritable duplication supplied one
    // live owned HANDLE; no IO mode or access rights were normalized/upgraded.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
}

fn captured_type(handle: Handle) -> io::Result<u32> {
    let (kind, error) = unsafe {
        SetLastError(0);
        let kind = GetFileType(handle);
        (kind, GetLastError())
    };
    match kind {
        1..=3 => Ok(kind),
        0 => Err(native_error(error)),
        _ => Err(unsupported()),
    }
}

#[derive(Clone, Copy)]
enum DirectInformation {
    Access = 8,
    Mode = 16,
}

fn direct_information(handle: Handle, class: DirectInformation) -> io::Result<u32> {
    let mut status = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let mut information = 0u32;
    // SAFETY: the caller retains the duplicate. These two classes are supplied
    // directly by the I/O manager without a filesystem IRP; this documented
    // pre-call invariant supplies buffer lifetime before synchronous mode is
    // known. It is not an inference from a completed status or a class24 claim.
    let result = unsafe {
        NtQueryInformationFile(
            handle,
            &mut status,
            (&mut information as *mut u32).cast(),
            4,
            class as u32,
        )
    };
    if result != 0 || status.status as i32 != result || status.information != 4 {
        Err(unsupported())
    } else {
        Ok(information)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Eligibility {
    pub(super) access: u32,
    pub(super) mode: u32,
}

pub(super) fn bootstrap(handle: Handle) -> io::Result<Eligibility> {
    let access = direct_information(handle, DirectInformation::Access)?;
    let mode = direct_information(handle, DirectInformation::Mode)?;
    Ok(Eligibility { access, mode })
}

pub(super) fn eligibility(handle: Handle) -> io::Result<Eligibility> {
    let eligibility = bootstrap(handle)?;
    if eligibility.access & (WRITE_DATA | APPEND_DATA) == 0
        || eligibility.access & SYNCHRONIZE == 0
        || eligibility.mode & SYNCHRONOUS != NONALERT
        || eligibility.mode & UNBUFFERED != 0
    {
        return Err(unsupported());
    }
    Ok(eligibility)
}

#[cfg(test)]
static DRIVER_QUERIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static FILE_WRITES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn activity_counts() -> (usize, usize) {
    use std::sync::atomic::Ordering;
    (
        DRIVER_QUERIES.load(Ordering::SeqCst),
        FILE_WRITES.load(Ordering::SeqCst),
    )
}

fn completed(result: i32, status: &IoStatusBlock, bytes: usize) -> io::Result<()> {
    if result == 0 && status.status as i32 == result && status.information == bytes {
        Ok(())
    } else {
        Err(unsupported())
    }
}

fn device(handle: Handle) -> io::Result<DeviceInformation> {
    eligibility(handle)?;
    #[cfg(test)]
    DRIVER_QUERIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut status = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let mut information = DeviceInformation {
        kind: 0,
        characteristics: 0,
    };
    // SAFETY: the direct-kernel bootstrap above establishes actual NONALERT
    // synchronous IO before this driver-facing query. The retained owner/lock
    // covers its completed call; no alertable/async or cancellation path exists.
    let result = unsafe {
        NtQueryVolumeInformationFile(
            handle,
            &mut status,
            (&mut information as *mut DeviceInformation).cast(),
            8,
            4,
        )
    };
    completed(result, &status, 8)?;
    Ok(information)
}

fn disk(handle: Handle) -> io::Result<()> {
    eligibility(handle)?;
    #[cfg(test)]
    DRIVER_QUERIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut status = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let mut information = StandardInformation {
        _allocation_size: 0,
        _end_of_file: 0,
        _links: 0,
        _delete_pending: 0,
        directory: 1,
        _padding: [0; 2],
    };
    // SAFETY: actual NONALERT mode precedes this completed driver query.
    let result = unsafe {
        NtQueryInformationFile(
            handle,
            &mut status,
            (&mut information as *mut StandardInformation).cast(),
            24,
            5,
        )
    };
    completed(result, &status, 24)?;
    if information.directory != 0 {
        Err(unsupported())
    } else {
        Ok(())
    }
}

pub(super) fn null_device(handle: Handle) -> io::Result<()> {
    let information = device(handle)?;
    if information.kind == NULL_DEVICE && information.characteristics & REMOTE_DEVICE == 0 {
        Ok(())
    } else {
        Err(unsupported())
    }
}

pub(super) fn pipe_state(handle: Handle) -> io::Result<u32> {
    let local = device(handle)?;
    if local.kind != NAMED_PIPE_DEVICE || local.characteristics & REMOTE_DEVICE != 0 {
        return Err(unsupported());
    }
    eligibility(handle)?;
    #[cfg(test)]
    DRIVER_QUERIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut status = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let mut information = PipeLocalInformation {
        kind: u32::MAX,
        configuration: u32::MAX,
        _maximum_instances: 0,
        _current_instances: 0,
        _inbound_quota: 0,
        _read_available: 0,
        _outbound_quota: 0,
        _write_available: 0,
        state: 0,
        end: u32::MAX,
    };
    // SAFETY: this local NPFS call follows the actual NONALERT mode/access and
    // device checks on the same retained owner, not GetFileType's PIPE label.
    // No READ_ATTRIBUTES upgrade/assumption, reads, writes or mode change occur.
    let result = unsafe {
        NtQueryInformationFile(
            handle,
            &mut status,
            (&mut information as *mut PipeLocalInformation).cast(),
            40,
            24,
        )
    };
    if matches!(result as u32, 0xc000_00b0 | 0xc000_00b1 | 0xc000_014b) {
        return Err(io::Error::from(ErrorKind::BrokenPipe));
    }
    completed(result, &status, 40)?;
    if information.kind != 0 || information.configuration > 2 || information.end > 1 {
        return Err(unsupported());
    }
    match information.state {
        3 => Ok(3),
        1 | 4 => Err(io::Error::from(ErrorKind::BrokenPipe)),
        _ => Err(unsupported()),
    }
}

struct CheckedFile {
    file: Arc<File>,
}

fn write_error(error: io::Error) -> io::Error {
    match error.raw_os_error() {
        Some(109 | 232 | 233) => io::Error::from(ErrorKind::BrokenPipe),
        _ => error,
    }
}

impl Write for CheckedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        eligibility(self.file.as_raw_handle())?;
        #[cfg(test)]
        FILE_WRITES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (&*self.file).write(bytes).map_err(write_error)
    }
    fn flush(&mut self) -> io::Result<()> {
        eligibility(self.file.as_raw_handle())?;
        (&*self.file).flush().map_err(write_error)
    }
}

pub(super) fn capture() -> io::Result<Arc<Output<StdoutWriter>>> {
    let owner = duplicate_stdout()?;
    let raw = owner.as_raw_handle();
    let kind = captured_type(raw)?;
    if kind == 2 {
        let mut mode = 0;
        // GetConsoleMode is a synchronous console API, not a file query/write.
        if unsafe { GetConsoleMode(raw, &mut mode) } != 0 {
            // Preserve the prior global Stdout Unicode behavior. This console
            // path is not native duplicate-owned redirected output proof.
            return Ok(Arc::new(Output::new(Box::new(std::io::stdout()))));
        }
        // GetConsoleMode failure is not permission to adopt an arbitrary char
        // device. device() first revalidates direct8/16 NONALERT/write access,
        // then completes the native device query on this same retained owner.
        null_device(raw)?;
        // Native NUL is ordinary output discarding, not a closed connection.
        // It uses real checked File IO below and has no peer-state probe.
    }
    eligibility(raw)?;
    if kind == 1 {
        disk(raw)?;
    }
    if kind == 3 {
        match pipe_state(raw) {
            Ok(_) => {}
            // A real closed local pipe retains an output owner. Defer its
            // observation; do not latch before the original admitted request.
            Err(error) if error.kind() == ErrorKind::BrokenPipe => {}
            Err(error) => return Err(error),
        }
    }
    // Only now is std File allowed to perform synchronous IO on this handle.
    let file = Arc::new(File::from(owner));
    let probe = if kind == 3 {
        let file = file.clone();
        Some(Box::new(move || pipe_state(file.as_raw_handle()).map(|_| ())) as Probe)
    } else {
        None
    };
    let mut output = Output::new(Box::new(CheckedFile { file }) as StdoutWriter);
    output.probe = probe;
    Ok(Arc::new(output))
}
