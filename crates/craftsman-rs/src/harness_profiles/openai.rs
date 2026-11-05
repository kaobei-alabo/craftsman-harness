//! OpenAI-family and exact Codex harness profiles.

use craftsman_harness::{HarnessProfile, HarnessProfileConfig, HarnessProfileRegistry};

// PROVENANCE(P5): adapted from deepagents main profiles/harness/_openai_codex.py;
// see docs/oss-review.md 5.1 #13. Rewritten in P5.
pub(super) const SYSTEM_PROMPT_SUFFIX: &str = "## OpenAI model behavior\n\nAct as an autonomous senior operator. Once the user gives a clear direction, gather context, implement or execute the work, verify the result, and finish the task end-to-end without repeatedly asking for confirmation unless genuinely blocked or a permission gate requires it. Batch independent tool calls when possible. Do not claim success before checking the relevant files, tests, command output, or platform response. Before finishing, reconcile all todo items as completed, blocked with a reason, or cancelled.";

const CODEX_MODELS: &[&str] = &["gpt-5.1-codex", "gpt-5.2-codex", "gpt-5.3-codex"];

pub(super) fn register(registry: &HarnessProfileRegistry) {
    let profile = HarnessProfile::from_config(HarnessProfileConfig {
        system_prompt_suffix: Some(SYSTEM_PROMPT_SUFFIX.to_string()),
        ..Default::default()
    });
    registry
        .register("openai", profile.clone())
        .expect("valid built-in OpenAI harness profile");
    for model in CODEX_MODELS {
        registry
            .register(format!("openai:{model}"), profile.clone())
            .expect("valid built-in Codex harness profile");
    }
}
