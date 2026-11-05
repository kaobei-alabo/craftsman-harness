//! Thin hand-rolled REST client for harness-facing platform endpoints —
//! the token-era replacement for the SDK's app-manifest machinery
//! (`docs/platform-token-refactor.md` §4.3). Scope: skills only.
//!
//! Paths come from the platform's published OpenAPI spec (pulled
//! 2026-10-16): `GET /apps/skills/api/skill/` (`list_skill`) and
//! `GET /apps/skills/api/skill/{id}/download` (`skill_download`). Auth is
//! the static personal API token from `platform { base_url, api_token }`.

use crate::config::Config;
use serde_json::Value;

/// Normalize a user-supplied platform URL.
///
/// Re-homed verbatim from the removed `craftsman-sdk` crate (`core/src/url.rs`),
/// so the rules stay in one place:
/// - Bare host → `https://<host>`.
/// - `app.<rest>` host → `api.<rest>` (web → api convention).
/// - Strip any path/query/trailing slashes.
/// - Reject anything that doesn't end up as http/https.
pub fn normalize_platform_url(raw: &str) -> anyhow::Result<String> {
    let s = raw.trim();
    if s.is_empty() {
        anyhow::bail!("platform URL is empty");
    }
    let s = if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    let parsed = url::Url::parse(&s).map_err(|e| anyhow::anyhow!("invalid platform URL: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        anyhow::bail!("unsupported scheme: {scheme:?}");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("invalid platform URL: {raw:?}"))?
        .to_ascii_lowercase();
    let host = match host.strip_prefix("app.") {
        Some(rest) => format!("api.{rest}"),
        None => host,
    };
    let netloc = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    Ok(format!("{scheme}://{netloc}"))
}

/// A token-authed REST client rooted at the platform base URL. Constructed
/// only when the platform block is fully configured.
pub struct PlatformRest {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl PlatformRest {
    /// `None` when `platform.base_url`/`platform.api_token` (or the
    /// `CRAFTSMAN_PLATFORM_TOKEN` fallback) aren't configured.
    pub fn from_config(cfg: &Config) -> Option<PlatformRest> {
        let base_url = cfg.platform.base_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            return None;
        }
        let token = cfg.platform_api_token();
        if token.is_empty() {
            return None;
        }
        Some(PlatformRest {
            base_url: base_url.to_string(),
            token,
            http: reqwest::Client::new(),
        })
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            // Char-boundary-safe truncation — a byte slice can panic on
            // multi-byte UTF-8 in the error body.
            let short: String = text.chars().take(300).collect();
            anyhow::bail!("platform HTTP {status}: {short}");
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Skills catalog — `GET /apps/skills/api/skill/` (spec summary
    /// `list_skill`). Returns the raw listing; row parsing lives with the
    /// skills engine.
    pub async fn list_skills(&self) -> anyhow::Result<Value> {
        self.get("/apps/skills/api/skill/").await
    }

    /// Bundle download info — `GET /apps/skills/api/skill/{id}/download`
    /// (spec summary `skill_download`): `{url, sha256, version, name}`.
    pub async fn skill_download(&self, skill_id: &str) -> anyhow::Result<Value> {
        self.get(&format!("/apps/skills/api/skill/{skill_id}/download"))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn configured_cfg(server_uri: &str) -> Config {
        Config {
            platform: crate::config::PlatformConfig {
                base_url: server_uri.to_string(),
                api_token: "mpat-token".into(),
            },
            ..Config::default()
        }
    }

    #[test]
    fn normalize_happy_paths() {
        // Table carried over from the removed SDK's normalization tests.
        let cases = [
            ("api.craftsman.local", "https://api.craftsman.local"),
            ("app.craftsman.local", "https://api.craftsman.local"),
            ("https://api.craftsman.local", "https://api.craftsman.local"),
            (
                "https://api.craftsman.local/",
                "https://api.craftsman.local",
            ),
            (
                "https://app.X.craftsmanai.com",
                "https://api.x.craftsmanai.com",
            ),
            ("http://127.0.0.1:8765", "http://127.0.0.1:8765"),
            (
                "app.craftsman.local:8080",
                "https://api.craftsman.local:8080",
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                normalize_platform_url(raw).unwrap(),
                expected,
                "input: {raw}"
            );
        }
    }

    #[test]
    fn normalize_rejects_bad_input_and_strips_extras() {
        for raw in ["", "  ", "ftp://craftsman.local", "ws://craftsman.local"] {
            assert!(
                normalize_platform_url(raw).is_err(),
                "should reject: {raw:?}"
            );
        }
        assert_eq!(
            normalize_platform_url("https://api.craftsman.local/foo/bar?x=1").unwrap(),
            "https://api.craftsman.local"
        );
    }

    #[test]
    fn from_config_requires_full_configuration() {
        let unconfigured = Config::default();
        assert!(PlatformRest::from_config(&unconfigured).is_none());

        let url_only = Config {
            platform: crate::config::PlatformConfig {
                base_url: "https://api.craftsman.local".into(),
                api_token: String::new(),
            },
            ..Config::default()
        };
        assert!(PlatformRest::from_config(&url_only).is_none());

        assert!(
            PlatformRest::from_config(&configured_cfg("https://api.craftsman.local")).is_some()
        );
    }

    #[tokio::test]
    async fn skills_endpoints_hit_the_published_paths_with_bearer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/skills/api/skill/"))
            .and(header("authorization", "Bearer mpat-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [{"id": "web-search", "name": "Web Search", "version": "1"}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/apps/skills/api/skill/web-search/download"))
            .and(header("authorization", "Bearer mpat-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "url": "https://blob.example/web-search.tar.gz",
                "sha256": "abc",
                "version": "1",
                "name": "Web Search"
            })))
            .mount(&server)
            .await;

        let cfg = configured_cfg(&server.uri());
        let rest = PlatformRest::from_config(&cfg).unwrap();
        let listing = rest.list_skills().await.unwrap();
        assert_eq!(listing["items"][0]["id"], "web-search");
        let info = rest.skill_download("web-search").await.unwrap();
        assert_eq!(info["sha256"], "abc");
    }

    #[tokio::test]
    async fn http_error_surfaces_status_and_short_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/skills/api/skill/"))
            .respond_with(ResponseTemplate::new(401).set_body_string("invalid token"))
            .mount(&server)
            .await;
        let cfg = configured_cfg(&server.uri());
        let rest = PlatformRest::from_config(&cfg).unwrap();
        let error = rest.list_skills().await.unwrap_err();
        assert!(error.to_string().contains("401"));
        assert!(error.to_string().contains("invalid token"));
    }
}
