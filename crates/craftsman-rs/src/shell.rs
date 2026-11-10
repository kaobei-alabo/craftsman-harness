//! Settings of the `execute` tool and the `!cmd` escape: the `shell` object
//! of `~/.craftsman/config.json` (user-level config; Craftsman has no project-level
//! config file). Unknown keys stay in `Config::extra`, so the object
//! round-trips untouched.
//!
//! The environment scrub covers every command the agent starts through the
//! shell (`execute` and the `!cmd` escape). Stdio MCP servers and language
//! servers are out of scope: Craftsman starts them from the user's own config,
//! an MCP server usually needs the credentials its `env` names, and a
//! language server needs the toolchain environment; their containment is
//! P7a's.
//!
//! Every model-visible setting is OFF by default (harness plan, P2b: the
//! flags stay off until a live scoreboard run clears them in P3b).

use crate::config::Config;
use serde_json::Value;

/// Default limit of one `execute` call, in seconds (today's fixed value).
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Upper bound of `shell.timeout_secs`.
pub const MAX_CONFIG_TIMEOUT_SECS: u64 = 3600;
/// Upper bound of the per-call `timeout` argument, unless the configured
/// default is higher (then the default is the bound).
pub const MAX_CALL_TIMEOUT_SECS: u64 = 600;

/// How the `execute` tool words a timed-out (and, from P2b deliverable 3, an
/// oversized) result. Model-visible, so `Legacy` is the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// Today's text: a timeout returns only `Error: command timed out after
    /// <limit>s` and discards the output.
    #[default]
    Legacy,
    /// `shell.output_format = "head_tail"`: a timeout returns the output
    /// printed before it, after an error line naming the limit.
    HeadTail,
}

/// The resolved `shell` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSettings {
    /// `shell.process_group` (default false, Unix only): run each command in
    /// a process group of its own, killed as a whole on timeout and on drop.
    /// Stays off until P2c routes SIGHUP/SIGTERM through `shutdown()`; see
    /// `platform_os::SpawnOptions::process_group`.
    pub process_group: bool,
    /// `shell.timeout_secs` (default 120, clamped to 1..=3600): the limit of
    /// a call that names none. Model-visible only through the timeout text,
    /// which names the limit; at the default the text is today's.
    pub timeout_secs: u64,
    /// `shell.timeout_param` (default false): expose a per-call `timeout`
    /// argument in the execute tool's schema and honour it. Model-visible.
    pub timeout_param: bool,
    /// `shell.output_format` (default `"legacy"`). Model-visible.
    pub output_format: OutputFormat,
    /// `shell.clean_output` (default false): strip ANSI escape sequences
    /// and resolve carriage-return overwrites in captured output. Model-
    /// visible (a coloured or progress-bar result reads differently).
    pub clean_output: bool,
    /// `shell.powershell_utf8` (default false; Windows only): put
    /// `[Console]::OutputEncoding = [Text.Encoding]::UTF8;` at the front of
    /// the PowerShell `-Command` string, so text PowerShell prints reaches
    /// Craftsman as UTF-8 instead of the OEM code page a `CREATE_NO_WINDOW`
    /// child starts at (A32). Model-visible on Windows (non-ASCII output
    /// changes from lossy to correct), so behind a flag; ASCII is unchanged.
    pub powershell_utf8: bool,
    /// `shell.env_scrub` (default TRUE): remove secret-named variables from
    /// the command's environment and set the hints in [`ENV_HINTS`]. This
    /// changes what commands see, not what the model is told. `false`
    /// restores today's fully inherited environment (and no hints).
    pub env_scrub: bool,
    /// `shell.env_pass`: names let through the scrub. Read from the
    /// user-level `~/.craftsman/config.json` only, the one config file Craftsman has;
    /// a project-level config must never be able to set it, or a repository
    /// could re-enable its own access to the user's secrets.
    pub env_pass: Vec<String>,
    /// The variables Craftsman's own credentials come from, removed even when
    /// their names look harmless: `CRAFTSMAN_PLATFORM_TOKEN` and every
    /// `${NAME}` placeholder of `api_key`, `platform.api_token` and the MCP
    /// servers' `headers` and `env`. Filled by [`ShellSettings::from_config`].
    pub secret_vars: Vec<String>,
}

/// Hints set in a scrubbed environment: no colour codes, no pager, and no
/// interactive git credential prompt that would hang a command with no stdin.
pub const ENV_HINTS: &[(&str, &str)] = &[
    ("NO_COLOR", "1"),
    ("PAGER", "cat"),
    ("GIT_TERMINAL_PROMPT", "0"),
];

/// Variables always kept, whatever the deny patterns say: what toolchains,
/// shells, SSH agents and proxies need. Compared without regard to case.
pub const ENV_PASS_LIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "TMP",
    "TEMP",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "GOPATH",
    "GOROOT",
    "JAVA_HOME",
    "NVM_DIR",
    "PYENV_ROOT",
    "VIRTUAL_ENV",
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    // Windows
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "PSMODULEPATH",
    "USERNAME",
    "COMPUTERNAME",
];

/// Whether a variable name looks like a secret: `*_KEY`, `*_TOKEN`,
/// `*_SECRET`, `*PASSWORD*`, `AWS_*` or `DATABASE_URL`, without regard to
/// case.
pub fn secret_named(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.ends_with("_KEY")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_SECRET")
        || upper.contains("PASSWORD")
        || upper.starts_with("AWS_")
        || upper == "DATABASE_URL"
}

impl Default for ShellSettings {
    fn default() -> Self {
        Self {
            process_group: false,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            timeout_param: false,
            output_format: OutputFormat::Legacy,
            clean_output: false,
            powershell_utf8: false,
            env_scrub: true,
            env_pass: Vec::new(),
            secret_vars: vec!["CRAFTSMAN_PLATFORM_TOKEN".into()],
        }
    }
}

impl ShellSettings {
    pub fn from_config(cfg: &Config) -> Self {
        let mut settings = Self::from_value(cfg.extra.get("shell"));
        let mut sources = vec![cfg.api_key.clone(), cfg.platform.api_token.clone()];
        sources.extend(
            cfg.providers
                .values()
                .map(|provider| provider.api_key.clone()),
        );
        for server in &cfg.mcp_servers {
            for field in ["headers", "env"] {
                if let Some(map) = server.get(field).and_then(Value::as_object) {
                    sources.extend(map.values().filter_map(Value::as_str).map(str::to_string));
                }
            }
        }
        for source in &sources {
            for name in placeholder_names(source) {
                if !settings.secret_vars.contains(&name) {
                    settings.secret_vars.push(name);
                }
            }
        }
        settings
    }

    /// Parse the `shell` object; a missing key, a wrong type or a value that
    /// is not an object falls back to the default.
    pub fn from_value(value: Option<&Value>) -> Self {
        let mut settings = Self::default();
        let Some(shell) = value.and_then(Value::as_object) else {
            return settings;
        };
        if let Some(on) = shell.get("process_group").and_then(Value::as_bool) {
            settings.process_group = on;
        }
        if let Some(secs) = shell.get("timeout_secs").and_then(Value::as_u64) {
            settings.timeout_secs = secs.clamp(1, MAX_CONFIG_TIMEOUT_SECS);
        }
        if let Some(on) = shell.get("timeout_param").and_then(Value::as_bool) {
            settings.timeout_param = on;
        }
        if let Some(on) = shell.get("clean_output").and_then(Value::as_bool) {
            settings.clean_output = on;
        }
        if let Some(on) = shell.get("powershell_utf8").and_then(Value::as_bool) {
            settings.powershell_utf8 = on;
        }
        if let Some(on) = shell.get("env_scrub").and_then(Value::as_bool) {
            settings.env_scrub = on;
        }
        if let Some(names) = shell.get("env_pass").and_then(Value::as_array) {
            settings.env_pass = names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
        }
        if let Some(format) = shell.get("output_format").and_then(Value::as_str) {
            settings.output_format = match format {
                "head_tail" => OutputFormat::HeadTail,
                _ => OutputFormat::Legacy,
            };
        }
        settings
    }

    /// The largest per-call `timeout` honoured.
    pub fn max_call_timeout_secs(&self) -> u64 {
        MAX_CALL_TIMEOUT_SECS.max(self.timeout_secs)
    }

    /// The limit of one call: the per-call `timeout` argument when
    /// `shell.timeout_param` is on and it is a positive number (rounded up
    /// to whole seconds, clamped to [`Self::max_call_timeout_secs`]),
    /// otherwise `shell.timeout_secs`.
    pub fn call_timeout_secs(&self, argument: Option<&Value>) -> u64 {
        if !self.timeout_param {
            return self.timeout_secs;
        }
        let requested = match argument {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
            _ => None,
        };
        match requested {
            Some(secs) if secs.is_finite() && secs > 0.0 => {
                (secs.ceil() as u64).clamp(1, self.max_call_timeout_secs())
            }
            _ => self.timeout_secs,
        }
    }

    /// Whether the scrub removes `name` from a command's environment.
    pub fn scrubs(&self, name: &str) -> bool {
        self.env_scrub && self.scrubs_enabled(name)
    }

    fn scrubs_enabled(&self, name: &str) -> bool {
        let same = |other: &String| other.eq_ignore_ascii_case(name);
        if self.env_pass.iter().any(same) {
            return false;
        }
        if ENV_PASS_LIST
            .iter()
            .any(|kept| kept.eq_ignore_ascii_case(name))
        {
            return false;
        }
        secret_named(name) || self.secret_vars.iter().any(same)
    }

    /// How a command is spawned: the group flag and the environment plan for
    /// the current process environment.
    pub(crate) fn spawn_options(&self) -> crate::platform_os::SpawnOptions {
        self.spawn_options_for(std::env::vars_os().map(|(name, _)| name))
    }

    /// Headless keeps the captured credential sources even for names that the
    /// legacy toolchain pass list normally retains. No source can undo scrub.
    pub(crate) fn headless_spawn_options(&self) -> crate::platform_os::SpawnOptions {
        self.headless_spawn_options_for(std::env::vars_os().map(|(name, _)| name))
    }

    pub(crate) fn headless_spawn_options_for(
        &self,
        names: impl Iterator<Item = std::ffi::OsString>,
    ) -> crate::platform_os::SpawnOptions {
        self.spawn_options_for_profile(names, true)
    }

    /// [`Self::spawn_options`] for a given list of variable names (tests).
    pub(crate) fn spawn_options_for(
        &self,
        names: impl Iterator<Item = std::ffi::OsString>,
    ) -> crate::platform_os::SpawnOptions {
        self.spawn_options_for_profile(names, false)
    }

    fn spawn_options_for_profile(
        &self,
        names: impl Iterator<Item = std::ffi::OsString>,
        headless: bool,
    ) -> crate::platform_os::SpawnOptions {
        let (env_remove, env_set) = if self.env_scrub || headless {
            // A name that is not valid Unicode cannot be matched; drop it.
            let remove = names
                .filter(|name| {
                    name.to_str().is_none_or(|name| {
                        if headless {
                            self.scrubs_enabled(name)
                                || self
                                    .secret_vars
                                    .iter()
                                    .any(|source| source.eq_ignore_ascii_case(name))
                        } else {
                            self.scrubs(name)
                        }
                    })
                })
                .collect();
            let set = ENV_HINTS
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect();
            (remove, set)
        } else {
            (Vec::new(), Vec::new())
        };
        crate::platform_os::SpawnOptions {
            process_group: self.process_group,
            env_remove,
            env_set,
            powershell_utf8: self.powershell_utf8,
        }
    }
}

/// The `NAME`s of the `${NAME}` placeholders in a config value.
fn placeholder_names(raw: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            names.push(name.to_string());
        }
        rest = &after[end + 1..];
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_scrub_cannot_retain_captured_credentials_via_legacy_pass_list() {
        let settings = ShellSettings {
            env_scrub: false,
            env_pass: vec!["PUBLIC_ADAPTER_SETTING".into()],
            secret_vars: vec!["LANG".into(), "INNOCUOUS_CREDENTIAL_SOURCE".into()],
            ..Default::default()
        };
        let names = || {
            [
                "LANG",
                "INNOCUOUS_CREDENTIAL_SOURCE",
                "SYNTHETIC_API_KEY",
                "PUBLIC_ADAPTER_SETTING",
                "PATH",
            ]
            .into_iter()
            .map(std::ffi::OsString::from)
        };
        assert!(settings.spawn_options_for(names()).env_remove.is_empty());
        let strict = settings.headless_spawn_options_for(names());
        assert_eq!(
            strict.env_remove,
            ["LANG", "INNOCUOUS_CREDENTIAL_SOURCE", "SYNTHETIC_API_KEY"]
                .map(std::ffi::OsString::from)
                .to_vec()
        );
        assert!(
            strict
                .env_set
                .iter()
                .any(|(name, value)| name == "NO_COLOR" && value == "1")
        );
    }

    use serde_json::json;

    #[test]
    fn defaults_keep_todays_behaviour() {
        let s = ShellSettings::from_value(None);
        assert!(!s.process_group);
        assert_eq!(s.timeout_secs, 120);
        assert!(!s.timeout_param);
        assert_eq!(s.output_format, OutputFormat::Legacy);
        assert!(
            s.env_scrub,
            "the scrub is on by default: the pass list exists"
        );
        assert!(!s.clean_output && !s.powershell_utf8);
        let on = ShellSettings::from_value(Some(
            &json!({"clean_output": true, "powershell_utf8": true}),
        ));
        assert!(on.clean_output && on.powershell_utf8);
        assert_eq!(s, ShellSettings::from_value(Some(&json!("pwsh"))));
        assert_eq!(
            s,
            ShellSettings::from_value(Some(&json!({"process_group": "yes"})))
        );
    }

    #[test]
    fn timeouts_parse_and_clamp() {
        let s = ShellSettings::from_value(Some(&json!({"timeout_secs": 0})));
        assert_eq!(s.timeout_secs, 1);
        let s = ShellSettings::from_value(Some(&json!({"timeout_secs": 99999})));
        assert_eq!(s.timeout_secs, MAX_CONFIG_TIMEOUT_SECS);
        assert_eq!(s.max_call_timeout_secs(), MAX_CONFIG_TIMEOUT_SECS);

        // Flag off: the argument is ignored.
        let off = ShellSettings::from_value(Some(&json!({"timeout_secs": 30})));
        assert_eq!(off.call_timeout_secs(Some(&json!(5))), 30);

        let on = ShellSettings::from_value(Some(&json!({"timeout_param": true})));
        assert_eq!(on.call_timeout_secs(None), 120);
        assert_eq!(on.call_timeout_secs(Some(&json!(10))), 10);
        assert_eq!(on.call_timeout_secs(Some(&json!(2.5))), 3);
        assert_eq!(on.call_timeout_secs(Some(&json!("7"))), 7);
        assert_eq!(on.call_timeout_secs(Some(&json!(100000))), 600);
        assert_eq!(on.call_timeout_secs(Some(&json!(0))), 120);
        assert_eq!(on.call_timeout_secs(Some(&json!(-3))), 120);
        assert_eq!(on.call_timeout_secs(Some(&json!(true))), 120);
    }

    #[test]
    fn output_format_parses() {
        let s = ShellSettings::from_value(Some(&json!({"output_format": "head_tail"})));
        assert_eq!(s.output_format, OutputFormat::HeadTail);
        let s = ShellSettings::from_value(Some(&json!({"output_format": "other"})));
        assert_eq!(s.output_format, OutputFormat::Legacy);
    }

    #[test]
    fn deny_patterns_match_secret_names_without_regard_to_case() {
        for name in [
            "OPENAI_API_KEY",
            "github_token",
            "CLIENT_SECRET",
            "DB_PASSWORD_FILE",
            "MYPASSWORD",
            "AWS_PROFILE",
            "DATABASE_URL",
        ] {
            assert!(secret_named(name), "{name}");
        }
        for name in [
            "PATH",
            "KEYCHAIN",
            "TOKENIZERS_PARALLELISM",
            "SSH_AUTH_SOCK",
            "DATABASE",
        ] {
            assert!(!secret_named(name), "{name}");
        }
    }

    #[test]
    fn the_scrub_keeps_the_pass_list_and_env_pass_and_drops_configured_keys() {
        let mut s = ShellSettings::from_value(Some(&json!({
            "env_pass": ["CRAFTSMAN_TEST_PASSED_KEY"]
        })));
        s.secret_vars.push("CRAFTSMAN_TEST_LLM_VAR".into());
        assert!(s.scrubs("CRAFTSMAN_TEST_FAKE_API_KEY"));
        assert!(
            s.scrubs("CRAFTSMAN_TEST_LLM_VAR"),
            "configured key variable"
        );
        assert!(s.scrubs("CRAFTSMAN_PLATFORM_TOKEN"));
        assert!(!s.scrubs("craftsman_test_passed_key"), "env_pass, any case");
        for kept in [
            "PATH",
            "HOME",
            "CARGO_HOME",
            "SSH_AUTH_SOCK",
            "https_proxy",
            "TERM",
        ] {
            assert!(!s.scrubs(kept), "{kept}");
        }
        let off = ShellSettings::from_value(Some(&json!({"env_scrub": false})));
        assert!(!off.scrubs("CRAFTSMAN_TEST_FAKE_API_KEY"));
    }

    #[test]
    fn spawn_options_list_the_names_to_remove_and_the_hints() {
        let s = ShellSettings::default();
        let names = [
            "PATH",
            "CRAFTSMAN_TEST_FAKE_API_KEY",
            "HOME",
            "CRAFTSMAN_TEST_FAKE_TOKEN",
        ]
        .into_iter()
        .map(std::ffi::OsString::from);
        let options = s.spawn_options_for(names);
        assert_eq!(
            options.env_remove,
            vec![
                std::ffi::OsString::from("CRAFTSMAN_TEST_FAKE_API_KEY"),
                std::ffi::OsString::from("CRAFTSMAN_TEST_FAKE_TOKEN")
            ]
        );
        assert!(options.env_set.contains(&("NO_COLOR".into(), "1".into())));
        let off = ShellSettings::from_value(Some(&json!({"env_scrub": false})));
        let options = off.spawn_options_for(["CRAFTSMAN_TEST_FAKE_API_KEY".into()].into_iter());
        assert!(options.env_remove.is_empty() && options.env_set.is_empty());
    }

    #[test]
    fn configured_key_variables_come_from_placeholders() {
        let cfg: Config = serde_json::from_value(json!({
            "api_key": "${CRAFTSMAN_TEST_LLM_VAR}",
            "platform": {"api_token": "pre-${CRAFTSMAN_TEST_PLATFORM_VAR}-post"},
            "mcp_servers": [
                {"name": "a", "url": "http://x", "headers": {"Authorization": "Bearer ${CRAFTSMAN_TEST_MCP_VAR}"}},
                {"name": "b", "transport": "stdio", "command": "x", "env": {"T": "${CRAFTSMAN_TEST_STDIO_VAR}"}}
            ]
        }))
        .unwrap();
        let s = ShellSettings::from_config(&cfg);
        for name in [
            "CRAFTSMAN_PLATFORM_TOKEN",
            "CRAFTSMAN_TEST_LLM_VAR",
            "CRAFTSMAN_TEST_PLATFORM_VAR",
            "CRAFTSMAN_TEST_MCP_VAR",
            "CRAFTSMAN_TEST_STDIO_VAR",
        ] {
            assert!(s.secret_vars.iter().any(|v| v == name), "{name}");
        }
    }

    #[test]
    fn process_group_flag_parses() {
        let s = ShellSettings::from_value(Some(&json!({"process_group": true})));
        assert!(s.process_group);
    }
}
