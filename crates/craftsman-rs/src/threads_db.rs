//! Canonical `~/.craftsman/threads.db` path and one-time legacy migration.
//!
//! The retired Python client also used `threads.db`, but with a LangGraph
//! schema incompatible with the Rust checkpointer. During migration we archive
//! that database (including WAL/SHM sidecars) and move the Rust
//! `threads-rs.db` database into the canonical name. No database is deleted.
//!
//! The schema version also lives here: `PRAGMA user_version` records which
//! table layout a Rust-schema database has, and the checkpointer stamps it when
//! it opens a local database that has none. Archived legacy databases are only
//! inspected and moved, never handed to the checkpointer, so they keep their
//! version (0).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, MutexGuard, OnceCell};

const CURRENT_NAME: &str = "threads.db";
const LEGACY_RUST_NAME: &str = "threads-rs.db";

/// Table-layout version recorded in `PRAGMA user_version`. 1 is the layout
/// `LibsqlCheckpointer::apply_schema` creates; `store_files` is created
/// separately by `StoreFs` and is not part of it.
///
/// What a stored value tells a reader:
///
/// - **0 means unknown, not "the 0.4.0 layout".** Every database written by
///   0.4.0 or earlier reads 0, and so does a file whose first launch did not
///   finish: `apply_schema` stamps last, and the agent opens `StoreFs` (which
///   creates `store_files` and nothing else) before the checkpointer, so an
///   interrupted launch leaves a partial layout at 0. A migrator that finds 0
///   must run `apply_schema`, which is idempotent (it creates what is missing
///   and adds the columns older layouts lack), and must not assume one fixed
///   layout.
/// - A positive value is a layout some binary finished. [`stamp_schema_version`]
///   never rewrites or lowers it; a layout change that raises this number needs
///   a migration that moves older databases up.
/// - A negative value is not one any Craftsman version writes. It is treated like a
///   positive one: left alone, never replaced by a stamp.
pub(crate) const SCHEMA_VERSION: i64 = 1;

static THREADS_DB_PATH: OnceCell<Result<PathBuf, String>> = OnceCell::const_new();
/// Serialize schema/bootstrap work across the multiple lightweight local
/// connections used by the desktop catalog, checkpointer, and memory store.
/// libSQL local databases otherwise return `database is locked` when those
/// idempotent writers race during desktop startup.
static LOCAL_DB_INIT_LOCK: Mutex<()> = Mutex::const_new(());

pub(crate) async fn local_database_init_guard() -> MutexGuard<'static, ()> {
    LOCAL_DB_INIT_LOCK.lock().await
}

/// Configure local thread databases for concurrent desktop catalog/session
/// access. WAL permits readers while a checkpoint is being written; the busy
/// timeout covers the remaining short single-writer handoffs.
pub(crate) async fn configure_local_connection(conn: &libsql::Connection) -> Result<(), String> {
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let mut rows = conn
        .query("PRAGMA journal_mode = WAL", ())
        .await
        .map_err(|error| error.to_string())?;
    let mode = rows
        .next()
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "SQLite did not report a journal mode".to_string())?
        .get::<String>(0)
        .map_err(|error| error.to_string())?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(format!(
            "SQLite refused WAL journal mode and returned {mode}"
        ));
    }
    Ok(())
}

/// `PRAGMA user_version` of the database behind `conn`; 0 when never stamped.
pub(crate) async fn user_version(conn: &libsql::Connection) -> Result<i64, libsql::Error> {
    let mut rows = conn.query("PRAGMA user_version", ()).await?;
    match rows.next().await? {
        Some(row) => row.get::<i64>(0),
        None => Ok(0),
    }
}

/// Record [`SCHEMA_VERSION`] in `PRAGMA user_version` when the database has no
/// version yet (0). Any other value, a negative one included, is left alone, so
/// a database a newer binary already raised is never lowered. Changes no table
/// or column.
pub(crate) async fn stamp_schema_version(conn: &libsql::Connection) -> Result<(), libsql::Error> {
    // An already stamped database, the usual case, takes no write lock.
    if user_version(conn).await? != 0 {
        return Ok(());
    }
    // Look again under the write lock: another process may have stamped the
    // database, or a newer binary raised it, since the read above.
    let transaction = conn
        .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
        .await?;
    if user_version(&transaction).await? == 0 {
        transaction
            .execute(&format!("PRAGMA user_version = {SCHEMA_VERSION}"), ())
            .await?;
    }
    transaction.commit().await
}

/// Test helper: a plain local connection to the database at `path`. The
/// `Database` comes back too, because dropping it closes the connection.
///
/// Open as few connections as a test needs. libsql 0.9 closes every local
/// connection twice (`disconnect` runs from two `Drop` impls and does not clear
/// the handle), which is a use after free inside SQLite: rarely, and in
/// whichever test thread allocates next, it shows up as a `bad parameter` error
/// or a crash. So a test reuses a connection it already holds, and reads the
/// database from the outside through one [`Probe`], never one connection per
/// read.
#[cfg(test)]
pub(crate) async fn open_plain(path: &Path) -> (libsql::Database, libsql::Connection) {
    let db = libsql::Builder::new_local(path.to_string_lossy().to_string())
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    (db, conn)
}

/// Test helper: one connection that only reads the database at `path`, so a test
/// can look at the file from outside the code it exercises (what another process
/// would see once a change is committed) and look again after each step.
#[cfg(test)]
pub(crate) struct Probe {
    _db: libsql::Database,
    conn: libsql::Connection,
}

#[cfg(test)]
impl Probe {
    pub(crate) async fn open(path: &Path) -> Self {
        let (db, conn) = open_plain(path).await;
        Self { _db: db, conn }
    }

    /// The connection itself, for a query the helpers below do not cover.
    pub(crate) fn conn(&self) -> &libsql::Connection {
        &self.conn
    }

    /// `PRAGMA user_version` as committed right now.
    pub(crate) async fn user_version(&self) -> i64 {
        user_version(&self.conn).await.unwrap()
    }

    /// [`dump`] of the database as committed right now.
    pub(crate) async fn dump(&self) -> Vec<String> {
        dump(&self.conn).await
    }
}

/// Test helper: every table and index definition plus every row of every table,
/// as text, so two snapshots of one database can be compared for equality.
/// `PRAGMA user_version` is not part of it.
#[cfg(test)]
pub(crate) async fn dump(conn: &libsql::Connection) -> Vec<String> {
    let mut definitions = conn
        .query(
            "SELECT type, name, sql FROM sqlite_master
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            (),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    let mut tables = Vec::new();
    while let Some(row) = definitions.next().await.unwrap() {
        let kind: String = row.get(0).unwrap();
        let name: String = row.get(1).unwrap();
        out.push(format!("{kind} {name}: {:?}", row.get_value(2).unwrap()));
        if kind == "table" {
            tables.push(name);
        }
    }
    for table in tables {
        let mut rows = conn
            .query(&format!("SELECT * FROM \"{table}\" ORDER BY rowid"), ())
            .await
            .unwrap();
        while let Some(row) = rows.next().await.unwrap() {
            let cells = (0..row.column_count())
                .map(|index| format!("{:?}", row.get_value(index).unwrap()))
                .collect::<Vec<_>>();
            out.push(format!("{table}: {}", cells.join(" | ")));
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SchemaKind {
    Missing,
    Empty,
    Rust,
    Python,
    Unknown(Vec<String>),
}

pub async fn path() -> Result<PathBuf, String> {
    THREADS_DB_PATH
        .get_or_init(|| async { prepare_at(&crate::config::config_dir()).await })
        .await
        .clone()
}

pub(crate) async fn prepare_at(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let current = dir.join(CURRENT_NAME);
    let legacy_rust = dir.join(LEGACY_RUST_NAME);
    let current_kind = classify(&current).await?;
    let legacy_kind = classify(&legacy_rust).await?;

    match (current_kind, legacy_kind) {
        (SchemaKind::Rust | SchemaKind::Empty, SchemaKind::Missing) => Ok(current),
        (SchemaKind::Missing, SchemaKind::Missing) => Ok(current),
        (SchemaKind::Missing, SchemaKind::Rust) => {
            move_database_set(&legacy_rust, &current)?;
            Ok(current)
        }
        (SchemaKind::Python, SchemaKind::Rust) => {
            archive_database(&current, "threads-python-legacy")?;
            move_database_set(&legacy_rust, &current)?;
            Ok(current)
        }
        (SchemaKind::Python, SchemaKind::Missing) => {
            archive_database(&current, "threads-python-legacy")?;
            Ok(current)
        }
        (SchemaKind::Empty, SchemaKind::Rust) => {
            archive_database(&current, "threads-empty-legacy")?;
            move_database_set(&legacy_rust, &current)?;
            Ok(current)
        }
        (SchemaKind::Rust, SchemaKind::Rust) => Err(format!(
            "both {} and {} contain Rust thread data; refusing to choose one automatically",
            current.display(),
            legacy_rust.display()
        )),
        (current_kind, legacy_kind) => Err(format!(
            "cannot migrate thread database: {} is {current_kind:?}, {} is {legacy_kind:?}",
            current.display(),
            legacy_rust.display()
        )),
    }
}

async fn classify(path: &Path) -> Result<SchemaKind, String> {
    if !path.exists() {
        return Ok(SchemaKind::Missing);
    }
    if path.metadata().map_err(|error| error.to_string())?.len() == 0 {
        return Ok(SchemaKind::Empty);
    }

    let db = libsql::Builder::new_local(path.to_string_lossy().to_string())
        .build()
        .await
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    let conn = db.connect().map_err(|error| error.to_string())?;
    let mut rows = conn
        .query("PRAGMA table_info(checkpoints)", ())
        .await
        .map_err(|error| error.to_string())?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
        columns.push(row.get::<String>(1).map_err(|error| error.to_string())?);
    }
    drop(rows);
    drop(conn);
    drop(db);

    if columns.is_empty() {
        return Ok(SchemaKind::Empty);
    }
    if ["thread_id", "seq", "state_json", "suspension_json"]
        .iter()
        .all(|column| columns.iter().any(|candidate| candidate == column))
    {
        return Ok(SchemaKind::Rust);
    }
    if ["thread_id", "checkpoint_ns", "checkpoint_id", "checkpoint"]
        .iter()
        .all(|column| columns.iter().any(|candidate| candidate == column))
    {
        return Ok(SchemaKind::Python);
    }
    columns.sort();
    Ok(SchemaKind::Unknown(columns))
}

fn archive_database(path: &Path, label: &str) -> Result<PathBuf, String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("database path has no parent: {}", path.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    for sequence in 0..10_000u32 {
        let suffix = if sequence == 0 {
            String::new()
        } else {
            format!("-{sequence}")
        };
        let candidate = dir.join(format!("{label}-{timestamp}{suffix}.db"));
        if !candidate.exists() {
            move_database_set(path, &candidate)?;
            return Ok(candidate);
        }
    }
    Err(format!(
        "could not choose a unique backup name for {}",
        path.display()
    ))
}

fn move_database_set(from: &Path, to: &Path) -> Result<(), String> {
    if !from.exists() {
        return Ok(());
    }
    if to.exists() {
        return Err(format!(
            "refusing to replace existing database {}",
            to.display()
        ));
    }

    let mut moved = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let source = path_with_suffix(from, suffix);
        if !source.exists() {
            continue;
        }
        let destination = path_with_suffix(to, suffix);
        if let Err(error) = std::fs::rename(&source, &destination) {
            for (moved_from, moved_to) in moved.into_iter().rev() {
                let _ = std::fs::rename(moved_to, moved_from);
            }
            return Err(format!(
                "could not move {} to {}: {error}",
                source.display(),
                destination.display()
            ));
        }
        moved.push((source, destination));
    }
    Ok(())
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create the database at `path` with `schema`. The connection that did it
    /// comes back, for a test that wants to keep working through it; most callers
    /// let it drop.
    async fn create_db(path: &Path, schema: &str) -> (libsql::Database, libsql::Connection) {
        let (db, conn) = open_plain(path).await;
        conn.execute(schema, ()).await.unwrap();
        (db, conn)
    }

    async fn create_rust_db(path: &Path) -> (libsql::Database, libsql::Connection) {
        create_db(
            path,
            "CREATE TABLE checkpoints (
                thread_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                state_json TEXT NOT NULL,
                suspension_json TEXT,
                PRIMARY KEY (thread_id, seq)
            )",
        )
        .await
    }

    async fn create_python_db(path: &Path) -> (libsql::Database, libsql::Connection) {
        create_db(
            path,
            "CREATE TABLE checkpoints (
                thread_id TEXT NOT NULL,
                checkpoint_ns TEXT NOT NULL DEFAULT '',
                checkpoint_id TEXT NOT NULL,
                checkpoint BLOB,
                PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id)
            )",
        )
        .await
    }

    #[tokio::test]
    async fn missing_databases_select_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            prepare_at(dir.path()).await.unwrap(),
            dir.path().join(CURRENT_NAME)
        );
    }

    #[tokio::test]
    async fn legacy_rust_database_moves_to_canonical_name_with_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(LEGACY_RUST_NAME);
        create_rust_db(&old).await;
        let current = prepare_at(dir.path()).await.unwrap();
        assert_eq!(current, dir.path().join(CURRENT_NAME));
        assert!(!old.exists());
        assert!(current.exists());
        assert_eq!(classify(&current).await.unwrap(), SchemaKind::Rust);
    }

    #[tokio::test]
    async fn python_database_is_archived_before_rust_database_moves() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join(CURRENT_NAME);
        let old = dir.path().join(LEGACY_RUST_NAME);
        create_python_db(&current).await;
        create_rust_db(&old).await;
        assert_eq!(prepare_at(dir.path()).await.unwrap(), current);
        assert_eq!(classify(&current).await.unwrap(), SchemaKind::Rust);
        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("threads-python-legacy-"))
            .collect::<Vec<_>>();
        assert!(backups.iter().any(|name| name.ends_with(".db")));
    }

    #[tokio::test]
    async fn python_only_database_is_archived_for_a_fresh_rust_store() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join(CURRENT_NAME);
        create_python_db(&current).await;

        assert_eq!(prepare_at(dir.path()).await.unwrap(), current);
        assert!(!current.exists());
        assert!(std::fs::read_dir(dir.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("threads-python-legacy-")
        }));
    }

    #[test]
    fn database_set_move_includes_wal_and_shm_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.db");
        let to = dir.path().join("to.db");
        std::fs::write(&from, "db").unwrap();
        std::fs::write(path_with_suffix(&from, "-wal"), "wal").unwrap();
        std::fs::write(path_with_suffix(&from, "-shm"), "shm").unwrap();

        move_database_set(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "db");
        assert_eq!(
            std::fs::read_to_string(path_with_suffix(&to, "-wal")).unwrap(),
            "wal"
        );
        assert_eq!(
            std::fs::read_to_string(path_with_suffix(&to, "-shm")).unwrap(),
            "shm"
        );
    }

    #[tokio::test]
    async fn two_rust_databases_fail_instead_of_discarding_history() {
        let dir = tempfile::tempdir().unwrap();
        create_rust_db(&dir.path().join(CURRENT_NAME)).await;
        create_rust_db(&dir.path().join(LEGACY_RUST_NAME)).await;
        assert!(prepare_at(dir.path()).await.unwrap_err().contains("both"));
    }

    #[tokio::test]
    async fn stamp_sets_version_one_without_changing_any_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CURRENT_NAME);
        let (_db, conn) = create_rust_db(&path).await;
        // A second connection looks at the file from outside, before and after.
        let outside = Probe::open(&path).await;
        assert_eq!(user_version(&conn).await.unwrap(), 0);
        assert_eq!(outside.user_version().await, 0);
        let before = dump(&conn).await;
        assert!(!before.is_empty());

        stamp_schema_version(&conn).await.unwrap();
        assert_eq!(user_version(&conn).await.unwrap(), 1);
        assert_eq!(dump(&conn).await, before);
        // The stamp is committed, not only visible to the connection that set it.
        assert_eq!(outside.user_version().await, 1);
        assert_eq!(outside.dump().await, before);
    }

    #[tokio::test]
    async fn stamp_never_lowers_an_existing_version() {
        // 1 is already current; 2 and 41 stand for a database a newer binary
        // raised before this one opened it. One database, set to each in turn.
        let dir = tempfile::tempdir().unwrap();
        let (_db, conn) = create_rust_db(&dir.path().join(CURRENT_NAME)).await;
        for existing in [1, 2, 41] {
            conn.execute(&format!("PRAGMA user_version = {existing}"), ())
                .await
                .unwrap();

            stamp_schema_version(&conn).await.unwrap();
            assert_eq!(user_version(&conn).await.unwrap(), existing);
        }
    }

    /// A negative version can only have been written on purpose, and no Craftsman
    /// version writes one. It is not "unstamped": like any nonzero value it is
    /// left exactly as it is (SQLite keeps the field as a signed 32-bit integer).
    #[tokio::test]
    async fn stamp_leaves_a_negative_version_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (_db, conn) = create_rust_db(&dir.path().join(CURRENT_NAME)).await;
        for existing in [-1, -3, i64::from(i32::MIN)] {
            conn.execute(&format!("PRAGMA user_version = {existing}"), ())
                .await
                .unwrap();
            assert_eq!(user_version(&conn).await.unwrap(), existing);

            stamp_schema_version(&conn).await.unwrap();
            assert_eq!(user_version(&conn).await.unwrap(), existing);
        }
    }

    /// The usual case, a database that already has its version, needs no write
    /// lock: opening it while another connection is writing must not wait.
    #[tokio::test]
    async fn stamp_on_a_stamped_database_does_not_take_the_write_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CURRENT_NAME);
        let (_db, writer) = create_rust_db(&path).await;
        configure_local_connection(&writer).await.unwrap();
        stamp_schema_version(&writer).await.unwrap();
        let writing = writer
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .unwrap();

        let (_db, opener) = open_plain(&path).await;
        // No waiting: a second write lock would fail at once.
        opener.busy_timeout(Duration::ZERO).unwrap();
        stamp_schema_version(&opener).await.unwrap();

        writing.commit().await.unwrap();
        assert_eq!(user_version(&opener).await.unwrap(), 1);
    }

    /// Two openers of an unstamped database at once: one reads 0, then waits for
    /// the write lock while the other raises the version. The waiting one must
    /// look again under the lock and leave the higher version alone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stamp_rereads_under_the_write_lock_and_keeps_a_version_raised_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CURRENT_NAME);
        let (_db, other) = create_rust_db(&path).await;
        // WAL, as every real opener configures it: readers do not wait for the
        // writer, so the stamp's first read sees the still unstamped database.
        configure_local_connection(&other).await.unwrap();
        let raising = other
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .unwrap();
        raising
            .execute("PRAGMA user_version = 7", ())
            .await
            .unwrap();

        let stamping = tokio::spawn({
            let path = path.clone();
            async move {
                let (_db, conn) = open_plain(&path).await;
                conn.busy_timeout(Duration::from_secs(30)).unwrap();
                stamp_schema_version(&conn).await
            }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !stamping.is_finished(),
            "the stamp should be waiting for the write lock"
        );

        raising.commit().await.unwrap();
        stamping.await.unwrap().unwrap();
        // The stamp ran on another connection; what is committed is still 7.
        assert_eq!(user_version(&other).await.unwrap(), 7);
    }

    #[tokio::test]
    async fn archived_python_database_is_not_stamped() {
        for with_legacy_rust in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let current = dir.path().join(CURRENT_NAME);
            create_python_db(&current).await;
            if with_legacy_rust {
                create_rust_db(&dir.path().join(LEGACY_RUST_NAME)).await;
            }
            let original = std::fs::read(&current).unwrap();
            // The user_version field of the file header (bytes 60 to 63): none.
            assert_eq!(original[60..64], [0, 0, 0, 0]);

            let path = prepare_at(dir.path()).await.unwrap();

            let archive = std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| {
                    let name = path.file_name().unwrap().to_string_lossy();
                    name.starts_with("threads-python-legacy-") && name.ends_with(".db")
                })
                .expect("the legacy database is archived");
            // Moved byte for byte, then still the legacy schema at version 0.
            assert_eq!(std::fs::read(&archive).unwrap(), original);
            assert_eq!(classify(&archive).await.unwrap(), SchemaKind::Python);

            // The checkpointer then opens the database that took its place (a
            // fresh one, or the moved Rust database): that one is stamped, the
            // archive is not.
            let store = crate::checkpoints::LibsqlCheckpointer::open_at_for_workspace(
                path.clone(),
                "/workspace".into(),
            )
            .await
            .unwrap();
            assert_eq!(user_version(store.connection()).await.unwrap(), 1);
            // Untouched: the same bytes, so the same version (0) and schema.
            assert_eq!(std::fs::read(&archive).unwrap(), original);
            drop(store);
        }
    }
}
