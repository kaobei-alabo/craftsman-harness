use super::envelope::Envelope;
use super::*;
use crate::privacy_vault::{HostNamespace, PrivacyError, VaultLocation};

fn pass(value: &str) -> Passphrase {
    Passphrase::from_owned(Zeroizing::new(value.as_bytes().to_vec())).unwrap()
}

#[test]
fn actual_fixed_envelope_random_key_round_trip_and_secret_debug_absence() {
    let _serial = envelope::test_serial();
    let secret = pass("owned passphrase one");
    let (envelope, original) = Envelope::create(&secret).unwrap();
    assert_eq!(envelope.as_bytes().len(), 112);
    assert_eq!(&envelope.as_bytes()[..8], b"CPVKENV1");
    assert_eq!(&envelope.as_bytes()[16..20], &600000u32.to_be_bytes());
    assert!(
        !envelope
            .as_bytes()
            .windows(32)
            .any(|slice| slice == original.as_ref())
    );
    assert!(!format!("{secret:?} {envelope:?}").contains("owned passphrase"));
    let opened = Envelope::parse(Zeroizing::new(envelope.as_bytes().to_vec()))
        .unwrap()
        .unlock(&secret)
        .unwrap();
    assert_eq!(opened.as_ref(), original.as_ref());
    let (other, other_key) = Envelope::create(&secret).unwrap();
    assert_ne!(original.as_ref(), other_key.as_ref());
    assert_ne!(envelope.as_bytes(), other.as_bytes());
}

#[test]
fn raw_and_normalized_byte_bounds_are_independent_no_trim_or_loss() {
    let _serial = envelope::test_serial();
    for raw in [vec![b'x'; 11], vec![b'x'; 1025], vec![0xff; 12]] {
        assert!(matches!(
            Passphrase::from_owned(Zeroizing::new(raw)),
            Err(KeyError::Passphrase)
        ));
    }
    assert!(Passphrase::from_owned(Zeroizing::new("e\u{301}".repeat(4).into_bytes())).is_err());
    let composed = pass("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}");
    let decomposed = pass("e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}");
    let (envelope, key) = Envelope::create(&composed).unwrap();
    assert_eq!(envelope.unlock(&decomposed).unwrap().as_ref(), key.as_ref());
    let leading = pass(" owned phrase");
    let trailing = pass("owned phrase ");
    let (spaced, _) = Envelope::create(&leading).unwrap();
    assert!(matches!(spaced.unlock(&trailing), Err(KeyError::Unlock)));
    assert!(Passphrase::from_owned(Zeroizing::new(vec![b'x'; 1024])).is_ok());
}

#[test]
fn wrong_password_and_every_header_group_ciphertext_and_tag_fail_authentication() {
    let _serial = envelope::test_serial();
    let (envelope, _) = Envelope::create(&pass("owned correct secret")).unwrap();
    assert!(matches!(
        Envelope::parse(Zeroizing::new(envelope.as_bytes().to_vec()))
            .unwrap()
            .unlock(&pass("owned incorrect secret")),
        Err(KeyError::Unlock)
    ));
    for index in [0, 8, 12, 16] {
        let mut bytes = Zeroizing::new(envelope.as_bytes().to_vec());
        bytes[index] ^= 1;
        assert!(matches!(Envelope::parse(bytes), Err(KeyError::Invalid)));
    }
    for index in [20, 51, 52, 63, 64, 95, 96, 111] {
        let mut bytes = Zeroizing::new(envelope.as_bytes().to_vec());
        bytes[index] ^= 1;
        assert!(matches!(
            Envelope::parse(bytes)
                .unwrap()
                .unlock(&pass("owned correct secret")),
            Err(KeyError::Unlock)
        ));
    }
    for length in [0, 64, 96, 111, 113] {
        assert!(matches!(
            Envelope::parse(Zeroizing::new(vec![0; length])),
            Err(KeyError::Invalid)
        ));
    }
}

#[cfg(unix)]
fn owned_root() -> (tempfile::TempDir, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().canonicalize().unwrap();
    (directory, path)
}
#[cfg(unix)]
fn manager(path: &std::path::Path) -> KeyManager {
    KeyManager::from_root(ConfigRoot::capture_owned_test_root(path).unwrap())
}

#[cfg(unix)]
#[tokio::test]
async fn owned_create_only_initialize_and_existing_unlock_keep_actual_encrypted_files() {
    let _serial = envelope::test_serial();
    use std::os::unix::fs::PermissionsExt;
    let (_root, path) = owned_root();
    let owner = manager(&path);
    let opened = owner
        .initialize(pass("owned filesystem secret"))
        .await
        .unwrap();
    let namespace = HostNamespace::supplied_by_host(
        "host",
        "workspace",
        "opaque-conversation",
        "profile",
        "context",
    )
    .unwrap();
    let session = opened.vault().session(namespace).unwrap();
    assert!(!format!("{owner:?} {opened:?}").contains("filesystem secret"));
    drop(session);
    opened.close();
    drop(opened);
    owner.close_and_wait().await.unwrap();
    for name in [root::KEY_FILENAME, "privacy-vault.db"] {
        assert_eq!(
            std::fs::metadata(path.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let contents = std::fs::read(path.join(name)).unwrap();
        assert!(
            !contents
                .windows(b"owned filesystem secret".len())
                .any(|s| s == b"owned filesystem secret")
        );
    }
    let key_before = std::fs::read(path.join(root::KEY_FILENAME)).unwrap();
    let second = manager(&path);
    let unlocked = second
        .unlock(pass("owned filesystem secret"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(path.join(root::KEY_FILENAME)).unwrap(),
        key_before
    );
    assert!(unlocked.key_source().acquire_privacy_key().is_ok());
    unlocked.close();
    assert!(unlocked.key_source().acquire_privacy_key().is_err());
    second.close_and_wait().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn missing_existing_and_partial_initialization_never_create_adopt_or_delete() {
    let _serial = envelope::test_serial();
    let (_root, path) = owned_root();
    let owner = manager(&path);
    assert!(matches!(
        owner.unlock(pass("owned missing secret")).await,
        Err(KeyError::Missing)
    ));
    assert!(std::fs::read_dir(&path).unwrap().next().is_none());
    owner.close_and_wait().await.unwrap();
    let (envelope, _) = Envelope::create(&pass("owned missing secret")).unwrap();
    let root = ConfigRoot::capture_owned_test_root(&path).unwrap();
    root.create_envelope(envelope.as_bytes()).unwrap();
    let key = std::fs::read(path.join(root::KEY_FILENAME)).unwrap();
    let second = manager(&path);
    assert!(matches!(
        second.initialize(pass("owned missing secret")).await,
        Err(KeyError::AlreadyInitialized)
    ));
    assert_eq!(std::fs::read(path.join(root::KEY_FILENAME)).unwrap(), key);
    assert!(matches!(
        second.unlock(pass("owned missing secret")).await,
        Err(KeyError::Vault)
    ));
    assert!(!path.join("privacy-vault.db").exists());
    second.close_and_wait().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn wrong_passphrase_attempts_are_finite_and_existing_files_stay_exact() {
    let _serial = envelope::test_serial();
    let (_root, path) = owned_root();
    let owner = manager(&path);
    let initialized = owner
        .initialize(pass("owned correct attempts"))
        .await
        .unwrap();
    drop(initialized);
    owner.close_and_wait().await.unwrap();
    let before = std::fs::read(path.join(root::KEY_FILENAME)).unwrap();
    let owner = manager(&path);
    for _ in 0..3 {
        assert!(matches!(
            owner.unlock(pass("owned wrong attempts")).await,
            Err(KeyError::Unlock)
        ));
    }
    assert!(matches!(
        owner.unlock(pass("owned correct attempts")).await,
        Err(KeyError::Attempts)
    ));
    assert_eq!(
        std::fs::read(path.join(root::KEY_FILENAME)).unwrap(),
        before
    );
    owner.close_and_wait().await.unwrap();
}

#[cfg(unix)]
#[test]
fn no_follow_single_link_user_only_files_and_ancestor_bindings_are_real() {
    let _serial = envelope::test_serial();
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (_root, path) = owned_root();
    let root = ConfigRoot::capture_owned_test_root(&path).unwrap();
    let (envelope, _) = Envelope::create(&pass("owned binding secret")).unwrap();
    let lease = root.create_envelope(envelope.as_bytes()).unwrap();
    std::fs::hard_link(path.join(root::KEY_FILENAME), path.join("second-link")).unwrap();
    assert!(lease.recheck().is_err());
    std::fs::remove_file(path.join("second-link")).unwrap();
    std::fs::set_permissions(
        path.join(root::KEY_FILENAME),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    assert!(root.read_envelope().is_err());
    std::fs::remove_file(path.join(root::KEY_FILENAME)).unwrap();
    symlink("owned-absent", path.join(root::KEY_FILENAME)).unwrap();
    assert!(root.read_envelope().is_err());
    assert!(!path.join("owned-absent").exists());
    let alias = path.join("alias-root");
    symlink(&path, &alias).unwrap();
    assert!(ConfigRoot::capture_owned_test_root(&alias).is_err());
}

struct TestKey;
impl HostKeySource for TestKey {
    fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([0x47; 32])))
    }
}
#[tokio::test]
async fn additive_existing_only_vault_never_creates_absence_and_authenticates_real_header() {
    let builder = tempfile::Builder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::PermissionsExt;
        let mut builder = builder;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
        builder
    };
    let root = builder.tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let location = || VaultLocation::under_host_config_root(&path).unwrap();
    assert!(
        Vault::open_existing(location(), &TestKey, Limits::default())
            .await
            .is_err()
    );
    assert!(!path.join("privacy-vault.db").exists());
    let opened = Vault::open(location(), &TestKey, Limits::default())
        .await
        .unwrap();
    opened.close();
    drop(opened);
    let existing = Vault::open_existing(location(), &TestKey, Limits::default())
        .await
        .unwrap();
    existing.close();
    drop(existing);
    assert!(path.join("privacy-vault.db").is_file());
    struct Wrong;
    impl HostKeySource for Wrong {
        fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
            Ok(HostKey::supplied_by_host(Zeroizing::new([0x48; 32])))
        }
    }
    assert!(matches!(
        Vault::open_existing(location(), &Wrong, Limits::default()).await,
        Err(PrivacyError::Key)
    ));
}

#[cfg(not(unix))]
#[test]
fn secure_creation_refuses_without_real_owner_dacl_proof() {
    assert!(matches!(
        KeyManager::capture_actual(),
        Err(KeyError::SecureStorageUnsupported)
    ));
}
