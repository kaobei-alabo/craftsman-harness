use super::grants::GrantStore;
use super::host::{HostState, Workspace, bindings};
use super::manifest::{
    Capabilities, MAX_COMPONENT_BYTES, MAX_GADGETS_PER_ROOT, MAX_MANIFEST_BYTES, Manifest, valid_id,
};
use super::{ErrorKind, GadgetError};
use crate::agent::{ApprovalPrompt, ApprovalPromptResult};
use crate::hitl::ApprovalState;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use craftsman_gadget::{Description, FunctionDescription, MAX_JSON_BYTES};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store, Trap, UpdateDeadline};

#[derive(Clone)]
pub struct RuntimeOptions {
    pub timeout: Duration,
    pub memory_bytes: usize,
    pub interactive: bool,
    pub trust_workspace: bool,
    pub allowed_gadgets: BTreeSet<String>,
    pub approval: Arc<Mutex<ApprovalState>>,
    pub prompt: Option<ApprovalPrompt>,
    pub disk_cache: bool,
    pub workspace_checkpoints: Option<Arc<crate::workspace_checkpoint::WorkspaceCheckpointManager>>,
}
impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            memory_bytes: 256 * 1024 * 1024,
            interactive: false,
            trust_workspace: false,
            allowed_gadgets: BTreeSet::new(),
            approval: Arc::new(Mutex::new(ApprovalState::default())),
            prompt: None,
            disk_cache: true,
            workspace_checkpoints: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GadgetInfo {
    pub id: String,
    pub version: String,
    pub description: String,
    pub source: PathBuf,
    pub project: bool,
    pub status: String,
    pub component_hash: Option<String>,
    pub manifest_hash: Option<String>,
    pub commands: Vec<String>,
    pub agents: Vec<String>,
    pub has_prompt: bool,
    pub capabilities: Capabilities,
    pub functions: Vec<FunctionDescription>,
    pub error: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
}
impl Stamp {
    fn new(metadata: &fs::Metadata) -> Self {
        Self {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            },
        }
    }
}
#[derive(Clone)]
struct Bundle {
    manifest: Manifest,
    manifest_hash: String,
    source: PathBuf,
    project: bool,
    hash: Option<String>,
}
#[derive(Clone)]
struct Scanned {
    info: GadgetInfo,
    bundle: Option<Bundle>,
}
#[derive(Clone)]
struct CachedScan {
    manifest: Stamp,
    component: Option<Stamp>,
    bundle: Bundle,
}

#[derive(Debug)]
pub struct CallResult {
    pub json: String,
    pub duration: Duration,
    pub network_calls: u64,
    pub logs: String,
    pub logs_truncated: bool,
}

// One ticker per runtime engine; its Drop wakes and joins the thread. It does
// not hold an Engine alive after the last runtime is dropped.
pub(super) struct Ticker {
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Ticker {
    pub(super) fn new(engine: Engine) -> Result<Self, GadgetError> {
        let (stop, receive) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("gadget-epochs".into())
            .spawn(move || {
                while matches!(
                    receive.recv_timeout(Duration::from_millis(10)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    engine.increment_epoch();
                }
            })
            .map_err(|e| GadgetError::new(ErrorKind::Io, e.to_string()))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for Ticker {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The wall-clock budget pauses only while a human approval modal is open.
/// Epoch checks bound CPU loops; the outer timer also bounds WASI sleeps and
/// asynchronous host calls, which do not advance guest instructions.
pub struct CallBudget {
    deadline: Mutex<Instant>,
    paused: AtomicBool,
    changed: tokio::sync::Notify,
}
impl CallBudget {
    pub fn new(duration: Duration) -> Self {
        Self {
            deadline: Mutex::new(Instant::now() + duration),
            paused: AtomicBool::new(false),
            changed: tokio::sync::Notify::new(),
        }
    }
    pub(super) fn expired(&self) -> bool {
        !self.paused.load(Ordering::Acquire) && Instant::now() >= *self.deadline.lock().unwrap()
    }
    pub fn remaining(&self) -> Duration {
        self.deadline
            .lock()
            .unwrap()
            .saturating_duration_since(Instant::now())
    }
    pub fn pause(self: &Arc<Self>) -> Pause {
        self.paused.store(true, Ordering::Release);
        self.changed.notify_one();
        Pause {
            budget: self.clone(),
            started: Instant::now(),
        }
    }
    pub(super) async fn run<T>(
        &self,
        future: impl Future<Output = Result<T, GadgetError>>,
    ) -> Result<T, GadgetError> {
        let mut future = std::pin::pin!(future);
        loop {
            let changed = self.changed.notified();
            if self.paused.load(Ordering::Acquire) {
                tokio::select! { result = &mut future => return result, _ = changed => {} }
            } else {
                let deadline = tokio::time::Instant::from_std(*self.deadline.lock().unwrap());
                tokio::select! {
                    result = &mut future => return result,
                    _ = changed => {},
                    _ = tokio::time::sleep_until(deadline) => {
                        if self.expired() { return Err(GadgetError::new(ErrorKind::Timeout, "gadget call timed out")); }
                    },
                }
            }
        }
    }
}
pub struct Pause {
    budget: Arc<CallBudget>,
    started: Instant,
}
impl Drop for Pause {
    fn drop(&mut self) {
        *self.budget.deadline.lock().unwrap() += self.started.elapsed();
        self.budget.paused.store(false, Ordering::Release);
        self.budget.changed.notify_one();
    }
}

pub struct GadgetRuntime {
    engine: Engine,
    _ticker: Ticker,
    linker: Linker<HostState>,
    workspace: PathBuf,
    config: PathBuf,
    filesystem: Arc<Workspace>,
    options: RuntimeOptions,
    client: reqwest::Client,
    grants: Arc<GrantStore>,
    scans: Arc<Mutex<HashMap<PathBuf, CachedScan>>>,
    components: Mutex<HashMap<String, Component>>,
    descriptions: Mutex<HashMap<String, Result<Description, GadgetError>>>,
    last_errors: Mutex<HashMap<String, String>>,
    approval_lock: tokio::sync::Mutex<()>,
    catalog_seen: Mutex<BTreeSet<String>>,
}

impl GadgetRuntime {
    pub fn new(
        workspace: &Path,
        config: &Path,
        options: RuntimeOptions,
    ) -> Result<Self, GadgetError> {
        if options.timeout.is_zero()
            || options.timeout > Duration::from_secs(300)
            || !(4 * 1024 * 1024..=256 * 1024 * 1024).contains(&options.memory_bytes)
            || options.allowed_gadgets.iter().any(|id| !valid_id(id))
        {
            return Err(GadgetError::new(
                ErrorKind::InvalidArgs,
                "invalid gadget timeout, memory limit or allow-gadget id",
            ));
        }
        let workspace = workspace.canonicalize().map_err(io_error)?;
        let config = if config.is_absolute() {
            config.to_path_buf()
        } else {
            std::env::current_dir().map_err(io_error)?.join(config)
        };
        let filesystem = Arc::new(Workspace::new(&workspace, &config)?);
        let mut settings = wasmtime::Config::new();
        settings
            .wasm_component_model(true)
            .epoch_interruption(true)
            .wasm_gc(false);
        // GC, exception handling and threads are disabled by the selected
        // Wasmtime cargo features; their configuration APIs are not compiled.
        if options.disk_cache {
            // Wasmtime's persistent cache contains native code, not just WASM.
            // Never trust a project/shared cache directory. Windows remains
            // in-memory until a DACL ownership check is available.
            if let Some(directory) = trusted_cache_directory(&config, &workspace) {
                let mut cache = wasmtime::CacheConfig::new();
                cache
                    .with_directory(directory)
                    .with_files_total_size_soft_limit(512 * 1024 * 1024);
                if let Ok(cache) = wasmtime::Cache::new(cache) {
                    settings.cache(Some(cache));
                }
            }
        }
        let engine = Engine::new(&settings).map_err(wasm_error)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(wasm_error)?;
        bindings::craftsman::gadget::host::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
            &mut linker,
            |state| state,
        )
        .map_err(wasm_error)?;
        let ticker = Ticker::new(engine.clone())?;
        // Redirects and environment proxy routing must not bypass the exact
        // host allowlist. No user cookies, credentials or default headers.
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(io_error)?;
        let grants = Arc::new(GrantStore::new(&config));
        Ok(Self {
            engine,
            _ticker: ticker,
            linker,
            workspace,
            config,
            filesystem,
            options,
            client,
            grants,
            scans: Arc::new(Mutex::new(HashMap::new())),
            components: Mutex::new(HashMap::new()),
            descriptions: Mutex::new(HashMap::new()),
            last_errors: Mutex::new(HashMap::new()),
            approval_lock: tokio::sync::Mutex::new(()),
            catalog_seen: Mutex::new(BTreeSet::new()),
        })
    }

    async fn discover(&self) -> Result<Vec<Scanned>, GadgetError> {
        let roots = [
            (self.config.join("gadgets"), false),
            (self.workspace.join(".craftsman/gadgets"), true),
        ];
        let scans = self.scans.clone();
        tokio::task::spawn_blocking(move || {
            let mut effective = BTreeMap::new();
            let mut seen = BTreeSet::new();
            for (root, project) in roots {
                let directory = match fs::read_dir(&root) {
                    Ok(directory) => directory,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => {
                        effective.insert(
                            format!("{}:discovery", root.display()),
                            broken("<discovery>", &root, project, e.to_string()),
                        );
                        continue;
                    }
                };
                let mut children = Vec::new();
                for (index, entry) in directory.enumerate() {
                    if index >= crate::extensions::MAX_DIR_ENTRIES {
                        effective.insert(
                            format!("{}:limit", root.display()),
                            broken(
                                "<limit>",
                                &root,
                                project,
                                "at most 1024 total directory entries per gadget root".into(),
                            ),
                        );
                        children.clear();
                        break;
                    }
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(e) => {
                            effective.insert(
                                format!("{}:discovery", root.display()),
                                broken("<discovery>", &root, project, e.to_string()),
                            );
                            break;
                        }
                    };
                    if entry.file_name().to_string_lossy().starts_with('.') {
                        continue;
                    }
                    children.push(entry.path());
                    if children.len() > MAX_GADGETS_PER_ROOT {
                        effective.insert(
                            format!("{}:limit", root.display()),
                            broken(
                                "<limit>",
                                &root,
                                project,
                                "at most 64 gadgets per root".into(),
                            ),
                        );
                        children.clear(); // Fail the whole over-limit root deterministically.
                        break;
                    }
                }
                children.sort();
                for directory in children.into_iter().take(MAX_GADGETS_PER_ROOT) {
                    let id = directory
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    seen.insert(directory.clone());
                    let scanned = scan(&directory, &id, project, &scans);
                    // Project definitions win, including broken definitions.
                    // A broken project override never runs the user gadget by
                    // accident, nor inherits its source-bound approval.
                    effective.insert(id, scanned);
                }
            }
            scans.lock().unwrap().retain(|path, _| seen.contains(path));
            Ok(effective.into_values().collect())
        })
        .await
        .map_err(io_error)?
    }

    async fn find(&self, id: &str) -> Result<Bundle, GadgetError> {
        if !valid_id(id) {
            return Err(GadgetError::new(
                ErrorKind::InvalidArgs,
                "invalid gadget id",
            ));
        }
        let scanned = self
            .discover()
            .await?
            .into_iter()
            .find(|g| g.info.id == id)
            .ok_or_else(|| {
                GadgetError::new(ErrorKind::NotFound, format!("gadget not found: {id}"))
            })?;
        scanned.bundle.ok_or_else(|| {
            GadgetError::new(
                ErrorKind::InvalidGadget,
                scanned.info.error.unwrap_or_else(|| "broken gadget".into()),
            )
        })
    }

    async fn approved(&self, bundle: &Bundle, prompt: bool) -> Result<bool, GadgetError> {
        let Some(hash) = &bundle.hash else {
            return Ok(true);
        };
        if bundle.project && !self.options.interactive && !self.options.trust_workspace {
            return Err(GadgetError::new(
                ErrorKind::NeedsApproval,
                "project gadgets need --trust-workspace in headless runs",
            ));
        }
        if self.options.allowed_gadgets.contains(&bundle.manifest.id) {
            return Ok(true);
        }
        let _prompt_lock = if prompt && self.options.interactive {
            Some(self.approval_lock.lock().await)
        } else {
            None
        };
        let grants = self.grants.clone();
        let copy = bundle.clone();
        if tokio::task::spawn_blocking(move || {
            grants.approved(
                &copy.manifest.id,
                &copy.source,
                copy.hash.as_ref().unwrap(),
                &copy.manifest.capabilities,
            )
        })
        .await
        .map_err(io_error)??
        {
            return Ok(true);
        }
        let Some(callback) = self
            .options
            .prompt
            .clone()
            .filter(|_| prompt && self.options.interactive)
        else {
            return Ok(false);
        };
        let summary = format!(
            "Approve gadget {} ({})",
            bundle.manifest.id, bundle.manifest.version
        );
        let details = format!(
            "Source: {}\nComponent SHA-256: {hash}\n\n{}\n\nA rebuild inherits this grant only at the same source path with equal or narrower capabilities. Read + HTTP can send workspace contents to those hosts. File writes still require their own approval.",
            bundle.source.display(),
            serde_json::to_string_pretty(&bundle.manifest).map_err(io_error)?
        );
        let decision =
            tokio::task::spawn_blocking(move || callback("gadget_call", &summary, &details))
                .await
                .map_err(io_error)?;
        match decision {
            ApprovalPromptResult::Approve | ApprovalPromptResult::ApproveAlways => {
                let grants = self.grants.clone();
                let bundle = bundle.clone();
                tokio::task::spawn_blocking(move || {
                    grants.approve(
                        &bundle.manifest.id,
                        &bundle.source,
                        bundle.hash.as_ref().unwrap(),
                        &bundle.manifest.capabilities,
                    )
                })
                .await
                .map_err(io_error)??;
                Ok(true)
            }
            ApprovalPromptResult::Reject(reason) => Err(GadgetError::new(
                ErrorKind::NeedsApproval,
                format!("gadget approval rejected: {reason}"),
            )),
        }
    }

    async fn component(&self, bundle: &Bundle) -> Result<Component, GadgetError> {
        let hash = bundle.hash.as_ref().ok_or_else(|| {
            GadgetError::new(ErrorKind::NotFound, "data-only gadget has no functions")
        })?;
        if let Some(component) = self.components.lock().unwrap().get(hash).cloned() {
            return Ok(component);
        }
        let source = bundle.source.clone();
        let expected = hash.clone();
        let engine = self.engine.clone();
        let component = tokio::task::spawn_blocking(move || {
            let bytes = read_bounded(&source, "gadget.wasm", MAX_COMPONENT_BYTES)?;
            if digest(&bytes) != expected {
                return Err(GadgetError::new(
                    ErrorKind::InvalidGadget,
                    "gadget changed while loading; retry to rescan and recheck approval",
                ));
            }
            Component::new(&engine, bytes).map_err(wasm_error)
        })
        .await
        .map_err(io_error)??;
        let mut cache = self.components.lock().unwrap();
        if cache.len() >= 16 {
            cache.clear();
        }
        cache.insert(hash.clone(), component.clone());
        Ok(component)
    }

    fn store(&self, capabilities: Capabilities) -> Store<HostState> {
        let state = HostState::new(
            self.filesystem.clone(),
            capabilities,
            self.options.clone(),
            self.client.clone(),
        );
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(|context| {
            if context.data().budget.expired() {
                Err(wasmtime::Error::new(Trap::Interrupt))
            } else {
                Ok(UpdateDeadline::Yield(1))
            }
        });
        store
    }

    fn cache_description(&self, hash: &str, result: &Result<Description, GadgetError>) {
        let mut cache = self.descriptions.lock().unwrap();
        if cache.len() >= 128 && !cache.contains_key(hash) {
            cache.clear();
        }
        cache.insert(hash.to_owned(), result.clone());
    }

    async fn describe(
        &self,
        bundle: &Bundle,
        component: &Component,
    ) -> Result<Description, GadgetError> {
        let hash = bundle.hash.as_ref().unwrap();
        if let Some(result) = self.descriptions.lock().unwrap().get(hash).cloned() {
            return result;
        }
        let result = self.describe_uncached(component).await;
        self.cache_description(hash, &result);
        result
    }

    async fn catalog_description(&self, bundle: &Bundle) -> Result<Description, GadgetError> {
        let hash = bundle.hash.as_ref().unwrap();
        if let Some(result) = self.descriptions.lock().unwrap().get(hash).cloned() {
            return result;
        }
        let result = match self.component(bundle).await {
            Ok(component) => self.describe(bundle, &component).await,
            Err(error) => Err(error),
        };
        self.cache_description(hash, &result);
        result
    }

    async fn describe_uncached(&self, component: &Component) -> Result<Description, GadgetError> {
        // Metadata discovery is pure: even an approved gadget's describe
        // export gets no read, write or HTTP capabilities.
        let mut store = self.store(Capabilities::default());
        let budget = store.data().budget.clone();
        let text = budget
            .run(async {
                let gadget =
                    bindings::Gadget::instantiate_async(&mut store, component, &self.linker)
                        .await
                        .map_err(wasm_error)?;
                gadget
                    .craftsman_gadget_guest()
                    .call_describe(&mut store)
                    .await
                    .map_err(wasm_error)
            })
            .await
            .map_err(|e| classify(e, store.data()))?;
        if text.len() > MAX_JSON_BYTES {
            return Err(GadgetError::new(
                ErrorKind::OutputLimit,
                "gadget description exceeds 1 MiB",
            ));
        }
        let mut description: Description = serde_json::from_str(&text).map_err(|e| {
            GadgetError::new(
                ErrorKind::InvalidGadget,
                format!("invalid gadget description: {e}"),
            )
        })?;
        let mut names = BTreeSet::new();
        if description.functions.len() > 128 {
            return Err(GadgetError::new(
                ErrorKind::InvalidGadget,
                "at most 128 functions per gadget",
            ));
        }
        for function in &description.functions {
            if !craftsman_gadget::valid_function_name(&function.name)
                || !names.insert(&function.name)
                || function.description.trim().is_empty()
                || function.description.len() > 16 * 1024
                || !function.input_schema.is_object()
                || jsonschema::options()
                    .with_pattern_options(
                        jsonschema::PatternOptions::fancy_regex().backtrack_limit(10_000),
                    )
                    .build(&function.input_schema)
                    .is_err()
            {
                return Err(GadgetError::new(
                    ErrorKind::InvalidGadget,
                    "invalid/duplicate function name, description or JSON schema",
                ));
            }
        }
        description
            .functions
            .sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(description)
    }

    pub async fn list(&self) -> Result<Vec<GadgetInfo>, GadgetError> {
        self.list_scanned(self.discover().await?).await
    }

    pub(super) async fn refresh_catalog(&self) -> Result<Vec<GadgetInfo>, GadgetError> {
        let scanned = self.discover().await?;
        let mut identities = BTreeSet::new();
        for gadget in &scanned {
            if let Some(bundle) = &gadget.bundle {
                identities.insert(catalog_identity(bundle));
            }
        }
        self.catalog_seen
            .lock()
            .unwrap()
            .retain(|key| identities.contains(key));
        if self.options.interactive {
            for gadget in &scanned {
                let Some(bundle) = &gadget.bundle else {
                    continue;
                };
                if bundle.hash.is_none() {
                    continue;
                }
                let first_seen = self
                    .catalog_seen
                    .lock()
                    .unwrap()
                    .insert(catalog_identity(bundle));
                if first_seen {
                    // Rejection is data, not a turn failure. Do not re-prompt
                    // every round for an unchanged, declined component.
                    if let Err(error) = self.approved(bundle, true).await {
                        self.last_errors
                            .lock()
                            .unwrap()
                            .insert(bundle.manifest.id.clone(), error.to_string());
                    }
                }
            }
        }
        self.list_scanned(scanned).await
    }

    async fn list_scanned(&self, scanned: Vec<Scanned>) -> Result<Vec<GadgetInfo>, GadgetError> {
        let mut result = Vec::new();
        for mut scanned in scanned {
            if let Some(bundle) = &scanned.bundle {
                match self.approved(bundle, false).await {
                    Ok(false) => scanned.info.status = "needs-approval".into(),
                    Err(error) => {
                        scanned.info.status = "needs-approval".into();
                        scanned.info.error = Some(error.to_string());
                    }
                    Ok(true) if bundle.hash.is_some() => {
                        match self.catalog_description(bundle).await {
                            Ok(description) => scanned.info.functions = description.functions,
                            Err(error) => {
                                scanned.info.status = "broken".into();
                                scanned.info.error = Some(error.to_string());
                            }
                        }
                    }
                    Ok(true) => {}
                }
            }
            scanned.info.last_error = self
                .last_errors
                .lock()
                .unwrap()
                .get(&scanned.info.id)
                .cloned();
            result.push(scanned.info);
        }
        Ok(result)
    }

    pub async fn call(
        &self,
        id: &str,
        function: &str,
        args: serde_json::Value,
    ) -> Result<CallResult, GadgetError> {
        let result = self.call_inner(id, function, args).await;
        let mut errors = self.last_errors.lock().unwrap();
        if errors.len() >= 128 {
            errors.clear();
        }
        if let Err(error) = &result {
            errors.insert(id.to_owned(), error.to_string());
        } else {
            errors.remove(id);
        }
        result
    }

    async fn call_inner(
        &self,
        id: &str,
        function: &str,
        args: serde_json::Value,
    ) -> Result<CallResult, GadgetError> {
        let started = Instant::now();
        if !craftsman_gadget::valid_function_name(function) {
            return Err(GadgetError::new(
                ErrorKind::InvalidArgs,
                "invalid gadget function name",
            ));
        }
        let json = craftsman_gadget::__private::encode(Ok(&args)).map_err(|_| {
            GadgetError::new(ErrorKind::InvalidArgs, "gadget arguments exceed 1 MiB")
        })?;
        if json.len() > MAX_JSON_BYTES {
            return Err(GadgetError::new(
                ErrorKind::InvalidArgs,
                "gadget arguments exceed 1 MiB",
            ));
        }
        let bundle = self.find(id).await?;
        if bundle.hash.is_none() {
            return Err(GadgetError::new(
                ErrorKind::NotFound,
                "data-only gadget has no functions",
            ));
        }
        if !self.approved(&bundle, true).await? {
            return Err(GadgetError::new(
                ErrorKind::NeedsApproval,
                "approve this gadget on first load, or name it with --allow-gadget in headless runs",
            ));
        }
        let component = self.component(&bundle).await?;
        let description = self.describe(&bundle, &component).await?;
        let definition = description
            .functions
            .iter()
            .find(|f| f.name == function)
            .ok_or_else(|| {
                GadgetError::new(
                    ErrorKind::NotFound,
                    format!("gadget function not found: {function}"),
                )
            })?;
        let validator = jsonschema::options()
            .with_pattern_options(jsonschema::PatternOptions::fancy_regex().backtrack_limit(10_000))
            .build(&definition.input_schema)
            .map_err(io_error)?;
        if let Err(error) = validator.validate(&args) {
            return Err(GadgetError::new(ErrorKind::InvalidArgs, error.to_string()));
        }
        let mut store = self.store(bundle.manifest.capabilities);
        let budget = store.data().budget.clone();
        let returned = budget
            .run(async {
                let gadget =
                    bindings::Gadget::instantiate_async(&mut store, &component, &self.linker)
                        .await
                        .map_err(wasm_error)?;
                gadget
                    .craftsman_gadget_guest()
                    .call_call(&mut store, function, &json)
                    .await
                    .map_err(wasm_error)
            })
            .await
            .map_err(|error| classify(error, store.data()))?;
        use bindings::exports::craftsman::gadget::guest::CallError;
        let json = returned.map_err(|error| {
            let error = match error {
                CallError::UnknownFunction(message) => {
                    GadgetError::new(ErrorKind::NotFound, message)
                }
                CallError::InvalidArgs(message) => {
                    GadgetError::new(ErrorKind::InvalidArgs, message)
                }
                CallError::Failed(message) => GadgetError::new(
                    store.data().last_failure.unwrap_or(
                        if message.contains("JSON output exceeds 1 MiB") {
                            ErrorKind::OutputLimit
                        } else {
                            ErrorKind::Failed
                        },
                    ),
                    message,
                ),
            };
            classify(error, store.data())
        })?;
        if json.len() > MAX_JSON_BYTES {
            return Err(GadgetError::new(
                ErrorKind::OutputLimit,
                "gadget result exceeds 1 MiB",
            ));
        }
        let value = serde_json::from_str::<serde_json::Value>(&json).map_err(|e| {
            classify(
                GadgetError::new(
                    ErrorKind::InvalidGadget,
                    format!("gadget returned invalid JSON: {e}"),
                ),
                store.data(),
            )
        })?;
        // Preserve JSON semantics while making even headless activity previews
        // safe: C1 controls may otherwise be emitted literally by serde_json.
        let json = super::cli::terminal_safe(&serde_json::to_string(&value).map_err(io_error)?);
        if json.len() > MAX_JSON_BYTES {
            return Err(classify(
                GadgetError::new(
                    ErrorKind::OutputLimit,
                    "escaped gadget result exceeds 1 MiB",
                ),
                store.data(),
            ));
        }
        let (logs, logs_truncated) = store.data().capture.contents();
        Ok(CallResult {
            json,
            duration: started.elapsed(),
            network_calls: store.data().network_calls,
            logs,
            logs_truncated,
        })
    }

    #[cfg(test)]
    pub(super) fn compiled_count(&self) -> usize {
        self.components.lock().unwrap().len()
    }

    /// Check the component and its WIT exports/imports without executing it.
    /// Authoring builds must not call describe before first-load approval.
    pub(super) fn validate_component(&self, bytes: &[u8]) -> Result<(), GadgetError> {
        let component = Component::new(&self.engine, bytes).map_err(wasm_error)?;
        let pre = self
            .linker
            .instantiate_pre(&component)
            .map_err(wasm_error)?;
        bindings::GadgetPre::new(pre).map_err(wasm_error)?;
        Ok(())
    }

    pub fn tools(self: &Arc<Self>) -> Vec<Arc<dyn craftsman_harness::Tool>> {
        super::tools::all(self.clone())
    }
}

fn classify(mut error: GadgetError, state: &HostState) -> GadgetError {
    error.network_calls = state.network_calls;
    error.logs_truncated = state.capture.contents().1;
    if state.limits.memory_denied {
        error.kind = ErrorKind::OutOfMemory;
    } else if state.budget.expired() {
        error.kind = ErrorKind::Timeout;
    }
    error
}
fn io_error(error: impl std::fmt::Display) -> GadgetError {
    GadgetError::new(ErrorKind::Io, error.to_string())
}
fn wasm_error(error: wasmtime::Error) -> GadgetError {
    let kind = if error.downcast_ref::<Trap>() == Some(&Trap::Interrupt) {
        ErrorKind::Timeout
    } else {
        ErrorKind::Trap
    };
    GadgetError::new(kind, format!("{error:#}"))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_bounded(directory: &Path, name: &str, maximum: usize) -> Result<Vec<u8>, GadgetError> {
    let directory = cap_std::fs::Dir::open_ambient_dir(directory, cap_std::ambient_authority())
        .map_err(io_error)?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = directory.open_with(name, &options).map_err(io_error)?;
    if !file.metadata().map_err(io_error)?.is_file() {
        return Err(GadgetError::new(
            ErrorKind::InvalidGadget,
            "gadget inputs must be regular files, not symlinks/devices",
        ));
    }
    let mut bytes = Vec::new();
    file.take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > maximum {
        return Err(GadgetError::new(
            ErrorKind::InvalidGadget,
            format!("{name} exceeds its size limit"),
        ));
    }
    Ok(bytes)
}

fn scan(
    directory: &Path,
    id: &str,
    project: bool,
    scans: &Mutex<HashMap<PathBuf, CachedScan>>,
) -> Scanned {
    match scan_inner(directory, id, project, scans) {
        Ok(bundle) => {
            let info = GadgetInfo {
                id: bundle.manifest.id.clone(),
                version: bundle.manifest.version.clone(),
                description: bundle.manifest.description.clone(),
                source: bundle.source.clone(),
                project,
                status: "ready".into(),
                component_hash: bundle.hash.clone(),
                manifest_hash: Some(bundle.manifest_hash.clone()),
                commands: bundle
                    .manifest
                    .commands
                    .iter()
                    .map(|c| c.name.clone())
                    .collect(),
                agents: bundle
                    .manifest
                    .agents
                    .iter()
                    .map(|a| a.name.clone())
                    .collect(),
                has_prompt: bundle
                    .manifest
                    .prompt
                    .as_deref()
                    .is_some_and(|p| !p.trim().is_empty()),
                capabilities: bundle.manifest.capabilities.clone(),
                functions: vec![],
                error: None,
                last_error: None,
            };
            Scanned {
                info,
                bundle: Some(bundle),
            }
        }
        Err(error) => broken(id, directory, project, error.to_string()),
    }
}
fn scan_inner(
    directory: &Path,
    id: &str,
    project: bool,
    scans: &Mutex<HashMap<PathBuf, CachedScan>>,
) -> Result<Bundle, GadgetError> {
    let kind = fs::symlink_metadata(directory)
        .map_err(io_error)?
        .file_type();
    if !kind.is_dir() || kind.is_symlink() {
        return Err(GadgetError::new(
            ErrorKind::InvalidGadget,
            "gadget directories must not be symlinks",
        ));
    }
    let source = directory.canonicalize().map_err(io_error)?;
    let manifest = fs::symlink_metadata(source.join("gadget.toml")).map_err(io_error)?;
    if !manifest.is_file()
        || manifest.file_type().is_symlink()
        || manifest.len() > MAX_MANIFEST_BYTES as u64
    {
        return Err(GadgetError::new(
            ErrorKind::InvalidGadget,
            "manifest must be a regular file of at most 256 KiB",
        ));
    }
    let manifest = Stamp::new(&manifest);
    let component = match fs::symlink_metadata(source.join("gadget.wasm")) {
        Ok(m)
            if m.is_file()
                && !m.file_type().is_symlink()
                && m.len() <= MAX_COMPONENT_BYTES as u64 =>
        {
            Some(Stamp::new(&m))
        }
        Ok(_) => {
            return Err(GadgetError::new(
                ErrorKind::InvalidGadget,
                "component must be a regular file of at most 32 MiB",
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_error(e)),
    };
    if let Some(cached) = scans.lock().unwrap().get(directory).filter(|c| {
        c.manifest == manifest
            && c.component == component
            && c.bundle.source == source
            && c.bundle.project == project
    }) {
        return Ok(cached.bundle.clone());
    }
    let bytes = read_bounded(&source, "gadget.toml", MAX_MANIFEST_BYTES)?;
    let text = std::str::from_utf8(&bytes).map_err(io_error)?;
    let parsed = Manifest::parse(text, id)?;
    let manifest_hash = digest(&bytes);
    let hash = if component.is_some() {
        Some(digest(&read_bounded(
            &source,
            "gadget.wasm",
            MAX_COMPONENT_BYTES,
        )?))
    } else {
        None
    };
    let bundle = Bundle {
        manifest: parsed,
        manifest_hash,
        source,
        project,
        hash,
    };
    scans.lock().unwrap().insert(
        directory.to_path_buf(),
        CachedScan {
            manifest,
            component,
            bundle: bundle.clone(),
        },
    );
    Ok(bundle)
}
fn broken(id: &str, source: &Path, project: bool, error: String) -> Scanned {
    Scanned {
        bundle: None,
        info: GadgetInfo {
            id: id.into(),
            version: String::new(),
            description: String::new(),
            source: source.into(),
            project,
            status: "broken".into(),
            component_hash: None,
            manifest_hash: None,
            commands: vec![],
            agents: vec![],
            has_prompt: false,
            capabilities: Capabilities::default(),
            functions: vec![],
            error: Some(error),
            last_error: None,
        },
    }
}

fn catalog_identity(bundle: &Bundle) -> String {
    digest(
        &serde_json::to_vec(&(
            &bundle.manifest.id,
            &bundle.source,
            &bundle.hash,
            &bundle.manifest.capabilities,
        ))
        .expect("gadget identity is serializable"),
    )
}

#[cfg(unix)]
pub(super) fn trusted_cache_directory(config: &Path, workspace: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let config = config.canonicalize().ok()?;
    let workspace = workspace.canonicalize().ok()?;
    if config.starts_with(&workspace) {
        return None;
    }
    // SAFETY: geteuid has no pointer arguments or caller preconditions.
    let uid = unsafe { libc::geteuid() };
    let metadata = fs::symlink_metadata(&config).ok()?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.permissions().mode() & 0o022 != 0 {
        return None;
    }
    let mut path = config;
    for name in ["cache", "gadgets"] {
        path.push(name);
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
        let metadata = fs::symlink_metadata(&path).ok()?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != uid
            || metadata.permissions().mode() & 0o077 != 0
        {
            return None;
        }
    }
    Some(path)
}

#[cfg(not(unix))]
fn trusted_cache_directory(_: &Path, _: &Path) -> Option<PathBuf> {
    None
}
