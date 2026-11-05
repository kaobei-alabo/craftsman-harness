use super::{Error, Read};
use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr::{null, null_mut};

type Handle = *mut c_void;
const NOWAIT: u32 = 1;
const MESSAGE: u32 = 2;
const PIPE: u32 = 3;
const PIPE_TYPE_MESSAGE: u32 = 4;
#[cfg(test)]
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
const FILE_MODE_INFORMATION: u32 = 16;
const FILE_READ_DATA: u32 = 1;
const SYNCHRONOUS_IO: u32 = 0x10 | 0x20;
// IO_STATUS_BLOCK starts with an NTSTATUS/pointer union, followed by ULONG_PTR.
// Pointer-sized storage preserves its C layout on both supported pointer widths.
#[repr(C)]
struct IoStatusBlock {
    status: usize,
    information: usize,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryObject(
        object: Handle,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
    fn NtQueryInformationFile(
        file: Handle,
        status: *mut IoStatusBlock,
        information: *mut c_void,
        length: u32,
        class: u32,
    ) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
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
    fn GetFileType(file: Handle) -> u32;
    fn GetNamedPipeInfo(
        pipe: Handle,
        flags: *mut u32,
        output_size: *mut u32,
        input_size: *mut u32,
        instances: *mut u32,
    ) -> i32;
    fn PeekNamedPipe(
        pipe: Handle,
        buffer: *mut c_void,
        length: u32,
        copied: *mut u32,
        available: *mut u32,
        left_in_message: *mut u32,
    ) -> i32;
    fn ReadFile(
        file: Handle,
        buffer: *mut c_void,
        length: u32,
        read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn GetNamedPipeHandleStateW(
        pipe: Handle,
        state: *mut u32,
        instances: *mut u32,
        maximum: *mut u32,
        timeout: *mut u32,
        user: *mut u16,
        user_length: u32,
    ) -> i32;
    fn SetNamedPipeHandleState(
        pipe: Handle,
        mode: *const u32,
        maximum: *const u32,
        timeout: *const u32,
    ) -> i32;
    #[cfg(test)]
    fn CreatePipe(
        reader: *mut Handle,
        writer: *mut Handle,
        attributes: *mut c_void,
        size: u32,
    ) -> i32;
}
fn granted_access(file: &File) -> Result<u32, Error> {
    // PUBLIC_OBJECT_BASIC_INFORMATION is fourteen ULONGs: attributes, granted
    // access, handle/pointer counts, then ten reserved ULONGs. Extra alignment
    // also covers the private native trailing timestamp layout.
    #[repr(C, align(8))]
    struct Basic([u32; 14]);
    let mut basic = Basic([0; 14]);
    let mut returned = 0u32;
    // SAFETY: ObjectBasicInformation (0) writes into this correctly sized and
    // aligned owned buffer; file keeps the HANDLE live. This is object metadata,
    // not a pipe read. Read ACTUAL HANDLE grants, not the file object's old mask.
    let result = unsafe {
        NtQueryObject(
            file.as_raw_handle(),
            0,
            (&mut basic as *mut Basic).cast(),
            std::mem::size_of::<Basic>() as u32,
            &mut returned,
        )
    };
    if result != 0 || returned as usize != std::mem::size_of::<Basic>() {
        Err(Error::Unsupported)
    } else {
        Ok(basic.0[1])
    }
}
fn file_mode(file: &File) -> Result<u32, Error> {
    let mut status = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let mut information = 0u32;
    // SAFETY: FILE_MODE_INFORMATION is one ULONG; both C-layout outputs remain
    // live during the native metadata query. No data or OVERLAPPED read starts.
    let result = unsafe {
        NtQueryInformationFile(
            file.as_raw_handle(),
            &mut status,
            (&mut information as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
            FILE_MODE_INFORMATION,
        )
    };
    if result != 0
        || status.status as i32 != result
        || status.information != std::mem::size_of::<u32>()
    {
        Err(Error::Unsupported)
    } else {
        Ok(information)
    }
}
fn readable_synchronous_byte_pipe(file: &File) -> Result<u32, Error> {
    // Reject a write-only pipe without consuming bytes or changing its mode;
    // do not infer read access from generic metadata calls alone.
    let access = granted_access(file)?;
    if access & FILE_READ_DATA == 0 {
        return Err(Error::Unsupported);
    }
    let mut flags = 0;
    // SAFETY: file owns its live HANDLE; flags has DWORD storage. All optional
    // outputs are null; this metadata operation never consumes pipe bytes.
    if unsafe {
        GetNamedPipeInfo(
            file.as_raw_handle(),
            &mut flags,
            null_mut(),
            null_mut(),
            null_mut(),
        )
    } == 0
        || flags & PIPE_TYPE_MESSAGE != 0
    {
        return Err(Error::Unsupported);
    }
    // Refuse overlapped handles BEFORE lending stack storage to ReadFile.
    if !matches!(file_mode(file)? & SYNCHRONOUS_IO, 0x10 | 0x20) {
        Err(Error::Unsupported)
    } else {
        Ok(access)
    }
}
fn mode(file: &File) -> Result<u32, Error> {
    let mut state = 0;
    // SAFETY: file owns its live HANDLE; state has DWORD storage; all optional
    // outputs are null. This only reads native pipe mode, never source bytes.
    if unsafe {
        GetNamedPipeHandleStateW(
            file.as_raw_handle(),
            &mut state,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            0,
        )
    } == 0
    {
        Err(Error::Io)
    } else {
        Ok(state)
    }
}
fn set_mode(file: &File, state: u32) -> Result<(), Error> {
    // SAFETY: file owns a live pipe HANDLE; state is a valid byte/nowait mode;
    // optional network collection outputs/settings remain untouched.
    if unsafe { SetNamedPipeHandleState(file.as_raw_handle(), &state, null(), null()) } == 0 {
        Err(Error::Unsupported)
    } else {
        Ok(())
    }
}
pub(super) struct Native {
    file: File,
    original: u32,
    expected: u32,
    restored: bool,
    peek: bool,
}
impl Native {
    pub(super) fn stdin() -> Result<Self, Error> {
        // SAFETY: the Win32 calls validate the standard HANDLE and atomically
        // duplicate it. No borrowed/invalid HANDLE becomes a Rust File.
        let original = unsafe { GetStdHandle((-10i32) as u32) };
        if original.is_null() || original as isize == -1 {
            return Err(Error::Unsupported);
        }
        let mut duplicate = null_mut();
        // SAFETY: target supplies HANDLE storage; same-access, non-inheritable
        // duplication uses current-process pseudo handles, which are not closed.
        // Preserve EXACT original grants; no access upgrade or reopening.
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                original,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                2,
            )
        } == 0
        {
            return Err(Error::Io);
        }
        // SAFETY: DuplicateHandle returned this new, exclusively owned HANDLE.
        Self::new(unsafe { File::from_raw_handle(duplicate) })
    }
    pub(super) fn new(file: File) -> Result<Self, Error> {
        // SAFETY: file owns a live HANDLE; GetFileType is read-only metadata.
        if unsafe { GetFileType(file.as_raw_handle()) } != PIPE {
            return Err(Error::Unsupported);
        }
        let access = readable_synchronous_byte_pipe(&file)?;
        let original = mode(&file)?;
        if original & MESSAGE != 0 {
            return Err(Error::Unsupported);
        }
        // Select from ORIGINAL mode/ACTUAL grants, not a caught setter error.
        // Inherited read-only clients commonly lack FILE_WRITE_ATTRIBUTES.
        // They keep their mode and use bounded native availability polling.
        let expected = if original & NOWAIT != 0 || access & FILE_WRITE_ATTRIBUTES != 0 {
            original | NOWAIT
        } else {
            original
        };
        let native = Self {
            file,
            original,
            expected,
            restored: false,
            peek: expected & NOWAIT == 0,
        };
        if native.original != native.expected {
            set_mode(&native.file, native.expected)?;
        }
        native.check()?;
        Ok(native)
    }
    fn check(&self) -> Result<(), Error> {
        if mode(&self.file)? == self.expected {
            Ok(())
        } else {
            Err(Error::Changed)
        }
    }
    fn ready_length(&self, maximum: usize) -> Result<Option<usize>, Error> {
        let mut available = 0u32;
        // SAFETY: this owns a validated synchronous BYTE pipe. No output data
        // buffer is supplied: peek observes readiness only, never source bytes.
        let success = unsafe {
            PeekNamedPipe(
                self.file.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            )
        };
        let failure = (success == 0).then(std::io::Error::last_os_error);
        self.check()?;
        match failure {
            None if available == 0 => Ok(None),
            None => Ok(Some(maximum.min(available as usize))),
            Some(e) if e.raw_os_error() == Some(232) => Ok(None),
            // A terminal pipe condition authorizes a concrete READ probe, not
            // EOF. Preserve buffered bytes even if peek reports peer closure.
            Some(e) if matches!(e.raw_os_error(), Some(109 | 233)) => Ok(Some(maximum)),
            Some(_) => Err(Error::Io),
        }
    }
    pub(super) fn read(&mut self, buffer: &mut [u8]) -> Result<Read, Error> {
        self.check()?;
        let length = if self.peek {
            let Some(length) = self.ready_length(buffer.len())? else {
                return Ok(Read::Pending);
            };
            self.check()?;
            length
        } else {
            buffer.len()
        };
        let mut received = 0u32;
        // SAFETY: validated SYNCHRONOUS byte pipe, unchanged expected mode;
        // either NOWAIT or <=observed available bytes/a terminal-state probe.
        // Never issue a blind open-idle read. The <=16KiB outputs remain live
        // until return; no OVERLAPPED operation or reader thread starts.
        // Foreign concurrent readers/mode changes can defeat readiness; this
        // private lease is not global stdin exclusion/native interruptibility.
        let success = unsafe {
            ReadFile(
                self.file.as_raw_handle(),
                buffer.as_mut_ptr().cast(),
                length as u32,
                &mut received,
                null_mut(),
            )
        };
        // Capture GetLastError immediately: the following mode query can
        // change it. File::read erases BrokenPipe-kind errors into Ok(0), so it
        // cannot distinguish Windows ERROR_NO_DATA from real pipe EOF here.
        let failure = (success == 0).then(std::io::Error::last_os_error);
        self.check()?;
        match failure {
            // Win32 byte-pipe NOWAIT is synchronous polling, NOT overlapped IO.
            // An empty writer-open pipe (or zero-byte write) is not EOF.
            None if received == 0 => Ok(Read::Pending),
            None => Ok(Read::Bytes(received as usize)),
            Some(e) if e.raw_os_error() == Some(232) => Ok(Read::Pending),
            Some(e) if matches!(e.raw_os_error(), Some(109 | 233)) => Ok(Read::End),
            Some(_) => Err(Error::Io),
        }
    }
    pub(super) fn restore(&mut self) -> Result<(), Error> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        let current = mode(&self.file)?;
        if current == self.original {
            return Ok(());
        }
        if current != self.expected {
            return Err(Error::Changed);
        }
        set_mode(&self.file, self.original)?;
        if mode(&self.file)? == self.original {
            Ok(())
        } else {
            Err(Error::Changed)
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
#[cfg(test)]
pub(super) fn pair_for_tests() -> std::io::Result<(File, File)> {
    let mut reader = null_mut();
    let mut writer = null_mut();
    // SAFETY: both outputs provide HANDLE storage; null attributes make these
    // test-only handles non-inheritable; CreatePipe creates a native byte pipe.
    if unsafe { CreatePipe(&mut reader, &mut writer, null_mut(), 0) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful CreatePipe transferred both distinct owned handles.
    Ok(unsafe { (File::from_raw_handle(reader), File::from_raw_handle(writer)) })
}

#[cfg(test)]
pub(super) fn nonblocking_for_tests(file: &File) -> Result<bool, Error> {
    Ok(mode(file)? & NOWAIT != 0)
}
#[cfg(test)]
pub(super) fn set_nonblocking_for_tests(file: &File, enabled: bool) -> Result<(), Error> {
    let current = mode(file)?;
    set_mode(
        file,
        if enabled {
            current | NOWAIT
        } else {
            current & !NOWAIT
        },
    )
}

#[cfg(test)]
pub(super) fn read_only_pair_for_tests() -> std::io::Result<(File, File)> {
    let (reader, writer) = pair_for_tests()?;
    let mut duplicate = null_mut();
    // SAFETY: narrow this isolated fixture's reader to GENERIC_READ only.
    // No production upgrade/reopen/source injection; target provides storage.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            reader.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            GENERIC_READ,
            0,
            0,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    drop(reader);
    // SAFETY: successful DuplicateHandle transferred this new owned HANDLE.
    Ok((unsafe { File::from_raw_handle(duplicate) }, writer))
}
