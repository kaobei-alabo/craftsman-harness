//! Explicit, provider-owned model catalogs. Credentials are resolved only for
//! the selected connection; catalog metadata is never a generation option.
use crate::config::{Config, expand_env_placeholders};
use crate::model_controls::{CatalogModel, ReasoningEffort};
use crate::route::ChatRoute;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    #[default]
    OpenaiChatCompletions,
    OpenaiResponses,
    AnthropicMessages,
    GeminiGenerateContent,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
pub struct Provider {
    pub protocol: Protocol,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default, deserialize_with = "unique_map")]
    pub models: BTreeMap<String, CatalogEntry>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Provider { connection: [redacted] }")
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct CatalogEntry {
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_controls: Option<ReasoningControls>,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ModelCapabilities {
    #[serde(default)]
    pub tool_calling: Option<bool>,
    #[serde(default)]
    pub streaming: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
fn usable(entry: &CatalogEntry) -> bool {
    entry
        .capabilities
        .as_ref()
        .is_none_or(|caps| caps.tool_calling != Some(false) && caps.streaming != Some(false))
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningControls {
    Effort { values: Vec<ReasoningEffort> },
    Budget { min: u32, max: u32 },
}

/// Map visitors refuse duplicate provider/model names before JSON loses them.
pub fn unique_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a map with unique names")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some((name, value)) = map.next_entry::<String, T>()? {
                if out.insert(name, value).is_some() {
                    return Err(serde::de::Error::custom(
                        "Duplicate provider or model name.",
                    ));
                }
                if out.len() > 512 {
                    return Err(serde::de::Error::custom(
                        "Provider catalog exceeds its limit.",
                    ));
                }
            }
            Ok(out)
        }
    }
    deserializer.deserialize_map(Visitor(std::marker::PhantomData))
}

pub struct Selection<'a> {
    pub provider_id: &'a str,
    pub model_id: &'a str,
    pub provider: &'a Provider,
    pub entry: Option<&'a CatalogEntry>,
}
pub fn selection<'a>(cfg: &'a Config, id: &'a str) -> Result<Selection<'a>, String> {
    validate(cfg)?;
    crate::model_controls::validate_model_id(id)?;
    let (provider_id, model_id) = id
        .split_once('/')
        .ok_or("Use provider/model-id to select a model.")?;
    crate::model_controls::validate_model_id(model_id)?;
    if model_id.trim() != model_id || model_id.chars().any(char::is_whitespace) {
        return Err("Invalid provider model ID.".into());
    }
    let provider = cfg
        .providers
        .get(provider_id)
        .ok_or("Unknown model provider.")?;
    if provider
        .models
        .get(model_id)
        .is_some_and(|entry| !usable(entry))
    {
        return Err("Selected model does not support coding-agent tools and streaming.".into());
    }
    Ok(Selection {
        provider_id,
        model_id,
        provider,
        entry: provider.models.get(model_id),
    })
}

pub fn validate(cfg: &Config) -> Result<(), String> {
    if serde_json::to_vec(&cfg.providers)
        .map_err(|_| "Invalid provider configuration.")?
        .len()
        > 1_048_576
    {
        return Err("Provider configuration exceeds 1 MiB.".into());
    }
    if cfg.providers.len() > 16 {
        return Err("Configuration exceeds 16 providers.".into());
    }
    let mut count = 0usize;
    for (name, provider) in &cfg.providers {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(
                "Provider names must contain only letters, numbers, underscore or hyphen.".into(),
            );
        }
        let base = endpoint(provider);
        let url = reqwest::Url::parse(base).map_err(|_| "Invalid provider endpoint.")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || base.len() > 4096
        {
            return Err(
                "Provider endpoint must be HTTP(S), without URL credentials, query or fragment."
                    .into(),
            );
        }
        if provider.api_key.len() > 16384 {
            return Err("Provider credential exceeds its limit.".into());
        }
        count = count
            .checked_add(provider.models.len())
            .ok_or("Provider catalog exceeds its limit.")?;
        if count > 512 {
            return Err("Combined model catalog exceeds 512 models.".into());
        }
        for (id, entry) in &provider.models {
            crate::model_controls::validate_model_id(&format!("{name}/{id}"))?;
            if id.trim() != id || id.chars().any(char::is_whitespace) || entry.label.len() > 512 {
                return Err("Invalid provider model metadata.".into());
            }
            if let Some(controls) = &entry.reasoning_controls {
                match controls {
                    ReasoningControls::Effort { values } => {
                        if provider.protocol == Protocol::GeminiGenerateContent
                            && values.iter().any(|value| {
                                !matches!(
                                    value,
                                    ReasoningEffort::Minimal
                                        | ReasoningEffort::Low
                                        | ReasoningEffort::Medium
                                        | ReasoningEffort::High
                                )
                            })
                        {
                            return Err(
                                "Native Gemini thinking levels are minimal, low, medium or high."
                                    .into(),
                            );
                        }
                        if values.is_empty()
                            || values.len() > ReasoningEffort::ALL.len()
                            || values
                                .iter()
                                .enumerate()
                                .any(|(i, v)| values[..i].contains(v))
                        {
                            return Err("Invalid model reasoning choices.".into());
                        }
                    }
                    ReasoningControls::Budget { min, max } if min > max || *max > 1_000_000 => {
                        return Err("Invalid model thinking budget bounds.".into());
                    }
                    _ => {}
                }
            }
            let selected_reasoning = reasoning(entry)?;
            if let Some(Reasoning::Effort(value)) = selected_reasoning {
                if provider.protocol == Protocol::GeminiGenerateContent
                    && !matches!(
                        value,
                        ReasoningEffort::Minimal
                            | ReasoningEffort::Low
                            | ReasoningEffort::Medium
                            | ReasoningEffort::High
                    )
                {
                    return Err(
                        "Native Gemini thinking levels are minimal, low, medium or high.".into(),
                    );
                }
            }
            if let Some(Reasoning::Budget(budget)) = selected_reasoning {
                match provider.protocol {
                    Protocol::AnthropicMessages => {
                        let limit = entry
                            .settings
                            .get("generation")
                            .and_then(|value| value.get("max_tokens"))
                            .and_then(Value::as_u64)
                            .unwrap_or(8192);
                        if budget < 1024 || u64::from(budget) >= limit {
                            return Err("Claude thinking budget must be at least 1024 and below max_tokens.".into());
                        }
                    }
                    Protocol::GeminiGenerateContent => {}
                    _ => {
                        return Err(
                            "This protocol does not support a thinking token budget.".into()
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn endpoint(provider: &Provider) -> &str {
    if !provider.base_url.is_empty() {
        return &provider.base_url;
    }
    match provider.protocol {
        Protocol::OpenaiChatCompletions | Protocol::OpenaiResponses => "https://api.openai.com/v1",
        Protocol::AnthropicMessages => "https://api.anthropic.com/v1",
        Protocol::GeminiGenerateContent => "https://generativelanguage.googleapis.com/v1beta",
    }
}

pub async fn resolve(cfg: &Config) -> Result<(ChatRoute, Protocol, String), String> {
    let id = cfg.model.as_deref().ok_or("No model selected.")?;
    let chosen = selection(cfg, id)?;
    let api_key = expand_env_placeholders(&chosen.provider.api_key)
        .map_err(|_| "Selected provider credential could not be resolved.")?;
    Ok((
        ChatRoute {
            base_url: endpoint(chosen.provider).trim_end_matches('/').into(),
            api_key,
            source: "configured provider",
        },
        chosen.provider.protocol,
        chosen.model_id.into(),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reasoning {
    Effort(ReasoningEffort),
    Budget(u32),
}
pub fn reasoning(entry: &CatalogEntry) -> Result<Option<Reasoning>, String> {
    let Some(value) = entry.settings.get("reasoning").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let obj = value
        .as_object()
        .ok_or("Invalid model reasoning setting.")?;
    let result = match obj.get("type").and_then(Value::as_str) {
        Some("effort") => Reasoning::Effort(ReasoningEffort::parse(
            obj.get("value")
                .and_then(Value::as_str)
                .ok_or("Invalid reasoning effort.")?,
        )?),
        Some("budget") => Reasoning::Budget(
            obj.get("value")
                .and_then(Value::as_u64)
                .filter(|n| *n <= 1_000_000)
                .ok_or("Invalid thinking budget.")? as u32,
        ),
        _ => return Err("Unknown reasoning control type.".into()),
    };
    match (&entry.reasoning_controls, result) {
        (Some(ReasoningControls::Effort { values }), Reasoning::Effort(e))
            if values.contains(&e) => {}
        (Some(ReasoningControls::Budget { min, max }), Reasoning::Budget(n))
            if (*min..=*max).contains(&n) => {}
        (None, Reasoning::Effort(_)) => {}
        _ => return Err("Reasoning setting is not declared for this model.".into()),
    }
    Ok(Some(result))
}

pub fn catalog(cfg: &Config) -> Result<Vec<CatalogModel>, String> {
    validate(cfg)?;
    let mut out = Vec::new();
    for (provider_id, provider) in &cfg.providers {
        for (model_id, entry) in &provider.models {
            if !usable(entry) {
                continue;
            }
            out.push(CatalogModel {
                id: format!("{provider_id}/{model_id}"),
                label: entry.label.clone(),
                reasoning_efforts: match &entry.reasoning_controls {
                    Some(ReasoningControls::Effort { values }) => values.clone(),
                    _ => Vec::new(),
                },
            });
        }
    }
    if let Some(id) = &cfg.model {
        if selection(cfg, id).is_ok() && !out.iter().any(|m| &m.id == id) {
            out.push(CatalogModel::named(id.clone()));
        }
    }
    if out.len() > 512 {
        return Err("Combined model catalog exceeds 512 models.".into());
    }
    Ok(out)
}

pub fn set_effort(
    cfg: &mut Config,
    model: &str,
    effort: Option<ReasoningEffort>,
) -> Result<(), String> {
    let chosen = selection(cfg, model)?;
    let provider_id = chosen.provider_id.to_owned();
    let model_id = chosen.model_id.to_owned();
    let mut entry = chosen.entry.cloned().unwrap_or_default();
    entry.settings.insert(
        "reasoning".into(),
        effort.map_or(
            Value::Null,
            |value| serde_json::json!({"type":"effort","value":value}),
        ),
    );
    reasoning(&entry)?;
    let mut candidate = cfg.clone();
    candidate
        .providers
        .get_mut(&provider_id)
        .unwrap()
        .models
        .insert(model_id, entry);
    validate(&candidate)?;
    *cfg = candidate;
    Ok(())
}

pub fn options(cfg: &Config, model: &str) -> Result<BTreeMap<String, Value>, String> {
    let chosen = selection(cfg, model)?;
    let mut out = BTreeMap::new();
    if let Some(entry) = chosen.entry {
        if let Some(value) = entry.settings.get("generation") {
            let object = value
                .as_object()
                .ok_or("Model generation settings must be an object.")?;
            out.extend(object.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        // Internal transport control; the native adapter maps it to its wire form.
        if let Some(value) = reasoning(entry)? {
            out.insert(
                "craftsman_reasoning".into(),
                match value {
                    Reasoning::Effort(e) => serde_json::json!({"type":"effort","value":e}),
                    Reasoning::Budget(n) => serde_json::json!({"type":"budget","value":n}),
                },
            );
        }
    }
    Ok(out)
}

pub fn set_budget(cfg: &mut Config, model: &str, budget: u32) -> Result<(), String> {
    let chosen = selection(cfg, model)?;
    let provider_id = chosen.provider_id.to_owned();
    let model_id = chosen.model_id.to_owned();
    let mut entry = chosen.entry.cloned().unwrap_or_default();
    entry.settings.insert(
        "reasoning".into(),
        serde_json::json!({"type":"budget","value":budget}),
    );
    reasoning(&entry)?;
    let mut candidate = cfg.clone();
    candidate
        .providers
        .get_mut(&provider_id)
        .unwrap()
        .models
        .insert(model_id, entry);
    validate(&candidate)?;
    *cfg = candidate;
    Ok(())
}

#[cfg(test)]
mod tests;
