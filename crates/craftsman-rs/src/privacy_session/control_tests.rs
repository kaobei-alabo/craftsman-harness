//! The controller adapters of a privacy conversation, with the original
//! context and failure controller, the framework's own agent, a real vault
//! and guard, and the private Chat provider against a loopback server. The
//! detector is fixed host reply rules.
//!
//! Nothing here makes a callback of a running turn fail. What must be
//! refused is tried before a turn exists, or with an event whose callback
//! is over.

use super::super::tests::vault;
use super::super::*;
use super::*;
use crate::context_compiler::ContextOptions;
use crate::failure_control::Settings;
use crate::llm::LlmClient;
use crate::privacy_hook::tests::{EMAIL, LiteralSource, PHONE};
use crate::privacy_send::tests::answer;
use craftsman_harness::{Agent, Tool, ToolCtx, ToolDef, ToolOutcome, TurnResult};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const CONTEXT_KEY: &str = "_craftsman_context_v1";
const INPUT_KEY: &str = "_craftsman_failure_input_v1";

/// A file reader with a fixed answer: an error for a missing file.
struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "read_file".into(),
            description: "Read a file.".into(),
            parameters: json!({"type": "object", "properties": {"file_path": {"type": "string"}}}),
        }
    }

    async fn call(&self, args: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        Ok(match args["file_path"].as_str() {
            Some("notes.txt") => ToolOutcome::ok(format!("Their number is {PHONE}.")),
            _ => ToolOutcome::err("No such file."),
        })
    }
}

fn asks_to_read(id: &str, path: &str) -> ResponseTemplate {
    let arguments = json!({"file_path": path}).to_string();
    let body = json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": id, "type": "function",
        "function": {"name": "read_file", "arguments": arguments}}]}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 1}});
    ResponseTemplate::new(200).set_body_raw(
        format!("data: {body}\n\ndata: [DONE]\n\n"),
        "text/event-stream",
    )
}

async fn scripted(replies: Vec<ResponseTemplate>) -> MockServer {
    let server = MockServer::start().await;
    for (place, reply) in replies.into_iter().enumerate() {
        Mock::given(method("POST"))
            .respond_with(reply)
            .up_to_n_times(1)
            .with_priority(place as u8 + 1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .respond_with(answer("chat", &["fixture answer"]))
        .mount(&server)
        .await;
    server
}

async fn sent(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect()
}

/// One conversation that is not built yet: the session, the boundary of its
/// root agent, and the original controller a session would configure.
struct Unbuilt {
    session: Arc<PrivacySession>,
    boundary: Arc<ConversationBoundary>,
    controls: Arc<AppControls>,
    provider: Arc<ViewChatProvider>,
}

async fn unbuilt(vault: &Vault, server: &MockServer) -> Unbuilt {
    let source = LiteralSource::new(&[(EMAIL, "email"), (PHONE, "phone")]);
    let session = Arc::new(
        PrivacySession::open(
            vault,
            FreshConversationBinding::reserve(&std::env::temp_dir()).unwrap(),
            Detector::from_test_source(source),
        )
        .await
        .unwrap(),
    );
    let boundary = session.boundary(&[]).unwrap();
    let settings = Settings::parse(Some(&json!({"repeat_limit": 5})))
        .unwrap()
        .unwrap();
    let controls = AppControls::new(
        ContextOptions {
            enabled: true,
            max_bytes: 4096,
        },
        Some(&settings),
        &std::env::temp_dir(),
    )
    .unwrap();
    let client = LlmClient::new_with_options(
        &server.uri(),
        "fixture-key",
        "fixture-model",
        &BTreeMap::new(),
    );
    let provider = ViewChatProvider::new(
        client,
        session.clone(),
        Arc::default(),
        Arc::new(|_: &str| {}),
    )
    .await
    .unwrap();
    Unbuilt {
        session,
        boundary,
        controls,
        provider: Arc::new(provider),
    }
}

impl Unbuilt {
    /// The agent as the session builder makes it: the original
    /// controller's own middlewares, and `policy` as the installed
    /// controller.
    fn agent(&self, policy: Option<Arc<dyn AgentBoundaryPolicy>>) -> Agent {
        let controls = &self.controls;
        let mut builder = Agent::builder(self.provider.clone())
            .tool(Arc::new(ReadFile))
            .middleware(controls.prepare())
            .middleware(controls.context.compiler())
            .middleware(controls.context.observer_with_trust(Arc::new(|_| true)))
            .middleware(controls.context.request_observer())
            .middleware(controls.observer())
            .model_view_boundary(Some(self.boundary.clone()));
        if let Some(policy) = policy {
            builder = builder.boundary_policy(policy);
        }
        builder.build()
    }

    async fn typed(&self, text: &str) -> SourceRef {
        let raw = CapturedRaw::from_producer(RawOrigin::TerminalDraft, text);
        let node = self.session.admit_raw(&raw).await.unwrap();
        self.session.source(&node).unwrap()
    }
}

fn system_of(request: &Value) -> &str {
    request["messages"][0]["content"].as_str().unwrap()
}

#[tokio::test]
async fn the_original_controller_runs_behind_its_adapter_on_the_agent_it_was_made_for() {
    let directory = tempfile::tempdir().unwrap();
    let vault = vault(&directory).await;
    let server = scripted(vec![
        asks_to_read("call-1", "notes.txt"),
        asks_to_read("call-2", "missing.txt"),
        asks_to_read("call-3", "missing.txt"),
    ])
    .await;
    let made = unbuilt(&vault, &server).await;
    let adapter = SelectedControls::install(&made.boundary, made.controls.clone())
        .expect("the adapter is made before the agent");
    let agent = made.agent(Some(adapter.clone()));

    let typed = format!("Who handles {EMAIL}? See notes.txt.");
    let source = made.typed(&typed).await;
    let TurnResult::Done(output) = agent.run_turn_with_view(&typed, source).await.unwrap() else {
        panic!("the turn finishes");
    };
    assert_eq!(output.final_text(), "fixture answer");

    // The record is exact, and beside it are the two keys of the original
    // controller and nothing else.
    let state = agent.state.lock().unwrap().clone();
    assert_eq!(state.messages.len(), 8);
    assert_eq!(state.messages[0], Msg::user(typed.clone()));
    assert!(matches!(&state.messages[2], Msg::Tool { content, .. } if content.contains(PHONE)));
    let mut keys: Vec<&str> = state.custom.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, [CONTEXT_KEY, INPUT_KEY]);
    assert_eq!(keys, {
        let mut own = made.controls.own_keys();
        own.sort_unstable();
        own
    });
    // The original controller recorded the input itself: its stamp is the
    // one in the context record and the one the failure controls keep.
    let stamp = made
        .controls
        .recorded_input()
        .expect("an input was recorded");
    assert_eq!(stamp.revision, 1);
    assert_eq!(state.custom[INPUT_KEY]["stamp"]["revision"], json!(1));
    assert_eq!(
        state.custom[CONTEXT_KEY]["initial_input"]["text"],
        json!(typed)
    );
    assert_eq!(
        state.custom[CONTEXT_KEY]["files"][0]["selector"],
        json!("notes.txt")
    );

    // One accepted-input event, for exactly the installed adapter.
    let events = agent.actual_boundary_inputs().unwrap();
    assert_eq!(events.len(), 1);
    let installed: Arc<dyn AgentBoundaryPolicy> = adapter.clone();
    assert!(events[0].same_controller(&installed) && !events[0].in_callback());

    let requests = sent(&server).await;
    assert_eq!(requests.len(), 4);
    for request in &requests {
        let body = request.to_string();
        assert!(!body.contains(EMAIL) && !body.contains(PHONE));
        // The section quotes the input as the input itself is seen.
        let system = system_of(request);
        let input = request["messages"][1]["content"].as_str().unwrap();
        assert!(input.starts_with("Who handles ") && !input.contains(EMAIL));
        assert!(
            system.contains(&format!("Initial request (event 1):\n{input}\n")),
            "{system}"
        );
        // No digest or size of recorded text is part of it.
        assert!(!system.contains("digest") && !system.contains("original_bytes"));
    }
    // A path is named as the call of the committed reply wrote it, once the
    // observer saw that call.
    assert!(!system_of(&requests[0]).contains("File observations"));
    assert!(system_of(&requests[1]).contains(
        "File observations (historical, not a freshness guarantee):\n- notes.txt | event 2 | read | completed=true | historical | read at event 2, offset 0, limit 100\n"
    ));
    // The failure controls saw the same error twice and say so with the
    // application's own fixed text. The turn was not paused.
    assert!(!system_of(&requests[2]).contains("returned the same error repeatedly"));
    assert!(system_of(&requests[3]).contains("returned the same error repeatedly"));
    assert!(made.controls.confirmed_pause().is_none());
    assert!(made.session.check().is_ok());

    // A second input is recorded by the same controller and quoted beside
    // the first.
    let source = made.typed("And who else?").await;
    agent
        .run_turn_with_view("And who else?", source)
        .await
        .unwrap();
    let requests = sent(&server).await;
    let system = system_of(requests.last().unwrap());
    assert!(system.contains("Initial request (event 1):\nWho handles "));
    assert!(system.contains("):\nAnd who else?\n"), "{system}");
    assert_eq!(agent.actual_boundary_inputs().unwrap().len(), 2);
    assert_eq!(made.controls.recorded_input().unwrap().revision, {
        let state = agent.state.lock().unwrap();
        state.custom[INPUT_KEY]["stamp"]["revision"]
            .as_u64()
            .unwrap()
    });
}

#[tokio::test]
async fn an_event_is_taken_only_in_its_own_callback_of_its_own_installed_adapter() {
    let directory = tempfile::tempdir().unwrap();
    let vault = vault(&directory).await;
    let server = scripted(Vec::new()).await;
    let made = unbuilt(&vault, &server).await;
    let adapter = SelectedControls::install(&made.boundary, made.controls.clone()).unwrap();
    let agent = made.agent(Some(adapter.clone()));
    let typed = format!("Who handles {EMAIL}?");
    let source = made.typed(&typed).await;
    agent.run_turn_with_view(&typed, source).await.unwrap();
    let event = agent.actual_boundary_inputs().unwrap().remove(0);
    let before = agent.state.lock().unwrap().clone();
    let stamp = made.controls.recorded_input();
    let unchanged = |state: &AgentState| {
        state.messages == before.messages
            && state.custom == before.custom
            && made.controls.recorded_input() == stamp
    };

    // The agent's own event, after its callback: refused, and the original
    // controller is not reached.
    let mut state = before.clone();
    assert!(adapter.user_input_with_view(&mut state, &event).is_err());
    assert!(unchanged(&state));
    // The entry without an event: refused.
    assert!(
        adapter
            .user_input(&mut state, 0, HumanInputKind::Initial)
            .is_err()
    );
    assert!(unchanged(&state));

    // The same event handed to the adapter of another conversation, and
    // another conversation's event handed to this one: refused.
    let other = unbuilt(&vault, &server).await;
    let foreign = SelectedControls::install(&other.boundary, other.controls.clone()).unwrap();
    let other_agent = other.agent(Some(foreign.clone()));
    let source = other.typed("Another conversation.").await;
    other_agent
        .run_turn_with_view("Another conversation.", source)
        .await
        .unwrap();
    let other_event = other_agent.actual_boundary_inputs().unwrap().remove(0);
    assert!(foreign.user_input_with_view(&mut state, &event).is_err());
    assert!(
        adapter
            .user_input_with_view(&mut state, &other_event)
            .is_err()
    );
    assert!(unchanged(&state));
    // The boundary gives no source for either, whoever asks.
    let installed: Arc<dyn AgentBoundaryPolicy> = adapter.clone();
    let foreign_installed: Arc<dyn AgentBoundaryPolicy> = foreign.clone();
    assert!(made.boundary.accepted_input(&event, &installed).is_err());
    assert!(
        made.boundary
            .accepted_input(&event, &foreign_installed)
            .is_err()
    );
    assert!(
        made.boundary
            .accepted_input(&other_event, &installed)
            .is_err()
    );
    // None of this ended either conversation.
    assert!(made.session.check().is_ok() && other.session.check().is_ok());

    // After the conversation ended the adapter refuses before the original.
    made.session.stop();
    assert!(adapter.before_model(&before).is_err());
    assert!(adapter.user_input_with_view(&mut state, &event).is_err());
    assert!(unchanged(&state));
}

/// A controller that states what the adapter states and hands out the
/// adapter's own source, but is another object.
struct Impostor(SourceRef);

impl AgentBoundaryPolicy for Impostor {
    fn view_support(&self) -> ViewSupport {
        ViewSupport::Tracked
    }
    fn source_for_view(&self) -> Option<SourceRef> {
        Some(self.0.clone())
    }
    fn user_input_with_view(
        &self,
        _: &mut AgentState,
        _: &ActualBoundaryInput,
    ) -> Result<(), AgentError> {
        panic!("a controller the conversation did not make is never called");
    }
}

#[tokio::test]
async fn an_agent_runs_only_with_exactly_the_adapters_its_conversation_made() {
    let directory = tempfile::tempdir().unwrap();
    let vault = vault(&directory).await;
    let server = scripted(Vec::new()).await;
    let typed = "A benign input.";
    // Each case: an agent that must not run. Nothing is recorded, saved or
    // sent, no controller is called, and the conversation stays live.
    let refused = |made: Unbuilt, agent: Agent| async move {
        let source = made.typed(typed).await;
        assert!(agent.run_turn_with_view(typed, source).await.is_err());
        let state = agent.state.lock().unwrap();
        assert!(state.messages.is_empty() && state.custom.is_empty());
        assert!(agent.actual_boundary_inputs().unwrap().is_empty());
        assert!(made.controls.recorded_input().is_none());
        assert!(made.session.check().is_ok());
    };

    // The original controller without an adapter.
    let made = unbuilt(&vault, &server).await;
    let agent = made.agent(Some(made.controls.clone()));
    refused(made, agent).await;

    // The adapter of another conversation.
    let other = unbuilt(&vault, &server).await;
    let foreign = SelectedControls::install(&other.boundary, other.controls.clone()).unwrap();
    let made = unbuilt(&vault, &server).await;
    let agent = made.agent(Some(foreign.clone()));
    refused(made, agent).await;

    // Another object with the adapter's own statement and source.
    let made = unbuilt(&vault, &server).await;
    let adapter = SelectedControls::install(&made.boundary, made.controls.clone()).unwrap();
    let agent = made.agent(Some(Arc::new(Impostor(adapter.source_for_view().unwrap()))));
    refused(made, agent).await;

    // An adapter that was made, and an agent built without it.
    let made = unbuilt(&vault, &server).await;
    let _adapter = SelectedControls::install(&made.boundary, made.controls.clone()).unwrap();
    let agent = made.agent(None);
    refused(made, agent).await;

    // An adapter that no longer exists.
    let made = unbuilt(&vault, &server).await;
    drop(SelectedControls::install(&made.boundary, made.controls.clone()).unwrap());
    let agent = made.agent(None);
    refused(made, agent).await;

    // An adapter is made once, before the agent, for a live conversation.
    let made = unbuilt(&vault, &server).await;
    let adapter = SelectedControls::install(&made.boundary, made.controls.clone()).unwrap();
    assert!(SelectedControls::install(&made.boundary, made.controls.clone()).is_none());
    let agent = made.agent(Some(adapter));
    let built = unbuilt(&vault, &server).await;
    let plain = built.agent(None);
    assert!(SelectedControls::install(&built.boundary, built.controls.clone()).is_none());
    let ended = unbuilt(&vault, &server).await;
    ended.session.stop();
    assert!(SelectedControls::install(&ended.boundary, ended.controls.clone()).is_none());

    // The genuine pair runs, and so does an agent without controllers on a
    // boundary without adapters. No request was sent before these two.
    assert!(server.received_requests().await.unwrap().is_empty());
    let source = made.typed(typed).await;
    assert!(agent.run_turn_with_view(typed, source).await.is_ok());
    let source = built.typed(typed).await;
    assert!(plain.run_turn_with_view(typed, source).await.is_ok());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[test]
fn a_callback_sees_one_message_and_its_own_keys_and_writes_only_those() {
    let message = Msg::user("typed");
    let mut state = AgentState::default();
    state.messages = vec![Msg::user("earlier"), message.clone()];
    state.custom.insert("own".into(), json!(1));
    state.custom.insert("theirs".into(), json!("kept"));
    let before = state.clone();

    // Its own keys are read and written: one changed, one new, one removed.
    on_own_keys(&mut state, Some(&message), &["own", "new", "gone"], |own| {
        assert_eq!(own.messages, [Msg::user("typed")]);
        assert_eq!(own.custom.len(), 1);
        own.custom.insert("own".into(), json!(2));
        own.custom.insert("new".into(), json!(true));
        Ok(())
    })
    .unwrap();
    assert_eq!(state.messages, before.messages);
    assert_eq!(state.custom["own"], json!(2));
    assert_eq!(state.custom["new"], json!(true));
    assert_eq!(state.custom["theirs"], json!("kept"));
    assert_eq!(state.custom.len(), 3);

    // What it recorded is kept when it refuses afterwards, as the ordinary
    // callback keeps it.
    let failed = on_own_keys(&mut state, Some(&message), &["own"], |own| {
        own.custom.insert("own".into(), json!(3));
        Err(AgentError::State("the original refused".into()))
    });
    assert!(matches!(failed, Err(AgentError::State(text)) if text == "the original refused"));
    assert_eq!(state.custom["own"], json!(3));

    // Anything else it changes is refused, and nothing is written back.
    let kept = state.clone();
    type Change = fn(&mut AgentState);
    let changes: [Change; 5] = [
        |own| {
            own.custom
                .insert("theirs".into(), json!("taken"))
                .map_or((), |_| ())
        },
        |own| own.messages.push(Msg::user("added")),
        |own| own.messages[0] = Msg::user("rewritten"),
        |own| own.system_extras.push("system text".into()),
        |own| own.last_usage_msg_count = 1,
    ];
    for change in changes {
        let result = on_own_keys(&mut state, Some(&message), &["own"], |own| {
            own.custom.insert("own".into(), json!(4));
            change(own);
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(state.messages, kept.messages);
        assert_eq!(state.custom, kept.custom);
    }
}

#[tokio::test]
async fn a_section_within_its_limit_leaves_out_whole_entries_and_says_so() {
    let directory = tempfile::tempdir().unwrap();
    let vault = vault(&directory).await;
    let server = scripted(vec![asks_to_read("call-1", "notes.txt")]).await;
    let mut made = unbuilt(&vault, &server).await;
    // The smallest limit the settings accept, and no failure controls.
    made.controls = AppControls::new(
        ContextOptions {
            enabled: true,
            max_bytes: 2048,
        },
        None,
        &std::env::temp_dir(),
    )
    .unwrap();
    let adapter = SelectedControls::install(&made.boundary, made.controls.clone()).unwrap();
    let agent = made.agent(Some(adapter));
    // An input that fits the record and not the section.
    let typed = format!("Who handles {EMAIL}? {}", "word ".repeat(500));
    assert!(typed.len() > 2048 && typed.len() < 4096);
    let source = made.typed(&typed).await;
    agent.run_turn_with_view(&typed, source).await.unwrap();

    // The record keeps the whole input under the controller's one key.
    let state = agent.state.lock().unwrap().clone();
    assert_eq!(state.custom.len(), 1);
    assert_eq!(
        state.custom[CONTEXT_KEY]["initial_input"]["text"],
        json!(typed)
    );
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 2);
    for request in &requests {
        let system = system_of(request);
        // The input is sent once, as the input, and the section says that
        // its text was left out. Nothing is cut in the middle.
        assert!(system.contains("Initial request (event 1): text omitted (storage_budget).\n"));
        assert!(!system.contains("word word"));
        assert!(system.contains("\"included_input_texts\":0"));
        assert!(system.contains("\"omitted_input_texts\":1"));
        assert_eq!(request.to_string().matches("Who handles ").count(), 1);
        assert!(!request.to_string().contains(EMAIL));
    }
    // Entries are left out in the ordinary section's order: the observed
    // path went first, and it is counted as left out, not listed.
    let second = system_of(&requests[1]);
    assert!(!second.contains("File observations") && !second.contains("notes.txt"));
    assert!(second.contains("\"included_files\":0") && second.contains("\"omitted_files\":1"));
    assert_eq!(
        state.custom[CONTEXT_KEY]["files"][0]["selector"],
        json!("notes.txt")
    );
}

/// The original verification controller a session would configure, for a
/// workspace it never has to read here: no source changes, so no check.
fn verification(root: &std::path::Path) -> Arc<VerificationController> {
    let mut cfg = crate::config::Config::default();
    cfg.extra.insert(
        "verification".into(),
        json!({"commands": ["exit 0"], "paths": ["src"]}),
    );
    let plan = crate::verification::VerificationPlan::load(&cfg, root)
        .unwrap()
        .expect("a declared plan");
    let shell = crate::tools::ShellRunner::new(
        root.to_path_buf(),
        crate::shell::ShellSettings::from_config(&cfg),
    );
    Arc::new(VerificationController::new(
        root.to_path_buf(),
        plan,
        Arc::new(shell),
    ))
}

/// A completion check that hands out another one's source.
struct CheckImpostor(SourceRef);

#[async_trait::async_trait]
impl CompletionCheck for CheckImpostor {
    fn source_for_view(&self) -> Option<SourceRef> {
        Some(self.0.clone())
    }
    async fn before_finish(&self, _: &AgentState) -> Result<CompletionAction, AgentError> {
        panic!("a completion check the conversation did not make is never called");
    }
    async fn after_check(&self, _: &AgentState) -> Result<CompletionAfterCheck, AgentError> {
        panic!("a completion check the conversation did not make is never called");
    }
}

#[tokio::test]
async fn the_completion_check_is_accepted_only_as_the_adapter_its_conversation_made() {
    let directory = tempfile::tempdir().unwrap();
    let vault = vault(&directory).await;
    let server = scripted(Vec::new()).await;
    let typed = "A benign input.";
    let with_check = |made: &Unbuilt, check: Arc<dyn CompletionCheck>| {
        Agent::builder(made.provider.clone())
            .model_view_boundary(Some(made.boundary.clone()))
            .completion_check(check)
            .build()
    };
    let refused = |made: Unbuilt, agent: Agent| async move {
        let source = made.typed(typed).await;
        assert!(agent.run_turn_with_view(typed, source).await.is_err());
        assert!(agent.state.lock().unwrap().messages.is_empty());
        assert!(made.session.check().is_ok());
    };

    // The original controller without an adapter gives no source.
    let made = unbuilt(&vault, &server).await;
    let agent = with_check(&made, verification(directory.path()));
    refused(made, agent).await;

    // The adapter of another conversation.
    let other = unbuilt(&vault, &server).await;
    let foreign =
        SelectedVerification::install(&other.boundary, verification(directory.path())).unwrap();
    let made = unbuilt(&vault, &server).await;
    let agent = with_check(&made, foreign.clone());
    refused(made, agent).await;

    // Another object with the adapter's own source.
    let made = unbuilt(&vault, &server).await;
    let adapter =
        SelectedVerification::install(&made.boundary, verification(directory.path())).unwrap();
    let source = adapter.source_for_view().unwrap();
    assert!(adapter.has_source(&source) && !foreign.has_source(&source));
    let agent = with_check(&made, Arc::new(CheckImpostor(source)));
    refused(made, agent).await;

    // An adapter that was made, and an agent built without it.
    let made = unbuilt(&vault, &server).await;
    let _adapter =
        SelectedVerification::install(&made.boundary, verification(directory.path())).unwrap();
    let agent = Agent::builder(made.provider.clone())
        .model_view_boundary(Some(made.boundary.clone()))
        .build();
    refused(made, agent).await;

    // The adapter is made once, before the agent, for a live conversation.
    let made = unbuilt(&vault, &server).await;
    let adapter =
        SelectedVerification::install(&made.boundary, verification(directory.path())).unwrap();
    assert!(
        SelectedVerification::install(&made.boundary, verification(directory.path())).is_none()
    );
    assert!(server.received_requests().await.unwrap().is_empty());

    // The genuine adapter runs. Nothing changed in the declared sources, so
    // the original controller accepts the end of the turn without a check
    // and directs none.
    let agent = with_check(&made, adapter.clone());
    let source = made.typed(typed).await;
    let TurnResult::Done(output) = agent.run_turn_with_view(typed, source).await.unwrap() else {
        panic!("the turn finishes");
    };
    assert_eq!(output.final_text(), "fixture answer");
    assert!(agent.actual_host_scheduled_calls().unwrap().is_empty());
    assert!(adapter.take_directed().is_none());
    assert!(
        SelectedVerification::install(&made.boundary, verification(directory.path())).is_none()
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_section_quotes_an_input_only_from_the_source_its_own_agent_was_given() {
    use craftsman_harness::provenance::{ModelViewBoundary, PartFrom, SourcePart};
    let directory = tempfile::tempdir().unwrap();
    let vault = vault(&directory).await;
    let server = scripted(Vec::new()).await;
    let typed = format!("Who handles {EMAIL}?");
    // Three conversations, each with one accepted input.
    let mut made = Vec::new();
    for _ in 0..3 {
        let one = unbuilt(&vault, &server).await;
        let adapter = SelectedControls::install(&one.boundary, one.controls.clone()).unwrap();
        let agent = one.agent(Some(adapter));
        let source = one.typed(&typed).await;
        agent.run_turn_with_view(&typed, source).await.unwrap();
        made.push((one, agent));
    }
    let quoted = |one: &Unbuilt| {
        let kept = one.controls.context.retained_input_sources();
        assert_eq!(kept.len(), 1);
        kept[0].clone()
    };
    let part = |text: &str, source: &SourceRef| SourcePart {
        text: Arc::from(text),
        from: PartFrom::Source {
            source: source.clone(),
        },
    };
    let (first, second, third) = (&made[0].0, &made[1].0, &made[2].0);

    // The whole input, from the source the agent's own event gave: held.
    let own = quoted(first);
    assert!(
        first
            .boundary
            .system_addition(&[part(&typed, &own)])
            .await
            .is_ok()
    );
    assert!(first.session.check().is_ok());
    // The same source for other text: refused, and the conversation ends.
    assert!(
        first
            .boundary
            .system_addition(&[part("Who handles them?", &own)])
            .await
            .is_err()
    );
    assert!(first.session.check().is_err());

    // The source another conversation's agent was given: refused.
    let foreign = quoted(third);
    assert!(
        second
            .boundary
            .system_addition(&[part(&typed, &foreign)])
            .await
            .is_err()
    );
    assert!(second.session.check().is_err());

    // What stands beside the recorded input is the draft's own source. It
    // is not the source of a quotation, and neither is an adapter's source.
    let recorded = made[2].1.recorded_sources().unwrap()[0].clone().unwrap();
    assert!(
        third
            .boundary
            .system_addition(&[part(&typed, &own_source("not an input"))])
            .await
            .is_err()
    );
    let fourth = unbuilt(&vault, &server).await;
    let draft = fourth.typed(&typed).await;
    assert!(
        fourth
            .boundary
            .system_addition(&[part(&typed, &draft)])
            .await
            .is_err()
    );
    assert!(fourth.session.check().is_err());
    let _ = recorded;
}
