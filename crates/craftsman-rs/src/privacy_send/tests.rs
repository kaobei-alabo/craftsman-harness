//! These use an actual isolated local vault with a host-supplied synthetic key
//! and host-supplied detector reports, and real loopback HTTP for the client
//! cases. No detector runs, nothing is masked or restored, and no normal
//! session, configuration, model service or user data is involved.

use super::*;
use crate::llm::{LlmClient, Message, ToolCall};
use crate::privacy_vault::{
    Category, DeclaredIdentifiers, DetectorReport, DetectorSpan, HostKey, HostKeySource,
    HostNamespace, IdentifierKind, Limits as VaultLimits, MaskedText, Vault, VaultLocation,
};
use crate::provider_config::Protocol;
use craftsman_harness::{ModelRequest, Msg};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

pub(crate) const EMAIL: &str = "fixture.person@example.test";
const PHONE: &str = "5550142";
const CUSTOMER: &str = "CUSTOMER-4821";

struct SyntheticKey;
impl HostKeySource for SyntheticKey {
    fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([0x5a; 32])))
    }
}

/// An actual vault under an owned private root, and one live session.
pub(crate) async fn open(root: &Path, thread: &str) -> (Vault, Arc<MappingSession>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let vault = Vault::open(
        VaultLocation::under_host_config_root(&std::fs::canonicalize(root).unwrap()).unwrap(),
        &SyntheticKey,
        VaultLimits::default(),
    )
    .await
    .unwrap();
    let session = session(&vault, thread);
    (vault, session)
}

fn session(vault: &Vault, thread: &str) -> Arc<MappingSession> {
    let namespace =
        HostNamespace::supplied_by_host("owned-host", "owned-project", thread, "v1", "send")
            .unwrap();
    Arc::new(vault.session(namespace).unwrap())
}

/// Real mappings: the host supplies the detector report for synthetic text.
pub(crate) async fn mask(session: &MappingSession, values: &[(&str, Category)]) -> MaskedText {
    let source = values
        .iter()
        .map(|(text, _)| *text)
        .collect::<Vec<_>>()
        .join("\n");
    let input = session.capture_text(&source).unwrap();
    let mut spans = Vec::new();
    let mut identifiers = Vec::new();
    for &(value, category) in values {
        for (start, _) in source.match_indices(value) {
            spans.push(DetectorSpan {
                start,
                end: start + value.len(),
                category,
            });
        }
        if let Category::Identifier(kind) = category {
            identifiers.push((kind, value));
        }
    }
    let digest = input.input_sha256_for_host_detector().unwrap();
    session
        .mask(
            input,
            DetectorReport {
                input_sha256: digest,
                spans,
            },
            DeclaredIdentifiers::supplied_by_host(&identifiers).unwrap(),
        )
        .await
        .unwrap()
}

pub(crate) fn guard_for(session: &Arc<MappingSession>, masked: &MaskedText) -> Arc<SendGuard> {
    SendGuard::from_vault(
        session.clone(),
        masked.resolution_records_for_host_fields().unwrap(),
    )
    .unwrap()
}

async fn fixture() -> (
    tempfile::TempDir,
    Vault,
    Arc<MappingSession>,
    Arc<SendGuard>,
) {
    let root = tempfile::tempdir().unwrap();
    let (vault, session) = open(root.path(), "owned-thread").await;
    let masked = mask(
        &session,
        &[
            (EMAIL, Category::Email),
            (PHONE, Category::Telephone),
            (CUSTOMER, Category::Identifier(IdentifierKind::Customer)),
        ],
    )
    .await;
    let guard = guard_for(&session, &masked);
    (root, vault, session, guard)
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// One complete streamed answer whose visible text is `parts`, in each wire's
/// own event format. Separate parts stay separate provider items.
pub(crate) fn answer(protocol: &str, parts: &[&str]) -> ResponseTemplate {
    let raw = match protocol {
        "chat" => {
            let body = json!({"choices":[{"delta":{"content":parts.concat()},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
            format!("data: {body}\n\ndata: [DONE]\n\n")
        }
        "responses" => {
            let content: Vec<Value> = parts
                .iter()
                .map(|part| json!({"type":"output_text","text":part,"annotations":[]}))
                .collect();
            let value = json!({"type":"response.completed","response":{"id":"response-fixture","status":"completed","output":[{"type":"message","id":"message-fixture","role":"assistant","status":"completed","content":content}],"usage":{"input_tokens":3,"output_tokens":2}}});
            format!("event: response.completed\ndata: {value}\n\n")
        }
        "claude" => {
            let mut events =
                vec![json!({"type":"message_start","message":{"usage":{"input_tokens":3}}})];
            for (index, part) in parts.iter().enumerate() {
                events.push(json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}));
                events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":part}}));
                events.push(json!({"type":"content_block_stop","index":index}));
            }
            events.push(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}));
            events.push(json!({"type":"message_stop"}));
            events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect()
        }
        "gemini" => {
            let mut raw = String::new();
            for (index, part) in parts.iter().enumerate() {
                let mut event = json!({"candidates":[{"content":{"parts":[{"text":part}]}}]});
                if index + 1 == parts.len() {
                    event["candidates"][0]["finishReason"] = json!("STOP");
                    event["usageMetadata"] = json!({"promptTokenCount":3,"candidatesTokenCount":2});
                }
                raw.push_str(&format!("data: {event}\n\n"));
            }
            raw
        }
        _ => panic!("unregistered fixture protocol"),
    };
    ResponseTemplate::new(200).set_body_raw(raw, "text/event-stream")
}

#[tokio::test]
async fn clean_final_bytes_are_sealed_exactly_and_stay_ready() {
    let (_root, _vault, _session, guard) = fixture().await;
    let body = bytes(&json!({"model":"fixture","messages":[
        {"role":"system","content":"Synthetic instructions with \"quotes\" and a\nline break."},
        {"role":"user","content":"An ordinary question about example.test and 555-0143."}
    ],"stream":true}));
    let sealed = guard.seal(body.clone(), &[]).unwrap();
    assert_eq!(sealed.bytes(), body.as_slice());
    guard.ready(&sealed, &[]).unwrap();
    guard.ready(&sealed, &[]).unwrap();
    assert_eq!(sealed.into_bytes(), body);
}

#[tokio::test]
async fn known_originals_are_refused_in_every_supported_final_encoding() {
    let (_root, _vault, _session, guard) = fixture().await;
    let arguments = json!({"file_path":"notes.txt","content":format!("Write to {EMAIL} today.")});
    let document = json!({"contact":{"note":format!("\"primary\": {EMAIL}")}}).to_string();
    let nested = json!({"content":json!({"inner":document}).to_string()}).to_string();
    let cases: Vec<(&str, Value)> = vec![
        (
            "user text",
            json!({"messages":[{"role":"user","content":format!("Reach me at {EMAIL}.")}]}),
        ),
        (
            "string with escapes around it",
            json!({"instructions":format!("Line one\n\"quoted\" {CUSTOMER}\ttab")}),
        ),
        ("object key", json!({"metadata":{EMAIL:"owner"}})),
        (
            "number token",
            json!({"tools":[{"parameters":{"default":5_550_142_u64}}]}),
        ),
        (
            "tool-call argument JSON carried as a string",
            json!({"messages":[Message::assistant_with_calls(None, &[ToolCall {
                id: "call-1".into(),
                name: "write_file".into(),
                arguments: arguments.to_string(),
            }])]}),
        ),
        (
            "tool-call argument JSON carried as an object",
            json!({"contents":[{"role":"model","parts":[{"functionCall":{"name":"write_file","args":arguments}}]}]}),
        ),
        (
            "JSON documents nested three strings deep",
            json!({"input":[{"type":"function_call","arguments":nested}]}),
        ),
    ];
    for (label, body) in cases {
        let encoded = bytes(&body);
        assert_eq!(
            guard.seal(encoded, &[]).unwrap_err(),
            SendRefusal::KnownOriginal,
            "{label}"
        );
    }

    // Escapes a minimal encoder would not emit. The bytes as sent do not hold
    // the original; only decoding the inner document or the string finds it.
    let provider_escaped = r#"{"content":"fixture.person\u0040example.test"}"#;
    let inner = bytes(&json!({"input":[{"type":"function_call","arguments":provider_escaped}]}));
    assert!(!contains(&inner, EMAIL));
    assert_eq!(
        guard.seal(inner, &[]).unwrap_err(),
        SendRefusal::KnownOriginal
    );
    let outer =
        br#"{"messages":[{"role":"user","content":"fixture.person\u0040example.test"}]}"#.to_vec();
    assert!(!contains(&outer, EMAIL));
    assert_eq!(
        guard.seal(outer, &[]).unwrap_err(),
        SendRefusal::KnownOriginal
    );
}

#[tokio::test]
async fn a_text_node_split_across_encoded_parts_needs_the_request_text() {
    let (_root, _vault, _session, guard) = fixture().await;
    let (head, tail) = EMAIL.split_at(15);
    let split =
        bytes(&json!({"contents":[{"role":"model","parts":[{"text":head},{"text":tail}]}]}));
    assert!(!contains(&split, EMAIL));
    // The request's own joined text is how a split replay item is caught.
    assert_eq!(
        guard
            .seal(split.clone(), &["Earlier question", EMAIL])
            .unwrap_err(),
        SendRefusal::KnownOriginal
    );
    // Without it, a value split across separate strings is outside the
    // supported representations and is not detected.
    guard.seal(split, &[]).unwrap();
}

#[tokio::test]
async fn unsupported_or_out_of_bound_bodies_are_refused_not_passed() {
    let root = tempfile::tempdir().unwrap();
    let (_vault, session) = open(root.path(), "owned-thread").await;
    let masked = mask(&session, &[(EMAIL, Category::Email)]).await;
    let records = masked.resolution_records_for_host_fields().unwrap();
    let guard = SendGuard::from_vault(session.clone(), records).unwrap();
    for body in [
        b"plain text is not a model request".to_vec(),
        b"{} {}".to_vec(),
        b"{\"unterminated\":".to_vec(),
        vec![b'"', 0xff, b'"'],
    ] {
        assert_eq!(guard.seal(body, &[]).unwrap_err(), SendRefusal::Unsupported);
    }

    let tight = |change: fn(&mut Limits)| {
        let mut limits = Limits::default();
        change(&mut limits);
        SendGuard::with_limits(session.clone(), records, limits).unwrap()
    };
    let clean = bytes(&json!({"messages":[{"role":"user","content":"x".repeat(256)}]}));
    assert_eq!(
        tight(|limits| limits.body_bytes = 64)
            .seal(clean.clone(), &[])
            .unwrap_err(),
        SendRefusal::Bounds
    );
    assert_eq!(
        tight(|limits| limits.work = 128)
            .seal(clean.clone(), &[])
            .unwrap_err(),
        SendRefusal::Bounds
    );
    assert_eq!(
        tight(|limits| limits.work = 600)
            .seal(clean.clone(), &["y".repeat(512).as_str()])
            .unwrap_err(),
        SendRefusal::Bounds
    );
    let deep = tight(|limits| limits.containers = 3);
    deep.seal(b"[[[1]]]".to_vec(), &[]).unwrap();
    assert_eq!(
        deep.seal(b"[[[[1]]]]".to_vec(), &[]).unwrap_err(),
        SendRefusal::Bounds
    );
    assert_eq!(
        guard.seal(vec![b'['; 200], &[]).unwrap_err(),
        SendRefusal::Bounds
    );

    // A document inside a document inside the body: decoded by default,
    // refused when one fewer embedded level is allowed.
    let second = json!({"value":"a \"quoted\" word"}).to_string();
    let first = json!({"content":second}).to_string();
    let body = bytes(&json!({"arguments":first}));
    guard.seal(body.clone(), &[]).unwrap();
    assert_eq!(
        tight(|limits| limits.embedded = 1)
            .seal(body.clone(), &[])
            .unwrap_err(),
        SendRefusal::Bounds
    );
    tight(|limits| limits.embedded = 2).seal(body, &[]).unwrap();

    // Ceilings can only be tightened.
    let raised = Limits {
        body_bytes: Limits::default().body_bytes + 1,
        ..Limits::default()
    };
    assert_eq!(
        SendGuard::with_limits(session.clone(), records, raised).unwrap_err(),
        SendRefusal::Bounds
    );
}

#[tokio::test]
async fn originals_outside_the_supported_shape_close_the_guard() {
    let root = tempfile::tempdir().unwrap();
    let (_vault, session) = open(root.path(), "owned-thread").await;
    let supported = mask(&session, &[(EMAIL, Category::Email)]).await;
    let quoted = mask(
        &session,
        &[(
            "ACCT \"42\" north",
            Category::Identifier(IdentifierKind::Account),
        )],
    )
    .await;
    let slashed = mask(
        &session,
        &[(
            r"PROJECT\alpha",
            Category::Identifier(IdentifierKind::Project),
        )],
    )
    .await;
    let clean = bytes(&json!({"messages":[{"role":"user","content":"hello"}]}));
    for unsupported in [&quoted, &slashed] {
        let records = unsupported.resolution_records_for_host_fields().unwrap();
        assert_eq!(
            SendGuard::from_vault(session.clone(), records).unwrap_err(),
            SendRefusal::Unsupported
        );
        // Admitting it later must not leave a guard that silently ignores it.
        let guard = guard_for(&session, &supported);
        let sealed = guard.seal(clean.clone(), &[]).unwrap();
        assert_eq!(guard.admit(records).unwrap_err(), SendRefusal::Unsupported);
        assert_eq!(guard.ready(&sealed, &[]).unwrap_err(), SendRefusal::Closed);
        assert_eq!(
            guard.seal(clean.clone(), &[]).unwrap_err(),
            SendRefusal::Closed
        );
        assert_eq!(guard.check().unwrap_err(), SendRefusal::Closed);
    }

    let guard = SendGuard::with_limits(
        session.clone(),
        supported.resolution_records_for_host_fields().unwrap(),
        Limits {
            originals: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    let more = mask(&session, &[(PHONE, Category::Telephone)]).await;
    assert_eq!(
        guard
            .admit(more.resolution_records_for_host_fields().unwrap())
            .unwrap_err(),
        SendRefusal::Bounds
    );
    assert_eq!(guard.seal(clean, &[]).unwrap_err(), SendRefusal::Closed);
}

#[tokio::test]
async fn records_and_seals_of_another_session_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let (vault, first) = open(root.path(), "first-thread").await;
    let second = session(&vault, "second-thread");
    let first_masked = mask(&first, &[(EMAIL, Category::Email)]).await;
    let second_masked = mask(&second, &[(EMAIL, Category::Email)]).await;
    let first_records = first_masked.resolution_records_for_host_fields().unwrap();
    let second_records = second_masked.resolution_records_for_host_fields().unwrap();

    // Records are bound to the session that produced them, not to equal text.
    assert_eq!(
        SendGuard::from_vault(first.clone(), second_records).unwrap_err(),
        SendRefusal::Foreign
    );
    let first_guard = SendGuard::from_vault(first.clone(), first_records).unwrap();
    let twin_guard = SendGuard::from_vault(first.clone(), first_records).unwrap();
    let second_guard = SendGuard::from_vault(second.clone(), second_records).unwrap();
    let clean = bytes(&json!({"messages":[{"role":"user","content":"hello"}]}));
    let sealed = first_guard.seal(clean.clone(), &[]).unwrap();
    assert_eq!(
        second_guard.ready(&sealed, &[]).unwrap_err(),
        SendRefusal::Foreign
    );
    // Even a guard over the same session and records is a different guard.
    assert_eq!(
        twin_guard.ready(&sealed, &[]).unwrap_err(),
        SendRefusal::Foreign
    );
    first_guard.ready(&sealed, &[]).unwrap();

    assert_eq!(
        second_guard.admit(first_records).unwrap_err(),
        SendRefusal::Foreign
    );
    assert_eq!(
        second_guard.seal(clean, &[]).unwrap_err(),
        SendRefusal::Closed
    );
}

#[tokio::test]
async fn a_seal_is_verified_again_when_more_originals_are_admitted() {
    let root = tempfile::tempdir().unwrap();
    let (_vault, session) = open(root.path(), "owned-thread").await;
    let first = mask(&session, &[(EMAIL, Category::Email)]).await;
    let guard = guard_for(&session, &first);
    let body = bytes(
        &json!({"messages":[{"role":"user","content":format!("Order {CUSTOMER} is ready.")}]}),
    );
    let sealed = guard.seal(body.clone(), &[]).unwrap();
    let before = guard.admitted_generation();

    // The same records again change nothing and leave the seal current.
    guard
        .admit(first.resolution_records_for_host_fields().unwrap())
        .unwrap();
    assert_eq!(guard.admitted_generation(), before);
    guard.ready(&sealed, &[]).unwrap();

    // An unrelated new original: the seal is checked again and still holds.
    let unrelated = mask(&session, &[(PHONE, Category::Telephone)]).await;
    guard
        .admit(unrelated.resolution_records_for_host_fields().unwrap())
        .unwrap();
    assert_eq!(guard.admitted_generation(), before + 1);
    guard.ready(&sealed, &[]).unwrap();

    // The body's own value is admitted: the earlier seal is no longer usable.
    let present = mask(
        &session,
        &[(CUSTOMER, Category::Identifier(IdentifierKind::Customer))],
    )
    .await;
    guard
        .admit(present.resolution_records_for_host_fields().unwrap())
        .unwrap();
    assert_eq!(
        guard.ready(&sealed, &[]).unwrap_err(),
        SendRefusal::KnownOriginal
    );
    assert_eq!(
        guard.ready(&sealed, &[]).unwrap_err(),
        SendRefusal::KnownOriginal
    );
    assert_eq!(
        guard.seal(body, &[]).unwrap_err(),
        SendRefusal::KnownOriginal
    );
    // An ordinary refusal does not close the guard for later clean requests.
    guard.seal(bytes(&json!({"messages":[]})), &[]).unwrap();
}

#[tokio::test]
async fn a_closed_session_vault_or_guard_refuses_seal_and_ready() {
    let clean = bytes(&json!({"messages":[{"role":"user","content":"hello"}]}));
    let (_root, _vault, session, guard) = fixture().await;
    let sealed = guard.seal(clean.clone(), &[]).unwrap();
    session.close();
    assert_eq!(guard.ready(&sealed, &[]).unwrap_err(), SendRefusal::Closed);
    assert_eq!(
        guard.seal(clean.clone(), &[]).unwrap_err(),
        SendRefusal::Closed
    );
    assert_eq!(guard.check().unwrap_err(), SendRefusal::Closed);

    let (_root, vault, _session, guard) = fixture().await;
    let sealed = guard.seal(clean.clone(), &[]).unwrap();
    vault.close();
    assert_eq!(guard.ready(&sealed, &[]).unwrap_err(), SendRefusal::Closed);
    assert_eq!(
        guard.seal(clean.clone(), &[]).unwrap_err(),
        SendRefusal::Closed
    );

    let (_root, _vault, session, guard) = fixture().await;
    let sealed = guard.seal(clean.clone(), &[]).unwrap();
    guard.close();
    assert_eq!(guard.ready(&sealed, &[]).unwrap_err(), SendRefusal::Closed);
    assert_eq!(guard.seal(clean, &[]).unwrap_err(), SendRefusal::Closed);
    // Closing a guard is its own decision; the vault session is untouched.
    session.capture_text("still live").unwrap();
}

#[tokio::test]
async fn refusals_and_private_types_expose_only_fixed_text() {
    let (_root, _vault, _session, guard) = fixture().await;
    let sealed = guard
        .seal(
            bytes(&json!({"messages":[{"role":"user","content":"visible only to the wire"}]})),
            &[],
        )
        .unwrap();
    assert_eq!(format!("{guard:?}"), "SendGuard([private])");
    assert_eq!(format!("{sealed:?}"), "Sealed([private])");
    let refusal = guard
        .seal(
            bytes(&json!({"content":format!("{EMAIL} {PHONE} {CUSTOMER}")})),
            &[],
        )
        .unwrap_err();
    let error = anyhow::Error::new(refusal);
    for text in [
        format!("{error}"),
        format!("{error:?}"),
        format!("{error:#}"),
    ] {
        for private in [EMAIL, PHONE, CUSTOMER, "example.test"] {
            assert!(!text.contains(private));
        }
    }
    for refusal in [
        SendRefusal::Closed,
        SendRefusal::KnownOriginal,
        SendRefusal::Bounds,
        SendRefusal::Unsupported,
        SendRefusal::Unsealed,
        SendRefusal::Foreign,
    ] {
        assert!(refusal.to_string().ends_with("not sent."));
    }
}

async fn server(protocol: &'static str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(answer(protocol, &["fixture answer"]))
        .mount(&server)
        .await;
    server
}

fn chat(server: &MockServer) -> LlmClient {
    LlmClient::new_with_options(
        &server.uri(),
        "fixture-key",
        "fixture-model",
        &BTreeMap::new(),
    )
}

fn refusal(error: &anyhow::Error) -> SendRefusal {
    *error
        .downcast_ref::<SendRefusal>()
        .expect("the fixed private send refusal")
}

#[tokio::test]
async fn a_bound_chat_client_sends_the_same_bytes_and_headers_as_an_unbound_one() {
    let (_root, _vault, _session, guard) = fixture().await;
    let server = server("chat").await;
    let messages = vec![
        Message::System {
            content: "Synthetic \"system\" text.\nSecond line.".into(),
        },
        Message::User {
            content: "An ordinary question.".into(),
        },
        Message::assistant_with_calls(
            None,
            &[ToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                arguments: json!({"file_path":"notes \"draft\".txt"}).to_string(),
            }],
        ),
        Message::Tool {
            content: "ordinary file text".into(),
            tool_call_id: "call-1".into(),
        },
    ];
    let tools = vec![
        json!({"type":"function","function":{"name":"read_file","description":"Read a file","parameters":{"type":"object","properties":{"file_path":{"type":"string"}}}}}),
    ];
    let unbound = chat(&server)
        .stream_turn(&messages, &tools, |_| {}, |_| {})
        .await
        .unwrap();
    let mut shown = String::new();
    let bound = chat(&server)
        .with_send_guard(guard)
        .unwrap()
        .stream_turn(&messages, &tools, |text| shown.push_str(text), |_| {})
        .await
        .unwrap();
    assert_eq!(bound.content, unbound.content);
    assert_eq!(shown, "fixture answer");
    assert_eq!(
        (bound.usage.input_tokens, bound.usage.output_tokens),
        (unbound.usage.input_tokens, unbound.usage.output_tokens)
    );
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].body, received[1].body);
    assert_eq!(received[0].url, received[1].url);
    for name in ["content-type", "accept", "accept-encoding", "authorization"] {
        assert_eq!(received[0].headers.get(name), received[1].headers.get(name));
        assert!(received[1].headers.get(name).is_some());
    }
}

#[tokio::test]
async fn a_bound_chat_client_sends_nothing_for_known_unsealed_or_foreign_bodies() {
    let root = tempfile::tempdir().unwrap();
    let (vault, first) = open(root.path(), "first-thread").await;
    let masked = mask(&first, &[(EMAIL, Category::Email)]).await;
    let guard = guard_for(&first, &masked);
    let other = session(&vault, "second-thread");
    let other_masked = mask(&other, &[(EMAIL, Category::Email)]).await;
    let other_guard = guard_for(&other, &other_masked);
    let server = server("chat").await;
    let bound = chat(&server).with_send_guard(guard.clone()).unwrap();

    let call = ToolCall {
        id: "call-1".into(),
        name: "write_file".into(),
        arguments: json!({"content":format!("to {EMAIL}")}).to_string(),
    };
    for messages in [
        vec![Message::User {
            content: format!("Reach me at {EMAIL}."),
        }],
        vec![Message::assistant_with_calls(None, &[call])],
        vec![Message::Tool {
            content: format!("owner: {EMAIL}"),
            tool_call_id: "call-1".into(),
        }],
    ] {
        let error = bound
            .stream_turn(&messages, &[], |_| {}, |_| {})
            .await
            .unwrap_err();
        assert_eq!(refusal(&error), SendRefusal::KnownOriginal);
    }
    let tool = json!({"type":"function","function":{"name":"mail","description":format!("Default recipient {EMAIL}")}});
    let error = bound
        .stream_turn(&[], &[tool], |_| {}, |_| {})
        .await
        .unwrap_err();
    assert_eq!(refusal(&error), SendRefusal::KnownOriginal);

    let clean = json!({"model":"fixture-model","messages":[{"role":"user","content":"hello"}],"stream":true});
    let error = bound.send_unsealed_for_tests(&clean).await.unwrap_err();
    assert_eq!(refusal(&error), SendRefusal::Unsealed);
    let foreign = other_guard.seal(bytes(&clean), &[]).unwrap();
    let error = bound.send_sealed_for_tests(&foreign).await.unwrap_err();
    assert_eq!(refusal(&error), SendRefusal::Foreign);
    let own = guard.seal(bytes(&clean), &[]).unwrap();
    let error = chat(&server).send_sealed_for_tests(&own).await.unwrap_err();
    assert_eq!(refusal(&error), SendRefusal::Foreign);

    // A named Chat connection keeps the fixed refusal instead of its generic
    // connection diagnostic, and a second guard cannot replace the first.
    let named = LlmClient::configured(
        &server.uri(),
        "fixture-key",
        "fixture-model",
        Protocol::OpenaiChatCompletions,
        "test/fixture-model",
        &BTreeMap::new(),
    )
    .unwrap()
    .with_send_guard(guard.clone())
    .unwrap();
    let error = named
        .stream_turn(
            &[Message::User {
                content: EMAIL.into(),
            }],
            &[],
            |_| {},
            |_| {},
        )
        .await
        .unwrap_err();
    assert_eq!(refusal(&error), SendRefusal::KnownOriginal);
    assert_eq!(
        named.with_send_guard(other_guard).err(),
        Some(SendRefusal::Unsupported)
    );

    assert!(server.received_requests().await.unwrap().is_empty());
    // Refusals leave the guard usable: the same client still sends clean text.
    bound
        .stream_turn(
            &[Message::User {
                content: "hello".into(),
            }],
            &[],
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_bound_chat_retry_sends_the_same_sealed_bytes_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (_root, _vault, _session, guard) = fixture().await;
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                // The existing truncated-before-output case: one retry.
                ResponseTemplate::new(200)
                    .set_body_raw("data: {\"choices\":[]}\n\n", "text/event-stream")
            } else {
                answer("chat", &["fixture answer"])
            }
        })
        .mount(&server)
        .await;
    let messages = vec![Message::User {
        content: "An ordinary question.".into(),
    }];
    let mut statuses = Vec::new();
    let result = chat(&server)
        .with_send_guard(guard)
        .unwrap()
        .stream_turn(
            &messages,
            &[],
            |_| {},
            |status| statuses.push(status.to_owned()),
        )
        .await
        .unwrap();
    assert_eq!(result.content, "fixture answer");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(
        statuses
            .iter()
            .any(|status| status.contains("retrying once"))
    );
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].body, received[1].body);
    let expected = json!({"model":"fixture-model","messages":[{"role":"user","content":"An ordinary question."}],"stream":true,"stream_options":{"include_usage":true}});
    assert_eq!(
        serde_json::from_slice::<Value>(&received[1].body).unwrap(),
        expected
    );
}

fn native(server: &MockServer, protocol: Protocol) -> LlmClient {
    LlmClient::configured(
        &server.uri(),
        "fixture-key",
        "fixture-model",
        protocol,
        "test/fixture-model",
        &BTreeMap::new(),
    )
    .unwrap()
}

fn request(text: &str) -> ModelRequest {
    ModelRequest {
        system: "Synthetic instructions".into(),
        messages: vec![Msg::user(text)],
        tools: vec![craftsman_harness::ToolDef {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: json!({"type":"object","properties":{"file_path":{"type":"string"}}}),
        }],
    }
}

#[tokio::test]
async fn bound_native_clients_send_identical_clean_bytes_and_nothing_for_a_known_original() {
    let (_root, _vault, _session, guard) = fixture().await;
    for (name, protocol) in [
        ("responses", Protocol::OpenaiResponses),
        ("claude", Protocol::AnthropicMessages),
        ("gemini", Protocol::GeminiGenerateContent),
    ] {
        let server = server(name).await;
        let unbound = native(&server, protocol);
        let bound = native(&server, protocol)
            .with_send_guard(guard.clone())
            .unwrap();
        let clean = request("An ordinary question.");
        let first = unbound
            .native
            .as_ref()
            .unwrap()
            .complete(&clean, |_| {})
            .await
            .unwrap();
        let second = bound
            .native
            .as_ref()
            .unwrap()
            .complete(&clean, |_| {})
            .await
            .unwrap();
        assert_eq!(first.message, second.message, "{name}");
        assert_eq!(first.usage, second.usage, "{name}");
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 2, "{name}");
        assert_eq!(received[0].body, received[1].body, "{name}");
        assert_eq!(received[0].url, received[1].url, "{name}");
        for header in ["content-type", "accept"] {
            assert_eq!(
                received[0].headers.get(header),
                received[1].headers.get(header),
                "{name}"
            );
        }

        let mut dirty = request("An ordinary question.");
        dirty.tools[0].description = format!("Read a file owned by {EMAIL}");
        for request in [request(&format!("Reach me at {EMAIL}.")), dirty] {
            let error = bound
                .native
                .as_ref()
                .unwrap()
                .complete(&request, |_| {})
                .await
                .unwrap_err();
            assert_eq!(refusal(&error), SendRefusal::KnownOriginal, "{name}");
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 2, "{name}");
    }
}

#[tokio::test]
async fn a_bound_native_client_refuses_a_replayed_answer_split_across_provider_parts() {
    let (_root, _vault, _session, guard) = fixture().await;
    let (head, tail) = EMAIL.split_at(15);
    for (name, protocol) in [
        ("responses", Protocol::OpenaiResponses),
        ("claude", Protocol::AnthropicMessages),
        ("gemini", Protocol::GeminiGenerateContent),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(answer(name, &[head, tail]))
            .mount(&server)
            .await;
        let bound = native(&server, protocol)
            .with_send_guard(guard.clone())
            .unwrap();
        let client = bound.native.as_ref().unwrap();
        let mut request = request("An ordinary question.");
        let response = client.complete(&request, |_| {}).await.unwrap();
        assert_eq!(
            response.message,
            Msg::assistant(EMAIL),
            "{name}: the provider's parts join into the portable answer"
        );
        // The next request replays the provider's own separate parts.
        request.messages.push(response.message);
        request.messages.push(Msg::user("A follow-up question."));
        let error = client.complete(&request, |_| {}).await.unwrap_err();
        assert_eq!(refusal(&error), SendRefusal::KnownOriginal, "{name}");
        assert_eq!(server.received_requests().await.unwrap().len(), 1, "{name}");
    }
}
