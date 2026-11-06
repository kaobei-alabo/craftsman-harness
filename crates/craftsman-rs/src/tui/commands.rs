//! Slash commands, submission routing and the actions modals trigger.

use super::app::{App, TranscriptKind};
use super::input::{VoiceRequest, request_voice_cancel, request_voice_start};
use super::modals::{
    AppAction, EditorModal, Modal, SelectionItem, SelectionModal, show_harness_modal,
    show_mode_modal, show_queue_modal, show_setup_modal, show_skills_modal, show_tasks_modal,
    show_voice_model_modal,
};
use super::render::{auth_status, server_status};
use super::session_bridge::{AgentSlot, UiEvent, hydrate_transcript, state_as_markdown};
use crate::agent::{self, Session};
use crate::checkpoints::LibsqlCheckpointer;
use crate::config::Config;
use crate::hitl::ApprovalState;
use crate::route::resolve_chat_endpoint;
use craftsman_harness::Checkpointer;
use std::sync::{Arc, Mutex};

const HELP: &str = "Commands\n  /setup      guided platform or direct-provider setup\n  /help       show this help\n  /model      list models; /model <name> selects\n  /mode       choose permissions; Shift-Tab cycles modes\n  /status     show route, model, mode, token, and task status\n  /doctor     diagnose local configuration and connectivity state\n  /agents     list agents; /agents <name> selects/creates\n  /skills     list installed/platform skills\n  /gadgets    list local gadgets, capabilities, and approval status\n  /mcp        list MCP servers and tools\n  /lsp        show/restart language servers; /lsp restart <name>\n  /config     show current config\n  /harness    choose or inspect the harness profile\n  /clear      start a fresh conversation\n  /compact    summarize older messages\n  /session    list/resume sessions; supports rename/delete/export\n  /queue      inspect or remove queued follow-ups\n  /tokens     show token usage\n  /tasks      open the subagent task drawer\n  /memories   list long-term memory files\n  /diff       show staged, unstaged, and untracked workspace changes\n  /checkpoints list per-turn workspace file checkpoints\n  /rewind     restore files; /rewind [checkpoint-id] [--force]\n  /copy       copy the latest Craftsman answer\n  /editor     open a large multiline prompt editor\n  /version    print version\n  /voice      dictate by voice (Ctrl+G)\n  /quit       exit (also /exit)\nInline: !cmd runs a shell command. Ctrl-C stops a running turn.
Navigation: ↑/↓ prompt/history · trackpad/mouse or PgUp/PgDn chat · click tool/task rows or Ctrl-O to expand.";

pub(super) const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/setup", "guided connection and model setup"),
    ("/help", "show help"),
    ("/model", "list or select a model"),
    ("/mode", "choose a permission mode"),
    ("/status", "show runtime status"),
    ("/doctor", "diagnose configuration"),
    ("/agents", "list or select an agent"),
    ("/skills", "list skills"),
    ("/mcp", "list MCP servers"),
    ("/gadgets", "list local gadgets and capabilities"),
    ("/lsp", "show or restart language servers"),
    ("/config", "show configuration"),
    ("/harness", "choose or inspect harness profile"),
    ("/clear", "fresh conversation"),
    ("/compact", "compact transcript"),
    (
        "/session",
        "list, resume, rename, delete, or export threads",
    ),
    ("/queue", "inspect queued follow-ups"),
    ("/tokens", "show token usage"),
    ("/tasks", "list background agents"),
    ("/memories", "list memories"),
    ("/diff", "show workspace changes"),
    ("/checkpoints", "list workspace file checkpoints"),
    ("/rewind", "restore files from a checkpoint"),
    ("/copy", "copy the latest answer"),
    ("/editor", "open the multiline prompt editor"),
    ("/version", "print version"),
    ("/voice", "dictate by voice (Ctrl+G)"),
    ("/quit", "exit"),
    ("/exit", "exit"),
];

fn help_text(app: &App) -> String {
    if app.voice.available {
        HELP.to_string()
    } else {
        HELP.lines()
            .filter(|line| !line.contains("/voice"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Where a submitted line goes once it reaches the idle path.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SubmissionRoute {
    /// `!cmd`: an inline shell command the user typed themselves.
    Shell(String),
    /// A built-in `/command`.
    Slash,
    /// Text for the model.
    Prompt,
}

/// Decide where a line goes. `extension_prompt` is the expansion of a gadget or legacy plugin
/// slash command when the line was one. That text is model input only: it can
/// start with `!` or `/`, but only the user's own typing may run a shell
/// command or a built-in, so an expansion always goes to the model.
pub(super) fn submission_route(line: &str, extension_prompt: Option<&str>) -> SubmissionRoute {
    if extension_prompt.is_some() {
        return SubmissionRoute::Prompt;
    }
    if let Some(command) = line.strip_prefix('!') {
        return SubmissionRoute::Shell(command.to_string());
    }
    if line.starts_with('/') {
        return SubmissionRoute::Slash;
    }
    SubmissionRoute::Prompt
}

pub(super) fn live_command(line: &str) -> bool {
    matches!(
        line.split_whitespace().next().unwrap_or_default(),
        "/help"
            | "/config"
            | "/status"
            | "/doctor"
            | "/mode"
            | "/tokens"
            | "/tasks"
            | "/queue"
            | "/memories"
            | "/diff"
            | "/checkpoints"
            | "/copy"
            | "/editor"
            | "/version"
            | "/voice"
    )
}

fn checkpoint_age(created: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(created);
    let seconds = now.saturating_sub(created);
    match seconds {
        0..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn set_permission_mode(
    value: &str,
    cfg: &mut Config,
    approval: &Arc<Mutex<ApprovalState>>,
    app: &mut App,
) {
    let Some(mode) = crate::config::PermissionMode::parse(value) else {
        app.push_error(
            "Unknown mode. Use plan, accept-edits, manual, bypass-permissions, or auto.",
        );
        return;
    };
    cfg.permission_mode = mode;
    cfg.auto_approve = false;
    {
        let mut state = approval.lock().unwrap();
        state.mode = mode;
        state.auto_approve = false;
        state.auto_approve_network = cfg.auto_approve_web();
    }
    match cfg.save() {
        Ok(()) => app.push(format!(
            "Permission mode: {} — {}",
            mode.as_str(),
            mode.description()
        )),
        Err(error) => app.push_error(format!(
            "Mode changed for this run, but could not save it: {error}"
        )),
    }
}

fn status_text(
    cfg: &Config,
    session: Option<&Session>,
    approval: &Arc<Mutex<ApprovalState>>,
    app: &App,
) -> String {
    let mode = approval.lock().unwrap().mode;
    let (input, output) = session
        .map(|session| *session.usage.lock().unwrap())
        .unwrap_or_default();
    let tasks = session
        .map(|session| {
            session
                .registry
                .list()
                .into_iter()
                .filter(|(_, status)| *status == craftsman_harness::AgentStatus::Running)
                .count()
        })
        .unwrap_or(0);
    format!(
        "model: {}\nagent: {}\nmode: {} — {}\nroute: {}\nserver: {}\nauth: {}\ntokens: input {input}, output {output}\nqueued: {}\nrunning tasks: {tasks}/{}",
        cfg.model.as_deref().unwrap_or("(none)"),
        cfg.agent,
        mode.as_str(),
        mode.description(),
        session.map_or("starting", |session| session.route_source.as_str()),
        server_status(cfg),
        auth_status(cfg),
        app.queued_messages.len(),
        cfg.effective_max_parallel_agents(),
    )
}

fn doctor_text(cfg: &Config, session: Option<&Session>) -> String {
    let mut rows = vec![format!(
        "✓ config: {}",
        crate::config::config_path().display()
    )];
    rows.push(if cfg.agent.trim().is_empty() {
        "✗ agent: empty".to_string()
    } else {
        format!("✓ agent: {}", cfg.agent)
    });
    rows.push(match cfg.model.as_deref() {
        Some(model) => format!("✓ model: {model}"),
        None => "✗ model: not selected (run /setup)".to_string(),
    });
    rows.push(
        if cfg.platform.base_url.is_empty() && cfg.base_url.is_empty() {
            "✗ route: no endpoint configured".to_string()
        } else if let Some(session) = session {
            format!("✓ route: {}", session.route_source)
        } else {
            "… route: starting".to_string()
        },
    );
    if !cfg.platform.base_url.is_empty() {
        rows.push(if !cfg.platform_configured() {
            "✗ platform: no API token configured".to_string()
        } else {
            format!("✓ platform: {} (api-token)", cfg.platform.base_url)
        });
    }
    if !cfg.base_url.is_empty() {
        rows.push(format!("✓ direct endpoint: {}", cfg.base_url));
        rows.push(if cfg.api_key.is_empty() {
            "! direct API key: empty".to_string()
        } else {
            "✓ direct API key: configured".to_string()
        });
    }
    if let Some(session) = session {
        rows.push(if session.checkpointer.is_some() {
            "✓ session persistence".to_string()
        } else {
            "! session persistence unavailable".to_string()
        });
        rows.push(format!(
            "✓ MCP: {} connected, {} unavailable",
            session.mcp.len(),
            session.mcp_errors.len()
        ));
        rows.push(format!(
            "✓ LSP: {} configured server(s); run /lsp for availability",
            cfg.effective_lsp_servers().len()
        ));
    }
    rows.join("\n")
}

pub(super) async fn handle_slash(
    line: &str,
    cfg: &mut Config,
    slot: &mut AgentSlot,
    approval: &Arc<Mutex<ApprovalState>>,
    app: &mut App,
) -> bool {
    let mut parts = line.split_whitespace();
    let command = parts.next().unwrap_or_default();
    match command {
        "/setup" => show_setup_modal(app),
        "/help" => {
            let mut text = help_text(app);
            let extension_commands = crate::extensions::help_section_in_cwd(cfg);
            if !extension_commands.is_empty() {
                text.push('\n');
                text.push_str(&extension_commands);
            }
            app.push(text);
        }
        "/mode" => match parts.next() {
            None => show_mode_modal(cfg, app),
            Some(value) => set_permission_mode(value, cfg, approval, app),
        },
        "/status" => app.push(status_text(cfg, slot.peek(), approval, app)),
        "/doctor" => app.push(doctor_text(cfg, slot.peek())),
        "/queue" => show_queue_modal(app),
        "/diff" => {
            app.push_kind(TranscriptKind::Activity, "Generating workspace diff…");
            let tx = slot.bridge.tx.clone();
            tokio::spawn(async move {
                let text = tokio::task::spawn_blocking(crate::workspace_diff::workspace_diff)
                    .await
                    .unwrap_or_else(|error| format!("Unable to show workspace diff: {error}"));
                let _ = tx.send(UiEvent::Activity(text));
            });
        }
        "/checkpoints" => {
            let Some(workspace) = slot
                .get()
                .await
                .as_ref()
                .and_then(|session| session.workspace_checkpoints.clone())
            else {
                app.push("Workspace checkpointing is unavailable.");
                return false;
            };
            match workspace.list().await {
                Ok(checkpoints) if checkpoints.is_empty() => app.push("(no workspace checkpoints)"),
                Ok(checkpoints) => app.push(
                    checkpoints
                        .into_iter()
                        .map(|checkpoint| {
                            format!(
                                "{}  turn {}  {}  {} file(s){}
  {}",
                                checkpoint.id,
                                checkpoint.turn_seq,
                                checkpoint_age(checkpoint.created),
                                checkpoint.file_count,
                                if checkpoint.partial {
                                    " · partial"
                                } else {
                                    ""
                                },
                                checkpoint.summary
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(
                            "
",
                        ),
                ),
                Err(error) => app.push_error(format!("Could not list checkpoints: {error}")),
            }
        }
        "/rewind" => {
            let args = parts.collect::<Vec<_>>();
            let force = args.contains(&"--force");
            let unknown = args
                .iter()
                .copied()
                .filter(|arg| arg.starts_with('-') && *arg != "--force")
                .collect::<Vec<_>>();
            if !unknown.is_empty() {
                app.push_error("usage: /rewind [checkpoint-id] [--force]");
                return false;
            }
            let ids = args
                .iter()
                .copied()
                .filter(|arg| !arg.starts_with('-'))
                .collect::<Vec<_>>();
            if ids.len() > 1 {
                app.push_error("usage: /rewind [checkpoint-id] [--force]");
                return false;
            }
            let Some(workspace) = slot
                .get()
                .await
                .as_ref()
                .and_then(|session| session.workspace_checkpoints.clone())
            else {
                app.push("Workspace checkpointing is unavailable.");
                return false;
            };
            match workspace.rewind(ids.first().copied(), force).await {
                Ok(report) => {
                    let mut lines = vec![format!(
                        "Rewound workspace checkpoint {}",
                        report.checkpoint_id
                    )];
                    if !report.restored.is_empty() {
                        lines.push(format!(
                            "Restored:
  {}",
                            report.restored.join(
                                "
  "
                            )
                        ));
                    }
                    if !report.conflicts.is_empty() {
                        lines.push(format!(
                            "Skipped conflicts:
  {}
Re-run with --force only if overwriting those newer changes is intended.",
                            report.conflicts.join(
                                "
  "
                            )
                        ));
                    }
                    if !report.unrestorable.is_empty() {
                        lines.push(format!(
                            "Unrestorable (snapshot size/safety limit):
  {}",
                            report.unrestorable.join(
                                "
  "
                            )
                        ));
                    }
                    app.push(lines.join(
                        "

",
                    ));
                }
                Err(error) => app.push_error(format!("Could not rewind workspace: {error}")),
            }
        }
        "/copy" => app.copy_requested = true,
        "/editor" => {
            app.modal = Some(Modal::Editor(EditorModal {
                text: app.input.clone(),
            }));
            app.dirty = true;
        }
        "/config" => app.push(config_text(cfg, slot.peek())),
        "/harness" => match parts.next() {
            None => show_harness_modal(cfg, app),
            Some("status") => match slot.get().await.as_ref() {
                Some(session) => app.push(harness_status(session)),
                None => app.push("no agent yet"),
            },
            Some(selection) => match crate::harness_profiles::select_harness_preset(cfg, selection)
            {
                Ok(preset) => match cfg.save() {
                    Ok(()) => {
                        app.push(format!("Harness profile: {} ({})", preset.label, preset.id));
                        return true;
                    }
                    Err(error) => app.push(format!("could not save harness profile: {error}")),
                },
                Err(error) => app.push(error),
            },
        },
        "/tokens" => match slot.get().await.as_ref() {
            Some(session) => {
                let (input, output) = *session.usage.lock().unwrap();
                app.push(format!("tokens: ↑{input} ↓{output}"));
            }
            None => app.push("no agent yet"),
        },
        "/tasks" => handle_tasks_command(parts.collect(), slot, app).await,
        "/memories" => {
            let session = slot.get().await;
            let Some(store) = session
                .as_ref()
                .and_then(|session| session.memories.as_ref())
            else {
                app.push("(store unavailable)");
                return false;
            };
            use craftsman_harness::Backend;
            match store.ls("/memories/").await {
                Ok(files) if files.is_empty() => app.push("(no memories yet)"),
                Ok(files) => app.push(
                    files
                        .into_iter()
                        .map(|file| format!("  {}", file.path))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                Err(error) => app.push(format!("could not list memories: {error}")),
            }
        }
        "/clear" => match slot.get().await.as_mut() {
            Some(session) => {
                agent::clear_session(session);
                let id = LibsqlCheckpointer::new_thread_id();
                session.set_thread_id(id);
                app.transcript.clear();
                app.tool_blocks.clear();
                app.block_hitboxes.clear();
                app.queued_messages.clear();
                app.push("(fresh conversation)");
            }
            None => app.push("(nothing to clear)"),
        },
        "/compact" => {
            let Some(session) = slot.get().await.as_mut() else {
                app.push("no agent yet");
                return false;
            };
            app.push(compact_session(session.agent.clone(), session.compaction.clone()).await);
        }
        "/agents" => {
            let Some(name) = parts.next() else {
                let mut agents = crate::agents::list_agents();
                if !agents.iter().any(|name| name == &cfg.agent) {
                    agents.push(cfg.agent.clone());
                    agents.sort();
                }
                let selected = agents
                    .iter()
                    .position(|name| name == &cfg.agent)
                    .unwrap_or_default();
                app.modal = Some(Modal::Selection(SelectionModal {
                    title: "Select agent".to_string(),
                    items: agents
                        .into_iter()
                        .map(|name| SelectionItem {
                            detail: if name == cfg.agent {
                                "current".to_string()
                            } else {
                                String::new()
                            },
                            enter_action: Some(AppAction::SelectAgent(name.clone())),
                            delete_action: None,
                            label: name,
                        })
                        .collect(),
                    selected,
                    hint: "↑/↓ select · Enter switch · Esc close · /agents <name> creates"
                        .to_string(),
                }));
                app.dirty = true;
                return false;
            };
            if let Err(error) = crate::agents::validate_agent_name(name) {
                app.push(format!("invalid agent name: {error}"));
                return false;
            }
            match crate::agents::ensure_agent(name) {
                Ok(_) => {
                    cfg.agent = name.to_string();
                    match cfg.save() {
                        Ok(()) => {
                            app.push(format!("Agent: {name}"));
                            return true;
                        }
                        Err(error) => app.push(format!("could not save agent selection: {error}")),
                    }
                }
                Err(error) => app.push(format!("could not create agent {name}: {error}")),
            }
        }
        "/skills" => match load_skill_statuses(cfg).await {
            Ok(statuses) if statuses.is_empty() => app.push("(no skills available)"),
            Ok(statuses) => show_skills_modal(app, statuses),
            Err(error) => app.push(format!("could not load skills: {error}")),
        },
        "/gadgets" => {
            #[cfg(feature = "gadgets")]
            {
                let active = slot
                    .get()
                    .await
                    .as_ref()
                    .and_then(|session| session.gadgets.clone());
                let runtime = match active {
                    Some(runtime) => Ok(runtime),
                    None => crate::gadgets::cli::options(cfg).and_then(|options| {
                        crate::gadgets::GadgetRuntime::new(
                            &std::env::current_dir()?,
                            &crate::config::config_dir(),
                            options,
                        )
                        .map(Arc::new)
                        .map_err(Into::into)
                    }),
                };
                match runtime {
                    Ok(runtime) => match runtime.list().await {
                        Ok(gadgets) if gadgets.is_empty() => app.push("(no gadgets installed)"),
                        Ok(gadgets) => app.push(
                            gadgets
                                .into_iter()
                                .map(|g| {
                                    format!(
                                        "{} {} · {} · {}\n  capabilities: {}{}{}",
                                        g.id,
                                        g.version,
                                        g.status,
                                        g.source.display(),
                                        serde_json::to_string(&g.capabilities).unwrap(),
                                        g.error.map(|e| format!("\n  {e}")).unwrap_or_default(),
                                        g.last_error
                                            .map(|e| format!("\n  last error: {e}"))
                                            .unwrap_or_default(),
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                        ),
                        Err(error) => app.push(format!("could not list gadgets: {error}")),
                    },
                    Err(error) => app.push(format!("gadget runtime unavailable: {error}")),
                }
            }
            #[cfg(not(feature = "gadgets"))]
            app.push("This build has no gadget runtime; rebuild with the gadgets cargo feature.");
        }
        "/mcp" => match slot.get().await.as_ref() {
            Some(session) => {
                let mut lines = session
                    .mcp
                    .iter()
                    .map(|server| {
                        format!(
                            "{} · {} tool(s) · {}",
                            server.name, server.tools, server.url
                        )
                    })
                    .collect::<Vec<_>>();
                lines.extend(
                    session
                        .mcp_errors
                        .iter()
                        .map(|(server, error)| format!("{server} · unavailable: {error}")),
                );
                app.push(if lines.is_empty() {
                    "(no MCP servers)".to_string()
                } else {
                    lines.join("\n")
                });
            }
            None => app.push("no agent yet"),
        },
        "/lsp" => {
            let Some(session) = slot.get().await.as_ref() else {
                app.push("no agent yet");
                return false;
            };
            match parts.next() {
                Some("restart") => {
                    let Some(name) = parts.next() else {
                        app.push_error("usage: /lsp restart <name>");
                        return false;
                    };
                    match session.lsp.restart(name).await {
                        Ok(message) => app.push(message),
                        Err(error) => app.push_error(error),
                    }
                }
                Some(_) => app.push_error("usage: /lsp [restart <name>]"),
                None => {
                    let statuses = session.lsp.statuses().await;
                    app.push(
                        statuses
                            .into_iter()
                            .map(|status| {
                                format!(
                                    "{} · {} · .{} · {}",
                                    status.name,
                                    status.command,
                                    status.extensions.join(" ."),
                                    status.state
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(
                                "
",
                            ),
                    );
                }
            }
        }
        "/session" => handle_session_command(parts.collect(), slot, app).await,
        "/voice" => {
            if !app.voice.available {
                let reason = if std::env::var_os("SSH_CONNECTION").is_some()
                    || std::env::var_os("SSH_TTY").is_some()
                {
                    "Voice input is unavailable over SSH because the remote process has no local microphone."
                } else {
                    "Voice input is unavailable in this build or on this platform."
                };
                app.push(reason);
                return false;
            }
            match parts.next() {
                None => request_voice_start(app),
                Some("status") => {
                    let path = crate::voice::model_path(&app.voice.model)
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "(unavailable)".to_string());
                    let voice_config = crate::voice::VoiceConfig::from_config(cfg);
                    app.push(format!(
                        "voice: {}\nstate: {:?}\nmode: continuous\nengine: whisper\nmodel: {}\nmodel file: {}\ndownloaded: {}\npartials: configured {}, effective {}\nVAD threshold: {:.2}\nsegment silence: {} ms\nidle timeout: {} ms\nmax session: {} min\nauto-submit: {} (minimum {} units)",
                        if app.voice.enabled { "enabled" } else { "disabled" },
                        app.voice.state,
                        app.voice.model,
                        path,
                        crate::voice::model_downloaded(&app.voice.model),
                        voice_config.partial_transcripts,
                        app.voice.partials_enabled.map_or("not benchmarked".to_string(), |enabled| enabled.to_string()),
                        voice_config.vad_threshold,
                        voice_config.vad_silence_ms,
                        voice_config.idle_timeout_ms,
                        voice_config.max_session_minutes,
                        app.voice.auto_submit,
                        app.voice.min_words_to_submit,
                    ));
                }
                Some("on") => {
                    app.voice.enabled = true;
                    app.voice.state = crate::voice::VoiceState::Idle;
                    app.voice_request = Some(VoiceRequest::Recreate);
                    app.push("Voice input enabled for this session.");
                }
                Some("off") => {
                    if app.voice.active() {
                        request_voice_cancel(app);
                    }
                    app.voice.enabled = false;
                    app.push("Voice input disabled for this session.");
                }
                Some("model") => {
                    if app.voice.active() {
                        app.push_error("Stop voice input before changing its model.");
                        return false;
                    }
                    match parts.next() {
                        Some(name) => select_voice_model(name, cfg, app),
                        None => show_voice_model_modal(app),
                    }
                }
                Some(_) => app.push_error("usage: /voice [status|on|off|model <name>]"),
            }
        }
        "/version" => app.push(format!("craftsman {}", env!("CARGO_PKG_VERSION"))),
        "/model" => {
            if let Some(name) = parts.next() {
                cfg.model = Some(name.to_string());
                if let Err(error) = cfg.save() {
                    app.push_error(format!(
                        "Model changed for this run, but could not save it: {error}"
                    ));
                }
                app.push(format!("Model: {name}"));
                return true;
            }
            let route = match resolve_chat_endpoint(cfg).await {
                Ok(route) => route,
                Err(error) => {
                    app.push_error(format!("route resolution failed: {error}"));
                    return false;
                }
            };
            match crate::platform::list_models(cfg, &route).await {
                Ok(models) if !models.is_empty() => {
                    let selected = cfg
                        .model
                        .as_deref()
                        .and_then(|current| models.iter().position(|model| model == current))
                        .unwrap_or_default();
                    app.modal = Some(Modal::Selection(SelectionModal {
                        title: "Select model".to_string(),
                        items: models
                            .into_iter()
                            .map(|model| SelectionItem {
                                detail: if cfg.model.as_deref() == Some(model.as_str()) {
                                    "current".to_string()
                                } else {
                                    String::new()
                                },
                                enter_action: Some(AppAction::SelectModel(model.clone())),
                                delete_action: None,
                                label: model,
                            })
                            .collect(),
                        selected,
                        hint: "↑/↓ select · Enter choose · Esc close".to_string(),
                    }));
                    app.dirty = true;
                }
                Ok(_) => app.push("no models reported; use /model <name>"),
                Err(error) => app.push(format!("could not list models: {error}")),
            }
        }
        other => app.push(format!("unknown command {other} — /help for the list")),
    }
    false
}

pub(super) async fn compact_session(
    agent: craftsman_harness::Agent,
    compaction: Arc<craftsman_harness::builtin::CompactionMiddleware>,
) -> String {
    let messages = agent.state.lock().unwrap().messages.clone();
    match compaction.compact_now(&messages).await {
        Ok(Some(compacted)) => {
            let mut state = agent.state.lock().unwrap();
            if craftsman_harness::TurnCancellation::current()
                .is_some_and(|turn| turn.is_cancelled())
            {
                return "could not compact conversation: cancelled by the user".into();
            }
            if state.messages != messages {
                return "could not compact conversation: conversation changed during compaction"
                    .into();
            }
            let before = messages.len();
            let after = compacted.len();
            state.messages = compacted;
            state.last_usage = None;
            state.last_usage_msg_count = 0;
            format!("compacted conversation: {before} -> {after} messages")
        }
        Ok(None) => "(conversation too short to compact)".into(),
        Err(error) => format!("could not compact conversation: {error}"),
    }
}

async fn handle_tasks_command(args: Vec<&str>, slot: &mut AgentSlot, app: &mut App) {
    let mut parts = args.into_iter();
    let Some(session) = slot.get().await.as_ref() else {
        app.push("(no background tasks)");
        return;
    };
    match parts.next() {
        Some("cancel") => {
            let Some(name) = parts.next() else {
                app.push("usage: /tasks cancel <name>");
                return;
            };
            if session.registry.abort(name) {
                app.push_kind(TranscriptKind::Task, format!("Cancelled task {name}"));
            } else {
                app.push_error(format!(
                    "Task {name} is not a cancellable running background task"
                ));
            }
        }
        Some("message") => {
            let Some(name) = parts.next() else {
                app.push("usage: /tasks message <name> <message>");
                return;
            };
            let message = parts.collect::<Vec<_>>().join(" ");
            if message.is_empty() {
                app.push("usage: /tasks message <name> <message>");
            } else if session.registry.deliver(name, message) {
                app.push_kind(TranscriptKind::Task, format!("Message sent to {name}"));
            } else {
                app.push_error(format!("Could not message task {name}"));
            }
        }
        _ => show_tasks_modal(app, session.registry.details()),
    }
}

async fn handle_session_command(args: Vec<&str>, slot: &mut AgentSlot, app: &mut App) {
    let mut parts = args.into_iter();
    let Some(session) = slot.get().await.as_mut() else {
        app.push("no agent yet");
        return;
    };
    match parts.next() {
        Some("rename") => {
            let Some(id) = parts.next() else {
                app.push("usage: /session rename <id> <title>");
                return;
            };
            let title = parts.collect::<Vec<_>>().join(" ");
            if title.is_empty() {
                app.push("usage: /session rename <id> <title>");
            } else if let Some(checkpointer) = session.checkpointer.as_ref() {
                match checkpointer.rename_thread(id, &title).await {
                    Ok(()) => app.push(format!("Renamed session {id} to {title}")),
                    Err(error) => app.push_error(format!("Could not rename session: {error}")),
                }
            }
        }
        Some("delete") => {
            let Some(id) = parts.next() else {
                app.push("usage: /session delete <id> confirm");
                return;
            };
            if parts.next() != Some("confirm") {
                app.push(format!(
                    "Deletion is permanent. Re-run: /session delete {id} confirm"
                ));
                return;
            }
            if id == session.thread_id {
                app.push_error("Cannot delete the active session.");
            } else if let Some(checkpointer) = session.checkpointer.as_ref() {
                match checkpointer.delete_thread(id).await {
                    Ok(()) => app.push(format!("Deleted session {id}")),
                    Err(error) => app.push_error(format!("Could not delete session: {error}")),
                }
            }
        }
        Some("export") => {
            let id = parts.next().unwrap_or(&session.thread_id).to_string();
            let Some(checkpointer) = session.checkpointer.as_ref() else {
                app.push("Persistence unavailable");
                return;
            };
            match checkpointer.latest_state(&id).await {
                Ok(Some(state)) => {
                    let dir = crate::config::config_dir().join("exports");
                    let safe: String = id
                        .chars()
                        .map(|c| {
                            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    let path = dir.join(format!("{safe}.md"));
                    let result = std::fs::create_dir_all(&dir)
                        .and_then(|()| std::fs::write(&path, state_as_markdown(&state)));
                    match result {
                        Ok(()) => app.push(format!("Exported session to {}", path.display())),
                        Err(error) => app.push_error(format!("Could not export session: {error}")),
                    }
                }
                Ok(None) => app.push_error(format!("No such session {id}")),
                Err(error) => app.push_error(format!("Could not export session: {error}")),
            }
        }
        Some("search") | None => {
            let query = parts.collect::<Vec<_>>().join(" ").to_ascii_lowercase();
            let Some(checkpointer) = session.checkpointer.as_ref() else {
                app.push("Persistence unavailable");
                return;
            };
            match checkpointer.list_threads().await {
                Ok(threads) => {
                    let threads = threads
                        .into_iter()
                        .filter(|thread| {
                            query.is_empty()
                                || thread.title.to_ascii_lowercase().contains(&query)
                                || thread.id.to_ascii_lowercase().contains(&query)
                        })
                        .collect::<Vec<_>>();
                    if threads.is_empty() {
                        app.push("(no matching sessions)");
                        return;
                    }
                    let selected = threads
                        .iter()
                        .position(|thread| thread.id == session.thread_id)
                        .unwrap_or_default();
                    app.modal = Some(Modal::Selection(SelectionModal {
                        title: "Sessions".to_string(),
                        items: threads
                            .into_iter()
                            .map(|thread| {
                                let current = thread.id == session.thread_id;
                                SelectionItem {
                                    label: if thread.title.is_empty() {
                                        "(untitled)".to_string()
                                    } else {
                                        thread.title
                                    },
                                    detail: format!(
                                        "{} · {} checkpoint(s){}",
                                        thread.id,
                                        thread.checkpoints,
                                        if current { " · current" } else { "" }
                                    ),
                                    enter_action: Some(AppAction::ResumeSession(thread.id.clone())),
                                    delete_action: None,
                                }
                            })
                            .collect(),
                        selected,
                        hint: "↑/↓ select · Enter resume · /session rename|delete|export"
                            .to_string(),
                    }));
                    app.dirty = true;
                }
                Err(error) => app.push_error(format!("Could not list sessions: {error}")),
            }
        }
        Some(id) => match session.load_thread(id).await {
            Ok(true) => {
                hydrate_transcript(app, &session.agent.state.lock().unwrap());
                app.push(format!("Resumed session {id}"));
            }
            Ok(false) => app.push_error(format!("No such session {id}")),
            Err(error) => app.push_error(format!("Could not load session {id}: {error}")),
        },
    }
}

pub(super) fn action_status(action: &AppAction) -> Option<String> {
    use crate::skills_browser::BrowserAction;
    match action {
        AppAction::Skill(BrowserAction::Install(id)) => Some(format!("installing skill {id}…")),
        AppAction::Skill(BrowserAction::Update(id)) => Some(format!("updating skill {id}…")),
        AppAction::Skill(BrowserAction::Remove(id)) => Some(format!("removing skill {id}…")),
        AppAction::SetupPlatform { .. } => Some("connecting to the platform…".to_string()),
        AppAction::SetupDirect { model, .. } => Some(format!("configuring direct model {model}…")),
        _ => None,
    }
}

pub(super) async fn handle_app_action(
    action: AppAction,
    cfg: &mut Config,
    slot: &mut AgentSlot,
    approval: &Arc<Mutex<ApprovalState>>,
    app: &mut App,
) {
    match action {
        AppAction::SelectModel(model) => {
            cfg.model = Some(model.clone());
            match cfg.save() {
                Ok(()) => {
                    app.push(format!("Model: {model}"));
                    slot.rebuild(cfg, approval.clone());
                }
                Err(error) => app.push(format!("could not save model selection: {error}")),
            }
        }
        AppAction::SelectVoiceModel(model) => {
            select_voice_model(&model, cfg, app);
        }
        AppAction::SelectHarness(selection) => {
            match crate::harness_profiles::select_harness_preset(cfg, &selection) {
                Ok(preset) => match cfg.save() {
                    Ok(()) => {
                        app.push(format!("Harness profile: {} ({})", preset.label, preset.id));
                        slot.rebuild(cfg, approval.clone());
                    }
                    Err(error) => app.push(format!("could not save harness profile: {error}")),
                },
                Err(error) => app.push(error),
            }
        }
        AppAction::SelectAgent(name) => {
            if let Err(error) = crate::agents::validate_agent_name(&name) {
                app.push(format!("invalid agent name: {error}"));
                return;
            }
            match crate::agents::ensure_agent(&name) {
                Ok(_) => {
                    cfg.agent = name.clone();
                    match cfg.save() {
                        Ok(()) => {
                            app.push(format!("Agent: {name}"));
                            slot.rebuild(cfg, approval.clone());
                        }
                        Err(error) => app.push(format!("could not save agent selection: {error}")),
                    }
                }
                Err(error) => app.push(format!("could not select agent {name}: {error}")),
            }
        }
        AppAction::SelectMode(mode) => set_permission_mode(mode.as_str(), cfg, approval, app),
        AppAction::CancelTask(name) => {
            let Some(session) = slot.get().await.as_ref() else {
                app.push("no agent yet");
                return;
            };
            if session.registry.abort(&name) {
                app.push_kind(TranscriptKind::Task, format!("Cancelled task {name}"));
            } else {
                app.push_error(format!(
                    "Task {name} is not a cancellable running background task"
                ));
            }
        }
        AppAction::RemoveQueued(index) => {
            if let Some(removed) = app.queued_messages.remove(index) {
                app.push(format!(
                    "Removed queued follow-up: {}",
                    display_submission(&removed)
                ));
            }
        }
        AppAction::ResumeSession(id) => {
            let Some(session) = slot.get().await.as_mut() else {
                app.push("no agent yet");
                return;
            };
            if id == session.thread_id {
                app.push(format!("already on thread {id}"));
                return;
            }
            match session.load_thread(&id).await {
                Ok(true) => {
                    hydrate_transcript(app, &session.agent.state.lock().unwrap());
                    app.push(format!("Resumed session {id}"));
                    if let Some(suspension) = session.agent.pending_suspension() {
                        app.push(format!(
                            "thread is awaiting approval: {}",
                            suspension.request.summary
                        ));
                    }
                }
                Ok(false) => app.push(format!("no such thread {id}")),
                Err(error) => app.push(format!("could not load thread {id}: {error}")),
            }
        }
        AppAction::SetupPlatform { url, token } => {
            handle_setup_platform(url, token, cfg, slot, approval, app).await;
        }
        AppAction::SetupDirect {
            base_url,
            api_key,
            model,
        } => {
            handle_setup_direct(base_url, api_key, model, cfg, slot, approval, app).await;
        }
        AppAction::Skill(action) => {
            use crate::skills_browser::BrowserAction;
            let result = match &action {
                BrowserAction::Install(id) => {
                    match crate::platform_skills::SkillsClient::connect(cfg).await {
                        Ok(client) => crate::platform_skills::install_skill(&client, id)
                            .await
                            .map(|marker| format!("installed {} v{}", marker.name, marker.version)),
                        Err(error) => Err(error),
                    }
                }
                BrowserAction::Update(id) => {
                    match crate::platform_skills::SkillsClient::connect(cfg).await {
                        Ok(client) => {
                            crate::platform_skills::update_skill(&client, id)
                                .await
                                .map(|marker| {
                                    format!("updated {} to v{}", marker.name, marker.version)
                                })
                        }
                        Err(error) => Err(error),
                    }
                }
                BrowserAction::Remove(id) => {
                    if crate::platform_skills::remove_skill(id) {
                        Ok(format!("removed {id}"))
                    } else {
                        Err(anyhow::anyhow!(
                            "refused to remove {id}: not platform-installed"
                        ))
                    }
                }
            };
            match result {
                Ok(message) => {
                    app.push(message);
                    slot.rebuild(cfg, approval.clone());
                }
                Err(error) => app.push(format!("skill action failed: {error}")),
            }
            match load_skill_statuses(cfg).await {
                Ok(statuses) if !statuses.is_empty() => show_skills_modal(app, statuses),
                Ok(_) => {}
                Err(error) => app.push(format!("could not refresh skills: {error}")),
            }
        }
    }
}

async fn handle_setup_platform(
    url: String,
    token: String,
    cfg: &mut Config,
    slot: &mut AgentSlot,
    approval: &Arc<Mutex<ApprovalState>>,
    app: &mut App,
) {
    let normalized = match crate::platform_rest::normalize_platform_url(&url) {
        Ok(url) => url,
        Err(error) => {
            app.push_error(format!("Invalid platform URL: {error}"));
            show_setup_modal(app);
            return;
        }
    };
    if token.trim().is_empty() {
        app.push_error("An API token is required; mint one on the platform (auth → API keys)");
        show_setup_modal(app);
        return;
    }
    // Headless token configuration (docs/platform-token-refactor.md): the
    // platform block holds the credential for skills REST, and the LLM route
    // is the derived OpenAI-compatible endpoint.
    cfg.platform.base_url = normalized.clone();
    cfg.platform.api_token = token.trim().to_string();
    cfg.base_url = format!("{}/apps/llm/api/v1", normalized.trim_end_matches('/'));
    cfg.api_key = token.trim().to_string();
    let _ = cfg.save();
    let route = match resolve_chat_endpoint(cfg).await {
        Ok(route) => route,
        Err(error) => {
            app.push_error(format!("Platform configuration failed: {error}"));
            show_setup_modal(app);
            return;
        }
    };
    match crate::platform::list_models(cfg, &route).await {
        Ok(models) if !models.is_empty() => {
            app.push(format!("Platform connected: {normalized}"));
            app.modal = Some(Modal::Selection(SelectionModal {
                title: "Choose your default model".to_string(),
                items: models
                    .into_iter()
                    .map(|model| SelectionItem {
                        label: model.clone(),
                        detail: String::new(),
                        enter_action: Some(AppAction::SelectModel(model)),
                        delete_action: None,
                    })
                    .collect(),
                selected: 0,
                hint: "↑/↓ select · Enter finish setup · Esc close".to_string(),
            }));
        }
        Ok(_) => app.push("Platform configured, but no models were reported. Use /model <name>."),
        Err(error) => app.push_error(format!(
            "Platform configured, but model discovery failed: {error}"
        )),
    }
    slot.rebuild(cfg, approval.clone());
}

async fn handle_setup_direct(
    base_url: String,
    api_key: String,
    model: String,
    cfg: &mut Config,
    slot: &mut AgentSlot,
    approval: &Arc<Mutex<ApprovalState>>,
    app: &mut App,
) {
    cfg.base_url = base_url.trim_end_matches('/').to_string();
    cfg.api_key = api_key;
    cfg.model = Some(model.clone());
    cfg.platform = crate::config::PlatformConfig::default();
    match cfg.save() {
        Ok(()) => {
            let route = match resolve_chat_endpoint(cfg).await {
                Ok(route) => route,
                Err(error) => {
                    app.push_error(format!("Invalid direct configuration: {error}"));
                    return;
                }
            };
            match crate::platform::list_models(cfg, &route).await {
                Ok(models) if models.iter().any(|candidate| candidate == &model) => app.push(
                    format!("Direct endpoint verified; model {model} is available"),
                ),
                Ok(models) if !models.is_empty() => app.push(format!(
                    "Direct endpoint returned {} models; {model} was not listed",
                    models.len()
                )),
                Ok(_) => app.push(format!("Direct provider configured with model {model}")),
                Err(error) => app.push_error(format!(
                    "Configuration saved, but endpoint verification failed: {error}"
                )),
            }
            slot.rebuild(cfg, approval.clone());
        }
        Err(error) => app.push_error(format!("Could not save setup: {error}")),
    }
}

async fn load_skill_statuses(
    cfg: &Config,
) -> anyhow::Result<Vec<crate::platform_skills::SkillStatus>> {
    use crate::platform_skills::{SkillsClient, compute_status, installed_platform_skills};
    let installed = installed_platform_skills();
    let platform = match SkillsClient::connect(cfg).await {
        Ok(client) => client.list().await?,
        Err(error) if installed.is_empty() => return Err(error),
        Err(_) => Vec::new(),
    };
    Ok(compute_status(&installed, &platform))
}

fn select_voice_model(name: &str, cfg: &mut Config, app: &mut App) {
    let Some(name) = crate::voice::canonical_model_name(name) else {
        app.push_error(format!(
            "Unknown voice model. Use {}.",
            crate::voice::model_names().collect::<Vec<_>>().join(", ")
        ));
        return;
    };
    cfg.set_voice_setting("model", serde_json::json!(name));
    app.voice.model = name.to_string();
    app.voice.partials_enabled = None;
    app.voice_request = Some(VoiceRequest::Recreate);
    match cfg.save() {
        Ok(()) => app.push(format!("Voice model: {name} (loads on next activation)")),
        Err(error) => app.push_error(format!(
            "Voice model changed for this run, but could not save it: {error}"
        )),
    }
}

fn harness_status(session: &Session) -> String {
    format!(
        "selected preset: {}\nroute: {}\nmodel identity: {}{}\nactive harness: {}\nlayers: {}\nprovider profile: {} ({})",
        session.harness_preset,
        session.route_source,
        session
            .model_provider
            .as_deref()
            .map(|provider| format!("{provider}:"))
            .unwrap_or_default(),
        session.profile_model_id,
        session.harness_profile,
        session.harness_profile_layers.join(" -> "),
        session.provider_profile.as_deref().unwrap_or("(none)"),
        session.provider_profile_mode,
    )
}

pub(super) fn display_submission(line: &str) -> String {
    line.to_string()
}

fn config_text(cfg: &Config, session: Option<&Session>) -> String {
    format!(
        "agent:   {}\nmodel:   {}\nharness: {}\npreset:  {}\nparallel agents: {}\nendpoint: {}\nplatform: {}",
        cfg.agent,
        cfg.model.as_deref().unwrap_or("(none — run /model)"),
        session.map_or("(resolving)", |session| session.harness_profile.as_str()),
        crate::harness_profiles::configured_harness_preset(cfg).id,
        cfg.effective_max_parallel_agents(),
        if cfg.base_url.is_empty() {
            "(not configured — run /setup)"
        } else {
            &cfg.base_url
        },
        if cfg.platform.base_url.is_empty() {
            "(not configured)".to_string()
        } else if cfg.platform_configured() {
            format!("{} (api-token)", cfg.platform.base_url)
        } else {
            format!("{} (no api token)", cfg.platform.base_url)
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_command_is_hidden_when_runtime_is_unavailable() {
        let mut app = App::new(&Config::default());
        app.voice.available = false;
        app.input = "/vo".into();
        app.cursor = app.input.len();
        assert!(app.slash_matches().is_empty());
        assert!(!help_text(&app).contains("/voice"));
    }

    /// Plugin commands never shadow built-ins; this fails when a command the
    /// composer offers or the help text advertises is missing from the
    /// reserved list.
    #[test]
    fn every_builtin_command_is_reserved_against_extensions() {
        let offered = SLASH_COMMANDS
            .iter()
            .map(|(name, _)| name.trim_start_matches('/'));
        let advertised = HELP
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix('/'))
            .filter_map(|rest| rest.split_whitespace().next());
        let mut checked = 0;
        for name in offered.chain(advertised) {
            assert!(
                crate::extensions::is_reserved(name),
                "/{name} is a built-in but not in extensions::RESERVED_COMMANDS"
            );
            checked += 1;
        }
        assert!(
            checked > SLASH_COMMANDS.len(),
            "parsed no commands from HELP"
        );
    }

    #[test]
    fn plugin_expansions_are_model_input_never_shell_or_slash() {
        assert_eq!(
            submission_route("!ls -la", None),
            SubmissionRoute::Shell("ls -la".into())
        );
        assert_eq!(submission_route("/mode", None), SubmissionRoute::Slash);
        assert_eq!(
            submission_route("explain this", None),
            SubmissionRoute::Prompt
        );
        // A plugin command's line starts with "/" and its expansion may start
        // with "!" or "/", but an expansion always goes to the model.
        for expansion in ["!rm -rf ~", "/model other", "plain prompt"] {
            assert_eq!(
                submission_route("/danger", Some(expansion)),
                SubmissionRoute::Prompt,
                "{expansion:?}"
            );
        }
    }

    #[cfg(feature = "gadgets")]
    #[test]
    fn migrated_gadget_commands_are_model_input_never_shell_or_slash() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(config.path().join("plugins")).unwrap();
        std::fs::write(
            config.path().join("plugins/danger.json"),
            serde_json::json!({
                "version": 1, "name": "danger", "commands": [
                    {"name":"shell","prompt":"!rm -rf ~"},
                    {"name":"slash","prompt":"/model other"},
                    {"name":"path","prompt":"/etc/hosts has a typo. {$ARGS}"}
                ]
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            crate::gadgets::migration::migrate(config.path(), workspace.path(), false, false)
                .unwrap()
                .complete
        );
        let extensions = crate::extensions::discover_roots(config.path(), workspace.path(), true);
        for line in ["/shell", "/slash", "/path fix it"] {
            let expanded = crate::extensions::expand_command(&extensions, line).unwrap();
            assert_eq!(
                submission_route(line, Some(&expanded)),
                SubmissionRoute::Prompt
            );
        }
        assert!(crate::extensions::expand_command(&extensions, "/help").is_none());
    }
}
