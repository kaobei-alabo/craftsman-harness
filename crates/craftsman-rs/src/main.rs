//! `craftsman` — Rust Craftsman CLI.
//!
//! `craftsman`                         → interactive REPL (or one-shot with -m)
//! `craftsman setup`                    → guided first-run configuration
//! Platform access is headless and token-configured (no login subcommand):
//! LLM credentials and MCP servers live in `~/.craftsman/config.json`.

mod a2ui;
mod acp_bridge;
mod acp_serve;
mod agent;
mod agents;
mod ask_user;
mod backend_fs;
mod bridge_support;
mod checkpoints;
mod config;
mod execution_profile;
// Private P2a foundations; do not route a CLI command until all boundaries exist.
#[cfg_attr(not(test), allow(dead_code))]
mod exec;
mod extensions;
#[cfg(test)]
mod fixture_corpus;
#[cfg(feature = "gadgets")]
// Wasmtime raises only this optional module's Rust floor.
#[clippy::msrv = "1.95.0"]
mod gadgets;
mod harness_profiles;
mod history;
mod hitl;
mod hitl_middleware;
mod inbox_poller;
mod llm;
mod lsp;
mod mcp;
mod model_controls;
mod native_model;
mod oneshot_io;
mod picker;
mod platform;
mod platform_os;
mod platform_rest;
#[allow(dead_code, unused_imports)]
mod platform_skills;
mod plugins;
mod prompt;
mod provider_config;
mod route;
mod service_agent;
mod shell;
mod shell_output;
mod skills;
#[allow(dead_code)]
mod skills_browser;
mod store_fs;
mod subagent_gate;
#[cfg(test)]
mod test_environment;
#[allow(dead_code)]
mod theme;
mod threads_db;
mod token_counter;
mod tools;
mod tui;
mod util;
#[cfg(all(feature = "voice", any(target_os = "macos", target_os = "linux")))]
mod voice;
#[cfg(not(all(feature = "voice", any(target_os = "macos", target_os = "linux"))))]
#[path = "voice_stub.rs"]
mod voice;
mod web_tools;
mod workspace_checkpoint;
mod workspace_diff;

use clap::{Parser, Subcommand};
use config::{Config, PermissionMode};

#[derive(Parser)]
#[command(
    name = "craftsman",
    version,
    about = "Craftsman platform chat client (Rust)"
)]
struct Cli {
    /// One-shot: send a single message and exit.
    #[arg(short, long)]
    message: Option<String>,

    /// Bypass all permission prompts (legacy alias for --permission-mode bypass-permissions).
    #[arg(long)]
    auto_approve: bool,

    /// Permission behavior: plan, accept-edits, manual, bypass-permissions, or auto.
    #[arg(long, value_parser = ["plan", "accept-edits", "manual", "bypass-permissions", "auto"])]
    permission_mode: Option<String>,

    /// Compatibility flag; TUI mouse handling is enabled by default.
    #[arg(long, hide = true, conflicts_with = "no_mouse")]
    mouse: bool,

    /// Disable TUI mouse handling and use terminal-native selection instead.
    #[arg(long, conflicts_with = "mouse")]
    no_mouse: bool,

    /// Resume a persisted conversation thread on startup.
    #[arg(long, value_name = "THREAD_ID")]
    session: Option<String>,

    /// Explicitly allow a gadget in this one-shot run (repeatable).
    #[arg(long, value_name = "ID", requires = "message")]
    allow_gadget: Vec<String>,

    /// Permit project gadgets in a one-shot run; grants are still required.
    #[arg(long, requires = "message")]
    trust_workspace: bool,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Inspect or invoke local WASM gadgets without a model connection.
    #[cfg(feature = "gadgets")]
    Gadgets {
        #[command(subcommand)]
        command: gadgets::cli::Command,
    },
    /// Open the guided first-run setup flow.
    Setup,
    /// Run the agent as an Agent Client Protocol (ACP) server over stdio.
    AcpServe {
        /// Permission behavior: plan, accept-edits, manual, bypass-permissions, or auto.
        #[arg(long, value_parser = ["plan", "accept-edits", "manual", "bypass-permissions", "auto"])]
        permission_mode: Option<String>,
    },
    /// Bridge a WebSocket port to a stdio ACP harness (`… -- <cmd> [args]`).
    /// With no `--` tail it bridges to this binary's own `acp-serve`.
    AcpBridge {
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(long, default_value_t = 8080)]
        port: u16,
        /// The harness command line, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        harness: Vec<String>,
    },
    /// Run one restricted shared-runtime turn from a JSON request.
    ServiceAgent {
        /// Read the request from this JSON file; omit to read stdin.
        #[arg(long, value_name = "PATH")]
        request: Option<std::path::PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<std::process::ExitCode> {
    let cli = Cli::parse();
    let mut cfg = Config::load()?.unwrap_or_default();
    if let Some(mode) = cli
        .permission_mode
        .as_deref()
        .and_then(PermissionMode::parse)
    {
        cfg.permission_mode = mode;
    }
    if cfg.has_legacy_credentials() {
        eprintln!(
            "note: login-era fields found in config.json; they are ignored now. Set \
             platform.api_token (or $CRAFTSMAN_PLATFORM_TOKEN), point base_url at the platform's \
             OpenAI-compatible endpoint, and add the platform as an mcp_servers entry."
        );
    }

    #[cfg(not(feature = "gadgets"))]
    if !cli.allow_gadget.is_empty() || cli.trust_workspace {
        anyhow::bail!("this build has no gadget runtime; rebuild with the gadgets cargo feature");
    }
    match cli.command {
        #[cfg(feature = "gadgets")]
        Some(Cmd::Gadgets { command }) => gadgets::cli::run(cfg, command).await,
        Some(Cmd::Setup) => {
            return tui::run(
                cfg,
                cli.auto_approve,
                cli.mouse || !cli.no_mouse,
                true,
                None,
            )
            .await
            .map(|code| std::process::ExitCode::from(code as u8));
        }
        Some(Cmd::AcpServe { permission_mode }) => {
            if let Some(mode) = permission_mode.as_deref().and_then(PermissionMode::parse) {
                cfg.permission_mode = mode;
            }
            acp_serve::run(cfg).await
        }
        Some(Cmd::AcpBridge {
            host,
            port,
            harness,
        }) => acp_bridge::run(&host, port, acp_bridge::Harness::from_argv(harness)).await,
        Some(Cmd::ServiceAgent { request }) => service_agent::run(cfg, request.as_deref()).await,
        None => match cli.message {
            Some(msg) => {
                run_once(
                    cfg,
                    &msg,
                    cli.auto_approve,
                    #[cfg(feature = "gadgets")]
                    cli.allow_gadget.into_iter().collect(),
                    #[cfg(feature = "gadgets")]
                    cli.trust_workspace,
                )
                .await
            }
            None => {
                return tui::run(
                    cfg,
                    cli.auto_approve,
                    cli.mouse || !cli.no_mouse,
                    false,
                    cli.session,
                )
                .await
                .map(|code| std::process::ExitCode::from(code as u8));
            }
        },
    }?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// `craftsman -m` with `shell.process_group` on: a command runs in a process
/// group of its own, so the terminal's SIGINT (Ctrl-C), and SIGHUP when the
/// terminal goes away, no longer reach it. Without this hook an interrupted
/// run would orphan the whole command (P0 CancelToken spike, section 4).
/// The hook kills every live command group and exits with 128 + the signal,
/// as the default disposition would have ended craftsman. Installed only behind
/// the flag, so the default `-m` keeps today's signal behaviour. The TUI has
/// no such hook yet (SIGHUP and SIGTERM there are P2c's `shutdown()`), which
/// is why the flag stays off by default.
#[cfg(unix)]
fn install_one_shot_signal_hook() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut int), Ok(mut term), Ok(mut hup)) = (
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
        signal(SignalKind::hangup()),
    ) else {
        return;
    };
    tokio::spawn(async move {
        let code = tokio::select! {
            _ = int.recv() => 130,
            _ = term.recv() => 143,
            _ = hup.recv() => 129,
        };
        platform_os::kill_live_process_groups();
        std::process::exit(code);
    });
}

/// Headless one-shot (`craftsman -m`): stream to stdout, exit.
async fn run_once(
    cfg: Config,
    message: &str,
    auto_approve: bool,
    #[cfg(feature = "gadgets")] allowed_gadgets: std::collections::BTreeSet<String>,
    #[cfg(feature = "gadgets")] trust_workspace: bool,
) -> anyhow::Result<()> {
    use std::sync::{Arc, Mutex};
    let capabilities = execution_profile::ExecutionProfile::Local.capabilities();
    debug_assert!(capabilities.real_filesystem);
    debug_assert!(!capabilities.virtual_filesystem);
    debug_assert!(capabilities.shell);
    debug_assert!(capabilities.local_sessions);
    debug_assert!(capabilities.persistent_home);
    if cfg.model.is_none() {
        anyhow::bail!("no model configured; run the REPL and /model first");
    }
    let approval = Arc::new(Mutex::new(
        hitl::ApprovalState {
            mode: if auto_approve || cfg.auto_approve {
                config::PermissionMode::BypassPermissions
            } else {
                cfg.permission_mode
            },
            auto_approve: auto_approve || cfg.auto_approve,
            // --auto-approve covers network too; otherwise the network switch
            // is independent (config `auto_approve_web`) — see needs_approval.
            auto_approve_network: auto_approve || cfg.auto_approve_web(),
            shell_allow_list: cfg.shell_allow_list.clone(),
        }
        .with_env_allow_list(),
    ));
    #[cfg(unix)]
    if shell::ShellSettings::from_config(&cfg).process_group {
        install_one_shot_signal_hook();
    }
    // A custom main sink makes child/compaction providers silent. Its latched
    // failure cancels this turn, and provider guards prevent same-poll dispatch
    // of tools after a broken content delta. Other frontends keep their sinks.
    let output = Arc::new(oneshot_io::Output::new(std::io::stdout()));
    let Some(mut session) = agent::build_session_ext(
        &cfg,
        approval,
        prompt::NONINTERACTIVE_INSTRUCTIONS,
        true,
        false,
        None,
        agent::SessionExtras {
            token_sink: Some(output.sink()),
            output_closed: Some(output.closed_flag()),
            #[cfg(feature = "gadgets")]
            local_gadgets: true,
            #[cfg(feature = "gadgets")]
            allowed_gadgets,
            #[cfg(feature = "gadgets")]
            trust_workspace,
            ..agent::SessionExtras::default()
        },
    )
    .await
    else {
        anyhow::bail!("no model configured; run the REPL and /model first");
    };

    use craftsman_harness::TurnResult;
    let result = tokio::select! {
        biased;
        _ = output.failed() => None,
        result = session.agent.run_turn(message) => Some(result),
    };
    // Drain once at the end: foreground task results are already inline, but a
    // one-shot turn that spawned a background subagent may have a completion
    // waiting. Surface any that landed before exit.
    if output.failure_kind().is_none() {
        for n in session.notifications.drain() {
            if let craftsman_harness::NotificationKind::Completed(text) = n.kind {
                eprintln!("• background agent '{}' finished: {text}", n.from);
            }
        }
    }
    session.lsp.shutdown().await;
    if let Some(kind) = output.failure_kind() {
        return one_shot_output_failure(kind);
    }
    let result =
        result.ok_or_else(|| anyhow::anyhow!("one-shot output closed without an IO error"))?;
    if matches!(
        &result,
        Ok(_) | Err(craftsman_harness::AgentError::MaxRounds(_))
    ) {
        output.write(b"\n");
        if let Some(kind) = output.failure_kind() {
            return one_shot_output_failure(kind);
        }
    }
    match result {
        // A one-shot main-agent turn gates inline and shouldn't suspend; treat a
        // stray suspension as a clean finish rather than crashing.
        Ok(TurnResult::Done(_)) | Ok(TurnResult::Suspended(_)) => Ok(()),
        Err(craftsman_harness::AgentError::MaxRounds(_)) => {
            agent::rollback_dangling_user(&session.agent);
            std::process::exit(2);
        }
        Err(e) => {
            agent::rollback_dangling_user(&session.agent);
            anyhow::bail!("{e}");
        }
    }
}

/// Broken stdout pipes are ordinary consumer shutdown, not a panic or a model
/// failure. Other IO failures receive a fixed diagnostic, never writer text.
fn one_shot_output_failure(kind: std::io::ErrorKind) -> anyhow::Result<()> {
    if kind == std::io::ErrorKind::BrokenPipe {
        Ok(())
    } else {
        anyhow::bail!("could not write one-shot output to stdout")
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn retired_websocket_server_commands_are_rejected_before_dispatch() {
        for command in ["serve", "desktop-serve"] {
            let error = Cli::try_parse_from(["craftsman", command, "--port", "8080"])
                .err()
                .expect("retired command must not enter configuration or session construction");
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
        }
    }

    #[test]
    fn acp_stdio_and_bridge_commands_preserve_their_arguments() {
        let cli =
            Cli::try_parse_from(["craftsman", "acp-serve", "--permission-mode", "manual"]).unwrap();
        assert!(
            matches!(cli.command, Some(Cmd::AcpServe { permission_mode: Some(mode) }) if mode == "manual")
        );
        let cli = Cli::try_parse_from([
            "craftsman",
            "acp-bridge",
            "--host",
            "127.0.0.1",
            "--port",
            "8080",
            "--",
            "craftsman",
            "acp-serve",
        ])
        .unwrap();
        assert!(
            matches!(cli.command, Some(Cmd::AcpBridge { host, port: 8080, harness }) if host == "127.0.0.1" && harness == ["craftsman", "acp-serve"])
        );
    }
}
