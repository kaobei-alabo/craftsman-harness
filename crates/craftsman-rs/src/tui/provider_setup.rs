//! Edit configured provider connections without flattening or losing their catalog.
use super::app::App;
use super::modals::{AppAction, Modal, SelectionItem, SelectionModal};
use super::session_bridge::AgentSlot;
use crate::config::Config;
use crate::hitl::ApprovalState;
use crate::provider_config::{CatalogEntry, Protocol};
use std::sync::{Arc, Mutex};

fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenaiResponses => "OpenAI Responses",
        Protocol::OpenaiChatCompletions => "OpenAI-compatible Chat Completions",
        Protocol::AnthropicMessages => "Claude Messages",
        Protocol::GeminiGenerateContent => "Gemini GenerateContent",
    }
}

pub(super) fn show(cfg: &Config, app: &mut App) {
    if cfg.providers.is_empty() {
        super::modals::show_setup_modal(app);
        return;
    }
    if let Err(error) = crate::provider_config::validate(cfg) {
        app.push_error(error);
        return;
    }
    let selected = cfg
        .model
        .as_deref()
        .and_then(|id| id.split_once('/'))
        .map(|(provider, _)| provider);
    app.modal = Some(Modal::Selection(SelectionModal {
        title: "Configure a provider connection".into(),
        selected: cfg
            .providers
            .keys()
            .position(|name| Some(name.as_str()) == selected)
            .unwrap_or(0),
        items: cfg
            .providers
            .iter()
            .map(|(name, provider)| SelectionItem {
                label: name.clone(),
                detail: protocol_label(provider.protocol).into(),
                enter_action: Some(AppAction::EditProvider(name.clone())),
                delete_action: None,
            })
            .collect(),
        hint: "↑/↓ choose provider · Enter edit · Esc cancel".into(),
    }));
    app.dirty = true;
}

pub(super) fn edit(cfg: &Config, name: &str, app: &mut App) {
    if let Err(error) = crate::provider_config::validate(cfg) {
        app.push_error(error);
        return;
    }
    let Some(provider) = cfg.providers.get(name) else {
        app.push_error("Unknown model provider.");
        return;
    };
    let model = cfg
        .model
        .as_deref()
        .and_then(|id| id.split_once('/'))
        .filter(|(owner, _)| *owner == name)
        .map(|(_, model)| model)
        .unwrap_or("");
    super::modals::show_provider_setup_modal(
        app,
        name.into(),
        protocol_label(provider.protocol),
        vec![
            crate::provider_config::endpoint(provider).into(),
            provider.api_key.clone(),
            model.into(),
        ],
    );
}

fn candidate(
    cfg: &Config,
    name: &str,
    base_url: String,
    api_key: String,
    model: String,
) -> Result<Config, String> {
    if model.is_empty() || model.trim() != model || model.chars().any(char::is_whitespace) {
        return Err("Enter the provider's actual API model ID.".into());
    }
    let mut candidate = cfg.clone();
    let provider = candidate
        .providers
        .get_mut(name)
        .ok_or("Unknown model provider.")?;
    provider.base_url = base_url.trim_end_matches('/').into();
    provider.api_key = api_key;
    // Existing entries retain their settings; a new API ID gets provider
    // defaults rather than another model's effort/generation metadata.
    provider
        .models
        .entry(model.clone())
        .or_insert_with(CatalogEntry::default);
    let selection = format!("{name}/{model}");
    crate::provider_config::selection(&candidate, &selection)?;
    candidate.model = Some(selection);
    Ok(candidate)
}

pub(super) fn save(
    name: &str,
    base_url: String,
    api_key: String,
    model: String,
    cfg: &mut Config,
    slot: &mut AgentSlot,
    approval: &Arc<Mutex<ApprovalState>>,
    app: &mut App,
) {
    let candidate = match candidate(cfg, name, base_url, api_key, model) {
        Ok(candidate) => candidate,
        Err(error) => {
            app.push_error(error);
            return;
        }
    };
    if candidate.save().is_err() {
        app.push_error(
            "Provider setup could not be saved. The current configuration and session were kept.",
        );
        return;
    }
    *cfg = candidate;
    app.push(format!(
        "Provider {name} configured; model {} selected.",
        cfg.model.as_deref().unwrap_or_default()
    ));
    // The existing owner drains background/LSP work before transferring actual
    // committed history, approval, thread, sequence and usage without replay.
    slot.rebuild(cfg, approval.clone());
}

#[cfg(test)]
#[path = "provider_setup/tests.rs"]
mod tests;
