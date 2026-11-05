use crate::config::config_dir;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY_FILE: &str = "history-tui.jsonl";
const MAX_ENTRIES: usize = 100;
const LOGIN_PREFIX: &str = "/login";

#[allow(dead_code)]
fn history_path() -> PathBuf {
    config_dir().join(HISTORY_FILE)
}

#[derive(Serialize, Deserialize)]
struct HistoryRecord {
    ts: u64,
    project: String,
    text: String,
}

#[cfg(not(test))]
pub fn load() -> Vec<String> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    load_from(&history_path(), &cwd)
}

#[cfg(test)]
pub fn load() -> Vec<String> {
    Vec::new()
}

#[cfg(not(test))]
pub fn append(line: &str, cwd: &str) {
    append_to(&history_path(), line, cwd);
}

#[cfg(test)]
pub fn append(_line: &str, _cwd: &str) {}

fn load_from(path: &Path, cwd: &str) -> Vec<String> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut entries: VecDeque<String> = VecDeque::with_capacity(MAX_ENTRIES);
    for line in reader.lines() {
        let Ok(line) = line else {
            continue;
        };
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<HistoryRecord>(&line) {
            Ok(record) => {
                if record.project == cwd
                    && !record.text.is_empty()
                    && !record.text.starts_with(LOGIN_PREFIX)
                {
                    if entries.len() >= MAX_ENTRIES {
                        entries.pop_front();
                    }
                    entries.push_back(record.text);
                }
            }
            Err(_) => continue,
        }
    }
    entries.into_iter().collect()
}

fn append_to(path: &Path, line: &str, cwd: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with(LOGIN_PREFIX) {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let record = HistoryRecord {
        ts,
        project: cwd.to_string(),
        text: trimmed.to_string(),
    };
    let Ok(json) = serde_json::to_string(&record) else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{json}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn temp_history_file() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join(HISTORY_FILE);
        (dir, path)
    }

    #[test]
    fn append_creates_file() {
        let (_dir, path) = temp_history_file();
        append_to(&path, "hello world", "/tmp");
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(file.contains("hello world"));
        assert!(file.contains("/tmp"));
        assert!(file.contains("\"ts\""));
    }

    #[test]
    fn append_appends_lines() {
        let (_dir, path) = temp_history_file();
        append_to(&path, "first", "/a");
        append_to(&path, "second", "/b");
        let file = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = file.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("first"));
        assert!(lines[1].contains("second"));
    }

    #[test]
    fn load_returns_tail_in_order() {
        let (_dir, path) = temp_history_file();
        append_to(&path, "alpha", "/x");
        append_to(&path, "beta", "/x");
        append_to(&path, "gamma", "/x");
        let entries = load_from(&path, "/x");
        assert_eq!(entries, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn load_skips_malformed() {
        let (_dir, path) = temp_history_file();
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "{{this is not json").unwrap();
        writeln!(file, r#"{{"ts":100,"project":"/x","text":"good"}}"#).unwrap();
        writeln!(file, "another bad line").unwrap();
        drop(file);
        let entries = load_from(&path, "/x");
        assert_eq!(entries, vec!["good"]);
    }

    #[test]
    fn load_excludes_login_prefix() {
        let (_dir, path) = temp_history_file();
        append_to(&path, "/login user pass", "/x");
        append_to(&path, "/login", "/x");
        append_to(&path, "normal prompt", "/x");
        let entries = load_from(&path, "/x");
        assert_eq!(entries, vec!["normal prompt"]);
    }

    #[test]
    fn load_respects_max() {
        let (_dir, path) = temp_history_file();
        for i in 0..(MAX_ENTRIES + 50) {
            append_to(&path, &format!("prompt-{i}"), "/x");
        }
        let entries = load_from(&path, "/x");
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0], format!("prompt-50"));
        assert_eq!(
            entries.last().unwrap(),
            &format!("prompt-{}", MAX_ENTRIES + 49)
        );
    }

    #[test]
    fn load_returns_empty_when_no_file() {
        let (_dir, path) = temp_history_file();
        let entries = load_from(&path, "/x");
        assert!(entries.is_empty());
    }

    #[test]
    fn load_bounds_memory_for_large_files() {
        let (_dir, path) = temp_history_file();
        let mut file = std::fs::File::create(&path).unwrap();
        for i in 0..(MAX_ENTRIES * 3) {
            writeln!(file, r#"{{"ts":{i},"project":"/x","text":"entry-{i}"}}"#).unwrap();
        }
        drop(file);
        let entries = load_from(&path, "/x");
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0], format!("entry-{}", MAX_ENTRIES * 2));
        assert_eq!(
            entries.last().unwrap(),
            &format!("entry-{}", MAX_ENTRIES * 3 - 1)
        );
    }

    #[test]
    fn load_filters_by_project() {
        let (_dir, path) = temp_history_file();
        append_to(&path, "from-project-a", "/project-a");
        append_to(&path, "from-project-b", "/project-b");
        append_to(&path, "another-a", "/project-a");
        let entries = load_from(&path, "/project-a");
        assert_eq!(entries, vec!["from-project-a", "another-a"]);
    }

    #[test]
    fn load_returns_empty_for_unknown_project() {
        let (_dir, path) = temp_history_file();
        append_to(&path, "prompt", "/some-project");
        let entries = load_from(&path, "/other-project");
        assert!(entries.is_empty());
    }
}
