//! Explicit-only, immutable invocation selectors (headless contract R1).
//!
//! This module never reads Config, ApprovalState, persisted grants or the
//! process environment. It is a policy foundation, not an OS sandbox or a
//! filesystem resolver. Exact path helpers must receive normalized paths from
//! the future confined backend; a path allow never grants a tool or overrides
//! that backend's hard-denied config/credential paths.

use serde_json::Value;
use std::collections::BTreeSet;
use thiserror::Error;

const MAX_RULE_CHARS: usize = 4096;
const MAX_RULES: usize = 256;
const MAX_PREFIX_TOKENS: usize = 16;
const MAX_TOKEN_CHARS: usize = 256;
const MAX_ENV_NAMES: usize = 64;
const MAX_ENV_NAME_BYTES: usize = 128;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "ls",
    "glob",
    "grep",
    "execute",
    "web_fetch",
    "web_search",
    "write_todos",
    "task",
    "send_message",
    "gadget_list",
    "gadget_call",
    "ask_user",
];
const WRAPPERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "cmd",
    "cmd.exe",
    "powershell",
    "powershell.exe",
    "pwsh",
    "env",
    "sudo",
    "doas",
    "xargs",
    "exec",
];
const GIT_READ: &[&str] = &["status", "diff", "log", "show", "rev-parse", "ls-files"];
const SHELL_META: &[char] = &[';', '&', '|', '$', '`', '>', '<', '(', ')', '\n', '\r'];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Manual,
    Plan,
}

impl Mode {
    pub(crate) fn parse(value: &str) -> Result<Self, UsageError> {
        match value {
            "manual" => Ok(Self::Manual),
            "plan" => Ok(Self::Plan),
            _ => Err(UsageError::Mode),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum OnUnapproved {
    #[default]
    Deny,
    Fail,
}

impl OnUnapproved {
    pub(crate) fn parse(value: &str) -> Result<Self, UsageError> {
        match value {
            "deny" => Ok(Self::Deny),
            "fail" => Ok(Self::Fail),
            _ => Err(UsageError::OnUnapproved),
        }
    }
}

/// Only explicit invocation arguments belong here; no ambient approval fields.
#[derive(Debug, Clone, Default)]
pub(crate) struct Selectors {
    pub(crate) mode: Mode,
    pub(crate) on_unapproved: OnUnapproved,
    pub(crate) allow: Vec<String>,
    pub(crate) deny: Vec<String>,
    pub(crate) env_pass: Vec<String>,
}

/// Safe usage diagnostics: never echo an untrusted rule or a credential name/value.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub(crate) enum UsageError {
    #[error("headless v1 permission mode must be manual or plan")]
    Mode,
    #[error("on-unapproved must be deny or fail")]
    OnUnapproved,
    #[error("invalid invocation permission selector")]
    Rule,
    #[error("this selector cannot grant a hard-denied capability")]
    ForbiddenAllow,
    #[error("command-prefix selectors are unsupported on this platform")]
    UnsupportedPrefix,
    #[error("too many invocation permission selectors")]
    TooManyRules,
    #[error("duplicate invocation permission selector")]
    DuplicateRule,
    #[error("invalid environment pass name")]
    EnvironmentName,
    #[error("too many environment pass names")]
    TooManyEnvironmentNames,
    #[error("duplicate or case-ambiguous environment pass name")]
    DuplicateEnvironmentName,
    #[error("credential-source variables cannot be environment pass overrides")]
    CredentialEnvironmentName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Selector {
    Tool(String),
    Exec(Vec<String>),
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    source: String,
    selector: Selector,
}

impl Rule {
    fn parse(raw: &str, direction: Direction) -> Result<Self, UsageError> {
        if raw.chars().count() > MAX_RULE_CHARS {
            return Err(UsageError::Rule);
        }
        let selector = if let Some(name) = raw.strip_prefix("tool:") {
            if !TOOLS.contains(&name) {
                return Err(UsageError::Rule);
            }
            if direction == Direction::Allow && name == "ask_user" {
                return Err(UsageError::ForbiddenAllow);
            }
            Selector::Tool(name.to_owned())
        } else if let Some(path) = raw.strip_prefix("path:") {
            if !relative_file(path) {
                return Err(UsageError::Rule);
            }
            if direction == Direction::Allow && hard_reserved_path(path) {
                return Err(UsageError::ForbiddenAllow);
            }
            Selector::Path(path.to_owned())
        } else if let Some(json) = raw.strip_prefix("exec:") {
            // Escaped Unicode tokens are fine; the selector's JSON wire is ASCII.
            if !json.is_ascii() {
                return Err(UsageError::Rule);
            }
            let tokens: Vec<String> = serde_json::from_str(json).map_err(|_| UsageError::Rule)?;
            if !(1..=MAX_PREFIX_TOKENS).contains(&tokens.len())
                || tokens.iter().any(|t| t.chars().count() > MAX_TOKEN_CHARS)
                || !plain_tokens(&tokens)
            {
                return Err(UsageError::Rule);
            }
            Selector::Exec(tokens)
        } else {
            return Err(UsageError::Rule);
        };
        Ok(Self {
            source: raw.to_owned(),
            selector,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrefixSupport {
    Posix,
    Unsupported,
}

impl PrefixSupport {
    fn native() -> Self {
        if crate::platform_os::shell_allow_list_supported() {
            Self::Posix
        } else {
            Self::Unsupported
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolDecision {
    Allow,
    Deny,
    Unapproved,
}

/// Consumes selectors so a later config/mode mutation cannot widen this policy.
#[derive(Debug, Clone)]
pub(crate) struct InvocationPermissions {
    mode: Mode,
    on_unapproved: OnUnapproved,
    allow: Vec<Rule>,
    deny: Vec<Rule>,
    env_pass: Vec<String>,
}

impl InvocationPermissions {
    /// `credential_names` contains resolved credential *sources*, never values.
    pub(crate) fn from_explicit(
        input: Selectors,
        credential_names: &[String],
    ) -> Result<Self, UsageError> {
        Self::with_support(input, credential_names, PrefixSupport::native())
    }

    fn with_support(
        input: Selectors,
        credential_names: &[String],
        support: PrefixSupport,
    ) -> Result<Self, UsageError> {
        let parse = |raw: Vec<String>, direction| {
            if raw.len() > MAX_RULES {
                return Err(UsageError::TooManyRules);
            }
            let mut seen = BTreeSet::new();
            raw.into_iter()
                .map(|source| {
                    if !seen.insert(source.clone()) {
                        return Err(UsageError::DuplicateRule);
                    }
                    let rule = Rule::parse(&source, direction)?;
                    if support == PrefixSupport::Unsupported
                        && matches!(rule.selector, Selector::Exec(_))
                    {
                        return Err(UsageError::UnsupportedPrefix);
                    }
                    Ok(rule)
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let allow = parse(input.allow, Direction::Allow)?;
        let deny = parse(input.deny, Direction::Deny)?;
        validate_env_pass(&input.env_pass, credential_names)?;
        Ok(Self {
            mode: input.mode,
            on_unapproved: input.on_unapproved,
            allow,
            deny,
            env_pass: input.env_pass,
        })
    }

    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    pub(crate) fn on_unapproved(&self) -> OnUnapproved {
        self.on_unapproved
    }

    pub(crate) fn allow_rules(&self) -> Vec<&str> {
        self.allow.iter().map(|r| r.source.as_str()).collect()
    }

    pub(crate) fn deny_rules(&self) -> Vec<&str> {
        self.deny.iter().map(|r| r.source.as_str()).collect()
    }

    pub(crate) fn env_pass(&self) -> &[String] {
        &self.env_pass
    }

    /// File selectors for confined-backend preflight and conservative deny matching.
    /// The bool is true for an allow; callers still cannot override hard denies.
    pub(crate) fn file_selectors(&self) -> impl Iterator<Item = (&str, bool)> {
        self.allow
            .iter()
            .map(|r| (r, true))
            .chain(self.deny.iter().map(|r| (r, false)))
            .filter_map(|(rule, allow)| match &rule.selector {
                Selector::Path(path) => Some((path.as_str(), allow)),
                _ => None,
            })
    }

    /// Tool/command permission only. Files still require unconditional confinement
    /// and exact protected-path checks at the backend; grants do not approve guests.
    pub(crate) fn tool_decision(&self, name: &str, args: &Value) -> ToolDecision {
        let tool_matches = |r: &Rule| matches!(&r.selector, Selector::Tool(t) if t == name);
        if !TOOLS.contains(&name) || self.deny.iter().any(tool_matches) || name == "ask_user" {
            return ToolDecision::Deny;
        }
        if name == "task"
            && args
                .get("run_in_background")
                .is_some_and(|v| v != &Value::Bool(false))
        {
            return ToolDecision::Deny;
        }
        // No shell command is proven mutation-free by token-prefix matching.
        // Plan therefore fails closed for native execution, even with an allow.
        if self.mode == Mode::Plan && matches!(name, "write_file" | "edit_file" | "execute") {
            return ToolDecision::Deny;
        }
        if name == "execute" {
            let tokens = args
                .get("command")
                .and_then(Value::as_str)
                .and_then(plain_command);
            let exec_matches = |r: &Rule| match (&r.selector, &tokens) {
                (Selector::Exec(prefix), Some(tokens)) => tokens.starts_with(prefix),
                _ => false,
            };
            // A broad native allow must not turn a failed classification into
            // a way around a prefix deny. When a deny could apply but the
            // command is unclassifiable, deny conservatively instead.
            if tokens.is_none()
                && self
                    .deny
                    .iter()
                    .any(|r| matches!(r.selector, Selector::Exec(_)))
            {
                return ToolDecision::Deny;
            }
            if self.deny.iter().any(exec_matches) {
                return ToolDecision::Deny;
            }
            if self
                .allow
                .iter()
                .any(|r| tool_matches(r) || exec_matches(r))
            {
                return ToolDecision::Allow;
            }
            return ToolDecision::Unapproved;
        }
        if matches!(
            name,
            "write_file" | "edit_file" | "web_fetch" | "web_search"
        ) && !self.allow.iter().any(tool_matches)
        {
            return ToolDecision::Unapproved;
        }
        ToolDecision::Allow
    }

    /// Exact selector matching, not a resolver: the backend supplies a normalized
    /// root-relative file. Reads, listings and searches must filter denied paths too.
    pub(crate) fn path_is_denied(&self, relative: &str) -> bool {
        self.deny
            .iter()
            .any(|r| matches!(&r.selector, Selector::Path(p) if p == relative))
    }

    /// Does not grant a write/edit tool or override a hard-denied path. The confined
    /// backend must check both independently before any file operation.
    pub(crate) fn protected_write_is_explicitly_allowed(&self, relative: &str) -> bool {
        !hard_reserved_path(relative)
            && !self.path_is_denied(relative)
            && self
                .allow
                .iter()
                .any(|r| matches!(&r.selector, Selector::Path(p) if p == relative))
    }
}

fn control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

fn relative_file(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(['\\', ':', '*', '?', '[', ']', '{', '}'])
        && !path.chars().any(control)
        && path.split('/').all(|p| !matches!(p, "" | "." | ".."))
}

fn hard_reserved_path(path: &str) -> bool {
    path.split('/').any(|p| {
        let p = p.to_ascii_lowercase();
        matches!(p.as_str(), ".git" | ".craftsman") || p.starts_with(".env")
    })
}

fn bare_program(program: &str) -> bool {
    let mut bytes = program.bytes();
    bytes.next().is_some_and(|c| c.is_ascii_alphabetic())
        && bytes.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        && !WRAPPERS.contains(&program.to_ascii_lowercase().as_str())
}

fn plain_tokens(tokens: &[String]) -> bool {
    let Some(program) = tokens.first() else {
        return false;
    };
    if !bare_program(program)
        || tokens
            .iter()
            .any(|t| t.is_empty() || t.contains(SHELL_META) || t.chars().any(control))
    {
        return false;
    }
    if program.eq_ignore_ascii_case("git") {
        if !tokens
            .get(1)
            .is_some_and(|t| GIT_READ.contains(&t.as_str()))
        {
            return false;
        }
        // Check every trailing token, including arguments beyond the allowed prefix.
        const FORBIDDEN: &[&str] = &[
            "--out",
            "--git-dir",
            "--work-tree",
            "--config-env",
            "--exec-path",
            "--paginate",
        ];
        if tokens.iter().skip(2).any(|t| {
            FORBIDDEN.iter().any(|option| t.starts_with(option))
                || t.starts_with("-C")
                || t.starts_with("-c")
        }) {
            return false;
        }
    }
    true
}

/// Reject shell expansion/composition before tokenizing. There is no fallback to
/// a first word when parsing fails. Quoted/escaped glob characters stay literals.
fn plain_command(command: &str) -> Option<Vec<String>> {
    if command.len() > MAX_COMMAND_BYTES
        || command.contains(SHELL_META)
        || command.chars().any(control)
    {
        return None;
    }
    let mut quote = None;
    let mut escaped = false;
    for c in command.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                }
            }
            Some('"') => match c {
                '"' => quote = None,
                '\\' => escaped = true,
                _ => {}
            },
            None => match c {
                '\'' | '"' => quote = Some(c),
                '\\' => escaped = true,
                '*' | '?' | '[' | ']' | '{' | '}' | '~' | '#' => return None,
                _ => {}
            },
            _ => unreachable!("only single/double quote states exist"),
        }
    }
    if escaped || quote.is_some() {
        return None;
    }
    let tokens = shlex::split(command)?;
    plain_tokens(&tokens).then_some(tokens)
}

fn validate_env_pass(names: &[String], credential_names: &[String]) -> Result<(), UsageError> {
    if names.len() > MAX_ENV_NAMES {
        return Err(UsageError::TooManyEnvironmentNames);
    }
    let sources = credential_names
        .iter()
        .map(|n| n.to_ascii_uppercase())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    for name in names {
        let mut bytes = name.bytes();
        if name.len() > MAX_ENV_NAME_BYTES
            || !bytes
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            || !bytes.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err(UsageError::EnvironmentName);
        }
        let upper = name.to_ascii_uppercase();
        if !seen.insert(upper.clone()) {
            return Err(UsageError::DuplicateEnvironmentName);
        }
        if sources.contains(&upper)
            || ["_API_KEY", "_TOKEN", "_SECRET", "_PASSWORD"]
                .iter()
                .any(|s| upper.ends_with(s))
            || upper == "CRAFTSMAN_PLATFORM_TOKEN"
        {
            return Err(UsageError::CredentialEnvironmentName);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn selectors(allow: &[&str], deny: &[&str]) -> Selectors {
        Selectors {
            allow: allow.iter().map(|s| (*s).to_string()).collect(),
            deny: deny.iter().map(|s| (*s).to_string()).collect(),
            ..Selectors::default()
        }
    }

    fn policy(allow: &[&str], deny: &[&str]) -> InvocationPermissions {
        InvocationPermissions::with_support(selectors(allow, deny), &[], PrefixSupport::Posix)
            .unwrap()
    }

    fn exec(p: &InvocationPermissions, command: &str) -> ToolDecision {
        p.tool_decision("execute", &json!({"command": command}))
    }

    #[test]
    fn all_accepted_rule_vectors_match_native_parsing() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../../docs/schemas/exec-v1.vectors.json"
        ))
        .unwrap();
        let mut count = 0;
        for case in vectors["rules"].as_array().unwrap() {
            let direction = if case["allow"] == true {
                Direction::Allow
            } else {
                Direction::Deny
            };
            let valid = case["rule"]
                .as_str()
                .is_some_and(|r| Rule::parse(r, direction).is_ok());
            assert_eq!(valid, case["valid"].as_bool().unwrap(), "{}", case["name"]);
            count += 1;
        }
        assert_eq!(count, 33);
    }

    #[test]
    fn mode_and_unapproved_options_are_narrow_and_exact() {
        assert_eq!(Mode::parse("manual"), Ok(Mode::Manual));
        assert_eq!(Mode::parse("plan"), Ok(Mode::Plan));
        for mode in [
            "auto",
            "accept-edits",
            "bypass-permissions",
            "Manual",
            "",
            "manual ",
        ] {
            assert_eq!(Mode::parse(mode), Err(UsageError::Mode));
        }
        assert_eq!(OnUnapproved::parse("deny"), Ok(OnUnapproved::Deny));
        assert_eq!(OnUnapproved::parse("fail"), Ok(OnUnapproved::Fail));
        assert_eq!(
            OnUnapproved::parse("approve"),
            Err(UsageError::OnUnapproved)
        );
    }

    #[test]
    fn fresh_manual_permissions_gate_writes_shell_and_network() {
        let p = InvocationPermissions::from_explicit(Selectors::default(), &[]).unwrap();
        assert_eq!(p.mode(), Mode::Manual);
        assert_eq!(p.on_unapproved(), OnUnapproved::Deny);
        assert!(p.allow_rules().is_empty() && p.deny_rules().is_empty() && p.env_pass().is_empty());
        for tool in [
            "read_file",
            "ls",
            "glob",
            "grep",
            "write_todos",
            "task",
            "send_message",
        ] {
            assert_eq!(p.tool_decision(tool, &json!({})), ToolDecision::Allow);
        }
        for tool in [
            "write_file",
            "edit_file",
            "execute",
            "web_fetch",
            "web_search",
        ] {
            assert_eq!(p.tool_decision(tool, &json!({})), ToolDecision::Unapproved);
        }
        assert_eq!(p.tool_decision("ask_user", &json!({})), ToolDecision::Deny);
        assert_eq!(p.tool_decision("made_up", &json!({})), ToolDecision::Deny);
    }

    #[test]
    fn captured_permissions_do_not_follow_later_invocation_mutation() {
        let mut input = selectors(&["tool:read_file"], &[]);
        let p = InvocationPermissions::from_explicit(input.clone(), &[]).unwrap();
        input.mode = Mode::Plan;
        input.allow.push("tool:execute".into());
        input.env_pass.push("SOME_API_KEY".into());
        assert_eq!(p.mode(), Mode::Manual);
        assert_eq!(exec(&p, "echo hello"), ToolDecision::Unapproved);
        assert!(p.env_pass().is_empty());
        assert_eq!(p.allow_rules(), ["tool:read_file"]);
    }

    #[test]
    fn deny_wins_over_tool_and_command_allows() {
        let p = policy(
            &["tool:execute", r#"exec:["cargo","test"]"#],
            &["tool:execute"],
        );
        assert_eq!(exec(&p, "cargo test"), ToolDecision::Deny);
        let p = policy(&["tool:execute"], &[r#"exec:["cargo","test"]"#]);
        assert_eq!(exec(&p, "cargo test --locked"), ToolDecision::Deny);
        assert_eq!(exec(&p, "cargo build"), ToolDecision::Allow);
        let p = policy(&["tool:read_file"], &["tool:read_file"]);
        assert_eq!(p.tool_decision("read_file", &json!({})), ToolDecision::Deny);
    }

    #[test]
    fn unclassifiable_commands_cannot_bypass_prefix_denies_with_a_broad_allow() {
        let p = policy(&["tool:execute"], &[r#"exec:["cargo","test"]"#]);
        for command in [
            "cargo test; echo bypass",
            "cargo test && echo bypass",
            "sh -c 'cargo test'",
            "cargo test $(echo injected)",
            "cargo test *",
            "cargo 'test",
            "git log --output=/tmp/output",
        ] {
            assert_eq!(exec(&p, command), ToolDecision::Deny, "{command}");
        }
        assert_eq!(exec(&p, "cargo build"), ToolDecision::Allow);
    }

    #[test]
    fn prefixes_match_complete_tokens_not_head_words_or_strings() {
        let p = policy(&[r#"exec:["cargo","test"]"#], &[]);
        for command in [
            "cargo test",
            "cargo  test --locked",
            "cargo 'test' --package demo",
        ] {
            assert_eq!(exec(&p, command), ToolDecision::Allow, "{command}");
        }
        for command in [
            "cargo tester",
            "cargo build",
            "cargo",
            "other cargo test",
            "cargo 'test",
        ] {
            assert_eq!(exec(&p, command), ToolDecision::Unapproved, "{command}");
        }
    }

    #[test]
    fn composition_expansion_and_ambiguous_syntax_never_match_prefixes() {
        let p = policy(&[r#"exec:["cargo","test"]"#], &[]);
        for command in [
            "cargo test; echo unsafe",
            "cargo test && echo unsafe",
            "cargo test | cat",
            "cargo test > /tmp/out",
            "cargo test $(echo x)",
            "cargo test `echo x`",
            "cargo test\necho unsafe",
            "cargo test *",
            "cargo test a?b",
            "cargo test [ab]",
            "cargo test {a,b}",
            "cargo test ~",
            "cargo test #comment",
            "cargo test\u{9b}",
            "env FLAG=x cargo test",
            "sh -c 'cargo test'",
            "cargo test \\",
        ] {
            assert_eq!(exec(&p, command), ToolDecision::Unapproved, "{command:?}");
        }
    }

    #[test]
    fn quoted_or_escaped_literal_arguments_can_match_exact_prefix_tokens() {
        let p = policy(&[r#"exec:["printf","a * b"]"#], &[]);
        assert_eq!(exec(&p, "printf 'a * b'"), ToolDecision::Allow);
        assert_eq!(exec(&p, "printf \"a * b\""), ToolDecision::Allow);
        assert_eq!(exec(&p, "printf a\\ \\*\\ b"), ToolDecision::Allow);
        assert_eq!(exec(&p, "printf a * b"), ToolDecision::Unapproved);
    }

    #[test]
    fn git_checks_the_entire_command_not_only_the_allowed_prefix() {
        let p = policy(&[r#"exec:["git","log"]"#], &[]);
        assert_eq!(exec(&p, "git log --oneline -5"), ToolDecision::Allow);
        for flag in [
            "--output=x",
            "--output x",
            "--out=x",
            "--out x",
            "--git-dir=/tmp/x",
            "--work-tree=/tmp/x",
            "--config-env=core.fsmonitor=X",
            "--exec-path=/tmp/x",
            "--paginate",
            "-C/tmp/x",
            "-C /tmp/x",
            "-ccore.fsmonitor=x",
            "-c core.fsmonitor=x",
        ] {
            assert_eq!(
                exec(&p, &format!("git log {flag}")),
                ToolDecision::Unapproved,
                "{flag}"
            );
        }
        assert_eq!(
            exec(&p, "git --git-dir=/tmp/x log"),
            ToolDecision::Unapproved
        );
    }

    #[test]
    fn broad_execute_is_an_explicit_native_permission_not_a_prefix_or_sandbox() {
        let p = policy(&["tool:execute"], &[]);
        assert_eq!(
            exec(&p, "echo allowed; echo also-allowed"),
            ToolDecision::Allow
        );
        assert_eq!(exec(&p, "sh -c 'echo native'"), ToolDecision::Allow);
    }

    #[test]
    fn plan_cannot_regain_mutation_through_explicit_allows() {
        let mut input = selectors(
            &[
                "tool:write_file",
                "tool:edit_file",
                "tool:execute",
                "tool:web_fetch",
            ],
            &[],
        );
        input.mode = Mode::Plan;
        input.on_unapproved = OnUnapproved::Fail;
        let p = InvocationPermissions::from_explicit(input, &[]).unwrap();
        for tool in ["write_file", "edit_file", "execute"] {
            assert_eq!(
                p.tool_decision(tool, &json!({"command":"git status"})),
                ToolDecision::Deny
            );
        }
        assert_eq!(
            p.tool_decision("web_fetch", &json!({})),
            ToolDecision::Allow
        );
        assert_eq!(p.on_unapproved(), OnUnapproved::Fail);
    }

    #[test]
    fn background_tasks_and_malformed_background_flags_are_unavailable() {
        let p = policy(&["tool:task"], &[]);
        assert_eq!(p.tool_decision("task", &json!({})), ToolDecision::Allow);
        assert_eq!(
            p.tool_decision("task", &json!({"run_in_background":false})),
            ToolDecision::Allow
        );
        for flag in [json!(true), json!("true"), json!(null), json!(1)] {
            assert_eq!(
                p.tool_decision("task", &json!({"run_in_background":flag})),
                ToolDecision::Deny
            );
        }
    }

    #[test]
    fn exact_path_allows_do_not_grant_tools_and_path_denies_win() {
        let p = policy(&["path:.github/workflows/ci.yml"], &[]);
        assert!(p.protected_write_is_explicitly_allowed(".github/workflows/ci.yml"));
        assert!(!p.protected_write_is_explicitly_allowed(".github/workflows/other.yml"));
        assert_eq!(
            p.tool_decision("write_file", &json!({})),
            ToolDecision::Unapproved
        );
        let p = policy(
            &["tool:write_file", "path:.github/workflows/ci.yml"],
            &["path:.github/workflows/ci.yml"],
        );
        assert!(p.path_is_denied(".github/workflows/ci.yml"));
        assert!(!p.protected_write_is_explicitly_allowed(".github/workflows/ci.yml"));
        assert!(!p.path_is_denied(".github/workflows/ci.yml.extra"));
    }

    #[test]
    fn hard_reserved_path_allows_fail_conservatively() {
        for path in [
            ".Git/config",
            "a/.CRAFTSMAN/grants",
            ".ENV",
            "a/.env.production",
            "a/.envdir/f",
        ] {
            assert_eq!(
                Rule::parse(&format!("path:{path}"), Direction::Allow),
                Err(UsageError::ForbiddenAllow)
            );
            assert!(Rule::parse(&format!("path:{path}"), Direction::Deny).is_ok());
        }
    }

    #[test]
    fn selector_and_prefix_bounds_are_exact() {
        let max_path = format!("path:{}", "a".repeat(MAX_RULE_CHARS - 5));
        assert!(Rule::parse(&max_path, Direction::Allow).is_ok());
        assert_eq!(
            Rule::parse(&(max_path + "a"), Direction::Allow),
            Err(UsageError::Rule)
        );
        let max_token = "a".repeat(MAX_TOKEN_CHARS);
        assert!(
            Rule::parse(
                &format!("exec:{}", json!(["echo", max_token])),
                Direction::Allow
            )
            .is_ok()
        );
        assert!(
            Rule::parse(
                &format!("exec:{}", json!(["echo", "a".repeat(MAX_TOKEN_CHARS + 1)])),
                Direction::Allow
            )
            .is_err()
        );
        let tokens = vec!["echo"; MAX_PREFIX_TOKENS];
        assert!(Rule::parse(&format!("exec:{}", json!(tokens)), Direction::Allow).is_ok());
        assert!(
            Rule::parse(
                &format!("exec:{}", json!(vec!["echo"; MAX_PREFIX_TOKENS + 1])),
                Direction::Allow
            )
            .is_err()
        );
    }

    #[test]
    fn escaped_unicode_tokens_work_but_non_ascii_selector_json_does_not() {
        assert!(Rule::parse(r#"exec:["echo","\u4e2d\u6587"]"#, Direction::Allow).is_ok());
        assert!(Rule::parse("exec:[\"echo\",\"中文\"]", Direction::Allow).is_err());
        for raw in [
            r#"exec:["echo","\ud800"]"#,
            r#"exec:["echo","\u0000"]"#,
            "exec:[\"echo\",NaN]",
        ] {
            assert!(Rule::parse(raw, Direction::Allow).is_err());
        }
    }

    #[test]
    fn duplicate_and_excess_selector_lists_fail_before_policy_construction() {
        for (allow, deny) in [
            (&["tool:read_file", "tool:read_file"][..], &[][..]),
            (&[][..], &["tool:read_file", "tool:read_file"][..]),
        ] {
            assert!(matches!(
                InvocationPermissions::from_explicit(selectors(allow, deny), &[]),
                Err(UsageError::DuplicateRule)
            ));
        }
        let input = Selectors {
            allow: vec!["tool:read_file".into(); MAX_RULES + 1],
            ..Selectors::default()
        };
        assert!(matches!(
            InvocationPermissions::from_explicit(input, &[]),
            Err(UsageError::TooManyRules)
        ));
    }

    #[test]
    fn unsupported_platform_rejects_allow_and_deny_command_prefixes() {
        for input in [
            selectors(&[r#"exec:["cargo","test"]"#], &[]),
            selectors(&[], &[r#"exec:["cargo","test"]"#]),
        ] {
            assert!(matches!(
                InvocationPermissions::with_support(input.clone(), &[], PrefixSupport::Unsupported),
                Err(UsageError::UnsupportedPrefix)
            ));
            let native = InvocationPermissions::from_explicit(input, &[]);
            assert_eq!(
                native.is_ok(),
                crate::platform_os::shell_allow_list_supported()
            );
        }
        assert!(
            InvocationPermissions::with_support(
                selectors(&["tool:execute"], &[]),
                &[],
                PrefixSupport::Unsupported
            )
            .is_ok()
        );
    }

    #[test]
    fn environment_names_are_bounded_unique_and_credential_sources_are_not_passable() {
        let source = vec!["HARMLESS_NAME".into()];
        for name in [
            "OTHER_API_KEY",
            "some_token",
            "A_SECRET",
            "a_password",
            "CRAFTSMAN_PLATFORM_TOKEN",
            "HARMLESS_NAME",
            "harmless_name",
        ] {
            assert_eq!(
                validate_env_pass(&[name.into()], &source),
                Err(UsageError::CredentialEnvironmentName),
                "{name}"
            );
        }
        for name in ["", "1BAD", "A=B", "two names", "NAME\n", "é", "::notice"] {
            assert_eq!(
                validate_env_pass(&[name.into()], &[]),
                Err(UsageError::EnvironmentName)
            );
        }
        assert_eq!(
            validate_env_pass(&["NAME".into(), "name".into()], &[]),
            Err(UsageError::DuplicateEnvironmentName)
        );
        assert!(
            validate_env_pass(
                &["_DEBUG".into(), "PATH".into(), "CI_JOB_ID".into()],
                &source
            )
            .is_ok()
        );
        assert!(validate_env_pass(&["A".repeat(MAX_ENV_NAME_BYTES)], &[]).is_ok());
        assert_eq!(
            validate_env_pass(&["A".repeat(MAX_ENV_NAME_BYTES + 1)], &[]),
            Err(UsageError::EnvironmentName)
        );
        assert_eq!(
            validate_env_pass(&vec!["A".into(); MAX_ENV_NAMES + 1], &[]),
            Err(UsageError::TooManyEnvironmentNames)
        );
    }

    #[test]
    fn diagnostics_never_echo_untrusted_selector_or_credential_material() {
        let malicious = "tool:::notice\u{1b}[31msecret-value";
        let err = Rule::parse(malicious, Direction::Allow)
            .unwrap_err()
            .to_string();
        assert!(!err.contains("secret-value") && !err.contains("::") && !err.contains('\u{1b}'));
        let err = validate_env_pass(&["SECRET_VALUE_TOKEN".into()], &[])
            .unwrap_err()
            .to_string();
        assert!(!err.contains("SECRET_VALUE"));
    }
}

// Pure frontend resource preflight; these aliases never mint a policy/grant.
pub(in crate::exec) const fn frontend_argument_bounds() -> (usize, usize, usize, usize) {
    (
        MAX_RULES,
        MAX_RULE_CHARS * 4,
        MAX_ENV_NAMES,
        MAX_ENV_NAME_BYTES,
    )
}
