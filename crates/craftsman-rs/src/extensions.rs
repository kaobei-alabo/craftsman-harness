//! Shared declarative extension contracts: gadgets and the deprecated JSON loader.
//! Command expansions are always model input; agents retain SubagentGate.
//! This module reads data only. It never instantiates a WASM component.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) const MAX_TOTAL_BYTES: u64 = 512 * 1024;
pub(crate) const MAX_DIR_ENTRIES: usize = 1024;
pub(crate) const MAX_HELP_ENTRIES: usize = 64;
const MAX_MENU_ENTRIES: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 200;
const ARGS_PLACEHOLDER: &str = "{$ARGS}";

/// Built-ins always win over both gadget and legacy plugin commands.
/// tui/commands.rs tests every built-in against this list.
pub const RESERVED_COMMANDS: &[&str] = &[
    "agents",
    "checkpoints",
    "clear",
    "compact",
    "config",
    "copy",
    "diff",
    "doctor",
    "editor",
    "exit",
    "gadgets",
    "harness",
    "help",
    "lsp",
    "mcp",
    "memories",
    "mode",
    "model",
    "queue",
    "quit",
    "rewind",
    "session",
    "setup",
    "skills",
    "status",
    "tasks",
    "tokens",
    "version",
    "voice",
];

pub(crate) fn is_reserved(name: &str) -> bool {
    RESERVED_COMMANDS.contains(&name)
}

/// `[a-z0-9]([a-z0-9-]*[a-z0-9])?` — a safe single-word identifier.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Command {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Replace {$ARGS} once, or append trailing arguments after a blank line.
    pub prompt: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Agent {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    /// Resolve against the shared registry; no task/send_message recursion.
    /// All gated tools must still be wrapped in SubagentGate.
    #[serde(default)]
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Plugin,
    #[cfg(feature = "gadgets")]
    Gadget,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            #[cfg(feature = "gadgets")]
            Self::Gadget => "gadget",
        }
    }
}

pub(crate) trait ExtensionSpec {
    fn name(&self) -> &str;
    fn kind(&self) -> Kind;
    fn prompt(&self) -> Option<&str>;
    fn commands(&self) -> &[Command];
    fn agents(&self) -> &[Agent];
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Extension {
    /// Merge identity; migrated gadgets keep the original legacy name here.
    pub name: String,
    #[serde(skip)]
    pub kind: Kind,
    pub prompt: Option<String>,
    pub commands: Vec<Command>,
    pub agents: Vec<Agent>,
}
impl ExtensionSpec for Extension {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> Kind {
        self.kind
    }
    fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }
    fn commands(&self) -> &[Command] {
        &self.commands
    }
    fn agents(&self) -> &[Agent] {
        &self.agents
    }
}

pub(crate) fn valid_surfaces(commands: &[Command], agents: &[Agent]) -> bool {
    commands.iter().all(|command| {
        valid_name(&command.name)
            && !command
                .prompt
                .replace(ARGS_PLACEHOLDER, "")
                .trim()
                .is_empty()
    }) && agents.iter().all(|agent| {
        valid_name(&agent.name)
            && !agent.description.trim().is_empty()
            && !agent.system_prompt.trim().is_empty()
            && agent.name != "general-purpose"
    })
}

fn title<S: ExtensionSpec>(extensions: &[S]) -> &'static str {
    if extensions.iter().all(|e| e.kind() == Kind::Plugin) {
        return "Plugin";
    }
    #[cfg(feature = "gadgets")]
    if extensions.iter().all(|e| e.kind() == Kind::Gadget) {
        return "Gadget";
    }
    "Extension"
}

/// One warning per session using effective legacy manifests, not per disk scan.
/// Removal is a later breaking release, after this deprecation has shipped.
#[cfg(feature = "gadgets")]
pub(crate) const DEPRECATION_WARNING: &str = "Legacy plugins are deprecated; run craftsman gadgets migrate (add --project for project plugins) and enable gadgets.enabled to use the migrated gadgets. Originals are retained for this release.";

#[cfg(not(feature = "gadgets"))]
pub(crate) const DEPRECATION_WARNING: &str = "Legacy plugins are deprecated; rebuild with the gadgets Cargo feature before using craftsman gadgets migrate. Originals are retained for this release.";

pub(crate) fn terminal_gadgets_enabled(cfg: &crate::config::Config) -> bool {
    #[cfg(feature = "gadgets")]
    {
        crate::gadgets::cli::enabled(cfg)
    }
    #[cfg(not(feature = "gadgets"))]
    {
        let _ = cfg;
        false
    }
}

pub(crate) fn discover_at(workspace: &Path, include_gadgets: bool) -> Vec<Extension> {
    discover_roots(&crate::config::config_dir(), workspace, include_gadgets)
}

/// User plugin < user gadget < project plugin < project gadget. A gadget
/// overrides its retained migration original without duplicating instructions.
/// Each loader has a bounded read budget. After coalescing retained originals,
/// effective declarative surfaces share the legacy 512 KiB context budget.
pub(crate) fn discover_roots(
    config: &Path,
    workspace: &Path,
    include_gadgets: bool,
) -> Vec<Extension> {
    let mut plugin_budget = MAX_TOTAL_BYTES;
    #[cfg(feature = "gadgets")]
    let mut gadget_budget = MAX_TOTAL_BYTES;
    #[cfg(not(feature = "gadgets"))]
    let _ = include_gadgets;
    let mut sources = Vec::new();
    for base in [config.to_path_buf(), workspace.join(".craftsman")] {
        #[allow(unused_mut)]
        let mut found: Vec<Extension> =
            crate::plugins::discover_root(&base.join("plugins"), &mut plugin_budget)
                .into_iter()
                .map(Into::into)
                .collect();
        #[cfg(feature = "gadgets")]
        if include_gadgets {
            found.extend(crate::gadgets::surfaces::discover_root(
                &base.join("gadgets"),
                &mut gadget_budget,
            ));
        }
        sources.push(found);
    }
    merge_discovery(sources, include_gadgets)
}

/// Pure merge shared with explicit bootstrap discovery. Later source groups
/// win by identity, preserving migration ordering and effective context bounds.
pub(crate) fn merge_discovery(
    sources: impl IntoIterator<Item = Vec<Extension>>,
    bound_effective_context: bool,
) -> Vec<Extension> {
    let mut slots = Vec::new();
    let mut by_name = HashMap::new();
    for group in sources {
        for extension in group {
            if let Some(&previous) = by_name.get(&extension.name) {
                slots[previous] = None;
            }
            by_name.insert(extension.name.clone(), slots.len());
            slots.push(Some(extension));
        }
    }
    let found: Vec<Extension> = slots.into_iter().flatten().collect();
    if !bound_effective_context {
        return found;
    }
    let mut budget = MAX_TOTAL_BYTES as usize;
    found
        .into_iter()
        .filter(|extension| {
            let size = serde_json::to_vec(extension)
                .expect("extension serialization")
                .len();
            if size > budget {
                return false;
            }
            budget -= size;
            true
        })
        .collect()
}

fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The commands that can actually be invoked: one per name (later extensions
/// win) and never a built-in's name. Linear in the number of commands.
pub(crate) fn effective_commands<S: ExtensionSpec>(plugins: &[S]) -> Vec<(&S, &Command)> {
    let mut slots: Vec<Option<(&S, &Command)>> = Vec::new();
    let mut by_name: HashMap<&str, usize> = HashMap::new();
    for plugin in plugins {
        for command in plugin.commands() {
            if is_reserved(&command.name) {
                continue;
            }
            if let Some(&earlier) = by_name.get(command.name.as_str()) {
                slots[earlier] = None;
            }
            by_name.insert(command.name.as_str(), slots.len());
            slots.push(Some((plugin, command)));
        }
    }
    slots.into_iter().flatten().collect()
}

/// The subagent definitions to compile: one per name, later extensions win.
/// `general-purpose` can't appear — validation rejects it.
pub(crate) fn effective_agents<S: ExtensionSpec>(plugins: Vec<S>) -> Vec<Agent> {
    let mut slots: Vec<Option<Agent>> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for plugin in plugins {
        for agent in plugin.agents() {
            if let Some(&earlier) = by_name.get(&agent.name) {
                slots[earlier] = None;
            }
            by_name.insert(agent.name.clone(), slots.len());
            slots.push(Some(agent.clone()));
        }
    }
    slots.into_iter().flatten().collect()
}

/// Split `/name rest of the line` into `("name", "rest of the line")`.
fn split_command(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim().strip_prefix('/')?;
    Some(match rest.split_once(char::is_whitespace) {
        Some((name, args)) => (name, args.trim()),
        None => (rest, ""),
    })
}

/// Expand an extension slash command into the prompt it submits. `None` when
/// the line isn't a known extension command — callers fall through to their
/// built-in slash handling untouched.
///
/// `/name arg text…` → the command's prompt with the args substituted at
/// `{$ARGS}` (all occurrences), or appended after a blank line when the
/// placeholder is absent.
///
/// The result is model input only. It may legitimately start with `/` or
/// `!` (a path, say), so callers must hand it to the turn directly rather
/// than back through their `!`/`/` dispatch.
pub(crate) fn expand_command<S: ExtensionSpec>(plugins: &[S], line: &str) -> Option<String> {
    let (name, args) = split_command(line)?;
    if is_reserved(name) {
        return None;
    }
    // The later extension (and the later command within it) wins.
    let command = plugins
        .iter()
        .rev()
        .flat_map(|plugin| plugin.commands().iter().rev())
        .find(|command| command.name == name)?;
    if command.prompt.contains(ARGS_PLACEHOLDER) {
        return Some(command.prompt.replace(ARGS_PLACEHOLDER, args));
    }
    if args.is_empty() {
        return Some(command.prompt.clone());
    }
    Some(format!("{}\n\n{}", command.prompt, args))
}

/// [`expand_command`], loading plugins only for a line that could name a
/// extension command: built-in names, anything that is not a valid command name
/// (a pasted path, say) and plain prompts never touch the disk.
pub(crate) fn expand_command_with<S: ExtensionSpec>(
    load: impl FnOnce() -> Vec<S>,
    line: &str,
) -> Option<String> {
    let (name, _) = split_command(line)?;
    if is_reserved(name) || !valid_name(name) {
        return None;
    }
    expand_command(&load(), line)
}

/// [`expand_command`] against the extensions visible from the current directory.
pub(crate) fn expand_command_in_cwd(line: &str, cfg: &crate::config::Config) -> Option<String> {
    expand_command_with(
        || discover_at(&current_dir(), terminal_gadgets_enabled(cfg)),
        line,
    )
}

/// Text from a manifest as one line safe to show: control characters and
/// runs of whitespace (newlines included) collapse to single spaces, and the
/// result is cut to [`MAX_DESCRIPTION_CHARS`].
fn one_line(text: &str) -> String {
    let mut out = text
        .split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if let Some((cut, _)) = out.char_indices().nth(MAX_DESCRIPTION_CHARS) {
        out.truncate(cut);
        out.push('…');
    }
    out
}

fn describe<S: ExtensionSpec>(plugin: &S, command: &Command) -> String {
    let about = one_line(&command.description);
    if about.is_empty() {
        format!("({} {})", plugin.kind().label(), plugin.name())
    } else {
        format!("{about} ({} {})", plugin.kind().label(), plugin.name())
    }
}

/// `(label, description)` rows for the slash menu, e.g.
/// `("/review", "Review the diff (plugin review-flow)")`.
pub(crate) fn menu_entries<S: ExtensionSpec>(plugins: &[S]) -> Vec<(String, String)> {
    effective_commands(plugins)
        .into_iter()
        .take(MAX_MENU_ENTRIES)
        .map(|(plugin, command)| (format!("/{}", command.name), describe(plugin, command)))
        .collect()
}

/// [`menu_entries`] for the extensions visible from the current directory.
pub(crate) fn menu_entries_in_cwd(cfg: &crate::config::Config) -> Vec<(String, String)> {
    menu_entries(&discover_at(&current_dir(), terminal_gadgets_enabled(cfg)))
}

/// `/help` lines for the invocable extension commands, formatted like the
/// built-in list (empty when there are none).
pub(crate) fn help_section<S: ExtensionSpec>(plugins: &[S]) -> String {
    let commands = effective_commands(plugins);
    if commands.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!("{} commands", title(plugins))];
    for (plugin, command) in commands.iter().take(MAX_HELP_ENTRIES) {
        let label = format!("/{}", command.name);
        lines.push(format!("  {label:<11} {}", describe(*plugin, command)));
    }
    if commands.len() > MAX_HELP_ENTRIES {
        lines.push(format!(
            "  … and {} more",
            commands.len() - MAX_HELP_ENTRIES
        ));
    }
    lines.join("\n")
}

/// [`help_section`] for the extensions visible from the current directory.
pub(crate) fn help_section_in_cwd(cfg: &crate::config::Config) -> String {
    help_section(&discover_at(&current_dir(), terminal_gadgets_enabled(cfg)))
}

/// The system-prompt block for extension instructions (empty when no plugin
/// declares one).
pub(crate) fn prompt_block<S: ExtensionSpec>(plugins: &[S]) -> String {
    let mut sections = Vec::new();
    for plugin in plugins {
        if let Some(prompt) = plugin.prompt().filter(|p| !p.trim().is_empty()) {
            sections.push(format!("## {}\n\n{}", plugin.name(), prompt.trim()));
        }
    }
    if sections.is_empty() {
        String::new()
    } else {
        format!(
            "# {} instructions\n\n{}",
            title(plugins),
            sections.join("\n\n")
        )
    }
}
