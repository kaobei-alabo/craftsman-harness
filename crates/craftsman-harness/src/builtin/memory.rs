//! `MemoryMiddleware` loads configured memory files at each turn start and
//! injects an `<agent_memory>` section plus the selected guidance every round.
//!
//! The `turn_start`/`prepare_model` hooks only receive `&mut AgentState`, so the
//! middleware holds an `Arc<dyn Backend>` at construction (the app passes the
//! composite backend). Sources are read once per turn through that backend and
//! the resulting section is (re)appended to `system_extras` each round, since the
//! agent loop clears `system_extras` at every round boundary.

use crate::backend::{Backend, EMPTY_FILE_REMINDER};
use crate::error::AgentError;
use crate::middleware::AgentMiddleware;
use crate::state::AgentState;
use async_trait::async_trait;
use std::sync::Arc;

/// Guidance block appended after the loaded memory contents — documents the
/// memory protocol.
// PROVENANCE(P5): adapted from deepagents 0.3.8 MEMORY_SYSTEM_PROMPT (middleware/memory.py);
// see docs/oss-review.md 5.1 #7. Rewritten in P5.
const MEMORY_GUIDELINES: &str = "<memory_guidelines>\n\
    The <agent_memory> above was loaded from files under `/memories/` in your filesystem.\n\
    As you learn durable facts and preferences from the user, persist them by writing or\n\
    editing files under `/memories/` (create with `write_file`, update with `edit_file`).\n\
    \n\
    - Read your memory before acting; treat it as authoritative context for this user.\n\
    - When the user states a lasting preference, gives corrective feedback, or provides\n\
      information useful for future tasks, update `/memories/` IMMEDIATELY — before replying.\n\
    - Capture the underlying principle behind a correction, not just the one-off fix.\n\
    - Do NOT record transient details (one-off tasks, small talk, momentary status).\n\
    - Never store API keys, tokens, passwords, or other credentials in memory.\n\
    </memory_guidelines>";

/// Guidance for an explicitly opted-in coding frontend.
const TASK_FOCUSED_GUIDELINES: &str = "<memory_guidelines>\n\
    The <agent_memory> section above is the turn-start snapshot loaded by the harness.\n\
    Use it as background context; the current user request, repository instructions, and permissions take priority.\n\
    Begin the current task using the relevant workspace files. Read more memory only when a missing durable fact matters to this task.\n\
    Save a preference or durable fact only when the user clearly establishes it for future work. Task requirements, test results, and boilerplate instructions do not need memory updates.\n\
    Save lasting preferences in `/memories/AGENTS.md`, the file loaded on the next turn. Read an existing file before editing and preserve unrelated preferences.\n\
    Never store API keys, tokens, passwords, or other credentials. Existing read and write permissions still apply.\n\
    </memory_guidelines>";

/// Reverse [`crate::backend::cat_n`]'s line numbering to recover raw file text.
///
/// `cat_n` emits `"{n:>6}\t{line}"` per line, so splitting each line on its first
/// tab and taking the remainder restores the original content (lines that
/// themselves contain tabs are preserved — only the first, separator, tab is
/// dropped). The empty-file sentinel maps back to an empty string.
fn strip_cat_n(numbered: &str) -> String {
    if numbered == EMPTY_FILE_REMINDER {
        return String::new();
    }
    numbered
        .lines()
        .map(|l| l.split_once('\t').map(|(_, rest)| rest).unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Middleware that preloads memory sources and surfaces them in the system prompt.
pub struct MemoryMiddleware {
    backend: Arc<dyn Backend>,
    sources: Vec<String>,
    guidelines: &'static str,
    /// Loaded `(path, content)` pairs — refreshed at each turn start
    /// (`turn_start`), reused across that turn's rounds. A `Mutex<Option<…>>`
    /// rather than `OnceCell` so an in-session memory write becomes visible on
    /// the NEXT turn — the guidelines tell the model its writes take effect,
    /// so a session-frozen cache would contradict the protocol.
    loaded: std::sync::Mutex<Option<Vec<(String, String)>>>,
}

impl MemoryMiddleware {
    /// `sources` are file paths (typically under `/memories/`) read through
    /// `backend` at each turn start and injected every round.
    pub fn new(backend: Arc<dyn Backend>, sources: Vec<String>) -> Self {
        Self {
            backend,
            sources,
            guidelines: MEMORY_GUIDELINES,
            loaded: std::sync::Mutex::new(None),
        }
    }

    /// Use already loaded memory without redundant discovery or task-local writes.
    /// The default constructor retains the established guidance.
    pub fn task_focused(mut self) -> Self {
        self.guidelines = TASK_FOCUSED_GUIDELINES;
        self
    }

    async fn load(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for src in &self.sources {
            // A missing source is not an error: skip it. Any read failure is
            // likewise non-fatal here.
            if let Ok(text) = self.backend.read(src, 0, usize::MAX).await {
                let content = strip_cat_n(&text);
                if !content.is_empty() {
                    out.push((src.clone(), content));
                }
            }
        }
        out
    }

    fn section(contents: &[(String, String)], guidelines: &str) -> String {
        let body = if contents.is_empty() {
            "(No memory loaded)".to_string()
        } else {
            contents
                .iter()
                .map(|(path, content)| format!("{path}\n{content}"))
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        format!("<agent_memory>\n{body}\n</agent_memory>\n\n{guidelines}")
    }
}

#[async_trait]
impl AgentMiddleware for MemoryMiddleware {
    fn name(&self) -> &str {
        "memory"
    }

    /// Refresh the memory cache at each turn start, so a memory the agent
    /// wrote last turn is visible this turn (and an external writer — another
    /// process sharing the store — is picked up too).
    async fn turn_start(&self, _st: &mut AgentState) -> Result<(), AgentError> {
        let fresh = self.load().await;
        *self.loaded.lock().unwrap() = Some(fresh);
        Ok(())
    }

    async fn prepare_model(&self, st: &mut AgentState) -> Result<(), AgentError> {
        // Lazily load if turn_start hasn't run (e.g. a bare middleware test);
        // otherwise reuse this turn's cache — no per-round re-reads.
        let cached = self.loaded.lock().unwrap().clone();
        let contents = match cached {
            Some(c) => c,
            None => {
                let fresh = self.load().await;
                *self.loaded.lock().unwrap() = Some(fresh.clone());
                fresh
            }
        };
        st.system_extras
            .push(Self::section(&contents, self.guidelines));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Agent;
    use crate::backend::StateFs;
    use crate::state::{AgentState, FileData, Msg};
    use crate::testing::ScriptedModel;
    use std::sync::{Arc, Mutex};

    fn backend_with(files: &[(&str, &str)]) -> Arc<StateFs> {
        let st = Arc::new(Mutex::new(AgentState::default()));
        {
            let mut g = st.lock().unwrap();
            for (p, c) in files {
                g.files.insert((*p).into(), FileData::text(*c));
            }
        }
        Arc::new(StateFs::new(st))
    }

    #[test]
    fn strip_cat_n_recovers_raw_text() {
        let numbered = "     1\thello\n     2\tworld";
        assert_eq!(strip_cat_n(numbered), "hello\nworld");
        // Lines containing their own tabs keep them (only the first tab drops).
        let with_tab = "     1\tcol1\tcol2";
        assert_eq!(strip_cat_n(with_tab), "col1\tcol2");
        assert_eq!(strip_cat_n(EMPTY_FILE_REMINDER), "");
    }

    #[tokio::test]
    async fn injects_loaded_memory_into_system_extras() {
        let backend = backend_with(&[("/memories/AGENTS.md", "Prefer teal.")]);
        let mw = MemoryMiddleware::new(backend, vec!["/memories/AGENTS.md".into()]);
        let mut st = AgentState::default();
        mw.prepare_model(&mut st).await.unwrap();
        assert_eq!(st.system_extras.len(), 1);
        let section = &st.system_extras[0];
        assert!(section.contains("<agent_memory>"));
        assert!(section.contains("/memories/AGENTS.md"));
        assert!(section.contains("Prefer teal."));
        assert!(section.contains("<memory_guidelines>"));
    }

    #[tokio::test]
    async fn missing_source_is_skipped_not_fatal() {
        let backend = backend_with(&[]);
        let mw = MemoryMiddleware::new(backend, vec!["/memories/AGENTS.md".into()]);
        let mut st = AgentState::default();
        mw.prepare_model(&mut st).await.unwrap();
        assert!(st.system_extras[0].contains("(No memory loaded)"));
    }

    #[tokio::test]
    async fn task_focused_memory_retains_loaded_facts_and_current_request_priority() {
        let backend = backend_with(&[("/memories/AGENTS.md", "Prefer small patches.")]);
        let mw = MemoryMiddleware::new(backend, vec!["/memories/AGENTS.md".into()]).task_focused();
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("done")]));
        let agent = Agent::builder(model.clone())
            .middleware(Arc::new(mw))
            .build();
        agent.run_turn("Fix the current bug.").await.unwrap();
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let section = &requests[0].system;
        assert!(section.contains("Prefer small patches."));
        assert!(section.contains(
            "current user request, repository instructions, and permissions take priority"
        ));
        assert!(section.contains("Read more memory only when a missing durable fact matters"));
        assert!(section.contains("Task requirements, test results, and boilerplate instructions do not need memory updates"));
        assert!(section.contains("Never store API keys, tokens, passwords, or other credentials"));
    }

    #[tokio::test]
    async fn task_focused_memory_refreshes_on_next_turn_without_extra_round_reads() {
        let backend = backend_with(&[("/memories/AGENTS.md", "original preference")]);
        let mw = MemoryMiddleware::new(backend.clone(), vec!["/memories/AGENTS.md".into()])
            .task_focused();
        let mut st = AgentState::default();
        mw.turn_start(&mut st).await.unwrap();
        backend.state.lock().unwrap().files.insert(
            "/memories/AGENTS.md".into(),
            FileData::text("new preference"),
        );
        mw.prepare_model(&mut st).await.unwrap();
        assert!(st.system_extras[0].contains("original preference"));
        st.system_extras.clear();
        mw.turn_start(&mut st).await.unwrap();
        mw.prepare_model(&mut st).await.unwrap();
        assert!(st.system_extras[0].contains("new preference"));
        assert!(!st.system_extras[0].contains("original preference"));
    }

    #[tokio::test]
    async fn default_memory_guidance_remains_unchanged_for_legacy_builders() {
        let mw = MemoryMiddleware::new(backend_with(&[]), Vec::new());
        let mut st = AgentState::default();
        mw.prepare_model(&mut st).await.unwrap();
        assert!(st.system_extras[0].contains(MEMORY_GUIDELINES));
        assert!(!st.system_extras[0].contains(TASK_FOCUSED_GUIDELINES));
    }

    #[tokio::test]
    async fn cached_within_a_turn_refreshed_at_turn_start() {
        let backend = backend_with(&[("/memories/AGENTS.md", "v1")]);
        let mw = MemoryMiddleware::new(backend.clone(), vec!["/memories/AGENTS.md".into()]);

        // Round 1 of turn 1 loads v1.
        let mut st = AgentState::default();
        mw.prepare_model(&mut st).await.unwrap();
        assert!(st.system_extras[0].contains("v1"));

        // The agent writes v2 mid-turn: later ROUNDS of the same turn still
        // see the turn-start snapshot (no per-round re-reads)…
        backend
            .state
            .lock()
            .unwrap()
            .files
            .insert("/memories/AGENTS.md".into(), FileData::text("v2"));
        let mut st2 = AgentState::default();
        mw.prepare_model(&mut st2).await.unwrap();
        assert!(st2.system_extras[0].contains("v1"));

        // …but the NEXT turn (turn_start refresh) sees the write — the
        // memory protocol promises in-session writes take effect.
        let mut st3 = AgentState::default();
        mw.turn_start(&mut st3).await.unwrap();
        let mut st4 = AgentState::default();
        mw.prepare_model(&mut st4).await.unwrap();
        assert!(st4.system_extras[0].contains("v2"));
        assert!(!st4.system_extras[0].contains("v1"));
    }

    #[tokio::test]
    async fn memory_surfaces_in_agent_system_prompt() {
        let backend = backend_with(&[("/memories/AGENTS.md", "remembered fact")]);
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("ok")]));
        let agent = Agent::builder(model.clone())
            .middleware(Arc::new(MemoryMiddleware::new(
                backend,
                vec!["/memories/AGENTS.md".into()],
            )))
            .build();
        agent.run_turn("hi").await.unwrap();
        let reqs = model.requests.lock().unwrap();
        assert!(reqs[0].system.contains("remembered fact"));
    }
}
