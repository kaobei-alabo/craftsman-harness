//! Terminal model catalog metadata and explicit per-model reasoning controls.
//! Metadata is never forwarded as generation parameters.
use crate::config::Config;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|effort| effort.as_str()==text).ok_or_else(|| "Unknown reasoning effort. Use default, none, minimal, low, medium, high, xhigh or max.".into())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CatalogModel {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub reasoning_efforts: Vec<ReasoningEffort>,
}

impl CatalogModel {
    pub fn named(id: String) -> Self {
        Self {
            id,
            label: String::new(),
            reasoning_efforts: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        validate_model_id(&self.id)?;
        if self.label.len() > 512 || self.reasoning_efforts.len() > ReasoningEffort::ALL.len() {
            return Err("Model catalog metadata exceeds its limit.".into());
        }
        for (index, effort) in self.reasoning_efforts.iter().enumerate() {
            if self.reasoning_efforts[..index].contains(effort) {
                return Err("Model catalog repeats a reasoning effort.".into());
            }
        }
        Ok(())
    }
}

pub fn validate_model_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
        return Err(
            "Model id must be nonempty, at most 512 UTF-8 bytes and contain no control characters."
                .into(),
        );
    }
    Ok(())
}

pub fn configured_catalog(cfg: &Config) -> Result<Vec<CatalogModel>, String> {
    if !cfg.providers.is_empty() {
        return crate::provider_config::catalog(cfg);
    }
    let entries: Vec<CatalogModel> = match cfg.extra.get("model_catalog") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|_| "Invalid model_catalog configuration.".to_string())?,
        None => Vec::new(),
    };
    if entries.len() > 512 {
        return Err("Model catalog exceeds 512 models.".into());
    }
    let mut models = BTreeMap::new();
    for entry in entries {
        entry.validate()?;
        if models.insert(entry.id.clone(), entry).is_some() {
            return Err("Model catalog repeats a model id.".into());
        }
    }
    if let Some(profiles) = cfg.extra.get("model_profiles").and_then(Value::as_object) {
        if profiles.len() > 512 {
            return Err("Model profiles exceed 512 models.".into());
        }
        for id in profiles.keys() {
            validate_model_id(id)?;
            models
                .entry(id.clone())
                .or_insert_with(|| CatalogModel::named(id.clone()));
        }
    }
    if let Some(id) = &cfg.model {
        validate_model_id(id)?;
        models
            .entry(id.clone())
            .or_insert_with(|| CatalogModel::named(id.clone()));
    }
    if models.len() > 512 {
        return Err("Model catalog exceeds 512 models.".into());
    }
    Ok(models.into_values().collect())
}

pub fn merge_catalog(cfg: &Config, remote: Vec<CatalogModel>) -> Result<Vec<CatalogModel>, String> {
    if remote.len() > 512 {
        return Err("Provider catalog exceeds 512 models.".into());
    }
    let mut models = BTreeMap::new();
    for entry in remote {
        entry.validate()?;
        if !cfg.providers.is_empty() && crate::provider_config::selection(cfg, &entry.id).is_err() {
            continue;
        }
        models.insert(entry.id.clone(), entry);
    }
    for configured in configured_catalog(cfg)? {
        match models.get_mut(&configured.id) {
            Some(remote) => {
                if !configured.label.is_empty() {
                    remote.label = configured.label;
                }
                if !configured.reasoning_efforts.is_empty() {
                    remote.reasoning_efforts = configured.reasoning_efforts;
                }
            }
            None => {
                models.insert(configured.id.clone(), configured);
            }
        }
    }
    if models.len() > 512 {
        return Err("Combined model catalog exceeds 512 models.".into());
    }
    Ok(models.into_values().collect())
}

/// None = original caller options. Some(None) = explicit provider default,
/// distinct from the model's literal `none` reasoning effort.
pub fn selected_effort(
    cfg: &Config,
    model: &str,
) -> Result<Option<Option<ReasoningEffort>>, String> {
    if !cfg.providers.is_empty() {
        let chosen = crate::provider_config::selection(cfg, model)?;
        return match chosen
            .entry
            .map(crate::provider_config::reasoning)
            .transpose()?
            .flatten()
        {
            Some(crate::provider_config::Reasoning::Effort(effort)) => Ok(Some(Some(effort))),
            _ => Ok(Some(None)),
        };
    }
    let Some(settings) = cfg.extra.get("model_settings") else {
        return Ok(None);
    };
    let settings = settings
        .as_object()
        .ok_or("Invalid model_settings configuration.")?;
    let Some(settings) = settings.get(model) else {
        return Ok(None);
    };
    let settings = settings.as_object().ok_or("Invalid per-model settings.")?;
    match settings.get("reasoning_effort") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(effort)) => Ok(Some(Some(ReasoningEffort::parse(effort)?))),
        _ => Err("Reasoning effort must be a string or null for provider default.".into()),
    }
}

pub fn apply(
    cfg: &Config,
    model: &str,
    options: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    if !cfg.providers.is_empty() {
        options.clear();
        options.extend(crate::provider_config::options(cfg, model)?);
        return Ok(());
    }
    match selected_effort(cfg, model)? {
        None => {}
        Some(None) => {
            options.remove("reasoning_effort");
        }
        Some(Some(effort)) => {
            options.insert(
                "reasoning_effort".into(),
                Value::String(effort.as_str().into()),
            );
        }
    }
    Ok(())
}

pub fn set_effort(
    cfg: &mut Config,
    model: &str,
    effort: Option<ReasoningEffort>,
) -> Result<(), String> {
    if !cfg.providers.is_empty() {
        return crate::provider_config::set_effort(cfg, model, effort);
    }
    validate_model_id(model)?;
    if let Some(effort) = effort {
        if let Some(entry) = configured_catalog(cfg)?
            .into_iter()
            .find(|entry| entry.id == model)
        {
            if !entry.reasoning_efforts.is_empty() && !entry.reasoning_efforts.contains(&effort) {
                return Err("This effort is not declared for the selected model.".into());
            }
        }
    }
    let settings = cfg
        .extra
        .entry("model_settings".into())
        .or_insert_with(|| serde_json::json!({}));
    let settings = settings
        .as_object_mut()
        .ok_or("Invalid model_settings configuration.")?;
    let entry = settings
        .entry(model)
        .or_insert_with(|| serde_json::json!({}));
    let entry = entry.as_object_mut().ok_or("Invalid per-model settings.")?;
    entry.insert(
        "reasoning_effort".into(),
        effort.map_or(Value::Null, |e| Value::String(e.as_str().into())),
    );
    Ok(())
}

pub fn display_effort(cfg: &Config) -> Option<String> {
    let model = cfg.model.as_deref()?;
    if !cfg.providers.is_empty() {
        let chosen = crate::provider_config::selection(cfg, model).ok()?;
        return Some(
            match chosen
                .entry
                .and_then(|entry| crate::provider_config::reasoning(entry).ok())
                .flatten()
            {
                Some(crate::provider_config::Reasoning::Effort(e)) => e.as_str().into(),
                Some(crate::provider_config::Reasoning::Budget(n)) => format!("budget:{n}"),
                None => "default".into(),
            },
        );
    }
    match selected_effort(cfg, model).ok()? {
        Some(Some(effort)) => Some(effort.as_str().into()),
        Some(None) => Some("default".into()),
        None => cfg
            .extra
            .get("model_kwargs")
            .and_then(|v| v.get("reasoning_effort"))
            .and_then(Value::as_str)
            .or_else(|| {
                cfg.extra
                    .get("model_profiles")
                    .and_then(|v| v.get(model))
                    .and_then(|v| v.get("reasoning_effort"))
                    .and_then(Value::as_str)
            })
            .map(str::to_owned),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ModelSelection {
    pub model: Option<String>,
    pub effort: Option<Option<ReasoningEffort>>,
}

pub fn parse_selection<'a>(
    tokens: impl Iterator<Item = &'a str>,
) -> Result<ModelSelection, String> {
    let mut selection = ModelSelection {
        model: None,
        effort: None,
    };
    for token in tokens {
        if let Some(effort) = token.strip_prefix("--reasoning=") {
            if selection.effort.is_some() {
                return Err("Reasoning effort was specified twice.".into());
            }
            selection.effort = Some(if effort == "default" {
                None
            } else {
                Some(ReasoningEffort::parse(effort)?)
            });
        } else if token.starts_with('-') || selection.model.is_some() {
            return Err("Usage: /model [model-id] [--reasoning=level]".into());
        } else {
            validate_model_id(token)?;
            selection.model = Some(token.into());
        }
    }
    Ok(selection)
}

#[cfg(test)]
#[path = "model_controls/tests.rs"]
mod tests;
