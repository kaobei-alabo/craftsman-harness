//! `RealFs` — a [`Backend`] over the actual host filesystem, adapting the
//! hardened operations in [`crate::tools`] to the framework's trait. Paths are
//! resolved relative to a fixed `root`;
//! `glob`/`grep` are scoped under it. Execution is supported.

use async_trait::async_trait;
use craftsman_harness::backend::glob_match;
use craftsman_harness::{AgentError, Backend, ExecOutcome, FileInfo};
use std::path::PathBuf;

/// A [`Backend`] over the real host filesystem, rooted at `root`.
pub struct RealFs {
    root: PathBuf,
    shell: crate::shell::ShellSettings,
}

impl RealFs {
    #[cfg(test)]
    pub fn new(root: PathBuf) -> Self {
        Self::with_shell(root, crate::shell::ShellSettings::default())
    }

    /// A backend whose `execute` runs with the given `shell` settings.
    pub fn with_shell(root: PathBuf, shell: crate::shell::ShellSettings) -> Self {
        Self { root, shell }
    }

    /// Resolve a caller-supplied path: absolute paths pass through, relative
    /// ones join `root`.
    fn resolve(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.root.join(p)
        }
    }
}

#[async_trait]
impl Backend for RealFs {
    async fn ls(&self, path: &str) -> Result<Vec<FileInfo>, AgentError> {
        let target = if path.is_empty() || path == "/" {
            self.root.clone()
        } else {
            self.resolve(path)
        };
        crate::tools::ls_impl(&target).map_err(AgentError::State)
    }

    async fn read(&self, path: &str, offset: usize, limit: usize) -> Result<String, AgentError> {
        crate::tools::read_file_impl(&self.resolve(path), offset, limit).map_err(AgentError::State)
    }

    async fn write(&self, path: &str, content: &str) -> Result<(), AgentError> {
        crate::tools::write_file_impl(&self.resolve(path), content)
            .map(|_| ())
            .map_err(AgentError::State)
    }

    async fn edit(
        &self,
        path: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> Result<(), AgentError> {
        crate::tools::edit_file_impl(&self.resolve(path), old, new, replace_all)
            .map(|_| ())
            .map_err(AgentError::State)
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, AgentError> {
        // Walk the tree under root, matching root-relative paths. Skip the same
        // noise dirs grep does so a repo checkout doesn't flood results.
        let mut matches = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for entry in rd.filter_map(|e| e.ok()) {
                let p = entry.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if p.is_dir() {
                    if matches!(name, ".git" | "node_modules" | "target") {
                        continue;
                    }
                    stack.push(p);
                    continue;
                }
                if let Ok(rel) = p.strip_prefix(&self.root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if glob_match(pattern, &rel) {
                        matches.push(rel);
                    }
                }
            }
        }
        matches.sort();
        Ok(matches)
    }

    async fn grep(&self, pattern: &str, path: Option<&str>) -> Result<String, AgentError> {
        let base = path
            .map(|p| self.resolve(p))
            .unwrap_or_else(|| self.root.clone());
        crate::tools::grep_impl(&base, pattern).map_err(AgentError::State)
    }

    fn supports_execution(&self) -> bool {
        true
    }

    async fn execute(&self, command: &str) -> Result<ExecOutcome, AgentError> {
        crate::tools::execute_impl(
            command,
            &self.root,
            &self.shell,
            std::time::Duration::from_secs(self.shell.timeout_secs),
        )
        .await
        .map(|raw| ExecOutcome {
            stdout: raw.stdout.text(),
            stderr: raw.stderr.text(),
            exit_code: raw.exit_code,
        })
        .map_err(|mut failure| AgentError::Tool {
            name: "execute".into(),
            message: failure.text(&self.shell),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craftsman_harness::Backend;

    #[tokio::test]
    async fn realfs_read_write_edit_execute_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let fs = RealFs::new(dir.path().to_path_buf());
        fs.write("n.txt", "one\ntwo").await.unwrap();
        let out = fs.read("n.txt", 0, 100).await.unwrap();
        assert_eq!(out, "     1\tone\n     2\ttwo");
        assert!(
            fs.write("n.txt", "x")
                .await
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        fs.edit("n.txt", "two", "2", false).await.unwrap();
        assert!(fs.read("n.txt", 0, 100).await.unwrap().contains("2"));
        assert!(fs.supports_execution());
        let e = fs.execute("echo hi").await.unwrap();
        assert_eq!(e.stdout.trim(), "hi");
    }

    #[tokio::test]
    async fn realfs_glob_and_grep_scope_to_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "needle here").unwrap();
        std::fs::write(dir.path().join("top.txt"), "no").unwrap();
        let fs = RealFs::new(dir.path().to_path_buf());
        assert_eq!(
            fs.glob("src/*.rs").await.unwrap(),
            vec!["src/a.rs".to_string()]
        );
        let hits = fs.grep("needle", None).await.unwrap();
        assert!(hits.contains("src/a.rs"));
    }
}
