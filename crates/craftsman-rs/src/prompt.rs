//! System prompt assembly:
//! base instructions + memory protocol + AGENTS.md (global + project) +
//! the skills listing.

use crate::config::Config;
use crate::skills;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

/// Appended in one-shot (-m) mode.
// PROVENANCE(P5): adapted from deepagents headless-mode instructions (libs/code/deepagents_code/agent.py);
// see docs/oss-review.md 5.1 #16. Rewritten in P5.
pub const NONINTERACTIVE_INSTRUCTIONS: &str = "\nYou are running non-interactively. \
Make reasonable assumptions and proceed autonomously rather than ask clarifying \
questions. Prefer non-interactive variants of shell commands (e.g. `npm init -y`, \
`apt-get install -y`).";

// PROVENANCE(P5): no match found in deepagents or langchain;
// see docs/oss-review.md 5.1 #17. Rewritten in P5.
const MEMORY_PROTOCOL: &str = "\n# Memory protocol\n\
Your long-term memory is the virtual /memories/ directory — it persists\n\
across sessions and is stored durably (not on the working filesystem).\n\
Always use paths starting exactly with /memories/ (e.g.\n\
/memories/preferences.md); never write memory anywhere else.\n\
1. Research — check it for relevant context before starting a task\n\
   (ls /memories/, read_file).\n\
2. Response — if unsure about something the user has shared previously,\n\
   check memories before asking.\n\
3. Learning — when you learn something useful for future sessions, save it\n\
   as a short markdown note there via write_file (or edit_file to update).\n\
   Keep notes short, factual, topical.\n";

#[cfg(test)]
pub fn build_system_prompt(
    cfg: &Config,
    profile: &craftsman_harness::HarnessProfile,
    extra_system: &str,
) -> String {
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    build_system_prompt_at(cfg, profile, extra_system, &workspace)
}

#[cfg(test)]
pub fn build_system_prompt_at(
    cfg: &Config,
    profile: &craftsman_harness::HarnessProfile,
    extra_system: &str,
    workspace: &Path,
) -> String {
    let extensions = crate::extensions::discover_at(workspace, false);
    build_system_prompt_with_extensions(cfg, profile, extra_system, workspace, &extensions)
}

pub fn build_system_prompt_with_extensions(
    cfg: &Config,
    profile: &craftsman_harness::HarnessProfile,
    extra_system: &str,
    workspace: &Path,
    extensions: &[crate::extensions::Extension],
) -> String {
    let base = profile
        .config
        .base_system_prompt
        .as_deref()
        .unwrap_or("You are Craftsman, a capable general assistant.");
    let mut chunks = vec![base.to_string()];
    chunks.push(crate::platform_os::system_prompt_block());
    chunks.push(MEMORY_PROTOCOL.to_string());
    let permission_guidance = match cfg.permission_mode {
        crate::config::PermissionMode::Plan => {
            "# Permission mode: plan\nInvestigate and produce a plan. Do not attempt to write or edit files. Use read-only tools and clearly describe proposed changes."
        }
        crate::config::PermissionMode::AcceptEdits => {
            "# Permission mode: accept-edits\nYou may make file edits directly. Shell commands and network actions may still require explicit approval."
        }
        crate::config::PermissionMode::Manual => {
            "# Permission mode: manual\nFile changes, shell commands, and network actions require user approval. Present precise, reviewable operations."
        }
        crate::config::PermissionMode::BypassPermissions => {
            "# Permission mode: bypass-permissions\nThe user has explicitly disabled permission prompts for this session. Proceed carefully and avoid unnecessary destructive actions."
        }
        crate::config::PermissionMode::Auto => {
            "# Permission mode: auto\nProceed autonomously with routine file and network actions. Shell commands require approval unless they are clearly read-only; prefer reversible operations."
        }
    };
    chunks.push(permission_guidance.to_string());

    let mut agents_md = String::new();
    let global = cfg.agent_dir().join("AGENTS.md");
    if let Ok(text) = std::fs::read_to_string(&global) {
        agents_md.push_str(&text);
    }
    let project = workspace.join(".craftsman").join("AGENTS.md");
    if let Ok(text) = std::fs::read_to_string(&project) {
        agents_md.push_str(&format!(
            "\n# Project AGENTS.md (./.craftsman/AGENTS.md)\n\n{text}"
        ));
    }
    if !agents_md.trim().is_empty() {
        chunks.push(format!("\n# AGENTS.md\n\n{}", agents_md.trim()));
    }

    let found = skills::discover_at(&cfg.agent, workspace);
    let block = skills::skills_prompt_block(&found);
    if !block.is_empty() {
        chunks.push(block);
    }
    let plugin_block = crate::extensions::prompt_block(extensions);
    if !plugin_block.is_empty() {
        chunks.push(plugin_block);
    }
    if !extra_system.trim().is_empty() {
        chunks.push(extra_system.trim().to_string());
    }
    if let Some(suffix) = &profile.config.system_prompt_suffix {
        chunks.push(suffix.clone());
    }

    chunks.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_workspace_controls_project_instructions_and_skills() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".craftsman/skills/workspace-skill"))
            .unwrap();
        std::fs::write(
            workspace.path().join(".craftsman/AGENTS.md"),
            "workspace-specific-instructions",
        )
        .unwrap();
        std::fs::write(
            workspace
                .path()
                .join(".craftsman/skills/workspace-skill/SKILL.md"),
            "---\nname: workspace-skill\ndescription: Workspace only\n---\n",
        )
        .unwrap();

        let prompt = build_system_prompt_at(
            &Config::default(),
            &craftsman_harness::HarnessProfile::default(),
            "",
            workspace.path(),
        );
        assert!(prompt.contains("workspace-specific-instructions"));
        assert!(prompt.contains("workspace-skill — Workspace only"));
    }
}
