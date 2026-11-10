use super::*;

fn config() -> Config {
    serde_json::from_value(serde_json::json!({
        "model":"first/org/model", "unrelated":{"keep":true},
        "providers":{
            "first":{"protocol":"openai_responses","api_key":"${INVALID-TEST-KEY}","models":{"org/model":{"label":"First model","reasoning_controls":{"type":"effort","values":["low","high"]},"settings":{"reasoning":{"type":"effort","value":"high"},"generation":{"max_output_tokens":8192}}}}},
            "second":{"protocol":"anthropic_messages","api_key":"placeholder","models":{"org/model":{"settings":{"reasoning":null}}}}
        }
    })).unwrap()
}

#[test]
fn native_model_ids_are_keys_and_provider_namespaces_disambiguate_them() {
    let cfg = config();
    let catalog = catalog(&cfg).unwrap();
    assert_eq!(
        catalog.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        vec!["first/org/model", "second/org/model"]
    );
    let chosen = selection(&cfg, "first/org/model").unwrap();
    assert_eq!(chosen.model_id, "org/model");
    assert_eq!(chosen.provider.protocol, Protocol::OpenaiResponses);
    assert!(selection(&cfg, "org/model").is_err());
    assert!(selection(&cfg, "missing/model").is_err());
}

#[tokio::test]
async fn selecting_one_provider_does_not_resolve_other_credentials() {
    let mut cfg = config();
    cfg.model = Some("second/org/model".into());
    let (route, protocol, model) = resolve(&cfg).await.unwrap();
    assert_eq!(route.api_key, "placeholder");
    assert_eq!(route.base_url, "https://api.anthropic.com/v1");
    assert_eq!(protocol, Protocol::AnthropicMessages);
    assert_eq!(model, "org/model");
    cfg.model = Some("first/org/model".into());
    assert_eq!(
        resolve(&cfg).await.err().unwrap(),
        "Selected provider credential could not be resolved."
    );
}

#[test]
fn selections_are_saved_under_their_provider_without_losing_unknown_data() {
    let mut cfg = config();
    set_effort(&mut cfg, "first/org/model", Some(ReasoningEffort::Low)).unwrap();
    assert!(set_effort(&mut cfg, "first/org/model", Some(ReasoningEffort::Medium)).is_err());
    assert_eq!(
        options(&cfg, "first/org/model").unwrap()["craftsman_reasoning"]["value"],
        "low"
    );
    assert!(options(&cfg, "second/org/model").unwrap().is_empty());
    set_effort(&mut cfg, "first/org/model", None).unwrap();
    let saved = serde_json::to_value(&cfg).unwrap();
    assert!(saved["providers"]["first"]["models"]["org/model"]["settings"]["reasoning"].is_null());
    assert_eq!(saved["unrelated"]["keep"], true);
    assert!(saved.get("config_version").is_none());
}

#[test]
fn metadata_is_not_a_generation_parameter_and_connections_have_redacted_debug() {
    let cfg = config();
    let options = options(&cfg, "first/org/model").unwrap();
    assert_eq!(options.len(), 2);
    assert_eq!(options["max_output_tokens"], 8192);
    let debug = format!("{:?}", cfg.providers["first"]);
    assert!(!debug.contains("INVALID-TEST-KEY"));
    assert!(!debug.contains("First model"));
}

#[test]
fn malformed_endpoints_metadata_and_duplicate_names_refuse() {
    for text in [
        r#"{"providers":{"p":{"protocol":"unknown","models":{}}}}"#,
        r#"{"providers":{"p":{"protocol":"openai_responses"},"p":{"protocol":"openai_responses"}}}"#,
        r#"{"providers":{"p":{"protocol":"openai_responses","models":{"m":{},"m":{}}}}}"#,
    ] {
        assert!(serde_json::from_str::<Config>(text).is_err());
    }
    for url in [
        "file:///tmp/x",
        "https://secret@example.com/v1",
        "https://example.com/v1?key=secret",
        "https://example.com/v1#frag",
    ] {
        let mut cfg = config();
        cfg.providers.get_mut("first").unwrap().base_url = url.into();
        let error = validate(&cfg).unwrap_err();
        assert!(!error.contains("secret"));
    }
    let mut cfg = config();
    cfg.providers
        .get_mut("first")
        .unwrap()
        .models
        .get_mut("org/model")
        .unwrap()
        .reasoning_controls = Some(ReasoningControls::Effort {
        values: vec![ReasoningEffort::High, ReasoningEffort::High],
    });
    assert!(validate(&cfg).is_err());
}

#[test]
fn actual_model_settings_remain_isolated_from_global_legacy_effort() {
    let mut cfg = config();
    cfg.extra.insert(
        "model_kwargs".into(),
        serde_json::json!({"reasoning_effort":"max"}),
    );
    let mut options = BTreeMap::from([("reasoning_effort".into(), Value::String("max".into()))]);
    crate::model_controls::apply(&cfg, "second/org/model", &mut options).unwrap();
    assert!(!options.contains_key("reasoning_effort"));
    assert!(!options.contains_key("craftsman_reasoning"));
    assert_eq!(
        crate::model_controls::selected_effort(&cfg, "second/org/model").unwrap(),
        Some(None)
    );
}

#[test]
fn known_non_coding_models_are_not_selectable_or_reintroduced_by_discovery() {
    let mut cfg = config();
    cfg.model = Some("second/org/model".into());
    cfg.providers
        .get_mut("first")
        .unwrap()
        .models
        .get_mut("org/model")
        .unwrap()
        .capabilities = Some(ModelCapabilities {
        tool_calling: Some(false),
        ..Default::default()
    });
    assert!(selection(&cfg, "first/org/model").is_err());
    let models = crate::model_controls::merge_catalog(
        &cfg,
        vec![CatalogModel::named("first/org/model".into())],
    )
    .unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "second/org/model");
}

#[test]
fn invalid_budget_changes_are_atomic_and_legacy_generation_does_not_leak() {
    let mut cfg:Config=serde_json::from_value(serde_json::json!({"model":"p/m","model_kwargs":{"temperature":99},"providers":{"p":{"protocol":"anthropic_messages","models":{"m":{"reasoning_controls":{"type":"budget","min":1024,"max":16000},"settings":{"reasoning":null}}}}}})).unwrap();
    let original = serde_json::to_value(&cfg).unwrap();
    assert!(set_budget(&mut cfg, "p/m", 8192).is_err());
    assert_eq!(serde_json::to_value(&cfg).unwrap(), original);
    set_budget(&mut cfg, "p/m", 4096).unwrap();
    let mut options = BTreeMap::from([
        ("temperature".into(), serde_json::json!(99)),
        (
            "default_headers".into(),
            serde_json::json!({"X-Old-Profile":"old"}),
        ),
    ]);
    crate::model_controls::apply(&cfg, "p/m", &mut options).unwrap();
    assert!(!options.contains_key("temperature"));
    assert!(!options.contains_key("default_headers"));
    assert_eq!(options["craftsman_reasoning"]["value"], 4096);
}
