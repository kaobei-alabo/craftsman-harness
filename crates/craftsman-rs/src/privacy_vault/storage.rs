//! A new local-only libSQL database. SQL never receives plaintext host values.

use super::crypto::{self, Keys, Sealed};
use super::format::{self, Selected};
use super::{
    CapturedText, Category, DeclaredIdentifiers, DetectorReport, FORMAT_VERSION, HostKey,
    HostKeySource, HostNamespace, Limits, Mapping, MappingSession, MaskedText, PrivacyError,
    ResolutionRecords, Result, SessionLife, VAULT_FILENAME, VaultLocation,
};
use libsql::{Connection, Value};
use std::fmt;
#[cfg(unix)]
use std::fs::Metadata;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zeroize::Zeroizing;

#[cfg(test)]
#[path = "open_observation.rs"]
mod open_observation;

#[cfg(all(test, windows))]
mod windows_tests;

const APPLICATION_ID: i64 = 0x43505631;
const DB_BYTES: u64 = 32 * 1024 * 1024;
const WAL_BYTES: u64 = 16 * 1024 * 1024;
const HEADER: &[u8] = b"craftsman/private-privacy-vault/host-key-check/v1";
const METADATA_SQL: &str = "CREATE TABLE vault_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    version INTEGER NOT NULL CHECK(version = 1),
    vault_id BLOB NOT NULL CHECK(length(vault_id) = 16),
    nonce BLOB NOT NULL CHECK(length(nonce) = 12),
    sealed BLOB NOT NULL CHECK(length(sealed) BETWEEN 16 AND 512)
)";
const MAPPINGS_SQL: &str = "CREATE TABLE vault_mappings (
    scope BLOB NOT NULL CHECK(length(scope) = 32),
    original_tag BLOB NOT NULL CHECK(length(original_tag) = 32),
    alias_tag BLOB NOT NULL UNIQUE CHECK(length(alias_tag) = 32),
    category INTEGER NOT NULL CHECK(category BETWEEN 1 AND 5),
    version INTEGER NOT NULL CHECK(version = 1),
    nonce BLOB NOT NULL CHECK(length(nonce) = 12),
    sealed BLOB NOT NULL CHECK(length(sealed) BETWEEN 16 AND 1024),
    PRIMARY KEY(scope, category, original_tag)
) WITHOUT ROWID";

pub struct Vault {
    pub(super) inner: Arc<Inner>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Vault([private])")
    }
}

impl Vault {
    pub async fn open(
        location: VaultLocation,
        source: &dyn HostKeySource,
        limits: Limits,
    ) -> Result<Self> {
        let started = Instant::now();
        let limits = limits.validate()?;
        let key = source
            .acquire_privacy_key()
            .map_err(|_| PrivacyError::Key)?;
        Self::open_key(
            location,
            key,
            limits,
            started,
            #[cfg(test)]
            None,
        )
        .await
    }

    async fn open_key(
        location: VaultLocation,
        key: HostKey,
        limits: Limits,
        started: Instant,
        #[cfg(test)] hook: Option<InitHook>,
    ) -> Result<Self> {
        #[cfg(test)]
        let (observation, _observation_waiter) = open_observation::Opening::new(started);
        let open = Arc::new(AtomicBool::new(true));
        let mut guard = OpenGuard {
            open: open.clone(),
            accepted: false,
        };
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| PrivacyError::Worker)?;
        #[cfg(test)]
        observation.pre_dispatch();
        let opened = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            observation.worker_entry();
            let result = runtime.block_on(open_worker(
                location,
                key,
                limits,
                &open,
                started,
                #[cfg(test)]
                hook.as_ref(),
                #[cfg(test)]
                observation.clone(),
            ));
            #[cfg(test)]
            if let Some(hook) = hook {
                hook(InitPhase::AfterIoReturn);
            }
            #[cfg(test)]
            observation.finish(result.as_ref().err().copied());
            result
        })
        .await
        .map_err(|_| PrivacyError::Worker)??;
        guard.accepted = true;
        Ok(Self {
            inner: Arc::new(opened),
        })
    }

    pub fn session(&self, namespace: HostNamespace) -> Result<MappingSession> {
        self.inner.check()?;
        let scope = self.inner.keys.scope(&self.inner.id, &namespace);
        Ok(MappingSession {
            live: Arc::new(SessionLife {
                open: AtomicBool::new(true),
                vault: self.inner.clone(),
                scope,
            }),
        })
    }

    /// Denies new work; does not interrupt/await started native IO or assert
    /// rollback/quiescence. Each started worker retains its DB/key through return.
    pub fn close(&self) {
        self.inner.healthy.store(false, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn test_hook(&self, hook: Hook) {
        *self.inner.hook.lock().expect("private test observer") = Some(hook);
    }

    #[cfg(test)]
    pub(super) fn test_fail_insert(&self, index: usize) {
        *self
            .inner
            .fail_insert_at
            .lock()
            .expect("private test SQL failure") = Some(index);
    }

    #[cfg(test)]
    pub(super) async fn test_open_with_hook(
        location: VaultLocation,
        source: &dyn HostKeySource,
        limits: Limits,
        hook: InitHook,
    ) -> Result<Self> {
        let started = Instant::now();
        let key = source
            .acquire_privacy_key()
            .map_err(|_| PrivacyError::Key)?;
        Self::open_key(location, key, limits.validate()?, started, Some(hook)).await
    }
}

struct OpenGuard {
    open: Arc<AtomicBool>,
    accepted: bool,
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        if !self.accepted {
            self.open.store(false, Ordering::Release);
        }
    }
}

pub(super) struct Inner {
    // Connections precede their database in field drop order. No connection is
    // cloned per operation, and the Database outlives its one local connection.
    conn: Mutex<Connection>,
    _database: libsql::Database,
    keys: Keys,
    id: [u8; 16],
    native: NativeLocation,
    pub(super) limits: Limits,
    healthy: AtomicBool,
    active: AtomicBool,
    #[cfg(test)]
    hook: Mutex<Option<Hook>>,
    #[cfg(test)]
    fail_insert_at: Mutex<Option<usize>>,
}

impl Inner {
    pub(super) fn check(&self) -> Result<()> {
        if !self.healthy.load(Ordering::Acquire) {
            return Err(PrivacyError::Closed);
        }
        Ok(())
    }

    fn fail_storage(&self, error: PrivacyError) {
        if matches!(
            error,
            PrivacyError::Integrity | PrivacyError::Io | PrivacyError::Worker | PrivacyError::Key
        ) {
            self.healthy.store(false, Ordering::Release);
        }
    }

    #[cfg(test)]
    fn observe(&self, phase: Phase) {
        let hook = self.hook.lock().expect("private test observer").clone();
        if let Some(hook) = hook {
            hook(phase);
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    AfterBegin,
    AfterInsert,
    AfterCommit,
    AfterIoReturn,
}

#[cfg(test)]
type Hook = Arc<dyn Fn(Phase) + Send + Sync>;

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum InitPhase {
    AfterCreate,
    BeforeCommit,
    AfterCommit,
    AfterIoReturn,
}

#[cfg(test)]
type InitHook = Arc<dyn Fn(InitPhase) + Send + Sync>;

/// There is one owned operation slot and no pending work queue per Vault.
struct OwnedSlot {
    inner: Arc<Inner>,
    returned: bool,
}

impl OwnedSlot {
    fn reserve(inner: &Arc<Inner>) -> Result<Self> {
        inner.check()?;
        if inner
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(PrivacyError::Busy);
        }
        Ok(Self {
            inner: inner.clone(),
            returned: false,
        })
    }
}

impl Drop for OwnedSlot {
    fn drop(&mut self) {
        if !self.returned {
            self.inner.healthy.store(false, Ordering::Release);
        }
        self.inner.active.store(false, Ordering::Release);
    }
}

struct RequestGuard {
    live: Arc<SessionLife>,
    accepted: bool,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if !self.accepted {
            self.live.close();
        }
    }
}

pub(super) struct Budget {
    pub(super) limits: Limits,
    started: Instant,
    calls: usize,
    work: usize,
    live: Option<Arc<SessionLife>>,
    #[cfg(test)]
    opening: Option<Arc<open_observation::Opening>>,
}

impl Budget {
    fn new(limits: Limits, live: Option<Arc<SessionLife>>) -> Self {
        Self {
            limits,
            started: Instant::now(),
            calls: 0,
            work: 0,
            live,
            #[cfg(test)]
            opening: None,
        }
    }

    fn check(&self) -> Result<()> {
        if let Some(live) = &self.live {
            live.check()?;
        }
        if self.started.elapsed() > self.limits.elapsed {
            #[cfg(test)]
            if let Some(opening) = &self.opening {
                opening.deadline(self.started.elapsed(), self.calls, self.work);
            }
            return Err(PrivacyError::Deadline);
        }
        Ok(())
    }

    #[cfg(test)]
    fn opening_enter(&self, phase: open_observation::Phase) {
        if let Some(opening) = &self.opening {
            opening.enter(phase);
        }
    }

    #[cfg(test)]
    fn opening_returned(&self) {
        if let Some(opening) = &self.opening {
            opening.returned();
        }
    }

    pub(super) fn work(&mut self, bytes: usize) -> Result<()> {
        self.check()?;
        self.work = self.work.checked_add(bytes).ok_or(PrivacyError::Bounds)?;
        if self.work > self.limits.work_bytes {
            return Err(PrivacyError::Bounds);
        }
        Ok(())
    }

    fn io(&mut self) -> Result<()> {
        self.check()?;
        // Reserve one final rollback call, even after stop/deadline/cap refusal.
        if self.calls >= self.limits.io_calls.saturating_sub(1) {
            return Err(PrivacyError::Bounds);
        }
        self.calls += 1;
        Ok(())
    }

    fn observed<T>(&self, value: libsql::Result<T>) -> Result<T> {
        self.check()?;
        value.map_err(sql_error)
    }
}

fn sql_error(error: libsql::Error) -> PrivacyError {
    match error {
        libsql::Error::SqliteFailure(code, _) if matches!(code & 255, 5 | 6) => PrivacyError::Busy,
        _ => PrivacyError::Io,
    }
}

async fn execute(
    conn: &Connection,
    sql: &str,
    params: Vec<Value>,
    budget: &mut Budget,
) -> Result<()> {
    budget.io()?;
    let result = conn.execute(sql, params).await;
    budget.observed(result).map(|_| ())
}

async fn query(
    conn: &Connection,
    sql: &str,
    params: Vec<Value>,
    budget: &mut Budget,
) -> Result<libsql::Rows> {
    budget.io()?;
    let result = conn.query(sql, params).await;
    budget.observed(result)
}

async fn next(rows: &mut libsql::Rows, budget: &mut Budget) -> Result<Option<libsql::Row>> {
    budget.io()?;
    let result = rows.next().await;
    budget.observed(result)
}

async fn integer(conn: &Connection, sql: &str, budget: &mut Budget) -> Result<i64> {
    let mut rows = query(conn, sql, vec![], budget).await?;
    let row = next(&mut rows, budget)
        .await?
        .ok_or(PrivacyError::Integrity)?;
    let value = row.get(0).map_err(|_| PrivacyError::Integrity)?;
    if next(&mut rows, budget).await?.is_some() {
        return Err(PrivacyError::Integrity);
    }
    Ok(value)
}

/// Actual rollback is awaited once, without a timeout/drop wrapper. An error
/// refuses; a dropped waiter or failed cleanup is never described as rollback.
async fn rollback(conn: &Connection, budget: &mut Budget) -> Result<()> {
    budget.calls += 1;
    if budget.calls > budget.limits.io_calls {
        return Err(PrivacyError::Bounds);
    }
    conn.execute("ROLLBACK", ())
        .await
        .map(|_| ())
        .map_err(sql_error)
}

async fn begin(conn: &Connection, sql: &str, budget: &mut Budget) -> Result<()> {
    // A late close/deadline may follow a successful actual BEGIN. Do not strand
    // that transaction by returning before its actual rollback is awaited.
    budget.io()?;
    let returned = conn.execute(sql, ()).await;
    #[cfg(test)]
    if returned.is_ok()
        && let Some(live) = &budget.live
    {
        live.vault.observe(Phase::AfterBegin);
    }
    if let Err(error) = budget.observed(returned) {
        if !conn.is_autocommit() {
            rollback(conn, budget).await?;
        }
        return Err(error);
    }
    Ok(())
}

async fn open_worker(
    location: VaultLocation,
    key: HostKey,
    limits: Limits,
    open: &AtomicBool,
    started: Instant,
    #[cfg(test)] hook: Option<&InitHook>,
    #[cfg(test)] observation: Arc<open_observation::Opening>,
) -> Result<Inner> {
    let mut budget = Budget::new(limits, None);
    budget.started = started;
    #[cfg(test)]
    {
        budget.opening = Some(observation);
        budget.opening_enter(open_observation::Phase::NativeCapture);
    }
    let native = NativeLocation::capture(location, &mut budget)?;
    #[cfg(test)]
    budget.opening_returned();
    #[cfg(test)]
    if native.created
        && let Some(hook) = hook
    {
        hook(InitPhase::AfterCreate);
    }
    if !open.load(Ordering::Acquire) {
        return Err(PrivacyError::Closed);
    }
    #[cfg(test)]
    budget.opening_enter(open_observation::Phase::KeyDerivation);
    let keys = Keys::derive(key)?;
    #[cfg(test)]
    {
        budget.opening_returned();
        budget.opening_enter(open_observation::Phase::DatabaseBuild);
    }
    budget.io()?;
    let database = libsql::Builder::new_local(&native.path)
        .build()
        .await
        .map_err(sql_error)?;
    #[cfg(test)]
    budget.opening_returned();
    budget.check()?;
    #[cfg(test)]
    budget.opening_enter(open_observation::Phase::ConnectionSetup);
    budget.io()?;
    let conn = database.connect().map_err(sql_error)?;
    budget.io()?;
    conn.busy_timeout(limits.busy_timeout).map_err(sql_error)?;
    #[cfg(test)]
    {
        budget.opening_returned();
        budget.opening_enter(open_observation::Phase::InitialLayout);
    }
    let table_count = integer(
        &conn,
        "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        &mut budget,
    )
    .await?;
    let version = integer(&conn, "PRAGMA user_version", &mut budget).await?;
    let application = integer(&conn, "PRAGMA application_id", &mut budget).await?;
    let fresh = table_count == 0 && version == 0 && application == 0;
    if fresh && !native.created {
        // A reserved name/Unix owner alone is not evidence that this operation
        // created or may adopt an unmarked empty database. Failed fresh init may
        // leave such a file; there is deliberately no automatic recovery/adopt.
        return Err(PrivacyError::Integrity);
    }
    if !fresh
        && (table_count != 2 || version != FORMAT_VERSION as i64 || application != APPLICATION_ID)
    {
        return Err(PrivacyError::Integrity);
    }
    // Foreign/threads/store layouts were inspected and refused before any WAL,
    // schema or version change. There is no migration of a different database.
    if !fresh {
        validate_layout(&conn, &mut budget).await?;
        // Reject a wrong supplied key before altering even connection pragmas.
        read_header(&conn, &keys, &mut budget).await?;
    }
    #[cfg(test)]
    {
        budget.opening_returned();
        budget.opening_enter(open_observation::Phase::WalSetup);
    }
    let mut mode = query(&conn, "PRAGMA journal_mode = WAL", vec![], &mut budget).await?;
    let row = next(&mut mode, &mut budget)
        .await?
        .ok_or(PrivacyError::Integrity)?;
    if row.get::<String>(0).map_err(|_| PrivacyError::Integrity)? != "wal" {
        return Err(PrivacyError::Integrity);
    }
    drop(row);
    if next(&mut mode, &mut budget).await?.is_some() {
        return Err(PrivacyError::Integrity);
    }
    drop(mode);
    #[cfg(test)]
    {
        budget.opening_returned();
        budget.opening_enter(open_observation::Phase::Pragmas);
    }
    execute(&conn, "PRAGMA synchronous = FULL", vec![], &mut budget).await?;
    execute(&conn, "PRAGMA temp_store = MEMORY", vec![], &mut budget).await?;
    if integer(&conn, "PRAGMA wal_autocheckpoint = 64", &mut budget).await? != 64
        || integer(&conn, "PRAGMA journal_size_limit = 1048576", &mut budget).await? != 1048576
        || integer(&conn, "PRAGMA max_page_count = 8192", &mut budget).await? > 8192
        || integer(&conn, "PRAGMA page_size", &mut budget).await? != 4096
        || integer(&conn, "PRAGMA synchronous", &mut budget).await? != 2
        || integer(&conn, "PRAGMA temp_store", &mut budget).await? != 2
    {
        return Err(PrivacyError::Integrity);
    }
    #[cfg(test)]
    budget.opening_returned();
    let mut id = [0; 16];
    if fresh {
        #[cfg(test)]
        budget.opening_enter(open_observation::Phase::FreshTransaction);
        getrandom::fill(&mut id).map_err(|_| PrivacyError::Key)?;
        let sealed = keys.seal(&crypto::header_aad(&id), Zeroizing::new(HEADER.to_vec()))?;
        begin(&conn, "BEGIN IMMEDIATE", &mut budget).await?;
        let created = async {
            // Another host/process may have won initialization while we waited.
            if integer(
                &conn,
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                &mut budget,
            )
            .await?
                != 0
            {
                return Err(PrivacyError::Busy);
            }
            execute(&conn, METADATA_SQL, vec![], &mut budget).await?;
            execute(&conn, MAPPINGS_SQL, vec![], &mut budget).await?;
            execute(
                &conn,
                "INSERT INTO vault_metadata VALUES(1, 1, ?1, ?2, ?3)",
                vec![
                    Value::Blob(id.to_vec()),
                    Value::Blob(sealed.nonce.to_vec()),
                    Value::Blob(sealed.bytes),
                ],
                &mut budget,
            )
            .await?;
            execute(&conn, "PRAGMA user_version = 1", vec![], &mut budget).await?;
            execute(
                &conn,
                "PRAGMA application_id = 1129338417",
                vec![],
                &mut budget,
            )
            .await?;
            #[cfg(test)]
            if let Some(hook) = hook {
                hook(InitPhase::BeforeCommit);
            }
            if !open.load(Ordering::Acquire) {
                return Err(PrivacyError::Closed);
            }
            native.recheck(&mut budget)?;
            Ok(())
        }
        .await;
        #[cfg(test)]
        if created.is_ok() {
            budget.opening_returned();
        }
        match created {
            Ok(()) => {
                #[cfg(test)]
                budget.opening_enter(open_observation::Phase::Commit);
                if let Err(error) = execute(&conn, "COMMIT", vec![], &mut budget).await {
                    if !conn.is_autocommit() {
                        rollback(&conn, &mut budget).await?;
                    }
                    return Err(error);
                }
                #[cfg(test)]
                budget.opening_returned();
                #[cfg(test)]
                if let Some(hook) = hook {
                    hook(InitPhase::AfterCommit);
                }
            }
            Err(error) => {
                rollback(&conn, &mut budget).await?;
                return Err(error);
            }
        }
    }
    #[cfg(test)]
    budget.opening_enter(open_observation::Phase::PostValidation);
    validate_layout(&conn, &mut budget).await?;
    let read_id = read_header(&conn, &keys, &mut budget).await?;
    if fresh && read_id != id {
        return Err(PrivacyError::Integrity);
    }
    #[cfg(test)]
    {
        budget.opening_returned();
        budget.opening_enter(open_observation::Phase::NativeReturn);
    }
    native.recheck(&mut budget)?;
    if !open.load(Ordering::Acquire) {
        return Err(PrivacyError::Closed);
    }
    budget.check()?;
    #[cfg(test)]
    budget.opening_returned();
    Ok(Inner {
        conn: Mutex::new(conn),
        _database: database,
        keys,
        id: read_id,
        native,
        limits,
        healthy: AtomicBool::new(true),
        active: AtomicBool::new(false),
        #[cfg(test)]
        hook: Mutex::new(None),
        #[cfg(test)]
        fail_insert_at: Mutex::new(None),
    })
}

async fn validate_layout(conn: &Connection, budget: &mut Budget) -> Result<()> {
    if integer(conn, "PRAGMA user_version", budget).await? != FORMAT_VERSION as i64
        || integer(conn, "PRAGMA application_id", budget).await? != APPLICATION_ID
        || integer(
            conn,
            "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            budget,
        )
        .await?
            != 2
    {
        return Err(PrivacyError::Integrity);
    }
    let mut rows = query(conn, "SELECT name, sql FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name", vec![], budget).await?;
    for (name, sql) in [
        ("vault_mappings", MAPPINGS_SQL),
        ("vault_metadata", METADATA_SQL),
    ] {
        let row = next(&mut rows, budget)
            .await?
            .ok_or(PrivacyError::Integrity)?;
        if row.get::<String>(0).map_err(|_| PrivacyError::Integrity)? != name
            || row.get::<String>(1).map_err(|_| PrivacyError::Integrity)? != sql
        {
            return Err(PrivacyError::Integrity);
        }
    }
    if next(&mut rows, budget).await?.is_some() {
        return Err(PrivacyError::Integrity);
    }
    Ok(())
}

async fn read_header(conn: &Connection, keys: &Keys, budget: &mut Budget) -> Result<[u8; 16]> {
    let mut rows = query(conn, "SELECT singleton, version, length(vault_id), length(nonce), length(sealed), vault_id, nonce, sealed FROM vault_metadata", vec![], budget).await?;
    let row = next(&mut rows, budget)
        .await?
        .ok_or(PrivacyError::Integrity)?;
    if row.get::<i64>(0).map_err(|_| PrivacyError::Integrity)? != 1
        || row.get::<i64>(1).map_err(|_| PrivacyError::Integrity)? != FORMAT_VERSION as i64
        || row.get::<i64>(2).map_err(|_| PrivacyError::Integrity)? != 16
        || row.get::<i64>(3).map_err(|_| PrivacyError::Integrity)? != 12
        || !(16..=512).contains(&row.get::<i64>(4).map_err(|_| PrivacyError::Integrity)?)
    {
        return Err(PrivacyError::Integrity);
    }
    let id: [u8; 16] = row
        .get::<Vec<u8>>(5)
        .map_err(|_| PrivacyError::Integrity)?
        .try_into()
        .map_err(|_| PrivacyError::Integrity)?;
    let sealed = Sealed {
        nonce: row
            .get::<Vec<u8>>(6)
            .map_err(|_| PrivacyError::Integrity)?
            .try_into()
            .map_err(|_| PrivacyError::Integrity)?,
        bytes: row.get(7).map_err(|_| PrivacyError::Integrity)?,
    };
    if next(&mut rows, budget).await?.is_some() {
        return Err(PrivacyError::Integrity);
    }
    budget.work(sealed.bytes.len())?;
    let plain = keys
        .open(&crypto::header_aad(&id), &sealed)
        .map_err(|_| PrivacyError::Key)?;
    if plain.as_slice() != HEADER {
        return Err(PrivacyError::Key);
    }
    Ok(id)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct NativeIdentity(u64, u64);

struct NativeLocation {
    root: PathBuf,
    path: PathBuf,
    root_id: NativeIdentity,
    file_id: NativeIdentity,
    created: bool,
}

impl NativeLocation {
    fn capture(location: VaultLocation, budget: &mut Budget) -> Result<Self> {
        let root = location.root;
        #[cfg(windows)]
        {
            use std::path::{Component, Prefix};
            // Rust treats a verbatim prefix as implicitly rooted, including
            // a bare verbatim drive. It still needs its native root separator.
            let mut components = root.components();
            if matches!(components.next(), Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
                && !matches!(components.next(), Some(Component::RootDir))
            {
                return Err(PrivacyError::Input);
            }
        }
        // A Windows drive prefix alone (C: or \\?\C:) is not the
        // complete absolute root. Borrow full native ancestors without
        // rebuilding/normalizing the supplied spelling; the admitted root's
        // 128-component ceiling bounds this list. Check root-to-leaf as before.
        for ancestor in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
            budget.io()?;
            let meta = fs::symlink_metadata(ancestor).map_err(|_| PrivacyError::Io)?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(PrivacyError::Integrity);
            }
        }
        let root_id = inspect(&root, true, 0, budget)?;
        let path = root.join(VAULT_FILENAME);
        let had_sidecars = check_sidecars(&path, budget)?;
        if had_sidecars {
            budget.io()?;
            match fs::symlink_metadata(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(PrivacyError::Integrity);
                }
                Err(_) => return Err(PrivacyError::Io),
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        budget.io()?;
        let created = match options.open(&path) {
            Ok(file) => {
                // Sidecars without a marked base may be foreign data. Creation
                // authority over this one new base never grants their mutation.
                if had_sidecars {
                    return Err(PrivacyError::Integrity);
                }
                budget.io()?;
                file.sync_all().map_err(|_| PrivacyError::Io)?;
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(_) => return Err(PrivacyError::Io),
        };
        let file_id = inspect(&path, false, DB_BYTES, budget)?;
        budget.io()?;
        if !created
            && fs::symlink_metadata(&path)
                .map_err(|_| PrivacyError::Io)?
                .len()
                == 0
        {
            return Err(PrivacyError::Integrity);
        }
        if created && check_sidecars(&path, budget)? {
            return Err(PrivacyError::Integrity);
        }
        let captured = Self {
            root,
            path,
            root_id,
            file_id,
            created,
        };
        captured.recheck(budget)?;
        Ok(captured)
    }

    fn recheck(&self, budget: &mut Budget) -> Result<()> {
        if inspect(&self.root, true, 0, budget)? != self.root_id
            || inspect(&self.path, false, DB_BYTES, budget)? != self.file_id
        {
            return Err(PrivacyError::Integrity);
        }
        check_sidecars(&self.path, budget)?;
        budget.check()
    }
}

fn check_sidecars(path: &Path, budget: &mut Budget) -> Result<bool> {
    let mut found = false;
    for (suffix, limit) in [
        ("-wal", WAL_BYTES),
        ("-shm", 1024 * 1024),
        ("-journal", WAL_BYTES),
    ] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let sidecar = PathBuf::from(name);
        budget.io()?;
        match fs::symlink_metadata(&sidecar) {
            Ok(_) => {
                inspect(&sidecar, false, limit, budget)?;
                found = true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(PrivacyError::Io),
        }
    }
    Ok(found)
}

fn inspect(
    path: &Path,
    directory: bool,
    limit: u64,
    budget: &mut Budget,
) -> Result<NativeIdentity> {
    budget.io()?;
    let meta = fs::symlink_metadata(path).map_err(|_| PrivacyError::Io)?;
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && (!meta.is_file() || meta.len() > limit))
    {
        return Err(PrivacyError::Integrity);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
            || (!directory && meta.nlink() != 1)
        {
            return Err(PrivacyError::Integrity);
        }
        Ok(identity_unix(&meta))
    }
    #[cfg(windows)]
    {
        identity_windows(path, directory, budget)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = meta;
        Err(PrivacyError::Unsupported)
    }
}

#[cfg(unix)]
fn identity_unix(meta: &Metadata) -> NativeIdentity {
    use std::os::unix::fs::MetadataExt;
    NativeIdentity(meta.dev(), meta.ino())
}

#[cfg(windows)]
fn identity_windows(path: &Path, directory: bool, budget: &mut Budget) -> Result<NativeIdentity> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, GetFileInformationByHandle,
    };
    budget.io()?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| PrivacyError::Io)?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    budget.io()?;
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err(PrivacyError::Integrity);
    }
    // Inherited Windows ACL privacy is not established by this supplied-root
    // boundary; the subsequent actual host/keystore adapter must establish it.
    Ok(NativeIdentity(
        info.dwVolumeSerialNumber as u64,
        ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    ))
}

pub(super) async fn mask(
    live: Arc<SessionLife>,
    input: CapturedText,
    report: DetectorReport,
    declared: DeclaredIdentifiers,
) -> Result<MaskedText> {
    let mut guard = RequestGuard {
        live: live.clone(),
        accepted: false,
    };
    live.check()?;
    if !Arc::ptr_eq(&live, &input.live) {
        return Err(PrivacyError::ForeignScope);
    }
    let mut slot = OwnedSlot::reserve(&live.vault)?;
    let worker_live = live.clone();
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| PrivacyError::Worker)?;
    let started = Instant::now();
    let returned = tokio::task::spawn_blocking(move || {
        let mut budget = Budget::new(worker_live.vault.limits, Some(worker_live.clone()));
        budget.started = started;
        let result = runtime.block_on(mask_worker(
            &worker_live,
            input,
            report,
            declared,
            &mut budget,
        ));
        if let Err(error) = &result {
            slot.inner.fail_storage(*error);
        }
        #[cfg(test)]
        let observed_inner = slot.inner.clone();
        slot.returned = true;
        drop(slot);
        #[cfg(test)]
        observed_inner.observe(Phase::AfterIoReturn);
        result
    })
    .await
    .map_err(|_| PrivacyError::Worker);
    let result = match returned {
        Ok(result) => result,
        Err(error) => {
            live.vault.fail_storage(error);
            Err(error)
        }
    }?;
    live.check()?;
    guard.accepted = true;
    Ok(result)
}

pub(super) async fn resolve(live: Arc<SessionLife>, aliases: &[&str]) -> Result<ResolutionRecords> {
    let mut guard = RequestGuard {
        live: live.clone(),
        accepted: false,
    };
    live.check()?;
    if aliases.len() > live.vault.limits.spans
        || aliases.iter().any(|s| s.is_empty() || s.len() > 256)
        || aliases.iter().map(|s| s.len()).sum::<usize>() > live.vault.limits.text_bytes
    {
        return Err(PrivacyError::Bounds);
    }
    for (index, alias) in aliases.iter().enumerate() {
        if aliases[..index].contains(alias) {
            return Err(PrivacyError::Input);
        }
    }
    let aliases: Vec<Zeroizing<String>> = aliases
        .iter()
        .map(|s| Zeroizing::new((*s).to_owned()))
        .collect();
    let mut slot = OwnedSlot::reserve(&live.vault)?;
    let worker_live = live.clone();
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| PrivacyError::Worker)?;
    let started = Instant::now();
    let returned = tokio::task::spawn_blocking(move || {
        let mut budget = Budget::new(worker_live.vault.limits, Some(worker_live.clone()));
        budget.started = started;
        let result = runtime.block_on(resolve_worker(&worker_live, aliases, &mut budget));
        if let Err(error) = &result {
            slot.inner.fail_storage(*error);
        }
        #[cfg(test)]
        let observed_inner = slot.inner.clone();
        slot.returned = true;
        drop(slot);
        #[cfg(test)]
        observed_inner.observe(Phase::AfterIoReturn);
        result
    })
    .await
    .map_err(|_| PrivacyError::Worker);
    let result = match returned {
        Ok(result) => result,
        Err(error) => {
            live.vault.fail_storage(error);
            Err(error)
        }
    }?;
    live.check()?;
    guard.accepted = true;
    Ok(result)
}

struct Stats {
    count: usize,
    bytes: usize,
    scopes: usize,
}

async fn stats(conn: &Connection, budget: &mut Budget) -> Result<Stats> {
    let mut rows = query(conn, "SELECT count(*), coalesce(sum(length(sealed)), 0), count(DISTINCT scope) FROM vault_mappings", vec![], budget).await?;
    let row = next(&mut rows, budget)
        .await?
        .ok_or(PrivacyError::Integrity)?;
    let count = usize::try_from(row.get::<i64>(0).map_err(|_| PrivacyError::Integrity)?)
        .map_err(|_| PrivacyError::Integrity)?;
    let bytes = usize::try_from(row.get::<i64>(1).map_err(|_| PrivacyError::Integrity)?)
        .map_err(|_| PrivacyError::Integrity)?;
    let scopes = usize::try_from(row.get::<i64>(2).map_err(|_| PrivacyError::Integrity)?)
        .map_err(|_| PrivacyError::Integrity)?;
    if count > budget.limits.vault_mappings
        || bytes > budget.limits.ciphertext_bytes
        || scopes > 128
    {
        return Err(PrivacyError::Bounds);
    }
    if next(&mut rows, budget).await?.is_some() {
        return Err(PrivacyError::Integrity);
    }
    Ok(Stats {
        count,
        bytes,
        scopes,
    })
}

async fn read_scope(
    inner: &Inner,
    conn: &Connection,
    scope: &[u8; 32],
    budget: &mut Budget,
) -> Result<Vec<Mapping>> {
    validate_layout(conn, budget).await?;
    if read_header(conn, &inner.keys, budget).await? != inner.id {
        return Err(PrivacyError::Integrity);
    }
    stats(conn, budget).await?;
    let mut rows = query(conn,
        "SELECT length(scope), length(original_tag), length(alias_tag), length(nonce), length(sealed), category, version, scope, original_tag, alias_tag, nonce, sealed
         FROM vault_mappings WHERE scope = ?1 ORDER BY category, original_tag LIMIT ?2",
        vec![Value::Blob(scope.to_vec()), Value::Integer(budget.limits.scope_mappings as i64 + 1)], budget).await?;
    let mut mappings: Vec<Mapping> = Vec::new();
    while let Some(row) = next(&mut rows, budget).await? {
        if mappings.len() >= budget.limits.scope_mappings {
            return Err(PrivacyError::Bounds);
        }
        let lengths: [i64; 5] =
            std::array::from_fn(|index| row.get::<i64>(index as i32).unwrap_or(-1));
        if lengths[..4] != [32, 32, 32, 12] || !(16..=1024).contains(&lengths[4]) {
            return Err(PrivacyError::Integrity);
        }
        let category = Category::from_code(row.get(5).map_err(|_| PrivacyError::Integrity)?)?;
        let version: i64 = row.get(6).map_err(|_| PrivacyError::Integrity)?;
        if version != FORMAT_VERSION as i64 {
            return Err(PrivacyError::Integrity);
        }
        let stored_scope: [u8; 32] = row
            .get::<Vec<u8>>(7)
            .map_err(|_| PrivacyError::Integrity)?
            .try_into()
            .map_err(|_| PrivacyError::Integrity)?;
        let identity: [u8; 32] = row
            .get::<Vec<u8>>(8)
            .map_err(|_| PrivacyError::Integrity)?
            .try_into()
            .map_err(|_| PrivacyError::Integrity)?;
        let alias_tag: [u8; 32] = row
            .get::<Vec<u8>>(9)
            .map_err(|_| PrivacyError::Integrity)?
            .try_into()
            .map_err(|_| PrivacyError::Integrity)?;
        if stored_scope != *scope {
            return Err(PrivacyError::Integrity);
        }
        let sealed = Sealed {
            nonce: row
                .get::<Vec<u8>>(10)
                .map_err(|_| PrivacyError::Integrity)?
                .try_into()
                .map_err(|_| PrivacyError::Integrity)?,
            bytes: row.get(11).map_err(|_| PrivacyError::Integrity)?,
        };
        budget.work(sealed.bytes.len())?;
        let aad = crypto::mapping_aad(
            &inner.id,
            scope,
            category,
            version as u32,
            &identity,
            &alias_tag,
        );
        let plain = inner.keys.open(&aad, &sealed)?;
        let mapping = crypto::decode(category, &plain)?;
        budget.work(mapping.original.len() + mapping.alias.len())?;
        if inner.keys.identity(scope, category, &mapping.original) != identity
            || inner.keys.alias_tag(&mapping.alias) != alias_tag
            || format::generate(
                &inner.keys,
                scope,
                category,
                &mapping.original,
                mapping.counter,
            ) != *mapping.alias
            || mapping.original.as_str() == mapping.alias.as_str()
        {
            return Err(PrivacyError::Integrity);
        }
        for prior in &mappings {
            budget.work(
                prior.original.len()
                    + mapping.original.len()
                    + prior.alias.len()
                    + mapping.alias.len(),
            )?;
            if prior.alias == mapping.alias || prior.original == mapping.original {
                return Err(PrivacyError::Integrity);
            }
        }
        mappings.push(mapping);
    }
    Ok(mappings)
}

async fn alias_exists(conn: &Connection, tag: &[u8; 32], budget: &mut Budget) -> Result<bool> {
    let mut rows = query(
        conn,
        "SELECT 1 FROM vault_mappings WHERE alias_tag = ?1 LIMIT 1",
        vec![Value::Blob(tag.to_vec())],
        budget,
    )
    .await?;
    Ok(next(&mut rows, budget).await?.is_some())
}

fn copy_mapping(mapping: &Mapping) -> Mapping {
    Mapping {
        category: mapping.category,
        original: Zeroizing::new(mapping.original.to_string()),
        alias: Zeroizing::new(mapping.alias.to_string()),
        counter: mapping.counter,
    }
}

async fn choose_aliases(
    inner: &Inner,
    conn: &Connection,
    scope: &[u8; 32],
    text: &str,
    selected: &[Selected],
    mappings: &mut Vec<Mapping>,
    budget: &mut Budget,
) -> Result<Vec<usize>> {
    // Any existing scope alias in original input is ambiguous, even if it was
    // not selected in this detector report. Never guess which value it meant.
    for mapping in mappings.iter() {
        budget.work(text.len() + mapping.alias.len())?;
        if text.contains(mapping.alias.as_str()) {
            return Err(PrivacyError::Collision);
        }
    }
    let mut indices = Vec::with_capacity(selected.len());
    for item in selected {
        if let Some(index) = mappings.iter().position(|m| m.original == item.value) {
            if mappings[index].category != item.category {
                return Err(PrivacyError::Input);
            }
            indices.push(index);
            continue;
        }
        if mappings.len() >= budget.limits.scope_mappings {
            return Err(PrivacyError::Bounds);
        }
        let mut chosen = None;
        for counter in 0..format::ALIAS_ATTEMPTS {
            budget.check()?;
            let alias = Zeroizing::new(format::generate(
                &inner.keys,
                scope,
                item.category,
                &item.value,
                counter,
            ));
            budget.work(text.len() + alias.len() + item.value.len())?;
            if text.contains(alias.as_str()) || alias.contains(item.value.as_str()) {
                continue;
            }
            let mut collision = false;
            for mapping in mappings.iter() {
                budget.work(mapping.original.len() + mapping.alias.len() + alias.len())?;
                if mapping.alias.as_str() == alias.as_str()
                    || alias.contains(mapping.original.as_str())
                {
                    collision = true;
                    break;
                }
            }
            if collision || alias_exists(conn, &inner.keys.alias_tag(&alias), budget).await? {
                continue;
            }
            chosen = Some(Mapping {
                category: item.category,
                original: Zeroizing::new(item.value.to_string()),
                alias,
                counter,
            });
            break;
        }
        let mapping = chosen.ok_or(PrivacyError::Collision)?;
        indices.push(mappings.len());
        mappings.push(mapping);
    }
    Ok(indices)
}

fn build_masked(
    text: &str,
    selected: &[Selected],
    indices: &[usize],
    mappings: &[Mapping],
    budget: &mut Budget,
) -> Result<Zeroizing<String>> {
    let mut length = text.len();
    for (span, &index) in selected.iter().zip(indices) {
        length = length
            .checked_sub(span.end - span.start)
            .and_then(|n| n.checked_add(mappings[index].alias.len()))
            .ok_or(PrivacyError::Bounds)?;
    }
    if length > budget.limits.text_bytes {
        return Err(PrivacyError::Bounds);
    }
    budget.work(length)?;
    let mut output = Zeroizing::new(String::with_capacity(length));
    let mut ranges = Vec::with_capacity(selected.len());
    let mut previous = 0;
    for (span, &index) in selected.iter().zip(indices) {
        output.push_str(&text[previous..span.start]);
        let start = output.len();
        output.push_str(&mappings[index].alias);
        ranges.push((start, output.len(), index));
        previous = span.end;
    }
    output.push_str(&text[previous..]);
    for (position, &index) in indices.iter().enumerate() {
        if indices[..position].contains(&index) {
            continue;
        }
        let mapping = &mappings[index];
        budget.work(output.len() + mapping.original.len() + output.len() + mapping.alias.len())?;
        if output.contains(mapping.original.as_str()) {
            return Err(PrivacyError::Collision);
        }
        for (start, alias) in output.match_indices(mapping.alias.as_str()) {
            if !ranges
                .iter()
                .any(|&(s, e, i)| i == index && s == start && e == start + alias.len())
            {
                return Err(PrivacyError::Collision);
            }
        }
    }
    budget.check()?;
    Ok(output)
}

async fn mask_worker(
    live: &Arc<SessionLife>,
    input: CapturedText,
    report: DetectorReport,
    declared: DeclaredIdentifiers,
    budget: &mut Budget,
) -> Result<MaskedText> {
    let selected = format::validate_spans(&input.text, &input.digest, report, &declared, budget)?;
    let inner = &live.vault;
    inner.native.recheck(budget)?;
    let conn = inner.conn.lock().map_err(|_| PrivacyError::Integrity)?;
    begin(&conn, "BEGIN IMMEDIATE", budget).await?;
    let prepared = async {
        let mut mappings = read_scope(inner, &conn, &live.scope, budget).await?;
        let old_count = mappings.len();
        let indices = choose_aliases(
            inner,
            &conn,
            &live.scope,
            &input.text,
            &selected,
            &mut mappings,
            budget,
        )
        .await?;
        let text = build_masked(&input.text, &selected, &indices, &mappings, budget)?;
        let stats = stats(&conn, budget).await?;
        let mut inserts = Vec::new();
        let mut added_bytes = 0;
        for mapping in &mappings[old_count..] {
            let identity = inner
                .keys
                .identity(&live.scope, mapping.category, &mapping.original);
            let alias_tag = inner.keys.alias_tag(&mapping.alias);
            let aad = crypto::mapping_aad(
                &inner.id,
                &live.scope,
                mapping.category,
                FORMAT_VERSION,
                &identity,
                &alias_tag,
            );
            let sealed = inner.keys.seal(&aad, crypto::encode(mapping))?;
            budget.work(sealed.bytes.len())?;
            added_bytes += sealed.bytes.len();
            inserts.push(vec![
                Value::Blob(live.scope.to_vec()),
                Value::Blob(identity.to_vec()),
                Value::Blob(alias_tag.to_vec()),
                Value::Integer(mapping.category.code()),
                Value::Integer(FORMAT_VERSION as i64),
                Value::Blob(sealed.nonce.to_vec()),
                Value::Blob(sealed.bytes),
            ]);
        }
        if stats.count + inserts.len() > budget.limits.vault_mappings
            || stats.bytes + added_bytes > budget.limits.ciphertext_bytes
            || (old_count == 0 && !inserts.is_empty() && stats.scopes >= 128)
        {
            return Err(PrivacyError::Bounds);
        }
        #[cfg(test)]
        let mut insertion_index = 0;
        for params in &mut inserts {
            #[cfg(test)]
            {
                insertion_index += 1;
                if *inner
                    .fail_insert_at
                    .lock()
                    .expect("private test SQL failure")
                    == Some(insertion_index)
                {
                    // A real CHECK failure on this new row, after earlier real IO.
                    params[3] = Value::Integer(99);
                }
            }
            execute(
                &conn,
                "INSERT INTO vault_mappings VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                std::mem::take(params),
                budget,
            )
            .await?;
            #[cfg(test)]
            inner.observe(Phase::AfterInsert);
        }
        inner.native.recheck(budget)?;
        let mut records = Vec::new();
        for (position, &index) in indices.iter().enumerate() {
            if !indices[..position].contains(&index) {
                records.push(copy_mapping(&mappings[index]));
            }
        }
        Ok(MaskedText {
            text,
            input_digest: input.digest,
            replacements: selected.len(),
            records: ResolutionRecords {
                live: live.clone(),
                mappings: records,
            },
        })
    }
    .await;
    match prepared {
        Err(error) => {
            rollback(&conn, budget).await?;
            Err(error)
        }
        Ok(masked) => {
            if let Err(error) = execute(&conn, "COMMIT", vec![], budget).await {
                if !conn.is_autocommit() {
                    rollback(&conn, budget).await?;
                }
                return Err(error);
            }
            #[cfg(test)]
            inner.observe(Phase::AfterCommit);
            inner.native.recheck(budget)?;
            budget.check()?;
            Ok(masked)
        }
    }
}

async fn resolve_worker(
    live: &Arc<SessionLife>,
    aliases: Vec<Zeroizing<String>>,
    budget: &mut Budget,
) -> Result<ResolutionRecords> {
    let inner = &live.vault;
    inner.native.recheck(budget)?;
    let conn = inner.conn.lock().map_err(|_| PrivacyError::Integrity)?;
    begin(&conn, "BEGIN DEFERRED", budget).await?;
    let prepared = async {
        let mappings = read_scope(inner, &conn, &live.scope, budget).await?;
        let mut records = Vec::with_capacity(aliases.len());
        for alias in aliases {
            budget.work(alias.len())?;
            let mapping = mappings
                .iter()
                .find(|mapping| mapping.alias == alias)
                .ok_or(PrivacyError::UnknownAlias)?;
            records.push(copy_mapping(mapping));
        }
        inner.native.recheck(budget)?;
        Ok(ResolutionRecords {
            live: live.clone(),
            mappings: records,
        })
    }
    .await;
    match prepared {
        Err(error) => {
            rollback(&conn, budget).await?;
            Err(error)
        }
        Ok(records) => {
            if let Err(error) = execute(&conn, "COMMIT", vec![], budget).await {
                if !conn.is_autocommit() {
                    rollback(&conn, budget).await?;
                }
                return Err(error);
            }
            inner.native.recheck(budget)?;
            Ok(records)
        }
    }
}

/// Additive whole-scope read for a host owner of this same live session. It
/// follows the resolve path exactly: one owned slot, one deferred transaction,
/// the existing authenticated scope reader and the same refusal handling.
pub(super) async fn scope_records(live: Arc<SessionLife>) -> Result<ResolutionRecords> {
    let mut guard = RequestGuard {
        live: live.clone(),
        accepted: false,
    };
    live.check()?;
    let mut slot = OwnedSlot::reserve(&live.vault)?;
    let worker_live = live.clone();
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| PrivacyError::Worker)?;
    let started = Instant::now();
    let returned = tokio::task::spawn_blocking(move || {
        let mut budget = Budget::new(worker_live.vault.limits, Some(worker_live.clone()));
        budget.started = started;
        let result = runtime.block_on(scope_records_worker(&worker_live, &mut budget));
        if let Err(error) = &result {
            slot.inner.fail_storage(*error);
        }
        slot.returned = true;
        drop(slot);
        result
    })
    .await
    .map_err(|_| PrivacyError::Worker);
    let result = match returned {
        Ok(result) => result,
        Err(error) => {
            live.vault.fail_storage(error);
            Err(error)
        }
    }?;
    live.check()?;
    guard.accepted = true;
    Ok(result)
}

async fn scope_records_worker(
    live: &Arc<SessionLife>,
    budget: &mut Budget,
) -> Result<ResolutionRecords> {
    let inner = &live.vault;
    inner.native.recheck(budget)?;
    let conn = inner.conn.lock().map_err(|_| PrivacyError::Integrity)?;
    begin(&conn, "BEGIN DEFERRED", budget).await?;
    let prepared = async {
        let mappings = read_scope(inner, &conn, &live.scope, budget).await?;
        inner.native.recheck(budget)?;
        Ok(ResolutionRecords {
            live: live.clone(),
            mappings,
        })
    }
    .await;
    match prepared {
        Err(error) => {
            rollback(&conn, budget).await?;
            Err(error)
        }
        Ok(records) => {
            if let Err(error) = execute(&conn, "COMMIT", vec![], budget).await {
                if !conn.is_autocommit() {
                    rollback(&conn, budget).await?;
                }
                return Err(error);
            }
            inner.native.recheck(budget)?;
            Ok(records)
        }
    }
}
