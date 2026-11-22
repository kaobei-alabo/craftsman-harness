//! Synthetic host keys/roots only. Every vault assertion uses actual libSQL IO.

use super::storage::{InitPhase, Phase};
use super::*;
use libsql::{Connection, Value};
use std::fs;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct SyntheticKey(u8);

fn private_tempdir() -> TempDir {
    let builder = tempfile::Builder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::PermissionsExt;
        let mut builder = builder;
        builder.permissions(fs::Permissions::from_mode(0o700));
        builder
    };
    builder.tempdir().unwrap()
}

impl HostKeySource for SyntheticKey {
    fn acquire_privacy_key(&self) -> Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([self.0; 32])))
    }
}

fn namespace(project: &str, thread: &str) -> HostNamespace {
    HostNamespace::supplied_by_host(
        "synthetic-host-33",
        project,
        thread,
        "profile-55",
        "context-66",
    )
    .unwrap()
}

fn session(vault: &Vault) -> MappingSession {
    vault
        .session(namespace("synthetic-project-11", "synthetic-thread-22"))
        .unwrap()
}

async fn open(root: &Path, key: u8, limits: Limits) -> Result<Vault> {
    Vault::open(
        VaultLocation::under_host_config_root(root)?,
        &SyntheticKey(key),
        limits,
    )
    .await
}

async fn fresh(limits: Limits) -> (TempDir, PathBuf, Vault) {
    let temp = private_tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    let vault = open(&root, 7, limits).await.unwrap();
    (temp, root, vault)
}

struct Probe {
    conn: Connection,
    _database: libsql::Database,
}

impl Probe {
    async fn open(root: &Path) -> Self {
        let database = libsql::Builder::new_local(root.join(VAULT_FILENAME))
            .build()
            .await
            .unwrap();
        let conn = database.connect().unwrap();
        conn.busy_timeout(Duration::from_millis(200)).unwrap();
        Self {
            conn,
            _database: database,
        }
    }

    async fn count(&self) -> i64 {
        let mut rows = self
            .conn
            .query("SELECT count(*) FROM vault_mappings", ())
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    }

    async fn bytes(&self, column: &str) -> Vec<u8> {
        let sql = match column {
            "sealed" => "SELECT sealed FROM vault_mappings ORDER BY original_tag LIMIT 1",
            "alias_tag" => "SELECT alias_tag FROM vault_mappings ORDER BY original_tag LIMIT 1",
            "vault_id" => "SELECT vault_id FROM vault_metadata",
            _ => panic!("unregistered synthetic probe column"),
        };
        let mut rows = self.conn.query(sql, ()).await.unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    }

    async fn change(&self, sql: &str, params: Vec<Value>) {
        self.conn.execute(sql, params).await.unwrap();
    }
}

async fn mask_values(
    session: &MappingSession,
    text: &str,
    values: &[(&str, Category)],
) -> Result<MaskedText> {
    let captured = session.capture_text(text)?;
    let mut spans = Vec::new();
    let mut declared = Vec::new();
    for &(value, category) in values {
        for (start, occurrence) in text.match_indices(value) {
            spans.push(DetectorSpan {
                start,
                end: start + occurrence.len(),
                category,
            });
        }
        if let Category::Identifier(kind) = category {
            declared.push((kind, value));
        }
    }
    let report = DetectorReport {
        input_sha256: captured.input_sha256_for_host_detector()?,
        spans,
    };
    session
        .mask(
            captured,
            report,
            DeclaredIdentifiers::supplied_by_host(&declared)?,
        )
        .await
}

fn fields(masked: &MaskedText) -> Vec<(String, String, Category)> {
    let records = masked.resolution_records_for_host_fields().unwrap();
    (0..records.len().unwrap())
        .map(|index| {
            records
                .with_host_field(index, |field| {
                    (
                        field.known_alias().to_owned(),
                        field.original_for_validated_host_field().to_owned(),
                        field.category(),
                    )
                })
                .unwrap()
        })
        .collect()
}

fn report_for(captured: &CapturedText, spans: Vec<DetectorSpan>) -> DetectorReport {
    DetectorReport {
        input_sha256: captured.input_sha256_for_host_detector().unwrap(),
        spans,
    }
}

#[tokio::test]
async fn actual_libsql_limit_pragmas_return_rows_and_are_not_successful_execute_calls() {
    let temp = private_tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    let database = libsql::Builder::new_local(root.join("owned-pragma-probe.db"))
        .build()
        .await
        .unwrap();
    let conn = database.connect().unwrap();
    for (sql, expected) in [
        ("PRAGMA wal_autocheckpoint = 64", 64_i64),
        ("PRAGMA journal_size_limit = 1048576", 1048576),
    ] {
        assert!(matches!(
            conn.execute(sql, ()).await,
            Err(libsql::Error::ExecuteReturnedRows)
        ));
        let mut rows = conn.query(sql, ()).await.unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            expected
        );
        assert!(rows.next().await.unwrap().is_none());
    }
}

#[tokio::test]
async fn actual_wal_pragma_row_must_reach_eof_before_write_transaction() {
    let temp = private_tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    let database = libsql::Builder::new_local(root.join("owned-wal-row-probe.db"))
        .build()
        .await
        .unwrap();
    let conn = database.connect().unwrap();
    let mut rows = conn.query("PRAGMA journal_mode = WAL", ()).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), "wal");
    conn.execute("BEGIN IMMEDIATE", ()).await.unwrap();
    // BEGIN succeeds; the still-live row prevents the actual COMMIT. Retain the
    // first incorrect BEGIN expectation separately in the failed source/log.
    let commit = conn.execute("COMMIT", ()).await;
    assert!(matches!(commit, Err(libsql::Error::SqliteFailure(code, _)) if code & 255 == 5));
    assert!(!conn.is_autocommit());
    assert!(rows.next().await.unwrap().is_none());
    drop(row);
    drop(rows);
    conn.execute("COMMIT", ()).await.unwrap();
    assert!(conn.is_autocommit());
}

#[cfg(unix)]
#[tokio::test]
async fn actual_default_tempfile_mode_does_not_waive_private_host_root_requirements() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    eprintln!("owned synthetic default tempfile mode = {mode:o}");
    let opened = open(&root, 7, Limits::default()).await;
    if mode & 0o077 != 0 {
        assert_eq!(opened.unwrap_err(), PrivacyError::Integrity);
    } else {
        opened.unwrap();
    }
}

#[tokio::test]
async fn stable_repeated_email_is_committed_and_reopens_with_the_same_opaque_key() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let first_session = session(&vault);
    let text =
        "mail first.owner+case@synthetic.example twice first.owner+case@synthetic.example\r\n";
    let email = "first.owner+case@synthetic.example";
    let first = mask_values(&first_session, text, &[(email, Category::Email)])
        .await
        .unwrap();
    let first_fields = fields(&first);
    assert_eq!(first.replacement_count().unwrap(), 2);
    assert_eq!(first_fields.len(), 1);
    assert_eq!(first_fields[0].1, email);
    assert!(first_fields[0].0.ends_with("@privacy.invalid"));
    assert_eq!(
        first
            .text_for_later_model_request()
            .unwrap()
            .matches(&first_fields[0].0)
            .count(),
        2
    );
    assert!(
        first
            .text_for_later_model_request()
            .unwrap()
            .ends_with("\r\n")
    );
    let probe = Probe::open(&root).await;
    assert_eq!(probe.count().await, 1);
    let second = mask_values(&first_session, email, &[(email, Category::Email)])
        .await
        .unwrap();
    assert_eq!(fields(&second), first_fields);
    drop(probe);
    drop(first);
    drop(second);
    drop(first_session);
    drop(vault);
    let reopened = open(&root, 7, Limits::default()).await.unwrap();
    let restored_session = session(&reopened);
    let restored = mask_values(&restored_session, email, &[(email, Category::Email)])
        .await
        .unwrap();
    assert_eq!(fields(&restored), first_fields);
    let exact = restored_session
        .resolve_exact_aliases(&[&first_fields[0].0])
        .await
        .unwrap();
    let display = exact
        .resolve_exact_display_field(&first_fields[0].0)
        .unwrap();
    assert_eq!(
        display.with_host_display_text(str::to_owned).unwrap(),
        email
    );
}

#[tokio::test]
async fn multiple_categories_preserve_unicode_escaped_bytes_and_phone_layout() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "Second.Owner@synthetic.example";
    let phone = "+1 (202) 555-0148";
    let customer = "客户-🏷️-42";
    let account = "acct-\\\"<&>-99";
    let project = "project-αβ-77";
    let text = format!(
        "α\r\nEmail:{email}; Phone:{phone}; Customer:[{customer}]; Account:[{account}]; Project:{project};🙂"
    );
    let values = [
        (email, Category::Email),
        (phone, Category::Telephone),
        (customer, Category::Identifier(IdentifierKind::Customer)),
        (account, Category::Identifier(IdentifierKind::Account)),
        (project, Category::Identifier(IdentifierKind::Project)),
    ];
    let masked = mask_values(&scoped, &text, &values).await.unwrap();
    let mapped = fields(&masked);
    assert_eq!(mapped.len(), 5);
    let output = masked.text_for_later_model_request().unwrap();
    assert!(output.starts_with("α\r\nEmail:"));
    assert!(output.ends_with(";🙂"));
    for &(value, _) in &values {
        assert!(!output.contains(value));
    }
    let (phone_alias, _, _) = mapped
        .iter()
        .find(|(_, original, _)| original == phone)
        .unwrap();
    assert_eq!(phone_alias.len(), phone.len());
    for (original, alias) in phone.bytes().zip(phone_alias.bytes()) {
        if original.is_ascii_digit() {
            assert!(alias.is_ascii_digit());
        } else {
            assert_eq!(original, alias);
        }
    }
    let records = masked.resolution_records_for_host_fields().unwrap();
    for (alias, original, _) in &mapped {
        let display = records.resolve_exact_display_field(alias).unwrap();
        assert_eq!(
            display.with_host_display_text(str::to_owned).unwrap(),
            *original
        );
    }
    assert_eq!(Probe::open(&root).await.count().await, 5);
}

#[tokio::test]
async fn namespaces_bind_host_project_thread_profile_context_and_version() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let email = "namespace.owner@synthetic.example";
    let names = [
        ["host-a", "project-a", "thread-a", "profile-a", "context-a"],
        ["host-b", "project-a", "thread-a", "profile-a", "context-a"],
        ["host-a", "project-b", "thread-a", "profile-a", "context-a"],
        ["host-a", "project-a", "thread-b", "profile-a", "context-a"],
        ["host-a", "project-a", "thread-a", "profile-b", "context-a"],
        ["host-a", "project-a", "thread-a", "profile-a", "context-b"],
        ["host-a", "project-a", "thread-ab", "c", "context-a"],
        ["host-a", "project-a", "thread-a", "bc", "context-a"],
    ];
    let mut aliases = Vec::new();
    for parts in names {
        let scoped = vault
            .session(
                HostNamespace::supplied_by_host(parts[0], parts[1], parts[2], parts[3], parts[4])
                    .unwrap(),
            )
            .unwrap();
        let masked = mask_values(&scoped, email, &[(email, Category::Email)])
            .await
            .unwrap();
        let alias = fields(&masked)[0].0.clone();
        assert!(!aliases.contains(&alias));
        aliases.push(alias);
    }
    assert_eq!(Probe::open(&root).await.count().await, 8);
}

#[tokio::test]
async fn encrypted_database_wal_shm_indexes_and_model_view_contain_no_plaintext_originals() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let originals = [
        "disk.scan.unique@synthetic.example",
        "+44 7700-901-234",
        "CUSTOMER-PRIVATE-ß-1234",
    ];
    let text = originals.join("\n");
    let masked = mask_values(
        &scoped,
        &text,
        &[
            (originals[0], Category::Email),
            (originals[1], Category::Telephone),
            (originals[2], Category::Identifier(IdentifierKind::Customer)),
        ],
    )
    .await
    .unwrap();
    let probe = Probe::open(&root).await;
    assert_eq!(probe.count().await, 3);
    let model_view =
        serde_json::json!({"text": masked.text_for_later_model_request().unwrap()}).to_string();
    for original in originals {
        assert!(!model_view.contains(original));
    }
    let mut seen_wal = false;
    let mut seen_shm = false;
    for entry in fs::read_dir(&root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        seen_wal |= name.to_str() == Some("privacy-vault.db-wal");
        seen_shm |= name.to_str() == Some("privacy-vault.db-shm");
        let bytes = fs::read(entry.path()).unwrap();
        for original in originals.iter().copied().chain([
            "synthetic-host-33",
            "synthetic-project-11",
            "synthetic-thread-22",
            "profile-55",
            "context-66",
        ]) {
            assert!(
                !bytes
                    .windows(original.len())
                    .any(|window| window == original.as_bytes())
            );
            let utf16: Vec<u8> = original.encode_utf16().flat_map(u16::to_le_bytes).collect();
            assert!(!bytes.windows(utf16.len()).any(|window| window == utf16));
        }
    }
    assert!(seen_wal && seen_shm);
    let mut rows = probe
        .conn
        .query(
            "SELECT scope, original_tag, alias_tag, nonce, sealed FROM vault_mappings",
            (),
        )
        .await
        .unwrap();
    while let Some(row) = rows.next().await.unwrap() {
        for column in 0..5 {
            let bytes: Vec<u8> = row.get(column).unwrap();
            for original in originals {
                assert!(
                    !bytes
                        .windows(original.len())
                        .any(|window| window == original.as_bytes())
                );
            }
        }
    }
    drop(rows);
    drop(probe);
    drop(masked);
    drop(scoped);
    drop(vault);
    // Force a real reopen/checkpoint, then scan the main database too.
    let reopened = open(&root, 7, Limits::default()).await.unwrap();
    let bytes = fs::read(root.join(VAULT_FILENAME)).unwrap();
    for original in originals {
        assert!(
            !bytes
                .windows(original.len())
                .any(|window| window == original.as_bytes())
        );
    }
    drop(reopened);
}

#[tokio::test]
async fn wrong_or_unavailable_host_key_refuses_without_plaintext_diagnostics() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "wrong.key.owner@synthetic.example";
    let masked = mask_values(&scoped, email, &[(email, Category::Email)])
        .await
        .unwrap();
    let wrong = open(&root, 8, Limits::default()).await.unwrap_err();
    assert_eq!(wrong, PrivacyError::Key);
    assert!(!format!("{wrong:?} {wrong}").contains(email));
    assert!(!format!("{wrong:?} {wrong}").contains(root.to_str().unwrap()));
    struct Unavailable;
    impl HostKeySource for Unavailable {
        fn acquire_privacy_key(&self) -> Result<HostKey> {
            Err(PrivacyError::Unsupported)
        }
    }
    let other = private_tempdir();
    let other_root = fs::canonicalize(other.path()).unwrap();
    let error = Vault::open(
        VaultLocation::under_host_config_root(&other_root).unwrap(),
        &Unavailable,
        Limits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error, PrivacyError::Key);
    assert!(!other_root.join(VAULT_FILENAME).exists());
    assert_eq!(fields(&masked)[0].1, email);
    assert_eq!(Probe::open(&root).await.count().await, 1);
}

#[tokio::test]
async fn ciphertext_bit_tamper_and_ciphertext_row_substitution_never_release_plaintext() {
    for substitution in [false, true] {
        let (_temp, root, vault) = fresh(Limits::default()).await;
        let scoped = session(&vault);
        let first = "tamper.first@synthetic.example";
        let second = "tamper.second@synthetic.example";
        let masked = mask_values(
            &scoped,
            &format!("{first} {second}"),
            &[(first, Category::Email), (second, Category::Email)],
        )
        .await
        .unwrap();
        let alias = fields(&masked)[0].0.clone();
        let probe = Probe::open(&root).await;
        let mut sealed = probe.bytes("sealed").await;
        if substitution {
            probe
                .change(
                    "UPDATE vault_mappings SET sealed = ?1 WHERE sealed != ?1",
                    vec![Value::Blob(sealed)],
                )
                .await;
        } else {
            sealed[0] ^= 1;
            probe.change("UPDATE vault_mappings SET sealed = ?1 WHERE original_tag = (SELECT original_tag FROM vault_mappings ORDER BY original_tag LIMIT 1)", vec![Value::Blob(sealed)]).await;
        }
        let error = scoped.resolve_exact_aliases(&[&alias]).await.unwrap_err();
        assert_eq!(error, PrivacyError::Integrity);
        assert!(!format!("{error:?} {error}").contains(first));
        assert!(matches!(
            masked.text_for_later_model_request(),
            Err(PrivacyError::Closed)
        ));
    }
}

#[tokio::test]
async fn scope_alias_name_identity_category_and_version_substitution_refuse() {
    for mutation in 0..5 {
        let (_temp, root, vault) = fresh(Limits::default()).await;
        let original = session(&vault);
        let foreign = vault
            .session(namespace("foreign-project", "foreign-thread"))
            .unwrap();
        let email = "binding.owner@synthetic.example";
        let masked = mask_values(&original, email, &[(email, Category::Email)])
            .await
            .unwrap();
        let alias = fields(&masked)[0].0.clone();
        let probe = Probe::open(&root).await;
        match mutation {
            0 => {
                probe
                    .change(
                        "UPDATE vault_mappings SET scope = ?1",
                        vec![Value::Blob(foreign.live.scope.to_vec())],
                    )
                    .await
            }
            1 => {
                probe
                    .change(
                        "UPDATE vault_mappings SET alias_tag = ?1",
                        vec![Value::Blob(vec![93; 32])],
                    )
                    .await
            }
            2 => {
                probe
                    .change(
                        "UPDATE vault_mappings SET original_tag = ?1",
                        vec![Value::Blob(vec![94; 32])],
                    )
                    .await
            }
            3 => {
                probe
                    .change("UPDATE vault_mappings SET category = 2", vec![])
                    .await
            }
            4 => {
                probe
                    .change("PRAGMA ignore_check_constraints = ON", vec![])
                    .await;
                probe
                    .change("UPDATE vault_mappings SET version = 2", vec![])
                    .await;
            }
            _ => unreachable!(),
        }
        let target = if mutation == 0 { &foreign } else { &original };
        assert_eq!(
            target.resolve_exact_aliases(&[&alias]).await.unwrap_err(),
            PrivacyError::Integrity
        );
        assert!(matches!(
            masked.text_for_later_model_request(),
            Err(PrivacyError::Closed)
        ));
    }
}

#[tokio::test]
async fn metadata_vault_id_version_and_unknown_layout_fail_closed() {
    for mutation in 0..3 {
        let (_temp, root, vault) = fresh(Limits::default()).await;
        let scoped = session(&vault);
        let email = "metadata.owner@synthetic.example";
        let masked = mask_values(&scoped, email, &[(email, Category::Email)])
            .await
            .unwrap();
        let alias = fields(&masked)[0].0.clone();
        let probe = Probe::open(&root).await;
        match mutation {
            0 => {
                probe
                    .change(
                        "UPDATE vault_metadata SET vault_id = ?1",
                        vec![Value::Blob(vec![95; 16])],
                    )
                    .await
            }
            1 => probe.change("PRAGMA user_version = 2", vec![]).await,
            2 => {
                probe
                    .change("CREATE TABLE foreign_table (name TEXT)", vec![])
                    .await
            }
            _ => unreachable!(),
        }
        let error = scoped.resolve_exact_aliases(&[&alias]).await.unwrap_err();
        assert!(matches!(error, PrivacyError::Key | PrivacyError::Integrity));
        assert!(matches!(
            masked.text_for_later_model_request(),
            Err(PrivacyError::Closed)
        ));
    }
}

#[tokio::test]
async fn exact_unknown_altered_malformed_foreign_aliases_are_not_guessed() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "exact.owner@synthetic.example";
    let masked = mask_values(&scoped, email, &[(email, Category::Email)])
        .await
        .unwrap();
    let alias = fields(&masked)[0].0.clone();
    let records = masked.resolution_records_for_host_fields().unwrap();
    for unknown in [
        format!("{alias} "),
        alias.to_uppercase(),
        alias.replace("pv1", "pv2"),
        "pv1-unknown@privacy.invalid".to_owned(),
        "bad@privacy.invalid".to_owned(),
        email.to_owned(),
    ] {
        assert_eq!(
            records.resolve_exact_display_field(&unknown).unwrap_err(),
            PrivacyError::UnknownAlias
        );
    }
    let foreign = vault
        .session(namespace("another-project", "another-thread"))
        .unwrap();
    assert_eq!(
        foreign.resolve_exact_aliases(&[&alias]).await.unwrap_err(),
        PrivacyError::UnknownAlias
    );
    assert_eq!(
        records
            .resolve_exact_display_field(&alias)
            .unwrap()
            .with_host_display_text(str::to_owned)
            .unwrap(),
        email
    );
    assert_eq!(Probe::open(&root).await.count().await, 1);
}

#[tokio::test]
async fn readonly_records_bind_the_exact_session_and_preserve_field_bytes() {
    let (_temp, _root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let identifier = "customer-\\\"<🏷️>-id";
    let masked = mask_values(
        &scoped,
        identifier,
        &[(identifier, Category::Identifier(IdentifierKind::Customer))],
    )
    .await
    .unwrap();
    let records = masked.resolution_records_for_host_fields().unwrap();
    records.validate_session_binding(&scoped).unwrap();
    let fresh_equal = session(&vault);
    assert_eq!(
        records.validate_session_binding(&fresh_equal),
        Err(PrivacyError::ForeignScope)
    );
    let encoded = records
        .with_host_field(0, |field| {
            serde_json::to_string(field.original_for_validated_host_field()).unwrap()
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<String>(&encoded).unwrap(),
        identifier
    );
    assert_eq!(records.with_host_field(1, |_| ()), Err(PrivacyError::Input));
    scoped.close();
    assert_eq!(
        records.with_host_field(0, |_| ()),
        Err(PrivacyError::Closed)
    );
}

#[tokio::test]
async fn generated_input_collision_chooses_another_stable_alias() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "collision.owner@synthetic.example";
    let keys = crypto::Keys::derive(SyntheticKey(7).acquire_privacy_key().unwrap()).unwrap();
    let forbidden = format::generate(&keys, &scoped.live.scope, Category::Email, email, 0);
    let text = format!("original:{email}; literal input:{forbidden}");
    let masked = mask_values(&scoped, &text, &[(email, Category::Email)])
        .await
        .unwrap();
    let alias = fields(&masked)[0].0.clone();
    assert_ne!(alias, forbidden);
    assert!(
        masked
            .text_for_later_model_request()
            .unwrap()
            .contains(&forbidden)
    );
    let again = mask_values(&scoped, email, &[(email, Category::Email)])
        .await
        .unwrap();
    assert_eq!(fields(&again)[0].0, alias);
    assert_eq!(Probe::open(&root).await.count().await, 1);
}

#[tokio::test]
async fn original_input_containing_an_existing_alias_refuses_without_rotating_identity() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "existing.owner@synthetic.example";
    let masked = mask_values(&scoped, email, &[(email, Category::Email)])
        .await
        .unwrap();
    let alias = fields(&masked)[0].0.clone();
    let text = format!("{email} and literal {alias}");
    assert_eq!(
        mask_values(&scoped, &text, &[(email, Category::Email)])
            .await
            .unwrap_err(),
        PrivacyError::Collision
    );
    assert_eq!(Probe::open(&root).await.count().await, 1);
    let reopened = open(&root, 7, Limits::default()).await.unwrap();
    assert_eq!(
        fields(
            &mask_values(&session(&reopened), email, &[(email, Category::Email)])
                .await
                .unwrap()
        )[0]
        .0,
        alias
    );
}

#[tokio::test]
async fn existing_global_alias_collision_avoids_cross_namespace_reuse() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let original = session(&vault);
    let other = vault
        .session(namespace("second-project", "second-thread"))
        .unwrap();
    let first = "global.first@synthetic.example";
    let second = "global.second@synthetic.example";
    let masked = mask_values(&original, first, &[(first, Category::Email)])
        .await
        .unwrap();
    let old_alias = fields(&masked)[0].0.clone();
    let keys = crypto::Keys::derive(SyntheticKey(7).acquire_privacy_key().unwrap()).unwrap();
    let blocked = format::generate(&keys, &other.live.scope, Category::Email, second, 0);
    let tag = keys.alias_tag(&blocked);
    // An actual global unique-index occupant. It remains outside the selected
    // namespace; foreign data is never decrypted to grant a read/restore.
    let probe = Probe::open(&root).await;
    probe
        .change(
            "UPDATE vault_mappings SET alias_tag = ?1",
            vec![Value::Blob(tag.to_vec())],
        )
        .await;
    let new = mask_values(&other, second, &[(second, Category::Email)])
        .await
        .unwrap();
    let alias = fields(&new)[0].0.clone();
    assert_ne!(alias, blocked);
    assert_ne!(alias, old_alias);
    assert_eq!(probe.count().await, 2);
    // Visiting the tampered original namespace refuses rather than returning it.
    assert_eq!(
        original
            .resolve_exact_aliases(&[&old_alias])
            .await
            .unwrap_err(),
        PrivacyError::Integrity
    );
}

#[tokio::test]
async fn hash_offsets_utf8_overlap_duplicate_and_foreign_capture_refuse_whole_input() {
    for case in 0..6 {
        let (_temp, root, vault) = fresh(Limits::default()).await;
        let scoped = session(&vault);
        let text = "🙂 email:span.owner@synthetic.example end";
        let email = "span.owner@synthetic.example";
        let start = text.find(email).unwrap();
        let captured = scoped.capture_text(text).unwrap();
        let span = DetectorSpan {
            start,
            end: start + email.len(),
            category: Category::Email,
        };
        let mut report = report_for(&captured, vec![span]);
        match case {
            0 => report.input_sha256[0] ^= 1,
            1 => report.spans[0].start = 1,
            2 => report.spans[0].end = text.len() + 1,
            3 => report.spans.push(span),
            4 => report.spans.push(DetectorSpan {
                start: start + 1,
                end: start + 4,
                category: Category::Identifier(IdentifierKind::Account),
            }),
            5 => {
                let other = session(&vault);
                assert_eq!(
                    other
                        .mask(captured, report, DeclaredIdentifiers::none())
                        .await
                        .unwrap_err(),
                    PrivacyError::ForeignScope
                );
                assert_eq!(Probe::open(&root).await.count().await, 0);
                continue;
            }
            _ => unreachable!(),
        }
        assert_eq!(
            scoped
                .mask(captured, report, DeclaredIdentifiers::none())
                .await
                .unwrap_err(),
            PrivacyError::Input
        );
        assert_eq!(Probe::open(&root).await.count().await, 0);
        assert_eq!(
            scoped.capture_text("safe").unwrap_err(),
            PrivacyError::Closed
        );
    }
}

#[tokio::test]
async fn explicitly_declared_identifiers_duplicate_declarations_and_ambiguous_identity_refuse() {
    assert_eq!(
        DeclaredIdentifiers::supplied_by_host(&[
            (IdentifierKind::Customer, "ID-1234"),
            (IdentifierKind::Account, "ID-1234")
        ])
        .unwrap_err(),
        PrivacyError::Input
    );
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let identifier = "ID-1234";
    let captured = scoped.capture_text(identifier).unwrap();
    let report = report_for(
        &captured,
        vec![DetectorSpan {
            start: 0,
            end: identifier.len(),
            category: Category::Identifier(IdentifierKind::Customer),
        }],
    );
    assert_eq!(
        scoped
            .mask(captured, report, DeclaredIdentifiers::none())
            .await
            .unwrap_err(),
        PrivacyError::Input
    );
    assert_eq!(Probe::open(&root).await.count().await, 0);
    let scoped = session(&vault);
    let email = "ambiguous.owner@synthetic.example";
    mask_values(&scoped, email, &[(email, Category::Email)])
        .await
        .unwrap();
    assert_eq!(
        mask_values(
            &scoped,
            email,
            &[(email, Category::Identifier(IdentifierKind::Customer))]
        )
        .await
        .unwrap_err(),
        PrivacyError::Input
    );
    assert_eq!(Probe::open(&root).await.count().await, 1);
}

#[tokio::test]
async fn missing_repeated_original_span_refuses_instead_of_a_masked_raw_mix() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "repeat.owner@synthetic.example";
    let text = format!("{email} {email}");
    let captured = scoped.capture_text(&text).unwrap();
    let report = report_for(
        &captured,
        vec![DetectorSpan {
            start: 0,
            end: email.len(),
            category: Category::Email,
        }],
    );
    assert_eq!(
        scoped
            .mask(captured, report, DeclaredIdentifiers::none())
            .await
            .unwrap_err(),
        PrivacyError::Input
    );
    assert_eq!(Probe::open(&root).await.count().await, 0);
}

#[tokio::test]
async fn unsupported_email_phone_and_short_literal_formats_fail_explicitly() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    for (value, category) in [
        ("用户@synthetic.example", Category::Email),
        ("\"quoted\"@synthetic.example", Category::Email),
        ("double..dot@synthetic.example", Category::Email),
        ("owner@localhost", Category::Email),
        ("1234567 ext 89", Category::Telephone),
        ("１２３４５６７８９０", Category::Telephone),
        ("+1 (202 555-0148", Category::Telephone),
        ("123--456-7890", Category::Telephone),
    ] {
        let scoped = session(&vault);
        assert_eq!(
            mask_values(&scoped, value, &[(value, category)])
                .await
                .unwrap_err(),
            PrivacyError::Unsupported
        );
    }
    assert_eq!(
        DeclaredIdentifiers::supplied_by_host(&[(IdentifierKind::Customer, "123")]).unwrap_err(),
        PrivacyError::Unsupported
    );
    assert_eq!(Probe::open(&root).await.count().await, 0);
}

#[tokio::test]
async fn empty_validated_report_is_explicit_and_does_not_claim_detection_coverage() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let text = "Ordinary host text 🦀\r\n";
    let captured = scoped.capture_text(text).unwrap();
    let report = report_for(&captured, vec![]);
    let masked = scoped
        .mask(captured, report, DeclaredIdentifiers::none())
        .await
        .unwrap();
    assert_eq!(masked.replacement_count().unwrap(), 0);
    assert_eq!(masked.text_for_later_model_request().unwrap(), text);
    assert!(
        masked
            .resolution_records_for_host_fields()
            .unwrap()
            .is_empty()
            .unwrap()
    );
    assert_eq!(Probe::open(&root).await.count().await, 0);
}

#[tokio::test]
async fn text_span_scope_ciphertext_work_and_total_mapping_bounds_never_publish_partial_views() {
    for case in 0..6 {
        let mut limits = Limits::default();
        match case {
            0 => limits.text_bytes = 64,
            1 => limits.spans = 1,
            2 => {
                limits.scope_mappings = 1;
                limits.vault_mappings = 2;
            }
            3 => limits.ciphertext_bytes = 128,
            4 => limits.work_bytes = 1024,
            5 => {
                limits.scope_mappings = 1;
                limits.vault_mappings = 1;
            }
            _ => unreachable!(),
        }
        let (_temp, root, vault) = fresh(limits).await;
        let scoped = session(&vault);
        let first = "bounds.first@synthetic.example";
        let second = "bounds.second@synthetic.example";
        let text = if case == 4 {
            format!("{} {first} {second}", "x".repeat(800))
        } else {
            format!("{first} {second}")
        };
        let error = mask_values(
            &scoped,
            &text,
            &[(first, Category::Email), (second, Category::Email)],
        )
        .await
        .unwrap_err();
        assert_eq!(error, PrivacyError::Bounds);
        assert_eq!(Probe::open(&root).await.count().await, 0);
        assert_eq!(
            scoped.capture_text("safe").unwrap_err(),
            PrivacyError::Closed
        );
    }
    let limits = Limits {
        text_bytes: Limits::default().text_bytes + 1,
        ..Limits::default()
    };
    let temp = private_tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    assert_eq!(
        open(&root, 7, limits).await.unwrap_err(),
        PrivacyError::Bounds
    );
    assert!(!root.join(VAULT_FILENAME).exists());
}

#[tokio::test]
async fn oversize_capture_resolution_declarations_and_io_caps_fail_before_raw_fallback() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    assert_eq!(
        scoped
            .capture_text(&"x".repeat(Limits::default().text_bytes + 1))
            .unwrap_err(),
        PrivacyError::Bounds
    );
    let scoped = session(&vault);
    let aliases = vec!["x"; 257];
    assert_eq!(
        scoped.resolve_exact_aliases(&aliases).await.unwrap_err(),
        PrivacyError::Bounds
    );
    let entries: Vec<_> = (0..129).map(|i| format!("CUSTOMER-{i:04}")).collect();
    let declarations: Vec<_> = entries
        .iter()
        .map(|value| (IdentifierKind::Customer, value.as_str()))
        .collect();
    assert_eq!(
        DeclaredIdentifiers::supplied_by_host(&declarations).unwrap_err(),
        PrivacyError::Bounds
    );
    assert_eq!(Probe::open(&root).await.count().await, 0);
    let temp = private_tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    let limits = Limits {
        io_calls: 1,
        ..Limits::default()
    };
    assert_eq!(
        open(&root, 7, limits).await.unwrap_err(),
        PrivacyError::Bounds
    );
}

#[tokio::test]
async fn real_second_insert_constraint_failure_rolls_back_every_new_mapping() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    vault.test_fail_insert(2);
    let scoped = session(&vault);
    let first = "transaction.first@synthetic.example";
    let second = "transaction.second@synthetic.example";
    let error = mask_values(
        &scoped,
        &format!("{first} {second}"),
        &[(first, Category::Email), (second, Category::Email)],
    )
    .await
    .unwrap_err();
    assert_eq!(error, PrivacyError::Io);
    assert_eq!(Probe::open(&root).await.count().await, 0);
    assert!(!format!("{error:?} {error}").contains(first));
    assert_eq!(
        scoped.capture_text("safe").unwrap_err(),
        PrivacyError::Closed
    );
    let reopened = open(&root, 7, Limits::default()).await.unwrap();
    assert_eq!(
        fields(
            &mask_values(&session(&reopened), first, &[(first, Category::Email)])
                .await
                .unwrap()
        )[0]
        .1,
        first
    );
}

#[tokio::test]
async fn actual_sqlite_busy_timeout_refuses_then_a_fresh_session_can_persist() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let probe = Probe::open(&root).await;
    probe.change("BEGIN IMMEDIATE", vec![]).await;
    let scoped = session(&vault);
    let email = "busy.owner@synthetic.example";
    let started = Instant::now();
    let error = mask_values(&scoped, email, &[(email, Category::Email)])
        .await
        .unwrap_err();
    assert_eq!(error, PrivacyError::Busy);
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(probe.count().await, 0);
    probe.change("ROLLBACK", vec![]).await;
    let next = session(&vault);
    mask_values(&next, email, &[(email, Category::Email)])
        .await
        .unwrap();
    assert_eq!(probe.count().await, 1);
}

struct Release {
    state: Mutex<bool>,
    changed: Condvar,
}

impl Release {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(false),
            changed: Condvar::new(),
        })
    }
    fn wait(&self) {
        let mut state = self.state.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !*state {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("owned test release deadline");
            let (next, timed) = self.changed.wait_timeout(state, remaining).unwrap();
            state = next;
            assert!(!timed.timed_out(), "owned test release deadline");
        }
    }
    fn release(&self) {
        *self.state.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

fn pause_io(
    vault: &Vault,
    at: Phase,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Receiver<()>,
    Arc<Release>,
) {
    let (entered_tx, entered) = tokio::sync::oneshot::channel();
    let (finished_tx, finished) = tokio::sync::oneshot::channel();
    let entered_tx = Mutex::new(Some(entered_tx));
    let finished_tx = Mutex::new(Some(finished_tx));
    let release = Release::new();
    let worker_release = release.clone();
    vault.test_hook(Arc::new(move |phase| {
        if phase == at
            && let Some(sender) = entered_tx.lock().unwrap().take()
        {
            sender.send(()).unwrap();
            worker_release.wait();
        }
        if phase == Phase::AfterIoReturn
            && let Some(sender) = finished_tx.lock().unwrap().take()
        {
            let _ = sender.send(());
        }
    }));
    (entered, finished, release)
}

#[tokio::test]
async fn masked_text_waits_for_the_actual_commit_and_concurrent_operations_have_no_queue() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = Arc::new(session(&vault));
    let (entered, finished, release) = pause_io(&vault, Phase::AfterInsert);
    let owned = scoped.clone();
    let email = "commit.owner@synthetic.example";
    let task =
        tokio::spawn(async move { mask_values(&owned, email, &[(email, Category::Email)]).await });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    assert!(!task.is_finished());
    let probe = Probe::open(&root).await;
    assert_eq!(probe.count().await, 0);
    let other = session(&vault);
    assert_eq!(
        other.resolve_exact_aliases(&[]).await.unwrap_err(),
        PrivacyError::Busy
    );
    release.release();
    tokio::time::timeout(Duration::from_secs(5), finished)
        .await
        .unwrap()
        .unwrap();
    let masked = task.await.unwrap().unwrap();
    assert_eq!(fields(&masked)[0].1, email);
    assert_eq!(probe.count().await, 1);
}

#[tokio::test]
async fn dropped_started_write_retains_ownership_and_never_promises_rollback_after_commit() {
    for at in [Phase::AfterBegin, Phase::AfterInsert, Phase::AfterCommit] {
        let (_temp, root, vault) = fresh(Limits::default()).await;
        let scoped = Arc::new(session(&vault));
        let (entered, finished, release) = pause_io(&vault, at);
        let owned = scoped.clone();
        let email = "drop.owner@synthetic.example";
        let task =
            tokio::spawn(
                async move { mask_values(&owned, email, &[(email, Category::Email)]).await },
            );
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        let probe = Probe::open(&root).await;
        assert_eq!(
            probe.count().await,
            if at == Phase::AfterCommit { 1 } else { 0 }
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            scoped.capture_text("safe").unwrap_err(),
            PrivacyError::Closed
        );
        release.release();
        tokio::time::timeout(Duration::from_secs(5), finished)
            .await
            .unwrap()
            .unwrap();
        let retained = probe.count().await;
        assert_eq!(retained, if at == Phase::AfterCommit { 1 } else { 0 });
        drop(probe);
        let reopened = open(&root, 7, Limits::default()).await.unwrap();
        mask_values(&session(&reopened), email, &[(email, Category::Email)])
            .await
            .unwrap();
        assert_eq!(Probe::open(&root).await.count().await, 1);
    }
}

#[tokio::test]
async fn unpolled_mask_future_does_not_admit_or_write_any_mapping() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "unpolled.owner@synthetic.example";
    let captured = scoped.capture_text(email).unwrap();
    let report = report_for(
        &captured,
        vec![DetectorSpan {
            start: 0,
            end: email.len(),
            category: Category::Email,
        }],
    );
    let unpolled = scoped.mask(captured, report, DeclaredIdentifiers::none());
    drop(unpolled);
    assert_eq!(Probe::open(&root).await.count().await, 0);
    assert!(scoped.capture_text("still host data").is_ok());
}

#[tokio::test]
async fn late_elapsed_bound_after_actual_commit_is_an_error_without_a_false_masked_receipt() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    vault.test_hook(Arc::new(|phase| {
        if phase == Phase::AfterCommit {
            std::thread::sleep(Duration::from_millis(5100));
        }
    }));
    let scoped = session(&vault);
    let email = "deadline.owner@synthetic.example";
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        mask_values(&scoped, email, &[(email, Category::Email)]),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error, PrivacyError::Deadline);
    assert_eq!(Probe::open(&root).await.count().await, 1);
    assert_eq!(
        scoped.capture_text("safe").unwrap_err(),
        PrivacyError::Closed
    );
}

#[tokio::test]
async fn private_debug_diagnostics_and_callback_revocation_have_no_original_or_key_fields() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "debug.owner@synthetic.example";
    let captured = scoped.capture_text(email).unwrap();
    let captured_debug = format!("{captured:?}");
    let report = report_for(
        &captured,
        vec![DetectorSpan {
            start: 0,
            end: email.len(),
            category: Category::Email,
        }],
    );
    let report_debug = format!("{report:?}");
    let key = SyntheticKey(7).acquire_privacy_key().unwrap();
    let masked = scoped
        .mask(captured, report, DeclaredIdentifiers::none())
        .await
        .unwrap();
    let records = masked.resolution_records_for_host_fields().unwrap();
    let alias = fields(&masked)[0].0.clone();
    let display = records.resolve_exact_display_field(&alias).unwrap();
    let debug = format!(
        "{vault:?} {scoped:?} {key:?} {masked:?} {records:?} {display:?} {captured_debug} {report_debug}"
    );
    assert!(!debug.contains(email));
    assert!(!debug.contains(&alias));
    assert!(!debug.contains(root.to_str().unwrap()));
    assert!(!debug.contains("[7, 7"));
    assert_eq!(
        display.with_host_display_text(|_| scoped.close()),
        Err(PrivacyError::Closed)
    );
    assert_eq!(
        records.with_host_field(0, |_| ()),
        Err(PrivacyError::Closed)
    );
}

#[tokio::test]
async fn separate_database_never_reads_or_changes_threads_store_files_or_user_configuration() {
    let temp = private_tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    let thread_path = root.join("threads.db");
    let thread_db = libsql::Builder::new_local(&thread_path)
        .build()
        .await
        .unwrap();
    let thread_conn = thread_db.connect().unwrap();
    thread_conn.execute_batch("CREATE TABLE store_files (path TEXT PRIMARY KEY, content TEXT); INSERT INTO store_files VALUES('synthetic', 'preserved'); PRAGMA user_version = 37;").await.unwrap();
    let config = b"{\"synthetic_unknown\":\"preserved\"}\n";
    fs::write(root.join("config.json"), config).unwrap();
    let before = fs::read(&thread_path).unwrap();
    let vault = open(&root, 7, Limits::default()).await.unwrap();
    let scoped = session(&vault);
    let email = "isolated.owner@synthetic.example";
    mask_values(&scoped, email, &[(email, Category::Email)])
        .await
        .unwrap();
    assert_eq!(fs::read(&thread_path).unwrap(), before);
    assert_eq!(fs::read(root.join("config.json")).unwrap(), config);
    let mut version = thread_conn.query("PRAGMA user_version", ()).await.unwrap();
    assert_eq!(
        version
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        37
    );
    assert_eq!(Probe::open(&root).await.count().await, 1);
}

#[tokio::test]
async fn a_foreign_layout_at_the_reserved_vault_name_is_refused_without_migration() {
    let temp = private_tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    let path = root.join(VAULT_FILENAME);
    let database = libsql::Builder::new_local(&path).build().await.unwrap();
    let conn = database.connect().unwrap();
    conn.execute_batch("CREATE TABLE store_files(path TEXT, content TEXT); INSERT INTO store_files VALUES('private', 'foreign synthetic data'); PRAGMA user_version = 37;").await.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let before = fs::read(&path).unwrap();
    assert_eq!(
        open(&root, 7, Limits::default()).await.unwrap_err(),
        PrivacyError::Integrity
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(!root.join("privacy-vault.db-wal").exists());
}

fn private_owned_file(path: &Path, bytes: &[u8]) {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[tokio::test]
async fn preexisting_zero_byte_and_unmarked_empty_sqlite_files_are_preserved_without_adoption() {
    for initialized in [false, true] {
        let temp = private_tempdir();
        let root = fs::canonicalize(temp.path()).unwrap();
        let path = root.join(VAULT_FILENAME);
        private_owned_file(&path, b"");
        if initialized {
            let database = libsql::Builder::new_local(&path).build().await.unwrap();
            let conn = database.connect().unwrap();
            conn.execute("VACUUM", ()).await.unwrap();
            let mut rows = conn
                .query("SELECT count(*) FROM sqlite_master", ())
                .await
                .unwrap();
            assert_eq!(
                rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
                0
            );
            assert!(rows.next().await.unwrap().is_none());
            drop(rows);
            drop(conn);
            drop(database);
            assert!(fs::metadata(&path).unwrap().len() > 0);
        }
        let before = fs::read(&path).unwrap();
        assert_eq!(
            open(&root, 7, Limits::default()).await.unwrap_err(),
            PrivacyError::Integrity
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }
}

#[tokio::test]
async fn orphan_wal_shm_and_journal_files_are_preserved_without_creating_or_adopting_a_base() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let temp = private_tempdir();
        let root = fs::canonicalize(temp.path()).unwrap();
        let sidecar = root.join(format!("{VAULT_FILENAME}{suffix}"));
        let original = b"preserve owned synthetic foreign sidecar";
        private_owned_file(&sidecar, original);
        assert_eq!(
            open(&root, 7, Limits::default()).await.unwrap_err(),
            PrivacyError::Integrity
        );
        assert_eq!(fs::read(&sidecar).unwrap(), original);
        assert!(!root.join(VAULT_FILENAME).exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }
}

#[tokio::test]
async fn interrupted_owned_initialization_never_authorizes_adoption_of_its_unmarked_remainder() {
    for at in [
        InitPhase::AfterCreate,
        InitPhase::BeforeCommit,
        InitPhase::AfterCommit,
    ] {
        let temp = private_tempdir();
        let root = fs::canonicalize(temp.path()).unwrap();
        let (entered_tx, entered) = tokio::sync::oneshot::channel();
        let (finished_tx, finished) = tokio::sync::oneshot::channel();
        let entered_tx = Mutex::new(Some(entered_tx));
        let finished_tx = Mutex::new(Some(finished_tx));
        let release = Release::new();
        let worker_release = release.clone();
        let hook = Arc::new(move |phase| {
            if phase == at
                && let Some(sender) = entered_tx.lock().unwrap().take()
            {
                sender.send(()).unwrap();
                worker_release.wait();
            }
            if phase == InitPhase::AfterIoReturn
                && let Some(sender) = finished_tx.lock().unwrap().take()
            {
                let _ = sender.send(());
            }
        });
        let location = VaultLocation::under_host_config_root(&root).unwrap();
        let task = tokio::spawn(async move {
            Vault::test_open_with_hook(location, &SyntheticKey(7), Limits::default(), hook).await
        });
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        assert!(!task.is_finished());
        assert!(root.join(VAULT_FILENAME).exists());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        release.release();
        tokio::time::timeout(Duration::from_secs(5), finished)
            .await
            .unwrap()
            .unwrap();
        let before = fs::read(root.join(VAULT_FILENAME)).unwrap();
        if at == InitPhase::AfterCommit {
            // Actual key-authenticated schema commit survived the dropped wait.
            let reopened = open(&root, 7, Limits::default()).await.unwrap();
            assert_eq!(Probe::open(&root).await.count().await, 0);
            drop(reopened);
        } else {
            assert_eq!(
                open(&root, 7, Limits::default()).await.unwrap_err(),
                PrivacyError::Integrity
            );
            assert_eq!(fs::read(root.join(VAULT_FILENAME)).unwrap(), before);
            let database = libsql::Builder::new_local(root.join(VAULT_FILENAME))
                .build()
                .await
                .unwrap();
            let conn = database.connect().unwrap();
            let mut rows = conn
                .query(
                    "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                    (),
                )
                .await
                .unwrap();
            assert_eq!(
                rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
                0
            );
            assert!(rows.next().await.unwrap().is_none());
            drop(rows);
            let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
            assert_eq!(
                rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
                0
            );
        }
    }
}

#[tokio::test]
async fn exhausting_every_input_collision_candidate_refuses_the_entire_batch() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "exhausted.owner@synthetic.example";
    let keys = crypto::Keys::derive(SyntheticKey(7).acquire_privacy_key().unwrap()).unwrap();
    let candidates = (0..format::ALIAS_ATTEMPTS)
        .map(|counter| format::generate(&keys, &scoped.live.scope, Category::Email, email, counter))
        .collect::<Vec<_>>()
        .join(" ");
    let text = format!("{email} {candidates}");
    assert_eq!(
        mask_values(&scoped, &text, &[(email, Category::Email)])
            .await
            .unwrap_err(),
        PrivacyError::Collision
    );
    assert_eq!(Probe::open(&root).await.count().await, 0);
}

#[tokio::test]
async fn repeated_phone_layouts_and_exact_literal_identities_remain_stable() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    for phone in [
        "2025550123",
        "+886 912-345-678",
        "(202) 555-0123",
        "7700.901.234",
    ] {
        let text = format!("{phone} again {phone}");
        let masked = mask_values(&scoped, &text, &[(phone, Category::Telephone)])
            .await
            .unwrap();
        let mapping = fields(&masked);
        assert_eq!(masked.replacement_count().unwrap(), 2);
        assert_eq!(mapping.len(), 1);
        let repeated = mask_values(&scoped, phone, &[(phone, Category::Telephone)])
            .await
            .unwrap();
        assert_eq!(fields(&repeated), mapping);
        assert_eq!(mapping[0].0.len(), phone.len());
    }
    let identifier = "ACCOUNT-exact-1234";
    let masked = mask_values(
        &scoped,
        &format!("{identifier} {identifier}"),
        &[(identifier, Category::Identifier(IdentifierKind::Account))],
    )
    .await
    .unwrap();
    let mapping = fields(&masked);
    assert_eq!(mapping.len(), 1);
    assert_eq!(masked.replacement_count().unwrap(), 2);
    let repeated = mask_values(
        &scoped,
        identifier,
        &[(identifier, Category::Identifier(IdentifierKind::Account))],
    )
    .await
    .unwrap();
    assert_eq!(fields(&repeated), mapping);
    assert_eq!(Probe::open(&root).await.count().await, 5);
}

#[tokio::test]
async fn global_mapping_bound_retains_an_earlier_committed_namespace_without_partial_new_records() {
    let limits = Limits {
        scope_mappings: 1,
        vault_mappings: 1,
        ..Limits::default()
    };
    let (_temp, root, vault) = fresh(limits).await;
    let original = session(&vault);
    let first = "global.bound.first@synthetic.example";
    let masked = mask_values(&original, first, &[(first, Category::Email)])
        .await
        .unwrap();
    let other = vault
        .session(namespace("different-project", "different-thread"))
        .unwrap();
    let second = "global.bound.second@synthetic.example";
    assert_eq!(
        mask_values(&other, second, &[(second, Category::Email)])
            .await
            .unwrap_err(),
        PrivacyError::Bounds
    );
    assert_eq!(fields(&masked)[0].1, first);
    assert_eq!(Probe::open(&root).await.count().await, 1);
}

#[tokio::test]
async fn duplicate_resolution_aliases_refuse_before_exposing_a_partial_map() {
    let (_temp, root, vault) = fresh(Limits::default()).await;
    let scoped = session(&vault);
    let email = "duplicate.resolve@synthetic.example";
    let masked = mask_values(&scoped, email, &[(email, Category::Email)])
        .await
        .unwrap();
    let alias = fields(&masked)[0].0.clone();
    assert_eq!(
        scoped
            .resolve_exact_aliases(&[&alias, &alias])
            .await
            .unwrap_err(),
        PrivacyError::Input
    );
    assert_eq!(Probe::open(&root).await.count().await, 1);
    assert_eq!(
        masked.text_for_later_model_request(),
        Err(PrivacyError::Closed)
    );
}

#[test]
fn missing_tokio_host_runtime_fails_fixed_without_opening_the_vault() {
    use futures_util::FutureExt;
    let temp = private_tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    let result = open(&root, 7, Limits::default()).now_or_never().unwrap();
    assert_eq!(result.unwrap_err(), PrivacyError::Worker);
    assert!(!root.join(VAULT_FILENAME).exists());
}

#[tokio::test]
async fn root_depth_file_size_and_plaintext_host_key_persistence_have_no_fallback() {
    assert_eq!(
        VaultLocation::under_host_config_root(Path::new("relative/root")).unwrap_err(),
        PrivacyError::Input
    );
    let too_deep = PathBuf::from(format!(
        "/{}",
        std::iter::repeat_n("synthetic", 129)
            .collect::<Vec<_>>()
            .join("/")
    ));
    assert_eq!(
        VaultLocation::under_host_config_root(&too_deep).unwrap_err(),
        PrivacyError::Input
    );
    let temp = private_tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    let path = root.join(VAULT_FILENAME);
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    file.set_len(32 * 1024 * 1024 + 1).unwrap();
    assert_eq!(
        open(&root, 7, Limits::default()).await.unwrap_err(),
        PrivacyError::Integrity
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), 32 * 1024 * 1024 + 1);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_hardlink_and_nonprivate_root_or_file_refuse_without_touching_foreign_targets() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for case in 0..4 {
        let temp = private_tempdir();
        let root = fs::canonicalize(temp.path()).unwrap();
        let foreign = root.join("foreign.bin");
        fs::write(&foreign, b"preserve synthetic foreign bytes").unwrap();
        fs::set_permissions(&foreign, fs::Permissions::from_mode(0o600)).unwrap();
        let path = root.join(VAULT_FILENAME);
        match case {
            0 => symlink(&foreign, &path).unwrap(),
            1 => fs::hard_link(&foreign, &path).unwrap(),
            2 => {
                fs::write(&path, b"foreign").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            }
            3 => fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(
            open(&root, 7, Limits::default()).await.unwrap_err(),
            PrivacyError::Integrity
        );
        assert_eq!(
            fs::read(&foreign).unwrap(),
            b"preserve synthetic foreign bytes"
        );
    }
}
