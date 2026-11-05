use super::{Error, Read};
use std::fs::File;
use std::io::Read as _;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::FileTypeExt;

pub(super) struct Native {
    file: File,
    original: i32,
    expected: i32,
    restored: bool,
}
fn flags(file: &File) -> Result<i32, Error> {
    // SAFETY: file owns the live descriptor for this entire call.
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if result < 0 {
        Err(Error::Io)
    } else {
        Ok(result)
    }
}
fn set_flags(file: &File, flags: i32) -> Result<(), Error> {
    // SAFETY: file owns the descriptor; F_SETFL takes this integer argument.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags) } < 0 {
        Err(Error::Io)
    } else {
        Ok(())
    }
}
impl Native {
    pub(super) fn stdin() -> Result<Self, Error> {
        // SAFETY: fcntl validates fd0 itself and atomically creates a new owned
        // descriptor. A closed/replaced fd0 is not borrowed into a Rust handle.
        let fd = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return Err(Error::Io);
        }
        // SAFETY: successful F_DUPFD_CLOEXEC returned this new owned descriptor.
        Self::new(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
    pub(super) fn new(file: File) -> Result<Self, Error> {
        if !file
            .metadata()
            .map_err(|_| Error::Io)?
            .file_type()
            .is_fifo()
        {
            return Err(Error::Unsupported);
        }
        let original = flags(&file)?;
        if original & libc::O_ACCMODE == libc::O_WRONLY {
            return Err(Error::Unsupported);
        }
        let native = Self {
            file,
            original,
            expected: original | libc::O_NONBLOCK,
            restored: false,
        };
        if native.original != native.expected {
            set_flags(&native.file, native.expected)?;
        }
        native.check()?;
        Ok(native)
    }
    fn check(&self) -> Result<(), Error> {
        if flags(&self.file)? == self.expected {
            Ok(())
        } else {
            Err(Error::Changed)
        }
    }
    pub(super) fn read(&mut self, buffer: &mut [u8]) -> Result<Read, Error> {
        self.check()?;
        let result = self.file.read(buffer);
        self.check()?;
        match result {
            Ok(0) => Ok(Read::End),
            Ok(n) => Ok(Read::Bytes(n)),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(Read::Pending)
            }
            Err(_) => Err(Error::Io),
        }
    }
    pub(super) fn restore(&mut self) -> Result<(), Error> {
        if self.restored {
            return Ok(());
        }
        // One owned restoration attempt; never overwrite a foreign mode change.
        self.restored = true;
        let current = flags(&self.file)?;
        if current == self.original {
            return Ok(());
        }
        if current != self.expected {
            return Err(Error::Changed);
        }
        set_flags(&self.file, self.original)?;
        if flags(&self.file)? == self.original {
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
    let mut pair = [-1; 2];
    // SAFETY: pair provides space for both newly created native descriptors.
    if unsafe { libc::pipe(pair.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful pipe() transferred ownership of both distinct fds.
    let reader = File::from(unsafe { OwnedFd::from_raw_fd(pair[0]) });
    let writer = File::from(unsafe { OwnedFd::from_raw_fd(pair[1]) });
    for file in [&reader, &writer] {
        // SAFETY: the two Files own live descriptors; FD_CLOEXEC is per-fd.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok((reader, writer))
}

#[cfg(test)]
pub(super) fn nonblocking_for_tests(file: &File) -> Result<bool, Error> {
    Ok(flags(file)? & libc::O_NONBLOCK != 0)
}
#[cfg(test)]
pub(super) fn set_nonblocking_for_tests(file: &File, enabled: bool) -> Result<(), Error> {
    let current = flags(file)?;
    set_flags(
        file,
        if enabled {
            current | libc::O_NONBLOCK
        } else {
            current & !libc::O_NONBLOCK
        },
    )
}
