use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::permissions::{Mode, Selectors};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.json"),
            serde_json::json!({
                "model":"synthetic-model", "api_key":"synthetic-key", "auto_approve":true,
                "permission_mode":"bypass-permissions", "auto_approve_web":true,
            })
            .to_string(),
        )
        .unwrap();
        Self {
            root,
            workspace,
            config,
        }
    }
    async fn prepare(&self, selectors: Selectors) -> Prepared {
        Prepared::prepare(Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session: SessionSelection::Ephemeral,
            selectors,
        })
        .await
        .unwrap()
    }
    async fn filesystem(&self, selectors: Selectors, declaration: Capabilities) -> ConfinedGuestFs {
        ConfinedGuestFs::from_prepared(&self.prepare(selectors).await, declaration).unwrap()
    }
}
fn capabilities(read: bool, write: bool) -> Capabilities {
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
        ..Default::default()
    }
}
fn allow_write() -> Selectors {
    Selectors {
        allow: vec!["tool:write_file".into()],
        ..Default::default()
    }
}
fn write(base: &Path, path: &str, bytes: &[u8]) {
    let file = base.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, bytes).unwrap();
}
fn kind<T>(result: Result<T, GadgetError>) -> ErrorKind {
    result.err().unwrap().kind
}

#[tokio::test]
async fn binary_guest_read_and_create_preserve_bytes_not_lossy_text() {
    let f = Fixture::new();
    let guest = f.filesystem(allow_write(), capabilities(true, true)).await;
    let bytes = [0, 0xff, b'\r', b'\n', 0xc3, 0x28];
    guest.write_file("data/binary.bin", &bytes).await.unwrap();
    assert_eq!(guest.read_file("data/binary.bin").await.unwrap(), bytes);
    assert_eq!(
        std::fs::read(f.workspace.join("data/binary.bin")).unwrap(),
        bytes
    );
    assert_eq!(
        kind(guest.write_file("data/binary.bin", b"overwrite").await),
        ErrorKind::DeniedCapability
    );
    assert_eq!(guest.read_file("data/binary.bin").await.unwrap(), bytes);
    assert!(!f.config.join("threads.db").exists());
    assert!(!f.config.join("gadgets/.sdk").exists());
}

#[tokio::test]
async fn declaration_is_a_restriction_not_approval_or_an_invocation_write_grant() {
    let f = Fixture::new();
    let guest = f
        .filesystem(Selectors::default(), capabilities(true, true))
        .await;
    assert_eq!(
        kind(guest.write_file("new/sub/file.bin", b"no").await),
        ErrorKind::NeedsApproval
    );
    assert!(!f.workspace.join("new").exists());
    // Ambient bypass in the selected config never widens invocation policy.
    let prepared = f.prepare(allow_write()).await;
    let guest = ConfinedGuestFs::from_prepared(&prepared, capabilities(false, false)).unwrap();
    assert_eq!(
        kind(guest.write_file("new/file", b"no").await),
        ErrorKind::DeniedCapability
    );
    assert_eq!(
        kind(guest.read_file("missing").await),
        ErrorKind::DeniedCapability
    );
    assert_eq!(kind(guest.list_dir(".").await), ErrorKind::DeniedCapability);
}

#[tokio::test]
async fn declared_scopes_are_validated_without_running_a_guest() {
    let f = Fixture::new();
    let prepared = f.prepare(allow_write()).await;
    let mut declaration = capabilities(true, true);
    declaration.read.insert("outside".into());
    assert_eq!(
        kind(ConfinedGuestFs::from_prepared(&prepared, declaration)),
        ErrorKind::InvalidGadget
    );
    assert!(!f.config.join("gadgets/.grants.json").exists());
    assert!(!f.config.join("gadgets/.sdk").exists());
}

#[tokio::test]
async fn write_only_declaration_does_not_grant_reads_or_directory_metadata() {
    let f = Fixture::new();
    write(&f.workspace, "input.bin", b"data");
    let guest = f.filesystem(allow_write(), capabilities(false, true)).await;
    guest.write_file("output.bin", b"created").await.unwrap();
    assert_eq!(
        kind(guest.read_file("input.bin").await),
        ErrorKind::DeniedCapability
    );
    assert_eq!(kind(guest.list_dir(".").await), ErrorKind::DeniedCapability);
}

#[tokio::test]
async fn ordinary_tool_denies_and_plan_remain_final() {
    let f = Fixture::new();
    write(&f.workspace, "input", b"data");
    let mut selectors = allow_write();
    selectors.deny = vec![
        "tool:write_file".into(),
        "tool:read_file".into(),
        "tool:ls".into(),
    ];
    let guest = f.filesystem(selectors, capabilities(true, true)).await;
    assert_eq!(
        kind(guest.write_file("no/child", b"no").await),
        ErrorKind::DeniedCapability
    );
    assert_eq!(
        kind(guest.read_file("input").await),
        ErrorKind::DeniedCapability
    );
    assert_eq!(kind(guest.list_dir(".").await), ErrorKind::DeniedCapability);
    assert!(!f.workspace.join("no").exists());
    let mut plan = allow_write();
    plan.mode = Mode::Plan;
    let guest = f.filesystem(plan, capabilities(true, true)).await;
    assert_eq!(
        kind(guest.write_file("Cargo.toml", b"no").await),
        ErrorKind::DeniedCapability
    );
    assert!(!f.workspace.join("Cargo.toml").exists());
}

#[tokio::test]
async fn config_credentials_path_denies_and_reserved_temporaries_never_appear() {
    let f = Fixture::new();
    write(&f.workspace, ".env", b"SYNTHETIC-SECRET");
    write(&f.workspace, "blocked.bin", b"SYNTHETIC-SECRET");
    write(
        &f.workspace,
        ".craftsman-exec-tmp-planted",
        b"SYNTHETIC-SECRET",
    );
    let mut selectors = allow_write();
    selectors.deny.push("path:blocked.bin".into());
    let guest = f.filesystem(selectors, capabilities(true, true)).await;
    for path in [
        ".env",
        "selected-config/config.json",
        "blocked.bin",
        ".craftsman-exec-tmp-planted",
    ] {
        let error = guest.read_file(path).await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::DeniedCapability);
        assert!(!error.to_string().contains("SYNTHETIC-SECRET"));
        assert_eq!(
            kind(guest.write_file(path, b"change").await),
            ErrorKind::DeniedCapability
        );
    }
    let entries = guest.list_dir(".").await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn exact_ci_build_override_needs_tool_permission_and_does_not_override_hard_denies() {
    let f = Fixture::new();
    let mut selectors = allow_write();
    selectors.allow.push("path:Cargo.toml".into());
    let guest = f.filesystem(selectors, capabilities(true, true)).await;
    guest
        .write_file("Cargo.toml", b"[package]\n")
        .await
        .unwrap();
    assert_eq!(
        kind(guest.write_file("build.rs", b"no").await),
        ErrorKind::NeedsApproval
    );
    for path in [
        ".git/config",
        ".craftsman/gadget.toml",
        ".env.local",
        "selected-config/new",
    ] {
        assert_eq!(
            kind(guest.write_file(path, b"no").await),
            ErrorKind::DeniedCapability
        );
    }
    let selectors = Selectors {
        allow: vec!["path:build.rs".into()],
        ..Default::default()
    };
    let guest = f.filesystem(selectors, capabilities(true, true)).await;
    assert_eq!(
        kind(guest.write_file("build.rs", b"no").await),
        ErrorKind::NeedsApproval
    );
    assert!(!f.workspace.join("build.rs").exists());
}

#[tokio::test]
async fn listings_are_structured_sorted_and_contain_only_scoped_names() {
    let f = Fixture::new();
    write(&f.workspace, "z.bin", &[1, 2, 3]);
    write(&f.workspace, "dir/a.bin", &[0, 0xff]);
    write(&f.workspace, "台.bin", &[4]);
    let guest = f
        .filesystem(Selectors::default(), capabilities(true, false))
        .await;
    let entries = guest.list_dir(".").await.unwrap();
    assert_eq!(
        entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["dir", "z.bin", "台.bin"]
    );
    assert!(entries[0].is_dir);
    assert_eq!(entries[0].size, 0);
    assert!(!entries[1].is_dir);
    assert_eq!(entries[1].size, 3);
    let child = guest.list_dir("dir").await.unwrap();
    assert_eq!(child[0].name, "a.bin");
    assert_eq!(child[0].size, 2);
}

#[tokio::test]
async fn outside_and_ambiguous_paths_fail_without_echoing_caller_data() {
    let f = Fixture::new();
    let outside = f.root.path().join("PRIVATE-PATH-MUST-NOT-ECHO");
    std::fs::write(&outside, b"secret").unwrap();
    let guest = f.filesystem(allow_write(), capabilities(true, true)).await;
    let error = guest
        .read_file(outside.to_str().unwrap())
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind, ErrorKind::DeniedCapability);
    assert!(!error.to_string().contains("PRIVATE-PATH-MUST-NOT-ECHO"));
    for path in ["../outside", "C:drive", "no\u{9b}", "NUL.bin"] {
        assert!(guest.read_file(path).await.is_err());
        assert!(guest.write_file(path, b"no").await.is_err());
    }
    assert_eq!(kind(guest.read_file("missing").await), ErrorKind::NotFound);
}

#[tokio::test]
async fn write_and_read_bounds_are_explicit_and_never_silently_truncate() {
    let f = Fixture::new();
    let guest = f.filesystem(allow_write(), capabilities(true, true)).await;
    let bytes = vec![0xff; craftsman_gadget::MAX_JSON_BYTES];
    guest.write_file("at-bound.bin", &bytes).await.unwrap();
    assert_eq!(guest.read_file("at-bound.bin").await.unwrap(), bytes);
    let oversized = vec![0; craftsman_gadget::MAX_JSON_BYTES + 1];
    assert_eq!(
        kind(guest.write_file("oversize/child.bin", &oversized).await),
        ErrorKind::OutputLimit
    );
    assert!(!f.workspace.join("oversize").exists());
    std::fs::write(
        f.workspace.join("read-oversize.bin"),
        vec![0; 10 * 1024 * 1024 + 1],
    )
    .unwrap();
    assert_eq!(
        kind(guest.read_file("read-oversize.bin").await),
        ErrorKind::OutputLimit
    );
}

#[tokio::test]
async fn later_config_or_declaration_changes_cannot_widen_the_captured_policy() {
    let f = Fixture::new();
    let prepared = f.prepare(Selectors::default()).await;
    let mut original = capabilities(true, false);
    let guest = ConfinedGuestFs::from_prepared(&prepared, original.clone()).unwrap();
    original.write.insert("workspace".into());
    write(
        &f.config,
        "config.json",
        br#"{"model":"synthetic","permission_mode":"bypass-permissions","auto_approve":true}"#,
    );
    assert_eq!(
        kind(guest.write_file("after-config/file.bin", b"no").await),
        ErrorKind::DeniedCapability
    );
    let guest = ConfinedGuestFs::from_prepared(&prepared, original).unwrap();
    assert_eq!(
        kind(guest.write_file("after-config/file.bin", b"no").await),
        ErrorKind::NeedsApproval
    );
    assert!(!f.workspace.join("after-config").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_hardlinks_and_fifos_cannot_bypass_the_strict_guest_boundary() {
    let f = Fixture::new();
    write(&f.workspace, "single.bin", b"safe");
    std::os::unix::fs::symlink("single.bin", f.workspace.join("alias.bin")).unwrap();
    write(&f.workspace, "linked.bin", b"data");
    std::fs::hard_link(
        f.workspace.join("linked.bin"),
        f.workspace.join("linked-alias.bin"),
    )
    .unwrap();
    let fifo = f.workspace.join("pipe");
    let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let guest = f.filesystem(allow_write(), capabilities(true, true)).await;
    for path in ["alias.bin", "linked.bin", "linked-alias.bin", "pipe"] {
        assert_eq!(
            kind(guest.read_file(path).await),
            ErrorKind::DeniedCapability
        );
    }
    let entries = guest.list_dir(".").await.unwrap();
    assert_eq!(
        entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["single.bin"]
    );
}

#[tokio::test]
async fn guest_listing_bound_includes_filtered_config_and_credential_entries() {
    let f = Fixture::new();
    for i in 0..1023 {
        std::fs::write(f.workspace.join(format!(".env-{i:04}")), b"hidden").unwrap();
    }
    let guest = f
        .filesystem(Selectors::default(), capabilities(true, false))
        .await;
    // 1023 denied files plus the selected config directory consume the budget.
    assert!(guest.list_dir(".").await.unwrap().is_empty());
    std::fs::write(f.workspace.join(".env-overflow"), b"hidden").unwrap();
    assert_eq!(kind(guest.list_dir(".").await), ErrorKind::OutputLimit);
}

#[cfg(unix)]
#[tokio::test]
async fn facet_construction_reuses_prepared_authority_after_root_rebinding() {
    let f = Fixture::new();
    write(&f.workspace, "identity.bin", &[0, 0xff]);
    let prepared = f.prepare(allow_write()).await;
    let moved = f.root.path().join("retained-workspace");
    std::fs::rename(&f.workspace, &moved).unwrap();
    std::fs::create_dir(&f.workspace).unwrap();
    write(&f.workspace, "identity.bin", b"REPLACED-MUST-NOT-READ");
    let guest = ConfinedGuestFs::from_prepared(&prepared, capabilities(true, true)).unwrap();
    assert_eq!(guest.read_file("identity.bin").await.unwrap(), [0, 0xff]);
    guest.write_file("after.bin", &[0xfe, 1]).await.unwrap();
    assert_eq!(std::fs::read(moved.join("after.bin")).unwrap(), [0xfe, 1]);
    assert!(!f.workspace.join("after.bin").exists());
}
