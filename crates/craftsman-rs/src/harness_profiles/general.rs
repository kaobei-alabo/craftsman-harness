//! Craftsman's unchanged fallback harness profile.

use craftsman_harness::{GeneralPurposeSubagentProfile, HarnessProfile, HarnessProfileConfig};

// PROVENANCE(P5): adapted from deepagents 0.3.8 DEFAULT_GENERAL_PURPOSE_DESCRIPTION (middleware/subagents.py);
// see docs/oss-review.md 5.1 #9. Rewritten in P5.
pub const GENERAL_PURPOSE_DESCRIPTION: &str = "General-purpose agent for researching complex questions, searching for files and content, and executing multi-step tasks. When you are searching for a keyword or file and are not confident that you will find the right match in the first few tries use this agent to perform the search for you. This agent has access to the main agent's non-interactive host, web, and platform tools.";

// PROVENANCE(P5): no match found in deepagents or langchain;
// see docs/oss-review.md 5.1 #10. Rewritten in P5.
pub const GENERAL_PURPOSE_SYSTEM_PROMPT: &str =
    "You are a focused sub-agent. Complete the delegated task and report back concisely.";

// This is the pre-profile Craftsman base prompt moved intact into the `general`
// fallback profile. Keeping it byte-for-byte stable prevents the profile
// rollout from silently changing unknown-model behavior.
// PROVENANCE(P5): no match found in deepagents or langchain;
// see docs/oss-review.md 5.1 #8. Rewritten in P5.
pub(super) const BASE_SYSTEM_PROMPT: &str = "You are Craftsman, a capable general assistant running in a terminal. \
You help with any task — research, writing, analysis, planning, and software engineering — using \
the tools available: host filesystem tools, shell execution (behind user approval), web fetch/search \
(behind user approval), platform capabilities, and delegation to subagents via the task tool.\n\
\n\
Guidance: Lead with the outcome; be concise and complete. Use tools rather than guessing — read \
files before editing, verify claims when you can. For multi-step work use write_todos and keep it \
current. Delegate self-contained subtasks to the general-purpose subagent. Long-term memory lives \
under /memories/ — read relevant memory files when a task begins and update them (write_file/edit_file \
on /memories/ paths) when you learn something durable the user would want kept.\n\
\n\
Format your answers with Markdown when useful.";

pub(super) fn profile() -> HarnessProfile {
    HarnessProfile::from_config(HarnessProfileConfig {
        base_system_prompt: Some(BASE_SYSTEM_PROMPT.to_string()),
        general_purpose_subagent: Some(GeneralPurposeSubagentProfile {
            enabled: Some(true),
            description: Some(GENERAL_PURPOSE_DESCRIPTION.to_string()),
            system_prompt: Some(GENERAL_PURPOSE_SYSTEM_PROMPT.to_string()),
        }),
        ..Default::default()
    })
}
