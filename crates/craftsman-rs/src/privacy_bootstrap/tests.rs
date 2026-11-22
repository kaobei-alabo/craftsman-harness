//! Actual private user-source ownership, without ambient config or secret IO.
use super::*;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn owned() -> tempfile::TempDir {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    root
}
fn config(root: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("config.json"))
        .unwrap();
    file.write_all(bytes).unwrap();
}

#[test]
fn real_captured_user_policy_is_not_a_merged_extra_or_an_activation_selector() {
    let root = owned();
    config(root.path(), br#"{"privacy":{"enabled":true,"detector":{"gadget":"owned-detector","function":"detect","component_sha256":"0000000000000000000000000000000000000000000000000000000000000000","manifest_sha256":"1111111111111111111111111111111111111111111111111111111111111111"}}}"#);
    let bootstrap = PrivateBootstrap::from_captured_root(
        ConfigRoot::capture_owned_test_root(root.path()).unwrap(),
        root.path(),
    )
    .unwrap();
    assert_eq!(
        bootstrap.policy.require_available(),
        Err(PolicyError::NotYetAvailable)
    );
    assert!(bootstrap.policy.proposed_detector().unwrap().is_some());
    assert_eq!(bootstrap.status().unwrap(), FilePresence::Missing);
    assert!(!format!("{bootstrap:?}").contains("owned-detector"));
    assert!(!root.path().join("privacy-key.v1").exists());
    assert!(!root.path().join("privacy-vault.db").exists());
    // The source descriptor is retained independently of any merged Config.
    std::fs::rename(
        root.path().join("config.json"),
        root.path().join("prior.json"),
    )
    .unwrap();
    config(root.path(), br#"{"privacy":{"enabled":false}}"#);
    assert_eq!(bootstrap.policy.recheck(), Err(PolicyError::Source));
    assert!(bootstrap.status().is_err());
}

#[test]
fn absent_disabled_and_declared_identifier_policies_have_explicit_private_limits() {
    for bytes in [
        b"{}".as_slice(),
        br#"{"privacy":{"enabled":false}}"#.as_slice(),
    ] {
        let root = owned();
        config(root.path(), bytes);
        let bootstrap = PrivateBootstrap::from_captured_root(
            ConfigRoot::capture_owned_test_root(root.path()).unwrap(),
            root.path(),
        )
        .unwrap();
        assert_eq!(bootstrap.policy.require_available(), Ok(()));
        assert_eq!(
            bootstrap.require_detector_policy(),
            Err(BootstrapError::NoPolicy)
        );
    }
    let root = owned();
    config(root.path(), br#"{"privacy":{"enabled":true,"detector":{"gadget":"owned-detector","function":"detect","component_sha256":"0000000000000000000000000000000000000000000000000000000000000000","manifest_sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"identifiers":["explicit-private-identifier"]}}"#);
    let bootstrap = PrivateBootstrap::from_captured_root(
        ConfigRoot::capture_owned_test_root(root.path()).unwrap(),
        root.path(),
    )
    .unwrap();
    assert_eq!(
        bootstrap.require_detector_policy(),
        Err(BootstrapError::IdentifiersUnavailable)
    );
    assert_eq!(bootstrap.status().unwrap(), FilePresence::Missing);
}

#[test]
fn malformed_or_replaced_real_policy_and_workspace_are_refused_before_key_io() {
    let root = owned();
    config(
        root.path(),
        br#"{"privacy":{"enabled":false,"unexpected":"disabled must not hide malformed"}}"#,
    );
    assert!(matches!(
        PrivateBootstrap::from_captured_root(
            ConfigRoot::capture_owned_test_root(root.path()).unwrap(),
            root.path()
        ),
        Err(BootstrapError::Policy(PolicyError::Invalid))
    ));
    let workspace = owned();
    let target = workspace.path().join("captured");
    std::fs::create_dir(&target).unwrap();
    let held = Workspace::capture(&target).unwrap();
    std::fs::rename(&target, workspace.path().join("old")).unwrap();
    std::fs::create_dir(&target).unwrap();
    assert_eq!(held.recheck(), Err(BootstrapError::Workspace));
    assert!(!root.path().join("privacy-key.v1").exists());
}
