//! Required checkpoints for explicitly selected, idle terminal compaction.

use super::*;
use crate::builtin::CompactionMiddleware;

impl Agent {
    /// Compact an idle durable conversation without running a turn or its hooks.
    /// The original state is saved before summary sampling, and the replacement
    /// is saved before success returns. Failures retain completed effects in
    /// memory; cancellation before replacement retains the original messages.
    /// An active turn or parked approval refuses this operation. Steering is
    /// rejected so the application can queue it as a separate user turn.
    ///
    /// Only agents selecting `durable_turn_checkpoints` may use this path.
    /// Legacy callers retain their existing manual-compaction behavior.
    pub async fn compact_idle(
        &self,
        compaction: &CompactionMiddleware,
    ) -> Result<Option<(usize, usize)>, AgentError> {
        if !self.durable_turn_checkpoints {
            return Err(AgentError::State(
                "Durable idle compaction is not enabled for this agent.".into(),
            ));
        }
        let _activity = self.begin_activity(true)?;
        let cancellation = self.turn_input.lock().unwrap().cancellation.clone();
        match cancellation {
            Some(turn) => turn.scope(self.compact_idle_inner(compaction)).await,
            None => crate::cancellation::unscoped(self.compact_idle_inner(compaction)).await,
        }
    }

    async fn compact_idle_inner(
        &self,
        compaction: &CompactionMiddleware,
    ) -> Result<Option<(usize, usize)>, AgentError> {
        if self.pending.lock().unwrap().is_some()
            || self.parked_suspension.lock().unwrap().is_some()
        {
            return Err(AgentError::State(
                "Resolve the pending approval before compacting the conversation.".into(),
            ));
        }
        if self.take_cancel_request() {
            return Err(AgentError::Cancelled);
        }
        // A previous failed turn may have left an unanswered call. Preserve
        // actual results and label unknown effects before summarizing them.
        reconcile_recovered_calls(&mut self.state.lock().unwrap(), None);
        self.save_progress_checkpoint(None).await?;
        let messages = self.snapshot().messages;
        let compacted = compaction.compact_now(&messages).await?;
        let counts = {
            // The same stop mutex linearizes cancellation with replacement.
            // Only the message list changes; concurrent files/todos/custom state
            // are retained and included in the subsequent committed snapshot.
            let mut input = self.turn_input.lock().unwrap();
            if input.cancel_requested
                || input
                    .cancellation
                    .as_ref()
                    .is_some_and(|turn| turn.is_cancelled())
            {
                return Err(AgentError::Cancelled);
            }
            let mut state = self.state.lock().unwrap();
            if state.messages != messages {
                return Err(AgentError::State(
                    "Conversation changed during compaction; the summary was discarded.".into(),
                ));
            }
            let counts = compacted.map(|compacted| {
                let counts = (messages.len(), compacted.len());
                state.messages = compacted;
                state.last_usage = None;
                state.last_usage_msg_count = 0;
                counts
            });
            input.finishing = true;
            counts
        };
        // Do not select against cancellation or replay turn_end here. The
        // actual save retains ownership through return, including a late stop.
        self.save_progress_checkpoint(None).await?;
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelResponse;
    use crate::builtin::CompactionTrigger;
    use crate::testing::{MemCheckpointer, ScriptedModel};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::Notify;

    const WAIT: Duration = Duration::from_secs(2);

    fn history() -> Vec<Msg> {
        (0..5)
            .flat_map(|n| {
                [
                    Msg::user(format!("request {n}")),
                    Msg::assistant(format!("result {n}")),
                ]
            })
            .collect()
    }

    fn compaction(model: Arc<dyn ModelProvider>) -> CompactionMiddleware {
        CompactionMiddleware::new(
            model,
            CompactionTrigger::MessageCount {
                threshold_msgs: 100,
                keep_tail_msgs: 20,
            },
        )
    }

    struct Hooks(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl AgentMiddleware for Hooks {
        fn name(&self) -> &str {
            "idle-compaction-hook-probe"
        }
        async fn turn_start(&self, _: &mut AgentState) -> Result<(), AgentError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn turn_end(&self, _: &mut AgentState) -> Result<(), AgentError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn idle_summary_is_saved_before_success_and_reopens_without_turn_hook_replay() {
        let cp = Arc::new(MemCheckpointer::default());
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("actual summary")]));
        let hooks = Arc::new(AtomicUsize::new(0));
        let agent = Agent::builder(model.clone())
            .checkpointer(cp.clone())
            .durable_turn_checkpoints(true)
            .thread_id("idle")
            .middleware(Arc::new(Hooks(hooks.clone())))
            .build();
        {
            let mut state = agent.state.lock().unwrap();
            state.messages = history();
            state
                .custom
                .insert("retained".into(), serde_json::json!({"value":7}));
            state.todos.push(crate::Todo {
                content: "keep completed work".into(),
                status: crate::TodoStatus::Completed,
            });
        }
        let prior = agent.snapshot();
        assert_eq!(
            agent
                .compact_idle(&compaction(model.clone()))
                .await
                .unwrap(),
            Some((10, 5))
        );
        assert_eq!(hooks.load(Ordering::SeqCst), 0);
        assert!(!agent.is_turn_active());
        assert!(!agent.checkpoint_failed());
        let (_, saved, suspension) = cp.load_latest("idle").await.unwrap().unwrap();
        assert!(suspension.is_none());
        assert_eq!(saved.custom[RECOVERY_KEY]["in_progress"], false);
        assert_eq!(saved.messages, agent.snapshot().messages);
        assert_eq!(saved.todos, prior.todos);
        assert_eq!(saved.custom["retained"], prior.custom["retained"]);
        let saves = cp.saves.lock().unwrap();
        assert_eq!(saves.len(), 2);
        assert_eq!(saves[0].2.messages, prior.messages);
        assert_eq!(saves[0].2.custom[RECOVERY_KEY]["in_progress"], true);
        drop(saves);
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].tools.is_empty());
        drop(requests);
        let fresh = Agent::builder(model.clone())
            .checkpointer(cp)
            .durable_turn_checkpoints(true)
            .build();
        assert!(fresh.load_thread("idle").await.unwrap());
        assert!(fresh.recovery_info().is_none());
        assert_eq!(fresh.snapshot().messages, agent.snapshot().messages);
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }

    struct FailedSave {
        saved: MemCheckpointer,
        after_summary: bool,
    }
    #[async_trait::async_trait]
    impl Checkpointer for FailedSave {
        async fn save(
            &self,
            thread: &str,
            seq: u64,
            state: &AgentState,
            suspension: Option<&Suspension>,
        ) -> Result<(), AgentError> {
            let summary = matches!(state.messages.first(), Some(Msg::User { content }) if content.starts_with("<conversation_summary>"));
            if !self.after_summary || summary {
                return Err(AgentError::State(
                    "secret storage diagnostic\u{1b}[2J".into(),
                ));
            }
            self.saved.save(thread, seq, state, suspension).await
        }
        async fn load_latest(
            &self,
            thread: &str,
        ) -> Result<Option<(u64, AgentState, Option<Suspension>)>, AgentError> {
            self.saved.load_latest(thread).await
        }
        async fn list_threads(&self) -> Result<Vec<crate::ThreadInfo>, AgentError> {
            Ok(vec![])
        }
        async fn delete_thread(&self, _: &str) -> Result<(), AgentError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn unavailable_idle_store_refuses_before_summary_sampling() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
        for cp in [
            None,
            Some(Arc::new(FailedSave {
                saved: MemCheckpointer::default(),
                after_summary: false,
            }) as Arc<dyn Checkpointer>),
        ] {
            let mut builder = Agent::builder(model.clone()).durable_turn_checkpoints(true);
            if let Some(cp) = cp {
                builder = builder.checkpointer(cp);
            }
            let agent = builder.build();
            agent.state.lock().unwrap().messages = history();
            let error = agent
                .compact_idle(&compaction(model.clone()))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("Further work was stopped"));
            assert!(!error.contains("secret storage") && !error.contains('\u{1b}'));
            assert!(agent.checkpoint_failed());
            assert!(!agent.is_turn_active());
            assert_eq!(agent.snapshot().messages, history());
        }
        assert!(model.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_summary_save_retains_memory_and_prior_disk_state_without_success() {
        let cp = Arc::new(FailedSave {
            saved: MemCheckpointer::default(),
            after_summary: true,
        });
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("kept in memory")]));
        let agent = Agent::builder(model.clone())
            .checkpointer(cp.clone())
            .thread_id("idle")
            .durable_turn_checkpoints(true)
            .build();
        agent.state.lock().unwrap().messages = history();
        let error = agent
            .compact_idle(&compaction(model.clone()))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Further work was stopped"));
        assert!(agent.checkpoint_failed());
        assert!(
            matches!(&agent.snapshot().messages[0], Msg::User {content} if content.contains("kept in memory"))
        );
        let (_, saved, _) = cp.load_latest("idle").await.unwrap().unwrap();
        assert_eq!(saved.messages, history());
        assert_eq!(saved.custom[RECOVERY_KEY]["in_progress"], true);
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }

    struct PausedSummary {
        started: Notify,
        release: Notify,
    }
    #[async_trait::async_trait]
    impl ModelProvider for PausedSummary {
        async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(ModelResponse {
                message: Msg::assistant("paused summary"),
                usage: crate::Usage::default(),
            })
        }
    }

    #[tokio::test]
    async fn idle_ownership_rejects_turns_steering_and_transfer_and_retains_changed_transcript() {
        let cp = Arc::new(MemCheckpointer::default());
        let model = Arc::new(PausedSummary {
            started: Notify::new(),
            release: Notify::new(),
        });
        let agent = Agent::builder(model.clone())
            .checkpointer(cp.clone())
            .durable_turn_checkpoints(true)
            .build();
        agent.state.lock().unwrap().messages = history();
        let running = agent.clone();
        let provider = model.clone();
        let task = tokio::spawn(async move { running.compact_idle(&compaction(provider)).await });
        tokio::time::timeout(WAIT, model.started.notified())
            .await
            .unwrap();
        assert!(agent.run_turn("must be queued").await.is_err());
        assert!(!agent.steer("queue this instead"));
        let replacement = Agent::builder(model.clone()).build();
        assert!(replacement.inherit_conversation(&agent).is_err());
        agent
            .state
            .lock()
            .unwrap()
            .messages
            .push(Msg::user("external committed change"));
        model.release.notify_one();
        let error = tokio::time::timeout(WAIT, task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("summary was discarded"));
        assert_eq!(
            agent.snapshot().messages.last(),
            Some(&Msg::user("external committed change"))
        );
        assert_eq!(cp.saves.lock().unwrap().len(), 1);
        assert!(!agent.is_turn_active());
    }

    #[tokio::test]
    async fn summary_commit_preserves_custom_and_todo_updates_made_while_sampling() {
        let cp = Arc::new(MemCheckpointer::default());
        let model = Arc::new(PausedSummary {
            started: Notify::new(),
            release: Notify::new(),
        });
        let agent = Agent::builder(model.clone())
            .checkpointer(cp.clone())
            .thread_id("idle")
            .durable_turn_checkpoints(true)
            .build();
        agent.state.lock().unwrap().messages = history();
        let running = agent.clone();
        let provider = model.clone();
        let task = tokio::spawn(async move { running.compact_idle(&compaction(provider)).await });
        tokio::time::timeout(WAIT, model.started.notified())
            .await
            .unwrap();
        {
            let mut state = agent.state.lock().unwrap();
            state
                .custom
                .insert("completed-background-fact".into(), serde_json::json!(19));
            state.todos.push(crate::Todo {
                content: "concurrent completed work".into(),
                status: crate::TodoStatus::Completed,
            });
        }
        model.release.notify_one();
        assert_eq!(
            tokio::time::timeout(WAIT, task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            Some((10, 5))
        );
        let (_, saved, _) = cp.load_latest("idle").await.unwrap().unwrap();
        assert_eq!(saved.custom["completed-background-fact"], 19);
        assert_eq!(saved.todos, agent.snapshot().todos);
        assert_eq!(saved.todos.len(), 1);
    }

    #[tokio::test]
    async fn cancelled_idle_summary_keeps_original_and_a_fresh_turn_still_works() {
        let cp = Arc::new(MemCheckpointer::default());
        let model = Arc::new(PausedSummary {
            started: Notify::new(),
            release: Notify::new(),
        });
        let main = Arc::new(ScriptedModel::new(vec![Msg::assistant("next turn")]));
        let agent = Agent::builder(main.clone())
            .checkpointer(cp.clone())
            .durable_turn_checkpoints(true)
            .interrupt_model_on_cancel(true)
            .build();
        agent.state.lock().unwrap().messages = history();
        let running = agent.clone();
        let provider = model.clone();
        let task = tokio::spawn(async move { running.compact_idle(&compaction(provider)).await });
        tokio::time::timeout(WAIT, model.started.notified())
            .await
            .unwrap();
        assert!(agent.request_cancel());
        assert!(matches!(
            tokio::time::timeout(WAIT, task).await.unwrap().unwrap(),
            Err(AgentError::Cancelled)
        ));
        assert_eq!(agent.snapshot().messages, history());
        assert_eq!(cp.saves.lock().unwrap().len(), 1);
        assert!(!agent.is_turn_active());
        agent.run_turn("continue normally").await.unwrap();
        assert_eq!(main.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn parked_approval_and_legacy_agents_refuse_idle_compaction_without_sampling() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("unused")]));
        let cp = Arc::new(MemCheckpointer::default());
        let agent = Agent::builder(model.clone())
            .checkpointer(cp.clone())
            .durable_turn_checkpoints(true)
            .build();
        agent.state.lock().unwrap().messages = history();
        *agent.pending.lock().unwrap() = Some(PendingResume {
            call: ToolCall {
                id: "pending".into(),
                name: "effect".into(),
                args: serde_json::json!({}),
            },
            remaining: vec![],
            round: 0,
        });
        assert!(
            agent
                .compact_idle(&compaction(model.clone()))
                .await
                .unwrap_err()
                .to_string()
                .contains("pending approval")
        );
        assert!(agent.pending.lock().unwrap().is_some());
        let legacy = Agent::builder(model.clone())
            .checkpointer(cp.clone())
            .build();
        assert!(
            legacy
                .compact_idle(&compaction(model.clone()))
                .await
                .is_err()
        );
        assert!(cp.saves.lock().unwrap().is_empty());
        assert!(model.requests.lock().unwrap().is_empty());
    }

    struct PausedFinalSave {
        saved: MemCheckpointer,
        started: Notify,
        release: Notify,
    }
    #[async_trait::async_trait]
    impl Checkpointer for PausedFinalSave {
        async fn save(
            &self,
            thread: &str,
            seq: u64,
            state: &AgentState,
            suspension: Option<&Suspension>,
        ) -> Result<(), AgentError> {
            if state.custom[RECOVERY_KEY]["in_progress"] == false {
                self.started.notify_one();
                self.release.notified().await;
            }
            self.saved.save(thread, seq, state, suspension).await
        }
        async fn load_latest(
            &self,
            thread: &str,
        ) -> Result<Option<(u64, AgentState, Option<Suspension>)>, AgentError> {
            self.saved.load_latest(thread).await
        }
        async fn list_threads(&self) -> Result<Vec<crate::ThreadInfo>, AgentError> {
            Ok(vec![])
        }
        async fn delete_thread(&self, _: &str) -> Result<(), AgentError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn late_stop_keeps_final_checkpoint_owned_until_actual_return() {
        let cp = Arc::new(PausedFinalSave {
            saved: MemCheckpointer::default(),
            started: Notify::new(),
            release: Notify::new(),
        });
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant(
            "completed summary",
        )]));
        let agent = Agent::builder(model.clone())
            .checkpointer(cp.clone())
            .durable_turn_checkpoints(true)
            .interrupt_model_on_cancel(true)
            .thread_id("idle")
            .build();
        agent.state.lock().unwrap().messages = history();
        let running = agent.clone();
        let task = tokio::spawn(async move { running.compact_idle(&compaction(model)).await });
        tokio::time::timeout(WAIT, cp.started.notified())
            .await
            .unwrap();
        assert!(agent.request_cancel());
        assert!(agent.is_turn_active());
        assert!(!task.is_finished());
        assert_eq!(cp.saved.saves.lock().unwrap().len(), 1);
        cp.release.notify_one();
        assert_eq!(
            tokio::time::timeout(WAIT, task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            Some((10, 5))
        );
        assert!(!agent.is_turn_active());
        let (_, saved, _) = cp.load_latest("idle").await.unwrap().unwrap();
        assert_eq!(saved.custom[RECOVERY_KEY]["in_progress"], false);
        assert_eq!(saved.messages, agent.snapshot().messages);
    }
}
