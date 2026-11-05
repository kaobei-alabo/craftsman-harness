//! Lightweight stdio Language Server Protocol client and agent tools.
//!
//! Servers are configured in `~/.craftsman/config.json` and started lazily for a
//! matching file extension. Craftsman does not install language servers.

use async_trait::async_trait;
use craftsman_harness::{AgentError, Tool, ToolCtx, ToolDef, ToolOutcome};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc, oneshot};
use url::Url;

use crate::config::{Config, LspServerConfig};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DIAGNOSTIC_WAIT: Duration = Duration::from_millis(500);
const STDERR_TAIL_BYTES: usize = 8 * 1024;

type PendingResponse = oneshot::Sender<Result<Value, String>>;
type PendingRequests = Arc<Mutex<HashMap<u64, PendingResponse>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerStatus {
    pub name: String,
    pub command: String,
    pub extensions: Vec<String>,
    pub state: String,
}

pub struct LspManager {
    root: PathBuf,
    root_canonical: PathBuf,
    servers: BTreeMap<String, LspServerConfig>,
    clients: AsyncMutex<HashMap<String, Arc<LspClient>>>,
    last_errors: Mutex<HashMap<String, String>>,
    generations: Mutex<HashMap<String, u64>>,
    shutting_down: AtomicBool,
    lifecycle: AsyncMutex<()>,
}

impl LspManager {
    pub fn new(root: PathBuf, config: &Config) -> Self {
        let root_canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        Self {
            root,
            root_canonical,
            servers: config.effective_lsp_servers(),
            clients: AsyncMutex::new(HashMap::new()),
            last_errors: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
            lifecycle: AsyncMutex::new(()),
        }
    }

    /// Explicitly disabled manager for the private headless profile. In contrast
    /// to `new` with empty declarations, this never selects built-in servers or
    /// inspects the workspace. The caller already supplies a canonical scope.
    pub(crate) fn disabled(root: PathBuf) -> Self {
        Self {
            root_canonical: root.clone(),
            root,
            servers: BTreeMap::new(),
            clients: AsyncMutex::new(HashMap::new()),
            last_errors: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(true),
            lifecycle: AsyncMutex::new(()),
        }
    }

    pub fn tools(self: &Arc<Self>) -> Vec<Arc<dyn Tool>> {
        [
            LspToolKind::Diagnostics,
            LspToolKind::Definition,
            LspToolKind::References,
            LspToolKind::Hover,
            LspToolKind::DocumentSymbols,
            LspToolKind::WorkspaceSymbols,
        ]
        .into_iter()
        .map(|kind| {
            Arc::new(LspTool {
                kind,
                manager: self.clone(),
            }) as Arc<dyn Tool>
        })
        .collect()
    }

    pub async fn statuses(&self) -> Vec<LspServerStatus> {
        let clients = self.clients.lock().await;
        let errors = self.last_errors.lock().unwrap();
        self.servers
            .iter()
            .map(|(name, server)| {
                let state = if clients.contains_key(name) {
                    "ready".to_string()
                } else if let Some(error) = errors.get(name) {
                    format!("error: {error}")
                } else {
                    match server_preflight(name, server) {
                        Ok(()) => "available (lazy)".to_string(),
                        Err(error) => format!("unavailable: {error}"),
                    }
                };
                LspServerStatus {
                    name: name.clone(),
                    command: server.command.clone(),
                    extensions: server.extensions.clone(),
                    state,
                }
            })
            .collect()
    }

    pub async fn restart(&self, name: &str) -> Result<String, String> {
        if !self.servers.contains_key(name) {
            return Err(format!("unknown LSP server '{name}'"));
        }
        let _lifecycle = self.lifecycle.lock().await;
        {
            let mut generations = self.generations.lock().unwrap();
            *generations.entry(name.to_string()).or_default() += 1;
        }
        let client = self.clients.lock().await.remove(name);
        self.last_errors.lock().unwrap().remove(name);
        if let Some(client) = client {
            client.stop().await;
            Ok(format!(
                "LSP server '{name}' stopped; it will restart on the next request"
            ))
        } else {
            Ok(format!(
                "LSP server '{name}' is reset and will start on demand"
            ))
        }
    }

    pub async fn shutdown(&self) {
        let _lifecycle = self.lifecycle.lock().await;
        self.shutting_down.store(true, Ordering::SeqCst);
        let clients = self
            .clients
            .lock()
            .await
            .drain()
            .map(|(_, client)| client)
            .collect::<Vec<_>>();
        for client in clients {
            client.stop().await;
        }
    }

    pub async fn diagnostics(&self, file_path: &str) -> Result<String, String> {
        let (server, client, path, uri, language_id) = self.prepare(file_path).await?;
        client.sync_document(&path, &uri, &language_id).await?;

        let pull = client
            .request(
                "textDocument/diagnostic",
                json!({"textDocument": {"uri": uri}}),
            )
            .await;
        let items = match pull {
            Ok(value) => value
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            Err(error) if diagnostic_pull_is_unsupported(&error) => {
                let _ = tokio::time::timeout(DIAGNOSTIC_WAIT, client.diagnostic_notify.notified())
                    .await;
                client.cached_diagnostics(&uri)
            }
            Err(error) => return Err(error),
        };
        Ok(format_diagnostics(&server, &path, &items, &self.root))
    }

    pub async fn definition(
        &self,
        file_path: &str,
        line: u64,
        column: u64,
    ) -> Result<String, String> {
        let (_server, client, path, uri, language_id) = self.prepare(file_path).await?;
        client.sync_document(&path, &uri, &language_id).await?;
        let position = client.position(&path, line, column)?;
        let value = client
            .request(
                "textDocument/definition",
                json!({"textDocument": {"uri": uri}, "position": position}),
            )
            .await?;
        Ok(format_locations(&value, &self.root))
    }

    pub async fn references(
        &self,
        file_path: &str,
        line: u64,
        column: u64,
        include_declaration: bool,
    ) -> Result<String, String> {
        let (_server, client, path, uri, language_id) = self.prepare(file_path).await?;
        client.sync_document(&path, &uri, &language_id).await?;
        let position = client.position(&path, line, column)?;
        let value = client
            .request(
                "textDocument/references",
                json!({
                    "textDocument": {"uri": uri},
                    "position": position,
                    "context": {"includeDeclaration": include_declaration}
                }),
            )
            .await?;
        Ok(format_locations(&value, &self.root))
    }

    pub async fn hover(&self, file_path: &str, line: u64, column: u64) -> Result<String, String> {
        let (_server, client, path, uri, language_id) = self.prepare(file_path).await?;
        client.sync_document(&path, &uri, &language_id).await?;
        let position = client.position(&path, line, column)?;
        let value = client
            .request(
                "textDocument/hover",
                json!({"textDocument": {"uri": uri}, "position": position}),
            )
            .await?;
        Ok(format_hover(&value))
    }

    pub async fn document_symbols(&self, file_path: &str) -> Result<String, String> {
        let (_server, client, path, uri, language_id) = self.prepare(file_path).await?;
        client.sync_document(&path, &uri, &language_id).await?;
        let value = client
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": uri}}),
            )
            .await?;
        Ok(format_symbols(&value, &self.root))
    }

    pub async fn workspace_symbols(&self, file_path: &str, query: &str) -> Result<String, String> {
        let (_server, client, _path, _uri, _language_id) = self.prepare(file_path).await?;
        let value = client
            .request("workspace/symbol", json!({"query": query}))
            .await?;
        Ok(format_symbols(&value, &self.root))
    }

    async fn prepare(
        &self,
        file_path: &str,
    ) -> Result<(String, Arc<LspClient>, PathBuf, String, String), String> {
        let path = self.resolve_file(file_path)?;
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| format!("cannot choose an LSP server for {file_path}"))?;
        let (name, config) = self
            .servers
            .iter()
            .find(|(_, server)| server.extensions.iter().any(|item| item == &extension))
            .map(|(name, server)| (name.clone(), server.clone()))
            .ok_or_else(|| format!("no LSP server is configured for .{extension} files"))?;
        let client = self.client(&name, &config).await?;
        let uri = Url::from_file_path(&path)
            .map_err(|_| format!("could not convert {} to a file URI", path.display()))?
            .to_string();
        let language_id = config
            .language_id
            .clone()
            .unwrap_or_else(|| language_id_for_extension(&extension).to_string());
        Ok((name, client, path, uri, language_id))
    }

    fn resolve_file(&self, file_path: &str) -> Result<PathBuf, String> {
        let requested = PathBuf::from(file_path);
        let path = if requested.is_absolute() {
            requested
        } else {
            self.root.join(requested)
        };
        if !path.is_file() {
            return Err(format!("file does not exist: {}", path.display()));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("could not resolve {}: {error}", path.display()))?;
        if !canonical.starts_with(&self.root_canonical) {
            return Err(format!(
                "LSP file must be inside the workspace: {}",
                canonical.display()
            ));
        }
        Ok(canonical)
    }

    async fn client(&self, name: &str, config: &LspServerConfig) -> Result<Arc<LspClient>, String> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err("LSP manager is shutting down".to_string());
        }
        if let Some(client) = self.clients.lock().await.get(name).cloned() {
            return Ok(client);
        }
        if let Err(error) = server_preflight(name, config) {
            self.last_errors
                .lock()
                .unwrap()
                .insert(name.to_string(), error.clone());
            return Err(error);
        }
        let generation = *self.generations.lock().unwrap().get(name).unwrap_or(&0);
        let started = match LspClient::start(name, config, &self.root).await {
            Ok(client) => client,
            Err(error) => {
                self.last_errors
                    .lock()
                    .unwrap()
                    .insert(name.to_string(), error.clone());
                return Err(error);
            }
        };
        let _lifecycle = self.lifecycle.lock().await;
        let changed = self.shutting_down.load(Ordering::SeqCst)
            || self
                .generations
                .lock()
                .unwrap()
                .get(name)
                .copied()
                .unwrap_or(0)
                != generation;
        if changed {
            started.stop().await;
            return Err(format!("{name} LSP startup was cancelled"));
        }
        let mut clients = self.clients.lock().await;
        if let Some(existing) = clients.get(name).cloned() {
            drop(clients);
            started.stop().await;
            return Ok(existing);
        }
        self.last_errors.lock().unwrap().remove(name);
        clients.insert(name.to_string(), started.clone());
        Ok(started)
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        let Ok(mut clients) = self.clients.try_lock() else {
            return;
        };
        let clients = clients
            .drain()
            .map(|(_, client)| client)
            .collect::<Vec<_>>();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            for client in clients {
                runtime.spawn(async move {
                    client.stop().await;
                });
            }
        } else {
            for client in clients {
                client.start_kill();
            }
        }
    }
}

struct DocumentState {
    version: i64,
    content: String,
}

struct PendingRequestGuard {
    id: u64,
    pending: PendingRequests,
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}

struct LspClient {
    name: String,
    outgoing: mpsc::UnboundedSender<Value>,
    pending: PendingRequests,
    next_id: AtomicU64,
    diagnostics: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    diagnostic_notify: Arc<Notify>,
    documents: AsyncMutex<HashMap<String, DocumentState>>,
    position_encoding: Mutex<String>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    child: AsyncMutex<Option<Child>>,
}

impl LspClient {
    async fn start(name: &str, config: &LspServerConfig, root: &Path) -> Result<Arc<Self>, String> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not start {}: {error}", config.command))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("{} did not provide stdin", config.command))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("{} did not provide stdout", config.command))?;
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        if let Some(mut stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];
                while let Ok(count) = stderr.read(&mut buffer).await {
                    if count == 0 {
                        break;
                    }
                    let mut tail = tail.lock().unwrap();
                    tail.extend_from_slice(&buffer[..count]);
                    if tail.len() > STDERR_TAIL_BYTES {
                        let excess = tail.len() - STDERR_TAIL_BYTES;
                        tail.drain(..excess);
                    }
                }
            });
        }

        let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(message) = outgoing_rx.recv().await {
                let Ok(body) = serde_json::to_vec(&message) else {
                    continue;
                };
                let header = format!("Content-Length: {}\r\n\r\n", body.len());
                if stdin.write_all(header.as_bytes()).await.is_err()
                    || stdin.write_all(&body).await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let pending = Arc::new(Mutex::new(HashMap::new()));
        let diagnostics = Arc::new(Mutex::new(HashMap::new()));
        let diagnostic_notify = Arc::new(Notify::new());
        let root_uri = Url::from_directory_path(root)
            .map_err(|_| format!("could not convert {} to a file URI", root.display()))?
            .to_string();
        tokio::spawn(read_loop(
            BufReader::new(stdout),
            ReadLoopState {
                pending: pending.clone(),
                diagnostics: diagnostics.clone(),
                diagnostic_notify: diagnostic_notify.clone(),
                outgoing: outgoing.clone(),
                root_uri: root_uri.clone(),
                root_name: root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("workspace")
                    .to_string(),
                stderr_tail: stderr_tail.clone(),
            },
        ));

        let client = Arc::new(Self {
            name: name.to_string(),
            outgoing,
            pending,
            next_id: AtomicU64::new(1),
            diagnostics,
            diagnostic_notify,
            documents: AsyncMutex::new(HashMap::new()),
            position_encoding: Mutex::new("utf-16".to_string()),
            stderr_tail,
            child: AsyncMutex::new(Some(child)),
        });
        let initialize = client
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "clientInfo": {"name": "craftsman", "version": env!("CARGO_PKG_VERSION")},
                    "rootUri": root_uri,
                    "capabilities": {
                        "general": {"positionEncodings": ["utf-8", "utf-16"]},
                        "workspace": {"workspaceFolders": true, "symbol": {}},
                        "textDocument": {
                            "synchronization": {"dynamicRegistration": true, "didSave": true},
                            "publishDiagnostics": {},
                            "diagnostic": {},
                            "definition": {},
                            "references": {},
                            "hover": {},
                            "documentSymbol": {}
                        }
                    },
                    "initializationOptions": config.initialization_options,
                    "workspaceFolders": [{
                        "uri": root_uri,
                        "name": root.file_name().and_then(|name| name.to_str()).unwrap_or("workspace")
                    }]
                }),
            )
            .await
            .map_err(|error| client.with_stderr(error))?;
        if let Some(encoding) = initialize
            .pointer("/capabilities/positionEncoding")
            .and_then(Value::as_str)
        {
            *client.position_encoding.lock().unwrap() = encoding.to_string();
        }
        client.notify("initialized", json!({}))?;
        Ok(client)
    }

    fn with_stderr(&self, error: String) -> String {
        let tail = self.stderr_tail.lock().unwrap();
        let detail = String::from_utf8_lossy(&tail).trim().to_string();
        if detail.is_empty() || error.contains(&detail) {
            error
        } else {
            format!("{error}: {detail}")
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.outgoing
            .send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .map_err(|_| format!("{} LSP process is not accepting messages", self.name))
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let _guard = PendingRequestGuard {
            id,
            pending: self.pending.clone(),
        };
        if self
            .outgoing
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .is_err()
        {
            return Err(format!(
                "{} LSP process is not accepting requests",
                self.name
            ));
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!("{} LSP response channel closed", self.name)),
            Err(_) => Err(format!("{} LSP request timed out", self.name)),
        }
    }

    async fn sync_document(&self, path: &Path, uri: &str, language_id: &str) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|error| format!("LSP requires UTF-8 text ({}): {error}", path.display()))?;
        let mut documents = self.documents.lock().await;
        match documents.get_mut(uri) {
            None => {
                self.notify(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": language_id,
                            "version": 1,
                            "text": content
                        }
                    }),
                )?;
                documents.insert(
                    uri.to_string(),
                    DocumentState {
                        version: 1,
                        content,
                    },
                );
            }
            Some(document) if document.content != content => {
                document.version += 1;
                document.content = content.clone();
                self.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": {"uri": uri, "version": document.version},
                        "contentChanges": [{"text": content}]
                    }),
                )?;
            }
            Some(_) => {}
        }
        Ok(())
    }

    fn position(&self, path: &Path, line: u64, column: u64) -> Result<Value, String> {
        if line == 0 || column == 0 {
            return Err("LSP line and column are 1-based and must be at least 1".to_string());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let source_line = content
            .lines()
            .nth((line - 1) as usize)
            .ok_or_else(|| format!("line {line} is past the end of {}", path.display()))?;
        let wanted = (column - 1) as usize;
        let char_count = source_line.chars().count();
        if wanted > char_count {
            return Err(format!(
                "column {column} is past line {line} of {}",
                path.display()
            ));
        }
        let prefix = source_line.chars().take(wanted).collect::<String>();
        let encoding = self.position_encoding.lock().unwrap().clone();
        let character = match encoding.as_str() {
            "utf-8" => prefix.len(),
            "utf-32" => prefix.chars().count(),
            _ => prefix.encode_utf16().count(),
        };
        Ok(json!({"line": line - 1, "character": character}))
    }

    fn cached_diagnostics(&self, uri: &str) -> Vec<Value> {
        self.diagnostics
            .lock()
            .unwrap()
            .get(uri)
            .cloned()
            .unwrap_or_default()
    }

    async fn stop(&self) {
        let _ = tokio::time::timeout(
            Duration::from_millis(750),
            self.request("shutdown", Value::Null),
        )
        .await;
        let _ = self.notify("exit", Value::Null);
        let mut child = self.child.lock().await;
        if let Some(child) = child.as_mut() {
            let _ = child.kill().await;
        }
        *child = None;
    }

    fn start_kill(&self) {
        if let Ok(mut child) = self.child.try_lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.start_kill();
        }
    }
}

struct ReadLoopState {
    pending: PendingRequests,
    diagnostics: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    diagnostic_notify: Arc<Notify>,
    outgoing: mpsc::UnboundedSender<Value>,
    root_uri: String,
    root_name: String,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
}

async fn read_loop<R: AsyncBufRead + Unpin>(mut reader: R, state: ReadLoopState) {
    let ReadLoopState {
        pending,
        diagnostics,
        diagnostic_notify,
        outgoing,
        root_uri,
        root_name,
        stderr_tail,
    } = state;
    while let Ok(Some(message)) = read_message(&mut reader).await {
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            if let Some(id) = message.get("id").cloned() {
                let result = match method {
                    "workspace/configuration" => json!([]),
                    "workspace/workspaceFolders" => {
                        json!([{"uri": root_uri, "name": root_name}])
                    }
                    "workspace/applyEdit" => json!({
                        "applied": false,
                        "failureReason": "Craftsman LSP tools are read-only"
                    }),
                    _ => Value::Null,
                };
                let _ = outgoing.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
            } else if method == "textDocument/publishDiagnostics"
                && let Some(uri) = message.pointer("/params/uri").and_then(Value::as_str)
            {
                let items = message
                    .pointer("/params/diagnostics")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                diagnostics.lock().unwrap().insert(uri.to_string(), items);
                diagnostic_notify.notify_waiters();
            }
            continue;
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if let Some(sender) = pending.lock().unwrap().remove(&id) {
            let result = if let Some(error) = message.get("error") {
                Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown LSP error")
                    .to_string())
            } else {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = sender.send(result);
        }
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let detail = {
        let tail = stderr_tail.lock().unwrap();
        String::from_utf8_lossy(&tail).trim().to_string()
    };
    let error = if detail.is_empty() {
        "LSP process exited".to_string()
    } else {
        format!("LSP process exited: {detail}")
    };
    let senders = pending
        .lock()
        .unwrap()
        .drain()
        .map(|(_, sender)| sender)
        .collect::<Vec<_>>();
    for sender in senders {
        let _ = sender.send(Err(error.clone()));
    }
}

async fn read_message<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Value>, String> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let count = reader
            .read_line(&mut line)
            .await
            .map_err(|error| format!("could not read LSP header: {error}"))?;
        if count == 0 {
            return Ok(None);
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(value) = line
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
            .map(|(_, value)| value.trim())
        {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid LSP Content-Length: {value}"))?,
            );
        }
    }
    let length = content_length.ok_or_else(|| "LSP response omitted Content-Length".to_string())?;
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|error| format!("could not read LSP body: {error}"))?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| format!("invalid LSP JSON: {error}"))
}

#[derive(Clone, Copy)]
enum LspToolKind {
    Diagnostics,
    Definition,
    References,
    Hover,
    DocumentSymbols,
    WorkspaceSymbols,
}

struct LspTool {
    kind: LspToolKind,
    manager: Arc<LspManager>,
}

#[async_trait]
impl Tool for LspTool {
    fn def(&self) -> ToolDef {
        let location_properties = json!({
            "file_path": {"type": "string", "description": "Workspace-relative or absolute file path"},
            "line": {"type": "integer", "minimum": 1, "description": "1-based line"},
            "column": {"type": "integer", "minimum": 1, "description": "1-based Unicode character column"}
        });
        match self.kind {
            LspToolKind::Diagnostics => ToolDef {
                name: "lsp_diagnostics".into(),
                description: "Return compiler/language-server diagnostics for a source file.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {"file_path": location_properties["file_path"].clone()},
                    "required": ["file_path"]
                }),
            },
            LspToolKind::Definition => ToolDef {
                name: "lsp_definition".into(),
                description: "Find the semantic definition at a source position.".into(),
                parameters: json!({"type":"object","properties":location_properties,"required":["file_path","line","column"]}),
            },
            LspToolKind::References => ToolDef {
                name: "lsp_references".into(),
                description: "Find semantic references to the symbol at a source position.".into(),
                parameters: json!({
                    "type":"object",
                    "properties": {
                        "file_path": location_properties["file_path"].clone(),
                        "line": location_properties["line"].clone(),
                        "column": location_properties["column"].clone(),
                        "include_declaration": {"type":"boolean","default":true}
                    },
                    "required":["file_path","line","column"]
                }),
            },
            LspToolKind::Hover => ToolDef {
                name: "lsp_hover".into(),
                description: "Return type information and documentation at a source position.".into(),
                parameters: json!({"type":"object","properties":location_properties,"required":["file_path","line","column"]}),
            },
            LspToolKind::DocumentSymbols => ToolDef {
                name: "lsp_document_symbols".into(),
                description: "List semantic symbols declared in a source file.".into(),
                parameters: json!({
                    "type":"object",
                    "properties":{"file_path":location_properties["file_path"].clone()},
                    "required":["file_path"]
                }),
            },
            LspToolKind::WorkspaceSymbols => ToolDef {
                name: "lsp_workspace_symbols".into(),
                description: "Search semantic symbols in the workspace. file_path selects the language server.".into(),
                parameters: json!({
                    "type":"object",
                    "properties":{
                        "file_path":location_properties["file_path"].clone(),
                        "query":{"type":"string"}
                    },
                    "required":["file_path","query"]
                }),
            },
        }
    }

    async fn call(&self, args: Value, _ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let file = args["file_path"].as_str().unwrap_or_default();
        let line = args["line"].as_u64().unwrap_or_default();
        let column = args["column"].as_u64().unwrap_or_default();
        let result = match self.kind {
            LspToolKind::Diagnostics => self.manager.diagnostics(file).await,
            LspToolKind::Definition => self.manager.definition(file, line, column).await,
            LspToolKind::References => {
                self.manager
                    .references(
                        file,
                        line,
                        column,
                        args["include_declaration"].as_bool().unwrap_or(true),
                    )
                    .await
            }
            LspToolKind::Hover => self.manager.hover(file, line, column).await,
            LspToolKind::DocumentSymbols => self.manager.document_symbols(file).await,
            LspToolKind::WorkspaceSymbols => {
                self.manager
                    .workspace_symbols(file, args["query"].as_str().unwrap_or_default())
                    .await
            }
        };
        Ok(match result {
            Ok(value) => ToolOutcome::ok(value),
            Err(error) => ToolOutcome::err(format!("LSP error: {error}")),
        })
    }
}

fn server_preflight(name: &str, config: &LspServerConfig) -> Result<(), String> {
    if !command_exists(&config.command) {
        return Err(format!(
            "{} is not installed or not on PATH (configure lsp_servers.{name}.command)",
            config.command
        ));
    }
    let executable = Path::new(&config.command)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(&config.command);
    if executable == "rust-analyzer" {
        let output = std::process::Command::new(&config.command)
            .arg("--version")
            .output()
            .map_err(|error| format!("could not run rust-analyzer: {error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if detail.is_empty() {
                "rust-analyzer failed its version check; install it with `rustup component add rust-analyzer` or configure another command".to_string()
            } else {
                format!(
                    "{detail} — install it with `rustup component add rust-analyzer` or configure another command"
                )
            });
        }
    }
    Ok(())
}

fn command_exists(command: &str) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|directory| {
        let candidate = directory.join(command);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            ["exe", "cmd", "bat", "com"]
                .iter()
                .any(|extension| directory.join(format!("{command}.{extension}")).is_file())
        }
        #[cfg(not(windows))]
        {
            false
        }
    })
}

fn language_id_for_extension(extension: &str) -> &str {
    match extension {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" => "javascript",
        "jsx" => "javascriptreact",
        "py" => "python",
        "go" => "go",
        other => other,
    }
}

fn diagnostic_pull_is_unsupported(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("method not found")
        || error.contains("not supported")
        || error.contains("unsupported")
        || error.contains("unhandled method")
}

fn format_diagnostics(server: &str, path: &Path, items: &[Value], root: &Path) -> String {
    if items.is_empty() {
        return format!(
            "No diagnostics from {server} for {}",
            display_path(path, root)
        );
    }
    items
        .iter()
        .map(|item| {
            let start = item.pointer("/range/start").unwrap_or(&Value::Null);
            let line = start["line"].as_u64().unwrap_or(0) + 1;
            let column = start["character"].as_u64().unwrap_or(0) + 1;
            let severity = match item["severity"].as_u64() {
                Some(1) => "error",
                Some(2) => "warning",
                Some(3) => "info",
                Some(4) => "hint",
                _ => "diagnostic",
            };
            format!(
                "{}:{line}:{column}: {severity}: {}",
                display_path(path, root),
                item["message"].as_str().unwrap_or("(no message)")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_locations(value: &Value, root: &Path) -> String {
    let entries = match value {
        Value::Array(entries) => entries.clone(),
        Value::Null => Vec::new(),
        entry => vec![entry.clone()],
    };
    let locations = entries
        .iter()
        .filter_map(|entry| {
            let uri = entry["uri"]
                .as_str()
                .or_else(|| entry["targetUri"].as_str())?;
            let range = entry
                .get("range")
                .or_else(|| entry.get("targetSelectionRange"))?;
            let line = range.pointer("/start/line")?.as_u64()? + 1;
            let column = range.pointer("/start/character")?.as_u64()? + 1;
            Some(format!("{}:{line}:{column}", display_uri(uri, root)))
        })
        .collect::<Vec<_>>();
    if locations.is_empty() {
        "No locations found".to_string()
    } else {
        locations.join("\n")
    }
}

fn format_hover(value: &Value) -> String {
    let contents = value.get("contents").unwrap_or(value);
    let mut parts = Vec::new();
    collect_markup(contents, &mut parts);
    if parts.is_empty() {
        "No hover information".to_string()
    } else {
        parts.join("\n\n")
    }
}

fn collect_markup(value: &Value, parts: &mut Vec<String>) {
    match value {
        Value::String(text) if !text.trim().is_empty() => parts.push(text.clone()),
        Value::Array(values) => {
            for value in values {
                collect_markup(value, parts);
            }
        }
        Value::Object(object) => {
            if let Some(text) = object.get("value").and_then(Value::as_str)
                && !text.trim().is_empty()
            {
                parts.push(text.to_string());
            }
        }
        _ => {}
    }
}

fn format_symbols(value: &Value, root: &Path) -> String {
    let mut rows = Vec::new();
    if let Some(symbols) = value.as_array() {
        for symbol in symbols {
            collect_symbol(symbol, root, 0, &mut rows);
        }
    }
    if rows.is_empty() {
        "No symbols found".to_string()
    } else {
        rows.join("\n")
    }
}

fn collect_symbol(symbol: &Value, root: &Path, depth: usize, rows: &mut Vec<String>) {
    let name = symbol["name"].as_str().unwrap_or("(unnamed)");
    let location = if let Some(uri) = symbol.pointer("/location/uri").and_then(Value::as_str) {
        let line = symbol
            .pointer("/location/range/start/line")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        format!("{}:{line}", display_uri(uri, root))
    } else {
        let line = symbol
            .pointer("/selectionRange/start/line")
            .or_else(|| symbol.pointer("/range/start/line"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        format!("line {line}")
    };
    rows.push(format!("{}{} — {location}", "  ".repeat(depth), name));
    if let Some(children) = symbol["children"].as_array() {
        for child in children {
            collect_symbol(child, root, depth + 1, rows);
        }
    }
}

fn display_uri(uri: &str, root: &Path) -> String {
    Url::parse(uri)
        .ok()
        .and_then(|uri| uri.to_file_path().ok())
        .map(|path| display_path(&path, root))
        .unwrap_or_else(|| uri.to_string())
}

fn display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_request_guard_removes_pending_entry() {
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let (sender, _receiver) = oneshot::channel();
        pending.lock().unwrap().insert(7, sender);
        {
            let _guard = PendingRequestGuard {
                id: 7,
                pending: pending.clone(),
            };
        }
        assert!(pending.lock().unwrap().is_empty());
        assert!(diagnostic_pull_is_unsupported("Method not found"));
        assert!(!diagnostic_pull_is_unsupported("LSP process exited"));
    }

    #[tokio::test]
    async fn framing_reads_lsp_message() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let framed = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        let mut reader = BufReader::new(framed.as_bytes());
        let value = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(value["id"], 1);
    }

    #[test]
    fn configured_servers_override_defaults_and_normalize_extensions() {
        let mut config = Config::default();
        config.lsp_servers.insert(
            "rust".into(),
            LspServerConfig {
                command: "custom-ra".into(),
                args: vec!["x".into()],
                extensions: vec![".RS".into()],
                language_id: Some("rust".into()),
                initialization_options: Value::Null,
                enabled: true,
            },
        );
        let servers = config.effective_lsp_servers();
        assert_eq!(servers["rust"].command, "custom-ra");
        assert_eq!(servers["rust"].extensions, vec!["rs"]);
        assert!(servers.contains_key("python"));
        let missing = LspServerConfig {
            command: "definitely-not-a-real-language-server".into(),
            args: Vec::new(),
            extensions: vec!["none".into()],
            language_id: None,
            initialization_options: Value::Null,
            enabled: true,
        };
        assert!(
            server_preflight("missing", &missing)
                .unwrap_err()
                .contains("not installed")
        );
    }

    #[test]
    fn unicode_position_converts_to_utf16() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "a😀b\n").unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = LspClient {
            name: "test".into(),
            outgoing: tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            diagnostic_notify: Arc::new(Notify::new()),
            documents: AsyncMutex::new(HashMap::new()),
            position_encoding: Mutex::new("utf-16".into()),
            stderr_tail: Arc::new(Mutex::new(Vec::new())),
            child: AsyncMutex::new(None),
        };
        assert_eq!(client.position(&path, 1, 3).unwrap()["character"], 3);
    }

    #[tokio::test]
    async fn fake_stdio_server_round_trip() {
        if !command_exists("python3") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake_lsp.py");
        std::fs::write(
            &script,
            r#"import json, sys

def read_msg():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b'\r\n', b'\n'):
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1].strip())
    return json.loads(sys.stdin.buffer.read(length))

def send(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n' % len(body)).encode() + body)
    sys.stdout.buffer.flush()

while True:
    msg = read_msg()
    if msg is None:
        break
    method = msg.get('method')
    if 'id' not in msg:
        if method == 'exit':
            break
        continue
    mid = msg['id']
    uri = msg.get('params', {}).get('textDocument', {}).get('uri', '')
    if method == 'initialize':
        result = {'capabilities': {'positionEncoding': 'utf-16'}}
    elif method == 'textDocument/diagnostic':
        result = {'kind':'full','items':[{'range':{'start':{'line':0,'character':1},'end':{'line':0,'character':2}},'severity':2,'message':'fake warning'}]}
    elif method == 'textDocument/definition':
        result = {'uri':uri,'range':{'start':{'line':1,'character':2},'end':{'line':1,'character':3}}}
    elif method == 'textDocument/references':
        result = [{'uri':uri,'range':{'start':{'line':2,'character':0},'end':{'line':2,'character':1}}}]
    elif method == 'textDocument/hover':
        result = {'contents':{'kind':'markdown','value':'fake hover'}}
    elif method == 'textDocument/documentSymbol':
        result = [{'name':'FakeSymbol','kind':12,'range':{'start':{'line':0,'character':0},'end':{'line':0,'character':1}},'selectionRange':{'start':{'line':0,'character':0},'end':{'line':0,'character':1}}}]
    elif method == 'workspace/symbol':
        result = [{'name':'WorkspaceFake','kind':12,'location':{'uri':uri,'range':{'start':{'line':0,'character':0},'end':{'line':0,'character':1}}}}]
    else:
        result = None
    send({'jsonrpc':'2.0','id':mid,'result':result})
"#,
        )
        .unwrap();
        let source = dir.path().join("sample.fake");
        std::fs::write(&source, "a😀b\nline2\nline3\n").unwrap();
        let mut config = Config::default();
        for server in ["rust", "typescript", "python", "go"] {
            config.lsp_servers.insert(
                server.into(),
                LspServerConfig {
                    command: String::new(),
                    args: Vec::new(),
                    extensions: Vec::new(),
                    language_id: None,
                    initialization_options: Value::Null,
                    enabled: false,
                },
            );
        }
        config.lsp_servers.insert(
            "fake".into(),
            LspServerConfig {
                command: "python3".into(),
                args: vec![script.to_string_lossy().into_owned()],
                extensions: vec!["fake".into()],
                language_id: Some("fake".into()),
                initialization_options: Value::Null,
                enabled: true,
            },
        );
        let manager = LspManager::new(dir.path().to_path_buf(), &config);
        assert!(
            manager
                .diagnostics("sample.fake")
                .await
                .unwrap()
                .contains("fake warning")
        );
        assert!(
            manager
                .definition("sample.fake", 1, 3)
                .await
                .unwrap()
                .contains("sample.fake:2:3")
        );
        assert!(
            manager
                .references("sample.fake", 1, 1, true)
                .await
                .unwrap()
                .contains("sample.fake:3:1")
        );
        assert_eq!(
            manager.hover("sample.fake", 1, 1).await.unwrap(),
            "fake hover"
        );
        assert!(
            manager
                .document_symbols("sample.fake")
                .await
                .unwrap()
                .contains("FakeSymbol")
        );
        assert!(
            manager
                .workspace_symbols("sample.fake", "Fake")
                .await
                .unwrap()
                .contains("WorkspaceFake")
        );
        manager.restart("fake").await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires rust-analyzer on PATH"]
    async fn rust_analyzer_smoke_test() {
        if !command_exists("rust-analyzer")
            || !std::process::Command::new("rust-analyzer")
                .arg("--version")
                .status()
                .is_ok_and(|status| status.success())
        {
            return;
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf();
        let manager = LspManager::new(root, &Config::default());
        let diagnostics = manager
            .diagnostics("crates/craftsman-rs/src/lsp.rs")
            .await
            .unwrap();
        assert!(!diagnostics.trim().is_empty());
        let _ = manager.restart("rust").await;
    }

    #[test]
    fn formatting_handles_locations_hover_symbols_and_diagnostics() {
        // A drive-less `file:///repo` URL is valid on Unix but cannot be
        // converted into an absolute Windows path. Build the fixture from a
        // native absolute path first, then derive the file URL exactly as a
        // real language server would.
        let root_path = std::env::temp_dir().join("craftsman-lsp-format-root");
        let source_path = root_path.join("src").join("lib.rs");
        let source_uri = Url::from_file_path(&source_path).unwrap();
        let root = root_path.as_path();
        let location = json!([{
            "uri": source_uri.as_str(),
            "range": {"start": {"line": 2, "character": 4}}
        }]);
        assert_eq!(format_locations(&location, root), "src/lib.rs:3:5");
        assert_eq!(
            format_hover(&json!({"contents":{"kind":"markdown","value":"hello"}})),
            "hello"
        );
        assert!(
            format_symbols(
                &json!([{"name":"Thing","selectionRange":{"start":{"line":1}}}]),
                root
            )
            .contains("Thing — line 2")
        );
        assert!(
            format_diagnostics(
                "rust",
                &root_path.join("a.rs"),
                &[json!({"range":{"start":{"line":0,"character":1}},"severity":1,"message":"bad"})],
                root
            )
            .contains("a.rs:1:2: error: bad")
        );
    }
}
