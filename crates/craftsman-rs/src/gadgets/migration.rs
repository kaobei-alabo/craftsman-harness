//! Create-only JSON-plugin migration. Originals, grants and user config are
//! never modified. Declarative migration does not run any guest or build code.

use super::authoring::{directory, regular_bytes, write_new};
use super::manifest::{MAX_MANIFEST_BYTES, Manifest, Migration, valid_id};
use crate::extensions::MAX_TOTAL_BYTES;
use crate::plugins::Plugin;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub(crate) struct Row {
    pub source: PathBuf,
    pub gadget: Option<String>,
    pub destination: Option<PathBuf>,
    pub status: &'static str,
    pub error: Option<String>,
}
#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub complete: bool,
    pub project: bool,
    pub dry_run: bool,
    pub originals_preserved: bool,
    pub model_opt_in_required: bool,
    pub migrations: Vec<Row>,
}

fn gadget_id(name: &str) -> String {
    if valid_id(name) {
        return name.to_owned();
    }
    // Single-character, digit-leading and oversized legacy names remain
    // distinct. The original name remains the merge identity in [migration].
    let prefix = &name[..name.len().min(44)]; // validated legacy names are ASCII
    let prefix = prefix.trim_end_matches('-');
    let digest = format!("{:x}", Sha256::digest(name.as_bytes()));
    format!("plugin-{prefix}-{}", &digest[..12])
}

pub(crate) fn manifest_from_plugin(plugin: Plugin, order: usize) -> anyhow::Result<Manifest> {
    anyhow::ensure!(
        crate::plugins::valid_plugin(&plugin),
        "invalid legacy plugin"
    );
    let id = gadget_id(&plugin.name);
    let manifest = Manifest {
        id: id.clone(),
        version: "0.1.0".into(),
        description: "Migrated declarative plugin.".into(),
        capabilities: Default::default(),
        prompt: plugin.prompt,
        commands: plugin.commands,
        agents: plugin.agents,
        migration: Some(Migration {
            plugin_name: plugin.name,
            order,
        }),
    };
    // TOML can be larger than JSON. Never publish a manifest the host rejects.
    let text = toml::to_string_pretty(&manifest)?;
    Ok(Manifest::parse(&text, &id)?)
}

fn check_directory(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => anyhow::ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "migration destination must not be a symlink or non-directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn visible_bundles(base: &Path) -> anyhow::Result<usize> {
    check_directory(base)?;
    check_directory(&base.join("gadgets"))?;
    let entries = match fs::read_dir(base.join("gadgets")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut visible = 0;
    for (index, entry) in entries.enumerate() {
        anyhow::ensure!(
            index < crate::extensions::MAX_DIR_ENTRIES,
            "gadget root exceeds 1024 total directory entries; migration cannot publish into an undiscoverable root"
        );
        if !entry?.file_name().to_string_lossy().starts_with('.') {
            visible += 1;
        }
    }
    anyhow::ensure!(
        visible <= super::manifest::MAX_GADGETS_PER_ROOT,
        "gadget root exceeds 64 visible bundles; migration cannot publish into an undiscoverable root"
    );
    Ok(visible)
}

fn publish(base: &Path, manifest: &Manifest, dry_run: bool) -> anyhow::Result<&'static str> {
    check_directory(base)?;
    let gadgets = base.join("gadgets");
    check_directory(&gadgets)?;
    let target = gadgets.join(&manifest.id);
    match fs::symlink_metadata(&target) {
        Ok(meta) => {
            anyhow::ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "existing gadget source must not be a symlink or non-directory"
            );
            let bytes = regular_bytes(&target.join("gadget.toml"), MAX_MANIFEST_BYTES)?;
            let existing = Manifest::parse(std::str::from_utf8(&bytes)?, &manifest.id)?;
            anyhow::ensure!(
                existing == *manifest,
                "existing gadget differs; no files were overwritten (resolve or rename it manually)"
            );
            return Ok("already-migrated");
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if dry_run {
        return Ok("would-migrate");
    }
    // Parent creation uses the same symlink checks/private modes as authoring.
    let parent = base
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid migration root"))?;
    let name = base
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("migration root must be UTF-8"))?;
    directory(parent, name)?;
    directory(base, "gadgets")?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&target)?; // create-only even if another migration races us
    let text = toml::to_string_pretty(manifest)?;
    // A single create-only file; no existing bundle is ever replaced.
    write_new(&target.join("gadget.toml"), text.as_bytes())?;
    Ok("migrated")
}

pub(crate) fn migrate(
    config: &Path,
    workspace: &Path,
    project: bool,
    dry_run: bool,
) -> anyhow::Result<Report> {
    let base = if project {
        workspace.canonicalize()?.join(".craftsman")
    } else if config.exists() {
        config.canonicalize()?
    } else {
        config.to_path_buf()
    };
    let source = base.join("plugins");
    let paths = match crate::plugins::candidate_paths(&source) {
        Ok(paths) => paths,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => return Err(error.into()),
    };
    let mut report = Report {
        complete: true,
        project,
        dry_run,
        originals_preserved: true,
        model_opt_in_required: true,
        migrations: vec![],
    };
    let mut budget = MAX_TOTAL_BYTES;
    let mut slots: Vec<Option<(PathBuf, Plugin, usize)>> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (order, path) in paths.into_iter().enumerate() {
        if order >= super::manifest::MAX_GADGETS_PER_ROOT {
            report.migrations.push(Row {
                source: path,
                gadget: None,
                destination: None,
                status: "skipped-over-limit",
                error: None,
            });
            continue;
        }
        let Some(plugin) = crate::plugins::load_manifest(&path, &mut budget) else {
            report.migrations.push(Row {
                source: path,
                gadget: None,
                destination: None,
                status: "skipped-invalid-or-over-limit",
                error: None,
            });
            continue;
        };
        if let Some(&previous) = by_name.get(&plugin.name) {
            let (old_path, old_plugin, _): (PathBuf, Plugin, usize) =
                slots[previous].take().unwrap();
            report.migrations.push(Row {
                source: old_path,
                gadget: Some(gadget_id(&old_plugin.name)),
                destination: None,
                status: "shadowed-by-later-file",
                error: None,
            });
        }
        by_name.insert(plugin.name.clone(), slots.len());
        slots.push(Some((path, plugin, order)));
    }
    let (mut visible, root_error) = match visible_bundles(&base) {
        Ok(visible) => (visible, None),
        Err(error) => (0, Some(error.to_string())),
    };
    let mut converted_budget = MAX_TOTAL_BYTES as usize;
    for (path, plugin, order) in slots.into_iter().flatten() {
        let id = gadget_id(&plugin.name);
        let result = manifest_from_plugin(plugin, order).and_then(|m| {
            if let Some(error) = &root_error { anyhow::bail!("{error}"); }
            let size = toml::to_string_pretty(&m)?.len();
            anyhow::ensure!(size <= converted_budget, "converted manifests exceed the 512 KiB root budget; reduce/split them before retrying");
            converted_budget -= size;
            let existing = fs::symlink_metadata(base.join("gadgets").join(&m.id)).is_ok();
            anyhow::ensure!(existing || visible < super::manifest::MAX_GADGETS_PER_ROOT,
                "gadget root would exceed 64 visible bundles; no files were overwritten");
            let status = publish(&base, &m, dry_run)?;
            if matches!(status, "migrated" | "would-migrate") { visible += 1; }
            Ok(status)
        });
        let (status, error) = match result {
            Ok(status) => (status, None),
            Err(error) => {
                report.complete = false;
                ("conflict", Some(error.to_string()))
            }
        };
        report.migrations.push(Row {
            source: path,
            gadget: Some(id.clone()),
            destination: Some(base.join("gadgets").join(id).join("gadget.toml")),
            status,
            error,
        });
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_plugin(base: &Path, file: &str, value: serde_json::Value) -> Vec<u8> {
        fs::create_dir_all(base.join("plugins")).unwrap();
        let bytes = value.to_string().into_bytes();
        fs::write(base.join("plugins").join(file), &bytes).unwrap();
        bytes
    }
    fn plugin(name: &str, prompt: &str) -> serde_json::Value {
        json!({"version":1,"name":name,"prompt":prompt,
            "commands":[{"name":"review","prompt":"Review {$ARGS}"}],
            "agents":[{"name":"reviewer","description":"Read-only reviewer","system_prompt":"Inspect only","tools":["read_file","task","unknown"]}]})
    }

    #[test]
    fn migration_is_scoped_dry_run_create_only_idempotent_and_preserves_originals() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let original = write_plugin(
            config.path(),
            "review.json",
            plugin("review-flow", "User rules"),
        );
        write_plugin(
            &workspace.path().join(".craftsman"),
            "review.json",
            plugin("review-flow", "Project rules"),
        );
        let preview = migrate(config.path(), workspace.path(), false, true).unwrap();
        assert!(preview.complete);
        assert_eq!(preview.migrations[0].status, "would-migrate");
        assert!(!config.path().join("gadgets").exists());
        assert!(!workspace.path().join(".craftsman/gadgets").exists());
        let migrated = migrate(config.path(), workspace.path(), false, false).unwrap();
        assert!(migrated.complete);
        assert_eq!(migrated.migrations[0].status, "migrated");
        let target = config.path().join("gadgets/review-flow/gadget.toml");
        let before = fs::read(&target).unwrap();
        assert_eq!(
            fs::read(config.path().join("plugins/review.json")).unwrap(),
            original
        );
        assert!(
            !workspace.path().join(".craftsman/gadgets").exists(),
            "default scope is user only"
        );
        assert!(!config.path().join("gadgets/.grants.json").exists());
        assert!(!config.path().join("config.json").exists());
        let again = migrate(config.path(), workspace.path(), false, false).unwrap();
        assert_eq!(again.migrations[0].status, "already-migrated");
        assert_eq!(fs::read(&target).unwrap(), before);
        fs::write(
            &target,
            "id='review-flow'\nversion='9'\nprompt='User changes'\n",
        )
        .unwrap();
        let changed = fs::read(&target).unwrap();
        let conflict = migrate(config.path(), workspace.path(), false, false).unwrap();
        assert!(!conflict.complete);
        assert_eq!(conflict.migrations[0].status, "conflict");
        assert_eq!(fs::read(&target).unwrap(), changed);
        let project = migrate(config.path(), workspace.path(), true, false).unwrap();
        assert!(project.complete);
        assert!(
            workspace
                .path()
                .join(".craftsman/gadgets/review-flow/gadget.toml")
                .is_file()
        );
    }

    #[test]
    fn migration_preserves_effective_file_order_and_normalizes_legacy_names() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        // File order and id order disagree: the later JSON command must win.
        write_plugin(config.path(), "a.json", plugin("zzz", "First"));
        write_plugin(config.path(), "b.json", plugin("aaa", "Later"));
        write_plugin(config.path(), "c.json", plugin("a", "Single character"));
        write_plugin(config.path(), "d.json", plugin("0", "Digit leading"));
        write_plugin(
            config.path(),
            "e.json",
            plugin(&"long".repeat(30), "Long name"),
        );
        // A repeated name keeps the later file, exactly like the legacy loader.
        write_plugin(
            config.path(),
            "f.json",
            plugin("aaa", "Effective later definition"),
        );
        let report = migrate(config.path(), workspace.path(), false, false).unwrap();
        assert!(report.complete);
        assert!(
            report
                .migrations
                .iter()
                .any(|r| r.status == "shadowed-by-later-file")
        );
        for row in report.migrations.iter().filter(|r| r.destination.is_some()) {
            assert!(valid_id(row.gadget.as_deref().unwrap()));
        }
        let legacy = crate::extensions::discover_roots(config.path(), workspace.path(), false);
        let gadgets = crate::extensions::discover_roots(config.path(), workspace.path(), true);
        assert!(
            gadgets
                .iter()
                .all(|g| g.kind == crate::extensions::Kind::Gadget)
        );
        assert_eq!(
            legacy.iter().map(|p| &p.name).collect::<Vec<_>>(),
            gadgets.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
        assert_eq!(
            crate::extensions::expand_command(&legacy, "/review focus"),
            crate::extensions::expand_command(&gadgets, "/review focus")
        );
        let agents = crate::extensions::effective_agents(gadgets);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].tools, ["read_file", "task", "unknown"]);
    }

    #[test]
    fn migration_does_not_make_an_over_limit_gadget_root_look_successful() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(config.path().join("gadgets")).unwrap();
        for i in 0..64 {
            fs::create_dir(config.path().join(format!("gadgets/g{i:02}"))).unwrap();
        }
        write_plugin(config.path(), "a.json", plugin("new-one", "p"));
        for dry_run in [true, false] {
            let report = migrate(config.path(), workspace.path(), false, dry_run).unwrap();
            assert!(!report.complete);
            assert!(
                report.migrations[0]
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("64 visible")
            );
            assert!(!config.path().join("gadgets/new-one").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn migration_refuses_planted_output_symlinks() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_plugin(config.path(), "a.json", plugin("review-flow", "p"));
        std::os::unix::fs::symlink(outside.path(), config.path().join("gadgets")).unwrap();
        for dry_run in [true, false] {
            let report = migrate(config.path(), workspace.path(), false, dry_run).unwrap();
            assert!(!report.complete);
            assert_eq!(report.migrations[0].status, "conflict");
            assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn migration_reports_every_file_beyond_the_legacy_manifest_count_bound() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        for i in 0..70 {
            write_plugin(
                config.path(),
                &format!("p{i:02}.json"),
                plugin(&format!("p{i:02}"), "p"),
            );
        }
        let report = migrate(config.path(), workspace.path(), false, false).unwrap();
        assert!(report.complete);
        assert_eq!(
            report
                .migrations
                .iter()
                .filter(|r| r.status == "migrated")
                .count(),
            64
        );
        assert_eq!(
            report
                .migrations
                .iter()
                .filter(|r| r.status == "skipped-over-limit")
                .count(),
            6
        );
        assert_eq!(report.migrations.len(), 70);
        assert_eq!(
            fs::read_dir(config.path().join("plugins")).unwrap().count(),
            70
        );
    }
}
