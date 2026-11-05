//! Model-construction profiles, separate from runtime harness behavior.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

pub type ProviderPreInit = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
pub type ProviderKwargsFactory =
    Arc<dyn Fn() -> Result<BTreeMap<String, Value>, String> + Send + Sync>;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderProfileConfig {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub init_kwargs: BTreeMap<String, Value>,
}

#[derive(Clone, Default)]
pub struct ProviderProfile {
    pub config: ProviderProfileConfig,
    pub pre_init: Option<ProviderPreInit>,
    pub init_kwargs_factory: Option<ProviderKwargsFactory>,
}

impl ProviderProfile {
    pub fn from_config(config: ProviderProfileConfig) -> Self {
        Self {
            config,
            pre_init: None,
            init_kwargs_factory: None,
        }
    }

    pub fn merged(&self, overlay: &Self) -> Self {
        let mut init_kwargs = self.config.init_kwargs.clone();
        init_kwargs.extend(overlay.config.init_kwargs.clone());

        let pre_init = match (&self.pre_init, &overlay.pre_init) {
            (Some(base), Some(over)) => {
                let base = base.clone();
                let over = over.clone();
                Some(Arc::new(move |spec: &str| {
                    base(spec)?;
                    over(spec)
                }) as ProviderPreInit)
            }
            (_, Some(over)) => Some(over.clone()),
            (Some(base), None) => Some(base.clone()),
            (None, None) => None,
        };

        let init_kwargs_factory = match (&self.init_kwargs_factory, &overlay.init_kwargs_factory) {
            (Some(base), Some(over)) => {
                let base = base.clone();
                let over = over.clone();
                Some(Arc::new(move || {
                    let mut kwargs = base()?;
                    kwargs.extend(over()?);
                    Ok(kwargs)
                }) as ProviderKwargsFactory)
            }
            (_, Some(over)) => Some(over.clone()),
            (Some(base), None) => Some(base.clone()),
            (None, None) => None,
        };

        Self {
            config: ProviderProfileConfig { init_kwargs },
            pre_init,
            init_kwargs_factory,
        }
    }

    /// Resolve profile defaults beneath caller-supplied values.
    pub fn apply(
        &self,
        spec: &str,
        caller_kwargs: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, ProviderProfileError> {
        if let Some(pre_init) = &self.pre_init {
            pre_init(spec).map_err(ProviderProfileError::Hook)?;
        }
        let mut kwargs = self.config.init_kwargs.clone();
        if let Some(factory) = &self.init_kwargs_factory {
            kwargs.extend(factory().map_err(ProviderProfileError::Hook)?);
        }
        kwargs.extend(caller_kwargs.clone());
        Ok(kwargs)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProviderProfileError {
    #[error("invalid provider profile key '{0}'")]
    InvalidKey(String),
    #[error("provider profile hook failed: {0}")]
    Hook(String),
}

#[derive(Clone)]
pub struct ResolvedProviderProfile {
    pub active_key: Option<String>,
    pub layers: Vec<String>,
    pub profile: ProviderProfile,
}

#[derive(Clone, Default)]
pub struct ProviderProfileRegistry {
    entries: Arc<RwLock<BTreeMap<String, ProviderProfile>>>,
}

impl ProviderProfileRegistry {
    pub fn register(
        &self,
        key: impl Into<String>,
        profile: ProviderProfile,
    ) -> Result<(), ProviderProfileError> {
        let raw = key.into();
        let key = normalize_key(&raw)?;
        let mut entries = self.entries.write().unwrap();
        let merged = entries
            .get(&key)
            .map_or(profile.clone(), |existing| existing.merged(&profile));
        entries.insert(key, merged);
        Ok(())
    }

    pub fn resolve(&self, provider: Option<&str>, model: &str) -> ResolvedProviderProfile {
        let entries = self.entries.read().unwrap();
        let mut profile = ProviderProfile::default();
        let mut layers = Vec::new();
        let mut active_key = None;
        if let Some(provider) = provider.map(str::trim).filter(|s| !s.is_empty()) {
            let provider_key = provider.to_ascii_lowercase();
            if let Some(base) = entries.get(&provider_key) {
                profile = profile.merged(base);
                active_key = Some(provider_key.clone());
                layers.push(provider_key.clone());
            }
            let exact_key = format!("{provider_key}:{}", model.trim().to_ascii_lowercase());
            if let Some(exact) = entries.get(&exact_key) {
                profile = profile.merged(exact);
                active_key = Some(exact_key.clone());
                layers.push(exact_key);
            }
        }
        ResolvedProviderProfile {
            active_key,
            layers,
            profile,
        }
    }
}

fn normalize_key(raw: &str) -> Result<String, ProviderProfileError> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty() || key.matches(':').count() > 1 {
        return Err(ProviderProfileError::InvalidKey(raw.to_string()));
    }
    if let Some((provider, model)) = key.split_once(':') {
        if provider.is_empty() || model.is_empty() {
            return Err(ProviderProfileError::InvalidKey(raw.to_string()));
        }
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_and_exact_kwargs_merge_beneath_caller_values() {
        let registry = ProviderProfileRegistry::default();
        registry
            .register(
                "openai",
                ProviderProfile::from_config(ProviderProfileConfig {
                    init_kwargs: BTreeMap::from([
                        ("temperature".into(), Value::from(0)),
                        ("timeout".into(), Value::from(30)),
                    ]),
                }),
            )
            .unwrap();
        registry
            .register(
                "openai:gpt-x",
                ProviderProfile::from_config(ProviderProfileConfig {
                    init_kwargs: BTreeMap::from([("temperature".into(), Value::from(1))]),
                }),
            )
            .unwrap();
        let resolved = registry.resolve(Some("openai"), "gpt-x");
        let applied = resolved
            .profile
            .apply(
                "openai:gpt-x",
                &BTreeMap::from([("timeout".into(), Value::from(60))]),
            )
            .unwrap();
        assert_eq!(applied["temperature"], 1);
        assert_eq!(applied["timeout"], 60);
    }

    #[test]
    fn unknown_provider_resolves_to_an_empty_profile() {
        let resolved = ProviderProfileRegistry::default().resolve(Some("unknown"), "model");
        assert_eq!(resolved.active_key, None);
        assert!(resolved.layers.is_empty());
        assert!(resolved.profile.config.init_kwargs.is_empty());
    }

    #[test]
    fn invalid_provider_keys_are_rejected() {
        let registry = ProviderProfileRegistry::default();
        for key in ["", ":x", "x:", "a:b:c"] {
            assert!(matches!(
                registry.register(key, ProviderProfile::default()),
                Err(ProviderProfileError::InvalidKey(_))
            ));
        }
    }

    #[test]
    fn provider_hooks_chain_base_then_override() {
        let registry = ProviderProfileRegistry::default();
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        for label in ["base", "override"] {
            let log = log.clone();
            registry
                .register(
                    "openai",
                    ProviderProfile {
                        pre_init: Some(Arc::new(move |_| {
                            log.lock().unwrap().push(label);
                            Ok(())
                        })),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        registry
            .resolve(Some("openai"), "x")
            .profile
            .apply("openai:x", &BTreeMap::new())
            .unwrap();
        assert_eq!(*log.lock().unwrap(), ["base", "override"]);
    }
}
