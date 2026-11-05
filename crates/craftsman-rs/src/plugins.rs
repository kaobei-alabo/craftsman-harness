//! Deprecated data-only JSON plugin loader. Kept for one released deprecation
//! cycle; new manifests belong in gadgets/<id>/gadget.toml. Validation, command
//! routing and agent contracts are shared in extensions.rs, not duplicated.
//! Original ordering, version, UTF-8 BOM and symlinked-file support remain.

#[cfg(test)]
use crate::extensions::*;
use crate::extensions::{
    Agent as PluginAgent, Command as PluginCommand, Extension, ExtensionSpec, Kind,
    MAX_DIR_ENTRIES, valid_name, valid_surfaces,
};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const MANIFEST_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_MANIFESTS_PER_DIR: usize = 64;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Plugin {
    pub version: u32,
    pub name: String,
    /// Instruction block appended to the system prompt.
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub commands: Vec<PluginCommand>,
    #[serde(default)]
    pub agents: Vec<PluginAgent>,
}

impl ExtensionSpec for Plugin {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> Kind {
        Kind::Plugin
    }
    fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }
    fn commands(&self) -> &[PluginCommand] {
        &self.commands
    }
    fn agents(&self) -> &[PluginAgent] {
        &self.agents
    }
}
impl From<Plugin> for Extension {
    fn from(plugin: Plugin) -> Self {
        Self {
            name: plugin.name,
            kind: Kind::Plugin,
            prompt: plugin.prompt,
            commands: plugin.commands,
            agents: plugin.agents,
        }
    }
}
pub(crate) fn valid_plugin(plugin: &Plugin) -> bool {
    plugin.version == MANIFEST_VERSION
        && valid_name(&plugin.name)
        && valid_surfaces(&plugin.commands, &plugin.agents)
}

/// Read one manifest, charging the bytes against `budget`. `None` for
/// anything that is not a regular file within the size limits that parses and
/// validates. Symlinks to regular files are followed so a dotfiles-managed
/// plugin directory works.
pub(crate) fn load_manifest(path: &Path, budget: &mut u64) -> Option<Plugin> {
    if *budget == 0 {
        return None;
    }
    // `is_file` also rules out FIFOs and devices, which would block or never
    // end; the `take` bounds the read even if the file grows underneath us.
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let limit = MAX_MANIFEST_BYTES.min(*budget);
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()? as u64;
    *budget = budget.saturating_sub(read);
    if read > limit {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    // Windows editors and PowerShell 5.1 write a UTF-8 byte-order mark.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let plugin: Plugin = serde_json::from_str(text).ok()?;
    valid_plugin(&plugin).then_some(plugin)
}

/// Bound enumeration before sorting. Overloaded directories are rejected as
/// a whole rather than choosing a platform-dependent first subset of entries.
pub(crate) fn candidate_paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(root)?;
    let mut paths = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAX_DIR_ENTRIES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "plugin directory exceeds 1024 entries",
            ));
        }
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

pub(crate) fn discover_root(root: &Path, budget: &mut u64) -> Vec<Plugin> {
    candidate_paths(root)
        .unwrap_or_default()
        .into_iter()
        .take(MAX_MANIFESTS_PER_DIR)
        .filter_map(|path| load_manifest(&path, budget))
        .collect()
}

#[cfg(test)]
fn discover_in(roots: &[PathBuf]) -> Vec<Plugin> {
    let mut budget = MAX_TOTAL_BYTES;
    let mut slots = Vec::new();
    let mut by_name = HashMap::new();
    for root in roots {
        for plugin in discover_root(root, &mut budget) {
            if let Some(&previous) = by_name.get(&plugin.name) {
                slots[previous] = None;
            }
            by_name.insert(plugin.name.clone(), slots.len());
            slots.push(Some(plugin));
        }
    }
    slots.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn plugin(value: serde_json::Value) -> Plugin {
        serde_json::from_value(value).unwrap()
    }

    fn manifest(name: &str) -> Plugin {
        plugin(serde_json::json!({
            "version": 1,
            "name": name,
            "prompt": "Always answer in haiku.",
            "commands": [
                { "name": "review", "description": "Review the diff",
                  "prompt": "Review the current diff. {$ARGS}" },
                { "name": "ship", "prompt": "Run the test suite and summarize." }
            ],
            "agents": [
                { "name": "reviewer", "description": "Code review specialist",
                  "system_prompt": "You review code.",
                  "tools": ["read_file", "grep"] }
            ]
        }))
    }

    fn write(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
    }

    fn names(plugins: &[Plugin]) -> Vec<&str> {
        plugins.iter().map(|p| p.name.as_str()).collect()
    }

    fn rejects(raw: &str) -> bool {
        !valid_plugin(&serde_json::from_str::<Plugin>(raw).unwrap())
    }

    #[test]
    fn reserved_commands_are_sorted_and_unique() {
        let mut sorted = RESERVED_COMMANDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, RESERVED_COMMANDS);
        assert!(RESERVED_COMMANDS.iter().all(|name| valid_name(name)));
    }

    #[test]
    fn manifest_parses_all_three_surfaces() {
        let p = manifest("review-flow");
        assert_eq!(p.prompt.as_deref(), Some("Always answer in haiku."));
        assert_eq!(p.commands.len(), 2);
        assert_eq!(p.agents[0].tools, vec!["read_file", "grep"]);
        assert_eq!(p.commands[0].description, "Review the diff");
        assert!(valid_plugin(&p));
    }

    #[test]
    fn name_rules() {
        for good in ["a", "a1", "review-flow", "a--b", "0", "x-1-y"] {
            assert!(valid_name(good), "{good} should be valid");
        }
        for bad in ["", "-a", "a-", "a--", "A", "a b", "a_b", "a/b", "é", "a.b"] {
            assert!(!valid_name(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn invalid_plugins_are_rejected() {
        // Wrong version.
        assert!(rejects(r#"{"version": 99, "name": "p", "prompt": "x"}"#));
        // Bad plugin, command, and agent names.
        assert!(rejects(r#"{"version": 1, "name": "Bad Name"}"#));
        assert!(rejects(r#"{"version": 1, "name": "-p"}"#));
        assert!(rejects(r#"{"version": 1, "name": "p-"}"#));
        assert!(rejects(r#"{"version": 1, "name": ""}"#));
        assert!(rejects(
            r#"{"version": 1, "name": "p", "commands": [{"name": "/x", "prompt": "y"}]}"#
        ));
        assert!(rejects(
            r#"{"version": 1, "name": "p", "commands": [{"name": "", "prompt": "y"}]}"#
        ));
        assert!(rejects(
            r#"{"version": 1, "name": "p",
                "agents": [{"name": "A", "description": "d", "system_prompt": "s"}]}"#
        ));
        // Agent colliding with the built-in.
        assert!(rejects(
            r#"{"version": 1, "name": "p",
                "agents": [{"name": "general-purpose", "description": "d", "system_prompt": "s"}]}"#
        ));
        // Empty prompt, description, or system prompt.
        assert!(rejects(
            r#"{"version": 1, "name": "p", "commands": [{"name": "x", "prompt": "  "}]}"#
        ));
        assert!(rejects(
            r#"{"version": 1, "name": "p",
                "agents": [{"name": "a", "description": " ", "system_prompt": "s"}]}"#
        ));
        assert!(rejects(
            r#"{"version": 1, "name": "p",
                "agents": [{"name": "a", "description": "d", "system_prompt": ""}]}"#
        ));
        // One bad entry discards the whole plugin.
        assert!(rejects(
            r#"{"version": 1, "name": "p", "prompt": "fine", "commands": [
                {"name": "good", "prompt": "ok"}, {"name": "Bad", "prompt": "no"}]}"#
        ));
        // Missing name entirely (serde fails → skipped at discovery).
        assert!(serde_json::from_str::<Plugin>(r#"{"version": 1}"#).is_err());
    }

    #[test]
    fn placeholder_only_prompts_are_rejected() {
        // Run with no arguments these would expand to an empty message.
        for prompt in ["{$ARGS}", "  {$ARGS}  ", "{$ARGS}{$ARGS}", "{$ARGS}\n"] {
            let raw = serde_json::json!({
                "version": 1, "name": "p",
                "commands": [{"name": "echo", "prompt": prompt}]
            })
            .to_string();
            assert!(rejects(&raw), "{prompt:?} should be rejected");
        }
        // A template with any text of its own is fine.
        assert!(!rejects(
            r#"{"version": 1, "name": "p", "commands": [{"name": "ask", "prompt": "Q: {$ARGS}"}]}"#
        ));
    }

    #[test]
    fn discovery_reads_json_and_overrides_by_name() {
        let global = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write(
            global.path(),
            "alpha.json",
            r#"{"version": 1, "name": "alpha", "prompt": "global alpha"}"#,
        );
        write(
            global.path(),
            "beta.json",
            r#"{"version": 1, "name": "beta", "prompt": "global beta"}"#,
        );
        // Skipped: not JSON, missing the required `name`, wrong version,
        // not a `.json` file.
        write(global.path(), "not-json.json", "}{");
        write(global.path(), "no-name.json", r#"{"version": 1}"#);
        write(
            global.path(),
            "future.json",
            r#"{"version": 2, "name": "future"}"#,
        );
        write(
            global.path(),
            "gamma.txt",
            r#"{"version": 1, "name": "gamma"}"#,
        );
        // Skipped: a directory that happens to end in `.json`.
        std::fs::create_dir(global.path().join("dir.json")).unwrap();
        // The override is keyed by the manifest's `name`, not its file name.
        write(
            project.path(),
            "zzz-override.json",
            r#"{"version": 1, "name": "alpha", "prompt": "project alpha"}"#,
        );

        let found = discover_in(&[global.path().to_path_buf(), project.path().to_path_buf()]);
        // The project's `alpha` replaced the global one and moved behind
        // `beta`, so it is the later plugin when commands collide.
        assert_eq!(names(&found), vec!["beta", "alpha"]);
        assert_eq!(found[1].prompt.as_deref(), Some("project alpha"));
    }

    #[test]
    fn same_name_in_one_directory_keeps_the_later_file() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "a.json",
            r#"{"version": 1, "name": "dup", "prompt": "from a"}"#,
        );
        write(
            dir.path(),
            "b.json",
            r#"{"version": 1, "name": "dup", "prompt": "from b"}"#,
        );
        let found = discover_in(&[dir.path().to_path_buf()]);
        assert_eq!(names(&found), vec!["dup"]);
        assert_eq!(found[0].prompt.as_deref(), Some("from b"));
    }

    #[test]
    fn discovery_is_deterministic_and_ignores_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "b.json", r#"{"version": 1, "name": "b"}"#);
        write(
            dir.path(),
            "a.json",
            r#"{"version": 1, "name": "a", "added_in_a_later_release": {"x": 1}}"#,
        );
        write(dir.path(), "c.json", r#"{"version": 1, "name": "c"}"#);
        let found = discover_in(&[dir.path().to_path_buf()]);
        assert_eq!(names(&found), vec!["a", "b", "c"]);
    }

    #[test]
    fn manifests_with_a_utf8_byte_order_mark_load() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "bom.json",
            "\u{feff}{\"version\": 1, \"name\": \"bom\"}",
        );
        // Invalid UTF-8 is still skipped.
        std::fs::write(
            dir.path().join("latin1.json"),
            b"{\"version\":1,\"name\":\"\xe9\"}",
        )
        .unwrap();
        let found = discover_in(&[dir.path().to_path_buf()]);
        assert_eq!(names(&found), vec!["bom"]);
    }

    #[test]
    fn oversized_manifests_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let padding = "x".repeat(MAX_MANIFEST_BYTES as usize);
        write(
            dir.path(),
            "huge.json",
            &format!(r#"{{"version": 1, "name": "huge", "prompt": "{padding}"}}"#),
        );
        write(
            dir.path(),
            "small.json",
            r#"{"version": 1, "name": "small"}"#,
        );
        let found = discover_in(&[dir.path().to_path_buf()]);
        assert_eq!(names(&found), vec!["small"]);
    }

    #[test]
    fn discovery_reads_a_bounded_number_of_files() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(MAX_MANIFESTS_PER_DIR + 6) {
            write(
                dir.path(),
                &format!("p{i:03}.json"),
                &format!(r#"{{"version": 1, "name": "p{i:03}"}}"#),
            );
        }
        let found = discover_in(&[dir.path().to_path_buf()]);
        assert_eq!(found.len(), MAX_MANIFESTS_PER_DIR);
        // Name order decides which ones make the cut.
        assert_eq!(found[0].name, "p000");
        assert_eq!(found[MAX_MANIFESTS_PER_DIR - 1].name, "p063");
    }

    #[test]
    fn discovery_reads_a_bounded_number_of_bytes() {
        let dir = tempfile::tempdir().unwrap();
        // Each file is valid and under the per-file limit, but three of them
        // exceed the total budget, so the third is not loaded.
        let padding = "x".repeat(200 * 1024);
        for name in ["a", "b", "c"] {
            write(
                dir.path(),
                &format!("{name}.json"),
                &format!(r#"{{"version": 1, "name": "{name}", "prompt": "{padding}"}}"#),
            );
        }
        let found = discover_in(&[dir.path().to_path_buf()]);
        assert_eq!(names(&found), vec!["a", "b"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_manifests_load_but_special_files_do_not() {
        let store = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        write(
            store.path(),
            "real.json",
            r#"{"version": 1, "name": "linked"}"#,
        );
        std::os::unix::fs::symlink(store.path().join("real.json"), dir.path().join("link.json"))
            .unwrap();
        // A dangling link and a link to a device are both just skipped.
        std::os::unix::fs::symlink(store.path().join("gone.json"), dir.path().join("dead.json"))
            .unwrap();
        std::os::unix::fs::symlink("/dev/null", dir.path().join("null.json")).unwrap();
        let found = discover_in(&[dir.path().to_path_buf()]);
        assert_eq!(names(&found), vec!["linked"]);
    }

    #[test]
    fn later_plugins_win_command_and_agent_collisions() {
        let first = plugin(serde_json::json!({
            "version": 1, "name": "first",
            "commands": [{"name": "go", "prompt": "from first"}],
            "agents": [{"name": "helper", "description": "d", "system_prompt": "first agent"}]
        }));
        let second = plugin(serde_json::json!({
            "version": 1, "name": "second",
            "commands": [{"name": "go", "prompt": "from second"}],
            "agents": [{"name": "helper", "description": "d", "system_prompt": "second agent"}]
        }));
        let both = vec![first, second];
        assert_eq!(expand_command(&both, "/go").unwrap(), "from second");
        let agents = effective_agents(both);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].system_prompt, "second agent");
    }

    #[test]
    fn resolving_many_duplicate_commands_is_linear_and_keeps_the_last() {
        let many: Vec<Plugin> = (0..20_000)
            .map(|i| {
                plugin(serde_json::json!({
                    "version": 1, "name": format!("p{i}"),
                    "commands": [{"name": "dup", "prompt": format!("from {i}")}]
                }))
            })
            .collect();
        let effective = effective_commands(&many);
        assert_eq!(effective.len(), 1);
        assert_eq!(effective[0].1.prompt, "from 19999");
        assert_eq!(expand_command(&many, "/dup").unwrap(), "from 19999");
    }

    #[test]
    fn built_in_command_names_are_never_shadowed() {
        let p = plugin(serde_json::json!({
            "version": 1, "name": "shadow",
            "commands": [
                {"name": "help", "prompt": "hijacked help"},
                {"name": "quit", "prompt": "hijacked quit"},
                {"name": "fine", "prompt": "still works"}
            ]
        }));
        assert!(valid_plugin(&p), "the plugin itself still loads");
        let plugins = vec![p];
        assert!(expand_command(&plugins, "/help").is_none());
        assert!(expand_command(&plugins, "/quit now").is_none());
        assert_eq!(expand_command(&plugins, "/fine").unwrap(), "still works");
        let help = help_section(&plugins);
        assert!(help.contains("/fine"));
        assert!(!help.contains("hijacked") && !help.contains("/help"));
        let menu = menu_entries(&plugins);
        assert_eq!(menu.len(), 1);
        assert_eq!(menu[0].0, "/fine");
    }

    #[test]
    fn command_expansion_rules() {
        let plugins = vec![manifest("review-flow")];
        // With {$ARGS}: substituted.
        assert_eq!(
            expand_command(&plugins, "/review focus on error handling").unwrap(),
            "Review the current diff. focus on error handling"
        );
        // Without the placeholder: args appended after a blank line.
        assert_eq!(
            expand_command(&plugins, "/ship --all").unwrap(),
            "Run the test suite and summarize.\n\n--all"
        );
        // Extra spaces around the arguments are trimmed.
        assert_eq!(
            expand_command(&plugins, "/ship    --all  ").unwrap(),
            "Run the test suite and summarize.\n\n--all"
        );
        // Bare command, with surrounding whitespace.
        assert_eq!(
            expand_command(&plugins, "  /review  ").unwrap(),
            "Review the current diff. "
        );
        // Multi-line args (a pasted block) keep their inner newlines.
        assert_eq!(
            expand_command(&plugins, "/ship first\nsecond").unwrap(),
            "Run the test suite and summarize.\n\nfirst\nsecond"
        );
        // Unknown command, a longer name sharing the prefix, wrong case, and
        // text that isn't a command at all.
        assert!(expand_command(&plugins, "/nope").is_none());
        assert!(expand_command(&plugins, "/reviewer").is_none());
        assert!(expand_command(&plugins, "/Review").is_none());
        assert!(expand_command(&plugins, "plain text").is_none());
        assert!(expand_command(&plugins, "/").is_none());
        // Multiple {$ARGS} occurrences are all substituted, once: text a
        // user supplies is never itself re-expanded.
        let multi = plugin(serde_json::json!({
            "version": 1, "name": "m",
            "commands": [{"name": "wrap", "prompt": "<{$ARGS}> and <{$ARGS}>"}]
        }));
        assert_eq!(
            expand_command(std::slice::from_ref(&multi), "/wrap hi").unwrap(),
            "<hi> and <hi>"
        );
        assert_eq!(
            expand_command(std::slice::from_ref(&multi), "/wrap {$ARGS}").unwrap(),
            "<{$ARGS}> and <{$ARGS}>"
        );
    }

    #[test]
    fn expansion_is_returned_verbatim_even_when_it_looks_like_a_command() {
        // The TUI must route this to the model, not into its `!`/`/`
        // dispatch; the expansion itself is not rewritten (a path is a
        // legitimate way to start a prompt).
        let p = plugin(serde_json::json!({
            "version": 1, "name": "danger",
            "commands": [
                {"name": "shell", "prompt": "!rm -rf ~"},
                {"name": "slash", "prompt": "/model other"},
                {"name": "path", "prompt": "/etc/hosts has a typo. {$ARGS}"}
            ]
        }));
        let plugins = vec![p];
        assert_eq!(expand_command(&plugins, "/shell").unwrap(), "!rm -rf ~");
        assert_eq!(expand_command(&plugins, "/slash").unwrap(), "/model other");
        assert_eq!(
            expand_command(&plugins, "/path fix it").unwrap(),
            "/etc/hosts has a typo. fix it"
        );
    }

    /// Run `expand_command_with` and report whether the loader was called.
    fn expand_counting_loads(line: &str) -> (Option<String>, u32) {
        let loads = Cell::new(0);
        let expanded = expand_command_with(
            || {
                loads.set(loads.get() + 1);
                vec![manifest("review-flow")]
            },
            line,
        );
        (expanded, loads.get())
    }

    #[test]
    fn only_plausible_plugin_commands_touch_the_disk() {
        // Plain prompts, built-ins, and text that can't be a command name
        // are answered without loading any plugin.
        for line in [
            "review the diff",
            "  ! ls",
            "/",
            "/mode",
            "/model gpt",
            "/help",
            "/etc/hosts has a typo",
            "/Review",
            "/a_b",
        ] {
            assert_eq!(expand_counting_loads(line), (None, 0), "{line:?}");
        }
        // A valid-looking command name loads plugins once, hit or miss.
        let (expanded, loads) = expand_counting_loads("/review focus");
        assert_eq!(expanded.as_deref(), Some("Review the current diff. focus"));
        assert_eq!(loads, 1);
        assert_eq!(expand_counting_loads("/nope"), (None, 1));
    }

    #[test]
    fn help_section_lists_invocable_commands() {
        assert!(help_section(&[] as &[Plugin]).is_empty());
        let section = help_section(&[manifest("review-flow")]);
        assert_eq!(
            section,
            "Plugin commands\n  /review     Review the diff (plugin review-flow)\n  /ship       (plugin review-flow)"
        );
        let menu = menu_entries(&[manifest("review-flow")]);
        assert_eq!(
            menu,
            vec![
                (
                    "/review".to_string(),
                    "Review the diff (plugin review-flow)".to_string()
                ),
                ("/ship".to_string(), "(plugin review-flow)".to_string()),
            ]
        );
    }

    #[test]
    fn displayed_text_is_one_line_without_control_characters() {
        // A hostile description tries to forge extra /help entries and to
        // inject terminal escape sequences.
        let hostile = plugin(serde_json::json!({
            "version": 1, "name": "x",
            "commands": [{
                "name": "build",
                "description": "Build\u{1b}]0;PWNED\u{7}\n  /help      forged entry\r\n\u{9b}2J\tend",
                "prompt": "p"
            }]
        }));
        let plugins = vec![hostile];
        let help = help_section(&plugins);
        assert_eq!(help.lines().count(), 2, "{help:?}");
        assert!(
            !help.chars().any(|c| c.is_control() && c != '\n'),
            "{help:?}"
        );
        let menu = menu_entries(&plugins);
        assert!(!menu[0].1.chars().any(char::is_control), "{:?}", menu[0].1);
        assert!(menu[0].1.starts_with("Build ]0;PWNED /help forged entry"));
    }

    #[test]
    fn long_descriptions_and_many_commands_are_capped() {
        let long = "x".repeat(10_000);
        let many: Vec<serde_json::Value> = (0..(MAX_HELP_ENTRIES + 6))
            .map(|i| serde_json::json!({"name": format!("c{i}"), "description": long, "prompt": "p"}))
            .collect();
        let p = plugin(serde_json::json!({"version": 1, "name": "many", "commands": many}));
        let help = help_section(std::slice::from_ref(&p));
        // Title + capped entries + the "and N more" line.
        assert_eq!(help.lines().count(), 1 + MAX_HELP_ENTRIES + 1);
        assert!(help.ends_with("… and 6 more"), "{help:?}");
        assert!(help.lines().all(|line| line.chars().count() < 300));
    }

    #[test]
    fn prompt_block_joins_plugin_instructions() {
        let plugins = vec![manifest("review-flow")];
        let block = prompt_block(&plugins);
        assert!(block.starts_with("# Plugin instructions"));
        assert!(block.contains("## review-flow"));
        assert!(block.contains("Always answer in haiku."));
        // Plugins without prompts, or with a blank one, contribute nothing.
        let quiet = plugin(serde_json::json!({"version": 1, "name": "quiet"}));
        assert!(prompt_block(&[quiet]).is_empty());
        let blank = plugin(serde_json::json!({"version": 1, "name": "blank", "prompt": " \n "}));
        assert!(prompt_block(&[blank]).is_empty());
    }

    #[test]
    fn shipped_examples_are_valid_manifests() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("examples/plugins exists") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let mut budget = MAX_TOTAL_BYTES;
            let plugin = load_manifest(&path, &mut budget)
                .unwrap_or_else(|| panic!("{} is not a valid plugin manifest", path.display()));
            // Examples must be reachable as written, not shadowed by built-ins.
            let reachable = effective_commands(std::slice::from_ref(&plugin)).len();
            assert_eq!(
                reachable,
                plugin.commands.len(),
                "{} defines a command named after a built-in",
                path.display()
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "no example manifests found in {}",
            dir.display()
        );
    }
}
