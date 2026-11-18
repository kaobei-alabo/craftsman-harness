//! Read-only connection state of the captured stdout pipe; no reads or modes.

use super::{ErrorKind, Probe};
use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::AsRawHandle;

#[repr(C)]
struct IoStatusBlock {
    status: usize,
    information: usize,
}

// Existing native FILE_PIPE_LOCAL_INFORMATION ABI, not a stored/wire record.
#[repr(C)]
struct PipeLocalInformation {
    _pipe_type: u32,
    _configuration: u32,
    _maximum_instances: u32,
    _current_instances: u32,
    _inbound_quota: u32,
    _read_available: u32,
    _outbound_quota: u32,
    _write_available: u32,
    state: u32,
    _end: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileType(file: *mut c_void) -> u32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationFile(
        file: *mut c_void,
        status: *mut IoStatusBlock,
        information: *mut c_void,
        length: u32,
        class: u32,
    ) -> i32;
}

pub(super) fn probe(file: &File) -> Option<Probe> {
    // SAFETY: the captured File keeps this HANDLE live; GetFileType reads only
    // its type. Console/regular-file output keeps its existing write checks.
    if unsafe { GetFileType(file.as_raw_handle()) } != 3 {
        return None;
    }
    let handle = file.as_raw_handle() as usize;
    Some(Box::new(move || {
        let mut status = IoStatusBlock {
            status: 0,
            information: 0,
        };
        let mut information = PipeLocalInformation {
            _pipe_type: 0,
            _configuration: 0,
            _maximum_instances: 0,
            _current_instances: 0,
            _inbound_quota: 0,
            _read_available: 0,
            _outbound_quota: 0,
            _write_available: 0,
            state: 0,
            _end: 0,
        };
        // SAFETY: Output holds its writer lock and owns the captured File.
        // Class 24 writes only this C-layout pipe metadata and IO_STATUS_BLOCK;
        // it neither consumes data nor starts an OVERLAPPED IO or mode change.
        let result = unsafe {
            NtQueryInformationFile(
                handle as *mut c_void,
                &mut status,
                (&mut information as *mut PipeLocalInformation).cast(),
                std::mem::size_of::<PipeLocalInformation>() as u32,
                24,
            )
        };
        if matches!(result as u32, 0xc000_00b0 | 0xc000_00b1) {
            return Err(std::io::Error::from(ErrorKind::BrokenPipe));
        }
        if result != 0
            || status.status as i32 != result
            || status.information != std::mem::size_of::<PipeLocalInformation>()
        {
            return Err(std::io::Error::other("one-shot output state unavailable"));
        }
        match information.state {
            3 => Ok(()), // FILE_PIPE_CONNECTED_STATE
            1 | 4 => Err(std::io::Error::from(ErrorKind::BrokenPipe)),
            _ => Err(std::io::Error::other("one-shot output state unavailable")),
        }
    }))
}
