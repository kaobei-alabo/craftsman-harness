use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::permissions::{Mode, Selectors};
use craftsman_harness::Backend;
use serde_json::{Value, json};
use std::fs;

struct Fixture {
    root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let config = workspace.join("selected-config");
        fs::create_dir_all(&config).unwrap();
        write(&config, "config.json", json!({
            "model":"synthetic-model", "api_key":"synthetic-key", "gadgets":{"enabled":true},
            "permission_mode":"bypass-permissions", "auto_approve":true, "auto_approve_web":true,
        }).to_string().as_bytes());
        Self {
            root,
            workspace,
            config,
        }
    }
    async fn prepare(&self, trusted: bool, selectors: Selectors) -> Prepared {
        Prepared::prepare(Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: trusted,
            session: SessionSelection::Ephemeral,
            selectors,
        })
        .await
        .unwrap()
    }
    fn bundle(
        &self,
        kind: SourceKind,
        id: &str,
        capabilities: &Capabilities,
        component: &[u8],
    ) -> PathBuf {
        let directory = match kind {
            SourceKind::User => self.config.join("gadgets").join(id),
            SourceKind::Project => self.workspace.join(".craftsman/gadgets").join(id),
        };
        manifest(&directory, id, capabilities);
        write(&directory, "gadget.wasm", component);
        directory.canonicalize().unwrap()
    }
    fn grants(&self, entries: Vec<Value>) -> Vec<u8> {
        let bytes = json!({"version":1,"entries":entries})
            .to_string()
            .into_bytes();
        write(&self.config, "gadgets/.grants.json", &bytes);
        private(&self.config.join("gadgets/.grants.json"));
        bytes
    }
    async fn authority(&self, trusted: bool, ids: &[&str]) -> SourceAuthority {
        SourceAuthority::from_prepared(
            &self.prepare(trusted, Selectors::default()).await,
            ids.iter().map(|id| (*id).into()).collect(),
        )
        .await
        .unwrap()
    }
}
fn write(base: &Path, name: &str, bytes: &[u8]) {
    let file = base.join(name);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, bytes).unwrap();
}
fn manifest(directory: &Path, id: &str, capabilities: &Capabilities) {
    write(
        directory,
        "gadget.toml",
        format!(
            "id='{id}'\nversion='0.1.0'\n{}",
            toml::to_string(&json!({"capabilities":capabilities})).unwrap()
        )
        .as_bytes(),
    );
}
fn capabilities(read: bool, write: bool, hosts: &[&str]) -> Capabilities {
    Capabilities {
        read: if read {
            BTreeSet::from(["workspace".into()])
        } else {
            BTreeSet::new()
        },
        write: if write {
            BTreeSet::from(["workspace".into()])
        } else {
            BTreeSet::new()
        },
        http: hosts.iter().map(|h| (*h).into()).collect(),
    }
}
fn private(file: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = file;
}
fn grant(id: &str, source: &Path, component: &[u8], capabilities: &Capabilities) -> Value {
    json!({"id":id,"source":source,"component_hash":digest(component),"capabilities":capabilities})
}
fn error<T>(result: Result<T>, expected: SourceError) {
    assert_eq!(result.err().unwrap(), expected);
}

#[tokio::test]
async fn invocation_snapshot_pins_raw_hashes_bytes_source_and_origin_without_guest_execution() {
    let f = Fixture::new();
    let raw = b"\xef\xbb\xbfnot WASM\0\xff\r\n";
    let caps = capabilities(true, true, &["example.com"]);
    let directory = f.bundle(SourceKind::User, "demo", &caps, raw);
    let text = fs::read(directory.join("gadget.toml")).unwrap();
    let text = [b"\xef\xbb\xbf".as_slice(), &text].concat();
    fs::write(directory.join("gadget.toml"), &text).unwrap();
    let a = f.authority(false, &["demo"]).await;
    let source = a.authorize(SourceKind::User, "demo").await.unwrap();
    assert_eq!(source.component_bytes(), raw);
    assert_eq!(source.manifest_bytes(), text);
    assert_eq!(source.manifest().capabilities, caps);
    let record = source.approval();
    assert_eq!(record.id, "demo");
    assert_eq!(record.source, directory);
    assert_eq!(record.kind, SourceKind::User);
    assert_eq!(record.component_hash, digest(raw));
    assert_eq!(record.manifest_hash, digest(&text));
    assert_eq!(record.capabilities, caps);
    assert_eq!(
        record.origin,
        ApprovalOrigin::Invocation {
            manifest_hash: digest(&text),
            component_hash: digest(raw),
            capabilities: caps,
            inherited_rebuild: false,
        }
    );
    // Invalid executable bytes succeeding here proves no Wasmtime/describe runs.
    assert!(!f.config.join("gadgets/.grants.json").exists());
    assert!(!f.config.join("gadgets/.grants.lock").exists());
    assert!(!f.config.join("gadgets/.sdk").exists());
    assert!(!f.config.join("threads.db").exists());
    assert!(!f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn declaration_and_ambient_bypass_do_not_approve_any_source() {
    let f = Fixture::new();
    f.bundle(
        SourceKind::User,
        "demo",
        &capabilities(true, true, &["example.com"]),
        b"not code",
    );
    let a = f.authority(false, &[]).await;
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::NeedsApproval,
    );
    assert!(!f.config.join("gadgets/.grants.json").exists());
    assert!(!f.config.join("threads.db").exists());
}

#[tokio::test]
async fn invalid_explicit_identifiers_fail_before_source_or_grant_inspection() {
    let f = Fixture::new();
    write(
        &f.config,
        "gadgets/.grants.json",
        b"invalid and must not be opened",
    );
    let p = f.prepare(false, Selectors::default()).await;
    for ids in [
        vec!["../escape".into()],
        vec!["demo".into(), "demo".into()],
        vec!["A".into()],
        (0..257).map(|i| format!("id-{i}")).collect(),
    ] {
        error(
            SourceAuthority::from_prepared(&p, ids).await,
            SourceError::InvalidSelector,
        );
    }
    let a = SourceAuthority::from_prepared(&p, vec![]).await.unwrap();
    for id in ["../escape", "DEMO", "a", "demo\nsecret", "demo/foo"] {
        error(
            a.authorize(SourceKind::User, id).await,
            SourceError::InvalidSelector,
        );
    }
}

#[tokio::test]
async fn user_opt_in_is_required_even_with_an_explicit_id_or_valid_grant() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    f.grants(vec![grant(
        "demo",
        &directory,
        b"synthetic",
        &Capabilities::default(),
    )]);
    write(
        &f.config,
        "config.json",
        br#"{"model":"synthetic","gadgets":{"enabled":false}}"#,
    );
    let p = f.prepare(false, Selectors::default()).await;
    error(
        SourceAuthority::from_prepared(&p, vec!["demo".into()]).await,
        SourceError::Disabled,
    );
}

#[tokio::test]
async fn explicit_tool_deny_wins_over_invocation_and_source_bound_grants() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    f.grants(vec![grant(
        "demo",
        &directory,
        b"synthetic",
        &Capabilities::default(),
    )]);
    let p = f
        .prepare(
            true,
            Selectors {
                allow: vec!["tool:gadget_call".into()],
                deny: vec!["tool:gadget_call".into()],
                ..Default::default()
            },
        )
        .await;
    error(
        SourceAuthority::from_prepared(&p, vec!["demo".into()]).await,
        SourceError::Denied,
    );
}

#[tokio::test]
async fn project_trust_is_checked_before_source_read_even_with_a_user_level_grant() {
    let f = Fixture::new();
    let project = f.bundle(
        SourceKind::Project,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    f.grants(vec![grant(
        "demo",
        &project,
        b"synthetic",
        &Capabilities::default(),
    )]);
    // An invalid manifest makes any accidental read observable as InvalidSource.
    write(&project, "gadget.toml", b"invalid secret manifest");
    let a = f.authority(false, &["demo"]).await;
    error(
        a.authorize(SourceKind::Project, "demo").await,
        SourceError::UntrustedProject,
    );
    manifest(&project, "demo", &Capabilities::default());
    let trusted = f.authority(true, &[]).await;
    let source = trusted
        .authorize(SourceKind::Project, "demo")
        .await
        .unwrap();
    assert_eq!(source.approval().kind, SourceKind::Project);
    assert!(matches!(
        source.approval().origin,
        ApprovalOrigin::UserGrant { .. }
    ));
}

#[tokio::test]
async fn stored_grant_is_source_bound_and_project_grant_files_are_never_inputs() {
    let f = Fixture::new();
    let user = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"same bytes",
    );
    let project = f.bundle(
        SourceKind::Project,
        "demo",
        &Capabilities::default(),
        b"same bytes",
    );
    f.grants(vec![grant(
        "demo",
        &user,
        b"same bytes",
        &Capabilities::default(),
    )]);
    write(
        &f.workspace,
        ".craftsman/gadgets/.grants.json",
        json!({"version":1,"entries":[
        grant("demo", &project, b"same bytes", &Capabilities::default())]})
        .to_string()
        .as_bytes(),
    );
    let a = f.authority(true, &[]).await;
    assert!(a.authorize(SourceKind::User, "demo").await.is_ok());
    error(
        a.authorize(SourceKind::Project, "demo").await,
        SourceError::NeedsApproval,
    );
}

#[tokio::test]
async fn stored_grant_origin_records_original_component_capabilities_and_grant_file_hash() {
    let f = Fixture::new();
    let caps = capabilities(true, true, &["example.com"]);
    let directory = f.bundle(SourceKind::User, "demo", &caps, b"original");
    let bytes = f.grants(vec![grant("demo", &directory, b"original", &caps)]);
    let a = f.authority(false, &[]).await;
    let source = a.authorize(SourceKind::User, "demo").await.unwrap();
    assert_eq!(
        source.approval().origin,
        ApprovalOrigin::UserGrant {
            grant_file_hash: digest(&bytes),
            component_hash: digest(b"original"),
            capabilities: caps,
            inherited_rebuild: false,
        }
    );
}

#[tokio::test]
async fn stored_rebuilds_inherit_only_equal_or_narrower_capabilities_in_every_dimension() {
    let f = Fixture::new();
    let ceiling = capabilities(true, true, &["example.com", "other.example"]);
    let directory = f.bundle(SourceKind::User, "demo", &ceiling, b"original");
    f.grants(vec![grant("demo", &directory, b"original", &ceiling)]);
    let a = f.authority(false, &[]).await;
    write(&directory, "gadget.wasm", b"rebuilt");
    let narrower = capabilities(true, false, &["example.com"]);
    manifest(&directory, "demo", &narrower);
    let source = a.authorize(SourceKind::User, "demo").await.unwrap();
    assert!(matches!(
        source.approval().origin,
        ApprovalOrigin::UserGrant {
            inherited_rebuild: true,
            ..
        }
    ));
    assert_eq!(source.approval().capabilities, narrower);
    manifest(
        &directory,
        "demo",
        &capabilities(true, true, &["new.example"]),
    );
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::NeedsApproval,
    );
    // Independently pin read/write ceilings; an identical component hash cannot
    // excuse widening only the manifest's capabilities.
    for caps in [
        capabilities(true, false, &[]),
        capabilities(false, true, &[]),
    ] {
        f.grants(vec![grant(
            "demo",
            &directory,
            b"rebuilt",
            &Capabilities::default(),
        )]);
        manifest(&directory, "demo", &caps);
        let fresh = f.authority(false, &[]).await;
        error(
            fresh.authorize(SourceKind::User, "demo").await,
            SourceError::NeedsApproval,
        );
    }
}

#[tokio::test]
async fn invocation_binding_is_one_source_with_a_non_widening_capability_ceiling() {
    let f = Fixture::new();
    let caps = capabilities(true, false, &["example.com"]);
    let user = f.bundle(SourceKind::User, "demo", &caps, b"original");
    f.bundle(SourceKind::Project, "demo", &caps, b"original");
    let a = f.authority(true, &["demo"]).await;
    let first = a.authorize(SourceKind::User, "demo").await.unwrap();
    write(&user, "gadget.wasm", b"rebuilt");
    manifest(&user, "demo", &Capabilities::default());
    let next = a.authorize(SourceKind::User, "demo").await.unwrap();
    assert_eq!(
        next.approval().origin,
        ApprovalOrigin::Invocation {
            manifest_hash: first.approval().manifest_hash.clone(),
            component_hash: first.approval().component_hash.clone(),
            capabilities: caps,
            inherited_rebuild: true,
        }
    );
    error(
        a.authorize(SourceKind::Project, "demo").await,
        SourceError::NeedsApproval,
    );
    manifest(&user, "demo", &capabilities(true, true, &["example.com"]));
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::NeedsApproval,
    );
    // A separate invocation may explicitly consent to the now-selected source.
    assert!(
        f.authority(false, &["demo"])
            .await
            .authorize(SourceKind::User, "demo")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn grant_selection_is_deterministic_and_prefers_exact_component_evidence() {
    let f = Fixture::new();
    let caps = capabilities(true, false, &[]);
    let directory = f.bundle(SourceKind::User, "demo", &caps, b"current");
    let old = grant("demo", &directory, b"old", &caps);
    let exact = grant("demo", &directory, b"current", &caps);
    for entries in [vec![old.clone(), exact.clone()], vec![exact, old]] {
        f.grants(entries);
        let source = f
            .authority(false, &[])
            .await
            .authorize(SourceKind::User, "demo")
            .await
            .unwrap();
        assert!(
            matches!(&source.approval().origin, ApprovalOrigin::UserGrant {
            component_hash, inherited_rebuild:false, ..} if component_hash==&digest(b"current"))
        );
    }
    write(&directory, "gadget.wasm", b"rebuilt again");
    let expected = [digest(b"old"), digest(b"current")]
        .into_iter()
        .min()
        .unwrap();
    let old = grant("demo", &directory, b"old", &caps);
    let current = grant("demo", &directory, b"current", &caps);
    for entries in [vec![old.clone(), current.clone()], vec![current, old]] {
        f.grants(entries);
        let source = f
            .authority(false, &[])
            .await
            .authorize(SourceKind::User, "demo")
            .await
            .unwrap();
        assert!(
            matches!(&source.approval().origin, ApprovalOrigin::UserGrant {
            component_hash, inherited_rebuild:true, ..} if component_hash==&expected)
        );
    }
}

#[tokio::test]
async fn grant_snapshot_never_reloads_mutable_ambient_approvals_mid_invocation() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let a = f.authority(false, &[]).await;
    f.grants(vec![grant(
        "demo",
        &directory,
        b"synthetic",
        &Capabilities::default(),
    )]);
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::NeedsApproval,
    );
    let fresh = f.authority(false, &[]).await;
    fs::remove_file(f.config.join("gadgets/.grants.json")).unwrap();
    assert!(fresh.authorize(SourceKind::User, "demo").await.is_ok());
    assert!(!f.config.join("gadgets/.grants.json").exists());
}

#[tokio::test]
async fn source_approval_does_not_grant_host_writes_or_override_plan() {
    let f = Fixture::new();
    let caps = capabilities(true, true, &["example.com"]);
    f.bundle(SourceKind::User, "demo", &caps, b"synthetic");
    for mode in [Mode::Manual, Mode::Plan] {
        let p = f
            .prepare(
                false,
                Selectors {
                    mode,
                    ..Default::default()
                },
            )
            .await;
        let a = SourceAuthority::from_prepared(&p, vec!["demo".into()])
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&a.0.permissions, p.permissions()));
        let approved = a.authorize(SourceKind::User, "demo").await.unwrap();
        let guest = super::super::confined::ConfinedGuestFs::from_prepared(
            &p,
            approved.manifest().capabilities.clone(),
        )
        .unwrap();
        let kind = guest
            .write_file("should-not-exist", b"no")
            .await
            .err()
            .unwrap()
            .kind;
        assert_eq!(
            kind,
            if mode == Mode::Plan {
                ErrorKind::DeniedCapability
            } else {
                ErrorKind::NeedsApproval
            }
        );
        assert!(
            p.filesystem()
                .read("selected-config/config.json", 0, 10)
                .await
                .is_err()
        );
    }
    assert!(!f.workspace.join("should-not-exist").exists());
}

#[tokio::test]
async fn invalid_manifest_and_missing_data_only_components_never_become_approved_code() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let a = f.authority(false, &["demo"]).await;
    for text in [
        b"id='other'\nversion='1'\n".as_slice(),
        b"id='demo'\nversion='1'\nunknown='secret'",
        b"id='demo'\nversion='1'\n[capabilities]\nread=['outside']",
        b"\xff\0",
    ] {
        write(&directory, "gadget.toml", text);
        error(
            a.authorize(SourceKind::User, "demo").await,
            SourceError::InvalidSource,
        );
    }
    assert!(
        a.0.bound.lock().unwrap().is_empty(),
        "failed validation cannot bind an explicit grant"
    );
    manifest(&directory, "demo", &Capabilities::default());
    fs::remove_file(directory.join("gadget.wasm")).unwrap();
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::NotFound,
    );
    assert!(a.0.bound.lock().unwrap().is_empty());
}

#[tokio::test]
async fn selected_broken_project_source_never_falls_back_to_an_approved_user_bundle() {
    let f = Fixture::new();
    f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let project = f.bundle(
        SourceKind::Project,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    write(&project, "gadget.toml", b"broken");
    let a = f.authority(true, &["demo"]).await;
    error(
        a.authorize(SourceKind::Project, "demo").await,
        SourceError::InvalidSource,
    );
    assert!(a.0.bound.lock().unwrap().is_empty());
}

#[tokio::test]
async fn all_source_and_grant_bounds_are_checked_without_truncation_or_repair() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let a = f.authority(false, &["demo"]).await;
    fs::File::create(directory.join("gadget.toml"))
        .unwrap()
        .set_len((MAX_MANIFEST_BYTES + 1) as u64)
        .unwrap();
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::Bounds,
    );
    manifest(&directory, "demo", &Capabilities::default());
    fs::File::create(directory.join("gadget.wasm"))
        .unwrap()
        .set_len((MAX_COMPONENT_BYTES + 1) as u64)
        .unwrap();
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::Bounds,
    );
    write(&directory, "gadget.wasm", b"synthetic");
    let grant_path = f.config.join("gadgets/.grants.json");
    fs::File::create(&grant_path)
        .unwrap()
        .set_len((MAX_GRANTS_BYTES + 1) as u64)
        .unwrap();
    private(&grant_path);
    let a = f.authority(false, &[]).await;
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::Grants,
    );
    assert_eq!(
        fs::metadata(&grant_path).unwrap().len(),
        (MAX_GRANTS_BYTES + 1) as u64
    );
    f.grants(vec![
        grant(
            "demo",
            &directory,
            b"synthetic",
            &Capabilities::default()
        );
        MAX_GRANTS + 1
    ]);
    error(
        f.authority(false, &[])
            .await
            .authorize(SourceKind::User, "demo")
            .await,
        SourceError::Grants,
    );
}

#[tokio::test]
async fn malformed_grants_fail_closed_but_do_not_invalidate_independent_explicit_approval() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let mut entry = grant("demo", &directory, b"synthetic", &Capabilities::default());
    let valid = entry.clone();
    entry["component_hash"] = json!("bad secret hash");
    let mut bad_path = valid.clone();
    bad_path["source"] = json!("relative/secret");
    let mut bad_id = valid.clone();
    bad_id["id"] = json!("../secret");
    let mut bad_caps = valid.clone();
    bad_caps["capabilities"]["http"] = json!(["SECRET.invalid"]);
    let mut unknown = valid;
    unknown["secret"] = json!("secret value");
    for bytes in [
        b"invalid secret json".to_vec(),
        json!({"version":2,"entries":[]}).to_string().into_bytes(),
        json!({"version":1,"entries":[entry]})
            .to_string()
            .into_bytes(),
        json!({"version":1,"entries":[bad_path]})
            .to_string()
            .into_bytes(),
        json!({"version":1,"entries":[bad_id]})
            .to_string()
            .into_bytes(),
        json!({"version":1,"entries":[bad_caps]})
            .to_string()
            .into_bytes(),
        json!({"version":1,"entries":[unknown]})
            .to_string()
            .into_bytes(),
    ] {
        write(&f.config, "gadgets/.grants.json", &bytes);
        private(&f.config.join("gadgets/.grants.json"));
        let a = f.authority(false, &[]).await;
        error(
            a.authorize(SourceKind::User, "demo").await,
            SourceError::Grants,
        );
        let explicit = f.authority(false, &["demo"]).await;
        assert!(explicit.authorize(SourceKind::User, "demo").await.is_ok());
        assert_eq!(
            fs::read(f.config.join("gadgets/.grants.json")).unwrap(),
            bytes
        );
        assert!(!f.config.join("gadgets/.grants.lock").exists());
    }
}

#[tokio::test]
async fn exact_manifest_hash_changes_even_without_a_component_or_capability_change() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let a = f.authority(false, &["demo"]).await;
    let first = a.authorize(SourceKind::User, "demo").await.unwrap();
    write(
        &directory,
        "gadget.toml",
        b"id='demo'\nversion='0.1.0'\nprompt='changed context'\n",
    );
    let next = a.authorize(SourceKind::User, "demo").await.unwrap();
    assert_eq!(
        first.approval().component_hash,
        next.approval().component_hash
    );
    assert_ne!(
        first.approval().manifest_hash,
        next.approval().manifest_hash
    );
    assert_eq!(next.manifest().prompt.as_deref(), Some("changed context"));
    assert!(matches!(
        next.approval().origin,
        ApprovalOrigin::Invocation {
            inherited_rebuild: true,
            ..
        }
    ));
}

#[tokio::test]
async fn approved_owned_bytes_cannot_be_rebound_by_later_file_or_config_mutation() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"approved bytes",
    );
    let p = f.prepare(false, Selectors::default()).await;
    let a = SourceAuthority::from_prepared(&p, vec!["demo".into()])
        .await
        .unwrap();
    let source = a.authorize(SourceKind::User, "demo").await.unwrap();
    write(&directory, "gadget.wasm", b"replacement bytes");
    write(
        &f.config,
        "config.json",
        br#"{"model":"synthetic","auto_approve":true}"#,
    );
    assert_eq!(source.component_bytes(), b"approved bytes");
    assert_eq!(source.approval().component_hash, digest(b"approved bytes"));
    assert_eq!(source.manifest().capabilities, Capabilities::default());
    assert!(!p.config().auto_approve);
}

#[tokio::test]
async fn exact_source_id_path_and_capabilities_all_match_independently_of_component_hash() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"same bytes",
    );
    let other = f.bundle(
        SourceKind::User,
        "other",
        &Capabilities::default(),
        b"same bytes",
    );
    f.grants(vec![
        grant("other", &directory, b"same bytes", &Capabilities::default()),
        grant("demo", &other, b"same bytes", &Capabilities::default()),
    ]);
    error(
        f.authority(false, &[])
            .await
            .authorize(SourceKind::User, "demo")
            .await,
        SourceError::NeedsApproval,
    );
}

#[test]
fn typed_diagnostics_never_echo_os_parser_path_or_source_data() {
    for error in [
        SourceError::Disabled,
        SourceError::InvalidSelector,
        SourceError::Denied,
        SourceError::UntrustedProject,
        SourceError::NeedsApproval,
        SourceError::NotFound,
        SourceError::InvalidSource,
        SourceError::Bounds,
        SourceError::Changed,
        SourceError::Grants,
        SourceError::Worker,
    ] {
        let gadget: GadgetError = error.into();
        assert_eq!(gadget.message, error.to_string());
        assert!(
            !gadget.message.contains("secret")
                && !gadget.message.contains("/tmp/")
                && !gadget.message.contains('\x1b')
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn retained_config_and_workspace_authorities_never_reopen_rebound_ambient_roots() {
    let f = Fixture::new();
    let user = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"original user",
    );
    let project = f.bundle(
        SourceKind::Project,
        "project",
        &Capabilities::default(),
        b"original project",
    );
    f.grants(vec![
        grant("demo", &user, b"original user", &Capabilities::default()),
        grant(
            "project",
            &project,
            b"original project",
            &Capabilities::default(),
        ),
    ]);
    let p = f.prepare(true, Selectors::default()).await;
    let moved_config = f.root.path().join("moved-config");
    fs::rename(&f.config, &moved_config).unwrap();
    let moved_workspace = f.root.path().join("moved-work");
    fs::rename(&f.workspace, &moved_workspace).unwrap();
    // Malicious replacements use the old source labels, and the config grants
    // are corrupted. Construction AFTER rebinding must still clone old handles.
    write(
        &f.config,
        "gadgets/demo/gadget.toml",
        b"invalid replacement",
    );
    write(
        &f.config,
        "gadgets/.grants.json",
        b"invalid replacement grants",
    );
    write(
        &f.workspace,
        ".craftsman/gadgets/project/gadget.toml",
        b"invalid replacement",
    );
    let a = SourceAuthority::from_prepared(&p, vec![]).await.unwrap();
    let user_source = a.authorize(SourceKind::User, "demo").await.unwrap();
    assert_eq!(user_source.component_bytes(), b"original user");
    let project_source = a.authorize(SourceKind::Project, "project").await.unwrap();
    assert_eq!(project_source.component_bytes(), b"original project");
    assert_eq!(user_source.approval().source, user);
    assert_eq!(project_source.approval().source, project);
    assert!(!moved_config.join("threads.db").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn source_and_parent_symlinks_hardlinks_and_fifos_are_never_followed_or_read() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let outside = f.root.path().join("outside");
    write(&outside, "gadget.wasm", b"outside bytes");
    manifest(&outside, "demo", &Capabilities::default());
    let a = f.authority(true, &["demo"]).await;
    for name in ["gadget.toml", "gadget.wasm"] {
        let original = fs::read(directory.join(name)).unwrap();
        fs::remove_file(directory.join(name)).unwrap();
        symlink(outside.join(name), directory.join(name)).unwrap();
        error(
            a.authorize(SourceKind::User, "demo").await,
            SourceError::InvalidSource,
        );
        fs::remove_file(directory.join(name)).unwrap();
        fs::hard_link(outside.join(name), directory.join(name)).unwrap();
        error(
            a.authorize(SourceKind::User, "demo").await,
            SourceError::InvalidSource,
        );
        fs::remove_file(directory.join(name)).unwrap();
        let path = std::ffi::CString::new(directory.join(name).to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            error(
                a.authorize(SourceKind::User, "demo").await,
                SourceError::InvalidSource,
            );
        })
        .await
        .unwrap();
        fs::remove_file(directory.join(name)).unwrap();
        fs::write(directory.join(name), original).unwrap();
    }
    let moved = f.root.path().join("moved-bundle");
    fs::rename(&directory, &moved).unwrap();
    symlink(&outside, &directory).unwrap();
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::InvalidSource,
    );
    fs::remove_file(&directory).unwrap();
    fs::rename(&moved, &directory).unwrap();
    let gadgets = f.config.join("gadgets");
    let moved_root = f.root.path().join("moved-gadgets");
    fs::rename(&gadgets, &moved_root).unwrap();
    symlink(&moved_root, &gadgets).unwrap();
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::InvalidSource,
    );
    assert!(a.0.bound.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn grants_reject_non_private_symlink_hardlink_and_fifo_files_without_repair() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let bytes = f.grants(vec![grant(
        "demo",
        &directory,
        b"synthetic",
        &Capabilities::default(),
    )]);
    let path = f.config.join("gadgets/.grants.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    error(
        f.authority(false, &[])
            .await
            .authorize(SourceKind::User, "demo")
            .await,
        SourceError::Grants,
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    fs::remove_file(&path).unwrap();
    let outside = f.root.path().join("outside-grants");
    fs::write(&outside, &bytes).unwrap();
    private(&outside);
    symlink(&outside, &path).unwrap();
    error(
        f.authority(false, &[])
            .await
            .authorize(SourceKind::User, "demo")
            .await,
        SourceError::Grants,
    );
    fs::remove_file(&path).unwrap();
    fs::hard_link(&outside, &path).unwrap();
    error(
        f.authority(false, &[])
            .await
            .authorize(SourceKind::User, "demo")
            .await,
        SourceError::Grants,
    );
    fs::remove_file(&path).unwrap();
    let fifo = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        error(
            f.authority(false, &[])
                .await
                .authorize(SourceKind::User, "demo")
                .await,
            SourceError::Grants,
        );
    })
    .await
    .unwrap();
    assert_eq!(fs::read(outside).unwrap(), bytes);
}

#[tokio::test]
async fn manifest_or_component_in_place_mutation_between_reads_fails_before_binding() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    for name in ["gadget.toml", "gadget.wasm"] {
        manifest(&directory, "demo", &Capabilities::default());
        write(&directory, "gadget.wasm", b"synthetic");
        let a = f.authority(false, &["demo"]).await;
        let target = directory.join(name);
        *a.0.hook.lock().unwrap() = Some(Arc::new(move |stage, _, _| {
            if stage == "component" {
                fs::write(&target, b"changed in place").unwrap();
            }
        }));
        error(
            a.authorize(SourceKind::User, "demo").await,
            SourceError::Changed,
        );
        assert!(a.0.bound.lock().unwrap().is_empty());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn parent_replacement_and_identical_byte_file_replacement_fail_closed_before_binding() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let a = f.authority(false, &["demo"]).await;
    let target = directory.clone();
    let moved = f.root.path().join("moved-bundle");
    *a.0.hook.lock().unwrap() = Some(Arc::new(move |stage, _, _| {
        if stage == "directory" {
            fs::rename(&target, &moved).unwrap();
            manifest(&target, "demo", &Capabilities::default());
            write(&target, "gadget.wasm", b"synthetic");
        }
    }));
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::Changed,
    );
    assert!(a.0.bound.lock().unwrap().is_empty());
    let a = f.authority(false, &["demo"]).await;
    let target = directory.join("gadget.wasm");
    *a.0.hook.lock().unwrap() = Some(Arc::new(move |stage, _, _| {
        if stage == "component" {
            fs::rename(&target, target.with_extension("old")).unwrap();
            fs::write(&target, b"synthetic").unwrap();
        }
    }));
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::Changed,
    );
    assert!(a.0.bound.lock().unwrap().is_empty());
}

#[tokio::test]
async fn restored_size_and_mtime_cannot_hide_manifest_byte_mutation() {
    let f = Fixture::new();
    let directory = f.bundle(
        SourceKind::User,
        "demo",
        &Capabilities::default(),
        b"synthetic",
    );
    let a = f.authority(false, &["demo"]).await;
    let target = directory.join("gadget.toml");
    let modified = fs::metadata(&target).unwrap().modified().unwrap();
    let bytes = String::from_utf8(fs::read(&target).unwrap())
        .unwrap()
        .replace("0.1.0", "0.2.0");
    *a.0.hook.lock().unwrap() = Some(Arc::new(move |stage, _, _| {
        if stage == "component" {
            fs::write(&target, &bytes).unwrap();
            fs::OpenOptions::new()
                .write(true)
                .open(&target)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
    }));
    error(
        a.authorize(SourceKind::User, "demo").await,
        SourceError::Changed,
    );
    assert!(a.0.bound.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cloned_authorities_share_one_monotonic_invocation_binding_across_concurrent_loads() {
    let f = Fixture::new();
    f.bundle(SourceKind::User, "demo", &Capabilities::default(), b"user");
    f.bundle(
        SourceKind::Project,
        "demo",
        &Capabilities::default(),
        b"project",
    );
    let a = f.authority(true, &["demo"]).await;
    let b = a.clone();
    let (user, project) = tokio::join!(
        a.authorize(SourceKind::User, "demo"),
        b.authorize(SourceKind::Project, "demo")
    );
    match (user, project) {
        (Ok(source), Err(error)) | (Err(error), Ok(source)) => {
            assert_eq!(error, SourceError::NeedsApproval);
            assert_eq!(a.0.bound.lock().unwrap().len(), 1);
            assert_eq!(
                a.0.bound.lock().unwrap()["demo"].source,
                source.approval().source
            );
        }
        _ => panic!("one explicit id must not approve two sources through cloned authority"),
    }
}
