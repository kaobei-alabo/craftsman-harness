//! The retained directory of a file fixture on a target that is not unix.
//!
//! On unix a fixture opens its directory with `O_DIRECTORY | O_NOFOLLOW` and
//! compares device and inode. On Windows the same two questions are asked of
//! handles, by the rule the vault uses for its own directory
//! (`privacy_vault/storage.rs`, `identity_windows`): a directory is opened
//! for reading with backup semantics, a link in its place is opened itself
//! and not followed, the handle says whether it is a directory and not a
//! link, and the volume serial number with the file index identifies it.
//! Nothing here opens for writing, and no path stands in for a held handle.
//!
//! Any other target has no identity for a directory, so no named parent is
//! the held one there.

use std::fs::File;
use std::path::Path;

/// The volume serial number and the file index of an open directory, or
/// nothing when the handle is not a directory or is a link.
#[cfg(windows)]
pub(crate) fn directory_identity(directory: &File) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(directory.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return None;
    }
    Some((
        info.dwVolumeSerialNumber as u64,
        ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    ))
}

/// Whether `parent` names the directory that `held` is open on. Both are
/// asked by handle: the named parent is opened itself, for reading, and is
/// refused when it is a link or not a directory.
#[cfg(windows)]
pub(crate) fn names_held_directory(held: &File, parent: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };
    let Ok(named) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(parent)
    else {
        return false;
    };
    match (directory_identity(held), directory_identity(&named)) {
        (Some(held), Some(named)) => held == named,
        _ => false,
    }
}

#[cfg(not(windows))]
pub(crate) fn names_held_directory(_: &File, _: &Path) -> bool {
    false
}
