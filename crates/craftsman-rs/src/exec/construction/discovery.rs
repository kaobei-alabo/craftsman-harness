//! Data-only, no-follow bootstrap discovery. Ambient home/project discovery
//! remains unchanged in existing frontends; this path uses only explicit roots.

use super::{PreparationError, Result};
use crate::extensions::Extension;
use cap_fs_ext::{DirExt, MetadataExt};
use cap_std::fs::Dir;
use std::io::Read;
use std::path::Path;

const MAX_FILE: usize = 256 * 1024;
const MAX_ROOT_ENTRIES: usize = 1024;
const MAX_VISIBLE: usize = 64;
const MAX_CONTEXT: usize = 512 * 1024;

pub(crate) struct SourceRoot {
    directory: Dir,
}

impl SourceRoot {
    /// Only bootstrap may create a root. Later source construction clones this
    /// retained authority, never an ambient config/workspace pathname.
    pub(super) fn from_directory(directory: Dir) -> Self {
        Self { directory }
    }

    pub(crate) fn clone_directory(&self) -> std::io::Result<Dir> {
        self.directory.try_clone()
    }

    pub(super) fn open_optional(path: &Path) -> Result<Option<Self>> {
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(PreparationError::Scope),
            Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                Err(PreparationError::Scope)
            }
            Ok(_) => Dir::open_ambient_dir(path, cap_std::ambient_authority())
                .map(|directory| Some(Self { directory }))
                .map_err(|_| PreparationError::Scope),
        }
    }

    fn directory(&self, parts: &[&str]) -> Result<Option<Dir>> {
        let mut directory = self
            .directory
            .try_clone()
            .map_err(|_| PreparationError::Source)?;
        for name in parts {
            if !safe_name(name) {
                return Err(PreparationError::Source);
            }
            match directory.open_dir_nofollow(name) {
                Ok(next) => directory = next,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => return Err(PreparationError::Source),
            }
        }
        Ok(Some(directory))
    }

    pub(super) fn read_optional(&self, parts: &[&str], limit: usize) -> Result<Option<Vec<u8>>> {
        let (last, parent) = parts.split_last().ok_or(PreparationError::Source)?;
        if !safe_name(last) {
            return Err(PreparationError::Source);
        }
        let Some(directory) = self.directory(parent)? else {
            return Ok(None);
        };
        let file = match directory.open_with(
            last,
            &crate::platform_os::confined_file_options(false, false),
        ) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(PreparationError::Source),
        };
        let meta = file.metadata().map_err(|_| PreparationError::Source)?;
        if !meta.is_file() || meta.nlink() != 1 || meta.len() > limit as u64 {
            return Err(PreparationError::Source);
        }
        let mut bytes = Vec::new();
        file.take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| PreparationError::Source)?;
        if bytes.len() > limit {
            return Err(PreparationError::Source);
        }
        if bytes.starts_with(b"\xef\xbb\xbf") {
            bytes.drain(..3);
        }
        Ok(Some(bytes))
    }

    #[cfg(feature = "gadgets")]
    fn component_safe(&self, parts: &[&str]) -> Result<()> {
        let Some(directory) = self.directory(parts)? else {
            return Err(PreparationError::Source);
        };
        match directory.open_with(
            "gadget.wasm",
            &crate::platform_os::confined_file_options(false, false),
        ) {
            Ok(file) => {
                let meta = file.metadata().map_err(|_| PreparationError::Source)?;
                if !meta.is_file()
                    || meta.nlink() != 1
                    || meta.len() > crate::gadgets::surfaces::COMPONENT_LIMIT as u64
                {
                    return Err(PreparationError::Source);
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(PreparationError::Source),
        }
    }

    fn names(&self, parts: &[&str], directories: bool) -> Result<Vec<String>> {
        let Some(directory) = self.directory(parts)? else {
            return Ok(Vec::new());
        };
        let mut names = Vec::new();
        for (index, item) in directory
            .entries()
            .map_err(|_| PreparationError::Source)?
            .enumerate()
        {
            if index >= MAX_ROOT_ENTRIES {
                return Err(PreparationError::Source);
            }
            let item = item.map_err(|_| PreparationError::Source)?;
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !safe_name(&name) || name.starts_with('.') {
                continue;
            }
            let kind = item.file_type().map_err(|_| PreparationError::Source)?;
            if if directories {
                kind.is_dir()
            } else {
                kind.is_file() && name.ends_with(".json")
            } {
                names.push(name);
                if names.len() > MAX_VISIBLE {
                    return Err(PreparationError::Source);
                }
            }
        }
        names.sort();
        Ok(names)
    }
}

pub(super) fn valid_agent(value: &str) -> bool {
    safe_name(value) && value.chars().count() <= 128
}

fn safe_name(value: &str) -> bool {
    super::super::confined_fs::valid_name(value).is_ok()
        && !value.is_empty()
        && !matches!(value, "." | "..")
        && !value.contains(['/', '\\', ':'])
        && !value.chars().any(char::is_control)
        && !value.ends_with(['.', ' '])
        && !value.to_ascii_lowercase().starts_with(".env")
}

#[derive(Debug, Clone)]
pub(crate) struct Resource {
    pub(crate) name: String,
    pub(crate) source: String,
    pub(crate) text: String,
}

#[derive(Default)]
pub(crate) struct Discovery {
    pub(crate) instructions: Vec<Resource>,
    pub(crate) skills: Vec<Resource>,
    pub(crate) extensions: Vec<Extension>,
    pub(crate) skipped_sources: usize,
    pub(crate) trust_workspace: bool,
}

impl Discovery {
    pub(super) fn load(
        config: Option<&SourceRoot>,
        workspace: &Dir,
        agent: &str,
        trusted: bool,
        gadgets: bool,
    ) -> Result<Self> {
        let project = SourceRoot {
            directory: workspace
                .try_clone()
                .map_err(|_| PreparationError::Source)?,
        };
        let mut found = Self {
            trust_workspace: trusted,
            ..Self::default()
        };
        let mut groups = Vec::new();
        let mut context = MAX_CONTEXT;
        if let Some(config) = config {
            found.instruction(
                config,
                &["agents", agent, "AGENTS.md"],
                "user-agent",
                &mut context,
            );
            found.skill_root(
                config,
                &["agents", agent, "skills"],
                "user-agent",
                &mut context,
            );
            found.skill_root(config, &["skills"], "user", &mut context);
            groups.push(found.extensions_root(config, &["plugins"], &["gadgets"], gadgets));
        }
        if trusted {
            found.instruction(
                &project,
                &[".craftsman", "AGENTS.md"],
                "workspace",
                &mut context,
            );
            found.skill_root(
                &project,
                &[".craftsman", "skills"],
                "workspace-craftsman",
                &mut context,
            );
            found.skill_root(
                &project,
                &[".agents", "skills"],
                "workspace-agents",
                &mut context,
            );
            groups.push(found.extensions_root(
                &project,
                &[".craftsman", "plugins"],
                &[".craftsman", "gadgets"],
                gadgets,
            ));
        }
        found.extensions = crate::extensions::merge_discovery(groups, true);
        // Apply one effective budget after coalescing repeated skill/extension identities.
        let mut budget = MAX_CONTEXT;
        found
            .instructions
            .retain(|r| charge(&mut budget, r.text.len()));
        found.skills.retain(|r| charge(&mut budget, r.text.len()));
        found.extensions.retain(|e| {
            charge(
                &mut budget,
                serde_json::to_vec(e).expect("data extension").len(),
            )
        });
        Ok(found)
    }

    fn instruction(&mut self, root: &SourceRoot, path: &[&str], source: &str, budget: &mut usize) {
        match text(root, path) {
            Ok(Some(text)) if charge(budget, text.len()) => self.instructions.push(Resource {
                name: "AGENTS.md".into(),
                source: source.into(),
                text,
            }),
            Ok(None) => {}
            _ => self.skipped_sources += 1,
        }
    }

    fn skill_root(&mut self, root: &SourceRoot, path: &[&str], source: &str, budget: &mut usize) {
        let names = match root.names(path, true) {
            Ok(n) => n,
            Err(_) => {
                self.skipped_sources += 1;
                return;
            }
        };
        for directory in names {
            let mut parts = path.to_vec();
            parts.extend([directory.as_str(), "SKILL.md"]);
            let contents = match text(root, &parts) {
                Ok(Some(text)) if charge(budget, text.len()) => text,
                Ok(None) => continue,
                _ => {
                    self.skipped_sources += 1;
                    continue;
                }
            };
            let (name, _) = crate::skills::parse_frontmatter(&contents);
            let name = name.unwrap_or(directory);
            if name == "build-gadget" {
                continue;
            } // No source-write guide outside confined capability scope.
            if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
                self.skipped_sources += 1;
                continue;
            }
            let resource = Resource {
                name: name.clone(),
                source: source.into(),
                text: contents,
            };
            if let Some(previous) = self.skills.iter_mut().find(|r| r.name == name) {
                *previous = resource;
            } else {
                self.skills.push(resource);
            }
        }
    }

    fn extensions_root(
        &mut self,
        root: &SourceRoot,
        plugins: &[&str],
        _gadgets: &[&str],
        _include_gadgets: bool,
    ) -> Vec<Extension> {
        let mut extensions = Vec::new();
        let mut budget = MAX_CONTEXT;
        let names = match root.names(plugins, false) {
            Ok(n) => n,
            Err(_) => {
                self.skipped_sources += 1;
                Vec::new()
            }
        };
        for file in names {
            let mut parts = plugins.to_vec();
            parts.push(&file);
            let contents = match text(root, &parts) {
                Ok(Some(text)) if charge(&mut budget, text.len()) => text,
                _ => {
                    self.skipped_sources += 1;
                    continue;
                }
            };
            match serde_json::from_str::<crate::plugins::Plugin>(&contents) {
                Ok(plugin) if crate::plugins::valid_plugin(&plugin) => {
                    extensions.push(plugin.into())
                }
                _ => self.skipped_sources += 1,
            }
        }
        #[cfg(feature = "gadgets")]
        if _include_gadgets {
            let mut manifests = Vec::new();
            let names = match root.names(_gadgets, true) {
                Ok(n) => n,
                Err(_) => {
                    self.skipped_sources += 1;
                    Vec::new()
                }
            };
            let mut gadget_budget = MAX_CONTEXT;
            for id in names {
                let mut directory = _gadgets.to_vec();
                directory.push(id.as_str());
                if root.component_safe(&directory).is_err() {
                    self.skipped_sources += 1;
                    continue;
                }
                let mut parts = directory;
                parts.push("gadget.toml");
                let contents = match text(root, &parts) {
                    Ok(Some(text)) if charge(&mut gadget_budget, text.len()) => text,
                    _ => {
                        self.skipped_sources += 1;
                        continue;
                    }
                };
                match crate::gadgets::surfaces::parse_declarative(&contents, &id) {
                    Ok((order, extension)) => {
                        manifests.push((order, id, extension));
                    }
                    Err(_) => self.skipped_sources += 1,
                }
            }
            manifests.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
            extensions.extend(manifests.into_iter().map(|(_, _, e)| e));
        }
        extensions
    }

    /// Guidance is already provided as bounded context, never an absolute user
    /// config SKILL.md path the confined read_file tool would be unable to read.
    pub(crate) fn context_block(&self) -> String {
        let mut out = String::new();
        for r in self.instructions.iter().chain(&self.skills) {
            out.push_str(&format!(
                "\n<provided_context name={} source={}>\n{}\n</provided_context>\n",
                serde_json::to_string(&r.name).expect("resource name"),
                serde_json::to_string(&r.source).expect("resource source"),
                r.text
            ));
        }
        out.push_str(&crate::extensions::prompt_block(&self.extensions));
        out
    }
}

fn text(root: &SourceRoot, path: &[&str]) -> Result<Option<String>> {
    root.read_optional(path, MAX_FILE)?
        .map(|bytes| String::from_utf8(bytes).map_err(|_| PreparationError::Source))
        .transpose()
}
fn charge(budget: &mut usize, bytes: usize) -> bool {
    if bytes > *budget {
        false
    } else {
        *budget -= bytes;
        true
    }
}
