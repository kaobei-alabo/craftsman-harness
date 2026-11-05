//! Anthropic Claude family and exact-model harness profiles.

use craftsman_harness::{HarnessProfile, HarnessProfileConfig, HarnessProfileRegistry};

// PROVENANCE(P5): adapted from deepagents main Claude harness profiles (profiles/harness/_anthropic_*.py);
// see docs/oss-review.md 5.1 #11. Rewritten in P5.
const COMMON_SUFFIX: &str = "<claude_behavior>\nUse parallel tool calls when operations are independent; keep dependent calls sequential and never invent missing parameters. Investigate files, code, and external state before making claims about them. After tool results arrive, evaluate whether they are complete and reliable before choosing the next action. Prefer direct work for small tasks and use subagents only when decomposition or isolated context provides real value.\n</claude_behavior>";

// PROVENANCE(P5): adapted from deepagents main profiles/harness/_anthropic_opus_4_7.py;
// see docs/oss-review.md 5.1 #12. Rewritten in P5.
const OPUS_47_OVERLAY: &str = "<tool_usage>\nWhen a task depends on files, tests, or system output, use tools to observe that state directly rather than reasoning from memory. Active investigation is the default, not a fallback.\n</tool_usage>\n\n<subagent_usage>\nComplete work directly when it fits in one response. When genuine fan-out or isolated context is useful, spawn the independent subagents together rather than serially.\n</subagent_usage>";

const COMMON_MODELS: &[&str] = &["claude-haiku-4-5", "claude-sonnet-4-6"];

pub(super) fn register(registry: &HarnessProfileRegistry) {
    let common = HarnessProfile::from_config(HarnessProfileConfig {
        system_prompt_suffix: Some(COMMON_SUFFIX.to_string()),
        ..Default::default()
    });
    registry
        .register("anthropic", common.clone())
        .expect("valid built-in Claude harness profile");
    for model in COMMON_MODELS {
        registry
            .register(format!("anthropic:{model}"), common.clone())
            .expect("valid built-in Claude harness profile");
    }
    registry
        .register(
            "anthropic:claude-opus-4-7",
            HarnessProfile::from_config(HarnessProfileConfig {
                system_prompt_suffix: Some(format!("{COMMON_SUFFIX}\n\n{OPUS_47_OVERLAY}")),
                ..Default::default()
            }),
        )
        .expect("valid built-in Claude Opus harness profile");
}
