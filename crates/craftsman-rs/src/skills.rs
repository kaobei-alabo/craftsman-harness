//! Skills discovery.
//!
//! Skills are directories containing SKILL.md with a YAML-ish frontmatter
//! (`name:` / `description:`). Search dirs:
//! agent dir, shared ~/.craftsman/skills, ~/.agents/skills, ./.craftsman/skills,
//! ./.agents/skills. The system prompt lists them so the model reads
//! SKILL.md via read_file.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

pub fn search_dirs_at(agent: &str, cwd: &Path) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    vec![
        home.join(".craftsman")
            .join("agents")
            .join(agent)
            .join("skills"),
        home.join(".craftsman").join("skills"),
        home.join(".agents").join("skills"),
        cwd.join(".craftsman").join("skills"),
        cwd.join(".agents").join("skills"),
    ]
}

pub fn discover_at(agent: &str, cwd: &Path) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    for dir in search_dirs_at(agent, cwd) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        children.sort_by_key(|e| e.file_name());
        for child in children {
            if !child.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            if let Some(skill) = load_skill(&child.path()) {
                // Later dirs override earlier entries with the same name
                // (the last writer wins).
                if let Some(existing) = out.iter_mut().find(|s| s.name == skill.name) {
                    *existing = skill;
                } else {
                    out.push(skill);
                }
            }
        }
    }
    out
}

fn load_skill(dir: &Path) -> Option<Skill> {
    let md = dir.join("SKILL.md");
    let text = std::fs::read_to_string(&md).ok()?;
    let (name, description) = parse_frontmatter(&text);
    let fallback = dir.file_name()?.to_string_lossy().into_owned();
    Some(Skill {
        name: name.unwrap_or(fallback),
        description: description.unwrap_or_default(),
        path: md,
    })
}

/// Minimal frontmatter reader: `--- ... ---` block with `key: value` lines.
pub(crate) fn parse_frontmatter(text: &str) -> (Option<String>, Option<String>) {
    if !text.starts_with("---") {
        return (None, None);
    }
    let Some(end) = text[3..].find("\n---") else {
        return (None, None);
    };
    let block = &text[3..3 + end];
    let mut name = None;
    let mut description = None;
    for line in block.lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(v.trim().to_string());
        }
    }
    (name, description)
}

/// System-prompt block listing available skills.
pub fn skills_prompt_block(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n# Skills\nYou have access to these skills. To use one, first read its \
         SKILL.md with the read_file tool, then follow its instructions.\n",
    );
    for s in skills {
        out.push_str(&format!(
            "- {} — {} (SKILL.md: {})\n",
            s.name,
            s.description,
            s.path.display()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_lists_in_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let sk = dir.path().join("web-search");
        std::fs::create_dir_all(&sk).unwrap();
        std::fs::write(
            sk.join("SKILL.md"),
            "---\nname: web-search\ndescription: Search the web\n---\n\n# body\n",
        )
        .unwrap();
        let loaded = load_skill(&sk).unwrap();
        assert_eq!(loaded.name, "web-search");
        assert_eq!(loaded.description, "Search the web");
        let block = skills_prompt_block(&[loaded]);
        assert!(block.contains("web-search — Search the web"));
        assert!(block.contains("SKILL.md"));
    }

    #[test]
    fn missing_frontmatter_falls_back_to_dirname() {
        let dir = tempfile::tempdir().unwrap();
        let sk = dir.path().join("plain");
        std::fs::create_dir_all(&sk).unwrap();
        std::fs::write(sk.join("SKILL.md"), "# no frontmatter\n").unwrap();
        let loaded = load_skill(&sk).unwrap();
        assert_eq!(loaded.name, "plain");
    }

    #[test]
    fn no_skill_md_means_not_a_skill() {
        let dir = tempfile::tempdir().unwrap();
        let sk = dir.path().join("empty");
        std::fs::create_dir_all(&sk).unwrap();
        assert!(load_skill(&sk).is_none());
    }

    #[test]
    fn explicit_workspace_controls_project_skill_directories() {
        let workspace = tempfile::tempdir().unwrap();
        let dirs = search_dirs_at("main", workspace.path());
        assert!(dirs.contains(&workspace.path().join(".craftsman/skills")));
        assert!(dirs.contains(&workspace.path().join(".agents/skills")));
    }
}
