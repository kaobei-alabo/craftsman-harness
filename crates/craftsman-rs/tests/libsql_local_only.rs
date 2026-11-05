//! Guard: Craftsman's memory and thread store is one local file (`~/.craftsman/threads.db`), so
//! `libsql` is built with `core` only. Its default features also build the Turso remote
//! and replication client: an HTTP/gRPC stack, an SQL parser and an old TLS line
//! (rustls 0.22, rustls-webpki 0.102) that carried security advisories for a feature
//! Craftsman never used.
//!
//! This reads the resolved dependency list of `libsql` in `Cargo.lock` and fails if any
//! crate that exists only for those features comes back. It does not shell out to cargo,
//! so it works offline.

use std::fs;
use std::path::Path;

/// Crates `libsql` depends on only through its `remote`, `replication`, `sync` and `tls`
/// features.
const REMOTE_ONLY: &[&str] = &[
    "hyper",
    "hyper-rustls",
    "libsql-hrana",
    "libsql-sqlite3-parser",
    "libsql_replication",
    "tonic",
    "tonic-web",
    "tower",
    "tower-http",
];

/// The value of a top-level `key = "value"` line in one `[[package]]` block.
fn field<'a>(package: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key} = \"");
    package
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix.as_str()))
        .and_then(|rest| rest.strip_suffix('"'))
}

/// Names (without version) listed under `dependencies = [ ... ]` in one package block.
fn dependency_names(package: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut inside = false;
    for line in package.lines().map(str::trim) {
        if line == "dependencies = [" {
            inside = true;
        } else if inside && line == "]" {
            break;
        } else if inside {
            let entry = line.trim_end_matches(',').trim_matches('"');
            if let Some(name) = entry.split_whitespace().next() {
                names.push(name);
            }
        }
    }
    names
}

#[test]
fn libsql_is_built_without_its_remote_and_replication_features() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let lock = fs::read_to_string(&lock_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", lock_path.display()));

    let mut libsql_packages = 0;
    let mut offenders = Vec::new();
    for package in lock.split("[[package]]").skip(1) {
        if field(package, "name") != Some("libsql") {
            continue;
        }
        libsql_packages += 1;
        for name in dependency_names(package) {
            if REMOTE_ONLY.contains(&name) {
                offenders.push(name);
            }
        }
    }

    assert!(
        libsql_packages > 0,
        "no `libsql` package in Cargo.lock; if the database library changed, update or delete this guard"
    );
    assert!(
        offenders.is_empty(),
        "libsql is pulling in remote/replication crates {offenders:?}. Craftsman's database is a local \
         file only: crates/craftsman-rs/Cargo.toml declares libsql with `default-features = false, \
         features = [\"core\"]`. If a new libsql feature genuinely needs one of these crates, \
         change REMOTE_ONLY on purpose."
    );
}

#[test]
fn lock_parser_reads_names_and_versioned_dependencies() {
    let package = "\nname = \"libsql\"\nversion = \"0.9.30\"\ndependencies = [\n \"bytes\",\n \"hyper 0.14.32\",\n \"thiserror 1.0.69\",\n]\n";
    assert_eq!(field(package, "name"), Some("libsql"));
    assert_eq!(
        dependency_names(package),
        vec!["bytes", "hyper", "thiserror"]
    );
    assert_eq!(
        dependency_names("\nname = \"leaf\"\nversion = \"1.0.0\"\n"),
        Vec::<&str>::new()
    );
}
