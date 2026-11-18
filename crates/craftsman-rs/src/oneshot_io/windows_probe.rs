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
const WINSOCK_2_2: u16 = 0x0202;
const SOL_SOCKET: i32 = 0xffff;
const SO_TYPE: i32 = 0x1008;
const SOCKET_ERROR: i32 = -1;
const WSAENOTSOCK: i32 = 10038;

// WSADATA is400 bytes/alignment4 on Win32 and408/alignment8 on Win64. Its
// version WORD is the documented prefix on both. Own the complete output,
// without treating an arbitrary borrowed output handle as an OwnedSocket.
#[repr(C, align(8))]
struct WsaDataStorage([u8; 512]);

const _: () = assert!(std::mem::size_of::<WsaDataStorage>() >= 408);
const _: () = assert!(std::mem::align_of::<WsaDataStorage>() >= 8);

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

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut c_void) -> i32;
    fn getsockopt(socket: usize, level: i32, name: i32, value: *mut i8, length: *mut i32) -> i32;
    fn WSAGetLastError() -> i32;
    fn WSACleanup() -> i32;
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

#[cfg(test)]
#[derive(Clone, Copy)]
enum DiagnosticPhase {
    Original,
    Duplicate,
}

#[cfg(test)]
std::thread_local! {
    static DIAGNOSTIC_PHASE: std::cell::Cell<DiagnosticPhase> =
        const { std::cell::Cell::new(DiagnosticPhase::Original) };
}

#[cfg(test)]
struct DiagnosticPhaseGuard(DiagnosticPhase);

#[cfg(test)]
impl DiagnosticPhaseGuard {
    fn enter(phase: DiagnosticPhase) -> Self {
        Self(DIAGNOSTIC_PHASE.with(|current| current.replace(phase)))
    }
}

#[cfg(test)]
impl Drop for DiagnosticPhaseGuard {
    fn drop(&mut self) {
        DIAGNOSTIC_PHASE.with(|current| current.set(self.0));
    }
}

#[cfg(test)]
#[derive(serde::Serialize)]
struct QueryDiagnostic {
    stage: &'static str,
    phase: &'static str,
    class: u32,
    returned_status: i32,
    iosb_status_after: i32,
    iosb_bytes_after: usize,
    requested_bytes: usize,
    validated_complete: bool,
    fields: Option<Vec<(&'static str, u32)>>,
}

#[cfg(test)]
#[derive(serde::Serialize)]
struct SocketDiagnostic {
    stage: &'static str,
    startup_result: i32,
    version: Option<u16>,
    query_result: Option<i32>,
    query_error: Option<i32>,
    query_length: Option<i32>,
    socket_type: Option<i32>,
    cleanup_result: Option<i32>,
    cleanup_error: Option<i32>,
}

#[cfg(test)]
#[derive(serde::Serialize)]
pub(super) struct QueryDiagnostics {
    records: Vec<QueryDiagnostic>,
    socket_checks: Vec<SocketDiagnostic>,
    truncated: bool,
}

#[cfg(test)]
static QUERY_DIAGNOSTICS: std::sync::Mutex<Option<QueryDiagnostics>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(super) fn enable_query_diagnostics() {
    if let Ok(mut diagnostics) = QUERY_DIAGNOSTICS.lock() {
        *diagnostics = Some(QueryDiagnostics {
            records: Vec::with_capacity(32),
            socket_checks: Vec::new(),
            truncated: false,
        });
    }
}

#[cfg(test)]
pub(super) fn take_query_diagnostics() -> Option<QueryDiagnostics> {
    QUERY_DIAGNOSTICS.lock().ok()?.take()
}

#[cfg(test)]
fn record_query(
    stage: &'static str,
    class: u32,
    returned_status: i32,
    status: &IoStatusBlock,
    requested_bytes: usize,
    fields: &[(&'static str, u32)],
) {
    let Ok(mut diagnostics) = QUERY_DIAGNOSTICS.lock() else {
        return;
    };
    let Some(diagnostics) = diagnostics.as_mut() else {
        return;
    };
    if diagnostics.records.len() + diagnostics.socket_checks.len() == 32 {
        diagnostics.truncated = true;
        return;
    }
    let phase = DIAGNOSTIC_PHASE.with(|current| match current.get() {
        DiagnosticPhase::Original => "original",
        DiagnosticPhase::Duplicate => "duplicate",
    });
    let validated_complete =
        returned_status == 0 && status.status as i32 == 0 && status.information == requested_bytes;
    diagnostics.records.push(QueryDiagnostic {
        stage,
        phase,
        class,
        returned_status,
        iosb_status_after: status.status as i32,
        iosb_bytes_after: status.information,
        requested_bytes,
        validated_complete,
        // Never turn initialized/partial/error output into native field facts.
        fields: validated_complete.then(|| fields.to_vec()),
    });
}

#[cfg(test)]
fn record_socket(check: SocketDiagnostic) {
    let Ok(mut diagnostics) = QUERY_DIAGNOSTICS.lock() else {
        return;
    };
    let Some(diagnostics) = diagnostics.as_mut() else {
        return;
    };
    if diagnostics.records.len() + diagnostics.socket_checks.len() == 32 {
        diagnostics.truncated = true;
    } else {
        diagnostics.socket_checks.push(check);
    }
}

fn non_socket_candidate(handle: Handle) -> io::Result<()> {
    // Only the direct-kernel8/16 bootstrap precedes this provider-facing call.
    eligibility(handle)?;
    let mut data = WsaDataStorage([0; 512]);
    // WSAStartup returns its own error, not a WSAGetLastError observation.
    let startup = unsafe { WSAStartup(WINSOCK_2_2, data.0.as_mut_ptr().cast()) };
    if startup != 0 {
        #[cfg(test)]
        record_socket(SocketDiagnostic {
            stage: "socket_type",
            startup_result: startup,
            version: None,
            query_result: None,
            query_error: None,
            query_length: None,
            socket_type: None,
            cleanup_result: None,
            cleanup_error: None,
        });
        return Err(unsupported());
    }
    let version = u16::from_ne_bytes([data.0[0], data.0[1]]);
    let mut candidate = false;
    #[cfg(test)]
    let mut query_result = None;
    #[cfg(test)]
    let mut query_error = None;
    #[cfg(test)]
    let mut query_length = None;
    #[cfg(test)]
    let mut socket_type = None;
    if version == WINSOCK_2_2 {
        let mut value = 0i32;
        let mut length = 4i32;
        // getsockopt owns no asynchronous buffer loan. Keep selected HANDLE,
        // full startup output/reference and both int32 buffers through return.
        // The documented call can block/use alertable waits; no tighter OS
        // deadline or arbitrary foreign Winsock lifecycle proof is claimed.
        let result = unsafe {
            getsockopt(
                handle as usize,
                SOL_SOCKET,
                SO_TYPE,
                (&mut value as *mut i32).cast(),
                &mut length,
            )
        };
        let error = if result == SOCKET_ERROR {
            Some(unsafe { WSAGetLastError() })
        } else {
            None
        };
        candidate = result == SOCKET_ERROR && error == Some(WSAENOTSOCK);
        #[cfg(test)]
        {
            query_result = Some(result);
            query_error = error;
            query_length = Some(length);
            // Success of any socket type, malformed length or other result
            // still refuses. Error buffers never become type observations.
            socket_type = (result == 0 && length == 4).then_some(value);
        }
    }
    // No successful-startup path returns before its one matching cleanup,
    // including version mismatch/query failure. Do not retry a failed cleanup
    // or consume a caller's reference; only a checked success admits a candidate.
    let cleanup = unsafe { WSACleanup() };
    #[cfg(test)]
    let cleanup_error = if cleanup != 0 {
        Some(unsafe { WSAGetLastError() })
    } else {
        None
    };
    #[cfg(test)]
    record_socket(SocketDiagnostic {
        stage: "socket_type",
        startup_result: startup,
        version: Some(version),
        query_result,
        query_error,
        query_length,
        socket_type,
        cleanup_result: Some(cleanup),
        cleanup_error,
    });
    if cleanup != 0 || !candidate {
        return Err(unsupported());
    }
    Ok(())
}

fn duplicate_stdout() -> io::Result<OwnedHandle> {
    #[cfg(test)]
    let _phase = DiagnosticPhaseGuard::enter(DiagnosticPhase::Original);
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
    let kind = captured_type(source)?;
    if kind == 3 {
        // A device0x11 label does not distinguish NPFS from Winsock sockets.
        // Only a checked native non-socket candidate may reach the remaining
        // supported-device/state checks and socket-incompatible duplication.
        // This borrows the once-read caller handle; arbitrary unsafe concurrent
        // handle-table changes are outside this capture contract.
        non_socket_candidate(source)?;
        let information = device(source)?;
        if information.kind != NAMED_PIPE_DEVICE || information.characteristics & REMOTE_DEVICE != 0
        {
            return Err(unsupported());
        }
    }
    let mut duplicate = null_mut();
    #[cfg(test)]
    DUPLICATE_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
    // SAFETY: the caller retains the selected original or duplicate through
    // the call. These two classes are supplied
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
    #[cfg(test)]
    record_query(
        match class {
            DirectInformation::Access => "file_access",
            DirectInformation::Mode => "file_mode",
        },
        class as u32,
        result,
        &status,
        4,
        &[("value", information)],
    );
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
static DUPLICATE_ATTEMPTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn duplicate_attempts() -> usize {
    DUPLICATE_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst)
}

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
    #[cfg(test)]
    record_query(
        "fs_device",
        4,
        result,
        &status,
        8,
        &[
            ("device_type", information.kind),
            ("characteristics", information.characteristics),
        ],
    );
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
    #[cfg(test)]
    record_query(
        "file_standard",
        5,
        result,
        &status,
        24,
        &[("directory", u32::from(information.directory))],
    );
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
    #[cfg(test)]
    record_query(
        "file_pipe_local",
        24,
        result,
        &status,
        40,
        &[
            ("type", information.kind),
            ("configuration", information.configuration),
            ("maximum_instances", information._maximum_instances),
            ("current_instances", information._current_instances),
            ("inbound_quota", information._inbound_quota),
            ("read_available", information._read_available),
            ("outbound_quota", information._outbound_quota),
            ("write_available", information._write_available),
            ("state", information.state),
            ("end", information.end),
        ],
    );
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
        #[cfg(test)]
        let _phase = DiagnosticPhaseGuard::enter(DiagnosticPhase::Duplicate);
        eligibility(self.file.as_raw_handle())?;
        #[cfg(test)]
        FILE_WRITES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (&*self.file).write(bytes).map_err(write_error)
    }
    fn flush(&mut self) -> io::Result<()> {
        #[cfg(test)]
        let _phase = DiagnosticPhaseGuard::enter(DiagnosticPhase::Duplicate);
        eligibility(self.file.as_raw_handle())?;
        (&*self.file).flush().map_err(write_error)
    }
}

pub(super) fn capture() -> io::Result<Arc<Output<StdoutWriter>>> {
    let owner = duplicate_stdout()?;
    #[cfg(test)]
    let _phase = DiagnosticPhaseGuard::enter(DiagnosticPhase::Duplicate);
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
        Some(Box::new(move || {
            #[cfg(test)]
            let _phase = DiagnosticPhaseGuard::enter(DiagnosticPhase::Duplicate);
            pipe_state(file.as_raw_handle()).map(|_| ())
        }) as Probe)
    } else {
        None
    };
    let mut output = Output::new(Box::new(CheckedFile { file }) as StdoutWriter);
    output.probe = probe;
    Ok(Arc::new(output))
}
