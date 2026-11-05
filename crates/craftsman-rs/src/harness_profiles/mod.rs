//! Craftsman's built-in provider and harness profile catalog and resolution.
//!
//! Model-family definitions live in sibling modules so each profile can evolve
//! independently without turning this registry/resolution module into a prompt
//! monolith.

mod claude;
mod general;
mod nemotron;
mod openai;
mod providers;

pub use general::{GENERAL_PURPOSE_DESCRIPTION, GENERAL_PURPOSE_SYSTEM_PROMPT};

use crate::config::Config;
use crate::route::ChatRoute;
use craftsman_harness::{
    HarnessProfileRegistry, ProviderProfileRegistry, ResolvedHarnessProfile,
    ResolvedProviderProfile,
};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessPresetMode {
    /// Detect behavior from the selected model on every session build.
    Auto,
    /// Use only Craftsman's unchanged general fallback.
    General,
    /// Force one registered model-family profile.
    Family(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub mode: HarnessPresetMode,
}

const HARNESS_PRESETS: &[HarnessPreset] = &[
    HarnessPreset {
        id: "auto",
        label: "Automatic",
        description: "Select by model; unknown models use General",
        mode: HarnessPresetMode::Auto,
    },
    HarnessPreset {
        id: "general",
        label: "General",
        description: "Original Craftsman behavior with no model-specific tuning",
        mode: HarnessPresetMode::General,
    },
    HarnessPreset {
        id: "openai",
        label: "OpenAI / Codex",
        description: "Autonomous execution, parallel tools, and TODO hygiene",
        mode: HarnessPresetMode::Family("openai"),
    },
    HarnessPreset {
        id: "claude",
        label: "Claude",
        description: "Grounded investigation, reflection, and parallel tool use",
        mode: HarnessPresetMode::Family("anthropic"),
    },
    HarnessPreset {
        id: "nemotron",
        label: "NVIDIA Nemotron",
        description: "Tool grounding, bounded retries, and complete final answers",
        mode: HarnessPresetMode::Family("nvidia"),
    },
];

pub fn harness_presets() -> &'static [HarnessPreset] {
    HARNESS_PRESETS
}

pub fn find_harness_preset(value: &str) -> Option<&'static HarnessPreset> {
    let canonical = match value.trim().to_ascii_lowercase().as_str() {
        "auto" | "automatic" => "auto",
        "general" | "default" => "general",
        "openai" | "codex" => "openai",
        "claude" | "anthropic" => "claude",
        "nemotron" | "nvidia" => "nemotron",
        _ => return None,
    };
    HARNESS_PRESETS.iter().find(|preset| preset.id == canonical)
}

pub fn configured_harness_preset(cfg: &Config) -> &'static HarnessPreset {
    let configured = cfg
        .extra
        .get("harness_profile")
        .or_else(|| cfg.extra.get("harness_profile_key"))
        .and_then(Value::as_str)
        .unwrap_or("auto");
    find_harness_preset(configured).unwrap_or(&HARNESS_PRESETS[0])
}

pub fn select_harness_preset(
    cfg: &mut Config,
    value: &str,
) -> Result<&'static HarnessPreset, String> {
    let Some(preset) = find_harness_preset(value) else {
        return Err(format!(
            "unknown harness profile '{value}'; choose auto, general, openai, claude, or nemotron"
        ));
    };
    cfg.extra.remove("harness_profile_key");
    if preset.mode == HarnessPresetMode::Auto {
        cfg.extra.remove("harness_profile");
    } else {
        cfg.extra.insert(
            "harness_profile".into(),
            Value::String(preset.id.to_string()),
        );
    }
    Ok(preset)
}

struct ProfileCatalog {
    harness: HarnessProfileRegistry,
    provider: ProviderProfileRegistry,
}

static CATALOG: OnceLock<ProfileCatalog> = OnceLock::new();

fn catalog() -> &'static ProfileCatalog {
    CATALOG.get_or_init(|| {
        let harness = HarnessProfileRegistry::new(general::profile());
        openai::register(&harness);
        claude::register(&harness);
        nemotron::register(&harness);

        let provider = ProviderProfileRegistry::default();
        providers::register(&provider);

        ProfileCatalog { harness, provider }
    })
}

#[derive(Clone)]
pub struct ResolvedProfiles {
    pub provider: Option<String>,
    pub provider_profile_mode: String,
    pub model_id: String,
    pub harness_preset: String,
    pub harness: ResolvedHarnessProfile,
    pub provider_profile: ResolvedProviderProfile,
}

pub fn resolve(cfg: &Config, route: &ChatRoute, model: &str) -> ResolvedProfiles {
    let identity = model_identity(cfg, route, model);
    let preset = configured_harness_preset(cfg);
    let harness = match preset.mode {
        HarnessPresetMode::Auto => catalog()
            .harness
            .resolve(identity.harness_family.as_deref(), &identity.model_id),
        HarnessPresetMode::General => catalog().harness.resolve(None, &identity.model_id),
        HarnessPresetMode::Family(family) => {
            catalog().harness.resolve(Some(family), &identity.model_id)
        }
    };
    let provider_profile = catalog()
        .provider
        .resolve(identity.provider.as_deref(), &identity.model_id);
    ResolvedProfiles {
        harness,
        harness_preset: preset.id.to_string(),
        model_id: identity.model_id,
        provider_profile,
        provider: identity.provider,
        provider_profile_mode: identity.provider_profile_mode,
    }
}

struct ModelIdentity {
    provider: Option<String>,
    provider_profile_mode: String,
    harness_family: Option<String>,
    model_id: String,
}

fn model_identity(cfg: &Config, route: &ChatRoute, model: &str) -> ModelIdentity {
    let configured_model_provider = cfg
        .extra
        .get("model_provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let spec = model
        .split_once(':')
        .filter(|(provider, model_id)| !provider.is_empty() && !model_id.is_empty());
    let declared_provider = configured_model_provider
        .or_else(|| spec.map(|(provider, _)| provider.to_ascii_lowercase()));
    let inferred_direct_provider = declared_provider.clone().or_else(|| {
        route
            .base_url
            .to_ascii_lowercase()
            .contains("api.openai.com")
            .then(|| "openai".to_string())
    });
    let configured_provider_profile = cfg
        .extra
        .get("provider_profile")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let (provider, provider_profile_mode) = match configured_provider_profile.as_deref() {
        Some("none" | "off" | "disabled") => (None, "disabled".to_string()),
        Some("auto") | None => (inferred_direct_provider, "auto".to_string()),
        Some(provider) => (Some(provider.to_string()), format!("explicit:{provider}")),
    };
    let model_id = spec
        .map(|(_, model_id)| model_id.to_string())
        .unwrap_or_else(|| strip_provider(model));
    let lower = model_id.to_ascii_lowercase();
    let family = if lower.contains("nemotron") {
        Some("nvidia")
    } else if lower.contains("claude") {
        Some("anthropic")
    } else if lower.contains("codex")
        || lower.starts_with("gpt-")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
    {
        Some("openai")
    } else {
        None
    };
    let harness_family = family
        .map(str::to_string)
        .or(declared_provider)
        .or_else(|| provider.clone());

    ModelIdentity {
        provider,
        provider_profile_mode,
        harness_family,
        model_id,
    }
}

fn strip_provider(model: &str) -> String {
    model
        .split_once(':')
        .map_or_else(|| model.to_string(), |(_, model)| model.to_string())
}

#[cfg(test)]
mod tests;
