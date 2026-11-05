//! Built-in direct-transport provider profiles.

use craftsman_harness::{ProviderProfile, ProviderProfileConfig, ProviderProfileRegistry};
use std::collections::BTreeMap;

pub(super) fn register(registry: &ProviderProfileRegistry) {
    // NVIDIA app-origin attribution header (its value is open, plan A20). This only
    // applies to direct routes; the platform LLM proxy owns upstream headers.
    registry
        .register(
            "nvidia",
            ProviderProfile::from_config(ProviderProfileConfig {
                init_kwargs: BTreeMap::from([(
                    "default_headers".to_string(),
                    serde_json::json!({"X-BILLING-INVOKE-ORIGIN": "DeepAgents"}),
                )]),
            }),
        )
        .expect("valid built-in NVIDIA provider profile");
}
