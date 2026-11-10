//! Platform operations. The login/refresh era is gone (see
//! `docs/platform-token-refactor.md`): the CLI is headless and
//! token-configured. The platform's LLM endpoint is a stock OpenAI-compatible
//! provider reached through `route.rs`; harness-facing REST (skills) lives in
//! `platform_rest.rs`. What remains here is OpenAI-compatible model listing.

use crate::config::Config;
use serde_json::Value;

#[cfg(test)]
#[path = "platform/provider_tests.rs"]
mod provider_tests;

/// The parent future owns every bounded discovery request directly. Cancelling
/// it drops all request futures inline; no detached application task survives.
pub async fn list_provider_models(
    cfg: &Config,
) -> (Vec<crate::model_controls::CatalogModel>, bool) {
    use futures_util::StreamExt;
    if crate::provider_config::validate(cfg).is_err() {
        return (Vec::new(), false);
    }
    let mut requests = futures_util::stream::iter(
        cfg.providers
            .clone()
            .into_iter()
            .map(|(name, provider)| async move { discover_provider(&name, &provider).await }),
    )
    .buffer_unordered(16);
    let mut models = Vec::new();
    let mut all_available = true;
    while let Some(result) = requests.next().await {
        match result {
            Ok(discovered) if models.len().saturating_add(discovered.len()) <= 512 => {
                models.extend(discovered)
            }
            _ => all_available = false,
        }
    }
    (models, all_available)
}

async fn discover_provider(
    name: &str,
    provider: &crate::provider_config::Provider,
) -> Result<Vec<crate::model_controls::CatalogModel>, ()> {
    use crate::provider_config::Protocol;
    use futures_util::StreamExt;
    let key = crate::config::expand_env_placeholders(&provider.api_key).map_err(|_| ())?;
    if key.starts_with("REPLACE_WITH_") {
        return Err(());
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| ())?;
    let builder = http.get(format!(
        "{}/models",
        crate::provider_config::endpoint(provider).trim_end_matches('/')
    ));
    let builder = match provider.protocol {
        Protocol::AnthropicMessages => builder
            .header("x-api-key", key)
            .header("anthropic-version", "2026-10-21"),
        Protocol::GeminiGenerateContent => builder.header("x-goog-api-key", key),
        _ => builder.bearer_auth(key),
    };
    let response = builder.send().await.map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        if bytes.len().saturating_add(chunk.len()) > 1_048_576 {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    let records = value[if provider.protocol == Protocol::GeminiGenerateContent {
        "models"
    } else {
        "data"
    }]
    .as_array()
    .ok_or(())?;
    if records.len() > 512 {
        return Err(());
    }
    let mut models = std::collections::BTreeMap::new();
    for record in records {
        if record["capabilities"]["tool_calling"] == false
            || record["capabilities"]["streaming"] == false
        {
            continue;
        }
        if provider.protocol == Protocol::GeminiGenerateContent {
            if let Some(methods) = record["supportedGenerationMethods"].as_array() {
                if !methods.iter().any(|method| method == "generateContent") {
                    continue;
                }
            }
        }
        let (id, label) = if provider.protocol == Protocol::GeminiGenerateContent {
            let id = record["name"].as_str().ok_or(())?;
            (
                id.strip_prefix("models/").unwrap_or(id),
                record["displayName"].as_str().unwrap_or_default(),
            )
        } else {
            (
                record["id"].as_str().ok_or(())?,
                record["display_name"].as_str().unwrap_or_default(),
            )
        };
        let mut model = crate::model_controls::CatalogModel::named(format!("{name}/{id}"));
        model.label = label.into();
        if let Some(efforts) = record.get("reasoning_efforts") {
            model.reasoning_efforts = serde_json::from_value(efforts.clone()).map_err(|_| ())?;
        }
        model.validate().map_err(|_| ())?;
        if models.insert(model.id.clone(), model).is_some() {
            return Err(());
        }
    }
    Ok(models.into_values().collect())
}

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

/// Rich catalog for the terminal. Ordinary callers retain `list_models` above.
/// Providers may include explicit effort metadata; missing metadata is unknown.
pub(crate) async fn model_catalog(
    route: &crate::route::ChatRoute,
) -> anyhow::Result<Vec<crate::model_controls::CatalogModel>> {
    const LIMIT: usize = 1024 * 1024;
    let client = reqwest::Client::new();
    let mut response = client
        .get(format!("{}/models", route.base_url.trim_end_matches('/')))
        .bearer_auth(&route.api_key)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > LIMIT as u64)
    {
        anyhow::bail!("provider model catalog exceeds its size limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > LIMIT - bytes.len() {
            anyhow::bail!("provider model catalog exceeds its size limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid provider model catalog"))?;
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("invalid provider model catalog"))?;
    if entries.len() > 512 {
        anyhow::bail!("provider model catalog exceeds 512 models");
    }
    let mut out = Vec::new();
    for entry in entries {
        let model: crate::model_controls::CatalogModel = serde_json::from_value(entry.clone())
            .map_err(|_| anyhow::anyhow!("invalid provider model metadata"))?;
        model.validate().map_err(anyhow::Error::msg)?;
        if out
            .iter()
            .any(|existing: &crate::model_controls::CatalogModel| existing.id == model.id)
        {
            anyhow::bail!("provider catalog repeats a model id");
        }
        out.push(model);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
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

    #[tokio::test]
    async fn rich_catalog_uses_resolved_route_bearer_and_explicit_effort_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[
                    {"id":"b","reasoning_efforts":["low","high"]},{"id":"a"}
                ]})),
            )
            .mount(&server)
            .await;
        let route = crate::route::ChatRoute {
            base_url: format!("{}/v1", server.uri()),
            api_key: "synthetic-model-token".into(),
            source: "direct",
        };
        let models = model_catalog(&route).await.unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(models[0].reasoning_efforts.is_empty());
        assert_eq!(
            models[1].reasoning_efforts,
            vec![
                crate::model_controls::ReasoningEffort::Low,
                crate::model_controls::ReasoningEffort::High
            ]
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0].headers.get("authorization").unwrap(),
            "Bearer synthetic-model-token"
        );
    }

    #[tokio::test]
    async fn rich_catalog_refuses_invalid_duplicate_and_overbound_records() {
        for data in [
            serde_json::json!([{"id":"a"},{"id":"a"}]),
            serde_json::json!([{"id":"a","reasoning_efforts":["unknown"]}]),
            serde_json::json!([{"id":""}]),
            serde_json::json!([{"id":"x".repeat(513)}]),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":data})),
                )
                .mount(&server)
                .await;
            let route = crate::route::ChatRoute {
                base_url: server.uri(),
                api_key: "synthetic-model-token".into(),
                source: "direct",
            };
            assert!(model_catalog(&route).await.is_err());
        }
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(vec![b'x'; 1024 * 1024 + 1], "application/json"),
            )
            .mount(&server)
            .await;
        let route = crate::route::ChatRoute {
            base_url: server.uri(),
            api_key: "synthetic-model-token".into(),
            source: "direct",
        };
        assert!(model_catalog(&route).await.is_err());
    }
}
