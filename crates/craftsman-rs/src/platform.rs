//! Platform operations. The login/refresh era is gone (see
//! `docs/platform-token-refactor.md`): the CLI is headless and
//! token-configured. The platform's LLM endpoint is a stock OpenAI-compatible
//! provider reached through `route.rs`; harness-facing REST (skills) lives in
//! `platform_rest.rs`. What remains here is OpenAI-compatible model listing.

use crate::config::Config;
use serde_json::Value;

/// Fetch model ids for the active chat route: a plain OpenAI-style
/// `GET {base}/models` with the resolved bearer. The platform's `llm` app is
/// reached exactly the same way — it exposes `/apps/llm/api/v1/models`, an
/// OpenAI-compatible surface — so there is no platform-specific branch.
pub async fn list_models(
    _cfg: &Config,
    route: &crate::route::ChatRoute,
) -> anyhow::Result<Vec<String>> {
    let client = reqwest::Client::new();
    let v: Value = client
        .get(format!("{}/models", route.base_url.trim_end_matches('/')))
        .bearer_auth(&route.api_key)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut out: Vec<String> = v["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn lists_models_from_the_openai_compatible_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "model-b"}, {"id": "model-a"}]
            })))
            .mount(&server)
            .await;
        let route = crate::route::ChatRoute {
            base_url: server.uri(),
            api_key: "tok".into(),
            source: "litellm",
        };
        let cfg = Config::default();
        let models = list_models(&cfg, &route).await.unwrap();
        assert_eq!(models, vec!["model-a".to_string(), "model-b".to_string()]);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].headers.get("authorization").unwrap(),
            "Bearer tok"
        );
    }
}
