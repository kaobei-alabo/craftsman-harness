//! Explicit, private headless bootstrap (R1/R3/R6), not a CLI route.
//!
//! The caller supplies absolute paths; no HOME/config/cwd mutation or global
//! store cache is used. Discovery is bounded and no-follow. Ephemeral mode
//! never opens storage; durable selection uses a read-only catalog before any
//! writer, and does not retire legacy workspace metadata. PreparedInput adds
//! complete instruction/file admission before that session phase; the older
//! configuration-only Prepared entry point does not imply input validation.

mod discovery;
mod output_input;
mod stdin_input;
pub(in crate::exec) use stdin_input::PreparedStdinInput;
pub(crate) use stdin_input::StdinPreparationError;
mod sessions;
pub(crate) use output_input::OutputPreparationError;
pub(in crate::exec) use output_input::PreparedOutputInput;
#[cfg(test)]
mod tests;

use super::confined_fs::{ConfinedFs, resolve_config};
use super::input::{Admitted, InputError, Selected, Sources};
use super::permissions::{InvocationPermissions, Mode, Selectors};
use crate::config::{Config, PermissionMode};
use crate::exec::redaction::capture::Captured;
use crate::shell::ShellSettings;
use cap_std::fs::Dir;
use craftsman_harness::AgentState;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

pub(crate) use discovery::{Discovery, SourceRoot};
pub(crate) use sessions::Persistence;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum SessionSelection {
    #[default]
    Fresh,
    Ephemeral,
    Resume(String),
    Continue,
}

/// Typed selection makes mutually exclusive session modes unrepresentable.
pub(crate) struct Options {
    pub(crate) workspace: PathBuf,
    pub(crate) config_dir: PathBuf,
    pub(crate) trust_workspace: bool,
    pub(crate) session: SessionSelection,
    pub(crate) selectors: Selectors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum PreparationError {
    #[error("headless construction needs valid absolute workspace/config scopes")]
    Scope,
    #[error("headless config is not a bounded, regular UTF-8 JSON file")]
    Config,
    #[error("headless config selects an invalid agent identifier")]
    Agent,
    #[error("no model is configured in the selected config directory")]
    NoModel,
    #[error("invalid or unsupported explicit invocation permissions")]
    Permissions,
    #[error("headless bootstrap source is unsafe or exceeds its bounds")]
    Source,
    #[error("selected headless session does not exist")]
    MissingSession,
    #[error(
        "selected session has missing, inconsistent or mismatched canonical primary workspace metadata"
    )]
    WorkspaceMismatch,
    #[error(
        "selected thread store has an unsupported or legacy schema; choose/migrate it interactively"
    )]
    Schema,
    #[error("selected thread checkpoint is invalid or exceeds its bounds")]
    Checkpoint,
    #[error("selected headless thread store is unavailable")]
    Storage,
    #[error("operating-system random identity generation failed")]
    Random,
    #[error("headless configured credentials cannot be safely captured")]
    Credentials,
    #[error("headless construction worker failed")]
    Worker,
}

type Result<T> = std::result::Result<T, PreparationError>;

/// Source handles share bootstrap's selected config authority and the SAME
/// workspace handle as ConfinedFs. A project handle is not workspace trust.
#[derive(Clone)]
pub(crate) struct SourceRoots {
    pub(crate) config: Option<Arc<SourceRoot>>,
    pub(crate) workspace: Arc<SourceRoot>,
}

/// Owns preparation data only. It does not start providers, MCP/LSP, guest code,
/// memory, snapshots, a journal or the unfinished headless run/event producer.
pub(crate) struct Prepared {
    workspace: PathBuf,
    config_dir: PathBuf,
    config: Config,
    shell: ShellSettings,
    credentials: Captured,
    permissions: Arc<InvocationPermissions>,
    filesystem: ConfinedFs,
    discovery: Discovery,
    source_roots: SourceRoots,
    persistence: Persistence,
    state: AgentState,
    ignored_ambient_settings: bool,
}

/// Pairs complete admitted input with the bootstrap authority that read it.
/// Construction cannot assign a thread/store before all input/framing checks.
pub(crate) struct PreparedInput {
    prepared: Prepared,
    input: Admitted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum InputPreparationError {
    #[error("{0}")]
    Input(#[from] InputError),
    #[error("{0}")]
    Preparation(#[from] PreparationError),
    #[error("private input runtime construction failed")]
    Runtime,
}
impl PreparedInput {
    pub(crate) async fn prepare(
        options: Options,
        sources: Sources<'_>,
    ) -> std::result::Result<Self, InputPreparationError> {
        let selected = Selected::select(sources)?;
        let selection = options.session.clone();
        let bootstrap = tokio::task::spawn_blocking(move || Bootstrap::load(options))
            .await
            .map_err(|_| PreparationError::Worker)??;
        let input = selected.load(&bootstrap.filesystem).await?;
        let prepared = Prepared::from_bootstrap(bootstrap, selection).await?;
        Ok(Self { prepared, input })
    }
    pub(super) fn into_parts(self) -> (Prepared, Admitted) {
        (self.prepared, self.input)
    }
}

struct Bootstrap {
    workspace: PathBuf,
    config_dir: PathBuf,
    config: Config,
    shell: ShellSettings,
    credentials: Captured,
    permissions: Arc<InvocationPermissions>,
    filesystem: ConfinedFs,
    discovery: Discovery,
    source_roots: SourceRoots,
    ignored_ambient_settings: bool,
}

impl Prepared {
    pub(crate) async fn prepare(options: Options) -> Result<Self> {
        let selection = options.session.clone();
        let bootstrap = tokio::task::spawn_blocking(move || Bootstrap::load(options))
            .await
            .map_err(|_| PreparationError::Worker)??;
        Self::from_bootstrap(bootstrap, selection).await
    }

    #[cfg(test)]
    pub(crate) async fn prepare_with_environment_for_tests(
        options: Options,
        environment: std::collections::BTreeMap<String, String>,
    ) -> Result<Self> {
        let selection = options.session.clone();
        let bootstrap = tokio::task::spawn_blocking(move || {
            Bootstrap::load_with_capture(options, |cfg, names| {
                Captured::with_lookup(cfg, names, |name| Ok(environment.get(name).cloned()))
            })
        })
        .await
        .map_err(|_| PreparationError::Worker)??;
        Self::from_bootstrap(bootstrap, selection).await
    }

    async fn from_bootstrap(bootstrap: Bootstrap, selection: SessionSelection) -> Result<Self> {
        let (persistence, state) =
            sessions::prepare(&bootstrap.config_dir, &bootstrap.workspace, selection).await?;
        Ok(Self {
            workspace: bootstrap.workspace,
            config_dir: bootstrap.config_dir,
            config: bootstrap.config,
            shell: bootstrap.shell,
            credentials: bootstrap.credentials,
            permissions: bootstrap.permissions,
            filesystem: bootstrap.filesystem,
            discovery: bootstrap.discovery,
            source_roots: bootstrap.source_roots,
            persistence,
            state,
            ignored_ambient_settings: bootstrap.ignored_ambient_settings,
        })
    }

    pub(crate) fn workspace(&self) -> &Path {
        &self.workspace
    }
    pub(crate) fn config_dir(&self) -> &Path {
        &self.config_dir
    }
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }
    pub(crate) fn shell(&self) -> &ShellSettings {
        &self.shell
    }
    pub(crate) fn credentials(&self) -> &Captured {
        &self.credentials
    }
    pub(crate) fn permissions(&self) -> &Arc<InvocationPermissions> {
        &self.permissions
    }
    pub(crate) fn filesystem(&self) -> &ConfinedFs {
        &self.filesystem
    }
    pub(crate) fn discovery(&self) -> &Discovery {
        &self.discovery
    }
    pub(crate) fn source_roots(&self) -> &SourceRoots {
        &self.source_roots
    }
    pub(crate) fn persistence(&self) -> &Persistence {
        &self.persistence
    }
    pub(crate) fn initial_state(&self) -> &AgentState {
        &self.state
    }
    pub(crate) fn ignored_ambient_settings(&self) -> bool {
        self.ignored_ambient_settings
    }
}

impl Bootstrap {
    fn load(options: Options) -> Result<Self> {
        Self::load_with_capture(options, Captured::capture)
    }

    fn load_with_capture(
        options: Options,
        capture: impl FnOnce(
            &Config,
            &[String],
        ) -> std::result::Result<
            Captured,
            crate::exec::redaction::capture::CaptureError,
        >,
    ) -> Result<Self> {
        if !options.workspace.is_absolute() || !options.config_dir.is_absolute() {
            return Err(PreparationError::Scope);
        }
        let config_dir =
            resolve_config(&options.config_dir).map_err(|_| PreparationError::Scope)?;
        let config_root = SourceRoot::open_optional(&config_dir)?.map(Arc::new);
        let bytes = match &config_root {
            Some(root) => root
                .read_optional(&["config.json"], 1024 * 1024)
                .map_err(|_| PreparationError::Config)?,
            None => None,
        };
        let mut config: Config = match bytes {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| PreparationError::Config)?,
            None => Config::default(),
        };
        if config.model.as_deref().is_none_or(|m| m.trim().is_empty()) {
            return Err(PreparationError::NoModel);
        }
        if !discovery::valid_agent(&config.agent) {
            return Err(PreparationError::Agent);
        }
        let mut shell = ShellSettings::from_config(&config);
        let ignored_ambient_settings = config.auto_approve
            || config.permission_mode != PermissionMode::Manual
            || !config.shell_allow_list.is_empty()
            || config.auto_approve_web()
            || !shell.env_scrub
            || !shell.env_pass.is_empty();
        let permissions = Arc::new(
            InvocationPermissions::from_explicit(options.selectors, &shell.secret_vars)
                .map_err(|_| PreparationError::Permissions)?,
        );
        let credentials =
            capture(&config, &shell.secret_vars).map_err(|_| PreparationError::Credentials)?;
        shell.env_scrub = true;
        shell.env_pass = permissions.env_pass().to_vec();

        // Keep routing Config from reintroducing the ambient shell pass list if
        // a caller inspects it. Runtime adapters must still use the captured
        // shell settings, which retain credential sources from disabled services.
        let mut shell_json = config
            .extra
            .get("shell")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        shell_json.insert("env_scrub".into(), serde_json::Value::Bool(true));
        shell_json.insert("env_pass".into(), serde_json::json!(shell.env_pass));
        config
            .extra
            .insert("shell".into(), serde_json::Value::Object(shell_json));
        // Native shell timing/format choices may persist, broad approvals may not.
        config.permission_mode = match permissions.mode() {
            Mode::Manual => PermissionMode::Manual,
            Mode::Plan => PermissionMode::Plan,
        };
        config.auto_approve = false;
        config.shell_allow_list.clear();
        config.mcp_servers.clear();
        config.lsp_servers.clear();
        config.max_parallel_agents = 1;
        config
            .extra
            .insert("auto_approve_web".into(), serde_json::Value::Bool(false));
        let filesystem = ConfinedFs::new(&options.workspace, &config_dir, permissions.clone())
            .map_err(|_| PreparationError::Scope)?;
        let workspace = filesystem.root().to_path_buf();
        let directory: Dir = filesystem
            .discovery_directory()
            .map_err(|_| PreparationError::Scope)?;
        let discovery = Discovery::load(
            config_root.as_deref(),
            &directory,
            &config.agent,
            options.trust_workspace,
            crate::extensions::terminal_gadgets_enabled(&config),
        )?;
        let source_roots = SourceRoots {
            config: config_root,
            workspace: Arc::new(SourceRoot::from_directory(directory)),
        };
        Ok(Self {
            workspace,
            config_dir,
            config,
            shell,
            credentials,
            permissions,
            filesystem,
            discovery,
            source_roots,
            ignored_ambient_settings,
        })
    }
}

// Separate native-stdin/file composition; existing construction bodies stay exact.
mod stdin_output_input;
pub(in crate::exec) use stdin_output_input::PreparedStdinOutputInput;
pub(crate) use stdin_output_input::StdinOutputPreparationError;

// The private frontend reuses ONLY old ID grammar, never a store/runtime handle.
pub(in crate::exec) fn session_argument_id_valid(id: &str) -> bool {
    sessions::valid_argument_id(id)
}
