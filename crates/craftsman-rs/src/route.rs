//! Legacy direct OpenAI-compatible endpoint resolution.
//! `base_url` and `api_key` declare the connection; a configured OpenAI key
//! may use the standard API base. `${ENV_VAR}` references are explicit config.
//! Named provider connections use provider_config instead.

use crate::config::{Config, expand_env_placeholders};

const OPENAI_STANDARD_BASE: &str = "https://api.openai.com/v1";

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRoute {
    pub base_url: String,
    pub api_key: String,
    /// Route label for status displays ("litellm" kept so the UI label stays the same).
    pub source: &'static str,
}

/// Resolve the OpenAI-compatible chat endpoint. Errors only when `api_key`
/// carries an unresolvable `${ENV_VAR}` placeholder — a literal `${VAR}`
/// bearer would surface later as a confusing 401.
pub async fn resolve_chat_endpoint(cfg: &Config) -> anyhow::Result<ChatRoute> {
    let api_key = expand_env_placeholders(&cfg.api_key)?;
    Ok(with_resolved_key(cfg, api_key))
}

/// Pure route assembly for a captured headless credential. Do not re-expand a
/// value containing a literal `${...}` after the capture/seed boundary.
pub(crate) fn with_resolved_key(cfg: &Config, api_key: String) -> ChatRoute {
    let base_url = if cfg.base_url.is_empty() {
        OPENAI_STANDARD_BASE.to_string()
    } else {
        cfg.base_url.clone()
    };
    ChatRoute {
        base_url,
        api_key,
        source: "litellm",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(base: &str, key: &str) -> Config {
        Config {
            base_url: base.into(),
            api_key: key.into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn direct_route_resolves() {
        let r = resolve_chat_endpoint(&cfg("http://lite:4000", "sk-x"))
            .await
            .unwrap();
        assert_eq!(r.source, "litellm");
        assert_eq!(r.base_url, "http://lite:4000");
        assert_eq!(r.api_key, "sk-x");
    }

    #[tokio::test]
    async fn empty_base_defaults_to_openai() {
        let r = resolve_chat_endpoint(&cfg("", "sk-x")).await.unwrap();
        assert_eq!(r.base_url, OPENAI_STANDARD_BASE);
    }

    #[tokio::test]
    async fn api_key_env_placeholder_expands() {
        crate::test_environment::run_async(crate::test_environment::Case::Route).await;
    }

    pub(super) async fn api_key_env_placeholder_body() {
        let r = resolve_chat_endpoint(&cfg("", "${CRAFTSMAN_ROUTE_TEST_KEY}"))
            .await
            .unwrap();
        assert_eq!(r.api_key, "tok-1");
    }

    #[tokio::test]
    async fn api_key_missing_env_placeholder_errors() {
        assert!(
            resolve_chat_endpoint(&cfg("", "${CRAFTSMAN_ROUTE_MISSING_9}"))
                .await
                .is_err()
        );
    }
}

#[cfg(test)]
pub(crate) async fn route_environment_fixture_for_tests() {
    tests::api_key_env_placeholder_body().await;
}
