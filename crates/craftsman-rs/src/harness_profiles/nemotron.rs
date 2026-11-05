//! NVIDIA Nemotron family and exact-model harness profiles.

use craftsman_harness::{HarnessProfile, HarnessProfileConfig, HarnessProfileRegistry};
use std::collections::BTreeMap;

// PROVENANCE(P5): adapted from deepagents main profiles/harness/_nvidia_nemotron_3_ultra.py;
// see docs/oss-review.md 5.1 #14. Rewritten in P5.
const SYSTEM_PROMPT_SUFFIX: &str = "<approach>\nPlan briefly, then act. Batch independent reads and lookups.\n</approach>\n\n<grounding>\nVerify state with tools. Read files before describing or editing them, resolve opaque identifiers with lookup tools, and perform requested mutations before saying they are complete.\n</grounding>\n\n<loop_control>\nIf a tool fails, inspect the error and change the call or approach before retrying. Do not repeat the same failing call unchanged. Keep tool loops bounded and move to a useful fallback when progress stalls.\n</loop_control>\n\n<tool_selection>\nUse filesystem tools for repository, path, file, and document work. Prefer task-specific platform tools for business objects and operational data. For ranking or counting questions, gather candidates and compare observed results rather than guessing.\n</tool_selection>\n\n<final_answer>\nAfter successful tool calls, report the concrete result and preserve important names, versions, IDs, titles, and other short literals. Resolve opaque IDs to human-readable details when a suitable lookup tool is available.\n</final_answer>";

// PROVENANCE(P5): adapted from deepagents main _READ_FILE_DESCRIPTION_OVERRIDE (profiles/harness/_nvidia_nemotron_3_ultra.py);
// see docs/oss-review.md 5.1 #15. Rewritten in P5.
const READ_FILE_DESCRIPTION: &str = "Read a file with numbered source lines. Use offset and limit for pagination. When the user asks for an entire file, continue reading chunks until EOF rather than treating one full page as the whole file. Use smaller pages for very large files, and always read a file before editing it.";

const MODELS: &[&str] = &[
    "nvidia/nemotron-3-ultra-550b-a55b",
    "nvidia/nvidia-nemotron-3-ultra-550b-a55b",
    "accounts/fireworks/models/nemotron-3-ultra-nvfp4",
    "accounts/fireworks/models/nemotron-3-ultra-bf16",
];

pub(super) fn register(registry: &HarnessProfileRegistry) {
    let profile = HarnessProfile::from_config(HarnessProfileConfig {
        system_prompt_suffix: Some(SYSTEM_PROMPT_SUFFIX.to_string()),
        tool_description_overrides: BTreeMap::from([(
            "read_file".to_string(),
            READ_FILE_DESCRIPTION.to_string(),
        )]),
        ..Default::default()
    });
    registry
        .register("nvidia", profile.clone())
        .expect("valid built-in Nemotron harness profile");
    for model in MODELS {
        registry
            .register(format!("nvidia:{model}"), profile.clone())
            .expect("valid built-in Nemotron model harness profile");
    }
}
