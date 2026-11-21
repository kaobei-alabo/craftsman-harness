//! Read-only identity of the actual child, never a PID lookup or process policy.
use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

type Handle = *mut c_void;
const SAME_ACCESS: u32 = 2;
const INHERIT: u32 = 1;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const WAIT_FAILED: u32 = u32::MAX;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn GetLastError() -> u32;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn GetHandleInformation(handle: Handle, flags: *mut u32) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(handle: Handle, code: *mut u32) -> i32;
}

pub(super) struct Retained {
    handle: OwnedHandle,
    pub(super) observed_inherit_bit: bool,
}

impl Retained {
    pub(super) fn bind(child: &tokio::process::Child) -> io::Result<Self> {
        let selected = child
            .raw_handle()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let mut duplicate = std::ptr::null_mut();
        let process = unsafe { GetCurrentProcess() };
        // Retain the returned Child's exact process object. Same access, and
        // explicitly non-inheritable; neither permissions nor process flags change.
        if unsafe {
            DuplicateHandle(
                process,
                selected,
                process,
                &mut duplicate,
                0,
                0,
                SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        if duplicate.is_null() || duplicate as usize == usize::MAX {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(duplicate) };
        let mut flags = 0;
        if unsafe { GetHandleInformation(handle.as_raw_handle(), &mut flags) } == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        if flags & INHERIT != 0 {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        Ok(Self {
            handle,
            observed_inherit_bit: flags & INHERIT != 0,
        })
    }

    pub(super) fn observe(&self) -> io::Result<Option<u32>> {
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::from_raw_os_error(
                        unsafe { GetLastError() } as i32
                    ));
                }
                // A signalled process with exit259 is still a real exit259.
                // Never infer liveness from the STILL_ACTIVE numeric value.
                Ok(Some(code))
            }
            WAIT_TIMEOUT => Ok(None),
            WAIT_FAILED => Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            )),
            _ => Err(io::Error::from(io::ErrorKind::InvalidData)),
        }
    }
}
