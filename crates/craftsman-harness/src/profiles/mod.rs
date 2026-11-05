//! Provider and harness profiles for model-specific Agent behavior.

pub mod harness;
pub mod provider;

pub use harness::{
    GeneralPurposeSubagentProfile, HarnessProfile, HarnessProfileConfig, HarnessProfileRegistry,
    NamedMiddlewareFactory, ProfileError, ResolvedHarnessProfile, apply_middleware_profile,
    apply_tool_profile, apply_tool_profile_to_vec,
};
pub use provider::{
    ProviderProfile, ProviderProfileConfig, ProviderProfileError, ProviderProfileRegistry,
    ResolvedProviderProfile,
};
