//! The person's own settings changes for a running private conversation,
//! and two commands that need the same bound owners: the manual compaction
//! and the extension commands.
//!
//! Only the owned interactive router lends this to the frontend it selected.
//! Every other private entry holds none and keeps refusing all of it.
//!
//! One settings change goes through these steps, in this order:
//!
//! 1. **Asked.** The ordinary `/model`, `/effort` and `/mode` forms and their
//!    pickers. The ordinary selection decides what the change means: a named
//!    `provider/model` keeps its id, and an effort or a thinking-token
//!    budget is written where that model's own options live. The candidate
//!    bytes of the user file are built and validated as a whole here, before
//!    anything is touched. A refusal is a line; the conversation goes on.
//! 2. **Retired.** The conversation must be idle. Its agent retires and
//!    issues its whole record; the send guard, the vault scope and the key
//!    close. A refusal before the agent's retirement keeps the conversation.
//! 3. **Written and recaptured.** After the frontend retired its owners: the
//!    candidate is committed through the retained source's staged,
//!    identity-bound writer, and the startup of this same invocation is
//!    captured anew. A write that changed nothing does not end the
//!    conversation: it is carried over under the settings it had, and the
//!    refusal is shown.
//! 4. **Unlocked and built** by the frontend, as for a fresh clear, for the
//!    successor binding the retirement named. A password that the vault
//!    refuses does not end the conversation: the record stays here, the
//!    startup is captured again without another write, and the password is
//!    asked again, up to the three attempts one key owner allows. A
//!    cancelled unlock or a third refusal ends the terminal with what
//!    became of the change and where the record is.
//! 5. **Adopted.** The new session adopts the record. A failure here ends
//!    the terminal with a fixed error; the record up to the retirement is in
//!    the thread store.
//!
//! No model, tool or hook is replayed, and nothing of the old conversation
//! is sent anywhere. Refreshing a catalog over the network, provider setup,
//! harness and agent selection keep their refusals. So does a permission
//! mode that decides file writes without asking.
//!
//! `/compact` is the ordinary idle compaction on the conversation's own
//! agent, through its tracked path, started here and run by the frontend as
//! its owned local work. An extension command of the captured user root or
//! workspace gives its expansion back to the frontend as model input only.

use super::{
    OwnedSecretTerminal, PrivateTerminalError, SettingsAsked, SettingsAuthority, SettingsWork,
    live::Returned,
};
use crate::config::{Config, PermissionMode};
use crate::hitl::ApprovalState;
use crate::model_controls::ModelSelection;
use crate::privacy_bootstrap::{
    BootstrapError, CarriedConversation, PrivateBootstrap, PrivateBuiltSession,
};
use crate::tui::{app::App, modals::AppAction};
use std::sync::{Arc, Mutex};

/// One validated change that waits to be carried out.
struct Prepared {
    /// The user file as it was read for this change.
    original: Vec<u8>,
    /// The whole file with only this change.
    candidate: Vec<u8>,
    /// The ordinary terminal's line for the change.
    line: String,
    /// Whether the change is the permission mode.
    mode: bool,
}

#[derive(Default)]
pub(super) struct RouterSettings {
    /// A manual compaction that the last line started and the frontend has
    /// not taken yet.
    compaction: Option<std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>>,
    /// The pending handoff: the validated change, and from the retirement
    /// until the adoption the retired conversation itself. Both stay here
    /// through a refused password.
    prepared: Option<Prepared>,
    carried: Option<CarriedConversation>,
    /// Whether the change was handed to the writer. That happens once,
    /// however often the startup is captured again.
    committed: bool,
    /// Why the write did not complete, when the conversation is carried
    /// over all the same.
    unwritten: Option<String>,
    /// Passwords the vault refused since the retirement.
    refused: u8,
}

/// What a thinking-token budget for `model`, or for the current model, would
/// save: the candidate of the ordinary terminal's budget selection
/// (`model_commands::select_budget`), made by the same setter. Read-only.
fn budget_config(cfg: &Config, model: Option<&str>, budget: u32) -> Result<Config, String> {
    let mut candidate = cfg.clone();
    if let Some(model) = model {
        candidate.model = Some(model.into());
    }
    let Some(model) = candidate.model.clone() else {
        return Err("No model selected.".into());
    };
    crate::provider_config::set_budget(&mut candidate, &model, budget)?;
    Ok(candidate)
}

enum Change {
    /// What the ordinary terminal's model, effort or budget selection would
    /// save, or its refusal.
    Model(Result<Config, String>),
    Mode(PermissionMode),
}

impl RouterSettings {
    /// Builds and validates the candidate for `change`. Read-only.
    fn prepare(
        &mut self,
        change: Change,
        cfg: &Config,
        built: Option<&PrivateBuiltSession>,
    ) -> Result<(), String> {
        let Some(built) = built else {
            return Err("No private conversation is built yet; nothing was changed.".into());
        };
        built.carriable()?;
        let name = |key: &str| key.to_owned();
        let mut edits = Vec::new();
        let mode = matches!(change, Change::Mode(_));
        let (expected, line) = match change {
            Change::Model(candidate) => {
                let candidate = candidate?;
                let model = candidate.model.clone().unwrap_or_default();
                // The successor is built on the wire this selection names. A
                // named provider's native protocol sends back items that
                // cannot be inspected (`LlmClient::configured`). Where the
                // policy requires that they can, the build refuses that
                // wire, and it would do so only after the retirement. It is
                // refused here instead, with the build's own reason.
                if !candidate.providers.is_empty()
                    && built.requires_inspectable_items()
                    && crate::provider_config::selection(&candidate, &model)?
                        .provider
                        .protocol
                        != crate::provider_config::Protocol::OpenaiChatCompletions
                {
                    return Err(format!(
                        "{} Nothing was changed.",
                        crate::privacy_session::BuildRefusal::OpaqueProviderItems
                    ));
                }
                if candidate.model != cfg.model {
                    edits.push((vec![name("model")], serde_json::Value::from(model.as_str())));
                }
                // A named model's effort or budget lives with that provider's
                // model.
                if candidate.providers != cfg.providers {
                    let chosen = crate::provider_config::selection(&candidate, &model)?;
                    let reasoning = chosen
                        .entry
                        .and_then(|entry| entry.settings.get("reasoning"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    edits.push((
                        vec![
                            name("providers"),
                            name(chosen.provider_id),
                            name("models"),
                            name(chosen.model_id),
                            name("settings"),
                            name("reasoning"),
                        ],
                        reasoning,
                    ));
                }
                // Without named providers it lives in the per-model settings.
                let per_model = |cfg: &Config| cfg.extra.get("model_settings").cloned();
                if per_model(&candidate) != per_model(cfg) {
                    let effort = per_model(&candidate)
                        .and_then(|settings| settings.get(&model).cloned())
                        .and_then(|settings| settings.get("reasoning_effort").cloned())
                        .unwrap_or(serde_json::Value::Null);
                    edits.push((
                        vec![
                            name("model_settings"),
                            model.clone(),
                            name("reasoning_effort"),
                        ],
                        effort,
                    ));
                }
                if edits.is_empty() {
                    return Err(
                        "The current model and reasoning settings are already selected.".into(),
                    );
                }
                let effort = crate::model_controls::display_effort(&candidate)
                    .map_or(String::new(), |effort| format!(" · reasoning:{effort}"));
                let line = format!("Model: {model}{effort}");
                (candidate, line)
            }
            Change::Mode(mode) => {
                // A mode that decides file writes itself would write restored
                // private values that nobody was shown.
                if !crate::privacy_startup::mode_asks_before_writes(mode) {
                    return Err(crate::privacy_startup::StartupError::ModeDecidesWrites.to_string());
                }
                // An option of this start decides the mode before the file
                // does. Writing the file would change nothing here.
                let fixed = match built.startup_options() {
                    Some(options) => {
                        options.permission_mode_override().is_some()
                            || options.auto_approve_override()
                    }
                    None => true,
                };
                if fixed {
                    return Err("This start's own option fixes the permission mode; the setting was not changed.".into());
                }
                if cfg.permission_mode == mode && !cfg.auto_approve {
                    return Err("That permission mode is already selected.".into());
                }
                let mut candidate = cfg.clone();
                candidate.permission_mode = mode;
                candidate.auto_approve = false;
                let Ok(value) = serde_json::to_value(mode) else {
                    return Err(
                        "That permission mode has no stored form; nothing was changed.".into(),
                    );
                };
                edits.push((vec![name("permission_mode")], value));
                if cfg.auto_approve {
                    edits.push((vec![name("auto_approve")], false.into()));
                }
                let line = format!(
                    "Permission mode: {} — {}",
                    mode.as_str(),
                    mode.description()
                );
                (candidate, line)
            }
        };
        let original = match crate::privacy_startup::enabled_original() {
            Ok(original) => original,
            Err(error) => return Err(error.to_string()),
        };
        let candidate = match crate::privacy_startup::edited(&original, &edits) {
            Ok(candidate) => candidate,
            Err(error) => return Err(error.to_string()),
        };
        // What the bytes mean is exactly what the selection means.
        let written = match serde_json::from_slice::<Config>(&candidate) {
            Ok(written) => {
                written.model == expected.model
                    && written.providers == expected.providers
                    && written.extra.get("model_settings") == expected.extra.get("model_settings")
                    && written.permission_mode == expected.permission_mode
                    && written.auto_approve == expected.auto_approve
            }
            Err(_) => false,
        };
        if !written {
            return Err(
                "The user file cannot hold this selection as it is meant; nothing was changed."
                    .into(),
            );
        }
        self.prepared = Some(Prepared {
            original,
            candidate,
            line,
            mode,
        });
        Ok(())
    }
}

impl SettingsAuthority for RouterSettings {
    fn asked(
        &mut self,
        asked: SettingsAsked<'_>,
        cfg: &Config,
        built: Option<&PrivateBuiltSession>,
        app: &mut App,
        prompt: &mut Option<String>,
    ) -> bool {
        self.compaction = None;
        use crate::tui::model_commands::selected_config;
        let change = match asked {
            SettingsAsked::Action(AppAction::ChooseModel(model)) => {
                crate::tui::model_commands::show_model_effort_picker(cfg, app, model.clone());
                return true;
            }
            SettingsAsked::Action(AppAction::SelectModelEffort { model, effort }) => {
                Change::Model(selected_config(
                    cfg,
                    ModelSelection {
                        model: Some(model.clone()),
                        effort: Some(*effort),
                    },
                ))
            }
            SettingsAsked::Action(AppAction::SelectModelBudget { model, budget }) => {
                Change::Model(budget_config(cfg, Some(model), *budget))
            }
            SettingsAsked::Action(AppAction::SelectMode(mode)) => Change::Mode(*mode),
            SettingsAsked::Action(_) => return false,
            SettingsAsked::Line(line) => {
                let mut parts = line.split_whitespace();
                match (parts.next(), parts.next(), parts.clone().next()) {
                    (Some("/model"), None, _) => {
                        match crate::model_controls::configured_catalog(cfg) {
                            Ok(models) => crate::tui::model_commands::show_model_catalog(
                                app,
                                models,
                                cfg.model.as_deref(),
                                !cfg.providers.is_empty(),
                            ),
                            Err(error) => app.push_error(error),
                        }
                        return true;
                    }
                    // Discovery asks the configured connections over the
                    // network; it keeps its refusal.
                    (Some("/model"), Some("--refresh"), None) => return false,
                    (Some("/model"), Some(_), _) => {
                        let tokens = line.split_whitespace().skip(1);
                        match crate::model_controls::parse_selection(tokens) {
                            Ok(selection) => Change::Model(selected_config(cfg, selection)),
                            Err(error) => {
                                app.push_error(error);
                                return true;
                            }
                        }
                    }
                    (Some("/effort"), None, _) => {
                        crate::tui::model_commands::show_effort_picker(cfg, app);
                        return true;
                    }
                    // Digits are a thinking-token budget for the current
                    // model of a named provider, as in the ordinary terminal.
                    (Some("/effort"), Some(value), None)
                        if !cfg.providers.is_empty()
                            && value.bytes().all(|byte| byte.is_ascii_digit()) =>
                    {
                        match value.parse::<u32>() {
                            Ok(budget) => Change::Model(budget_config(cfg, None, budget)),
                            Err(_) => {
                                app.push_error("Thinking token budget exceeds its limit.");
                                return true;
                            }
                        }
                    }
                    (Some("/effort"), Some(value), None) => {
                        let effort = match value {
                            "default" => Ok(None),
                            value => crate::model_controls::ReasoningEffort::parse(value).map(Some),
                        };
                        match effort {
                            Ok(effort) => Change::Model(selected_config(
                                cfg,
                                ModelSelection {
                                    model: None,
                                    effort: Some(effort),
                                },
                            )),
                            Err(error) => {
                                app.push_error(error);
                                return true;
                            }
                        }
                    }
                    (Some("/mode"), None, _) => {
                        crate::tui::modals::show_mode_modal(cfg, app);
                        return true;
                    }
                    (Some("/mode"), Some(value), None) => match PermissionMode::parse(value) {
                        Some(mode) => Change::Mode(mode),
                        None => {
                            app.push_error(
                                "Unknown mode. Use plan, accept-edits, manual, bypass-permissions, or auto.",
                            );
                            return true;
                        }
                    },
                    // The ordinary manual compaction, on this conversation's
                    // own agent through its bound owner.
                    (Some("/compact"), None, _) => {
                        let started = match (built, app.running) {
                            (_, true) => Err(
                                "A compaction waits for the running work; it was not started."
                                    .to_owned(),
                            ),
                            (None, _) => Err("No private conversation is built yet.".to_owned()),
                            (Some(built), false) => built.compact(),
                        };
                        match started {
                            Ok(work) => self.compaction = Some(work),
                            Err(line) => app.push_error(line),
                        }
                        return true;
                    }
                    // An extension command of the captured configuration
                    // root or workspace: its expansion is model input only.
                    (Some(_), _, _) => {
                        let expansion = built.and_then(|built| {
                            let (config, workspace) = built.extension_roots()?;
                            let extensions = crate::extensions::discover_roots(
                                &config,
                                workspace,
                                crate::extensions::terminal_gadgets_enabled(cfg),
                            );
                            crate::extensions::expand_command(&extensions, line)
                        });
                        return match expansion {
                            Some(expansion) => {
                                *prompt = Some(expansion);
                                true
                            }
                            None => false,
                        };
                    }
                    _ => return false,
                }
            }
        };
        if let Err(line) = self.prepare(change, cfg, built) {
            app.push_error(line);
        }
        true
    }

    fn pending(&self) -> bool {
        self.prepared.is_some() && self.carried.is_none()
    }

    fn work(
        &mut self,
        session: &craftsman_harness::SessionCancellation,
        running: &mut Option<craftsman_harness::TurnCancellation>,
    ) -> Option<SettingsWork<'static>> {
        let work = self.compaction.take()?;
        let turn = session.new_turn();
        *running = Some(turn.clone());
        Some(Box::pin(async move {
            Returned::Operation(turn.scope(work).await)
        }))
    }

    fn retire<'a>(
        &'a mut self,
        built: Option<&'a PrivateBuiltSession>,
        busy: bool,
    ) -> SettingsWork<'a> {
        Box::pin(async move {
            let retired = match (built, busy) {
                (_, true) => {
                    Err("A settings change waits for the running work; it was not made.".to_owned())
                }
                (None, _) => {
                    Err("No private conversation is built yet; nothing was changed.".to_owned())
                }
                (Some(built), false) => built.retire_for_successor().await,
            };
            match retired {
                Ok(carried) => {
                    self.carried = Some(carried);
                    (self.committed, self.unwritten, self.refused) = (false, None, 0);
                    Returned::Listing(Ok(String::new()))
                }
                Err(line) => {
                    self.prepared = None;
                    Returned::Listing(Err(line))
                }
            }
        })
    }

    fn recapture(
        &mut self,
        live: &mut Arc<Mutex<ApprovalState>>,
    ) -> Result<PrivateBootstrap, PrivateTerminalError> {
        let (Some(prepared), Some(carried)) = (&self.prepared, &self.carried) else {
            return Err(PrivateTerminalError::InputHandoff);
        };
        // The person's own change, handed to the writer once. A refusal is
        // kept and shown after the conversation was carried over under
        // whatever the file now holds. A capture that is asked for again,
        // after a refused password, writes nothing.
        if !std::mem::replace(&mut self.committed, true) {
            self.unwritten =
                match crate::privacy_startup::commit(&prepared.original, &prepared.candidate) {
                    Ok(()) => None,
                    Err(error) => Some(format!("The settings change did not complete. {error}")),
                };
        }
        let (bootstrap, start) = crate::privacy_startup::recapture()?;
        let bootstrap = match bootstrap.carrying(carried) {
            Ok(bootstrap) => bootstrap,
            Err(error) => return Err(PrivateTerminalError::Source(error)),
        };
        let approval = OwnedSecretTerminal::captured_approval(&bootstrap, &start)?;
        // What the person granted during the conversation stays granted. A
        // new permission mode is taken as the ordinary terminal takes it:
        // without the earlier "always".
        let earlier = match live.lock() {
            Ok(state) => state.clone(),
            Err(_) => return Err(PrivateTerminalError::ApprovalSource),
        };
        let Ok(mut state) = approval.lock() else {
            return Err(PrivateTerminalError::ApprovalSource);
        };
        for command in earlier.shell_allow_list {
            if !state.shell_allow_list.contains(&command) {
                state.shell_allow_list.push(command);
            }
        }
        state.auto_approve_network |= earlier.auto_approve_network;
        if !prepared.mode || self.unwritten.is_some() {
            state.auto_approve |= earlier.auto_approve;
        }
        drop(state);
        *live = approval;
        Ok(bootstrap)
    }

    fn locked(&mut self, error: BootstrapError) -> Result<u8, PrivateTerminalError> {
        use crate::privacy_key::KeyError;
        // A password the vault refused, or an entry that cannot be one.
        let refused = matches!(
            error,
            BootstrapError::Key(KeyError::Unlock | KeyError::Passphrase)
        );
        if refused {
            // As many passwords as one key owner takes.
            self.refused = self.refused.saturating_add(1);
            if self.refused < 3 && self.carried.is_some() {
                return Ok(3 - self.refused);
            }
        }
        let why = match error {
            _ if refused => "The vault did not open with any of three passwords.".to_owned(),
            BootstrapError::Key(KeyError::Cancelled) => "The unlock was cancelled.".to_owned(),
            error => error.to_string(),
        };
        // What became of the change itself belongs to the last word: a
        // write that was made is never reported as absent.
        let change = match (self.committed, &self.unwritten) {
            (false, _) => "The settings change was not handed to the writer.",
            (true, None) => "The settings change itself was written to the user file.",
            (true, Some(unwritten)) => unwritten,
        };
        Err(PrivateTerminalError::CarryOver(format!("{why} {change}")))
    }

    fn adopt<'a>(&'a mut self, built: Option<&'a PrivateBuiltSession>) -> SettingsWork<'a> {
        Box::pin(async move {
            let (Some(built), Some(carried)) = (built, self.carried.take()) else {
                return Returned::Listing(Err("No retired conversation is waiting.".into()));
            };
            let prepared = self.prepared.take();
            (self.committed, self.refused) = (false, 0);
            if let Err(reason) = built.adopt(&carried).await {
                return Returned::Listing(Err(reason));
            }
            Returned::Listing(Ok(match (self.unwritten.take(), prepared) {
                (Some(unwritten), _) => unwritten,
                (None, Some(prepared)) => prepared.line,
                (None, None) => String::new(),
            }))
        })
    }
}
