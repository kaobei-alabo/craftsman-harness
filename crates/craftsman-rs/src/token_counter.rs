//! [`craftsman_harness::TokenCounter`] implementation for `CompactionMiddleware`.
//!
//! Uses `tiktoken-rs`'s `cl100k_base` (OpenAI's GPT-4 encoding) as a
//! universal estimator, regardless of the active model's actual provider —
//! the same fallback LiteLLM itself uses for every non-OpenAI model. This is
//! an estimate, not an exact count: Claude, Gemini, and other non-OpenAI
//! tokenizers segment text differently. Compaction only needs a fast,
//! roughly-consistent upper bound, not a provider-exact match for what the
//! model itself will report back in `Usage`.

use craftsman_harness::{Msg, TokenCounter};
use tiktoken_rs::CoreBPE;

pub struct TiktokenCounter {
    bpe: CoreBPE,
}

impl TiktokenCounter {
    pub fn new() -> Self {
        // `cl100k_base()` only fails if the bundled encoding data is
        // malformed, which would be a `tiktoken-rs` packaging bug rather
        // than anything runtime-dependent; treat it the same as any other
        // "this build is broken" startup invariant.
        Self {
            bpe: tiktoken_rs::cl100k_base().expect("bundled cl100k_base encoding must load"),
        }
    }
}

impl Default for TiktokenCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl TiktokenCounter {
    /// App-private bounded preflight reuses this exact encoding. Its caller
    /// admits the whole input and supplies scalar-aligned <=4096-byte pieces;
    /// chunked wire-envelope counts remain estimates, not model-exact usage.
    pub(crate) fn count_admitted_piece(&self, text: &str) -> u64 {
        self.bpe.count_ordinary(text) as u64
    }

    /// One message's estimated cost: its `content`, plus the name and
    /// serialized arguments of any `tool_calls` it carries. The `tool_calls`
    /// half is not optional — an assistant making tool calls usually has an
    /// empty `content` while its args JSON can be the largest item in the
    /// transcript, so counting `content` alone would score it zero.
    fn count_msg(&self, m: &Msg) -> u64 {
        let content = match m {
            Msg::System { content }
            | Msg::User { content }
            | Msg::Assistant { content, .. }
            | Msg::Tool { content, .. } => content,
        };
        let mut total = self.bpe.count_ordinary(content) as u64;
        if let Msg::Assistant { tool_calls, .. } = m {
            for tc in tool_calls {
                total += self.bpe.count_ordinary(&tc.name) as u64;
                total += self.bpe.count_ordinary(&tc.args.to_string()) as u64;
            }
        }
        total
    }
}

impl TokenCounter for TiktokenCounter {
    fn count(&self, messages: &[Msg]) -> u64 {
        messages.iter().map(|m| self.count_msg(m)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_positive_and_scale_with_content() {
        let counter = TiktokenCounter::new();
        let short = vec![Msg::user("hi")];
        let long = vec![Msg::user("hi ".repeat(200))];
        let short_count = counter.count(&short);
        let long_count = counter.count(&long);
        assert!(short_count > 0);
        assert!(long_count > short_count);
    }

    #[test]
    fn empty_slice_counts_to_zero() {
        let counter = TiktokenCounter::new();
        assert_eq!(counter.count(&[]), 0);
    }

    #[test]
    fn assistant_tool_call_args_are_counted() {
        use craftsman_harness::ToolCall;
        let counter = TiktokenCounter::new();
        // The shape that dominates a coding transcript: empty content, large args.
        let with_call = vec![Msg::Assistant {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "write_file".into(),
                args: serde_json::json!({
                    "path": "src/main.rs",
                    "content": "fn main() { println!(\"hello\"); }\n".repeat(50),
                }),
            }],
        }];
        let empty_assistant = vec![Msg::assistant("")];
        assert_eq!(counter.count(&empty_assistant), 0);
        assert!(
            counter.count(&with_call) > 100,
            "tool_call name + args must be tokenized, got {}",
            counter.count(&with_call)
        );
    }

    #[test]
    fn sums_across_multiple_messages() {
        let counter = TiktokenCounter::new();
        let one = vec![Msg::user("hello world")];
        let two = vec![Msg::user("hello world"), Msg::assistant("hello world")];
        assert_eq!(counter.count(&two), counter.count(&one) * 2);
    }
}
