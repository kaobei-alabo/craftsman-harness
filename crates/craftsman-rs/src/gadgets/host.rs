use super::manifest::Capabilities;
use super::runtime::{CallBudget, RuntimeOptions};
use super::{ErrorKind, GadgetError};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::AsyncWrite;
use wasmtime::{ResourceLimiter, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "../craftsman-gadget/wit", world: "gadget",
        imports: { default: async }, exports: { default: async },
    });
}
use bindings::craftsman::gadget::host::{DirEntry, Host, Level, Request, Response};

pub const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
const MAX_HTTP_BYTES: usize = 10 * 1024 * 1024;

#[derive(Default)]
struct LogBuffer {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Clone, Default)]
pub struct Capture(Arc<Mutex<LogBuffer>>);

impl Capture {
    pub(super) fn append(&self, bytes: &[u8]) {
        let mut buffer = self.0.lock().unwrap();
        let take = bytes
            .len()
            .min(MAX_LOG_BYTES.saturating_sub(buffer.bytes.len()));
        buffer.bytes.extend_from_slice(&bytes[..take]);
        buffer.truncated |= take < bytes.len();
    }
    pub fn contents(&self) -> (String, bool) {
        let buffer = self.0.lock().unwrap();
        (
            String::from_utf8_lossy(&buffer.bytes).into_owned(),
            buffer.truncated,
        )
    }
}
impl AsyncWrite for Capture {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.append(bytes);
        // Discard excess bytes but keep draining. A noisy guest cannot block
        // forever on a full pipe, nor print untrusted control text directly.
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl IsTerminal for Capture {
    fn is_terminal(&self) -> bool {
        false
    }
}
impl StdoutStream for Capture {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

pub struct Limits {
    inner: StoreLimits,
    pub memory_denied: bool,
}
impl Limits {
    pub(super) fn new(memory_bytes: usize) -> Self {
        Self {
            inner: StoreLimitsBuilder::new()
                .memory_size(memory_bytes)
                .memories(1)
                .table_elements(10_000)
                .tables(4)
                .instances(64)
                .trap_on_grow_failure(true)
                .build(),
            memory_denied: false,
        }
    }
}
impl ResourceLimiter for Limits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let result = self.inner.memory_growing(current, desired, maximum);
        if !matches!(result, Ok(true)) {
            self.memory_denied = true;
        }
        result
    }
    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.inner.table_growing(current, desired, maximum)
    }
    fn instances(&self) -> usize {
        self.inner.instances()
    }
    fn tables(&self) -> usize {
        self.inner.tables()
    }
    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

pub struct Workspace {
    root: PathBuf,
    config: PathBuf,
    directory: Dir,
}
impl Workspace {
    pub fn new(root: &Path, config: &Path) -> Result<Self, GadgetError> {
        let root = root
            .canonicalize()
            .map_err(|e| GadgetError::new(ErrorKind::Io, e.to_string()))?;
        let directory = Dir::open_ambient_dir(&root, cap_std::ambient_authority())
            .map_err(|e| GadgetError::new(ErrorKind::Io, e.to_string()))?;
        let config = config
            .canonicalize()
            .unwrap_or_else(|_| config.to_path_buf());
        Ok(Self {
            root,
            config,
            directory,
        })
    }

    pub fn relative(&self, raw: &str, write: bool) -> Result<PathBuf, GadgetError> {
        let path = Path::new(raw);
        let path = if path.is_absolute() {
            path.strip_prefix(&self.root)
                .map_err(|_| denied("absolute path is outside the workspace"))?
        } else {
            path
        };
        let mut relative = PathBuf::new();
        for part in path.components() {
            match part {
                Component::Normal(name) => {
                    let text = name.to_string_lossy().to_ascii_lowercase();
                    if text.contains(['\0', ':', '\\']) || text.ends_with(['.', ' ']) {
                        return Err(denied("ambiguous or invalid path component"));
                    }
                    let stem = text.split('.').next().unwrap_or("");
                    if matches!(stem, "con" | "prn" | "aux" | "nul" | "clock$")
                        || (stem.len() == 4
                            && (stem.starts_with("com") || stem.starts_with("lpt"))
                            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
                    {
                        return Err(denied("Windows device paths are not workspace files"));
                    }
                    if write
                        && (matches!(
                            text.as_str(),
                            ".git"
                                | ".craftsman"
                                | ".github"
                                | ".gitlab"
                                | ".gitlab-ci.yml"
                                | "jenkinsfile"
                                | "azure-pipelines.yml"
                        ) || text.starts_with(".env"))
                    {
                        return Err(denied("protected workspace path"));
                    }
                    relative.push(name);
                }
                Component::CurDir => {}
                _ => return Err(denied("parent traversal and rooted paths are not allowed")),
            }
        }
        if write
            && (relative.as_os_str().is_empty()
                || self.root.join(&relative).starts_with(&self.config))
        {
            return Err(denied(
                "gadget config/grants and the workspace root are protected",
            ));
        }
        Ok(relative)
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>, GadgetError> {
        // Capability-relative resolution confines symlinks to the root. The
        // nonblocking open also prevents a swapped-in FIFO from hanging a
        // blocking worker before we inspect the actual opened handle.
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = self
            .directory
            .open_with(path, &options)
            .map_err(|e| denied(format!("scoped file open: {e}")))?;
        if !file.metadata().map_err(io_error)?.is_file() {
            return Err(denied("only regular files may be read"));
        }
        let mut bytes = Vec::new();
        file.take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(GadgetError::new(
                ErrorKind::OutputLimit,
                "file exceeds 10 MiB",
            ));
        }
        Ok(bytes)
    }

    fn list(&self, path: &Path) -> Result<Vec<DirEntry>, GadgetError> {
        let directory = self
            .directory
            .open_dir(if path.as_os_str().is_empty() {
                Path::new(".")
            } else {
                path
            })
            .map_err(|e| denied(format!("scoped directory open: {e}")))?;
        let mut entries = Vec::new();
        for item in directory.entries().map_err(io_error)? {
            if entries.len() >= 1024 {
                return Err(GadgetError::new(
                    ErrorKind::OutputLimit,
                    "directory exceeds 1024 entries",
                ));
            }
            let item = item.map_err(io_error)?;
            let kind = item.file_type().map_err(io_error)?;
            // Do not follow a directory entry's symlink just to report size.
            let size = if kind.is_file() {
                item.metadata().map_err(io_error)?.len()
            } else {
                0
            };
            entries.push(DirEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                is_dir: kind.is_dir(),
                size,
            });
        }
        entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> Result<(), GadgetError> {
        // Match write_file: create only, never truncate existing files or hard
        // links. Walk one directory component at a time without following
        // symlinks, so symlink aliases cannot reach .git or
        // other protected paths after validation.
        let mut directory = self.directory.try_clone().map_err(io_error)?;
        if let Some(parent) = path.parent() {
            for part in parent.components() {
                let Component::Normal(name) = part else {
                    continue;
                };
                match directory.open_dir_nofollow(name) {
                    Ok(next) => directory = next,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        directory.create_dir(name).map_err(io_error)?;
                        directory = directory
                            .open_dir_nofollow(name)
                            .map_err(|e| denied(e.to_string()))?;
                    }
                    Err(e) => {
                        return Err(denied(format!("write parents must not be symlinks: {e}")));
                    }
                }
            }
        }
        let name = path
            .file_name()
            .ok_or_else(|| denied("missing file name"))?;
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        let mut file = directory.open_with(name, &options).map_err(io_error)?;
        file.write_all(contents).map_err(io_error)
    }
}

fn denied(message: impl Into<String>) -> GadgetError {
    GadgetError::new(ErrorKind::DeniedCapability, message)
}
fn io_error(error: impl std::fmt::Display) -> GadgetError {
    GadgetError::new(ErrorKind::Io, error.to_string())
}

pub struct HostState {
    wasi: WasiCtx,
    table: wasmtime::component::ResourceTable,
    pub limits: Limits,
    pub capture: Capture,
    workspace: Arc<Workspace>,
    capabilities: Capabilities,
    options: RuntimeOptions,
    client: reqwest::Client,
    pub budget: Arc<CallBudget>,
    pub network_calls: u64,
    pub last_failure: Option<ErrorKind>,
}
impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}
impl HostState {
    pub fn new(
        workspace: Arc<Workspace>,
        capabilities: Capabilities,
        options: RuntimeOptions,
        client: reqwest::Client,
    ) -> Self {
        let capture = Capture::default();
        let mut wasi = WasiCtxBuilder::new();
        wasi.stdout(capture.clone())
            .stderr(capture.clone())
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false);
        // No inherit_env, inherit_args, inherit_stdio, socket addresses or
        // preopened directories. Host functions are the only effect surface.
        Self {
            wasi: wasi.build(),
            table: wasmtime::component::ResourceTable::new(),
            limits: Limits::new(options.memory_bytes),
            capture,
            workspace,
            capabilities,
            budget: Arc::new(CallBudget::new(options.timeout)),
            options,
            client,
            network_calls: 0,
            last_failure: None,
        }
    }
    fn result<T>(&mut self, result: Result<T, GadgetError>) -> Result<T, String> {
        result.map_err(|e| {
            self.last_failure = Some(e.kind);
            e.to_string()
        })
    }
}

impl Host for HostState {
    async fn log(&mut self, level: Level, message: String) {
        self.capture.append(format!("[{level:?}] ").as_bytes());
        self.capture.append(message.as_bytes());
        self.capture.append(b"\n");
    }

    async fn read_file(&mut self, path: String) -> Result<Vec<u8>, String> {
        let result = async {
            if !self.capabilities.read.contains("workspace") {
                return Err(denied("workspace read was not granted"));
            }
            let path = self.workspace.relative(&path, false)?;
            let workspace = self.workspace.clone();
            bounded_blocking(self.budget.remaining(), move || workspace.read(&path)).await
        }
        .await;
        self.result(result)
    }

    async fn list_dir(&mut self, path: String) -> Result<Vec<DirEntry>, String> {
        let result = async {
            if !self.capabilities.read.contains("workspace") {
                return Err(denied("workspace read was not granted"));
            }
            let path = self.workspace.relative(&path, false)?;
            let workspace = self.workspace.clone();
            bounded_blocking(self.budget.remaining(), move || workspace.list(&path)).await
        }
        .await;
        self.result(result)
    }

    async fn write_file(&mut self, path: String, contents: Vec<u8>) -> Result<(), String> {
        let result = async {
            if !self.capabilities.write.contains("workspace") { return Err(denied("workspace write was not granted")); }
            if contents.len() > craftsman_gadget::MAX_JSON_BYTES { return Err(GadgetError::new(ErrorKind::OutputLimit, "write exceeds 1 MiB")); }
            let relative = self.workspace.relative(&path, true)?;
            let args = serde_json::json!({"file_path": path, "content": String::from_utf8_lossy(&contents)});
            let permission = self.options.approval.lock().unwrap().permission("write_file", &args);
            match permission {
                crate::hitl::ToolPermission::Allow => {},
                crate::hitl::ToolPermission::Deny(reason) => return Err(denied(reason)),
                crate::hitl::ToolPermission::Prompt => {
                    let Some(prompt) = self.options.prompt.clone().filter(|_| self.options.interactive) else {
                        return Err(denied("write_file requires approval; headless gadget grants do not approve writes"));
                    };
                    let summary = crate::tools::summarize_call("write_file", &args);
                    let details = crate::tools::approval_details_at("write_file", &args, &self.workspace.root);
                    let pause = self.budget.pause();
                    let decision = tokio::task::spawn_blocking(move || prompt("write_file", &summary, &details))
                        .await.map_err(io_error)?;
                    drop(pause);
                    match decision {
                        crate::agent::ApprovalPromptResult::Approve => {},
                        crate::agent::ApprovalPromptResult::ApproveAlways => self.options.approval.lock().unwrap().auto_approve = true,
                        crate::agent::ApprovalPromptResult::Reject(reason) => return Err(denied(reason)),
                    }
                },
            }
            let checkpoints = self.options.workspace_checkpoints.clone();
            let ticket = if let Some(checkpoints) = &checkpoints {
                tokio::time::timeout(self.budget.remaining(), checkpoints.begin_file_mutation(
                    &relative.to_string_lossy(), format!("gadget write: {path}")
                )).await.map_err(|_| GadgetError::new(ErrorKind::Timeout, "gadget I/O timed out"))?
            } else { None };
            let workspace = self.workspace.clone();
            let result = bounded_blocking(self.budget.remaining(), move || workspace.write(&relative, &contents)).await;
            if let Some(checkpoints) = &checkpoints {
                let _ = tokio::time::timeout(self.budget.remaining(), checkpoints.finish_mutation(ticket)).await;
            }
            result
        }.await;
        self.result(result)
    }

    async fn http_request(&mut self, request: Request) -> Result<Response, String> {
        let result = async {
            if request.url.len() > 16 * 1024 || request.method.len() > 32
                || request.headers.iter().map(|(k,v)| k.len().saturating_add(v.len())).sum::<usize>() > 64 * 1024
            { return Err(GadgetError::new(ErrorKind::OutputLimit, "HTTP routing inputs exceed their bounds")); }
            let url = url::Url::parse(&request.url).map_err(|_| denied("invalid HTTPS URL"))?;
            if url.scheme() != "https" || url.port_or_known_default() != Some(443)
                || !url.username().is_empty() || url.password().is_some()
                || !url.host_str().is_some_and(|h| self.capabilities.http.contains(h))
            { return Err(denied("HTTPS host was not granted (URLs with credentials or non-443 ports are denied)")); }
            if request.headers.len() > 64 || request.body.as_ref().is_some_and(|b| b.len() > craftsman_gadget::MAX_JSON_BYTES) {
                return Err(GadgetError::new(ErrorKind::OutputLimit, "HTTP request exceeds its bounds"));
            }
            let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(io_error)?;
            let mut builder = self.client.request(method, url).timeout(self.budget.remaining().min(Duration::from_secs(30)));
            for (name, value) in request.headers {
                if matches!(name.to_ascii_lowercase().as_str(), "host" | "connection" | "proxy-authorization" | "transfer-encoding" | "content-length") {
                    return Err(denied("HTTP routing/proxy headers may not be overridden"));
                }
                let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(io_error)?;
                let value = reqwest::header::HeaderValue::from_str(&value).map_err(io_error)?;
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body { builder = builder.body(body); }
            if self.network_calls >= 64 { return Err(GadgetError::new(ErrorKind::OutputLimit, "at most 64 HTTP requests per call")); }
            self.network_calls += 1;
            let mut response = builder.send().await.map_err(|e| GadgetError::new(if e.is_timeout() { ErrorKind::Timeout } else { ErrorKind::Failed }, e.to_string()))?;
            let status = response.status().as_u16();
            let headers = response.headers().iter().map(|(k,v)| (k.to_string(), v.to_str().unwrap_or("").to_owned())).collect();
            if response.content_length().is_some_and(|n| n > MAX_HTTP_BYTES as u64) { return Err(GadgetError::new(ErrorKind::OutputLimit, "HTTP response exceeds 10 MiB")); }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(io_error)? {
                if chunk.len() > MAX_HTTP_BYTES.saturating_sub(body.len()) { return Err(GadgetError::new(ErrorKind::OutputLimit, "HTTP response exceeds 10 MiB")); }
                body.extend_from_slice(&chunk);
            }
            Ok(Response { status, headers, body })
        }.await;
        self.result(result)
    }
}

async fn bounded_blocking<T: Send + 'static>(
    duration: Duration,
    work: impl FnOnce() -> Result<T, GadgetError> + Send + 'static,
) -> Result<T, GadgetError> {
    tokio::time::timeout(duration, tokio::task::spawn_blocking(work))
        .await
        .map_err(|_| GadgetError::new(ErrorKind::Timeout, "gadget I/O timed out"))?
        .map_err(io_error)?
}
