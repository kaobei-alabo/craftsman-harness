//! `CompactionMiddleware` — Claude-Code-style conversation compaction. When
//! the transcript grows too large, the oldest messages are replaced with a
//! single model-generated `<conversation_summary>` while a recent tail is
//! kept verbatim, so the agent retains continuity without an ever-growing
//! prompt.
//!
//! Two trigger modes, selected by [`CompactionTrigger`]:
//!
//! - [`CompactionTrigger::Tokens`] (preferred): compacts once the estimated
//!   prompt token count reaches `trigger_tokens` (typically the model's
//!   context window times some fraction, computed by the caller — this
//!   middleware has no opinion on that formula). The tail keeps the most
//!   recent `Msg::User` messages whose combined estimated token cost fits
//!   `keep_tail_tokens`; every `Msg::Assistant`/`Msg::Tool` message, plus any
//!   older `Msg::User` messages that didn't fit, is summarized away. Because
//!   only `Msg::User` messages are ever tail-eligible, and a tool-call/
//!   tool-result pair is always adjacent in this framework's transcripts,
//!   the pair can never be split across the summarized/kept groups.
//! - [`CompactionTrigger::MessageCount`] (fallback): compacts once
//!   `messages.len()` exceeds `threshold_msgs`, keeping the last
//!   `keep_tail_msgs` messages verbatim. Used when no context-window figure
//!   is available for the active model (see `TokenCounter`'s doc and the
//!   caller that decides which trigger to construct). The naive cut walks
//!   backward until the first kept message isn't an orphaned `Msg::Tool`
//!   result (a `Msg::Tool` can only appear here because *any* message type
//!   is eligible for this mode's tail, unlike the token-based mode above).
//!
//! Both variants describe the *automatic* pass in `prepare_model`. The manual
//! `/compact` entry point ([`CompactionMiddleware::compact_now`]) uses
//! neither: the user asking for a compaction has already settled "when", and
//! neither tail policy suits an on-demand pass. `MessageCount` would refuse
//! any transcript shorter than its own tail, and `Tokens` would summarize
//! away every assistant message including the reply just received. Manual
//! passes keep a fixed recent window instead — see [`MANUAL_KEEP_TAIL_MSGS`].
//!
//! The token-based estimate itself is anchor-based, not a full re-tokenize
//! on every call — see `AgentState::last_usage`.

use crate::error::AgentError;
use crate::middleware::AgentMiddleware;
use crate::model::{ModelProvider, ModelRequest, TokenCounter, Usage};
use crate::state::{AgentState, Msg};
use async_trait::async_trait;
use std::collections::BTreeSet;
use std::sync::Arc;

/// System prompt for the summarization sub-call. Asks for a structured,
/// self-contained continuation summary (Claude-Code style) rather than prose.
// PROVENANCE(P5): no match found in deepagents or langchain;
// see docs/oss-review.md 5.1 #5. Rewritten in P5.
const SUMMARY_SYSTEM: &str = "You are compacting an ongoing agent conversation so it can \
continue without the full history. Produce a dense, self-contained summary that a fresh \
reader could use to seamlessly resume the work. Capture, in order: (1) the user's overall \
goal and any explicit requirements or constraints, (2) key decisions, facts, and file paths \
established, (3) actions already taken and their outcomes (tool results, edits, findings), \
(4) the current state and the immediate next step. Be specific and terse; preserve names, \
paths, and identifiers exactly. Do not add commentary or ask questions — output only the \
summary.";

/// Final instruction appended to the messages being summarized.
// PROVENANCE(P5): no match found in deepagents or langchain;
// see docs/oss-review.md 5.1 #6. Rewritten in P5.
const SUMMARY_REQUEST: &str =
    "Summarize the conversation above per your instructions, as a continuation summary.";

/// Messages a manual `/compact` keeps verbatim — roughly one complete turn
/// (user → assistant → tool → assistant), enough to resume without the
/// summary having to restate what just happened.
const MANUAL_KEEP_TAIL_MSGS: usize = 4;

/// Which pass is compacting. The two size their kept tail differently; see
/// the module docs.
#[derive(Clone, Copy)]
enum Pass {
    Auto,
    Manual,
}

/// How [`CompactionMiddleware`] decides when to compact and how the
/// *automatic* pass sizes its kept tail. See the module docs for the
/// semantics of each variant.
pub enum CompactionTrigger {
    MessageCount {
        threshold_msgs: usize,
        keep_tail_msgs: usize,
    },
    Tokens {
        token_counter: Arc<dyn TokenCounter>,
        trigger_tokens: u64,
        keep_tail_tokens: u64,
    },
}

/// Whether a `Usage` carries figures the provider actually reported. Not
/// every OpenAI-compatible gateway honours `stream_options.include_usage`,
/// and the app-side parse defaults the missing figures to zero — an anchor
/// of zero means "unknown", never "the prompt was empty".
fn is_reported(usage: &Usage) -> bool {
    usage.input_tokens + usage.output_tokens > 0
}

/// Middleware that summarizes and drops older messages once `trigger` fires,
/// keeping a recent tail verbatim. See the module docs.
pub struct CompactionMiddleware {
    model: Arc<dyn ModelProvider>,
    trigger: CompactionTrigger,
}

impl CompactionMiddleware {
    /// `model` performs the summarization sub-call (typically the same
    /// provider the agent uses).
    pub fn new(model: Arc<dyn ModelProvider>, trigger: CompactionTrigger) -> Self {
        Self { model, trigger }
    }

    /// Estimated current prompt token count. Uses `st.last_usage` as an
    /// exact anchor plus a local estimate of only the messages appended
    /// since that snapshot, falling back to a full-transcript estimate when
    /// no usable anchor exists (first turn of a session, right after a
    /// compaction reset it, or an unreported usage).
    fn estimate_tokens(st: &AgentState, token_counter: &dyn TokenCounter) -> u64 {
        match &st.last_usage {
            Some(usage) if is_reported(usage) && st.last_usage_msg_count <= st.messages.len() => {
                let delta = &st.messages[st.last_usage_msg_count..];
                usage.input_tokens + usage.output_tokens + token_counter.count(delta)
            }
            _ => token_counter.count(&st.messages),
        }
    }

    /// The tool-safe cut index for keeping the last `keep_tail_msgs`
    /// messages under [`CompactionTrigger::MessageCount`]: the naive
    /// `len - keep_tail_msgs` walked earlier until the first kept message is
    /// not an orphan `Msg::Tool` result (an assistant message carrying
    /// `tool_calls` must stay adjacent to its matching `Tool` results, or
    /// OpenAI-compatible providers reject the orphaned result). `0` means
    /// nothing can be safely dropped.
    fn cut_point_by_message_count(messages: &[Msg], keep_tail_msgs: usize) -> usize {
        let n = messages.len();
        if keep_tail_msgs >= n {
            return 0;
        }
        let mut cut = n - keep_tail_msgs;
        while cut > 0 && matches!(messages[cut], Msg::Tool { .. }) {
            cut -= 1;
        }
        cut
    }

    /// `(to_summarize, tail)` split at [`Self::cut_point_by_message_count`].
    fn split_by_message_count(messages: &[Msg], keep_tail_msgs: usize) -> (Vec<Msg>, Vec<Msg>) {
        let cut = Self::cut_point_by_message_count(messages, keep_tail_msgs);
        (messages[..cut].to_vec(), messages[cut..].to_vec())
    }

    /// Tail size for a manual pass: [`MANUAL_KEEP_TAIL_MSGS`], shrunk so at
    /// least one message is always left to summarize, but never below one —
    /// a transcript that is nothing but tail has nothing to compact.
    fn manual_keep_tail(len: usize) -> usize {
        MANUAL_KEEP_TAIL_MSGS.min(len.saturating_sub(1)).max(1)
    }

    /// Partition `messages` into `(to_summarize, tail)` under `pass`'s tail
    /// policy, preserving relative order within each group. An empty
    /// `to_summarize` means there is nothing safe to drop.
    fn split(&self, messages: &[Msg], pass: Pass) -> (Vec<Msg>, Vec<Msg>) {
        match pass {
            Pass::Manual => {
                Self::split_by_message_count(messages, Self::manual_keep_tail(messages.len()))
            }
            Pass::Auto => match &self.trigger {
                CompactionTrigger::MessageCount { keep_tail_msgs, .. } => {
                    Self::split_by_message_count(messages, *keep_tail_msgs)
                }
                CompactionTrigger::Tokens {
                    token_counter,
                    keep_tail_tokens,
                    ..
                } => {
                    Self::select_tail_by_tokens(messages, token_counter.as_ref(), *keep_tail_tokens)
                }
            },
        }
    }

    /// Tail selection for [`CompactionTrigger::Tokens`]: walk backward from
    /// the end of `messages`, collecting `Msg::User` entries until adding
    /// the next one would exceed `keep_tail_tokens`, then stop (older
    /// messages are never scanned for a smaller fit — the selected set is
    /// always the most recent contiguous-by-recency run that fits).
    /// `Msg::Assistant`/`Msg::Tool` are never tail-eligible. Returns
    /// `(to_summarize, tail)`, each preserving the original relative order.
    fn select_tail_by_tokens(
        messages: &[Msg],
        token_counter: &dyn TokenCounter,
        keep_tail_tokens: u64,
    ) -> (Vec<Msg>, Vec<Msg>) {
        let mut tail_idx: Vec<usize> = Vec::new();
        let mut used = 0u64;
        for (i, m) in messages.iter().enumerate().rev() {
            if !matches!(m, Msg::User { .. }) {
                continue;
            }
            let cost = token_counter.count(std::slice::from_ref(m));
            if used.saturating_add(cost) > keep_tail_tokens {
                break;
            }
            used += cost;
            tail_idx.push(i);
        }
        let tail_set: BTreeSet<usize> = tail_idx.into_iter().collect();

        let mut to_summarize = Vec::with_capacity(messages.len() - tail_set.len());
        let mut tail = Vec::with_capacity(tail_set.len());
        for (i, m) in messages.iter().enumerate() {
            if tail_set.contains(&i) {
                tail.push(m.clone());
            } else {
                to_summarize.push(m.clone());
            }
        }
        (to_summarize, tail)
    }

    /// Compact `messages` NOW, regardless of the trigger condition — the
    /// manual `/compact` entry point. Keeps the most recent
    /// [`MANUAL_KEEP_TAIL_MSGS`] messages rather than the tail the trigger
    /// would pick. Returns the compacted list (summary message + kept tail),
    /// or `None` when the transcript is too short to have anything to
    /// summarize.
    pub async fn compact_now(&self, messages: &[Msg]) -> Result<Option<Vec<Msg>>, AgentError> {
        self.compact(messages, Pass::Manual).await
    }

    async fn compact(&self, messages: &[Msg], pass: Pass) -> Result<Option<Vec<Msg>>, AgentError> {
        let (to_summarize, tail) = self.split(messages, pass);
        if to_summarize.is_empty() {
            return Ok(None);
        }

        let summary = self.summarize(&to_summarize).await?;
        let mut compacted = Vec::with_capacity(tail.len() + 1);
        compacted.push(Msg::user(format!(
            "<conversation_summary>\n{summary}\n</conversation_summary>"
        )));
        compacted.extend(tail);
        Ok(Some(compacted))
    }

    /// Summarize the dropped prefix into a single string via the model.
    async fn summarize(&self, drop: &[Msg]) -> Result<String, AgentError> {
        let mut messages = drop.to_vec();
        messages.push(Msg::user(SUMMARY_REQUEST));
        let req = ModelRequest {
            system: SUMMARY_SYSTEM.to_string(),
            messages,
            tools: vec![],
        };
        let resp = crate::cancellation::model(self.model.complete(req)).await?;
        Ok(match resp.message {
            Msg::Assistant { content, .. } => content,
            Msg::User { content } | Msg::System { content } => content,
            Msg::Tool { content, .. } => content,
        })
    }
}

#[async_trait]
impl AgentMiddleware for CompactionMiddleware {
    fn name(&self) -> &str {
        "compaction"
    }

    async fn prepare_model(&self, st: &mut AgentState) -> Result<(), AgentError> {
        let should_compact = match &self.trigger {
            CompactionTrigger::MessageCount { threshold_msgs, .. } => {
                st.messages.len() > *threshold_msgs
            }
            CompactionTrigger::Tokens {
                token_counter,
                trigger_tokens,
                ..
            } => Self::estimate_tokens(st, token_counter.as_ref()) > *trigger_tokens,
        };
        if !should_compact {
            return Ok(());
        }
        if let Some(compacted) = self.compact(&st.messages, Pass::Auto).await? {
            st.messages = compacted;
            // The anchor's `messages.len()` no longer matches the
            // post-compaction transcript; the next model response
            // re-establishes it.
            st.last_usage = None;
            st.last_usage_msg_count = 0;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Msg, ToolCall};
    use crate::testing::ScriptedModel;
    use std::sync::Arc;

    fn state_with(messages: Vec<Msg>) -> AgentState {
        AgentState {
            messages,
            ..Default::default()
        }
    }

    fn tool_call_msg(id: &str) -> Msg {
        Msg::Assistant {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: "noop".into(),
                args: serde_json::json!({}),
            }],
        }
    }

    fn message_count_trigger(threshold_msgs: usize, keep_tail_msgs: usize) -> CompactionTrigger {
        CompactionTrigger::MessageCount {
            threshold_msgs,
            keep_tail_msgs,
        }
    }

    /// Counts a fixed number of tokens per message, for deterministic
    /// trigger/tail-budget arithmetic without a real tokenizer.
    struct FixedTokenCounter {
        per_message: u64,
    }

    impl TokenCounter for FixedTokenCounter {
        fn count(&self, messages: &[Msg]) -> u64 {
            messages.len() as u64 * self.per_message
        }
    }

    fn tokens_trigger(
        per_message: u64,
        trigger_tokens: u64,
        keep_tail_tokens: u64,
    ) -> CompactionTrigger {
        CompactionTrigger::Tokens {
            token_counter: Arc::new(FixedTokenCounter { per_message }),
            trigger_tokens,
            keep_tail_tokens,
        }
    }

    // ---- MessageCount trigger (regression guard: unchanged semantics) ----

    #[tokio::test]
    async fn message_count_no_op_under_threshold() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("SUMMARY")]));
        let mw = CompactionMiddleware::new(model.clone(), message_count_trigger(10, 2));
        let mut st = state_with(vec![Msg::user("a"), Msg::assistant("b"), Msg::user("c")]);
        let before = st.messages.clone();
        mw.prepare_model(&mut st).await.unwrap();
        assert_eq!(st.messages, before, "unchanged under threshold");
        assert!(model.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn message_count_over_threshold_shrinks_with_summary_and_intact_tail() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("SUMMARY TEXT")]));
        let mw = CompactionMiddleware::new(model, message_count_trigger(4, 2));
        let mut st = state_with(vec![
            Msg::user("q1"),
            Msg::assistant("a1"),
            Msg::user("q2"),
            Msg::assistant("a2"),
            Msg::user("q3"),
            Msg::assistant("a3"),
        ]);
        let tail = st.messages[4..].to_vec();
        mw.prepare_model(&mut st).await.unwrap();
        assert_eq!(st.messages.len(), 3, "1 summary + keep_tail(2)");
        match &st.messages[0] {
            Msg::User { content } => {
                assert!(content.contains("<conversation_summary>"));
                assert!(content.contains("SUMMARY TEXT"));
            }
            other => panic!("expected summary user message, got {other:?}"),
        }
        assert_eq!(&st.messages[1..], &tail[..]);
    }

    #[tokio::test]
    async fn message_count_cut_walks_back_to_avoid_splitting_tool_pairs() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("S")]));
        let mw = CompactionMiddleware::new(model, message_count_trigger(3, 2));
        // Naive cut = len(6) - keep_tail(2) = index 4 == Tool2, which would
        // orphan it from its assistant parent at index 3. Cut must walk
        // back to 3.
        let mut st = state_with(vec![
            Msg::user("start"),      // 0
            tool_call_msg("c1"),     // 1
            Msg::tool("c1", "r1"),   // 2
            tool_call_msg("c2"),     // 3  <- must be kept (parent of Tool2)
            Msg::tool("c2", "r2"),   // 4  <- naive cut lands here (a Tool)
            Msg::assistant("final"), // 5
        ]);
        mw.prepare_model(&mut st).await.unwrap();
        assert_eq!(st.messages.len(), 4);
        assert!(!matches!(st.messages[1], Msg::Tool { .. }));
        assert!(
            matches!(&st.messages[1], Msg::Assistant { tool_calls, .. } if tool_calls[0].id == "c2")
        );
        assert!(matches!(&st.messages[2], Msg::Tool { tool_call_id, .. } if tool_call_id == "c2"));
        assert_eq!(st.messages[3], Msg::assistant("final"));
    }

    #[tokio::test]
    async fn message_count_keep_tail_larger_than_len_is_no_op() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("S")]));
        let mw = CompactionMiddleware::new(model.clone(), message_count_trigger(1, 100));
        let mut st = state_with(vec![Msg::user("a"), Msg::assistant("b")]);
        let before = st.messages.clone();
        mw.prepare_model(&mut st).await.unwrap();
        assert_eq!(st.messages, before);
        assert!(model.requests.lock().unwrap().is_empty());
    }

    // ---- Tokens trigger: anchor+delta estimate ----

    #[tokio::test]
    async fn tokens_no_op_when_estimate_under_trigger() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("SUMMARY")]));
        // 3 messages * 10 tokens/msg = 30, well under trigger_tokens=1000.
        let mw = CompactionMiddleware::new(model.clone(), tokens_trigger(10, 1000, 500));
        let mut st = state_with(vec![Msg::user("a"), Msg::assistant("b"), Msg::user("c")]);
        let before = st.messages.clone();
        mw.prepare_model(&mut st).await.unwrap();
        assert_eq!(st.messages, before);
        assert!(model.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn tokens_anchor_plus_delta_path_triggers_using_exact_usage() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("SUMMARY")]));
        // trigger_tokens=150: exact anchor usage alone (100+40=140) is under
        // it, but +1 delta message @10 tokens/msg pushes the estimate to
        // 150, which is NOT > 150 — so this must NOT trigger yet.
        let mw = CompactionMiddleware::new(model.clone(), tokens_trigger(10, 150, 500));
        let mut st = state_with(vec![Msg::user("a"), Msg::assistant("b"), Msg::user("c")]);
        st.last_usage = Some(Usage {
            input_tokens: 100,
            output_tokens: 40,
        });
        st.last_usage_msg_count = 2; // anchor taken after message index 1
        let before = st.messages.clone();
        mw.prepare_model(&mut st).await.unwrap();
        assert_eq!(st.messages, before, "150 estimate must not exceed 150");
        assert!(model.requests.lock().unwrap().is_empty());

        // One more delta message tips the estimate to 160 > 150: triggers.
        st.messages.push(Msg::user("d"));
        mw.prepare_model(&mut st).await.unwrap();
        assert!(
            !model.requests.lock().unwrap().is_empty(),
            "must trigger now"
        );
    }

    #[tokio::test]
    async fn tokens_full_recount_path_when_no_anchor() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("SUMMARY")]));
        // No last_usage set: falls back to counting all 5 messages directly.
        // 5 * 50 = 250 > trigger_tokens=200 -> must trigger.
        let mw = CompactionMiddleware::new(model.clone(), tokens_trigger(50, 200, 1_000_000));
        let mut st = state_with(vec![
            Msg::user("a"),
            Msg::assistant("b"),
            Msg::user("c"),
            Msg::assistant("d"),
            Msg::user("e"),
        ]);
        mw.prepare_model(&mut st).await.unwrap();
        assert!(!model.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn tokens_stale_anchor_falls_back_to_full_recount() {
        // last_usage_msg_count > current messages.len() (e.g. state was
        // reset/replaced out from under the anchor) must not panic on a
        // slice out-of-bounds — it should fall back to a full recount.
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("SUMMARY")]));
        let mw = CompactionMiddleware::new(model.clone(), tokens_trigger(50, 10_000, 1_000_000));
        let mut st = state_with(vec![Msg::user("a")]);
        st.last_usage = Some(Usage {
            input_tokens: 1,
            output_tokens: 1,
        });
        st.last_usage_msg_count = 99; // stale — beyond current messages.len()
        // Must not panic; 1 message * 50 = 50, well under trigger.
        mw.prepare_model(&mut st).await.unwrap();
        assert!(model.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn tokens_zero_usage_anchor_falls_back_to_full_recount() {
        // Honouring a Usage{0,0} anchor would estimate only the delta (empty
        // here) and never compact; the full recount (5 * 50 > 200) must fire.
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("SUMMARY")]));
        let mw = CompactionMiddleware::new(model.clone(), tokens_trigger(50, 200, 1_000_000));
        let mut st = state_with(vec![
            Msg::user("a"),
            Msg::assistant("b"),
            Msg::user("c"),
            Msg::assistant("d"),
            Msg::user("e"),
        ]);
        st.last_usage = Some(Usage {
            input_tokens: 0,
            output_tokens: 0,
        });
        st.last_usage_msg_count = st.messages.len();
        mw.prepare_model(&mut st).await.unwrap();
        assert!(
            !model.requests.lock().unwrap().is_empty(),
            "zero-valued usage must not suppress the trigger"
        );
    }

    // ---- Tokens trigger: tail selection ----

    #[tokio::test]
    async fn tokens_tail_keeps_only_user_messages_within_budget() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("SUMMARY TEXT")]));
        // keep_tail_tokens=25 with 10 tokens/msg fits exactly the last 2
        // User messages (u2, u3); everything else (including a2, which sits
        // between u2 and u3) is summarized.
        let mw = CompactionMiddleware::new(model, tokens_trigger(10, 1, 25));
        let mut st = state_with(vec![
            Msg::user("u1"),
            Msg::assistant("a1"),
            Msg::user("u2"),
            Msg::assistant("a2"),
            Msg::user("u3"),
        ]);
        mw.prepare_model(&mut st).await.unwrap();
        // summary + u2 + u3 (a2 was NOT pulled in despite sitting after u2).
        assert_eq!(st.messages.len(), 3);
        assert!(
            matches!(&st.messages[0], Msg::User { content } if content.contains("SUMMARY TEXT"))
        );
        assert_eq!(st.messages[1], Msg::user("u2"));
        assert_eq!(st.messages[2], Msg::user("u3"));
    }

    #[tokio::test]
    async fn tokens_tail_never_includes_assistant_or_tool_messages() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("S")]));
        // A generous budget that could easily fit every message by count,
        // but assistant/tool must still never appear in the tail.
        let mw = CompactionMiddleware::new(model, tokens_trigger(1, 1, 1_000_000));
        let mut st = state_with(vec![
            Msg::user("start"),
            tool_call_msg("c1"),
            Msg::tool("c1", "r1"),
            Msg::user("end"),
        ]);
        mw.prepare_model(&mut st).await.unwrap();
        // summary + start + end: the tool_call/tool pair is always
        // summarized, never tail-eligible, regardless of budget.
        assert_eq!(st.messages.len(), 3);
        assert_eq!(st.messages[1], Msg::user("start"));
        assert_eq!(st.messages[2], Msg::user("end"));
    }

    #[tokio::test]
    async fn tokens_tail_stops_walk_on_first_overflow_rather_than_skipping() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("S")]));
        // u2 costs 30 (too big to fit alone in a 25 budget) but u1 costs 10
        // and WOULD fit — the walk must stop at the first (most recent)
        // overflow rather than skipping u2 to reach the smaller u1.
        struct VariableTokenCounter;
        impl TokenCounter for VariableTokenCounter {
            fn count(&self, messages: &[Msg]) -> u64 {
                messages
                    .iter()
                    .map(|m| match m {
                        Msg::User { content } if content == "u2" => 30,
                        _ => 10,
                    })
                    .sum()
            }
        }
        let mw = CompactionMiddleware::new(
            model,
            CompactionTrigger::Tokens {
                token_counter: Arc::new(VariableTokenCounter),
                trigger_tokens: 1,
                keep_tail_tokens: 25,
            },
        );
        let mut st = state_with(vec![Msg::user("u1"), Msg::user("u2")]);
        mw.prepare_model(&mut st).await.unwrap();
        // Tail is empty (u2 alone overflows, and the walk stops there
        // without ever considering u1) — everything is summarized.
        assert_eq!(st.messages.len(), 1, "summary only, no tail");
    }

    #[tokio::test]
    async fn tokens_auto_pass_is_a_no_op_when_everything_fits_the_tail() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("S")]));
        let mw = CompactionMiddleware::new(model.clone(), tokens_trigger(10, 1, 1_000_000));
        let mut st = state_with(vec![Msg::user("a"), Msg::user("b")]);
        let before = st.messages.clone();
        mw.prepare_model(&mut st).await.unwrap();
        assert_eq!(st.messages, before);
        assert!(model.requests.lock().unwrap().is_empty());
    }

    // ---- Manual /compact pass ----

    #[tokio::test]
    async fn manual_compacts_transcript_shorter_than_the_triggers_tail() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("MANUAL")]));
        // The production fallback settings: a 6-message transcript is far
        // under threshold_msgs and shorter than keep_tail_msgs, so the
        // automatic pass would (correctly) do nothing — but an explicit
        // /compact must still compact.
        let mw = CompactionMiddleware::new(model, message_count_trigger(120, 20));
        let msgs = vec![
            Msg::user("q1"),
            Msg::assistant("a1"),
            Msg::user("q2"),
            Msg::assistant("a2"),
            Msg::user("q3"),
            Msg::assistant("a3"),
        ];
        let out = mw.compact_now(&msgs).await.unwrap().expect("compacted");
        assert_eq!(out.len(), 5, "summary + MANUAL_KEEP_TAIL_MSGS(4)");
        assert!(matches!(&out[0], Msg::User { content } if content.contains("MANUAL")));
        assert_eq!(&out[1..], &msgs[2..]);
    }

    #[tokio::test]
    async fn manual_keeps_recent_assistant_messages_under_tokens_trigger() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("MANUAL")]));
        // The token trigger's own tail policy admits User messages only; a
        // manual pass must not discard the reply the user is looking at.
        let mw = CompactionMiddleware::new(model, tokens_trigger(10, 1, 1_000_000));
        let msgs = vec![
            Msg::user("q1"),
            Msg::assistant("a1"),
            Msg::user("q2"),
            Msg::assistant("a2"),
            Msg::user("q3"),
            Msg::assistant("a3"),
        ];
        let out = mw.compact_now(&msgs).await.unwrap().expect("compacted");
        assert_eq!(&out[1..], &msgs[2..]);
        assert!(out.iter().any(|m| matches!(m, Msg::Assistant { .. })));
    }

    #[tokio::test]
    async fn manual_cut_never_splits_tool_pairs() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("MANUAL")]));
        let mw = CompactionMiddleware::new(model, message_count_trigger(120, 20));
        // Naive cut = len(6) - 4 = index 2 == Tool("c1"), which would orphan
        // it from its parent at index 1. The cut must walk back to 1.
        let msgs = vec![
            Msg::user("start"),      // 0
            tool_call_msg("c1"),     // 1
            Msg::tool("c1", "r1"),   // 2
            tool_call_msg("c2"),     // 3
            Msg::tool("c2", "r2"),   // 4
            Msg::assistant("final"), // 5
        ];
        let out = mw.compact_now(&msgs).await.unwrap().expect("compacted");
        assert!(!matches!(out[1], Msg::Tool { .. }));
        assert_eq!(&out[1..], &msgs[1..]);
    }

    #[tokio::test]
    async fn manual_is_none_only_for_trivial_transcripts() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("MANUAL")]));
        let mw = CompactionMiddleware::new(model.clone(), message_count_trigger(120, 20));
        assert!(mw.compact_now(&[]).await.unwrap().is_none());
        assert!(
            mw.compact_now(&[Msg::user("only")])
                .await
                .unwrap()
                .is_none()
        );
        assert!(model.requests.lock().unwrap().is_empty());

        let two = vec![Msg::user("q"), Msg::assistant("a")];
        let out = mw.compact_now(&two).await.unwrap().expect("compacted");
        assert_eq!(out.len(), 2, "summary + the one message left as tail");
        assert_eq!(out[1], Msg::assistant("a"));
    }

    // ---- Post-compaction anchor reset ----

    #[tokio::test]
    async fn compaction_resets_usage_anchor() {
        let model = Arc::new(ScriptedModel::new(vec![Msg::assistant("SUMMARY")]));
        let mw = CompactionMiddleware::new(model, message_count_trigger(2, 1));
        let mut st = state_with(vec![Msg::user("a"), Msg::assistant("b"), Msg::user("c")]);
        st.last_usage = Some(Usage {
            input_tokens: 999,
            output_tokens: 999,
        });
        st.last_usage_msg_count = 3;
        mw.prepare_model(&mut st).await.unwrap();
        assert!(st.last_usage.is_none());
        assert_eq!(st.last_usage_msg_count, 0);
    }
}
