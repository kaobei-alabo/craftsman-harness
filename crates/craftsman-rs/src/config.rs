//! Craftsman's `~/.craftsman/config.json`.
//!
//! The file can hold legacy keys this struct does not model (the retired
//! Python client wrote keys like `langfuse_*`), so unknown fields are
//! preserved round-trip via `extra`. Secrets (`api_key`, the platform API
//! token) are never logged or printed (craftsman-harness rule #7).
//!
//! Platform access is headless and token-configured (see
//! `docs/platform-token-refactor.md`): the platform's LLM endpoint is a
//! stock OpenAI-compatible provider (`base_url` + `api_key`), the platform's
//! `/mcp` is an ordinary `mcp_servers` entry, and `platform.api_token` is
//! the single credential for harness-facing REST (skills) and the workspace
//! inbox. `${ENV_VAR}` placeholders are supported in `api_key` and MCP
//! header values. Legacy login-era fields (`auth_token`, `refresh_token`, …)
//! are no longer modeled; on-disk leftovers round-trip through `extra` and
//! trigger a one-time migration notice.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    Plan,
    AcceptEdits,
    #[default]
    Manual,
    BypassPermissions,
    Auto,
}

impl PermissionMode {
    pub const ALL: [PermissionMode; 5] = [
        PermissionMode::Plan,
        PermissionMode::AcceptEdits,
        PermissionMode::Manual,
        PermissionMode::BypassPermissions,
        PermissionMode::Auto,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Plan => "plan",
            PermissionMode::AcceptEdits => "accept-edits",
            PermissionMode::Manual => "manual",
            PermissionMode::BypassPermissions => "bypass-permissions",
            PermissionMode::Auto => "auto",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            PermissionMode::Plan => "read-only planning; mutations are blocked",
            PermissionMode::AcceptEdits => "file edits are accepted; shell and network still ask",
            PermissionMode::Manual => "ask before file, shell, and network actions",
            PermissionMode::BypassPermissions => "run all tools without approval prompts",
            PermissionMode::Auto => {
                "accept routine file/network actions; shell asks unless clearly read-only"
            }
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase().replace('_', "-");
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LspServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub language_id: Option<String>,
    #[serde(default)]
    pub initialization_options: serde_json::Value,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PlatformConfig {
    /// Platform root URL (normalized). Feeds the skills REST client and the
    /// workspace inbox poller — never the LLM route (that's `base_url`).
    #[serde(default)]
    pub base_url: String,
    /// Personal platform API token (secret — never logged). Static: expiry
    /// and rotation are handled on the platform side (`auth` app
    /// `api-keys/rotate`). Falls back to `$CRAFTSMAN_PLATFORM_TOKEN` when empty.
    #[serde(default)]
    pub api_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::provider_config::unique_map"
    )]
    pub providers: BTreeMap<String, crate::provider_config::Provider>,
    #[serde(default = "default_agent")]
    pub agent: String,

    #[serde(default)]
    pub platform: PlatformConfig,
    #[serde(default = "default_locale")]
    pub locale: String,

    #[serde(default)]
    pub mcp_servers: Vec<serde_json::Value>,
    #[serde(default)]
    pub lsp_servers: BTreeMap<String, LspServerConfig>,
    #[serde(default)]
    pub shell_allow_list: Vec<String>,
    #[serde(default)]
    pub auto_approve: bool,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    #[serde(default = "default_max_parallel_agents")]
    pub max_parallel_agents: usize,

    /// Every field this struct doesn't model (legacy keys among them).
    /// BTreeMap keeps the on-disk order stable across round-trips.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            model: None,
            default_model: None,
            providers: BTreeMap::new(),
            agent: default_agent(),
            platform: PlatformConfig::default(),
            locale: default_locale(),
            mcp_servers: Vec::new(),
            lsp_servers: BTreeMap::new(),
            shell_allow_list: Vec::new(),
            auto_approve: false,
            permission_mode: PermissionMode::Manual,
            max_parallel_agents: default_max_parallel_agents(),
            extra: BTreeMap::new(),
        }
    }
}

fn default_agent() -> String {
    "craftsman".into()
}

fn default_locale() -> String {
    "en".into()
}

const fn default_max_parallel_agents() -> usize {
    5
}

/// Expand `${ENV_VAR}` placeholders in a config value. Only `api_key` and
/// MCP server header values are expansion sites (deliberately narrow —
/// blanket expansion would be surprising). A missing variable is an error,
/// not an empty string: sending a literal `${VAR}` bearer would fail with a
/// confusing 401 instead of a clear startup message.
pub fn expand_env_placeholders(raw: &str) -> anyhow::Result<String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            anyhow::bail!("unterminated ${{ placeholder in `{raw}`");
        };
        let name = &after[..end];
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            anyhow::bail!("invalid placeholder name `{name}` in `{raw}`");
        }
        let value = std::env::var(name).map_err(|_| {
            anyhow::anyhow!("environment variable {name} (from `{raw}`) is not set")
        })?;
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

pub fn config_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("CRAFTSMAN_CONFIG_DIR") {
        return PathBuf::from(path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".craftsman")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// The shared skills directory (`~/.craftsman/skills`), where platform-installed
/// skills live.
pub fn skills_dir() -> PathBuf {
    config_dir().join("skills")
}

#[cfg(unix)]
fn write_config_file(
    dir: &std::path::Path,
    path: &std::path::Path,
    body: &[u8],
) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temp = dir.join(format!(
        ".config.json.tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(body)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        crate::platform_os::apply_config_permissions(dir, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(windows)]
fn write_config_file(
    dir: &std::path::Path,
    path: &std::path::Path,
    body: &[u8],
) -> std::io::Result<()> {
    // Files under the user profile inherit its ACL on Windows. Keep the write
    // direct until the OS boundary grows an atomic ReplaceFileW helper.
    std::fs::write(path, body)?;
    crate::platform_os::apply_config_permissions(dir, path)
}

impl Config {
    pub fn load() -> anyhow::Result<Option<Config>> {
        let path = config_path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)?;
        let cfg: Config = serde_json::from_str(&raw).map_err(|e| {
            anyhow::anyhow!(
                "could not parse {}: invalid configuration at line {}, column {}",
                path.display(),
                e.line(),
                e.column()
            )
        })?;
        Ok(Some(cfg))
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        let path = config_path();
        let body = serde_json::to_string_pretty(&self)? + "\n";
        write_config_file(&dir, &path, body.as_bytes())?;
        Ok(())
    }

    pub fn agent_dir(&self) -> PathBuf {
        config_dir().join("agents").join(&self.agent)
    }

    /// Per-model constructor/request overrides.
    ///
    /// The on-disk shape is `model_profiles[model] = { ... }`. Returning an
    /// owned map keeps profile application isolated: provider defaults can be
    /// layered underneath it without mutating the loaded configuration.
    pub fn model_profile(&self, model: &str) -> BTreeMap<String, serde_json::Value> {
        self.extra
            .get("model_profiles")
            .and_then(serde_json::Value::as_object)
            .and_then(|profiles| profiles.get(model))
            .and_then(serde_json::Value::as_object)
            .map(|profile| {
                profile
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Web-search provider API key (secret — never logged). Read from `extra`.
    /// Returns `None` when absent, non-string, or empty.
    pub fn search_api_key(&self) -> Option<&str> {
        self.extra
            .get("search_api_key")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    }

    /// Web-search provider name (default `"brave"`). Read from `extra`.
    pub fn search_provider(&self) -> &str {
        self.extra
            .get("search_provider")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("brave")
    }

    /// Built-in language-server defaults merged with config overrides.
    /// Servers are never installed automatically; unavailable commands remain
    /// visible in `/lsp` and return a clear tool error.
    pub fn effective_lsp_servers(&self) -> BTreeMap<String, LspServerConfig> {
        let mut servers = BTreeMap::from([
            (
                "rust".to_string(),
                LspServerConfig {
                    command: "rust-analyzer".to_string(),
                    args: Vec::new(),
                    extensions: vec!["rs".to_string()],
                    language_id: Some("rust".to_string()),
                    initialization_options: serde_json::Value::Null,
                    enabled: true,
                },
            ),
            (
                "typescript".to_string(),
                LspServerConfig {
                    command: "typescript-language-server".to_string(),
                    args: vec!["--stdio".to_string()],
                    extensions: vec![
                        "ts".to_string(),
                        "tsx".to_string(),
                        "js".to_string(),
                        "jsx".to_string(),
                    ],
                    language_id: None,
                    initialization_options: serde_json::Value::Null,
                    enabled: true,
                },
            ),
            (
                "python".to_string(),
                LspServerConfig {
                    command: "pyright-langserver".to_string(),
                    args: vec!["--stdio".to_string()],
                    extensions: vec!["py".to_string()],
                    language_id: Some("python".to_string()),
                    initialization_options: serde_json::Value::Null,
                    enabled: true,
                },
            ),
            (
                "go".to_string(),
                LspServerConfig {
                    command: "gopls".to_string(),
                    args: Vec::new(),
                    extensions: vec!["go".to_string()],
                    language_id: Some("go".to_string()),
                    initialization_options: serde_json::Value::Null,
                    enabled: true,
                },
            ),
        ]);
        for (name, server) in &self.lsp_servers {
            servers.insert(name.clone(), server.clone());
        }
        servers.retain(|_, server| server.enabled);
        for server in servers.values_mut() {
            server.extensions = server
                .extensions
                .iter()
                .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
                .filter(|extension| !extension.is_empty())
                .collect();
        }
        servers
    }

    /// Effective cap for concurrently running delegated tasks. Values are
    /// clamped so a malformed config cannot disable delegation or exhaust the
    /// runtime with an unbounded fan-out.
    pub fn effective_max_parallel_agents(&self) -> usize {
        self.max_parallel_agents.clamp(1, 32)
    }

    /// Pre-approve network-egress tools (`web_fetch`/`web_search`) without a
    /// prompt. Read from `extra["auto_approve_web"]`; defaults to false so
    /// network egress stays gated (rule #6).
    pub fn auto_approve_web(&self) -> bool {
        self.extra
            .get("auto_approve_web")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// The resolved platform API token: the configured value when present,
    /// otherwise `$CRAFTSMAN_PLATFORM_TOKEN`. `${ENV_VAR}` placeholders inside
    /// the configured value are expanded. Empty result ⇒ platform features
    /// (skills, inbox) are simply unconfigured.
    pub fn platform_api_token(&self) -> String {
        if self.platform.api_token.trim().is_empty() {
            return std::env::var("CRAFTSMAN_PLATFORM_TOKEN")
                .unwrap_or_default()
                .trim()
                .to_string();
        }
        expand_env_placeholders(&self.platform.api_token).unwrap_or_default()
    }

    /// True when both platform coordinates are configured (REST features and
    /// the inbox poller engage).
    pub fn platform_configured(&self) -> bool {
        !self.platform.base_url.is_empty() && !self.platform_api_token().is_empty()
    }

    /// True when the config file still carries login-era fields (no longer
    /// modeled; round-tripped through `extra`). Triggers the one-time
    /// migration notice at startup.
    pub fn has_legacy_credentials(&self) -> bool {
        [
            "auth_token",
            "refresh_token",
            "user_email",
            "platform_base_url",
        ]
        .iter()
        .any(|key| self.extra.contains_key(*key))
    }

    /// Token-limit context window for `model`, from
    /// `model_profiles.<model>.context_window` — the config fallback for the
    /// compaction token-trigger (platform metadata is no longer fetched).
    pub fn model_context_window(&self, model: &str) -> Option<u64> {
        if !self.providers.is_empty() {
            return crate::provider_config::selection(self, model)
                .ok()?
                .entry?
                .settings
                .get("context_window")
                .and_then(serde_json::Value::as_u64)
                .filter(|window| *window > 0);
        }
        self.model_profile(model)
            .get("context_window")
            .and_then(serde_json::Value::as_u64)
            .filter(|window| *window > 0)
    }

    /// Read one value from the nested `voice` object.
    #[allow(dead_code)]
    pub fn voice_setting(&self, key: &str) -> Option<&serde_json::Value> {
        self.extra
            .get("voice")
            .and_then(serde_json::Value::as_object)
            .and_then(|voice| voice.get(key))
    }

    /// Update one value in the nested `voice` object without replacing unknown
    /// keys owned by another CLI version.
    pub fn set_voice_setting(&mut self, key: &str, value: serde_json::Value) {
        let voice = self
            .extra
            .entry("voice".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !voice.is_object() {
            *voice = serde_json::json!({});
        }
        voice
            .as_object_mut()
            .expect("voice setting normalized to an object")
            .insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fields_round_trip() {
        let raw = r#"{
            "base_url": "http://x", "api_key": "k", "model": "m",
            "model_profiles": {"m": {"temperature": 0.2}},
            "langfuse_enabled": true,
            "auto_compact_threshold": 0.8
        }"#;
        let cfg: Config = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.base_url, "http://x");
        assert!(cfg.extra.contains_key("model_profiles"));
        let out = serde_json::to_string(&cfg).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back["model_profiles"]["m"]["temperature"], 0.2);
        assert_eq!(back["auto_compact_threshold"], 0.8);
    }

    #[test]
    fn model_profile_matches_python_config_shape_and_returns_a_copy() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "model_profiles": {
                    "model-a": {"temperature": 0.2, "max_tokens": 4096}
                }
            }"#,
        )
        .unwrap();
        let mut profile = cfg.model_profile("model-a");
        assert_eq!(profile["temperature"], 0.2);
        assert_eq!(profile["max_tokens"], 4096);
        profile.insert("temperature".into(), serde_json::json!(0.9));
        assert_eq!(cfg.model_profile("model-a")["temperature"], 0.2);
        assert!(cfg.model_profile("missing").is_empty());
    }

    #[test]
    fn web_tool_config_accessors() {
        let raw = r#"{
            "search_api_key": "bsk-xyz",
            "search_provider": "brave",
            "auto_approve_web": true
        }"#;
        let cfg: Config = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.search_api_key(), Some("bsk-xyz"));
        assert_eq!(cfg.search_provider(), "brave");
        assert!(cfg.auto_approve_web());

        // Defaults: no key, provider "brave", gate on (auto-approve off).
        let empty: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.search_api_key(), None);
        assert_eq!(empty.search_provider(), "brave");
        assert!(!empty.auto_approve_web());

        // Empty string key is treated as absent.
        let blank: Config = serde_json::from_str(r#"{"search_api_key": ""}"#).unwrap();
        assert_eq!(blank.search_api_key(), None);
    }

    #[test]
    fn voice_settings_preserve_unknown_nested_keys() {
        let mut cfg: Config = serde_json::from_str(
            r#"{
                "voice": {
                    "enabled": true,
                    "model": "base-q5_1",
                    "python_only": {"keep": 1}
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            cfg.voice_setting("enabled").and_then(|v| v.as_bool()),
            Some(true)
        );
        cfg.set_voice_setting("model", serde_json::json!("small-q5_1"));
        let encoded = serde_json::to_value(&cfg).unwrap();
        assert_eq!(encoded["voice"]["model"], "small-q5_1");
        assert_eq!(encoded["voice"]["python_only"]["keep"], 1);
    }

    #[test]
    fn permission_modes_round_trip_and_parse() {
        for mode in PermissionMode::ALL {
            let encoded = serde_json::to_string(&mode).unwrap();
            let decoded: PermissionMode = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, mode);
            assert_eq!(PermissionMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(
            PermissionMode::parse("accept_edits"),
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(PermissionMode::parse("unknown"), None);
    }

    #[test]
    fn parallel_agent_limit_defaults_and_clamps() {
        let configured: Config = serde_json::from_str(r#"{"max_parallel_agents": 9}"#).unwrap();
        assert_eq!(configured.effective_max_parallel_agents(), 9);
        let zero: Config = serde_json::from_str(r#"{"max_parallel_agents": 0}"#).unwrap();
        assert_eq!(zero.effective_max_parallel_agents(), 1);
        let huge: Config = serde_json::from_str(r#"{"max_parallel_agents": 999}"#).unwrap();
        assert_eq!(huge.effective_max_parallel_agents(), 32);
    }

    #[test]
    fn env_placeholder_expansion() {
        crate::test_environment::run(crate::test_environment::Case::Config);
    }

    pub(super) fn env_placeholder_expansion_body() {
        assert_eq!(
            expand_env_placeholders("Bearer ${CRAFTSMAN_TEST_TOKEN}").unwrap(),
            "Bearer tok-123"
        );
        assert_eq!(
            expand_env_placeholders("${CRAFTSMAN_TEST_TOKEN}").unwrap(),
            "tok-123"
        );
        // No placeholder: passthrough.
        assert_eq!(expand_env_placeholders("plain-key").unwrap(), "plain-key");
        // Missing variable: error, not empty.
        assert!(expand_env_placeholders("${CRAFTSMAN_TEST_MISSING_9}").is_err());
        // Malformed: unterminated / bad name.
        assert!(expand_env_placeholders("${CRAFTSMAN_TEST_TOKEN").is_err());
        assert!(expand_env_placeholders("${bad-name}").is_err());
    }

    #[test]
    fn platform_config_parses_and_defaults() {
        let raw = r#"{
            "platform": {
                "base_url": "https://platform.example.com",
                "api_token": "mpat_x"
            }
        }"#;
        let cfg: Config = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.platform.base_url, "https://platform.example.com");
        assert_eq!(cfg.platform.api_token, "mpat_x");
        assert_eq!(cfg.platform_api_token(), "mpat_x");
        assert!(cfg.platform_configured());

        let empty: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.platform, PlatformConfig::default());
        assert!(!empty.platform_configured());
        assert!(!empty.has_legacy_credentials());
    }

    #[test]
    fn legacy_login_fields_round_trip_and_warn() {
        let raw = r#"{
            "platform_base_url": "https://old.example.com",
            "auth_token": "jwt",
            "refresh_token": "rt",
            "user_email": "user@example.com"
        }"#;
        let cfg: Config = serde_json::from_str(raw).unwrap();
        assert!(cfg.has_legacy_credentials());
        // The unknown fields survive a round-trip via `extra`.
        let out = serde_json::to_string(&cfg).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back["auth_token"], "jwt");
        assert_eq!(back["platform_base_url"], "https://old.example.com");
    }

    #[test]
    fn context_window_reads_model_profile() {
        let cfg: Config =
            serde_json::from_str(r#"{"model_profiles": {"model-a": {"context_window": 131072}}}"#)
                .unwrap();
        assert_eq!(cfg.model_context_window("model-a"), Some(131_072));
        assert_eq!(cfg.model_context_window("missing"), None);
        let zero: Config =
            serde_json::from_str(r#"{"model_profiles": {"m": {"context_window": 0}}}"#).unwrap();
        assert_eq!(zero.model_context_window("m"), None);
    }

    #[test]
    fn defaults_match_python() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.agent, "craftsman");
        assert_eq!(cfg.locale, "en");
        assert_eq!(cfg.permission_mode, PermissionMode::Manual);
        assert_eq!(cfg.max_parallel_agents, 5);
        assert_eq!(cfg.effective_max_parallel_agents(), 5);
        assert!(!cfg.auto_approve);

        let native = Config::default();
        assert_eq!(native.agent, "craftsman");
        assert_eq!(native.locale, "en");
        assert_eq!(native.permission_mode, PermissionMode::Manual);
        assert_eq!(native.max_parallel_agents, 5);
    }
}

#[cfg(test)]
pub(crate) fn config_environment_fixture_for_tests() {
    tests::env_placeholder_expansion_body();
}
