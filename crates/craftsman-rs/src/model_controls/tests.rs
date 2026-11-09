use super::*;
use serde_json::json;

fn config(value: Value) -> Config {
    serde_json::from_value(value).unwrap()
}

#[test]
fn catalog_merges_provider_and_configured_metadata_without_losing_manual_models() {
    let cfg = config(
        json!({"model":"manual-model", "model_profiles":{"profile-model":{}},
        "model_catalog":[{"id":"provider-model","label":"My coding model","reasoning_efforts":["low","high"]}]}),
    );
    let remote = vec![CatalogModel {
        id: "provider-model".into(),
        label: "Remote label".into(),
        reasoning_efforts: vec![ReasoningEffort::Medium],
    }];
    let catalog = merge_catalog(&cfg, remote).unwrap();
    assert_eq!(
        catalog.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["manual-model", "profile-model", "provider-model"]
    );
    assert_eq!(catalog[2].label, "My coding model");
    assert_eq!(
        catalog[2].reasoning_efforts,
        vec![ReasoningEffort::Low, ReasoningEffort::High]
    );
}

#[test]
fn per_model_efforts_override_caller_kwargs_without_leaking_between_models() {
    let mut cfg = config(
        json!({"model_settings":{"a":{"reasoning_effort":"high"},"b":{"reasoning_effort":"low"}}}),
    );
    let original = BTreeMap::from([
        ("temperature".into(), json!(0.2)),
        ("reasoning_effort".into(), json!("medium")),
    ]);
    let mut a = original.clone();
    apply(&cfg, "a", &mut a).unwrap();
    let mut b = original.clone();
    apply(&cfg, "b", &mut b).unwrap();
    let mut c = original.clone();
    apply(&cfg, "c", &mut c).unwrap();
    assert_eq!(a["reasoning_effort"], "high");
    assert_eq!(b["reasoning_effort"], "low");
    assert_eq!(c, original);
    set_effort(&mut cfg, "a", Some(ReasoningEffort::Low)).unwrap();
    assert_eq!(
        selected_effort(&cfg, "b").unwrap(),
        Some(Some(ReasoningEffort::Low))
    );
    assert_eq!(a["temperature"], 0.2);
}

#[test]
fn provider_default_omits_parameter_and_is_distinct_from_literal_none() {
    let cfg = config(
        json!({"model_settings":{"default-model":{"reasoning_effort":null},"none-model":{"reasoning_effort":"none"}}}),
    );
    let original = BTreeMap::from([
        ("reasoning_effort".into(), json!("high")),
        ("max_tokens".into(), json!(128)),
    ]);
    let mut default = original.clone();
    apply(&cfg, "default-model", &mut default).unwrap();
    let mut none = original.clone();
    apply(&cfg, "none-model", &mut none).unwrap();
    assert!(!default.contains_key("reasoning_effort"));
    assert_eq!(default["max_tokens"], 128);
    assert_eq!(none["reasoning_effort"], "none");
}

#[test]
fn setter_preserves_unknown_user_data_and_refuses_undeclared_efforts() {
    let mut cfg = config(
        json!({"custom-user-data":{"keep":true},"model_catalog":[{"id":"a","reasoning_efforts":["low","medium"]}],
        "model_settings":{"a":{"keep":"value"},"other":{"reasoning_effort":"high"}}}),
    );
    let before = serde_json::to_value(&cfg).unwrap();
    assert!(set_effort(&mut cfg, "a", Some(ReasoningEffort::High)).is_err());
    assert_eq!(serde_json::to_value(&cfg).unwrap(), before);
    set_effort(&mut cfg, "a", Some(ReasoningEffort::Medium)).unwrap();
    assert_eq!(cfg.extra["custom-user-data"], json!({"keep":true}));
    assert_eq!(cfg.extra["model_settings"]["a"]["keep"], "value");
    assert_eq!(
        cfg.extra["model_settings"]["other"]["reasoning_effort"],
        "high"
    );
}

#[test]
fn selection_parses_model_and_effort_and_refuses_ignored_or_duplicate_arguments() {
    assert_eq!(
        parse_selection(["a", "--reasoning=high"].into_iter()).unwrap(),
        ModelSelection {
            model: Some("a".into()),
            effort: Some(Some(ReasoningEffort::High))
        }
    );
    assert_eq!(
        parse_selection(["--reasoning=default"].into_iter()).unwrap(),
        ModelSelection {
            model: None,
            effort: Some(None)
        }
    );
    for tokens in [
        vec!["a", "b"],
        vec!["--other=low"],
        vec!["--reasoning=high", "--reasoning=low"],
        vec!["--reasoning=guess"],
    ] {
        assert!(parse_selection(tokens.into_iter()).is_err());
    }
}

#[test]
fn malformed_and_overbound_catalogs_are_refused_without_guessing_capabilities() {
    for value in [
        json!([{"id":"a"},{"id":"a"}]),
        json!([{"id":"a","reasoning_efforts":["high","high"]}]),
        json!([{"id":"a","reasoning_efforts":["unknown"]}]),
        json!([{"id":"bad\u{1b}id"}]),
        json!({"a":{}}),
    ] {
        assert!(configured_catalog(&config(json!({"model_catalog":value}))).is_err());
    }
    assert!(
        configured_catalog(&config(json!({"model_catalog":[{"id":"x".repeat(513)}]}))).is_err()
    );
    let catalog = configured_catalog(&config(json!({"model":"unknown-model"}))).unwrap();
    assert!(catalog[0].reasoning_efforts.is_empty());
}

#[test]
fn catalog_metadata_never_enters_the_generation_options() {
    let cfg = config(
        json!({"model_catalog":[{"id":"a","label":"display label","reasoning_efforts":["high"]}],"model_settings":{"a":{"reasoning_effort":"high"}}}),
    );
    let mut options = BTreeMap::new();
    apply(&cfg, "a", &mut options).unwrap();
    assert_eq!(
        options,
        BTreeMap::from([("reasoning_effort".into(), json!("high"))])
    );
}

#[test]
fn malformed_controls_fail_fixed_and_keep_existing_options() {
    for settings in [
        json!("invalid"),
        json!({"a":[]}),
        json!({"a":{"reasoning_effort":42}}),
        json!({"a":{"reasoning_effort":"unknown"}}),
    ] {
        let cfg = config(json!({"model_settings":settings}));
        let mut options = BTreeMap::from([("max_tokens".into(), json!(128))]);
        let before = options.clone();
        assert!(apply(&cfg, "a", &mut options).is_err());
        assert_eq!(options, before);
    }
}
