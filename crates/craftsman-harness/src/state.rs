use crate::model::Usage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type FileMap = BTreeMap<String, FileData>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileData {
    pub content: String,
    #[serde(default)]
    pub modified: bool,
}

impl FileData {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: s.into(),
            modified: false,
        }
    }
}

/// Merge file updates into `left`: right side wins; a `None` value deletes the key.
pub fn merge_files(mut left: FileMap, right: BTreeMap<String, Option<FileData>>) -> FileMap {
    for (k, v) in right {
        match v {
            Some(data) => {
                left.insert(k, data);
            }
            None => {
                left.remove(&k);
            }
        }
    }
    left
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Msg {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl Msg {
    pub fn user(s: impl Into<String>) -> Self {
        Msg::User { content: s.into() }
    }
    pub fn assistant(s: impl Into<String>) -> Self {
        Msg::Assistant {
            content: s.into(),
            tool_calls: vec![],
        }
    }
    pub fn tool(id: impl Into<String>, s: impl Into<String>) -> Self {
        Msg::Tool {
            tool_call_id: id.into(),
            content: s.into(),
            is_error: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentState {
    pub messages: Vec<Msg>,
    #[serde(default)]
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub files: FileMap,
    /// Per-turn system prompt additions contributed by middleware (rebuilt each round, not persisted).
    #[serde(skip)]
    pub system_extras: Vec<String>,
    #[serde(default)]
    pub custom: serde_json::Map<String, serde_json::Value>,
    /// Exact token usage from the most recent model response, and the
    /// `messages.len()` at the time it was recorded. `CompactionMiddleware`
    /// uses this as a cheap anchor: it only needs to locally estimate the
    /// token cost of messages appended since this snapshot, rather than
    /// re-tokenizing the whole transcript on every `prepare_model` call.
    /// `None` before the first response of a session, and reset to `None`
    /// after compaction (the old snapshot no longer matches the transcript).
    #[serde(skip)]
    pub last_usage: Option<Usage>,
    #[serde(skip)]
    pub last_usage_msg_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_reducer_merges_and_deletes() {
        let mut left = FileMap::new();
        left.insert("a.txt".into(), FileData::text("A"));
        left.insert("b.txt".into(), FileData::text("B"));
        let mut right: std::collections::BTreeMap<String, Option<FileData>> = Default::default();
        right.insert("b.txt".into(), Some(FileData::text("B2"))); // update
        right.insert("a.txt".into(), None); // None deletes
        right.insert("c.txt".into(), Some(FileData::text("C"))); // insert
        let merged = merge_files(left, right);
        assert!(!merged.contains_key("a.txt"));
        assert_eq!(merged["b.txt"].content, "B2");
        assert_eq!(merged["c.txt"].content, "C");
    }

    #[test]
    fn state_round_trips_through_serde() {
        let mut st = AgentState::default();
        st.messages.push(Msg::user("hi"));
        st.messages.push(Msg::Assistant {
            content: "ok".into(),
            tool_calls: vec![ToolCall {
                id: "call_0".into(),
                name: "ls".into(),
                args: serde_json::json!({}),
            }],
        });
        st.todos.push(Todo {
            content: "port state".into(),
            status: TodoStatus::InProgress,
        });
        st.files.insert("x".into(), FileData::text("y"));
        let json = serde_json::to_string(&st).unwrap();
        let back: AgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.messages.len(), 2);
        assert_eq!(back.todos[0].status, TodoStatus::InProgress);
        assert_eq!(back.files["x"].content, "y");
    }
}
