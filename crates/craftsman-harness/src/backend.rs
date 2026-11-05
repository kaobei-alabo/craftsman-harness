use crate::error::AgentError;
use crate::state::{AgentState, FileData, FileMap};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// Sentinel returned when a read hits an existing file with empty contents.
// PROVENANCE(P5): verbatim from deepagents 0.3.8 EMPTY_CONTENT_WARNING (backends/utils.py);
// see docs/oss-review.md 5.1 #1. Rewritten in P5.
pub const EMPTY_FILE_REMINDER: &str = "System reminder: File exists but has empty contents";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// `cat -n`-style line numbering.
///
/// `offset` is 0-indexed (skip this many leading lines); `limit` caps the
/// number of emitted lines. Numbering continues from `offset` so the first
/// emitted line is numbered `offset + 1`. An empty file yields the
/// [`EMPTY_FILE_REMINDER`] sentinel.
pub fn cat_n(content: &str, offset: usize, limit: usize) -> String {
    if content.is_empty() {
        return EMPTY_FILE_REMINDER.to_string();
    }
    content
        .lines()
        .skip(offset)
        .take(limit)
        .enumerate()
        .map(|(i, line)| format!("{:>6}\t{}", i + 1 + offset, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Glob matcher supporting `*` (any run of non-`/`), `**` (any run including
/// `/`), `?` (a single non-`/` char), and literal characters. No regex.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = path.chars().collect();
    // Memoized DP over (pat_idx, text_idx) bounds the search to O(|pat|*|text|)
    // states, so adjacent wildcards can't trigger exponential backtracking.
    let mut memo = vec![vec![None::<bool>; text.len() + 1]; pat.len() + 1];
    glob_dp(&pat, &text, 0, 0, &mut memo)
}

fn glob_dp(
    pat: &[char],
    text: &[char],
    pi: usize,
    ti: usize,
    memo: &mut [Vec<Option<bool>>],
) -> bool {
    if let Some(cached) = memo[pi][ti] {
        return cached;
    }
    let result = if pi == pat.len() {
        ti == text.len()
    } else if pat[pi] == '*' && pi + 1 < pat.len() && pat[pi + 1] == '*' {
        // `**` matches zero or more characters, including `/`.
        let rest = pi + 2;
        let mut j = ti;
        loop {
            if glob_dp(pat, text, rest, j, memo) {
                break true;
            }
            if j >= text.len() {
                break false;
            }
            j += 1;
        }
    } else if pat[pi] == '*' {
        // `*` matches zero or more non-`/` characters.
        let rest = pi + 1;
        let mut j = ti;
        loop {
            if glob_dp(pat, text, rest, j, memo) {
                break true;
            }
            if j >= text.len() || text[j] == '/' {
                break false;
            }
            j += 1;
        }
    } else if pat[pi] == '?' {
        // `?` matches exactly one non-`/` character.
        ti < text.len() && text[ti] != '/' && glob_dp(pat, text, pi + 1, ti + 1, memo)
    } else {
        ti < text.len() && text[ti] == pat[pi] && glob_dp(pat, text, pi + 1, ti + 1, memo)
    };
    memo[pi][ti] = Some(result);
    result
}

/// A backend provides the host tools (filesystem + optional shell) the agent
/// operates against. `StateFs` is the in-memory implementation over
/// [`AgentState::files`].
#[async_trait]
pub trait Backend: Send + Sync {
    /// List files under `path` (prefix filter). `""` or `"/"` lists everything.
    async fn ls(&self, path: &str) -> Result<Vec<FileInfo>, AgentError>;
    async fn read(&self, path: &str, offset: usize, limit: usize) -> Result<String, AgentError>;
    async fn write(&self, path: &str, content: &str) -> Result<(), AgentError>;
    async fn edit(
        &self,
        path: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> Result<(), AgentError>;
    async fn glob(&self, pattern: &str) -> Result<Vec<String>, AgentError>;
    async fn grep(&self, pattern: &str, path: Option<&str>) -> Result<String, AgentError>;

    fn supports_execution(&self) -> bool {
        false
    }

    async fn execute(&self, _command: &str) -> Result<ExecOutcome, AgentError> {
        Err(AgentError::State(
            "this backend does not support command execution".into(),
        ))
    }
}

/// In-memory backend over [`AgentState::files`]. Its file semantics and
/// error strings are the reference that the app's `StoreFs` keeps byte-identical.
pub struct StateFs {
    pub state: Arc<Mutex<AgentState>>,
}

impl StateFs {
    pub fn new(state: Arc<Mutex<AgentState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Backend for StateFs {
    async fn ls(&self, path: &str) -> Result<Vec<FileInfo>, AgentError> {
        let prefix = if path == "/" { "" } else { path };
        let st = self.state.lock().unwrap();
        Ok(st
            .files
            .iter()
            .filter(|(fpath, _)| fpath.starts_with(prefix))
            .map(|(fpath, data)| FileInfo {
                path: fpath.clone(),
                size: data.content.len() as u64,
            })
            .collect())
    }

    async fn read(&self, path: &str, offset: usize, limit: usize) -> Result<String, AgentError> {
        let st = self.state.lock().unwrap();
        let data = st
            .files
            .get(path)
            .ok_or_else(|| AgentError::State(format!("File '{path}' not found")))?;
        Ok(cat_n(&data.content, offset, limit))
    }

    async fn write(&self, path: &str, content: &str) -> Result<(), AgentError> {
        let mut st = self.state.lock().unwrap();
        if st.files.contains_key(path) {
            return Err(AgentError::State(format!(
                "Cannot write to {path} because it already exists. Read and then make an edit, or write to a new path."
            )));
        }
        st.files.insert(
            path.to_string(),
            FileData {
                content: content.to_string(),
                modified: true,
            },
        );
        Ok(())
    }

    async fn edit(
        &self,
        path: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> Result<(), AgentError> {
        let mut st = self.state.lock().unwrap();
        let data = st
            .files
            .get(path)
            .ok_or_else(|| AgentError::State(format!("File '{path}' not found")))?;
        let content = data.content.clone();
        let count = content.matches(old).count();
        // Not-found applies to replace_all too: the check runs before the
        // replace_all branch; silently writing back unchanged
        // content would tell the model the edit landed when it didn't.
        if count == 0 {
            return Err(AgentError::State(format!(
                "Error: String not found in file: '{old}'"
            )));
        }
        let updated = if replace_all {
            content.replace(old, new)
        } else {
            match count {
                1 => content.replacen(old, new, 1),
                occurrences => {
                    return Err(AgentError::State(format!(
                        "Error: String '{old}' appears {occurrences} times in file. Use replace_all=True to replace all instances, or provide a more specific string with surrounding context."
                    )));
                }
            }
        };
        st.files.insert(
            path.to_string(),
            FileData {
                content: updated,
                modified: true,
            },
        );
        Ok(())
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, AgentError> {
        let st = self.state.lock().unwrap();
        // BTreeMap iteration is already sorted.
        Ok(st
            .files
            .keys()
            .filter(|path| glob_match(pattern, path))
            .cloned()
            .collect())
    }

    async fn grep(&self, pattern: &str, path: Option<&str>) -> Result<String, AgentError> {
        let st = self.state.lock().unwrap();
        let mut out = String::new();
        for (fpath, data) in st.files.iter() {
            if let Some(p) = path {
                if !fpath.starts_with(p) {
                    continue;
                }
            }
            for (i, line) in data.content.lines().enumerate() {
                if line.contains(pattern) {
                    out.push_str(&format!("{}:{}:{}\n", fpath, i + 1, line));
                }
            }
        }
        if out.is_empty() {
            Ok(format!("No matches found for '{pattern}'"))
        } else {
            Ok(out)
        }
    }
}

/// Hard limits for a [`ScopedVirtualFs`]. Limits apply to the complete virtual
/// file map after every mutation, not only to the incoming file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualFsLimits {
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
}

impl Default for VirtualFsLimits {
    fn default() -> Self {
        Self {
            max_files: 128,
            max_file_bytes: 256 * 1024,
            max_total_bytes: 2 * 1024 * 1024,
        }
    }
}

/// In-memory virtual filesystem with explicit writable path prefixes.
///
/// Paths are absolute virtual paths such as `/skills/demo/SKILL.md` and never
/// resolve against the process filesystem. There is deliberately no execution
/// support and no fallback backend.
pub struct ScopedVirtualFs {
    state: Arc<Mutex<AgentState>>,
    writable_prefixes: Vec<String>,
    limits: VirtualFsLimits,
}

impl ScopedVirtualFs {
    pub fn new(
        state: Arc<Mutex<AgentState>>,
        writable_prefixes: Vec<String>,
        limits: VirtualFsLimits,
    ) -> Result<Self, AgentError> {
        let mut prefixes = Vec::with_capacity(writable_prefixes.len());
        for prefix in writable_prefixes {
            let normalized = normalize_virtual_path(&prefix)?;
            if normalized == "/" {
                return Err(AgentError::State(
                    "the virtual filesystem root cannot be writable".into(),
                ));
            }
            if !prefixes.contains(&normalized) {
                prefixes.push(normalized);
            }
        }
        prefixes.sort();

        {
            let mut guard = state.lock().unwrap();
            let mut normalized_files = FileMap::new();
            for (path, data) in &guard.files {
                let normalized = normalize_virtual_path(path)?;
                if normalized == "/" {
                    return Err(AgentError::State(
                        "the virtual filesystem root cannot be a file".into(),
                    ));
                }
                if normalized_files
                    .insert(normalized.clone(), data.clone())
                    .is_some()
                {
                    return Err(AgentError::State(format!(
                        "multiple virtual files normalize to '{normalized}'"
                    )));
                }
            }
            validate_file_map(&normalized_files, limits)?;
            guard.files = normalized_files;
        }

        Ok(Self {
            state,
            writable_prefixes: prefixes,
            limits,
        })
    }

    /// Snapshot files changed through this backend. Used by a service runtime
    /// to return `/scratch` or governed-memory mutations to its authority.
    pub fn modified_files(&self) -> Vec<(String, FileData)> {
        self.state
            .lock()
            .unwrap()
            .files
            .iter()
            .filter(|(_, data)| data.modified)
            .map(|(path, data)| (path.clone(), data.clone()))
            .collect()
    }

    pub fn state(&self) -> Arc<Mutex<AgentState>> {
        self.state.clone()
    }

    fn ensure_writable(&self, path: &str) -> Result<(), AgentError> {
        if self
            .writable_prefixes
            .iter()
            .any(|prefix| path_is_within(path, prefix))
        {
            Ok(())
        } else {
            Err(AgentError::State(format!(
                "virtual path '{path}' is read-only or outside writable mounts"
            )))
        }
    }
}

fn normalize_virtual_path(path: &str) -> Result<String, AgentError> {
    if path.is_empty() || !path.starts_with('/') {
        return Err(AgentError::State(
            "virtual paths must be absolute and start with '/'".into(),
        ));
    }
    if path.contains('\0') || path.contains('\\') {
        return Err(AgentError::State(
            "virtual path contains a forbidden character".into(),
        ));
    }
    let mut parts = Vec::new();
    for part in path.split('/').skip(1) {
        if part.is_empty() {
            continue;
        }
        if matches!(part, "." | "..") {
            return Err(AgentError::State(
                "virtual paths cannot contain '.' or '..' components".into(),
            ));
        }
        parts.push(part);
    }
    if parts.is_empty() {
        Ok("/".into())
    } else {
        Ok(format!("/{}", parts.join("/")))
    }
}

fn validate_virtual_pattern(pattern: &str) -> Result<(), AgentError> {
    if pattern.is_empty() || !pattern.starts_with('/') {
        return Err(AgentError::State(
            "virtual glob patterns must be absolute and start with '/'".into(),
        ));
    }
    if pattern.contains('\0') || pattern.contains('\\') {
        return Err(AgentError::State(
            "virtual glob pattern contains a forbidden character".into(),
        ));
    }
    if pattern
        .split('/')
        .any(|component| matches!(component, "." | ".."))
    {
        return Err(AgentError::State(
            "virtual glob patterns cannot contain '.' or '..' components".into(),
        ));
    }
    Ok(())
}

fn path_is_within(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn validate_file_map(files: &FileMap, limits: VirtualFsLimits) -> Result<(), AgentError> {
    if files.len() > limits.max_files {
        return Err(AgentError::State(format!(
            "virtual filesystem has {} files; maximum is {}",
            files.len(),
            limits.max_files
        )));
    }
    let mut total = 0usize;
    for (path, data) in files {
        let size = data.content.len();
        if size > limits.max_file_bytes {
            return Err(AgentError::State(format!(
                "virtual file '{path}' is {size} bytes; maximum is {}",
                limits.max_file_bytes
            )));
        }
        total = total.saturating_add(size);
    }
    if total > limits.max_total_bytes {
        return Err(AgentError::State(format!(
            "virtual filesystem is {total} bytes; maximum is {}",
            limits.max_total_bytes
        )));
    }
    Ok(())
}

#[async_trait]
impl Backend for ScopedVirtualFs {
    async fn ls(&self, path: &str) -> Result<Vec<FileInfo>, AgentError> {
        let normalized = if path.is_empty() {
            "/".to_string()
        } else {
            normalize_virtual_path(path)?
        };
        let guard = self.state.lock().unwrap();
        Ok(guard
            .files
            .iter()
            .filter(|(file_path, _)| normalized == "/" || path_is_within(file_path, &normalized))
            .map(|(file_path, data)| FileInfo {
                path: file_path.clone(),
                size: data.content.len() as u64,
            })
            .collect())
    }

    async fn read(&self, path: &str, offset: usize, limit: usize) -> Result<String, AgentError> {
        let normalized = normalize_virtual_path(path)?;
        let guard = self.state.lock().unwrap();
        let data = guard
            .files
            .get(&normalized)
            .ok_or_else(|| AgentError::State(format!("File '{normalized}' not found")))?;
        Ok(cat_n(&data.content, offset, limit))
    }

    async fn write(&self, path: &str, content: &str) -> Result<(), AgentError> {
        let normalized = normalize_virtual_path(path)?;
        if normalized == "/" {
            return Err(AgentError::State(
                "the virtual filesystem root cannot be a file".into(),
            ));
        }
        self.ensure_writable(&normalized)?;
        let mut guard = self.state.lock().unwrap();
        if guard.files.contains_key(&normalized) {
            return Err(AgentError::State(format!(
                "Cannot write to {normalized} because it already exists. Read and then make an edit, or write to a new path."
            )));
        }
        let mut candidate = guard.files.clone();
        candidate.insert(
            normalized,
            FileData {
                content: content.to_string(),
                modified: true,
            },
        );
        validate_file_map(&candidate, self.limits)?;
        guard.files = candidate;
        Ok(())
    }

    async fn edit(
        &self,
        path: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> Result<(), AgentError> {
        let normalized = normalize_virtual_path(path)?;
        self.ensure_writable(&normalized)?;
        let mut guard = self.state.lock().unwrap();
        let data = guard
            .files
            .get(&normalized)
            .ok_or_else(|| AgentError::State(format!("File '{normalized}' not found")))?;
        let count = data.content.matches(old).count();
        if count == 0 {
            return Err(AgentError::State(format!(
                "Error: String not found in file: '{old}'"
            )));
        }
        let updated = if replace_all {
            data.content.replace(old, new)
        } else if count == 1 {
            data.content.replacen(old, new, 1)
        } else {
            return Err(AgentError::State(format!(
                "Error: String '{old}' appears {count} times in file. Use replace_all=True to replace all instances, or provide a more specific string with surrounding context."
            )));
        };
        let mut candidate = guard.files.clone();
        candidate.insert(
            normalized,
            FileData {
                content: updated,
                modified: true,
            },
        );
        validate_file_map(&candidate, self.limits)?;
        guard.files = candidate;
        Ok(())
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<String>, AgentError> {
        validate_virtual_pattern(pattern)?;
        let guard = self.state.lock().unwrap();
        Ok(guard
            .files
            .keys()
            .filter(|path| glob_match(pattern, path))
            .cloned()
            .collect())
    }

    async fn grep(&self, pattern: &str, path: Option<&str>) -> Result<String, AgentError> {
        let normalized_prefix = path
            .filter(|path| !path.is_empty())
            .map(normalize_virtual_path)
            .transpose()?;
        let guard = self.state.lock().unwrap();
        let mut out = String::new();
        for (file_path, data) in &guard.files {
            if normalized_prefix
                .as_ref()
                .is_some_and(|prefix| prefix != "/" && !path_is_within(file_path, prefix))
            {
                continue;
            }
            for (index, line) in data.content.lines().enumerate() {
                if line.contains(pattern) {
                    out.push_str(&format!("{}:{}:{}\n", file_path, index + 1, line));
                }
            }
        }
        if out.is_empty() {
            Ok(format!("No matches found for '{pattern}'"))
        } else {
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AgentState, FileData};
    use std::sync::{Arc, Mutex};

    fn fs_with(path: &str, content: &str) -> StateFs {
        let st = Arc::new(Mutex::new(AgentState::default()));
        st.lock()
            .unwrap()
            .files
            .insert(path.into(), FileData::text(content));
        StateFs::new(st)
    }

    #[tokio::test]
    async fn read_numbers_lines_cat_n_with_offset() {
        let fs = fs_with("f.txt", "a\nb\nc");
        let out = fs.read("f.txt", 1, 100).await.unwrap();
        assert_eq!(out, "     2\tb\n     3\tc");
    }

    #[tokio::test]
    async fn read_empty_file_returns_reminder() {
        let fs = fs_with("e.txt", "");
        let out = fs.read("e.txt", 0, 100).await.unwrap();
        assert!(out.contains("File exists but has empty contents"));
    }

    #[tokio::test]
    async fn write_refuses_overwrite() {
        let fs = fs_with("f.txt", "x");
        let err = fs.write("f.txt", "y").await.unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn edit_requires_unique_match() {
        let fs = fs_with("f.txt", "aa aa");
        let err = fs.edit("f.txt", "aa", "b", false).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("Error: String 'aa' appears 2 times in file.")
        );
        fs.edit("f.txt", "aa", "b", true).await.unwrap();
        {
            let st = fs.state.lock().unwrap();
            assert_eq!(st.files["f.txt"].content, "b b");
        }
        // replace_all with a missing old_string must error, not silently no-op.
        let err = fs.edit("f.txt", "missing", "x", true).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn glob_and_grep_over_state_files() {
        let fs = fs_with("src/a.rs", "hello\nworld");
        fs.state
            .lock()
            .unwrap()
            .files
            .insert("src/b.txt".into(), FileData::text("hello"));
        assert_eq!(
            fs.glob("src/*.rs").await.unwrap(),
            vec!["src/a.rs".to_string()]
        );
        let hits = fs.grep("world", None).await.unwrap();
        assert!(hits.contains("src/a.rs") && hits.contains("world"));
    }

    #[tokio::test]
    async fn grep_path_filter_uses_prefix_semantics() {
        let fs = fs_with("src/a.rs", "x");
        fs.state
            .lock()
            .unwrap()
            .files
            .insert("other/b.rs".into(), FileData::text("x"));
        let hits = fs.grep("x", Some("src")).await.unwrap();
        assert!(hits.contains("src/a.rs"), "src match must appear");
        assert!(
            !hits.contains("other/b.rs"),
            "non-prefixed file must be excluded"
        );
    }

    #[tokio::test]
    async fn ls_filters_by_path_prefix() {
        let fs = fs_with("src/a.rs", "x");
        fs.state
            .lock()
            .unwrap()
            .files
            .insert("other/b.rs".into(), FileData::text("y"));
        let all = fs.ls("").await.unwrap();
        assert_eq!(all.len(), 2);
        let src_only = fs.ls("src").await.unwrap();
        assert_eq!(src_only.len(), 1);
        assert_eq!(src_only[0].path, "src/a.rs");
    }

    fn scoped_fs() -> ScopedVirtualFs {
        let state = Arc::new(Mutex::new(AgentState::default()));
        state.lock().unwrap().files.insert(
            "/skills/demo/SKILL.md".into(),
            FileData::text("# Demo\nRead me"),
        );
        ScopedVirtualFs::new(state, vec!["/scratch".into()], VirtualFsLimits::default()).unwrap()
    }

    #[tokio::test]
    async fn scoped_fs_reads_skills_and_writes_scratch() {
        let fs = scoped_fs();
        let skill = fs.read("/skills/demo/SKILL.md", 0, 100).await.unwrap();
        assert!(skill.contains("Demo"));
        assert_eq!(fs.ls("").await.unwrap().len(), 1);
        assert_eq!(fs.ls("/").await.unwrap().len(), 1);
        assert!(
            fs.grep("Demo", Some(""))
                .await
                .unwrap()
                .contains("SKILL.md")
        );
        assert!(
            fs.grep("Demo", Some("/"))
                .await
                .unwrap()
                .contains("SKILL.md")
        );
        fs.write("/scratch/notes.md", "working").await.unwrap();
        assert!(
            fs.read("/scratch/notes.md", 0, 100)
                .await
                .unwrap()
                .contains("working")
        );
        assert_eq!(fs.modified_files().len(), 1);
    }

    #[tokio::test]
    async fn scoped_fs_keeps_skills_read_only_and_rejects_host_paths() {
        let fs = scoped_fs();
        let err = fs
            .edit("/skills/demo/SKILL.md", "Demo", "Changed", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("read-only"));
        let err = fs.read("/etc/passwd", 0, 100).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
        let err = fs.read("/skills/../etc/passwd", 0, 100).await.unwrap_err();
        assert!(err.to_string().contains("cannot contain"));
        assert!(!fs.supports_execution());
        assert!(fs.execute("echo unsafe").await.is_err());
    }

    #[tokio::test]
    async fn scoped_fs_enforces_file_and_total_limits() {
        let state = Arc::new(Mutex::new(AgentState::default()));
        let fs = ScopedVirtualFs::new(
            state,
            vec!["/scratch".into()],
            VirtualFsLimits {
                max_files: 1,
                max_file_bytes: 4,
                max_total_bytes: 4,
            },
        )
        .unwrap();
        fs.write("/scratch/a", "1234").await.unwrap();
        assert!(fs.write("/scratch/b", "x").await.is_err());
        assert!(fs.edit("/scratch/a", "1234", "12345", false).await.is_err());
    }

    #[test]
    fn glob_no_pathological_blowup() {
        // Exponential-backtracking guard: this must complete near-instantly.
        let text = "a".repeat(60);
        assert!(!glob_match("a*a*a*a*a*a*a*a*b", &text));
    }

    #[test]
    fn glob_match_cases() {
        assert!(glob_match("src/*.rs", "src/a.rs"));
        assert!(!glob_match("src/*.rs", "src/x/a.rs"));
        assert!(glob_match("**/*.rs", "a/b/c.rs"));
        assert!(glob_match("*.txt", "a.txt"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "a/c"));
    }
}
