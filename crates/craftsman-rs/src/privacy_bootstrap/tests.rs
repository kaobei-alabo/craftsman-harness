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

#[cfg(feature = "gadgets")]
async fn key_and_policy_owner(root: &Path, workspace: &Path) -> PrivateConversation {
    // This is a real key/policy/vault owner test, not detector or model proof.
    // The pinned source is never described/executed here. Its actual approved
    // source-built coverage remains in the owned WASM/PTY cases.
    // The real derivation below takes the process-wide production admission,
    // which answers Busy instead of waiting. Hold the key tests' own serial
    // gate from before it until the key owner has closed and waited.
    let _serial = crate::privacy_key::Passphrase::kdf_test_serial();
    let bootstrap = PrivateBootstrap::from_captured_root(
        ConfigRoot::capture_owned_test_root(root).unwrap(),
        workspace,
    )
    .unwrap();
    let phrase = crate::privacy_key::Passphrase::from_owned(zeroize::Zeroizing::new(
        b"owned bootstrap test passphrase".to_vec(),
    ))
    .unwrap();
    let confirmation = crate::privacy_key::Passphrase::from_owned(zeroize::Zeroizing::new(
        b"owned bootstrap test passphrase".to_vec(),
    ))
    .unwrap();
    let unlocked = bootstrap
        .operations
        .initialize(phrase, confirmation)
        .await
        .unwrap();
    bootstrap.operations.close_and_wait().await.unwrap();
    let detector = detector(&bootstrap.policy, &bootstrap.workspace).unwrap();
    let session = Arc::new(
        PrivacySession::open(unlocked.vault(), bootstrap.binding, detector)
            .await
            .unwrap(),
    );
    PrivateConversation {
        policy: bootstrap.policy,
        unlocked,
        workspace: bootstrap.workspace,
        session,
    }
}

#[cfg(feature = "gadgets")]
#[tokio::test]
async fn settings_refusal_keeps_live_owned_sources_and_external_changes_close_the_key() {
    let root = owned();
    let workspace = owned();
    let original = br#"{"privacy":{"enabled":true,"detector":{"gadget":"owned-detector","function":"detect","component_sha256":"0000000000000000000000000000000000000000000000000000000000000000","manifest_sha256":"1111111111111111111111111111111111111111111111111111111111111111"}}}"#;
    config(root.path(), original);
    let conversation = Arc::new(key_and_policy_owner(root.path(), workspace.path()).await);
    let retained = conversation.clone();
    assert_eq!(
        conversation.before_configuration_save(),
        Err(BootstrapError::SettingsSaveRefused)
    );
    assert!(retained.check_sources().is_ok());
    assert!(retained.unlocked.recheck_key_source().is_ok());
    assert_eq!(
        std::fs::read(root.path().join("config.json")).unwrap(),
        original
    );
    std::fs::rename(
        root.path().join("config.json"),
        root.path().join("retained-old-config"),
    )
    .unwrap();
    config(root.path(), original);
    assert_eq!(
        retained.check_sources(),
        Err(BootstrapError::ConfigurationChanged)
    );
    assert!(retained.unlocked.recheck_key_source().is_err());
    assert!(retained.session.check().is_err());
}

#[cfg(feature = "gadgets")]
#[tokio::test]
async fn failed_prebuild_clear_closes_session_and_key_with_a_retained_owner() {
    let root = owned();
    let workspace = owned();
    config(root.path(), br#"{"privacy":{"enabled":true,"detector":{"gadget":"owned-detector","function":"detect","component_sha256":"0000000000000000000000000000000000000000000000000000000000000000","manifest_sha256":"1111111111111111111111111111111111111111111111111111111111111111"}}}"#);
    let mut conversation = key_and_policy_owner(root.path(), workspace.path()).await;
    std::fs::rename(
        root.path().join("config.json"),
        root.path().join("retained-old-config"),
    )
    .unwrap();
    config(root.path(), br#"{"privacy":{"enabled":false}}"#);
    assert_eq!(
        conversation.clear_before_new_build().await,
        Err(BootstrapError::ConfigurationChanged)
    );
    assert!(conversation.session.check().is_err());
    assert!(conversation.unlocked.recheck_key_source().is_err());
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
