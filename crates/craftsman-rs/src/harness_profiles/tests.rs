use super::{general, openai, *};

fn route(base_url: &str) -> ChatRoute {
    route_from(base_url, "litellm")
}

fn route_from(base_url: &str, source: &'static str) -> ChatRoute {
    ChatRoute {
        base_url: base_url.to_string(),
        api_key: String::new(),
        source,
    }
}

#[test]
fn unknown_model_uses_general_fallback() {
    let resolved = resolve(&Config::default(), &route("http://local/v1"), "mystery-7b");
    assert_eq!(resolved.provider, None);
    assert_eq!(resolved.harness.active_key, "general");
    assert_eq!(resolved.harness.layers, ["general"]);
}

#[test]
fn model_families_switch_profiles() {
    for (model, expected) in [
        ("gpt-5", "openai"),
        ("claude-future", "anthropic"),
        ("nemotron-future", "nvidia"),
    ] {
        let resolved = resolve(&Config::default(), &route("http://local/v1"), model);
        assert_eq!(resolved.harness.active_key, expected);
        assert_eq!(resolved.harness.layers, ["general", expected]);
    }
}

#[test]
fn known_models_resolve_to_exact_profile_keys() {
    for (model, expected) in [
        ("gpt-5.3-codex", "openai:gpt-5.3-codex"),
        ("claude-sonnet-4-6", "anthropic:claude-sonnet-4-6"),
        (
            "nvidia/nemotron-3-ultra-550b-a55b",
            "nvidia:nvidia/nemotron-3-ultra-550b-a55b",
        ),
    ] {
        let resolved = resolve(&Config::default(), &route("http://local/v1"), model);
        assert_eq!(resolved.harness.active_key, expected);
        assert_eq!(
            resolved.harness.layers,
            [
                "general".to_string(),
                expected.split(':').next().unwrap().to_string(),
                expected.to_string(),
            ]
        );
    }

    let opus = resolve(
        &Config::default(),
        &route("http://local/v1"),
        "claude-opus-4-7",
    );
    assert_eq!(opus.harness.active_key, "anthropic:claude-opus-4-7");
    assert!(
        opus.harness
            .profile
            .config
            .system_prompt_suffix
            .as_deref()
            .unwrap()
            .contains("<subagent_usage>")
    );
}

#[test]
fn selectable_presets_are_stable_and_have_friendly_aliases() {
    assert_eq!(
        harness_presets()
            .iter()
            .map(|preset| preset.id)
            .collect::<Vec<_>>(),
        ["auto", "general", "openai", "claude", "nemotron"]
    );
    assert_eq!(find_harness_preset("anthropic").unwrap().id, "claude");
    assert_eq!(find_harness_preset("nvidia").unwrap().id, "nemotron");
    assert_eq!(find_harness_preset("default").unwrap().id, "general");
    assert!(find_harness_preset("unknown").is_none());
}

#[test]
fn explicit_preset_overrides_auto_detection_without_changing_transport() {
    let mut cfg = Config::default();
    select_harness_preset(&mut cfg, "general").unwrap();
    let general = resolve(&cfg, &route("https://api.openai.com/v1"), "gpt-5");
    assert_eq!(general.harness_preset, "general");
    assert_eq!(general.harness.active_key, "general");
    assert_eq!(general.provider.as_deref(), Some("openai"));

    select_harness_preset(&mut cfg, "claude").unwrap();
    let claude = resolve(&cfg, &route("https://api.openai.com/v1"), "gpt-5");
    assert_eq!(claude.harness_preset, "claude");
    assert_eq!(claude.harness.active_key, "anthropic");
    assert_eq!(claude.provider.as_deref(), Some("openai"));

    select_harness_preset(&mut cfg, "auto").unwrap();
    assert!(!cfg.extra.contains_key("harness_profile"));
    let automatic = resolve(&cfg, &route("https://api.openai.com/v1"), "gpt-5");
    assert_eq!(automatic.harness_preset, "auto");
    assert_eq!(automatic.harness.active_key, "openai");
}

#[test]
fn explicit_provider_config_wins_over_heuristics() {
    let mut cfg = Config::default();
    cfg.extra
        .insert("model_provider".into(), Value::String("anthropic".into()));
    let resolved = resolve(&cfg, &route("https://api.openai.com/v1"), "custom-model");
    assert_eq!(resolved.provider.as_deref(), Some("anthropic"));
    assert_eq!(resolved.harness.active_key, "anthropic");
}

#[test]
fn general_profile_preserves_the_pre_profile_craftsman_harness() {
    let resolved = resolve(&Config::default(), &route("http://local/v1"), "mystery-7b");
    let profile = &resolved.harness.profile.config;
    assert_eq!(
        profile.base_system_prompt.as_deref(),
        Some(general::BASE_SYSTEM_PROMPT)
    );
    assert!(general::BASE_SYSTEM_PROMPT.contains("web fetch/search"));
    assert!(general::BASE_SYSTEM_PROMPT.contains("Long-term memory lives under /memories/"));
    let gp = profile.general_purpose_subagent.as_ref().unwrap();
    assert_eq!(gp.enabled, Some(true));
    assert_eq!(gp.description.as_deref(), Some(GENERAL_PURPOSE_DESCRIPTION));
    assert_eq!(
        gp.system_prompt.as_deref(),
        Some(GENERAL_PURPOSE_SYSTEM_PROMPT)
    );
}

#[test]
fn model_suffix_is_last_in_the_assembled_prompt() {
    let cfg = Config::default();
    let resolved = resolve(&cfg, &route("http://local/v1"), "gpt-5");
    let prompt = crate::prompt::build_system_prompt(
        &cfg,
        &resolved.harness.profile,
        "one-shot instructions",
    );
    assert!(prompt.starts_with(general::BASE_SYSTEM_PROMPT));
    assert!(prompt.contains("# Memory protocol"));
    assert!(prompt.contains("one-shot instructions"));
    assert!(prompt.ends_with(openai::SYSTEM_PROMPT_SUFFIX));
}

#[test]
fn model_family_does_not_guess_transport_provider_defaults() {
    let resolved = resolve(
        &Config::default(),
        &route("http://local/v1"),
        "nemotron-3-ultra",
    );
    assert_eq!(resolved.harness.active_key, "nvidia");
    assert_eq!(resolved.provider, None);
    assert_eq!(resolved.provider_profile.active_key, None);
}

#[test]
fn explicit_nvidia_provider_profile_adds_attribution_header() {
    let mut cfg = Config::default();
    cfg.extra
        .insert("provider_profile".into(), Value::String("nvidia".into()));
    let resolved = resolve(&cfg, &route("http://local/v1"), "nemotron-3-ultra");
    assert_eq!(resolved.provider.as_deref(), Some("nvidia"));
    assert_eq!(
        resolved.provider_profile.active_key.as_deref(),
        Some("nvidia")
    );
    assert_eq!(
        resolved.provider_profile.profile.config.init_kwargs["default_headers"],
        serde_json::json!({"X-BILLING-INVOKE-ORIGIN": "DeepAgents"})
    );
}

#[test]
fn platform_llm_endpoint_behaves_like_any_direct_provider() {
    // The platform's `/apps/llm/api/v1` is a stock OpenAI-compatible
    // endpoint now; it no longer forces "platform-managed" provider
    // defaults. An explicit provider_profile applies, and the harness
    // family still keys on the model name.
    let mut cfg = Config::default();
    cfg.extra
        .insert("provider_profile".into(), Value::String("nvidia".into()));
    let resolved = resolve(
        &cfg,
        &route_from("https://platform.example/apps/llm/api/v1", "litellm"),
        "nemotron-3-ultra",
    );
    assert_eq!(resolved.harness.active_key, "nvidia");
    assert_eq!(resolved.provider.as_deref(), Some("nvidia"));
    assert_eq!(
        resolved.provider_profile.active_key.as_deref(),
        Some("nvidia")
    );
    assert_eq!(resolved.provider_profile_mode, "explicit:nvidia");
}

#[test]
fn direct_proxy_can_disable_provider_defaults_explicitly() {
    let mut cfg = Config::default();
    cfg.extra
        .insert("model_provider".into(), Value::String("nvidia".into()));
    cfg.extra
        .insert("provider_profile".into(), Value::String("none".into()));
    let resolved = resolve(&cfg, &route("http://localhost:4000/v1"), "custom-model");
    assert_eq!(resolved.harness.active_key, "nvidia");
    assert_eq!(resolved.provider, None);
    assert_eq!(resolved.provider_profile_mode, "disabled");
}

#[test]
fn third_party_transport_can_still_select_the_nemotron_harness() {
    let resolved = resolve(
        &Config::default(),
        &route("http://local/v1"),
        "openrouter:nvidia/nemotron-3-ultra-550b-a55b",
    );
    assert_eq!(resolved.provider.as_deref(), Some("openrouter"));
    assert_eq!(resolved.model_id, "nvidia/nemotron-3-ultra-550b-a55b");
    assert_eq!(
        resolved.harness.active_key,
        "nvidia:nvidia/nemotron-3-ultra-550b-a55b"
    );
    assert_eq!(resolved.provider_profile.active_key, None);
}
