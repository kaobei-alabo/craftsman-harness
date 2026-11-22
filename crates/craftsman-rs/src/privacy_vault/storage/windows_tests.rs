//! Windows-only native IO on default owned temporary paths and synthetic keys.
//! Raw diagnostics stay in this cfg(test) module, never production errors.

use super::*;
use crate::privacy_vault::DetectorSpan;
use std::ffi::{OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Prefix};
use tempfile::TempDir;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, GetFileInformationByHandle,
};

fn record_io<T>(operation: &str, path: &Path, result: io::Result<T>) -> io::Result<T> {
    let observed = match &result {
        Ok(_) => serde_json::json!({"returned": "Ok"}),
        Err(error) => serde_json::json!({
            "returned": "Err",
            "kind": format!("{:?}", error.kind()),
            "raw_os_error": error.raw_os_error(),
            "debug": format!("{error:?}"),
            "display": error.to_string(),
        }),
    };
    eprintln!(
        "PRIVATE_VAULT_WINDOWS_NATIVE_FACT={}",
        serde_json::json!({
            "operation": operation,
            "path_debug": format!("{path:?}"),
            "path_utf16": path.as_os_str().encode_wide().collect::<Vec<_>>(),
            "rust_path_is_absolute": path.is_absolute(),
            "rust_components": path.components().map(|c| format!("{c:?}")).collect::<Vec<_>>(),
            "observed": observed,
        })
    );
    result
}

fn native_io<T>(operation: &str, path: &Path, result: io::Result<T>) -> T {
    record_io(operation, path, result)
        .unwrap_or_else(|error| panic!("owned native {operation} failed: {error:?}; {error}"))
}

fn metadata_probe(operation: &str, path: &Path) -> io::Result<fs::Metadata> {
    let result = record_io(operation, path, fs::symlink_metadata(path));
    if let Ok(meta) = &result {
        eprintln!(
            "PRIVATE_VAULT_WINDOWS_NATIVE_METADATA={}",
            serde_json::json!({
                "operation": operation,
                "path_utf16": path.as_os_str().encode_wide().collect::<Vec<_>>(),
                "debug": format!("{meta:?}"),
                "file_attributes": meta.file_attributes(),
                "creation_time": meta.creation_time(),
                "last_access_time": meta.last_access_time(),
                "last_write_time": meta.last_write_time(),
                "file_size": meta.file_size(),
                "is_dir": meta.is_dir(),
                "is_file": meta.is_file(),
                "is_symlink": meta.file_type().is_symlink(),
            })
        );
    }
    result
}

fn handle_probe(path: &Path) -> BY_HANDLE_FILE_INFORMATION {
    let file = native_io(
        "OpenOptions(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)",
        path,
        OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path),
    );
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let returned = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    let result = if returned == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    native_io("GetFileInformationByHandle", path, result);
    eprintln!(
        "PRIVATE_VAULT_WINDOWS_NATIVE_HANDLE={}",
        serde_json::json!({
            "path_utf16": path.as_os_str().encode_wide().collect::<Vec<_>>(),
            "dwFileAttributes": info.dwFileAttributes,
            "ftCreationTime": [info.ftCreationTime.dwLowDateTime, info.ftCreationTime.dwHighDateTime],
            "ftLastAccessTime": [info.ftLastAccessTime.dwLowDateTime, info.ftLastAccessTime.dwHighDateTime],
            "ftLastWriteTime": [info.ftLastWriteTime.dwLowDateTime, info.ftLastWriteTime.dwHighDateTime],
            "dwVolumeSerialNumber": info.dwVolumeSerialNumber,
            "nFileSizeHigh": info.nFileSizeHigh,
            "nFileSizeLow": info.nFileSizeLow,
            "nNumberOfLinks": info.nNumberOfLinks,
            "nFileIndexHigh": info.nFileIndexHigh,
            "nFileIndexLow": info.nFileIndexLow,
        })
    );
    info
}

fn observed_identity(info: &BY_HANDLE_FILE_INFORMATION) -> NativeIdentity {
    NativeIdentity(
        info.dwVolumeSerialNumber as u64,
        ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    )
}

fn owned_roots() -> (TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    // Keep the two actual API results. No prefix stripping, replacement temp
    // layout, environment mutation or canonicalization of a selected input.
    let ordinary = temp.path().to_path_buf();
    assert!(ordinary.is_absolute());
    assert!(
        matches!(ordinary.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
    );
    let canonical = native_io(
        "canonicalize owned temp",
        &ordinary,
        fs::canonicalize(&ordinary),
    );
    assert!(canonical.is_absolute());
    assert!(
        matches!(canonical.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::VerbatimDisk(_)))
    );
    assert!(
        observed_identity(&handle_probe(&ordinary)) == observed_identity(&handle_probe(&canonical))
    );
    (temp, ordinary, canonical)
}

fn raw_child(root: &Path, name: &str) -> PathBuf {
    let mut raw = root.as_os_str().to_owned();
    raw.push("\\");
    raw.push(name);
    PathBuf::from(raw)
}

fn capture_owned(root: &Path) -> NativeLocation {
    let location = VaultLocation::under_host_config_root(root).unwrap();
    assert_eq!(location.root.as_os_str(), root.as_os_str());
    let mut budget = Budget::new(Limits::default(), None);
    let captured = NativeLocation::capture(location, &mut budget)
        .unwrap_or_else(|error| panic!("actual owned NativeLocation::capture failed: {error:?}"));
    assert!(captured.created);
    assert_eq!(captured.root.as_os_str(), root.as_os_str());
    assert_eq!(
        captured.path.as_os_str(),
        raw_child(root, VAULT_FILENAME).as_os_str()
    );
    let root_info = handle_probe(root);
    let file_info = handle_probe(&captured.path);
    assert_eq!(root_info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
    assert_eq!(file_info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
    assert_eq!(file_info.nNumberOfLinks, 1);
    assert!(captured.root_id == observed_identity(&root_info));
    assert!(captured.file_id == observed_identity(&file_info));
    captured.recheck(&mut budget).unwrap();
    captured
}

fn complete_ancestor_probe(root: &Path) {
    let boundary = root.ancestors().last().unwrap();
    let mut components = boundary.components();
    assert!(matches!(components.next(), Some(Component::Prefix(_))));
    assert_eq!(components.next(), Some(Component::RootDir));
    assert!(components.next().is_none());
    assert!(boundary.is_absolute());
    for ancestor in root.ancestors() {
        assert!(ancestor.is_absolute());
        let meta = metadata_probe("complete absolute ancestor", ancestor).unwrap();
        assert!(meta.is_dir());
        assert!(!meta.file_type().is_symlink());
    }
}

#[test]
fn canonical_owned_native_open_records_the_incomplete_verbatim_error_and_complete_roots() {
    let (_temp, _ordinary, root) = owned_roots();
    let Some(Component::Prefix(prefix)) = root.components().next() else {
        panic!("owned canonical path has no native prefix")
    };
    let incomplete = Path::new(prefix.as_os_str());
    assert!(incomplete.is_absolute());
    assert!(incomplete.components().nth(1).is_none());
    // This is the exact incomplete verbatim native name submitted by the old
    // first iteration, without inventing an error kind/code from failed CI.
    let actual = metadata_probe("old incomplete verbatim prefix", incomplete);
    assert!(
        actual.is_err(),
        "incomplete verbatim name unexpectedly resolved; retain the native facts above"
    );
    complete_ancestor_probe(&root);
    capture_owned(&root);
}

#[test]
fn ordinary_drive_absolute_owned_native_open_preserves_the_complete_original_path() {
    let (_temp, root, _canonical) = owned_roots();
    let Some(Component::Prefix(prefix)) = root.components().next() else {
        panic!("owned ordinary path has no native prefix")
    };
    // Do not query C:, which could refer to a per-drive current directory.
    assert!(!Path::new(prefix.as_os_str()).is_absolute());
    complete_ancestor_probe(&root);
    capture_owned(&root);
}

struct SyntheticWindowsKey;

impl HostKeySource for SyntheticWindowsKey {
    fn acquire_privacy_key(&self) -> Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([7; 32])))
    }
}

async fn vault_roundtrip(root: &Path) {
    let email = "windows.native.owner@synthetic.example";
    let mut prior = None;
    for _ in 0..2 {
        let vault = Vault::open(
            VaultLocation::under_host_config_root(root).unwrap(),
            &SyntheticWindowsKey,
            Limits::default(),
        )
        .await
        .unwrap();
        assert_eq!(vault.inner.native.root.as_os_str(), root.as_os_str());
        assert_eq!(
            vault.inner.native.path.as_os_str(),
            raw_child(root, VAULT_FILENAME).as_os_str()
        );
        let namespace = HostNamespace::supplied_by_host(
            "synthetic-windows-host",
            "synthetic-project",
            "synthetic-thread",
            "synthetic-profile",
            "synthetic-context",
        )
        .unwrap();
        let session = vault.session(namespace).unwrap();
        let input = session.capture_text(email).unwrap();
        let report = DetectorReport {
            input_sha256: input.input_sha256_for_host_detector().unwrap(),
            spans: vec![DetectorSpan {
                start: 0,
                end: email.len(),
                category: Category::Email,
            }],
        };
        let masked = session
            .mask(
                input,
                report,
                DeclaredIdentifiers::supplied_by_host(&[]).unwrap(),
            )
            .await
            .unwrap();
        let alias = masked.text_for_later_model_request().unwrap().to_owned();
        assert_ne!(alias, email);
        assert!(!alias.contains(email));
        let records = masked.resolution_records_for_host_fields().unwrap();
        assert_eq!(records.len().unwrap(), 1);
        let field = records.resolve_exact_display_field(&alias).unwrap();
        assert_eq!(field.with_host_display_text(str::to_owned).unwrap(), email);
        if let Some(expected) = &prior {
            assert_eq!(&alias, expected);
        }
        prior = Some(alias);
        session.close();
        vault.close();
    }
}

#[tokio::test]
async fn canonical_owned_vault_open_commit_and_reopen_use_the_supplied_verbatim_path() {
    let (_temp, _ordinary, root) = owned_roots();
    complete_ancestor_probe(&root);
    vault_roundtrip(&root).await;
}

#[tokio::test]
async fn ordinary_drive_absolute_owned_vault_open_commit_and_reopen_use_the_supplied_path() {
    let (_temp, root, _canonical) = owned_roots();
    complete_ancestor_probe(&root);
    vault_roundtrip(&root).await;
}

#[test]
fn incomplete_drive_and_root_relative_inputs_refuse_before_native_open_or_creation() {
    let (_temp, ordinary, canonical) = owned_roots();
    for root in [&ordinary, &canonical] {
        let Some(Component::Prefix(prefix)) = root.components().next() else {
            panic!("owned path has no native prefix")
        };
        let incomplete = Path::new(prefix.as_os_str());
        assert!(incomplete.components().nth(1).is_none());
        let location = VaultLocation::under_host_config_root(incomplete);
        if matches!(prefix.kind(), Prefix::Disk(_)) {
            assert!(!incomplete.is_absolute());
            assert_eq!(location.err(), Some(PrivacyError::Input));
        } else {
            // Pin the actual Rust behavior, then check the native boundary's
            // stricter refusal. No repair or native prefix IO is authorized.
            assert!(incomplete.is_absolute());
            let mut budget = Budget::new(Limits::default(), None);
            let result = NativeLocation::capture(location.unwrap(), &mut budget);
            assert_eq!(result.err(), Some(PrivacyError::Input));
            assert_eq!(budget.calls, 0);
        }
    }
    let Some(Component::Prefix(prefix)) = ordinary.components().next() else {
        panic!("owned ordinary path has no native prefix")
    };
    let mut drive_relative = prefix.as_os_str().to_owned();
    drive_relative.push("synthetic-relative-root");
    for path in [
        PathBuf::from(drive_relative),
        PathBuf::from(r"\synthetic-root-relative"),
        PathBuf::from("synthetic-relative-root"),
    ] {
        assert!(!path.is_absolute());
        assert_eq!(
            VaultLocation::under_host_config_root(&path).err(),
            Some(PrivacyError::Input)
        );
    }
    assert_eq!(fs::read_dir(&ordinary).unwrap().count(), 0);
}

#[test]
fn actual_nondirectory_ancestor_refuses_before_the_missing_leaf_and_vault_creation() {
    let (_temp, ordinary, canonical) = owned_roots();
    let blocker = raw_child(&ordinary, "owned-ancestor-file");
    native_io(
        "write owned ancestor file",
        &blocker,
        fs::write(&blocker, b"preserve owned ancestor"),
    );
    for root in [&ordinary, &canonical] {
        let selected = raw_child(&raw_child(root, "owned-ancestor-file"), "missing-leaf");
        let result = NativeLocation::capture(
            VaultLocation::under_host_config_root(&selected).unwrap(),
            &mut Budget::new(Limits::default(), None),
        );
        assert_eq!(result.err(), Some(PrivacyError::Integrity));
    }
    assert_eq!(fs::read(&blocker).unwrap(), b"preserve owned ancestor");
    assert_eq!(fs::read_dir(&ordinary).unwrap().count(), 1);
}

#[test]
fn actual_owned_hardlink_refuses_single_link_authority_without_mutating_either_name() {
    let (_temp, ordinary, canonical) = owned_roots();
    let original = raw_child(&ordinary, "owned-preserved-file");
    let path = raw_child(&ordinary, VAULT_FILENAME);
    native_io(
        "write owned preserved file",
        &original,
        fs::write(&original, b"preserve owned hardlink bytes"),
    );
    native_io(
        "hard_link owned file",
        &path,
        fs::hard_link(&original, &path),
    );
    let info = handle_probe(&path);
    assert_eq!(info.nNumberOfLinks, 2);
    for root in [&ordinary, &canonical] {
        let result = NativeLocation::capture(
            VaultLocation::under_host_config_root(root).unwrap(),
            &mut Budget::new(Limits::default(), None),
        );
        assert_eq!(result.err(), Some(PrivacyError::Integrity));
    }
    assert_eq!(
        fs::read(&original).unwrap(),
        b"preserve owned hardlink bytes"
    );
    assert_eq!(fs::read(&path).unwrap(), b"preserve owned hardlink bytes");
    assert!(observed_identity(&handle_probe(&original)) == observed_identity(&info));
    assert_eq!(fs::read_dir(&ordinary).unwrap().count(), 2);
}

#[test]
fn actual_owned_file_identity_replacement_refuses_and_preserves_the_original_inode() {
    for canonical in [false, true] {
        let (_temp, ordinary, verbatim) = owned_roots();
        let root = if canonical { &verbatim } else { &ordinary };
        let captured = capture_owned(root);
        native_io(
            "write owned captured file",
            &captured.path,
            fs::write(&captured.path, b"preserve originally captured bytes"),
        );
        let retained = raw_child(root, "owned-retained-original");
        native_io(
            "rename owned captured file",
            &retained,
            fs::rename(&captured.path, &retained),
        );
        native_io(
            "write owned replacement",
            &captured.path,
            fs::write(&captured.path, b"owned replacement bytes"),
        );
        assert_eq!(
            captured.recheck(&mut Budget::new(Limits::default(), None)),
            Err(PrivacyError::Integrity)
        );
        assert!(captured.file_id == observed_identity(&handle_probe(&retained)));
        assert_eq!(
            fs::read(&retained).unwrap(),
            b"preserve originally captured bytes"
        );
        assert_eq!(
            fs::read(&captured.path).unwrap(),
            b"owned replacement bytes"
        );
    }
}

#[test]
fn native_ancestor_walk_retains_call_and_elapsed_refusal_before_owned_creation() {
    let (_temp, ordinary, canonical) = owned_roots();
    for root in [&ordinary, &canonical] {
        let limits = Limits {
            io_calls: 2,
            ..Limits::default()
        };
        let result = NativeLocation::capture(
            VaultLocation::under_host_config_root(root).unwrap(),
            &mut Budget::new(limits, None),
        );
        assert_eq!(result.err(), Some(PrivacyError::Bounds));
        let limits = Limits {
            elapsed: std::time::Duration::from_nanos(1),
            ..Limits::default()
        };
        let mut budget = Budget::new(limits, None);
        while budget.started.elapsed() <= budget.limits.elapsed {
            std::hint::spin_loop();
        }
        let result = NativeLocation::capture(
            VaultLocation::under_host_config_root(root).unwrap(),
            &mut budget,
        );
        assert_eq!(result.err(), Some(PrivacyError::Deadline));
        assert_eq!(budget.calls, 0);
    }
    assert_eq!(fs::read_dir(&ordinary).unwrap().count(), 0);
}

#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "DeviceIoControl"]
    fn device_io_control(
        file: *mut c_void,
        control: u32,
        input: *mut c_void,
        input_bytes: u32,
        output: *mut c_void,
        output_bytes: u32,
        returned_bytes: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

fn create_owned_junction(path: &Path, target: &Path) {
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xa0000003;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x000900a4;
    native_io(
        "create owned junction directory",
        path,
        fs::create_dir(path),
    );
    let mut substitute = OsString::from(r"\??\");
    substitute.push(target.as_os_str());
    let substitute: Vec<u16> = substitute.encode_wide().collect();
    let print: Vec<u16> = target.as_os_str().encode_wide().collect();
    let mut paths = substitute.clone();
    paths.push(0);
    paths.extend_from_slice(&print);
    paths.push(0);
    let mut data = Vec::new();
    data.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    data.extend_from_slice(&u16::try_from(8 + paths.len() * 2).unwrap().to_le_bytes());
    data.extend_from_slice(&0_u16.to_le_bytes());
    data.extend_from_slice(&0_u16.to_le_bytes());
    data.extend_from_slice(&u16::try_from(substitute.len() * 2).unwrap().to_le_bytes());
    data.extend_from_slice(
        &u16::try_from((substitute.len() + 1) * 2)
            .unwrap()
            .to_le_bytes(),
    );
    data.extend_from_slice(&u16::try_from(print.len() * 2).unwrap().to_le_bytes());
    for word in paths {
        data.extend_from_slice(&word.to_le_bytes());
    }
    assert!(data.len() <= 16 * 1024);
    let file = native_io(
        "open owned junction directory",
        path,
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path),
    );
    let mut returned_bytes = 0;
    let returned = unsafe {
        device_io_control(
            file.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            data.as_mut_ptr().cast(),
            u32::try_from(data.len()).unwrap(),
            std::ptr::null_mut(),
            0,
            &mut returned_bytes,
            std::ptr::null_mut(),
        )
    };
    let result = if returned == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    native_io(
        "DeviceIoControl(FSCTL_SET_REPARSE_POINT) on owned junction",
        path,
        result,
    );
}

#[test]
fn actual_owned_junction_refuses_reparse_authority_without_touching_the_target() {
    let (_temp, ordinary, canonical) = owned_roots();
    let target = raw_child(&ordinary, "owned-junction-target");
    let junction = raw_child(&ordinary, "owned-junction");
    native_io(
        "create owned junction target",
        &target,
        fs::create_dir(&target),
    );
    let preserved = raw_child(&target, "owned-preserved-file");
    native_io(
        "write owned junction target file",
        &preserved,
        fs::write(&preserved, b"preserve owned reparse target"),
    );
    create_owned_junction(&junction, &target);
    assert_ne!(
        handle_probe(&junction).dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT,
        0
    );
    for root in [&ordinary, &canonical] {
        let selected = raw_child(root, "owned-junction");
        metadata_probe("actual owned junction", &selected).unwrap();
        assert_eq!(
            identity_windows(&selected, true, &mut Budget::new(Limits::default(), None)).err(),
            Some(PrivacyError::Integrity)
        );
        let result = NativeLocation::capture(
            VaultLocation::under_host_config_root(&selected).unwrap(),
            &mut Budget::new(Limits::default(), None),
        );
        assert_eq!(result.err(), Some(PrivacyError::Integrity));
    }
    assert_eq!(
        fs::read(&preserved).unwrap(),
        b"preserve owned reparse target"
    );
    assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
}
