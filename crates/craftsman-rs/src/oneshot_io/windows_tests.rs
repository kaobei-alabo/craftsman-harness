//! Marked real Windows handle/capture/builder controls. These do not execute on
//! Unix; injected metadata controls never stand in for native Windows proof.

use super::*;
use crate::agent::{SessionExtras, build_session_ext};
use crate::config::Config;
use crate::hitl::ApprovalState;
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, AsRawSocket, FromRawHandle, IntoRawSocket, OwnedHandle};
use std::path::Path;
use std::process::Stdio;
use std::ptr::{null, null_mut};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

type Handle = *mut c_void;
const MARKER: &str = "--oneshot-windows-output-v2";
const HELPER: &str = "oneshot_io::windows_tests::windows_output_fixture";
const STDOUT: u32 = (-11i32) as u32;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const READ_ATTRIBUTES: u32 = 0x80;
const OVERLAPPED: u32 = 0x4000_0000;
const INHERITED_SOCKET: &str = "CRAFTSMAN_TEST_INHERITED_SOCKET_V1";

#[repr(C)]
struct SecurityAttributes {
    size: u32,
    descriptor: *mut c_void,
    inherit: i32,
}

#[repr(C)]
struct StartupInfo {
    size: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved_size: u16,
    reserved_bytes: *mut u8,
    input: Handle,
    output: Handle,
    error: Handle,
}

#[repr(C)]
struct StartupInfoEx {
    startup: StartupInfo,
    attributes: *mut c_void,
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

#[repr(C, align(16))]
struct AttributeStorage([u8; 4096]);

const _: () = assert!(
    std::mem::size_of::<StartupInfo>()
        == if std::mem::size_of::<usize>() == 8 {
            104
        } else {
            68
        }
);
const _: () = assert!(
    std::mem::size_of::<StartupInfoEx>()
        == if std::mem::size_of::<usize>() == 8 {
            112
        } else {
            72
        }
);
const _: () = assert!(
    std::mem::size_of::<ProcessInformation>()
        == if std::mem::size_of::<usize>() == 8 {
            24
        } else {
            16
        }
);
const _: () = assert!(
    std::mem::size_of::<SecurityAttributes>()
        == if std::mem::size_of::<usize>() == 8 {
            24
        } else {
            12
        }
);

#[repr(C)]
struct Coord {
    x: i16,
    y: i16,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetStdHandle(which: u32, handle: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetFileType(handle: Handle) -> u32;
    fn SetLastError(error: u32);
    fn GetCurrentProcess() -> Handle;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn CreatePipe(
        reader: *mut Handle,
        writer: *mut Handle,
        attributes: *mut c_void,
        size: u32,
    ) -> i32;
    fn CreateNamedPipeW(
        name: *const u16,
        access: u32,
        mode: u32,
        instances: u32,
        output: u32,
        input: u32,
        timeout: u32,
        attributes: *mut c_void,
    ) -> Handle;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        attributes: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn DisconnectNamedPipe(handle: Handle) -> i32;
    fn CreateEventW(attributes: *mut c_void, manual: i32, initial: i32, name: *const u16)
    -> Handle;
    fn GetConsoleProcessList(processes: *mut u32, capacity: u32) -> u32;
    fn GetConsoleMode(console: Handle, mode: *mut u32) -> i32;
    fn AllocConsole() -> i32;
    fn FreeConsole() -> i32;
    fn CreateConsoleScreenBuffer(
        access: u32,
        share: u32,
        attributes: *mut c_void,
        flags: u32,
        data: *mut c_void,
    ) -> Handle;
    fn ReadConsoleOutputCharacterW(
        console: Handle,
        buffer: *mut u16,
        length: u32,
        position: Coord,
        read: *mut u32,
    ) -> i32;
    fn GetHandleInformation(handle: Handle, flags: *mut u32) -> i32;
    fn InitializeProcThreadAttributeList(
        attributes: *mut c_void,
        count: u32,
        flags: u32,
        bytes: *mut usize,
    ) -> i32;
    fn UpdateProcThreadAttribute(
        attributes: *mut c_void,
        flags: u32,
        key: usize,
        value: *mut c_void,
        bytes: usize,
        previous: *mut c_void,
        returned: *mut usize,
    ) -> i32;
    fn DeleteProcThreadAttributeList(attributes: *mut c_void);
    fn CreateProcessW(
        application: *const u16,
        command: *mut u16,
        process_security: *mut c_void,
        thread_security: *mut c_void,
        inherit: i32,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *mut StartupInfo,
        information: *mut ProcessInformation,
    ) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn TerminateProcess(process: Handle, code: u32) -> i32;
    fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
}

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut c_void) -> i32;
    fn WSACleanup() -> i32;
    fn WSASocketW(
        family: i32,
        kind: i32,
        protocol: i32,
        info: *mut c_void,
        group: u32,
        flags: u32,
    ) -> usize;
    fn closesocket(socket: usize) -> i32;
    fn getsockopt(socket: usize, level: i32, name: i32, value: *mut i8, length: *mut i32) -> i32;
    fn WSAGetLastError() -> i32;
}

fn owned(handle: Handle) -> OwnedHandle {
    assert!(
        !handle.is_null() && handle as usize != usize::MAX,
        "native fixture creation failed: {}",
        unsafe { GetLastError() }
    );
    // SAFETY: only successful create/duplicate operations call this helper.
    unsafe { OwnedHandle::from_raw_handle(handle) }
}

fn select(handle: Handle) {
    assert_ne!(unsafe { SetStdHandle(STDOUT, handle) }, 0);
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn anonymous() -> (OwnedHandle, OwnedHandle) {
    let mut read = null_mut();
    let mut write = null_mut();
    assert_ne!(
        unsafe { CreatePipe(&mut read, &mut write, null_mut(), 4096) },
        0
    );
    (owned(read), owned(write))
}

fn named(
    server_output: bool,
    server_overlap: bool,
    client_overlap: bool,
    client_attributes: bool,
) -> (OwnedHandle, OwnedHandle) {
    let name = wide(&format!(
        "\\\\.\\pipe\\craftsman-output-v2-{}",
        std::process::id()
    ));
    let access = if server_output { 2 } else { 1 } | if server_overlap { OVERLAPPED } else { 0 };
    let server =
        owned(unsafe { CreateNamedPipeW(name.as_ptr(), access, 0, 1, 4096, 4096, 0, null_mut()) });
    let client_access = if server_output {
        GENERIC_READ
    } else {
        GENERIC_WRITE
            | if client_attributes {
                READ_ATTRIBUTES
            } else {
                0
            }
    };
    // CreateFile connects to the existing listening instance. No pending
    // ConnectNamedPipe/OVERLAPPED request is issued by the fixture.
    let client = owned(unsafe {
        CreateFileW(
            name.as_ptr(),
            client_access,
            0,
            null_mut(),
            3,
            if client_overlap { OVERLAPPED } else { 0 },
            null_mut(),
        )
    });
    if server_output {
        (client, server)
    } else {
        (server, client)
    }
}

fn narrow_write(handle: &OwnedHandle) -> OwnedHandle {
    let mut result = null_mut();
    // Only a test-side rights reduction; production always duplicates SAME_ACCESS.
    assert_ne!(
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                handle.as_raw_handle(),
                GetCurrentProcess(),
                &mut result,
                0x0010_0002,
                0,
                0,
            )
        },
        0
    );
    owned(result)
}

fn raw_facts(handle: Handle) -> Value {
    let kind = unsafe {
        SetLastError(0);
        GetFileType(handle)
    };
    let type_error = unsafe { GetLastError() };
    match super::windows_probe::bootstrap(handle) {
        Ok(info) => {
            json!({"type":kind,"type_error":type_error,"access":info.access,"mode":info.mode})
        }
        Err(bootstrap_error) => {
            json!({"type":kind,"type_error":type_error,"eligibility_error":format!("{:?}",bootstrap_error.kind())})
        }
    }
}

fn console_attachment_facts() -> (u32, u32) {
    // Only the returned attachment count is inspected; no PID is queried or
    // reported. A console can be attached without a visible console window.
    let mut unused_process = 0u32;
    unsafe {
        SetLastError(0);
        let count = GetConsoleProcessList(&mut unused_process, 1);
        (count, GetLastError())
    }
}

fn usable_socket_type(socket: usize) -> i32 {
    let mut value = 0i32;
    let mut length = 4i32;
    let (result, error) = unsafe {
        let result = getsockopt(
            socket,
            0xffff,
            0x1008,
            (&mut value as *mut i32).cast(),
            &mut length,
        );
        (result, if result == -1 { WSAGetLastError() } else { 0 })
    };
    assert_eq!(result, 0, "actual caller socket query error {error}");
    assert_eq!(length, 4);
    assert!(value > 0);
    value
}

fn quoted_argument(value: &OsStr) -> Vec<u16> {
    let mut result = vec![b'"' as u16];
    let mut slashes = 0usize;
    for word in value.encode_wide() {
        match word {
            92 => slashes += 1,
            34 => {
                result.extend(std::iter::repeat_n(92, slashes * 2 + 1));
                result.push(word);
                slashes = 0;
            }
            _ => {
                result.extend(std::iter::repeat_n(92, slashes));
                slashes = 0;
                result.push(word);
            }
        }
    }
    result.extend(std::iter::repeat_n(92, slashes * 2));
    result.push(34);
    result
}

struct ExplicitInheritance {
    storage: Box<AttributeStorage>,
    handles: Box<[Handle; 3]>,
    initialized: bool,
}

impl ExplicitInheritance {
    fn new(handles: [Handle; 3]) -> Result<Self, String> {
        let mut bytes = 0usize;
        unsafe { SetLastError(0) };
        let sizing = unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        let error = unsafe { GetLastError() };
        if sizing != 0 || error != 122 || bytes == 0 || bytes > 4096 {
            return Err(format!("native inheritance sizing failed {error}"));
        }
        let mut owned = Self {
            storage: Box::new(AttributeStorage([0; 4096])),
            handles: Box::new(handles),
            initialized: false,
        };
        let list = owned.storage.0.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0 {
            return Err(format!(
                "native inheritance initialization failed {}",
                unsafe { GetLastError() }
            ));
        }
        owned.initialized = true;
        if unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                0x0002_0002,
                owned.handles.as_mut_ptr().cast(),
                std::mem::size_of::<[Handle; 3]>(),
                null_mut(),
                null_mut(),
            )
        } == 0
        {
            return Err(format!("native explicit inheritance failed {}", unsafe {
                GetLastError()
            }));
        }
        Ok(owned)
    }
}

impl Drop for ExplicitInheritance {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { DeleteProcThreadAttributeList(self.storage.0.as_mut_ptr().cast()) };
        }
    }
}

struct NativeChild {
    process: OwnedHandle,
    _thread: Option<OwnedHandle>,
    reaped: bool,
    cleanup_attempted: bool,
}

impl NativeChild {
    fn cleanup(&mut self) -> Result<(), String> {
        if self.reaped {
            return Ok(());
        }
        if self.cleanup_attempted {
            return Err("native child cleanup already failed".to_owned());
        }
        self.cleanup_attempted = true;
        if unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } == 0 {
            self.reaped = true;
            return Ok(());
        }
        let requested = unsafe { TerminateProcess(self.process.as_raw_handle(), 99) };
        let error = if requested == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        let waited = unsafe { WaitForSingleObject(self.process.as_raw_handle(), 5000) };
        if waited != 0 {
            return Err(format!(
                "native child cleanup did not reap {waited} termination_error {error}"
            ));
        }
        self.reaped = true;
        Ok(())
    }

    fn wait(mut self) -> Result<u32, String> {
        let waited = unsafe { WaitForSingleObject(self.process.as_raw_handle(), 10000) };
        if waited != 0 {
            self.cleanup()?;
            return Err(format!("native child wait refused or timed out {waited}"));
        }
        self.reaped = true;
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
            return Err(format!("native child exit query failed {}", unsafe {
                GetLastError()
            }));
        }
        Ok(code)
    }
}

impl Drop for NativeChild {
    fn drop(&mut self) {
        if !self.reaped && !self.cleanup_attempted && self.cleanup().is_err() {
            eprintln!("native inherited child cleanup failed");
        }
    }
}

struct CallerSocket {
    socket: usize,
    reference: bool,
}

impl CallerSocket {
    fn new() -> Self {
        let mut data = [0usize; 128];
        assert_eq!(unsafe { WSAStartup(0x0202, data.as_mut_ptr().cast()) }, 0);
        let mut owned = Self {
            socket: usize::MAX,
            reference: true,
        };
        assert_eq!(
            unsafe { std::ptr::read(data.as_ptr().cast::<u16>()) },
            0x0202
        );
        owned.socket = unsafe { WSASocketW(2, 1, 6, null_mut(), 0, 0) };
        assert_ne!(owned.socket, usize::MAX);
        assert_eq!(usable_socket_type(owned.socket), 1);
        owned
    }

    fn close(&mut self) -> Result<(), String> {
        if self.socket != usize::MAX {
            let socket = std::mem::replace(&mut self.socket, usize::MAX);
            if unsafe { closesocket(socket) } != 0 {
                return Err("native caller socket close failed".to_owned());
            }
        }
        if self.reference {
            self.reference = false;
            if unsafe { WSACleanup() } != 0 {
                return Err("native caller cleanup failed".to_owned());
            }
        }
        Ok(())
    }
}

impl Drop for CallerSocket {
    fn drop(&mut self) {
        if self.close().is_err() {
            eprintln!("native caller cleanup failed");
        }
    }
}

fn launch_inherited_socket(
    socket: usize,
    home: &Path,
    workspace: &Path,
    endpoint: &str,
    completed: &Path,
    forbidden: &Path,
    stderr_path: &Path,
) -> Result<NativeChild, String> {
    let mut security = SecurityAttributes {
        size: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: null_mut(),
        inherit: 1,
    };
    let nul_name = wide("NUL");
    let nul = owned(unsafe {
        CreateFileW(
            nul_name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            3,
            (&mut security as *mut SecurityAttributes).cast(),
            3,
            0,
            null_mut(),
        )
    });
    let stderr_name: Vec<u16> = stderr_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let stderr = owned(unsafe {
        CreateFileW(
            stderr_name.as_ptr(),
            GENERIC_WRITE,
            3,
            (&mut security as *mut SecurityAttributes).cast(),
            2,
            0,
            null_mut(),
        )
    });
    for (stage, handle) in [
        ("parent_socket", socket as Handle),
        ("parent_null", nul.as_raw_handle()),
        ("parent_stderr", stderr.as_raw_handle()),
    ] {
        let mut flags = 0u32;
        if unsafe { GetHandleInformation(handle, &mut flags) } == 0 || flags & 1 == 0 {
            return Err("native intended handle is not inheritable".to_owned());
        }
        eprintln!(
            "native-output-inheritance: {}",
            json!({"stage":stage,"inherit_bit":flags & 1 != 0})
        );
    }
    let mut inheritance = ExplicitInheritance::new([
        socket as Handle,
        nul.as_raw_handle(),
        stderr.as_raw_handle(),
    ])?;
    let executable =
        std::env::current_exe().map_err(|_| "native helper executable unavailable".to_owned())?;
    let application: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let arguments: Vec<OsString> = vec![
        executable.into_os_string(),
        HELPER.into(),
        "--exact".into(),
        "--ignored".into(),
        "--nocapture".into(),
        "--".into(),
        MARKER.into(),
        "socket-inherited".into(),
        endpoint.into(),
        completed.as_os_str().into(),
        forbidden.as_os_str().into(),
    ];
    let mut command = Vec::new();
    for argument in &arguments {
        if !command.is_empty() {
            command.push(32);
        }
        command.extend(quoted_argument(argument));
    }
    command.push(0);
    let mut environment = std::collections::BTreeMap::<&str, OsString>::new();
    for name in ["PATH", "SystemRoot"] {
        if let Some(value) = std::env::var_os(name) {
            environment.insert(name, value);
        }
    }
    environment.insert("HOME", home.as_os_str().into());
    environment.insert("USERPROFILE", home.as_os_str().into());
    environment.insert(
        "CRAFTSMAN_CONFIG_DIR",
        home.join(".craftsman").into_os_string(),
    );
    environment.insert(INHERITED_SOCKET, socket.to_string().into());
    let mut block = Vec::new();
    for (key, value) in environment {
        block.extend(key.encode_utf16());
        block.push(61);
        let words: Vec<u16> = value.encode_wide().collect();
        if words.contains(&0) {
            return Err("native environment contains a NUL".to_owned());
        }
        block.extend(words);
        block.push(0);
    }
    block.push(0);
    let directory: Vec<u16> = workspace.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut startup = StartupInfoEx {
        startup: StartupInfo {
            size: std::mem::size_of::<StartupInfoEx>() as u32,
            reserved: null_mut(),
            desktop: null_mut(),
            title: null_mut(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            columns: 0,
            rows: 0,
            fill: 0,
            flags: 0x100,
            show: 0,
            reserved_size: 0,
            reserved_bytes: null_mut(),
            input: nul.as_raw_handle(),
            output: nul.as_raw_handle(),
            error: stderr.as_raw_handle(),
        },
        attributes: inheritance.storage.0.as_mut_ptr().cast(),
    };
    let mut information = ProcessInformation {
        process: null_mut(),
        thread: null_mut(),
        process_id: 0,
        thread_id: 0,
    };
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            null_mut(),
            null_mut(),
            1,
            0x0008_0400,
            block.as_mut_ptr().cast(),
            directory.as_ptr(),
            &mut startup.startup,
            &mut information,
        )
    };
    if created == 0 {
        return Err(format!("native helper creation failed {}", unsafe {
            GetLastError()
        }));
    }
    // Only the newly returned process/thread HANDLEs drive completion. PIDs
    // are neither read nor logged; argv/environment/list/stdio stayed alive.
    let mut child = NativeChild {
        process: owned(information.process),
        _thread: None,
        reaped: false,
        cleanup_attempted: false,
    };
    if information.thread.is_null() || information.thread as usize == usize::MAX {
        return Err("native created thread handle is invalid".to_owned());
    }
    child._thread = Some(owned(information.thread));
    Ok(child)
}

struct QueryDiagnosticsOnFailure;

fn emit_query_diagnostics() {
    let queries = super::windows_probe::take_query_diagnostics();
    eprintln!(
        "native-output-query-diagnostics: {}",
        json!({
            "available":queries.is_some(),
            "queries":queries,
            "duplicate_attempts":super::windows_probe::duplicate_attempts(),
        })
    );
}

impl Drop for QueryDiagnosticsOnFailure {
    fn drop(&mut self) {
        if std::thread::panicking() {
            emit_query_diagnostics();
        }
    }
}

#[tokio::test]
#[ignore = "marked Windows output fixture; unmarked/unknown modes return before config/path/native IO"]
async fn windows_output_fixture() {
    let args: Vec<String> = std::env::args().collect();
    let markers: Vec<usize> = args
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (value == MARKER).then_some(index))
        .collect();
    let [marker] = markers.as_slice() else {
        return;
    };
    let [mode, endpoint, completed, forbidden] = &args[marker + 1..] else {
        return;
    };
    const MODES: &[&str] = &[
        "anonymous",
        "anonymous-closed",
        "named-server",
        "named-server-minimal",
        "named-client",
        "named-client-attributes",
        "named-server-disconnected",
        "named-client-closing",
        "after-first-write",
        "after-real-result",
        "overlapped-server",
        "overlapped-client",
        "overlapped-disk",
        "read-only",
        "null",
        "invalid",
        "type-error",
        "socket",
        "socket-sync",
        "socket-datagram",
        "socket-connected",
        "socket-inherited",
        "sync-disk",
        "unbuffered-disk",
        "nul-output",
        "character-read-only",
        "console-unicode",
    ];
    if !MODES.contains(&mode.as_str()) {
        return;
    }
    // Diagnostics exist only in this explicitly marked, known-mode child and
    // contain at most32 actual query records with fixed names/primitive fields.
    super::windows_probe::enable_query_diagnostics();
    let _diagnostics_on_failure = QueryDiagnosticsOnFailure;
    eprintln!("native-output-case: {mode}");
    // No path, config, environment or native handle inspection above this point.
    let mut socket = None;
    let mut socket_streams = None;
    let mut socket_type_before = None;
    let mut setup_file = None;
    let mut peer = None;
    let mut writer = None;
    let unsupported = matches!(
        mode.as_str(),
        "overlapped-server"
            | "overlapped-client"
            | "overlapped-disk"
            | "read-only"
            | "null"
            | "invalid"
            | "type-error"
            | "socket"
            | "socket-sync"
            | "socket-datagram"
            | "socket-connected"
            | "socket-inherited"
            | "unbuffered-disk"
            | "character-read-only"
    );
    match mode.as_str() {
        "null" => select(null_mut()),
        "invalid" => select(usize::MAX as Handle),
        "type-error" => {
            let event = owned(unsafe { CreateEventW(null_mut(), 1, 0, null()) });
            select(event.as_raw_handle());
            writer = Some(event);
        }
        "socket" | "socket-sync" | "socket-datagram" | "socket-connected" | "socket-inherited" => {
            // Aligned opaque WSADATA storage, larger than either native ABI.
            let mut data = [0usize; 128];
            assert_eq!(unsafe { WSAStartup(0x0202, data.as_mut_ptr().cast()) }, 0);
            assert_eq!(
                unsafe { std::ptr::read(data.as_ptr().cast::<u16>()) },
                0x0202
            );
            let (kind, protocol, flags) = if mode == "socket-datagram" {
                (2, 17, 1)
            } else if mode == "socket-sync" {
                (1, 6, 0)
            } else {
                (1, 6, 1)
            };
            let value = if mode == "socket-inherited" {
                // Only this known marked mode reads the private entry. The
                // parent created/inherited this real socket through an explicit
                // handle list, with initially safe NUL/file stdio for libtest.
                let value = std::env::var(INHERITED_SOCKET)
                    .expect("native inherited socket entry")
                    .parse::<usize>()
                    .expect("native inherited socket value");
                let mut inherit_flags = 0u32;
                assert_ne!(
                    unsafe { GetHandleInformation(value as Handle, &mut inherit_flags) },
                    0
                );
                eprintln!(
                    "native-output-inheritance: {}",
                    json!({"stage":"child_handle","inherit_bit":inherit_flags & 1 != 0})
                );
                value
            } else if mode == "socket-connected" {
                let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
                let selected =
                    std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
                let (accepted, _) = listener.accept().unwrap();
                let value = usize::try_from(selected.as_raw_socket())
                    .expect("std RawSocket fits native SOCKET");
                socket_streams = Some((selected, accepted));
                value
            } else {
                unsafe { WSASocketW(2, kind, protocol, null_mut(), 0, flags) }
            };
            assert_ne!(value, usize::MAX);
            // Do not prime inherited-socket lookup before the real factory's
            // first SO_TYPE query. Its actual type is checked after refusal.
            socket_type_before = if mode == "socket-inherited" {
                None
            } else {
                Some(usable_socket_type(value))
            };
            select(value as Handle);
            socket = Some(value);
        }
        "sync-disk" | "overlapped-disk" | "unbuffered-disk" => {
            let path = Path::new(completed);
            let name = wide(&path.to_string_lossy());
            let handle = owned(unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_WRITE,
                    3,
                    null_mut(),
                    2,
                    if mode == "overlapped-disk" {
                        OVERLAPPED
                    } else if mode == "unbuffered-disk" {
                        0x2000_0000
                    } else {
                        0
                    },
                    null_mut(),
                )
            });
            select(handle.as_raw_handle());
            writer = Some(handle);
            setup_file = Some(path.to_owned());
        }
        "nul-output" | "character-read-only" => {
            let name = wide("NUL");
            let handle = owned(unsafe {
                CreateFileW(
                    name.as_ptr(),
                    if mode == "nul-output" {
                        GENERIC_WRITE
                    } else {
                        GENERIC_READ
                    },
                    3,
                    null_mut(),
                    3,
                    0,
                    null_mut(),
                )
            });
            select(handle.as_raw_handle());
            writer = Some(handle);
        }
        "console-unicode" => {
            let (before, before_error) = console_attachment_facts();
            eprintln!(
                "native-console-attachment: {}",
                json!({"stage":"console_before","count":before,"last_error":before_error})
            );
            let allocated = before == 0;
            if allocated {
                let (result, error) = unsafe {
                    SetLastError(0);
                    let result = AllocConsole();
                    (result, GetLastError())
                };
                eprintln!(
                    "native-console-attachment: {}",
                    json!({"stage":"console_allocate","result":result,"last_error":error})
                );
                assert_ne!(result, 0, "actual console allocation error {error}");
            }
            let (attached, attached_error) = console_attachment_facts();
            eprintln!(
                "native-console-attachment: {}",
                json!({"stage":"console_after","count":attached,"last_error":attached_error})
            );
            assert!(
                attached > 0,
                "actual console attachment error {attached_error}"
            );
            // A separate inactive screen buffer avoids writing into a parent's
            // visible console while exercising the actual Unicode writer.
            let screen = owned(unsafe {
                CreateConsoleScreenBuffer(
                    GENERIC_READ | GENERIC_WRITE,
                    3,
                    null_mut(),
                    1,
                    null_mut(),
                )
            });
            let mut console_mode = 0u32;
            let (mode_result, mode_error) = unsafe {
                SetLastError(0);
                let result = GetConsoleMode(screen.as_raw_handle(), &mut console_mode);
                (result, GetLastError())
            };
            eprintln!(
                "native-console-attachment: {}",
                json!({"stage":"console_mode","result":mode_result,"last_error":mode_error,"mode":(mode_result != 0).then_some(console_mode)})
            );
            assert_ne!(mode_result, 0, "actual console mode error {mode_error}");
            select(screen.as_raw_handle());
            let output = super::stdout().unwrap();
            let text = "éλ";
            output.write(text.as_bytes());
            assert_eq!(output.failure_kind(), None);
            let expected: Vec<u16> = text.encode_utf16().collect();
            let mut observed = vec![0; expected.len()];
            let mut count = 0;
            assert_ne!(
                unsafe {
                    ReadConsoleOutputCharacterW(
                        screen.as_raw_handle(),
                        observed.as_mut_ptr(),
                        observed.len() as u32,
                        Coord { x: 0, y: 0 },
                        &mut count,
                    )
                },
                0
            );
            assert_eq!(count as usize, expected.len());
            assert_eq!(observed, expected);
            drop(output);
            drop(screen);
            if allocated {
                assert_ne!(unsafe { FreeConsole() }, 0);
            }
            emit_query_diagnostics();
            std::process::exit(0);
        }
        value if value.starts_with("named") || value.starts_with("overlapped") => {
            let server_output = matches!(
                value,
                "named-server"
                    | "named-server-minimal"
                    | "named-server-disconnected"
                    | "overlapped-server"
            );
            let (read, write) = named(
                server_output,
                value == "overlapped-server",
                value == "overlapped-client",
                value == "named-client-attributes",
            );
            let write = if matches!(value, "named-server-minimal" | "named-client") {
                narrow_write(&write)
            } else {
                write
            };
            if value == "named-server-disconnected" {
                assert_ne!(unsafe { DisconnectNamedPipe(write.as_raw_handle()) }, 0);
            }
            select(write.as_raw_handle());
            peer = Some(read);
            writer = Some(write);
            if value == "named-client-closing" {
                drop(peer.take());
            }
        }
        "read-only" => {
            let (read, write) = anonymous();
            select(read.as_raw_handle());
            writer = Some(read);
            peer = Some(write);
        }
        _ => {
            let (read, write) = anonymous();
            select(write.as_raw_handle());
            peer = Some(read);
            writer = Some(write);
            if mode == "anonymous-closed" {
                drop(peer.take());
            }
        }
    }
    let raw = writer.as_ref().map_or_else(
        || socket.map_or(null_mut(), |s| s as Handle),
        |w| w.as_raw_handle(),
    );
    let facts = if raw.is_null() || raw as usize == usize::MAX {
        json!({"invalid_or_null":true})
    } else {
        raw_facts(raw)
    };
    eprintln!("native-output-facts: {facts}");
    if mode == "named-client" || mode == "named-server-minimal" {
        let access = facts["access"]
            .as_u64()
            .expect("actual synchronous pure-write eligibility");
        assert_eq!(
            access & READ_ATTRIBUTES as u64,
            0,
            "fixture must not add READ_ATTRIBUTES"
        );
    }
    if unsupported {
        if mode.starts_with("overlapped") {
            assert_eq!(
                facts["mode"]
                    .as_u64()
                    .expect("actual overlapped mode query")
                    & 0x30,
                0
            );
        }
        assert!(
            super::stdout().is_err(),
            "unsupported native handle was admitted"
        );
        if matches!(
            mode.as_str(),
            "socket" | "socket-sync" | "socket-datagram" | "socket-connected" | "socket-inherited"
        ) {
            assert_eq!(
                super::windows_probe::duplicate_attempts(),
                0,
                "unsupported socket must be rejected before DuplicateHandle"
            );
        }
        let activity = super::windows_probe::activity_counts();
        assert_eq!(
            activity.1, 0,
            "unsupported capture must not write through File"
        );
        if mode.starts_with("overlapped")
            || matches!(
                mode.as_str(),
                "read-only"
                    | "null"
                    | "invalid"
                    | "type-error"
                    | "unbuffered-disk"
                    | "character-read-only"
            )
        {
            assert_eq!(
                activity.0, 0,
                "mode/type/access refusal must precede driver-facing queries"
            );
        }
        if let Some(value) = socket {
            // The factory's checked cleanup must retain the caller's separate
            // startup reference and the selected real socket's usability.
            let socket_type_after = usable_socket_type(value);
            if let Some(before) = socket_type_before {
                assert_eq!(socket_type_after, before);
            } else {
                assert_eq!(socket_type_after, 1);
            }
            eprintln!(
                "native-output-caller-socket: {}",
                json!({"stage":"caller_after","socket_type":socket_type_after})
            );
            let after = raw_facts(value as Handle);
            for field in ["access", "mode"] {
                assert_eq!(after[field], facts[field], "caller grant/mode changed");
            }
            if let Some((selected, accepted)) = socket_streams.take() {
                let expected = u64::try_from(value).expect("native SOCKET fits std RawSocket");
                assert!(
                    selected.into_raw_socket() == expected,
                    "selected socket identity changed"
                );
                drop(accepted);
            }
            assert_eq!(unsafe { closesocket(value) }, 0);
            assert_eq!(unsafe { WSACleanup() }, 0);
        }
        if let Some(path) = setup_file {
            assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
        }
        assert!(!Path::new(forbidden).exists());
        emit_query_diagnostics();
        std::process::exit(0);
    }
    if mode == "sync-disk" {
        let output = super::stdout().unwrap();
        output.write("匠 output\n".as_bytes());
        assert_eq!(output.failure_kind(), None);
        assert_eq!(std::fs::read(completed).unwrap(), "匠 output\n".as_bytes());
        emit_query_diagnostics();
        std::process::exit(0);
    }
    if mode == "nul-output" {
        super::windows_probe::null_device(raw).expect("actual native FILE_DEVICE_NULL identity");
        let before = super::windows_probe::bootstrap(raw).unwrap();
        let output = super::stdout().expect("canonical synchronous native NUL output must work");
        output.write("discarded native 匠 output\n".as_bytes());
        assert_eq!(output.failure_kind(), None);
        let captured_writes = super::windows_probe::activity_counts().1;
        assert!(
            captured_writes > 0,
            "selected captured NUL writer must perform real File IO"
        );
        let cfg: Config = serde_json::from_value(json!({
            "model":"mock-model", "base_url":endpoint, "api_key":"synthetic-nul-owner-key"
        }))
        .unwrap();
        let approval = Arc::new(Mutex::new(ApprovalState {
            auto_approve: true,
            ..ApprovalState::default()
        }));
        let session = build_session_ext(
            &cfg,
            approval,
            crate::prompt::NONINTERACTIVE_INSTRUCTIONS,
            true,
            false,
            None,
            SessionExtras {
                token_sink: Some(output.sink()),
                output_closed: Some(output.closed_flag()),
                output_guard: Some(output.guard()),
                ..SessionExtras::default()
            },
        )
        .await
        .unwrap();
        let result = session
            .agent
            .run_turn("complete the actual NUL output task")
            .await;
        session.lsp.shutdown().await;
        assert!(matches!(result, Ok(craftsman_harness::TurnResult::Done(_))));
        output.write(b"\n");
        assert_eq!(output.failure_kind(), None);
        assert_eq!(
            std::fs::read(completed).unwrap(),
            b"real completed Windows effect"
        );
        assert!(!Path::new(forbidden).exists());
        assert_eq!(*session.usage.lock().unwrap(), (16, 4));
        assert!(session.agent.state.lock().unwrap().messages.iter().any(|m|
            matches!(m,craftsman_harness::Msg::Tool { content,is_error:false,.. } if content.contains("Updated file"))));
        assert!(super::windows_probe::activity_counts().1 > captured_writes);
        let after = super::windows_probe::bootstrap(raw).unwrap();
        assert_eq!((before.access, before.mode), (after.access, after.mode));
        emit_query_diagnostics();
        std::process::exit(0);
    }
    if matches!(
        mode.as_str(),
        "anonymous"
            | "named-server"
            | "named-server-minimal"
            | "named-client"
            | "named-client-attributes"
    ) {
        assert_eq!(
            super::windows_probe::pipe_state(raw).unwrap(),
            3,
            "actual completed local NPFS query required"
        );
    }
    let output =
        super::stdout().expect("canonical same-access native output must remain supported");
    assert_eq!(
        output.failure_kind(),
        None,
        "capture must defer real closed-peer observation"
    );
    let peer = Arc::new(Mutex::new(peer));
    let after_result = mode == "after-real-result";
    let after_write = mode == "after-first-write";
    if !matches!(
        mode.as_str(),
        "anonymous-closed"
            | "named-server-disconnected"
            | "named-client-closing"
            | "after-first-write"
            | "after-real-result"
    ) {
        drop(peer.lock().unwrap().take());
    }
    let round = Arc::new(AtomicUsize::new(0));
    let round_sink = {
        let round = round.clone();
        let peer = peer.clone();
        Arc::new(move || {
            if round.fetch_add(1, Ordering::SeqCst) == 1 && after_result {
                drop(peer.lock().unwrap().take());
            }
        }) as crate::agent::RoundSink
    };
    let sink = {
        let output = output.clone();
        let peer = peer.clone();
        Arc::new(move |text: &str| {
            output.write(text.as_bytes());
            if after_write {
                if let Some(read) = peer.lock().unwrap().take() {
                    // CreatePipe's test read end was opened synchronously; only
                    // this synchronous fixture handle becomes a std File reader.
                    let mut reader = File::from(read);
                    let mut actual = vec![0; text.len()];
                    reader.read_exact(&mut actual).unwrap();
                    assert_eq!(actual, text.as_bytes());
                    drop(reader);
                }
            }
        }) as crate::agent::TokenSink
    };
    let cfg: Config = serde_json::from_value(
        json!({"model":"mock-model","base_url":endpoint,"api_key":"synthetic-windows-owner-key"}),
    )
    .unwrap();
    let approval = Arc::new(Mutex::new(ApprovalState {
        auto_approve: true,
        ..ApprovalState::default()
    }));
    let session = build_session_ext(
        &cfg,
        approval,
        crate::prompt::NONINTERACTIVE_INSTRUCTIONS,
        true,
        false,
        None,
        SessionExtras {
            token_sink: Some(sink),
            output_closed: Some(output.closed_flag()),
            output_guard: Some(output.guard()),
            round_sink: Some(round_sink),
            ..SessionExtras::default()
        },
    )
    .await
    .unwrap();
    let result = session
        .agent
        .run_turn("actual Windows output closure control")
        .await;
    session.lsp.shutdown().await;
    assert!(matches!(result, Err(AgentError::State(_))));
    assert_eq!(output.failure_kind(), Some(ErrorKind::BrokenPipe));
    assert!(!Path::new(forbidden).exists());
    let requests = if after_result { 2 } else { 1 };
    assert_eq!(round.load(Ordering::SeqCst), requests);
    assert_eq!(
        *session.usage.lock().unwrap(),
        (requests as u64 * 8, requests as u64 * 2)
    );
    if after_result {
        assert_eq!(
            std::fs::read(completed).unwrap(),
            b"real completed Windows effect"
        );
        assert!(session.agent.state.lock().unwrap().messages.iter().any(|m|matches!(m,craftsman_harness::Msg::Tool{content,is_error:false,..} if content.contains("Updated file"))));
    }
    emit_query_diagnostics();
    std::process::exit(0);
}

fn sse(events: &[Value]) -> ResponseTemplate {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}
fn tool(path: &str, id: &str, content: &str) -> Value {
    json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"function":{"name":"write_file", "arguments":json!({"file_path":path,"content":content}).to_string()}}]}}]})
}
struct Script {
    after_result: bool,
    nul_output: bool,
    completed: String,
    forbidden: String,
}
impl Respond for Script {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        let completed = messages
            .iter()
            .any(|m| m["role"] == "tool" && m["tool_call_id"] == "completed");
        let followed = messages
            .iter()
            .any(|m| m["role"] == "tool" && m["tool_call_id"] == "forbidden");
        let mut events = vec![];
        if self.nul_output && completed {
            events.push(json!({"choices":[{"delta":{"content":"completed Windows NUL output"},"finish_reason":"stop"}]}));
        } else if followed {
            events.push(json!({"choices":[{"delta":{"content":"unexpected Windows follow-up"},"finish_reason":"stop"}]}));
        } else {
            if !self.after_result || completed {
                events.push(json!({"choices":[{"delta":{"content":"actual Windows content"}}]}));
            }
            events.push(if (self.after_result || self.nul_output) && !completed {
                tool(
                    &self.completed,
                    "completed",
                    "real completed Windows effect",
                )
            } else {
                tool(&self.forbidden, "forbidden", "must not run")
            });
            events.push(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}));
        }
        events.push(json!({"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2}}));
        sse(&events)
    }
}

async fn case(mode: &str, requests: usize) {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let completed = workspace.path().join("completed.txt");
    let forbidden = workspace.path().join("must-not-be-dispatched.txt");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(Script {
            after_result: mode == "after-real-result",
            nul_output: mode == "nul-output",
            completed: completed.to_string_lossy().into_owned(),
            forbidden: forbidden.to_string_lossy().into_owned(),
        })
        .mount(&server)
        .await;
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            HELPER,
            "--exact",
            "--ignored",
            "--nocapture",
            "--",
            MARKER,
            mode,
        ])
        .arg(server.uri())
        .arg(&completed)
        .arg(&forbidden)
        .env_clear()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CRAFTSMAN_CONFIG_DIR", home.path().join(".craftsman"))
        .current_dir(workspace.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in ["PATH", "SystemRoot"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let result = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!String::from_utf8_lossy(&result.stderr).contains("panicked"));
    for line in String::from_utf8_lossy(&result.stderr)
        .lines()
        .filter(|line| {
            line.starts_with("native-output-") || line.starts_with("native-console-attachment:")
        })
    {
        eprintln!("{line}");
    }
    let observed = server.received_requests().await.unwrap();
    assert_eq!(observed.len(), requests);
    assert!(!forbidden.exists());
    if requests == 0 {
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
    if mode == "after-real-result" || mode == "nul-output" {
        assert_eq!(
            std::fs::read(completed).unwrap(),
            b"real completed Windows effect"
        );
        let second: Value = serde_json::from_slice(&observed[1].body).unwrap();
        assert!(second["messages"].as_array().unwrap().iter().any(|m| {
            m["role"] == "tool"
                && m["content"]
                    .as_str()
                    .is_some_and(|v| v.contains("Updated file"))
        }));
    }
}

#[tokio::test]
async fn actual_windows_anonymous_write_only_pipe_keeps_one_request_quiet_closure() {
    case("anonymous", 1).await;
}
#[tokio::test]
async fn actual_windows_closed_anonymous_pipe_defers_failure_until_initial_response() {
    case("anonymous-closed", 1).await;
}
#[tokio::test]
async fn actual_windows_named_server_write_only_pipe_queries_and_stops_dispatch() {
    case("named-server", 1).await;
}
#[tokio::test]
async fn actual_windows_minimal_named_server_without_read_attributes_queries_and_stops() {
    case("named-server-minimal", 1).await;
}
#[tokio::test]
async fn actual_windows_pure_write_named_client_without_read_attributes_queries_and_stops() {
    case("named-client", 1).await;
}
#[tokio::test]
async fn actual_windows_named_client_with_read_attributes_queries_and_stops() {
    case("named-client-attributes", 1).await;
}
#[tokio::test]
async fn actual_windows_disconnected_named_server_preserves_initial_request() {
    case("named-server-disconnected", 1).await;
}
#[tokio::test]
async fn actual_windows_closing_named_client_preserves_initial_request() {
    case("named-client-closing", 1).await;
}
#[tokio::test]
async fn actual_windows_close_after_read_content_blocks_same_response_tool() {
    case("after-first-write", 1).await;
}
#[tokio::test]
async fn actual_windows_prior_completed_effect_result_and_usage_survive_output_close() {
    case("after-real-result", 2).await;
}
#[tokio::test]
async fn actual_windows_overlapped_handles_refuse_before_model_or_std_file_io() {
    for mode in ["overlapped-server", "overlapped-client", "overlapped-disk"] {
        case(mode, 0).await;
    }
}
#[tokio::test]
async fn actual_windows_null_invalid_type_error_and_socket_refuse_without_work() {
    for mode in [
        "null",
        "invalid",
        "type-error",
        "socket",
        "socket-sync",
        "socket-datagram",
        "read-only",
        "character-read-only",
    ] {
        case(mode, 0).await;
    }
}

#[tokio::test]
async fn actual_windows_connected_socket_refuses_before_duplicate_and_keeps_caller_reference() {
    case("socket-connected", 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_windows_inherited_socket_refuses_before_duplicate_and_keeps_both_references() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let completed = workspace.path().join("completed.txt");
    let forbidden = workspace.path().join("must-not-be-dispatched.txt");
    let stderr_path = workspace.path().join("owned-inherited-socket.stderr");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(Script {
            after_result: false,
            nul_output: false,
            completed: completed.to_string_lossy().into_owned(),
            forbidden: forbidden.to_string_lossy().into_owned(),
        })
        .mount(&server)
        .await;
    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
        let mut caller = CallerSocket::new();
        let before = usable_socket_type(caller.socket);
        let child = launch_inherited_socket(
            caller.socket,
            home.path(),
            workspace.path(),
            &server.uri(),
            &completed,
            &forbidden,
            &stderr_path,
        )?;
        // The synchronous owner has only bounded10s wait/5s reap calls and
        // returns before any result/read. A second runtime worker keeps the
        // real loopback request witness responsive; no owner task is abandoned.
        let code = child.wait()?;
        let mut stderr = Vec::new();
        File::open(&stderr_path)
            .map_err(|_| "native stderr open failed".to_owned())?
            .take(65537)
            .read_to_end(&mut stderr)
            .map_err(|_| "native stderr read failed".to_owned())?;
        if stderr.len() > 65536 {
            return Err("native stderr exceeded its bound".to_owned());
        }
        if code != 0 {
            return Err(format!(
                "native child exit {code}: {}",
                String::from_utf8_lossy(&stderr)
            ));
        }
        assert!(!String::from_utf8_lossy(&stderr).contains("panicked"));
        for line in String::from_utf8_lossy(&stderr)
            .lines()
            .filter(|line| line.starts_with("native-output-"))
        {
            eprintln!("{line}");
        }
        assert_eq!(usable_socket_type(caller.socket), before);
        caller.close()?;
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(!forbidden.exists());
        assert!(!completed.exists());
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
        Ok::<(), String>(())
    })
    .await;
    assert!(
        started.elapsed() <= Duration::from_secs(20),
        "native inherited-socket deadline exceeded"
    );
    outcome.unwrap().unwrap();
}
#[tokio::test]
async fn actual_windows_synchronous_disk_keeps_utf8_output() {
    case("sync-disk", 0).await;
}

#[tokio::test]
async fn actual_windows_synchronous_nul_discards_captured_output_and_keeps_tool_result_usage() {
    case("nul-output", 2).await;
}

#[tokio::test]
async fn actual_windows_unbuffered_disk_refuses_before_driver_query_or_write() {
    case("unbuffered-disk", 0).await;
}

#[tokio::test]
async fn actual_windows_console_keeps_standard_unicode_output() {
    case("console-unicode", 0).await;
}
#[tokio::test]
async fn actual_windows_unmarked_and_unknown_fixture_modes_leave_storage_untouched() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    for args in [
        vec![],
        vec![MARKER, "unknown-mode", "unused", "unused", "unused"],
    ] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([HELPER, "--exact", "--ignored", "--nocapture", "--"])
            .args(args)
            .env_clear()
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("CRAFTSMAN_CONFIG_DIR", home.path().join(".craftsman"))
            .current_dir(workspace.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for name in ["PATH", "SystemRoot"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let result = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 0);
    }
}
