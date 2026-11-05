//! Manifest-only declarative discovery. Never call describe, compile, or run a
//! component here; these are the same prompt-level surfaces as legacy plugins.

use super::authoring::regular_bytes;
use super::manifest::{MAX_COMPONENT_BYTES, MAX_GADGETS_PER_ROOT, MAX_MANIFEST_BYTES, Manifest};
use crate::extensions::Extension;
use std::fs;
use std::path::{Path, PathBuf};

/// Shared data-only parse for explicit no-follow bootstrap discovery. The
/// caller owns source IO/metadata bounds; this never instantiates a component.
pub(crate) fn parse_declarative(
    text: &str,
    id: &str,
) -> Result<(usize, Extension), super::GadgetError> {
    let manifest = Manifest::parse(text, id)?;
    let order = manifest.migration.as_ref().map_or(usize::MAX, |m| m.order);
    Ok((order, manifest.into_extension()))
}

pub(crate) const COMPONENT_LIMIT: usize = MAX_COMPONENT_BYTES;

fn directories(root: &Path) -> Option<Vec<PathBuf>> {
    let entries = fs::read_dir(root).ok()?;
    let mut paths = Vec::new();
    for (index, entry) in entries.enumerate() {
        // Hidden SDK/grant files do not count toward 64 visible bundles, but
        // even a root full of hidden junk must have bounded enumeration.
        if index >= crate::extensions::MAX_DIR_ENTRIES {
            return None;
        }
        let entry = entry.ok()?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        paths.push(entry.path());
        if paths.len() > MAX_GADGETS_PER_ROOT {
            return None;
        }
    }
    paths.sort();
    Some(paths)
}

fn load(directory: &Path, budget: &mut u64) -> Option<Manifest> {
    if *budget == 0 {
        return None;
    }
    let metadata = fs::symlink_metadata(directory).ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    let id = directory.file_name()?.to_str()?;
    // Do not let a malformed/symlinked component make a bundle appear valid.
    // Metadata inspection is not execution and needs no capability grant.
    match fs::symlink_metadata(directory.join("gadget.wasm")) {
        Ok(meta)
            if meta.is_file()
                && !meta.file_type().is_symlink()
                && meta.len() <= MAX_COMPONENT_BYTES as u64 => {}
        Ok(_) => return None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    let maximum = (MAX_MANIFEST_BYTES as u64).min(*budget) as usize;
    let bytes = match regular_bytes(&directory.join("gadget.toml"), maximum) {
        Ok(bytes) => {
            *budget = budget.saturating_sub(bytes.len() as u64);
            bytes
        }
        Err(_) => {
            *budget = budget.saturating_sub(maximum as u64);
            return None;
        }
    };
    Manifest::parse(std::str::from_utf8(&bytes).ok()?, id).ok()
}

pub(crate) fn discover_root(root: &Path, budget: &mut u64) -> Vec<Extension> {
    let mut loaded = Vec::new();
    for directory in directories(root).unwrap_or_default() {
        if let Some(manifest) = load(&directory, budget) {
            let order = manifest.migration.as_ref().map_or(usize::MAX, |m| m.order);
            loaded.push((order, directory, manifest));
        }
    }
    // Preserve legacy file order across migrated manifests, even when their
    // names differ from JSON filenames. Native gadgets follow in id order.
    loaded.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    loaded
        .into_iter()
        .map(|(_, _, manifest)| manifest.into_extension())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{self, Kind};
    use serde_json::{Value, json};

    fn manifest(id: &str, surfaces: Value) -> Manifest {
        let mut value = surfaces;
        value["id"] = json!(id);
        value["version"] = json!("0.1.0");
        let document: toml::Value = serde_json::from_value(value).unwrap();
        Manifest::parse(&toml::to_string(&document).unwrap(), id).unwrap()
    }
    fn write(base: &Path, id: &str, surfaces: Value) {
        let root = base.join("gadgets").join(id);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("gadget.toml"),
            toml::to_string_pretty(&manifest(id, surfaces)).unwrap(),
        )
        .unwrap();
    }
    fn migrate(value: Value, order: usize) -> extensions::Extension {
        let plugin = serde_json::from_value(value).unwrap();
        super::super::migration::manifest_from_plugin(plugin, order)
            .unwrap()
            .into_extension()
    }

    #[test]
    fn migrated_manifest_uses_the_legacy_command_agent_and_prompt_contracts() {
        let value = json!({"version":1,"name":"review-flow","prompt":"Report defects first.",
            "commands":[
                {"name":"review","description":"Review the diff","prompt":"Review {$ARGS} and {$ARGS}"},
                {"name":"ship","prompt":"Run checks"},
                {"name":"shell","prompt":"!rm -rf ~"},
                {"name":"slash","prompt":"/model other"},
                {"name":"help","prompt":"never reachable"}],
            "agents":[{"name":"reviewer","description":"Read-only reviewer","system_prompt":"Inspect only","tools":["read_file","grep"]}]});
        let legacy: crate::plugins::Plugin = serde_json::from_value(value.clone()).unwrap();
        let gadget = migrate(value, 0);
        for line in [
            "/review focus",
            "/review {$ARGS}",
            "  /review  ",
            "/ship --all",
            "/ship first\nsecond",
            "/shell",
            "/slash",
            "/nope",
            "/reviewer",
            "/Review",
            "/help",
            "/help now",
            "plain prompt",
            "/",
        ] {
            assert_eq!(
                extensions::expand_command(std::slice::from_ref(&legacy), line),
                extensions::expand_command(std::slice::from_ref(&gadget), line),
                "{line:?}"
            );
        }
        assert_eq!(
            extensions::effective_agents(vec![legacy.clone()]),
            extensions::effective_agents(vec![gadget.clone()])
        );
        assert_eq!(
            extensions::prompt_block(std::slice::from_ref(&legacy)),
            extensions::prompt_block(std::slice::from_ref(&gadget))
                .replace("# Gadget instructions", "# Plugin instructions")
        );
        assert_eq!(
            extensions::help_section(std::slice::from_ref(&legacy)),
            extensions::help_section(std::slice::from_ref(&gadget))
                .replace("Gadget commands", "Plugin commands")
                .replace("(gadget ", "(plugin ")
        );
        let normalize = |rows: Vec<(String, String)>| {
            rows.into_iter()
                .map(|(name, description)| {
                    let description = description.replace("(gadget ", "(plugin ");
                    (name, description)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            extensions::menu_entries(std::slice::from_ref(&legacy)),
            normalize(extensions::menu_entries(std::slice::from_ref(&gadget)))
        );
    }

    #[test]
    fn gadget_surfaces_reject_the_same_invalid_entries_as_plugins() {
        for surfaces in [
            json!({"commands":[{"name":"/x","prompt":"p"}]}),
            json!({"commands":[{"name":"","prompt":"p"}]}),
            json!({"commands":[{"name":"x","prompt":"  "}]}),
            json!({"commands":[{"name":"x","prompt":"{$ARGS}\n{$ARGS}"}]}),
            json!({"commands":[{"name":"good","prompt":"p"},{"name":"Bad","prompt":"p"}]}),
            json!({"agents":[{"name":"A","description":"d","system_prompt":"p"}]}),
            json!({"agents":[{"name":"general-purpose","description":"d","system_prompt":"p"}]}),
            json!({"agents":[{"name":"a","description":" ","system_prompt":"p"}]}),
            json!({"agents":[{"name":"a","description":"d","system_prompt":""}]}),
        ] {
            let mut legacy = surfaces.clone();
            legacy["version"] = json!(1);
            legacy["name"] = json!("review-flow");
            assert!(!crate::plugins::valid_plugin(
                &serde_json::from_value(legacy).unwrap()
            ));
            let mut value = surfaces;
            value["version"] = json!("0.1.0");
            value["id"] = json!("review-flow");
            let document: toml::Value = serde_json::from_value(value).unwrap();
            assert!(Manifest::parse(&toml::to_string(&document).unwrap(), "review-flow").is_err());
        }
    }

    #[test]
    fn project_precedence_and_retained_originals_do_not_duplicate_surfaces() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write(
            config.path(),
            "shared",
            json!({"prompt":"USER-GADGET","commands":[{"name":"go","prompt":"user gadget"}]}),
        );
        fs::create_dir_all(workspace.path().join(".craftsman/plugins")).unwrap();
        fs::write(workspace.path().join(".craftsman/plugins/shared.json"), json!({"version":1,"name":"shared","prompt":"PROJECT-PLUGIN","commands":[{"name":"go","prompt":"project plugin"}]}).to_string()).unwrap();
        let before = extensions::discover_roots(config.path(), workspace.path(), true);
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].kind, Kind::Plugin);
        assert_eq!(
            extensions::expand_command(&before, "/go").unwrap(),
            "project plugin"
        );
        super::super::migration::migrate(config.path(), workspace.path(), true, false).unwrap();
        let after = extensions::discover_roots(config.path(), workspace.path(), true);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].kind, Kind::Gadget);
        let prompt = extensions::prompt_block(&after);
        assert_eq!(prompt.matches("PROJECT-PLUGIN").count(), 1);
        assert!(!prompt.contains("USER-GADGET"));
        assert!(
            workspace
                .path()
                .join(".craftsman/plugins/shared.json")
                .is_file()
        );
    }

    #[test]
    fn retained_copies_do_not_spend_the_effective_budget_twice() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(config.path().join("plugins")).unwrap();
        for name in ["alpha", "beta", "gamma"] {
            fs::write(
                config.path().join("plugins").join(format!("{name}.json")),
                json!({"version":1,"name":name,"prompt":"x".repeat(200 * 1024)}).to_string(),
            )
            .unwrap();
        }
        let report =
            super::super::migration::migrate(config.path(), workspace.path(), false, false)
                .unwrap();
        assert!(report.complete);
        let gadgets = extensions::discover_roots(config.path(), workspace.path(), true);
        assert_eq!(gadgets.len(), 2);
        assert!(gadgets.iter().all(|g| g.kind == Kind::Gadget));
        assert!(extensions::prompt_block(&gadgets).len() < extensions::MAX_TOTAL_BYTES as usize);
    }

    #[test]
    fn manifest_only_commands_refresh_live_without_guest_execution() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let discover = || extensions::discover_roots(config.path(), workspace.path(), true);
        assert!(extensions::expand_command_with(discover, "/review").is_none());
        write(
            config.path(),
            "review-flow",
            json!({"commands":[{"name":"review","prompt":"first"}]}),
        );
        // Invalid code remains unexecuted; declarative data does not run it.
        fs::write(
            config.path().join("gadgets/review-flow/gadget.wasm"),
            b"not a component",
        )
        .unwrap();
        assert_eq!(
            extensions::expand_command_with(discover, "/review").unwrap(),
            "first"
        );
        write(
            config.path(),
            "review-flow",
            json!({"commands":[{"name":"review","prompt":"changed"}]}),
        );
        assert_eq!(
            extensions::expand_command_with(discover, "/review").unwrap(),
            "changed"
        );
        assert!(extensions::discover_roots(config.path(), workspace.path(), false).is_empty());
        assert!(!config.path().join("gadgets/.grants.json").exists());
    }

    #[test]
    fn command_display_bounds_and_escaping_apply_to_gadgets() {
        let many = (0..70)
            .map(|i| json!({"name":format!("c{i}"),"description":"x".repeat(10_000),"prompt":"p"}))
            .collect::<Vec<_>>();
        // Construct a validated surface directly: display bounds are independent
        // of the outer manifest's byte cap, like the legacy contract tests.
        let gadget = extensions::Extension {
            name: "review-flow".into(),
            kind: Kind::Gadget,
            prompt: None,
            commands: serde_json::from_value(json!(many)).unwrap(),
            agents: vec![],
        };
        let help = extensions::help_section(std::slice::from_ref(&gadget));
        assert_eq!(help.lines().count(), 66);
        assert!(help.ends_with("… and 6 more"));
        assert!(help.lines().all(|line| line.chars().count() < 300));
        let hostile = manifest("hostile", json!({"commands":[{"name":"build","description":"Build\u{1b}]0;PWNED\u{7}\n /help forged\r\n\u{9b}2J\tend","prompt":"p"}]})).into_extension();
        let menu = extensions::menu_entries(&[hostile]);
        assert!(!menu[0].1.chars().any(char::is_control));
    }

    #[tokio::test]
    async fn manifest_only_edits_change_the_catalog_without_changing_tool_definitions() {
        use craftsman_harness::{AgentMiddleware, AgentState};
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        write(
            config.path(),
            "review-flow",
            json!({"prompt":"first","commands":[{"name":"review","prompt":"first"}]}),
        );
        let runtime = std::sync::Arc::new(
            super::super::GadgetRuntime::new(
                workspace.path(),
                config.path(),
                super::super::RuntimeOptions {
                    disk_cache: false,
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        let definitions = runtime.tools().iter().map(|t| t.def()).collect::<Vec<_>>();
        let middleware = super::super::GadgetCatalogMiddleware::new(runtime.clone());
        let mut state = AgentState::default();
        middleware.prepare_model(&mut state).await.unwrap();
        middleware.prepare_model(&mut state).await.unwrap();
        assert_eq!(super::super::catalogue::notifications(&state), 1);
        // Counts, version, description and component stay unchanged. The raw
        // manifest hash must still notice changed prompts/command definitions.
        write(
            config.path(),
            "review-flow",
            json!({"prompt":"second","commands":[{"name":"review","prompt":"second"}]}),
        );
        middleware.prepare_model(&mut state).await.unwrap();
        assert_eq!(super::super::catalogue::notifications(&state), 2);
        assert_eq!(runtime.compiled_count(), 0);
        assert_eq!(
            runtime.tools().iter().map(|t| t.def()).collect::<Vec<_>>(),
            definitions
        );
    }

    #[test]
    fn shipped_legacy_example_migrates_to_the_gadget_example() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let plugin: crate::plugins::Plugin =
            serde_json::from_slice(&fs::read(root.join("plugins/review-flow.json")).unwrap())
                .unwrap();
        let generated = super::super::migration::manifest_from_plugin(plugin, 0).unwrap();
        let shipped = Manifest::parse(
            &fs::read_to_string(root.join("gadgets/review-flow/gadget.toml")).unwrap(),
            "review-flow",
        )
        .unwrap();
        assert_eq!(generated, shipped);
    }
}
