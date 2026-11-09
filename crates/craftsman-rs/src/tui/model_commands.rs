use super::app::App;
use super::modals::{AppAction, Modal, SelectionItem, SelectionModal};
use super::session_bridge::AgentSlot;
use crate::config::Config;
use crate::hitl::ApprovalState;
use crate::model_controls::{CatalogModel, ModelSelection, ReasoningEffort};
use std::sync::{Arc, Mutex};

pub(super) fn selected_config(cfg: &Config, selection: ModelSelection) -> Result<Config, String> {
    let mut candidate = cfg.clone();
    if let Some(model) = selection.model {
        candidate.model = Some(model);
    }
    let model = candidate
        .model
        .clone()
        .ok_or("No model selected. Use /model MODEL first.")?;
    crate::model_controls::validate_model_id(&model)?;
    if let Some(effort) = selection.effort {
        crate::model_controls::set_effort(&mut candidate, &model, effort)?;
    }
    // Refuse malformed controls before saving the user's configuration.
    crate::model_controls::selected_effort(&candidate, &model)?;
    Ok(candidate)
}

pub(super) fn select_model(
    cfg: &mut Config,
    selection: ModelSelection,
    slot: &mut AgentSlot,
    approval: &Arc<Mutex<ApprovalState>>,
    app: &mut App,
) {
    let candidate = match selected_config(cfg, selection) {
        Ok(cfg) => cfg,
        Err(error) => {
            app.push_error(error);
            return;
        }
    };
    if !slot.cancellation().is_cancelled()
        && candidate.model == cfg.model
        && candidate.extra.get("model_settings") == cfg.extra.get("model_settings")
    {
        app.push("The current model and reasoning settings are already selected.");
        return;
    }
    if let Err(error) = candidate.save() {
        app.push_error(format!("Could not save model selection: {error}"));
        return;
    }
    *cfg = candidate;
    let effort = crate::model_controls::display_effort(cfg);
    app.push(format!(
        "Model: {}{}",
        cfg.model.as_deref().unwrap_or_default(),
        effort.map_or(String::new(), |e| format!(" · reasoning:{e}"))
    ));
    slot.rebuild(cfg, approval.clone());
}

pub(super) fn show_model_catalog(app: &mut App, models: Vec<CatalogModel>, current: Option<&str>) {
    if models.is_empty() {
        app.push("No models reported. Use /model MODEL or configure model_catalog.");
        return;
    }
    let mut selected = 0;
    let mut items = Vec::new();
    for model in models {
        if current == Some(model.id.as_str()) {
            selected = items.len();
        }
        let label = if model.label.is_empty() {
            model.id.clone()
        } else {
            model.label.clone()
        };
        items.push(SelectionItem {
            label: label.clone(),
            detail: format!(
                "{}{}",
                model.id,
                if current == Some(model.id.as_str()) {
                    " · current"
                } else {
                    ""
                }
            ),
            enter_action: Some(AppAction::SelectModel(model.id.clone())),
            delete_action: None,
        });
        for effort in model.reasoning_efforts {
            items.push(SelectionItem {
                label: format!("{label} · {}", effort.as_str()),
                detail: format!("{} · reasoning effort", model.id),
                enter_action: Some(AppAction::SelectModelEffort {
                    model: model.id.clone(),
                    effort: Some(effort),
                }),
                delete_action: None,
            });
        }
    }
    app.modal = Some(Modal::Selection(SelectionModal {
        title: "Select model and reasoning effort".into(),
        items,
        selected,
        hint: "↑/↓ select · Enter apply · Esc close · /model MODEL --reasoning=high".into(),
    }));
    app.dirty = true;
}

pub(super) fn show_effort_picker(cfg: &Config, app: &mut App) {
    let Some(model) = &cfg.model else {
        app.push_error("No model selected. Use /model MODEL first.");
        return;
    };
    let models = match crate::model_controls::configured_catalog(cfg) {
        Ok(models) => models,
        Err(error) => {
            app.push_error(error);
            return;
        }
    };
    let declared = models
        .iter()
        .find(|entry| &entry.id == model)
        .map(|entry| entry.reasoning_efforts.clone())
        .unwrap_or_default();
    let known = !declared.is_empty();
    let efforts = if known {
        declared
    } else {
        ReasoningEffort::ALL.to_vec()
    };
    let current = crate::model_controls::display_effort(cfg).unwrap_or_else(|| "default".into());
    let mut items = vec![SelectionItem {
        label: "Provider default".into(),
        detail: "Omit the reasoning_effort parameter".into(),
        enter_action: Some(AppAction::SelectModelEffort {
            model: model.clone(),
            effort: None,
        }),
        delete_action: None,
    }];
    let mut selected = 0;
    for effort in efforts {
        if current == effort.as_str() {
            selected = items.len();
        }
        items.push(SelectionItem {
            label: effort.as_str().into(),
            detail: if known {
                "configured model option".into()
            } else {
                "provider/model must support this value".into()
            },
            enter_action: Some(AppAction::SelectModelEffort {
                model: model.clone(),
                effort: Some(effort),
            }),
            delete_action: None,
        });
    }
    app.modal = Some(Modal::Selection(SelectionModal {
        title: format!("Reasoning effort · {model}"),
        items,
        selected,
        hint: "↑/↓ select · Enter apply · Esc close · default differs from none".into(),
    }));
    app.dirty = true;
}

#[cfg(test)]
#[path = "model_commands/tests.rs"]
mod tests;
