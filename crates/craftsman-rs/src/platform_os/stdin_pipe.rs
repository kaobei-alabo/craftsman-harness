//! Private concrete byte-pipe ownership. No generic reader, threads or stdio output.
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix::Native;
#[cfg(windows)]
use windows::Native;

pub(crate) const STEP: usize = 16 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Unsupported,
    Busy,
    Io,
    Changed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Read {
    Bytes(usize),
    Pending,
    End,
}
static CAPTURED: AtomicBool = AtomicBool::new(false);
struct Lease;
impl Lease {
    fn acquire() -> Result<Self, Error> {
        CAPTURED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| Error::Busy)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        CAPTURED.store(false, Ordering::Release);
    }
}
/// Only explicit selection may duplicate process stdin. Mode is leased only on
/// that byte pipe; unsupported handles refuse without reading. Fields/constructors
/// do not provide a caller-created safe record or an arbitrary IO implementation.
pub(crate) struct Pipe {
    // Close/restore native ownership BEFORE releasing the single-capture lease.
    native: Native,
    _lease: Option<Lease>,
    #[cfg(test)]
    max_read: Option<usize>,
}
impl Pipe {
    pub(crate) fn stdin(_selection: &crate::exec::input::StdinSelection) -> Result<Self, Error> {
        let lease = Lease::acquire()?;
        let native = Native::stdin()?;
        Ok(Self {
            native,
            _lease: Some(lease),
            #[cfg(test)]
            max_read: None,
        })
    }
    pub(crate) fn read(&mut self, buffer: &mut [u8]) -> Result<Read, Error> {
        if buffer.is_empty() || buffer.len() > STEP {
            return Err(Error::Unsupported);
        }
        #[cfg(test)]
        if let Some(maximum) = self.max_read {
            let length = buffer.len().min(maximum);
            return self.native.read(&mut buffer[..length]);
        }
        self.native.read(buffer)
    }
    #[cfg(test)]
    pub(crate) fn tighten_read_size_for_tests(mut self, maximum: usize) -> Self {
        self.max_read = Some(maximum);
        self
    }
    pub(crate) fn finish(mut self) -> Result<(), Error> {
        self.native.restore()
    }
    #[cfg(test)]
    pub(crate) fn owned_for_tests(file: std::fs::File) -> Result<Self, Error> {
        Ok(Self {
            native: Native::new(file)?,
            _lease: None,
            max_read: None,
        })
    }
}
#[cfg(test)]
pub(crate) fn pair_for_tests() -> std::io::Result<(std::fs::File, std::fs::File)> {
    #[cfg(unix)]
    return unix::pair_for_tests();
    #[cfg(windows)]
    return windows::pair_for_tests();
}

#[cfg(test)]
pub(crate) fn nonblocking_for_tests(file: &std::fs::File) -> Result<bool, Error> {
    #[cfg(unix)]
    return unix::nonblocking_for_tests(file);
    #[cfg(windows)]
    return windows::nonblocking_for_tests(file);
}
#[cfg(test)]
pub(crate) fn set_nonblocking_for_tests(file: &std::fs::File, enabled: bool) -> Result<(), Error> {
    #[cfg(unix)]
    return unix::set_nonblocking_for_tests(file, enabled);
    #[cfg(windows)]
    return windows::set_nonblocking_for_tests(file, enabled);
}

#[cfg(test)]
pub(crate) fn read_only_pair_for_tests() -> std::io::Result<(std::fs::File, std::fs::File)> {
    #[cfg(unix)]
    return unix::pair_for_tests();
    #[cfg(windows)]
    return windows::read_only_pair_for_tests();
}
