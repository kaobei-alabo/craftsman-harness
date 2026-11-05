//! Model/provider-specific harness behavior profiles.
//!
//! A profile shapes the agent after its model has been selected: prompt
//! overlays, tool visibility/descriptions, middleware, and the default
//! general-purpose subagent. Registries always start from an explicit fallback
//! profile, then layer provider and exact-model profiles on top.

use crate::error::AgentError;
use crate::middleware::AgentMiddleware;
use crate::tool::{Tool, ToolCtx, ToolOutcome};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

/// Declarative edits for the auto-added general-purpose subagent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneralPurposeSubagentProfile {
    /// `None` inherits, `Some(true)` forces inclusion, `Some(false)` disables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

impl GeneralPurposeSubagentProfile {
    fn merged(base: Option<&Self>, overlay: Option<&Self>) -> Option<Self> {
        match (base, overlay) {
            (None, None) => None,
            (Some(base), None) => Some(base.clone()),
            (None, Some(overlay)) => Some(overlay.clone()),
            (Some(base), Some(overlay)) => Some(Self {
                enabled: overlay.enabled.or(base.enabled),
                description: overlay
                    .description
                    .clone()
                    .or_else(|| base.description.clone()),
                system_prompt: overlay
                    .system_prompt
                    .clone()
                    .or_else(|| base.system_prompt.clone()),
            }),
        }
    }
}

/// YAML/JSON-friendly subset of a harness profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessProfileConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_suffix: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_description_overrides: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub excluded_tools: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub excluded_middleware: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub general_purpose_subagent: Option<GeneralPurposeSubagentProfile>,
}

/// Factory for middleware that must be freshly materialized per agent stack.
#[derive(Clone)]
pub struct NamedMiddlewareFactory {
    name: String,
    build: Arc<dyn Fn() -> Arc<dyn AgentMiddleware> + Send + Sync>,
}

impl NamedMiddlewareFactory {
    pub fn new(
        name: impl Into<String>,
        build: impl Fn() -> Arc<dyn AgentMiddleware> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            build: Arc::new(build),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn materialize(&self) -> Arc<dyn AgentMiddleware> {
        (self.build)()
    }
}

/// Runtime harness profile. Runtime-only middleware factories live alongside
/// the serializable config rather than being forced into JSON/YAML.
#[derive(Clone, Default)]
pub struct HarnessProfile {
    pub config: HarnessProfileConfig,
    pub extra_middleware: Vec<NamedMiddlewareFactory>,
}

impl HarnessProfile {
    pub fn from_config(config: HarnessProfileConfig) -> Self {
        Self {
            config,
            extra_middleware: Vec::new(),
        }
    }

    /// Layer `overlay` on top of `self` using additive profile semantics.
    pub fn merged(&self, overlay: &Self) -> Self {
        let mut descriptions = self.config.tool_description_overrides.clone();
        descriptions.extend(overlay.config.tool_description_overrides.clone());

        let mut excluded_tools = self.config.excluded_tools.clone();
        excluded_tools.extend(overlay.config.excluded_tools.iter().cloned());
        let mut excluded_middleware = self.config.excluded_middleware.clone();
        excluded_middleware.extend(overlay.config.excluded_middleware.iter().cloned());

        let mut factories = self.extra_middleware.clone();
        for incoming in &overlay.extra_middleware {
            if let Some(existing) = factories
                .iter_mut()
                .find(|existing| existing.name() == incoming.name())
            {
                *existing = incoming.clone();
            } else {
                factories.push(incoming.clone());
            }
        }

        Self {
            config: HarnessProfileConfig {
                base_system_prompt: overlay
                    .config
                    .base_system_prompt
                    .clone()
                    .or_else(|| self.config.base_system_prompt.clone()),
                system_prompt_suffix: overlay
                    .config
                    .system_prompt_suffix
                    .clone()
                    .or_else(|| self.config.system_prompt_suffix.clone()),
                tool_description_overrides: descriptions,
                excluded_tools,
                excluded_middleware,
                general_purpose_subagent: GeneralPurposeSubagentProfile::merged(
                    self.config.general_purpose_subagent.as_ref(),
                    overlay.config.general_purpose_subagent.as_ref(),
                ),
            },
            extra_middleware: factories,
        }
    }

    pub fn materialize_extra_middleware(&self) -> Vec<Arc<dyn AgentMiddleware>> {
        self.extra_middleware
            .iter()
            .map(NamedMiddlewareFactory::materialize)
            .collect()
    }

    /// Apply base replacement and suffix layering to an already assembled
    /// application prompt.
    pub fn apply_prompt(&self, base_prompt: &str) -> String {
        let mut prompt = self
            .config
            .base_system_prompt
            .as_deref()
            .unwrap_or(base_prompt)
            .to_string();
        if let Some(suffix) = &self.config.system_prompt_suffix {
            prompt.push_str("\n\n");
            prompt.push_str(suffix);
        }
        prompt
    }
}

struct ProfiledTool {
    inner: Arc<dyn Tool>,
    description: String,
}

#[async_trait]
impl Tool for ProfiledTool {
    fn def(&self) -> crate::model::ToolDef {
        let mut def = self.inner.def();
        def.description = self.description.clone();
        def
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        self.inner.call(args, ctx).await
    }
}

/// Apply one profile's final visibility/description policy to a tool. Returns
/// `None` when the tool is excluded.
pub fn apply_tool_profile(tool: Arc<dyn Tool>, profile: &HarnessProfile) -> Option<Arc<dyn Tool>> {
    let name = tool.def().name;
    if profile.config.excluded_tools.contains(&name) {
        return None;
    }
    match profile.config.tool_description_overrides.get(&name) {
        Some(description) => Some(Arc::new(ProfiledTool {
            inner: tool,
            description: description.clone(),
        })),
        None => Some(tool),
    }
}

pub fn apply_tool_profile_to_vec(
    tools: Vec<Arc<dyn Tool>>,
    profile: &HarnessProfile,
) -> Vec<Arc<dyn Tool>> {
    tools
        .into_iter()
        .filter_map(|tool| apply_tool_profile(tool, profile))
        .collect()
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProfileError {
    #[error("invalid harness profile key '{0}'")]
    InvalidKey(String),
    #[error("harness profile cannot exclude protected middleware: {0}")]
    ProtectedMiddleware(String),
    #[error("harness profile middleware exclusion matched nothing: {0}")]
    UnknownMiddleware(String),
}

/// Result of profile resolution, including the layers used for diagnostics.
#[derive(Clone)]
pub struct ResolvedHarnessProfile {
    pub active_key: String,
    pub layers: Vec<String>,
    pub profile: HarnessProfile,
}

/// Thread-safe profile registry with an explicit general fallback.
#[derive(Clone)]
pub struct HarnessProfileRegistry {
    fallback: HarnessProfile,
    entries: Arc<RwLock<BTreeMap<String, HarnessProfile>>>,
}

impl HarnessProfileRegistry {
    pub fn new(fallback: HarnessProfile) -> Self {
        Self {
            fallback,
            entries: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub fn register(
        &self,
        key: impl Into<String>,
        profile: HarnessProfile,
    ) -> Result<(), ProfileError> {
        let key = normalize_key(&key.into())?;
        let mut entries = self.entries.write().unwrap();
        let merged = entries
            .get(&key)
            .map_or(profile.clone(), |existing| existing.merged(&profile));
        entries.insert(key, merged);
        Ok(())
    }

    /// Resolve `general -> provider -> provider:model`. Missing provider/model
    /// entries simply leave the general fallback unchanged.
    pub fn resolve(&self, provider: Option<&str>, model: &str) -> ResolvedHarnessProfile {
        let entries = self.entries.read().unwrap();
        let mut profile = self.fallback.clone();
        let mut layers = vec!["general".to_string()];
        let mut active_key = "general".to_string();

        let provider = provider.map(str::trim).filter(|s| !s.is_empty());
        if let Some(provider) = provider {
            let provider_key = provider.to_ascii_lowercase();
            if let Some(provider_profile) = entries.get(&provider_key) {
                profile = profile.merged(provider_profile);
                active_key = provider_key.clone();
                layers.push(provider_key.clone());
            }
            let exact_key = format!("{provider_key}:{}", model.trim().to_ascii_lowercase());
            if let Some(exact_profile) = entries.get(&exact_key) {
                profile = profile.merged(exact_profile);
                active_key = exact_key.clone();
                layers.push(exact_key);
            }
        }

        ResolvedHarnessProfile {
            active_key,
            layers,
            profile,
        }
    }
}

fn normalize_key(raw: &str) -> Result<String, ProfileError> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty() || key.matches(':').count() > 1 {
        return Err(ProfileError::InvalidKey(raw.to_string()));
    }
    if let Some((provider, model)) = key.split_once(':') {
        if provider.is_empty() || model.is_empty() {
            return Err(ProfileError::InvalidKey(raw.to_string()));
        }
    }
    Ok(key)
}

/// Apply profile middleware exclusions and runtime additions while protecting
/// safety/structural middleware. Exclusions must match at least one entry.
pub fn apply_middleware_profile(
    mut middleware: Vec<Arc<dyn AgentMiddleware>>,
    profile: &HarnessProfile,
    protected: &BTreeSet<&str>,
) -> Result<Vec<Arc<dyn AgentMiddleware>>, ProfileError> {
    // Runtime profile middleware is appended after the core stack. Factory
    // slots are merged when profiles are layered (HarnessProfile::merged), not
    // against core middleware here. Exclusions run last so they also cover
    // profile-provided middleware and can never be reintroduced afterward.
    middleware.extend(profile.materialize_extra_middleware());

    for excluded in &profile.config.excluded_middleware {
        if protected.contains(excluded.as_str()) {
            return Err(ProfileError::ProtectedMiddleware(excluded.clone()));
        }
        if !middleware.iter().any(|mw| mw.name() == excluded) {
            return Err(ProfileError::UnknownMiddleware(excluded.clone()));
        }
    }
    middleware.retain(|mw| !profile.config.excluded_middleware.contains(mw.name()));
    Ok(middleware)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(suffix: &str, excluded: &[&str]) -> HarnessProfile {
        HarnessProfile::from_config(HarnessProfileConfig {
            system_prompt_suffix: Some(suffix.to_string()),
            excluded_tools: excluded.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        })
    }

    #[test]
    fn registry_layers_general_provider_and_exact_model() {
        let registry = HarnessProfileRegistry::new(profile("general", &["a"]));
        registry
            .register("openai", profile("provider", &["b"]))
            .unwrap();
        registry
            .register("openai:gpt-x", profile("exact", &["c"]))
            .unwrap();
        let resolved = registry.resolve(Some("OpenAI"), "GPT-X");
        assert_eq!(resolved.active_key, "openai:gpt-x");
        assert_eq!(resolved.layers, ["general", "openai", "openai:gpt-x"]);
        assert_eq!(
            resolved.profile.config.system_prompt_suffix.as_deref(),
            Some("exact")
        );
        assert_eq!(
            resolved.profile.config.excluded_tools,
            ["a", "b", "c"].into_iter().map(String::from).collect()
        );
    }

    #[test]
    fn unknown_model_uses_general_fallback() {
        let registry = HarnessProfileRegistry::new(profile("general", &[]));
        let resolved = registry.resolve(None, "mystery-model");
        assert_eq!(resolved.active_key, "general");
        assert_eq!(
            resolved.profile.config.system_prompt_suffix.as_deref(),
            Some("general")
        );
    }

    #[test]
    fn reregister_merges_instead_of_replacing() {
        let registry = HarnessProfileRegistry::new(HarnessProfile::default());
        registry.register("openai", profile("one", &["a"])).unwrap();
        registry.register("openai", profile("two", &["b"])).unwrap();
        let resolved = registry.resolve(Some("openai"), "x");
        assert_eq!(
            resolved.profile.config.system_prompt_suffix.as_deref(),
            Some("two")
        );
        assert_eq!(
            resolved.profile.config.excluded_tools,
            ["a", "b"].into_iter().map(String::from).collect()
        );
    }

    #[test]
    fn merge_semantics_cover_every_declarative_field() {
        let base = HarnessProfile::from_config(HarnessProfileConfig {
            base_system_prompt: Some("base".into()),
            system_prompt_suffix: Some("base suffix".into()),
            tool_description_overrides: BTreeMap::from([
                ("keep".into(), "base keep".into()),
                ("replace".into(), "base replace".into()),
            ]),
            excluded_tools: BTreeSet::from(["base_tool".into()]),
            excluded_middleware: BTreeSet::from(["base_middleware".into()]),
            general_purpose_subagent: Some(GeneralPurposeSubagentProfile {
                enabled: Some(false),
                description: Some("base description".into()),
                system_prompt: None,
            }),
        });
        let overlay = HarnessProfile::from_config(HarnessProfileConfig {
            system_prompt_suffix: Some("overlay suffix".into()),
            tool_description_overrides: BTreeMap::from([
                ("replace".into(), "overlay replace".into()),
                ("new".into(), "overlay new".into()),
            ]),
            excluded_tools: BTreeSet::from(["overlay_tool".into()]),
            excluded_middleware: BTreeSet::from(["overlay_middleware".into()]),
            general_purpose_subagent: Some(GeneralPurposeSubagentProfile {
                enabled: None,
                description: Some("overlay description".into()),
                system_prompt: Some("overlay prompt".into()),
            }),
            ..Default::default()
        });

        let merged = base.merged(&overlay);
        assert_eq!(merged.config.base_system_prompt.as_deref(), Some("base"));
        assert_eq!(
            merged.config.system_prompt_suffix.as_deref(),
            Some("overlay suffix")
        );
        assert_eq!(
            merged.config.tool_description_overrides,
            BTreeMap::from([
                ("keep".into(), "base keep".into()),
                ("replace".into(), "overlay replace".into()),
                ("new".into(), "overlay new".into()),
            ])
        );
        assert_eq!(
            merged.config.excluded_tools,
            BTreeSet::from(["base_tool".into(), "overlay_tool".into()])
        );
        assert_eq!(
            merged.config.excluded_middleware,
            BTreeSet::from(["base_middleware".into(), "overlay_middleware".into()])
        );
        assert_eq!(
            merged.config.general_purpose_subagent,
            Some(GeneralPurposeSubagentProfile {
                enabled: Some(false),
                description: Some("overlay description".into()),
                system_prompt: Some("overlay prompt".into()),
            })
        );
    }

    #[test]
    fn config_json_round_trip_is_deterministic() {
        let config = HarnessProfileConfig {
            tool_description_overrides: BTreeMap::from([("read_file".into(), "Read it".into())]),
            excluded_tools: BTreeSet::from(["z".into(), "a".into()]),
            general_purpose_subagent: Some(GeneralPurposeSubagentProfile {
                enabled: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let encoded = serde_json::to_string(&config).unwrap();
        assert_eq!(
            encoded,
            r#"{"tool_description_overrides":{"read_file":"Read it"},"excluded_tools":["a","z"],"general_purpose_subagent":{"enabled":false}}"#
        );
        assert_eq!(
            serde_json::from_str::<HarnessProfileConfig>(&encoded).unwrap(),
            config
        );
    }

    #[test]
    fn invalid_keys_are_rejected() {
        let registry = HarnessProfileRegistry::new(HarnessProfile::default());
        for key in ["", ":x", "x:", "a:b:c"] {
            assert!(matches!(
                registry.register(key, HarnessProfile::default()),
                Err(ProfileError::InvalidKey(_))
            ));
        }
    }
    #[test]
    fn tool_profile_excludes_and_rewrites_descriptions() {
        let profile = HarnessProfile::from_config(HarnessProfileConfig {
            tool_description_overrides: BTreeMap::from([(
                "keep".to_string(),
                "profile description".to_string(),
            )]),
            excluded_tools: BTreeSet::from(["drop".to_string()]),
            ..Default::default()
        });
        let keep: Arc<dyn Tool> = Arc::new(crate::testing::NoopTool::new("keep"));
        let drop: Arc<dyn Tool> = Arc::new(crate::testing::NoopTool::new("drop"));
        assert_eq!(
            apply_tool_profile(keep, &profile)
                .unwrap()
                .def()
                .description,
            "profile description"
        );
        assert!(apply_tool_profile(drop, &profile).is_none());
    }

    struct Named(&'static str);
    #[async_trait]
    impl AgentMiddleware for Named {
        fn name(&self) -> &str {
            self.0
        }
    }

    #[test]
    fn profile_merge_replaces_same_named_factory_slot() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let old_calls = Arc::new(AtomicUsize::new(0));
        let new_calls = Arc::new(AtomicUsize::new(0));
        let base = HarnessProfile {
            extra_middleware: vec![NamedMiddlewareFactory::new("slot", {
                let old_calls = old_calls.clone();
                move || {
                    old_calls.fetch_add(1, Ordering::SeqCst);
                    Arc::new(Named("slot"))
                }
            })],
            ..Default::default()
        };
        let overlay = HarnessProfile {
            extra_middleware: vec![NamedMiddlewareFactory::new("slot", {
                let new_calls = new_calls.clone();
                move || {
                    new_calls.fetch_add(1, Ordering::SeqCst);
                    Arc::new(Named("slot"))
                }
            })],
            ..Default::default()
        };

        let merged = base.merged(&overlay);
        assert_eq!(merged.materialize_extra_middleware().len(), 1);
        assert_eq!(old_calls.load(Ordering::SeqCst), 0);
        assert_eq!(new_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn exclusions_run_after_profile_middleware_is_materialized() {
        let profile = HarnessProfile {
            config: HarnessProfileConfig {
                excluded_middleware: BTreeSet::from(["drop".to_string()]),
                ..Default::default()
            },
            extra_middleware: vec![NamedMiddlewareFactory::new("drop", || {
                Arc::new(Named("drop"))
            })],
        };
        let base: Vec<Arc<dyn AgentMiddleware>> = vec![Arc::new(Named("keep"))];
        let applied = apply_middleware_profile(base, &profile, &BTreeSet::new()).unwrap();
        assert_eq!(
            applied.iter().map(|mw| mw.name()).collect::<Vec<_>>(),
            ["keep"]
        );
    }

    #[test]
    fn protected_and_unknown_middleware_exclusions_fail_closed() {
        let protected_profile = HarnessProfile::from_config(HarnessProfileConfig {
            excluded_middleware: BTreeSet::from(["hitl".to_string()]),
            ..Default::default()
        });
        let protected_error = match apply_middleware_profile(
            vec![Arc::new(Named("keep"))],
            &protected_profile,
            &BTreeSet::from(["hitl"]),
        ) {
            Err(error) => error,
            Ok(_) => panic!("protected middleware exclusion should fail"),
        };
        assert_eq!(
            protected_error,
            ProfileError::ProtectedMiddleware("hitl".to_string())
        );

        let unknown_profile = HarnessProfile::from_config(HarnessProfileConfig {
            excluded_middleware: BTreeSet::from(["missing".to_string()]),
            ..Default::default()
        });
        let unknown_error = match apply_middleware_profile(
            vec![Arc::new(Named("keep"))],
            &unknown_profile,
            &BTreeSet::new(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("unknown middleware exclusion should fail"),
        };
        assert_eq!(
            unknown_error,
            ProfileError::UnknownMiddleware("missing".to_string())
        );
    }
}
