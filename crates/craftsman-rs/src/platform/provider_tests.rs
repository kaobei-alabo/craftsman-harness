use super::*;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn discovery_uses_each_protocol_and_namespaces_native_ids_without_aliases() {
    let server = MockServer::start().await;
    let mut providers = serde_json::Map::new();
    for (name, protocol, base, key, auth, body) in [
        (
            "openai",
            "openai_responses",
            "openai",
            "openai-key",
            "authorization",
            json!({"data":[{"id":"same-model","reasoning_efforts":["low","high"]}]}),
        ),
        (
            "claude",
            "anthropic_messages",
            "claude",
            "claude-key",
            "x-api-key",
            json!({"data":[{"id":"same-model","display_name":"Claude model"}]}),
        ),
        (
            "google",
            "gemini_generate_content",
            "google",
            "google-key",
            "x-goog-api-key",
            json!({"models":[{"name":"models/gemini-model","displayName":"Gemini model"}]}),
        ),
        (
            "custom",
            "openai_chat_completions",
            "custom",
            "custom-key",
            "authorization",
            json!({"data":[{"id":"org/model"}]}),
        ),
    ] {
        let bearer = format!("Bearer {key}");
        Mock::given(method("GET"))
            .and(path(format!("/{base}/models")))
            .and(header(
                auth,
                if auth == "authorization" {
                    bearer.as_str()
                } else {
                    key
                },
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        providers.insert(name.into(),json!({"protocol":protocol,"base_url":format!("{}/{base}",server.uri()),"api_key":key,"models":{}}));
    }
    let cfg: Config = serde_json::from_value(json!({"providers":providers})).unwrap();
    let (mut models, available) = list_provider_models(&cfg).await;
    assert!(available);
    models.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "claude/same-model",
            "custom/org/model",
            "google/gemini-model",
            "openai/same-model"
        ]
    );
    assert_eq!(models[0].label, "Claude model");
    assert_eq!(models[2].label, "Gemini model");
    assert_eq!(models[3].reasoning_efforts.len(), 2);
    server.verify().await;
}

#[tokio::test]
async fn unavailable_credentials_or_providers_keep_the_configured_catalog_usable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/valid/models"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"discovered"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bad/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":[{"id":"duplicate"},{"id":"duplicate"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let cfg:Config=serde_json::from_value(json!({"model":"bad/manual","providers":{
        "valid":{"protocol":"openai_chat_completions","base_url":format!("{}/valid",server.uri()),"api_key":"key","models":{}},
        "bad":{"protocol":"openai_responses","base_url":format!("{}/bad",server.uri()),"api_key":"key","models":{"manual":{"label":"Configured model"}}},
        "offline":{"protocol":"anthropic_messages","base_url":format!("{}/offline",server.uri()),"api_key":"${INVALID-TEST-KEY}","models":{"remembered":{}}}
    }})).unwrap();
    let (remote, available) = list_provider_models(&cfg).await;
    assert!(!available);
    assert_eq!(remote.len(), 1);
    let models = crate::model_controls::merge_catalog(&cfg, remote).unwrap();
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["bad/manual", "offline/remembered", "valid/discovered"]
    );
    assert_eq!(models[0].label, "Configured model");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn cancelled_discovery_drops_the_owned_requests_and_can_run_again() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(60)))
        .expect(1)
        .mount(&server)
        .await;
    let cfg:Config=serde_json::from_value(json!({"providers":{"p":{"protocol":"openai_responses","base_url":server.uri(),"api_key":"key","models":{}}}})).unwrap();
    let mut lookup = Box::pin(list_provider_models(&cfg));
    let ready = async {
        loop {
            if server.received_requests().await.unwrap().len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {
        _=&mut lookup=>panic!("discovery finished before its pending request"),
        ready=tokio::time::timeout(std::time::Duration::from_secs(5),ready)=>ready.unwrap(),
    }
    drop(lookup);
    server.verify().await;
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"ready"}]})))
        .expect(1)
        .mount(&server)
        .await;
    let (models, available) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        list_provider_models(&cfg),
    )
    .await
    .unwrap();
    assert!(available);
    assert_eq!(models[0].id, "p/ready");
}
