use super::{GadgetRuntime, RuntimeOptions};
use crate::config::Config;
use clap::{Args, Subcommand};
use std::collections::BTreeSet;
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Args)]
pub struct Grants {
    /// Allow this gadget id for this invocation; does not persist a grant.
    #[arg(long, value_name = "ID")]
    allow_gadget: Vec<String>,
    /// Permit project-local gadgets (capability approval is still required).
    #[arg(long)]
    trust_workspace: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Convert legacy JSON plugins without overwriting gadgets or deleting originals.
    Migrate {
        #[arg(long)]
        project: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Scaffold a standalone gadget using the SDK embedded in this binary.
    New {
        id: String,
        #[arg(long)]
        project: bool,
    },
    /// Build and atomically publish a component; never execute it before approval.
    Build {
        id: String,
        #[arg(long)]
        project: bool,
    },
    /// Diagnose the pinned Rust compiler and WASM target without installing them.
    Doctor,

    /// List gadgets; unapproved components are never executed for schemas.
    List {
        #[command(flatten)]
        grants: Grants,
    },
    /// Call a gadget function directly, even with gadgets.enabled off.
    Call {
        gadget: String,
        function: String,
        #[arg(long, default_value = "{}", value_name = "JSON")]
        args: String,
        /// Show captured gadget stdout/stderr/log output, safely escaped.
        #[arg(long)]
        logs: bool,
        #[command(flatten)]
        grants: Grants,
    },
}

pub fn enabled(cfg: &Config) -> bool {
    cfg.extra
        .get("gadgets")
        .and_then(|v| v.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub fn options(cfg: &Config) -> anyhow::Result<RuntimeOptions> {
    let mut options = RuntimeOptions::default();
    if let Some(timeout) = cfg.extra.get("gadgets").and_then(|v| v.get("timeout_secs")) {
        let timeout = timeout
            .as_u64()
            .filter(|s| (1..=300).contains(s))
            .ok_or_else(|| {
                anyhow::anyhow!("gadgets.timeout_secs must be an integer from 1 to 300")
            })?;
        options.timeout = std::time::Duration::from_secs(timeout);
    }
    Ok(options)
}

/// JSON serialization escapes C0 characters; also escape Unicode controls
/// rather than writing gadget-provided control characters to a terminal.
pub fn terminal_safe(text: &str) -> String {
    let mut safe = String::new();
    for c in text.chars() {
        if c.is_control() {
            safe.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            safe.push(c);
        }
    }
    safe
}

fn print_value(value: &serde_json::Value) -> anyhow::Result<()> {
    let mut output = terminal_safe(&serde_json::to_string(value)?);
    output.push('\n');
    std::io::stdout().lock().write_all(output.as_bytes())?;
    Ok(())
}

pub async fn run(cfg: Config, command: Command) -> anyhow::Result<()> {
    let workspace = std::env::current_dir()?;
    let command = match command {
        Command::Migrate { project, dry_run } => {
            let report = super::migration::migrate(
                &crate::config::config_dir(),
                &workspace,
                project,
                dry_run,
            )
            .map_err(|e| anyhow::anyhow!(terminal_safe(&e.to_string())))?;
            print_value(&serde_json::to_value(&report)?)?;
            anyhow::ensure!(
                report.complete,
                "migration has conflicts; originals and existing gadgets were retained"
            );
            return Ok(());
        }
        Command::New { id, project } => {
            let value =
                super::authoring::scaffold(&crate::config::config_dir(), &workspace, &id, project)
                    .map_err(|e| anyhow::anyhow!(terminal_safe(&e.to_string())))?;
            return print_value(&value);
        }
        Command::Build { id, project } => {
            let value = super::authoring::build(&cfg, &workspace, &id, project)
                .await
                .map_err(|e| anyhow::anyhow!(terminal_safe(&e.to_string())))?;
            return print_value(&value);
        }
        Command::Doctor => {
            let value = super::authoring::doctor(&cfg, &workspace)
                .await
                .map_err(|e| anyhow::anyhow!(terminal_safe(&e.to_string())))?;
            return print_value(&value);
        }
        command => command,
    };
    let grants = match &command {
        Command::List { grants } | Command::Call { grants, .. } => grants,
        _ => unreachable!("authoring commands returned above"),
    };
    let mut options = options(&cfg)?;
    options.allowed_gadgets = grants.allow_gadget.iter().cloned().collect::<BTreeSet<_>>();
    options.trust_workspace = grants.trust_workspace;
    options.approval = Arc::new(Mutex::new(crate::hitl::ApprovalState {
        mode: if cfg.auto_approve {
            crate::config::PermissionMode::BypassPermissions
        } else {
            cfg.permission_mode
        },
        auto_approve: cfg.auto_approve,
        auto_approve_network: cfg.auto_approve_web(),
        shell_allow_list: cfg.shell_allow_list.clone(),
    }));
    let runtime = GadgetRuntime::new(
        &std::env::current_dir()?,
        &crate::config::config_dir(),
        options,
    )
    .map_err(|e| anyhow::anyhow!(terminal_safe(&e.to_string())))?;
    let value = match command {
        Command::List { .. } => serde_json::to_value(
            runtime
                .list()
                .await
                .map_err(|e| anyhow::anyhow!(terminal_safe(&e.to_string())))?,
        )?,
        Command::Call {
            gadget,
            function,
            args,
            logs,
            ..
        } => {
            let args = serde_json::from_str(&args)?;
            let result = runtime
                .call(&gadget, &function, args)
                .await
                .map_err(|e| anyhow::anyhow!(terminal_safe(&e.to_string())))?;
            if logs {
                std::io::stderr().lock().write_all(
                    format!(
                        "gadget logs: {}\n",
                        terminal_safe(&serde_json::to_string(&result.logs)?)
                    )
                    .as_bytes(),
                )?;
            }
            serde_json::from_str::<serde_json::Value>(&result.json)?
        }
        _ => unreachable!("authoring commands returned above"),
    };
    print_value(&value)
}
