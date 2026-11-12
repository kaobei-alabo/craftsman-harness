//! Minimal MCP client speaking the modern stateless protocol
//! (`2026-07-28`): `server/discover` → `tools/list` → `tools/call`. Two
//! transports, both configured as ordinary `mcp_servers` entries:
//!
//! - `streamable_http` (default): JSON-RPC over HTTP POST, responses as
//!   JSON or a one-event SSE body. The platform's `/mcp` is just another
//!   such server (add it with the API token as a bearer header).
//! - `stdio`: JSON-RPC over a child process's stdin/stdout
//!   (`{"transport": "stdio", "command": …, "args": […], "env": {…}}`),
//!   newline-delimited JSON, one message per line.
//!
//! The legacy initialize/session era is not implemented on either
//! transport (deprecated platform-side as of 2027-07-28; see
//! `docs/platform-token-refactor.md` §4.6) — a legacy-era server fails with
//! a clear per-server error instead of a silent skip. Per-server failures
//! surface as a message instead of crashing the session.

use serde_json::{Value, json};
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// Modern stateless protocol version (per the platform's `/mcp` contract).
pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";

const RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// The per-request `_meta` every modern-era request must carry — the
/// stateless replacement for session-establishment state.
fn request_meta() -> Value {
    json!({
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {
            "name": "craftsman",
            "version": env!("CARGO_PKG_VERSION")
        },
        "io.modelcontextprotocol/protocolVersion": MCP_PROTOCOL_VERSION
    })
}

/// A live stdio child: the request/response pair runs under one lock, so
/// lines on stdout are always matched to the request that wrote stdin.
struct StdioProc {
    #[allow(dead_code)]
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl StdioProc {
    fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&std::path::Path>,
    ) -> anyhow::Result<StdioProc> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // Server chatter (progress logs) never belongs in the agent's
            // transcript; the response channel is stdout.
            .stderr(std::process::Stdio::null())
            // The client drops with the session; the child must not outlive it.
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("could not start MCP server `{command}`: {e}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP server `{command}` stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP server `{command}` stdout was not piped"))?;
        Ok(StdioProc {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    /// One request/response round trip: write the JSON-RPC line, read lines
    /// until the response with our id arrives (skipping notifications).
    async fn round_trip(&mut self, body: &Value, id: u64) -> anyhow::Result<Value> {
        let mut line = serde_json::to_string(body)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        let payload = tokio::time::timeout(RPC_TIMEOUT, async {
            loop {
                let mut response = String::new();
                let read = self.stdout.read_line(&mut response).await?;
                if read == 0 {
                    anyhow::bail!("MCP server closed its output");
                }
                let trimmed = response.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                    // Non-JSON stdout noise — skip it.
                    continue;
                };
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    // A notification or another id (shouldn't happen under the
                    // lock, but tolerate it) — keep reading.
                    continue;
                }
                return Ok(value);
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("MCP server response timed out after 30s"))??;
        if let Some(err) = payload.get("error").filter(|e| !e.is_null()) {
            anyhow::bail!("MCP error: {err}");
        }
        Ok(payload["result"].clone())
    }
}

enum Transport {
    Http {
        http: reqwest::Client,
        url: String,
        headers: Vec<(String, String)>,
    },
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        /// Explicit ACP session directory; absent for existing config entries.
        cwd: Option<std::path::PathBuf>,
        /// Spawned lazily on the first request, so a bad `command` surfaces
        /// as a per-server error instead of failing config parsing.
        proc: tokio::sync::Mutex<Option<StdioProc>>,
    },
}

pub struct McpClient {
    transport: Transport,
    next_id: Mutex<u64>,
}

impl McpClient {
    /// A user-configured MCP server from `cfg.mcp_servers`. Requires
    /// `name`; the entry declares its transport:
    ///
    /// - `streamable_http` (default): requires `url`; carries the entry's
    ///   own `headers` (if any).
    /// - `stdio`: requires `command`; optional `args` and `env`.
    pub fn for_config_server(entry: &Value) -> Option<(String, McpClient)> {
        let name = entry.get("name").and_then(|v| v.as_str())?;
        if name.is_empty() {
            return None;
        }
        let transport = entry
            .get("transport")
            .and_then(|v| v.as_str())
            .unwrap_or("streamable_http");
        match transport {
            "streamable_http" => {
                let url = entry.get("url").and_then(|v| v.as_str())?;
                if url.is_empty() {
                    return None;
                }
                let headers = entry
                    .get("headers")
                    .and_then(|v| v.as_object())
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                Some((
                    name.to_string(),
                    McpClient::new(Transport::Http {
                        http: reqwest::Client::new(),
                        url: url.to_string(),
                        headers,
                    }),
                ))
            }
            "stdio" => {
                let command = entry.get("command").and_then(|v| v.as_str())?;
                if command.is_empty() {
                    return None;
                }
                let args = entry
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let env = entry
                    .get("env")
                    .and_then(|v| v.as_object())
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                Some((
                    name.to_string(),
                    McpClient::new(Transport::Stdio {
                        command: command.to_string(),
                        args,
                        env,
                        cwd: entry
                            .get("_craftsman_cwd")
                            .and_then(Value::as_str)
                            .map(std::path::PathBuf::from),
                        proc: tokio::sync::Mutex::new(None),
                    }),
                ))
            }
            _ => None,
        }
    }

    fn new(transport: Transport) -> McpClient {
        McpClient {
            transport,
            next_id: Mutex::new(1),
        }
    }

    /// The endpoint label for the `/mcp` listing — the URL for HTTP servers,
    /// `stdio:<command>` for child-process servers. Never carries auth.
    pub fn url(&self) -> &str {
        match &self.transport {
            Transport::Http { url, .. } => url,
            Transport::Stdio { command, .. } => command,
        }
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        tool_name: Option<&str>,
    ) -> anyhow::Result<Value> {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            *n - 1
        };
        match &self.transport {
            Transport::Http { http, url, headers } => {
                let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
                let mut req = http
                    .post(url)
                    .header("accept", "application/json, text/event-stream")
                    .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
                    .header("Mcp-Method", method)
                    .timeout(RPC_TIMEOUT)
                    .json(&body);
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                if let Some(name) = tool_name {
                    req = req.header("Mcp-Name", name);
                }
                let resp = req.send().await?;
                let status = resp.status();
                let ctype = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let text = resp.text().await?;
                if !status.is_success() {
                    // Char-boundary-safe truncation — a byte slice can panic
                    // on multi-byte UTF-8 in the error body and crash the REPL.
                    let short: String = text.chars().take(300).collect();
                    anyhow::bail!("MCP HTTP {status}: {short}");
                }
                let payload: Value = if ctype.contains("text/event-stream") {
                    // Take the first data: line carrying our response.
                    text.lines()
                        .filter_map(|l| l.strip_prefix("data: "))
                        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
                        .find(|v| v.get("id").is_some())
                        .ok_or_else(|| anyhow::anyhow!("no JSON-RPC response in SSE body"))?
                } else {
                    serde_json::from_str(&text)?
                };
                if let Some(err) = payload.get("error").filter(|e| !e.is_null()) {
                    anyhow::bail!("MCP error: {err}");
                }
                Ok(payload["result"].clone())
            }
            Transport::Stdio {
                command,
                args,
                env,
                cwd,
                proc,
            } => {
                let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
                let mut guard = proc.lock().await;
                // Spawn lazily on the first request, so a bad `command`
                // surfaces as a per-server error instead of a crash.
                let spawned = match guard.as_mut() {
                    Some(proc) => proc,
                    None => guard.insert(StdioProc::spawn(command, args, env, cwd.as_deref())?),
                };
                spawned.round_trip(&body, id).await
            }
        }
    }

    /// `server/discover` — the stateless replacement for `initialize`.
    /// Validates that the server speaks the modern protocol before tool
    /// listing; a legacy-era server fails here with a clear error instead
    /// of a silent skip.
    pub async fn discover(&self) -> anyhow::Result<()> {
        self.rpc("server/discover", json!({"_meta": request_meta()}), None)
            .await?;
        Ok(())
    }

    pub async fn list_tools(&self) -> anyhow::Result<Vec<McpTool>> {
        let result = self
            .rpc("tools/list", json!({"_meta": request_meta()}), None)
            .await?;
        let tools = result["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|t| McpTool {
                name: t["name"].as_str().unwrap_or_default().to_string(),
                description: t["description"].as_str().unwrap_or_default().to_string(),
                input_schema: t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"})),
            })
            .filter(|t| !t.name.is_empty())
            .collect();
        Ok(tools)
    }

    /// Call a tool; flatten the content parts to one text blob (what the
    /// LLM consumes).
    pub async fn call_tool(&self, name: &str, args: &Value) -> String {
        let params = json!({
            "_meta": request_meta(),
            "name": name,
            "arguments": args,
        });
        match self.rpc("tools/call", params, Some(name)).await {
            Ok(result) => {
                let text: Vec<String> = result["content"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p["text"].as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let joined = text.join("\n");
                if result["isError"].as_bool().unwrap_or(false) {
                    format!("Error from tool: {joined}")
                } else if joined.is_empty() {
                    result.to_string()
                } else {
                    joined
                }
            }
            Err(e) => format!("Error calling MCP tool {name}: {e}"),
        }
    }
}

/// Loaded MCP toolset for one server.
pub struct McpTools {
    /// Display label — the config entry's `name`.
    pub name: String,
    pub client: McpClient,
    pub tools: Vec<McpTool>,
}

/// One row of the `/mcp` listing — what the Session keeps after the tools
/// themselves have been consumed by the agent builder.
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub url: String,
    pub tools: usize,
}

/// All MCP servers loaded for a session: every user-configured server
/// (the platform's `/mcp` included, as an ordinary entry with a bearer
/// header). Per-server failures are collected as printable messages instead
/// of failing the whole load (a bad server just surfaces in `/mcp`).
#[derive(Default)]
pub struct LoadedMcp {
    /// One entry per server that connected + listed tools successfully.
    pub servers: Vec<McpTools>,
    /// `(server_label, error)` for each server that failed to load.
    pub errors: Vec<(String, String)>,
}

impl LoadedMcp {
    /// Total tool count across all connected servers.
    pub fn tool_count(&self) -> usize {
        self.servers.iter().map(|s| s.tools.len()).sum()
    }

    /// The `/mcp` listing rows — taken BEFORE the builder consumes `servers`.
    pub fn summary(&self) -> Vec<McpServerInfo> {
        self.servers
            .iter()
            .map(|s| McpServerInfo {
                name: s.name.clone(),
                url: s.client.url().to_string(),
                tools: s.tools.len(),
            })
            .collect()
    }
}

/// Connect to one server and list its tools.
async fn connect_server(name: String, client: McpClient) -> anyhow::Result<McpTools> {
    client.discover().await?;
    let tools = client.list_tools().await?;
    Ok(McpTools {
        name,
        client,
        tools,
    })
}

/// Expand `${ENV_VAR}` placeholders in one config entry's secret-bearing
/// values: `headers` (HTTP transport) and `env` (stdio transport). Missing
/// environment variables surface as a per-server error instead of a
/// literal `${VAR}` bearer silently 401-ing later.
fn expand_entry_secrets(entry: &Value) -> anyhow::Result<Value> {
    let mut out = entry.clone();
    for field in ["headers", "env"] {
        let Some(values) = out.get_mut(field).and_then(Value::as_object_mut) else {
            continue;
        };
        for value in values.values_mut() {
            if let Some(text) = value.as_str() {
                *value = Value::String(crate::config::expand_env_placeholders(text)?);
            }
        }
    }
    Ok(out)
}

/// Load every user-configured server in `cfg.mcp_servers` — streamable-HTTP
/// (including the platform's `/mcp`, as an ordinary entry with a bearer
/// header) and stdio child processes. Never fails the caller: per-server
/// errors land in `LoadedMcp.errors` for the REPL to surface.
pub async fn load_mcp_tools(cfg: &crate::config::Config) -> LoadedMcp {
    let mut loaded = LoadedMcp::default();

    for entry in &cfg.mcp_servers {
        let expanded = match expand_entry_secrets(entry) {
            Ok(expanded) => expanded,
            Err(error) => {
                let label = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unnamed")
                    .to_string();
                loaded.errors.push((label, error.to_string()));
                continue;
            }
        };
        let Some((name, client)) = McpClient::for_config_server(&expanded) else {
            continue; // not a valid entry — skip silently
        };
        match connect_server(name.clone(), client).await {
            Ok(t) => loaded.servers.push(t),
            Err(e) => loaded.errors.push((name, e.to_string())),
        }
    }

    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn config_server_parses_name_url_and_headers() {
        let entry = json!({
            "name": "acme",
            "url": "https://mcp.acme.test/mcp",
            "headers": {"Authorization": "Bearer xyz", "X-Env": "prod"}
        });
        let (name, client) = McpClient::for_config_server(&entry).expect("valid entry");
        assert_eq!(name, "acme");
        assert_eq!(client.url(), "https://mcp.acme.test/mcp");
        let Transport::Http { headers, .. } = &client.transport else {
            panic!("expected http transport");
        };
        // Both headers carried through (order-independent).
        assert!(headers.contains(&("Authorization".to_string(), "Bearer xyz".to_string())));
        assert!(headers.contains(&("X-Env".to_string(), "prod".to_string())));
    }

    #[test]
    fn config_server_without_headers_is_ok() {
        let entry = json!({"name": "n", "url": "https://x.test/mcp"});
        let (_, client) = McpClient::for_config_server(&entry).expect("valid");
        let Transport::Http { headers, .. } = &client.transport else {
            panic!("expected http transport");
        };
        assert!(headers.is_empty());
    }

    #[test]
    fn stdio_config_parses_command_args_and_env() {
        let entry = json!({
            "name": "context7",
            "transport": "stdio",
            "command": "npx",
            "args": ["-y", "@upstash/context7-mcp"],
            "env": {"API_KEY": "k"}
        });
        let (name, client) = McpClient::for_config_server(&entry).expect("valid entry");
        assert_eq!(name, "context7");
        assert_eq!(client.url(), "npx");
        let Transport::Stdio {
            command,
            args,
            env,
            proc,
            cwd,
        } = &client.transport
        else {
            panic!("expected stdio transport");
        };
        assert_eq!(command, "npx");
        assert_eq!(
            args,
            &vec!["-y".to_string(), "@upstash/context7-mcp".to_string()]
        );
        assert_eq!(env, &vec![("API_KEY".to_string(), "k".to_string())]);
        assert!(cwd.is_none());
        assert!(proc.blocking_lock().is_none(), "spawn is lazy");
    }

    #[test]
    fn stdio_config_requires_a_command() {
        // Missing command.
        assert!(
            McpClient::for_config_server(&json!({"name": "n", "transport": "stdio"})).is_none()
        );
        // Empty command.
        assert!(
            McpClient::for_config_server(
                &json!({"name": "n", "transport": "stdio", "command": ""})
            )
            .is_none()
        );
    }

    #[test]
    fn config_server_rejects_incomplete_or_wrong_transport() {
        // Missing url (http).
        assert!(McpClient::for_config_server(&json!({"name": "n"})).is_none());
        // Missing name.
        assert!(McpClient::for_config_server(&json!({"url": "https://x/mcp"})).is_none());
        // Empty fields.
        assert!(
            McpClient::for_config_server(&json!({"name": "", "url": "https://x/mcp"})).is_none()
        );
        // Unknown transport is skipped (streamable_http | stdio supported).
        assert!(
            McpClient::for_config_server(
                &json!({"name": "n", "url": "https://x/mcp", "transport": "sse"})
            )
            .is_none()
        );
        // Explicit transports are accepted.
        assert!(
            McpClient::for_config_server(
                &json!({"name": "n", "url": "https://x/mcp", "transport": "streamable_http"})
            )
            .is_some()
        );
        assert!(
            McpClient::for_config_server(
                &json!({"name": "n", "transport": "stdio", "command": "sh"})
            )
            .is_some()
        );
    }

    #[test]
    fn header_env_placeholders_expand() {
        crate::test_environment::run(crate::test_environment::Case::McpHeader);
    }

    pub(super) fn header_env_placeholders_body() {
        let entry = json!({
            "name": "craftsman-platform",
            "url": "https://platform.example.com/mcp",
            "headers": {"Authorization": "Bearer ${CRAFTSMAN_MCP_TEST_TOKEN}"}
        });
        let expanded = expand_entry_secrets(&entry).expect("expansion");
        let (_, client) = McpClient::for_config_server(&expanded).expect("valid");
        let Transport::Http { headers, .. } = &client.transport else {
            panic!("expected http transport");
        };
        assert!(headers.contains(&("Authorization".to_string(), "Bearer tok-7".to_string())));
        // Missing env var → error carrying the server name.
        let bad = json!({
            "name": "acme",
            "url": "https://x.test/mcp",
            "headers": {"Authorization": "Bearer ${CRAFTSMAN_MCP_MISSING_9}"}
        });
        assert!(expand_entry_secrets(&bad).is_err());
    }

    #[test]
    fn stdio_env_placeholders_expand() {
        crate::test_environment::run(crate::test_environment::Case::McpStdio);
    }

    pub(super) fn stdio_env_placeholders_body() {
        let entry = json!({
            "name": "n",
            "transport": "stdio",
            "command": "sh",
            "env": {"API_KEY": "${CRAFTSMAN_STDIO_TEST_KEY}"}
        });
        let expanded = expand_entry_secrets(&entry).expect("expansion");
        let (_, client) = McpClient::for_config_server(&expanded).expect("valid");
        let Transport::Stdio { env, .. } = &client.transport else {
            panic!("expected stdio transport");
        };
        assert_eq!(env, &vec![("API_KEY".to_string(), "sk-9".to_string())]);
    }

    #[test]
    fn secret_expansion_without_secrets_is_passthrough() {
        let entry = json!({"name": "n", "url": "https://x.test/mcp"});
        let expanded = expand_entry_secrets(&entry).expect("expansion");
        assert_eq!(expanded, entry);
    }

    #[tokio::test]
    async fn load_skips_invalid_config_entries_without_errors() {
        // Only invalid config entries → empty, no errors.
        let cfg = crate::config::Config {
            mcp_servers: vec![
                json!({"name": "n"}),
                json!({"transport": "stdio"}),
                json!({"transport": "bad"}),
            ],
            ..Default::default()
        };
        let loaded = load_mcp_tools(&cfg).await;
        assert_eq!(loaded.tool_count(), 0);
        assert!(loaded.servers.is_empty());
        assert!(
            loaded.errors.is_empty(),
            "invalid entries are skipped, not errors"
        );
    }

    #[tokio::test]
    async fn bad_stdio_command_surfaces_as_a_per_server_error() {
        let cfg = crate::config::Config {
            mcp_servers: vec![json!({
                "name": "no-such-server",
                "transport": "stdio",
                "command": "craftsman-definitely-not-a-command-9f3a"
            })],
            ..Default::default()
        };
        let loaded = load_mcp_tools(&cfg).await;
        assert!(loaded.servers.is_empty());
        assert_eq!(loaded.errors.len(), 1);
        assert_eq!(loaded.errors[0].0, "no-such-server");
        assert!(
            loaded.errors[0].1.contains("could not start MCP server"),
            "got: {}",
            loaded.errors[0].1
        );
    }

    /// A mock HTTP server speaking the modern stateless protocol.
    async fn modern_mcp_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION))
            .and(header("Mcp-Method", "server/discover"))
            .and(body_partial_json(json!({"method": "server/discover"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1, "result": {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "serverInfo": {"name": "mock", "version": "1"}
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("Mcp-Method", "tools/list"))
            .and(body_partial_json(json!({"method": "tools/list"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 2, "result": {
                    "tools": [{
                        "name": "workflows__list_workflows",
                        "description": "List workflows",
                        "inputSchema": {"type": "object"}
                    }]
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("Mcp-Method", "tools/call"))
            .and(header("Mcp-Name", "workflows__list_workflows"))
            .and(body_partial_json(json!({
                "method": "tools/call",
                "params": {"name": "workflows__list_workflows"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 3, "result": {
                    "content": [{"type": "text", "text": "no workflows"}],
                    "isError": false
                }
            })))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn modern_protocol_discover_list_call_round_trip() {
        let server = modern_mcp_server().await;
        let client = McpClient::new(Transport::Http {
            http: reqwest::Client::new(),
            url: format!("{}/mcp", server.uri()),
            headers: vec![("Authorization".to_string(), "Bearer mpat-1".to_string())],
        });
        client.discover().await.expect("discover");
        let tools = client.list_tools().await.expect("tools/list");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "workflows__list_workflows");
        let out = client
            .call_tool("workflows__list_workflows", &json!({}))
            .await;
        assert_eq!(out, "no workflows");

        // Every request carried the required per-request _meta.
        for request in server.received_requests().await.unwrap() {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(
                body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
                MCP_PROTOCOL_VERSION,
                "method {} must carry _meta",
                body["method"]
            );
            assert_eq!(
                request.headers.get("mcp-protocol-version").unwrap(),
                MCP_PROTOCOL_VERSION
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_transport_round_trips_through_a_child_process() {
        // A minimal modern-era MCP server as a shell script: newline-delimited
        // JSON-RPC on stdin/stdout, echoing the request id and the test env var.
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-mcp.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\n\
             while IFS= read -r line; do\n\
               id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\).*/\\1/p')\n\
               case \"$line\" in\n\
                 *'\"method\":\"server/discover\"'*)\n\
                   printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"protocolVersion\":\"2026-07-28\",\"env\":\"%s\"}}\\n' \"$id\" \"$FAKE_MCP_TEST_ENV\" ;;\n\
                 *'\"method\":\"tools/list\"'*)\n\
                   printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"tools\":[{\"name\":\"demo__echo\",\"description\":\"Echo\",\"inputSchema\":{\"type\":\"object\"}}]}}\\n' \"$id\" ;;\n\
                 *'\"method\":\"tools/call\"'*)\n\
                   printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"hello from stdio\"}],\"isError\":false}}\\n' \"$id\" ;;\n\
               esac\n\
             done\n",
        )
        .unwrap();
        let client = McpClient::new(Transport::Stdio {
            command: "sh".into(),
            args: vec![script.to_string_lossy().into_owned()],
            env: vec![("FAKE_MCP_TEST_ENV".to_string(), "plumbed".to_string())],
            cwd: None,
            proc: tokio::sync::Mutex::new(None),
        });
        client.discover().await.expect("stdio discover");
        let tools = client.list_tools().await.expect("stdio tools/list");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "demo__echo");
        let out = client.call_tool("demo__echo", &json!({})).await;
        assert_eq!(out, "hello from stdio");
        // Environment plumbing: the fake server echoes $FAKE_MCP_TEST_ENV
        // into its discover result.
        let result = client
            .rpc("server/discover", json!({"_meta": request_meta()}), None)
            .await
            .expect("second discover");
        assert_eq!(result["env"], "plumbed");
    }

    #[tokio::test]
    async fn load_collects_connection_errors_per_server() {
        let server = MockServer::start().await;
        // A server that only speaks the legacy era (rejects server/discover).
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(400).set_body_string("unknown method"))
            .mount(&server)
            .await;
        let cfg = crate::config::Config {
            mcp_servers: vec![json!({
                "name": "legacy-server",
                "url": format!("{}/mcp", server.uri()),
                "headers": {"Authorization": "Bearer t"}
            })],
            ..Default::default()
        };
        let loaded = load_mcp_tools(&cfg).await;
        assert!(loaded.servers.is_empty());
        assert_eq!(loaded.errors.len(), 1);
        assert_eq!(loaded.errors[0].0, "legacy-server");
        assert!(loaded.errors[0].1.contains("400"));
    }

    #[tokio::test]
    async fn http_401_maps_to_a_clear_token_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(401).set_body_string("missing bearer"))
            .mount(&server)
            .await;
        let client = McpClient::new(Transport::Http {
            http: reqwest::Client::new(),
            url: format!("{}/mcp", server.uri()),
            headers: vec![],
        });
        let error = client.discover().await.unwrap_err();
        assert!(error.to_string().contains("401"));
    }

    #[tokio::test]
    async fn sse_bodies_are_parsed_like_json() {
        let server = MockServer::start().await;
        let sse_body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2026-07-28\"}}\n\n";
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("Mcp-Method", "server/discover"))
            .respond_with(
                // set_body_string would force content-type text/plain; the
                // raw setter lets the SSE content-type through.
                ResponseTemplate::new(200)
                    .set_body_raw(sse_body.as_bytes().to_owned(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("Mcp-Method", "tools/list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 2, "result": {"tools": []}
            })))
            .mount(&server)
            .await;
        let client = McpClient::new(Transport::Http {
            http: reqwest::Client::new(),
            url: format!("{}/mcp", server.uri()),
            headers: vec![],
        });
        client.discover().await.expect("SSE discover");
        assert!(client.list_tools().await.unwrap().is_empty());
    }
}

#[cfg(test)]
pub(crate) fn mcp_header_environment_fixture_for_tests() {
    tests::header_env_placeholders_body();
}

#[cfg(test)]
pub(crate) fn mcp_stdio_environment_fixture_for_tests() {
    tests::stdio_env_placeholders_body();
}
