//! Private request fixtures pair ORIGINAL Prepared/effective options and actual
//! shared-builder gate. Synthetic control/usage is not a live billing/quality gate.
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::provider::request::{Fault, Source};
use crate::exec::runtime::InvocationGate;
use craftsman_harness::{ModelRequest, Msg, ToolCall, ToolDef, ToolErrorKind};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    server: MockServer,
}
struct Harness {
    _session: Session,
    client: LlmClient,
    twin: LlmClient,
    provider: Arc<dyn ModelProvider>,
    source: Arc<Source>,
    gate: Arc<InvocationGate>,
    filesystem: Arc<crate::exec::confined_fs::TextFs>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let config = root.path().join("selected");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        Self {
            _root: root,
            workspace,
            config,
            server: MockServer::start().await,
        }
    }
    async fn harness(&self, extra: Value, selectors: Selectors) -> Harness {
        let mut config = json!({"model":"synthetic","api_key":"synthetic-secret","search_api_key":"search-secret","base_url":self.server.uri()});
        config
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        std::fs::write(self.config.join("config.json"), config.to_string()).unwrap();
        let prepared = Prepared::prepare_with_environment_for_tests(
            Options {
                workspace: self.workspace.clone(),
                config_dir: self.config.clone(),
                session: SessionSelection::Ephemeral,
                trust_workspace: false,
                selectors,
            },
            BTreeMap::new(),
        )
        .await
        .unwrap();
        let (session, gate) = build_prepared_session(&prepared).await.unwrap();
        let model = prepared.config().model.as_deref().unwrap();
        let mut options = prepared.config().model_profile(model);
        if let Some(v) = prepared
            .config()
            .extra
            .get("model_kwargs")
            .and_then(Value::as_object)
        {
            options.extend(v.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        let mut agents =
            crate::extensions::effective_agents(prepared.discovery().extensions.clone())
                .into_iter()
                .map(|a| a.name)
                .collect::<Vec<_>>();
        agents.push("general-purpose".into());
        let scope = || ProviderScope {
            prepared: &prepared,
            gate: gate.clone(),
            model,
            options: &options,
            usage: session.usage.clone(),
            agents: &agents,
        };
        let client = LlmClient::headless(scope()).unwrap();
        let twin = LlmClient::headless(scope()).unwrap();
        let provider: Arc<dyn ModelProvider> =
            Arc::new(crate::exec::provider::Provider::from_scope(scope()).unwrap());
        let source = client.request_source_for_tests().unwrap();
        Harness {
            _session: session,
            client,
            twin,
            provider,
            source,
            gate: gate.clone(),
            filesystem: Arc::new(crate::exec::confined_fs::TextFs::from_scope(
                FilesystemScope {
                    prepared: &prepared,
                    gate,
                },
            )),
        }
    }
    async fn success(&self) {
        Mock::given(method("POST")).and(path("/chat/completions")).respond_with(ResponseTemplate::new(200).insert_header("content-type","text/event-stream").set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"ORIGINAL response\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")).mount(&self.server).await;
    }
}
fn request() -> ModelRequest {
    ModelRequest {
        system: "system synthetic-secret context".into(),
        messages: vec![
            Msg::user("search-secret user"),
            Msg::Assistant {
                content: "synthetic-secret assistant".into(),
                tool_calls: vec![ToolCall {
                    id: "call-one".into(),
                    name: "read_file".into(),
                    args: json!({"file_path":"search-secret/path","nested":{"safe_key":"synthetic-secret"}}),
                }],
            },
            Msg::tool("call-one", "search-secret result"),
        ],
        tools: vec![ToolDef {
            name: "read_file".into(),
            description: "synthetic-secret description".into(),
            parameters: json!({"type":"object","properties":{"file_path":{"type":"string","description":"search-secret field"}},"required":["file_path"]}),
        }],
    }
}
fn body(h: &Harness, req: &ModelRequest) -> Value {
    serde_json::from_slice(h.source.build(req).unwrap().bytes()).unwrap()
}
fn empty() -> ModelRequest {
    ModelRequest {
        system: "safe".into(),
        messages: vec![],
        tools: vec![],
    }
}
#[tokio::test]
async fn request_views_redact_all_text_and_decoded_arguments_without_mutating_original_state() {
    let f = Fixture::new().await;
    let h = f.harness(json!({}), Selectors::default()).await;
    let original = request();
    let view = body(&h, &original);
    let encoded = view.to_string();
    assert!(!encoded.contains("synthetic-secret") && !encoded.contains("search-secret"));
    assert_eq!(view["model"], "synthetic");
    assert_eq!(view["stream"], true);
    assert_eq!(view["stream_options"]["include_usage"], true);
    let call = &view["messages"][2]["tool_calls"][0];
    assert_eq!(call["id"], "call-one");
    assert_eq!(call["function"]["name"], "read_file");
    let args: Value =
        serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(
        args,
        json!({"file_path":"[REDACTED]/path","nested":{"safe_key":"[REDACTED]"}})
    );
    assert_eq!(original.system, "system synthetic-secret context");
    assert!(
        matches!(&original.messages[1],Msg::Assistant { tool_calls,.. } if tool_calls[0].args["file_path"]=="search-secret/path")
    );
    assert_eq!(
        original.tools[0].description,
        "synthetic-secret description"
    );
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn source_identity_collisions_refuse_not_rename_model_tool_names_or_call_ids() {
    let f = Fixture::new().await;
    let h = f.harness(json!({}), Selectors::default()).await;
    for field in 0..3 {
        let mut req = request();
        match field {
            0 => req.tools[0].name = "synthetic-secret".into(),
            1 => {
                if let Msg::Assistant { tool_calls, .. } = &mut req.messages[1] {
                    tool_calls[0].id = "search-secret".into();
                }
            }
            _ => {
                if let Msg::Tool { tool_call_id, .. } = &mut req.messages[2] {
                    *tool_call_id = "synthetic-secret".into();
                }
            }
        }
        assert!(matches!(h.source.build(&req), Err(Fault::Collision)));
    }
    let h = f
        .harness(json!({"search_api_key":"synthetic"}), Selectors::default())
        .await;
    assert!(matches!(h.source.build(&empty()), Err(Fault::Collision)));
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn untrusted_json_member_names_cannot_become_redacted_argument_aliases() {
    let f = Fixture::new().await;
    let h = f.harness(json!({}), Selectors::default()).await;
    let mut req = request();
    if let Msg::Assistant { tool_calls, .. } = &mut req.messages[1] {
        tool_calls[0].args = json!({"synthetic-secret":7});
    }
    assert!(matches!(h.source.build(&req), Err(Fault::Collision)));
}
#[tokio::test]
async fn schema_display_text_is_redacted_but_property_names_constraints_and_defaults_are_original()
{
    let f = Fixture::new().await;
    let h = f.harness(json!({}), Selectors::default()).await;
    let mut req = request();
    req.tools[0].parameters = json!({"type":"object","title":"synthetic-secret title","properties":{"description":{"type":"string","description":"search-secret prose"},"other":{"type":"string","enum":["one","two"],"default":"one"}},"required":["description"],"$defs":{"branch":{"type":"string","description":"synthetic-secret nested"}}});
    let view = body(&h, &req);
    let schema = &view["tools"][0]["function"]["parameters"];
    assert_eq!(schema["title"], "[REDACTED] title");
    assert_eq!(
        schema["properties"]["description"]["description"],
        "[REDACTED] prose"
    );
    assert_eq!(schema["required"], json!(["description"]));
    assert_eq!(schema["properties"]["other"]["enum"], json!(["one", "two"]));
    for keyword in ["enum", "default", "const", "pattern", "x-unknown"] {
        req.tools[0].parameters = json!({"type":"string",keyword:"synthetic-secret"});
        assert!(matches!(h.source.build(&req), Err(Fault::Collision)));
    }
}
#[tokio::test]
async fn property_maps_and_description_named_defaults_are_not_misclassified_as_display_text() {
    let f = Fixture::new().await;
    let h = f.harness(json!({}), Selectors::default()).await;
    let mut req = request();
    for schema in [
        json!({"properties":{"synthetic-secret":{"description":"safe"}}}),
        json!({"default":{"description":"search-secret"}}),
    ] {
        req.tools[0].parameters = schema;
        assert!(matches!(h.source.build(&req), Err(Fault::Collision)));
    }
}
#[tokio::test]
async fn numeric_boolean_null_protocol_and_json_syntax_collisions_fail_before_traffic() {
    for seed in ["true", "null", "messages", ":", "42"] {
        let f = Fixture::new().await;
        let h = f
            .harness(json!({"search_api_key":seed}), Selectors::default())
            .await;
        let mut req = empty();
        if seed == "null" || seed == "42" {
            req.messages.push(Msg::Assistant {
                content: "".into(),
                tool_calls: vec![ToolCall {
                    id: "id".into(),
                    name: "name".into(),
                    args: if seed == "null" {
                        json!({"n":null})
                    } else {
                        json!({"n":42})
                    },
                }],
            });
        }
        assert!(matches!(h.source.build(&req), Err(Fault::Collision)));
        assert!(f.server.received_requests().await.unwrap().is_empty());
    }
}
#[tokio::test]
async fn argument_and_outer_json_escaping_are_checked_before_sending_exact_bytes() {
    for seed in ["\\n", "\\\\n"] {
        let f = Fixture::new().await;
        let h = f
            .harness(json!({"search_api_key":seed}), Selectors::default())
            .await;
        let mut req = empty();
        req.messages.push(Msg::Assistant {
            content: "".into(),
            tool_calls: vec![ToolCall {
                id: "id".into(),
                name: "name".into(),
                args: json!({"text":"\n"}),
            }],
        });
        assert!(matches!(h.source.build(&req), Err(Fault::Collision)));
    }
}
#[tokio::test]
async fn literal_replacement_boundary_collisions_are_not_rescanned_into_safe_identity() {
    let f = Fixture::new().await;
    let h = f
        .harness(
            json!({"api_key":"secret","search_api_key":"x[REDACTED]"}),
            Selectors::default(),
        )
        .await;
    let req = ModelRequest {
        system: "xsecret".into(),
        ..empty()
    };
    assert!(matches!(h.source.build(&req), Err(Fault::Collision)));
}
#[tokio::test]
async fn effective_defaults_keep_generation_choices_and_redact_only_declared_metadata_views() {
    let f = Fixture::new().await;
    let h=f.harness(json!({"model_kwargs":{"temperature":0.2,"max_tokens":4000,"parallel_tool_calls":false,"tool_choice":"auto","metadata":{"label":"search-secret"}}}),Selectors::default()).await;
    let view = body(&h, &request());
    assert_eq!(view["temperature"], 0.2);
    assert_eq!(view["max_tokens"], 4000);
    assert_eq!(view["parallel_tool_calls"], false);
    assert_eq!(view["tool_choice"], "auto");
    assert_eq!(view["metadata"]["label"], "[REDACTED]");
    let h = f
        .harness(
            json!({"model_kwargs":{"tool_choice":"search-secret"}}),
            Selectors::default(),
        )
        .await;
    assert!(matches!(h.source.build(&empty()), Err(Fault::Collision)));
}
#[tokio::test]
async fn response_schema_options_use_display_views_without_renaming_schema_or_format_names() {
    let f = Fixture::new().await;
    let format = json!({"type":"json_schema","json_schema":{"name":"original","description":"search-secret description","strict":true,"schema":{"type":"object","description":"synthetic-secret schema","properties":{"answer":{"type":"string"}},"required":["answer"]}}});
    let h = f
        .harness(
            json!({"model_kwargs":{"response_format":format}}),
            Selectors::default(),
        )
        .await;
    let view = body(&h, &empty());
    assert_eq!(view["response_format"]["json_schema"]["name"], "original");
    assert_eq!(
        view["response_format"]["json_schema"]["description"],
        "[REDACTED] description"
    );
    assert_eq!(
        view["response_format"]["json_schema"]["schema"]["description"],
        "[REDACTED] schema"
    );
    let h=f.harness(json!({"model_kwargs":{"response_format":{"type":"json_schema","json_schema":{"name":"search-secret","schema":{}}}}}),Selectors::default()).await;
    assert!(matches!(h.source.build(&empty()), Err(Fault::Collision)));
}
#[tokio::test]
async fn fixed_body_precedence_is_preserved_but_config_is_not_an_extra_tool_source_grant() {
    let f = Fixture::new().await;
    let h=f.harness(json!({"model_kwargs":{"model":"search-secret","messages":["search-secret"],"stream":false,"stream_options":{"include_usage":false},"tools":[{"function":{"name":"gadget_call"}}]}}),Selectors::default()).await;
    let view = body(&h, &request());
    assert_eq!(view["model"], "synthetic");
    assert_eq!(view["stream"], true);
    assert_eq!(view["stream_options"]["include_usage"], true);
    assert_eq!(view["tools"].as_array().unwrap().len(), 1);
    assert_eq!(view["tools"][0]["function"]["name"], "read_file");
    assert!(matches!(h.source.build(&empty()), Err(Fault::Collision)));
}
#[tokio::test]
async fn request_source_work_depth_and_record_bounds_never_publish_a_repaired_prefix() {
    let f = Fixture::new().await;
    let h = f.harness(json!({}), Selectors::default()).await;
    assert!(matches!(
        h.source.build(&ModelRequest {
            system: "q".repeat(1024 * 1024),
            ..empty()
        }),
        Err(Fault::Limit)
    ));
    let mut req = request();
    let mut nested = json!("safe");
    for _ in 0..66 {
        nested = json!([nested]);
    }
    if let Msg::Assistant { tool_calls, .. } = &mut req.messages[1] {
        tool_calls[0].args = nested;
    }
    assert!(matches!(h.source.build(&req), Err(Fault::Limit)));
    let mut req = request();
    if let Msg::Assistant { tool_calls, .. } = &mut req.messages[1] {
        tool_calls[0].args = Value::Array(vec![Value::Null; 100_001]);
    }
    assert!(matches!(h.source.build(&req), Err(Fault::Limit)));
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn checked_bytes_are_bound_to_the_actual_client_not_just_an_equal_permission_gate() {
    let f = Fixture::new().await;
    let one = f.harness(json!({}), Selectors::default()).await;
    let two = f.harness(json!({}), Selectors::default()).await;
    let checked = one.source.build(&empty()).unwrap();
    assert!(matches!(two.source.ready(&checked), Err(Fault::Collision)));
    assert!(two.client.send_checked_for_tests(&checked).await.is_err());
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn legacy_wire_api_cannot_bypass_a_scoped_client_and_legacy_client_cannot_send_scoped_requests()
 {
    let f = Fixture::new().await;
    let h = f.harness(json!({}), Selectors::default()).await;
    assert!(
        h.client
            .stream_turn(
                &[],
                &[],
                |_| panic!("no raw output"),
                |_| panic!("no raw status")
            )
            .await
            .is_err()
    );
    let client = LlmClient::new_with_options(
        &f.server.uri(),
        "synthetic-secret",
        "synthetic",
        &BTreeMap::new(),
    );
    assert!(client.stream_model_turn(&empty()).await.is_err());
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn actual_wire_request_uses_exact_checked_body_with_original_literal_auth_and_headers() {
    let f = Fixture::new().await;
    f.success().await;
    let h = f
        .harness(
            json!({"model_kwargs":{"default_headers":{"x-original-header":"search-secret"}}}),
            Selectors::default(),
        )
        .await;
    let original = request();
    let checked = h.source.build(&original).unwrap();
    let expected = checked.bytes().to_vec();
    let turn = h.client.stream_model_turn(&original).await.unwrap();
    assert_eq!(turn.content, "ORIGINAL response");
    let network = f.server.received_requests().await.unwrap();
    assert_eq!(network.len(), 1);
    assert_eq!(network[0].body, expected);
    assert_eq!(
        network[0]
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        "Bearer synthetic-secret"
    );
    assert_eq!(
        network[0]
            .headers
            .get("x-original-header")
            .unwrap()
            .to_str()
            .unwrap(),
        "search-secret"
    );
    assert_eq!(
        network[0]
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/json"
    );
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}
#[tokio::test]
async fn replaced_config_does_not_change_source_auth_defaults_or_captured_request_seeds() {
    let f = Fixture::new().await;
    f.success().await;
    let h=f.harness(json!({"model_kwargs":{"temperature":0.2,"default_headers":{"x-original":"search-secret"}}}),Selectors::default()).await;
    std::fs::write(
        f.config.join("config.json"),
        json!({"model":"changed","api_key":"changed-secret","model_kwargs":{"temperature":0.9}})
            .to_string(),
    )
    .unwrap();
    let req = ModelRequest {
        system: "search-secret changed-secret".into(),
        ..empty()
    };
    h.client.stream_model_turn(&req).await.unwrap();
    let network = f.server.received_requests().await.unwrap();
    assert_eq!(network.len(), 1);
    let view: Value = serde_json::from_slice(&network[0].body).unwrap();
    assert_eq!(view["model"], "synthetic");
    assert_eq!(view["temperature"], 0.2);
    assert_eq!(view["messages"][0]["content"], "[REDACTED] changed-secret");
    assert_eq!(
        network[0]
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        "Bearer synthetic-secret"
    );
}
#[tokio::test]
async fn same_typed_gate_stop_refuses_prepared_payload_and_later_provider_requests_without_traffic()
{
    let f = Fixture::new().await;
    let h = f
        .harness(
            json!({}),
            Selectors {
                allow: vec!["tool:write_file".into()],
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
        )
        .await;
    let prepared = h.source.build(&empty()).unwrap();
    let out = h
        .filesystem
        .call("write_file", json!({"file_path":"build.rs","content":"no"}))
        .await;
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert!(h.gate.approval_required());
    assert!(matches!(h.source.ready(&prepared), Err(Fault::Stopped)));
    assert!(h.client.stream_model_turn(&empty()).await.is_err());
    assert!(
        f.server.received_requests().await.unwrap().is_empty()
            && !f.workspace.join("build.rs").exists()
    );
}
#[tokio::test]
async fn typed_stop_after_real_request_receipt_prevents_a_transport_retry_and_response_release() {
    let f = Fixture::new().await;
    let h = f
        .harness(
            json!({}),
            Selectors {
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
        )
        .await;
    let gate = h.gate.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            gate.observe_error_kind(ToolErrorKind::NeedsApproval);
            ResponseTemplate::new(503).set_body_string("synthetic-secret original error")
        })
        .mount(&f.server)
        .await;
    assert!(h.client.stream_model_turn(&empty()).await.is_err());
    assert!(h.gate.approval_required());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn declared_agent_identity_embedded_in_prose_cannot_become_a_redacted_dispatch_alias() {
    let f = Fixture::new().await;
    let h = f
        .harness(
            json!({"search_api_key":"general-purpose"}),
            Selectors::default(),
        )
        .await;
    assert!(matches!(h.source.build(&empty()), Err(Fault::Collision)));
    assert!(h.client.stream_model_turn(&empty()).await.is_err());
    assert!(f.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn distinct_client_sources_sharing_the_same_actual_gate_still_cannot_exchange_checked_bytes()
{
    let f = Fixture::new().await;
    let h = f.harness(json!({}), Selectors::default()).await;
    let checked = h.source.build(&empty()).unwrap();
    let twin = h.twin.request_source_for_tests().unwrap();
    assert!(matches!(twin.ready(&checked), Err(Fault::Collision)));
    assert!(h.twin.send_checked_for_tests(&checked).await.is_err());
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn mutable_legacy_model_field_cannot_override_the_frozen_scoped_request_identity() {
    let f = Fixture::new().await;
    f.success().await;
    let mut h = f.harness(json!({}), Selectors::default()).await;
    h.client.model = "caller-override".into();
    h.client.stream_model_turn(&empty()).await.unwrap();
    let network = f.server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&network[0].body).unwrap();
    assert_eq!(network.len(), 1);
    assert_eq!(body["model"], "synthetic");
}
struct RequestTransform;
#[async_trait]
impl AgentMiddleware for RequestTransform {
    fn name(&self) -> &str {
        "synthetic-transform"
    }
    async fn around_model(
        &self,
        mut request: ModelRequest,
        next: craftsman_harness::Next<'_>,
    ) -> Result<craftsman_harness::ModelResponse, AgentError> {
        request
            .system
            .push_str(" synthetic-secret after middleware");
        request.messages.push(Msg::user("search-secret injected"));
        next.run(request).await
    }
}
#[tokio::test]
async fn terminal_scoped_provider_checks_request_copies_after_middleware_transformations() {
    let f = Fixture::new().await;
    f.success().await;
    let h = f.harness(json!({}), Selectors::default()).await;
    let agent = craftsman_harness::Agent::builder(h.gate.wrap_provider(h.provider.clone()))
        .middleware(h.gate.wrap_middleware(Arc::new(RequestTransform)))
        .build();
    agent
        .run_turn("ORIGINAL synthetic-secret user")
        .await
        .unwrap();
    let network = f.server.received_requests().await.unwrap();
    assert_eq!(network.len(), 1);
    let body: Value = serde_json::from_slice(&network[0].body).unwrap();
    assert!(
        !body.to_string().contains("synthetic-secret")
            && !body.to_string().contains("search-secret")
    );
    assert!(
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"] == "[REDACTED] injected")
    );
    assert!(
        agent
            .state
            .lock()
            .unwrap()
            .messages
            .iter()
            .any(|m| matches!(m,Msg::User {content} if content=="ORIGINAL synthetic-secret user"))
    );
}
