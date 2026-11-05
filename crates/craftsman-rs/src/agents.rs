//! Agent profiles and AGENTS.md seeding. Each agent is a directory under
//! `~/.craftsman/agents/<name>/` holding its own `AGENTS.md`, `memories/`, and
//! `skills/`. The layout is unchanged from earlier Craftsman clients, so existing
//! profiles keep working.

use std::path::{Path, PathBuf};

/// Seeded into a new agent's AGENTS.md.
// PROVENANCE(P5): no match found in deepagents or langchain;
// see docs/oss-review.md 5.1 #19. Rewritten in P5.
pub const DEFAULT_AGENTS_MD: &str = "\
# AGENTS.md

This file is appended to the system prompt for every conversation. Use it
to record durable preferences, context, and conventions you want Craftsman to
remember across sessions.

## How you use memory
1. **Research** — before starting a task, check `/memories/` for relevant
   notes (`ls /memories/`, `read_file /memories/<topic>.md`).
2. **Response** — if you're uncertain about something the user has told
   you before, search `/memories/` before asking again.
3. **Learning** — when you learn something useful for future sessions
   (preferences, project facts, decisions), save it to
   `/memories/<topic>.md`. Keep entries short and specific.

## House rules
- Be concise. Format with Markdown when useful.
- Prefer running tools to guessing.
";

fn agents_root() -> PathBuf {
    crate::config::config_dir().join("agents")
}

/// Accept one portable directory component, never a path.
pub fn validate_agent_name(name: &str) -> Result<&str, &'static str> {
    if name.is_empty() {
        return Err("agent name cannot be empty");
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err("agent name must be a single directory name");
    }
    Ok(name)
}

/// All agent profile names (sorted directory names under `~/.craftsman/agents`).
pub fn list_agents() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(agents_root()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// Create the agent's directory (if needed) and seed its default AGENTS.md.
/// Returns the agent directory.
pub fn ensure_agent(name: &str) -> std::io::Result<PathBuf> {
    validate_agent_name(name).map_err(std::io::Error::other)?;
    seed_agent_at(&agents_root(), name)
}

fn seed_agent_at(root: &Path, name: &str) -> std::io::Result<PathBuf> {
    let d = root.join(name);
    let p = d.join("AGENTS.md");
    if !p.exists() {
        std::fs::create_dir_all(&d)?;
        std::fs::write(&p, DEFAULT_AGENTS_MD)?;
    }
    std::fs::create_dir_all(d.join("memories"))?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    // list/ensure operate on the real ~/.craftsman,
    // so tests exercise only the pure path/seed logic against a temp HOME-like
    // root by testing seed semantics through a scratch dir instead.

    #[test]
    fn default_agents_md_matches_python_shape() {
        assert!(DEFAULT_AGENTS_MD.starts_with("# AGENTS.md\n"));
        assert!(DEFAULT_AGENTS_MD.contains("## How you use memory"));
        assert!(DEFAULT_AGENTS_MD.contains("## House rules"));
        assert!(DEFAULT_AGENTS_MD.ends_with("guessing.\n"));
    }

    #[test]
    fn seed_is_idempotent_and_preserves_existing_content() {
        let tmp = tempfile::tempdir().unwrap();
        let d = seed_agent_at(tmp.path(), "x").unwrap();
        let p = d.join("AGENTS.md");
        std::fs::write(&p, "custom content").unwrap();
        seed_agent_at(tmp.path(), "x").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "custom content");
        assert!(d.join("memories").is_dir());
    }

    #[test]
    fn agent_name_rejects_paths() {
        assert!(validate_agent_name("").is_err());
        assert!(validate_agent_name("..").is_err());
        assert!(validate_agent_name("a/b").is_err());
        assert!(validate_agent_name("a\\b").is_err());
        assert_eq!(validate_agent_name("researcher"), Ok("researcher"));
    }
}
