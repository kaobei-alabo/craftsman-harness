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
    system_prompt_chunks(cfg, profile, extra_system, workspace, extensions)
        .into_iter()
        .map(|chunk| chunk.text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// One section of the system prompt, as its producer made it. The prompt is
/// these sections joined by a blank line.
pub struct SystemChunk {
    pub text: String,
    /// What the section was read from, when it is not fixed application
    /// text: a profile, instruction files, discovered skills, extensions or
    /// the frontend's own addition.
    pub read: Option<&'static str>,
    /// The section piece by piece, when it holds text of more than one
    /// origin: each instruction file, each skill, each extension, and the
    /// application's own headings between them. The pieces join to `text`.
    /// Empty for a section of one origin.
    pub pieces: Vec<SystemPiece>,
}

/// One piece of a section, taken where its text was read, before the section
/// was joined.
pub struct SystemPiece {
    pub text: String,
    /// What the piece was read from. `None` for the application's own text.
    pub read: Option<&'static str>,
}

/// The pieces of a section, if they join to exactly its text. A section
/// whose pieces do not is kept as one piece of its own origin.
fn pieces_of(text: &str, pieces: Vec<SystemPiece>) -> Vec<SystemPiece> {
    match joined(&pieces) == text {
        true => pieces,
        false => Vec::new(),
    }
}

fn joined(pieces: &[SystemPiece]) -> String {
    let mut text = String::new();
    for piece in pieces {
        text.push_str(&piece.text);
    }
    text
}

/// Pieces from a producer's list, in which a flag marks what was read from
/// `what` and everything else is the application's own text.
fn labelled(list: Vec<(String, bool)>, what: &'static str) -> Vec<SystemPiece> {
    let mut pieces = Vec::with_capacity(list.len());
    for (text, read) in list {
        pieces.push(SystemPiece {
            text,
            read: match read {
                true => Some(what),
                false => None,
            },
        });
    }
    pieces
}

/// The pieces of `heading` followed by the trimmed join of `segments`: what
/// the trimming leaves of each segment, in order, each with its own origin.
fn trimmed_pieces(heading: &str, segments: Vec<SystemPiece>) -> Vec<SystemPiece> {
    let whole = joined(&segments);
    let start = whole.len() - whole.trim_start().len();
    let end = whole.trim_end().len();
    let mut pieces = vec![SystemPiece {
        text: heading.to_string(),
        read: None,
    }];
    let mut at = 0;
    for segment in segments {
        let (from, to) = (at.max(start), (at + segment.text.len()).min(end));
        if from < to {
            pieces.push(SystemPiece {
                text: whole[from..to].to_string(),
                read: segment.read,
            });
        }
        at += segment.text.len();
    }
    pieces
}

/// The sections of the system prompt, each still apart and with its origin.
pub fn system_prompt_chunks(
    cfg: &Config,
    profile: &craftsman_harness::HarnessProfile,
    extra_system: &str,
    workspace: &Path,
    extensions: &[crate::extensions::Extension],
) -> Vec<SystemChunk> {
    let fixed = |text: String| SystemChunk {
        text,
        read: None,
        pieces: Vec::new(),
    };
    let read = |what: &'static str, text: String| SystemChunk {
        text,
        read: Some(what),
        pieces: Vec::new(),
    };
    let piece = |read: Option<&'static str>, text: &str| SystemPiece {
        text: text.to_string(),
        read,
    };
    let mut chunks = vec![match profile.config.base_system_prompt.as_deref() {
        Some(base) => read("harness profile", base.to_string()),
        None => fixed("You are Craftsman, a capable general assistant.".to_string()),
    }];
    chunks.push(fixed(crate::platform_os::system_prompt_block()));
    chunks.push(fixed(MEMORY_PROTOCOL.to_string()));
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
    chunks.push(fixed(permission_guidance.to_string()));

    // Each instruction file is kept as its own piece from where it is
    // read. The section is still the same text.
    let mut agents_md = String::new();
    let mut files = Vec::new();
    let global = cfg.agent_dir().join("AGENTS.md");
    if let Ok(text) = std::fs::read_to_string(&global) {
        agents_md.push_str(&text);
        files.push(piece(Some("global instruction file"), &text));
    }
    let project = workspace.join(".craftsman").join("AGENTS.md");
    if let Ok(text) = std::fs::read_to_string(&project) {
        let heading = "\n# Project AGENTS.md (./.craftsman/AGENTS.md)\n\n";
        agents_md.push_str(&format!("{heading}{text}"));
        files.push(piece(None, heading));
        files.push(piece(Some("project instruction file"), &text));
    }
    if !agents_md.trim().is_empty() {
        let heading = "\n# AGENTS.md\n\n";
        let text = format!("{heading}{}", agents_md.trim());
        let pieces = pieces_of(&text, trimmed_pieces(heading, files));
        chunks.push(SystemChunk {
            text,
            read: Some("instruction files"),
            pieces,
        });
    }

    let found = skills::discover_at(&cfg.agent, workspace);
    let block = skills::skills_prompt_block(&found);
    if !block.is_empty() {
        let pieces = labelled(skills::skills_prompt_pieces(&found), "discovered skill");
        chunks.push(SystemChunk {
            pieces: pieces_of(&block, pieces),
            text: block,
            read: Some("discovered skills"),
        });
    }
    let plugin_block = crate::extensions::prompt_block(extensions);
    if !plugin_block.is_empty() {
        let pieces = labelled(crate::extensions::prompt_pieces(extensions), "extension");
        chunks.push(SystemChunk {
            pieces: pieces_of(&plugin_block, pieces),
            text: plugin_block,
            read: Some("extensions"),
        });
    }
    if !extra_system.trim().is_empty() {
        chunks.push(read("frontend addition", extra_system.trim().to_string()));
    }
    if let Some(suffix) = &profile.config.system_prompt_suffix {
        chunks.push(read("harness profile", suffix.clone()));
    }
    chunks
}

#[cfg(test)]
mod piece_tests {
    use super::*;

    const HEADING: &str = "\n# AGENTS.md\n\n";
    const PROJECT: &str = "\n# Project AGENTS.md (./.craftsman/AGENTS.md)\n\n";
    const GLOBAL_FILE: Option<&str> = Some("global instruction file");
    const PROJECT_FILE: Option<&str> = Some("project instruction file");

    /// The pieces of the section made from `segments`, checked against the
    /// section the way it was always built.
    fn section(segments: &[(Option<&'static str>, &str)]) -> Vec<(Option<&'static str>, String)> {
        let mut list = Vec::new();
        let mut whole = String::new();
        for (read, text) in segments {
            whole.push_str(text);
            list.push(SystemPiece {
                text: text.to_string(),
                read: *read,
            });
        }
        let ordinary = format!("{HEADING}{}", whole.trim());
        let mut shape = Vec::new();
        for piece in pieces_of(&ordinary, trimmed_pieces(HEADING, list)) {
            shape.push((piece.read, piece.text));
        }
        shape
    }

    #[test]
    fn each_instruction_file_is_its_own_piece_of_the_same_section() {
        // Both files, with space around them that the section trims.
        assert_eq!(
            section(&[
                (GLOBAL_FILE, "  global rule\n\n"),
                (None, PROJECT),
                (PROJECT_FILE, "project rule\n  "),
            ]),
            vec![
                (None, HEADING.to_string()),
                (GLOBAL_FILE, "global rule\n\n".to_string()),
                (None, PROJECT.to_string()),
                (PROJECT_FILE, "project rule".to_string()),
            ]
        );
        // Only the project file, or a global file of nothing but space: the
        // trimming takes the first line break of the application's own
        // heading, and the project file is still one piece.
        let project_only = vec![
            (None, HEADING.to_string()),
            (None, PROJECT.trim_start().to_string()),
            (PROJECT_FILE, "rule".to_string()),
        ];
        assert_eq!(
            section(&[(None, PROJECT), (PROJECT_FILE, "rule\n")]),
            project_only
        );
        assert_eq!(
            section(&[
                (GLOBAL_FILE, " \n"),
                (None, PROJECT),
                (PROJECT_FILE, "rule")
            ]),
            project_only
        );
    }

    #[test]
    fn pieces_that_do_not_join_to_the_section_are_not_used() {
        let two = |first: &str, second: &str| {
            labelled(
                vec![(first.to_string(), false), (second.to_string(), true)],
                "extension",
            )
        };
        assert!(pieces_of("section", two("sec", "tio")).is_empty());
        assert_eq!(pieces_of("section", two("sec", "tion")).len(), 2);
    }
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
