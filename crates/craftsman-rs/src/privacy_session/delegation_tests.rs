//! Delegated agents of one privacy conversation, through the framework's
//! `task` and `send_message` tools, this session's boundaries and the
//! private Chat provider, against loopback servers. The vault and the guard
//! are real; the detector is fixed host reply rules.

use super::*;
use craftsman_harness::builtin::SubAgentMiddleware;
use craftsman_harness::{AgentRegistry, AgentStatus, SubAgentSpec};
use std::sync::atomic::AtomicBool;
use wiremock::{Request, Respond};

struct Delegating {
    session: Arc<PrivacySession>,
    boundary: Arc<ConversationBoundary>,
    agent: Arc<Agent>,
    gate: Arc<Gate>,
    registry: Arc<AgentRegistry>,
    usage: Arc<Mutex<(u64, u64)>>,
    delegated_usage: Arc<Mutex<(u64, u64)>>,
}

/// One conversation whose agent can delegate to `helper`. The root agent's
/// client speaks to `root` and the delegated agents' client to `side`.
async fn delegating(vault: &Vault, root: &MockServer, side: &MockServer) -> Delegating {
    let source = LiteralSource::new(&[(EMAIL, "email"), (PHONE, "phone"), (OPERATOR, "email")]);
    let session = Arc::new(
        PrivacySession::open(
            vault,
            FreshConversationBinding::reserve(&std::env::temp_dir()).unwrap(),
            Detector::from_test_source(source),
        )
        .await
        .unwrap(),
    );
    let system = session
        .admit_raw(&CapturedRaw::from_producer(
            RawOrigin::HostStatic,
            "You are a careful assistant.",
        ))
        .await
        .unwrap();
    let boundary = session.boundary(&[system]).unwrap();
    let usage = Arc::new(Mutex::new((0, 0)));
    let delegated_usage = Arc::new(Mutex::new((0, 0)));
    let provider = ViewChatProvider::new(
        client(root),
        session.clone(),
        usage.clone(),
        Arc::new(|_: &str| {}),
    )
    .await
    .unwrap();
    let delegated = delegated_provider(
        client(side),
        session.clone(),
        delegated_usage.clone(),
        Arc::new(|_: &str| {}),
    )
    .await
    .unwrap();
    let helper = SubAgentSpec {
        name: "helper".into(),
        description: format!("Helps with contacts. Problems go to {OPERATOR}."),
        system_prompt: format!("You help with contacts. The operator is {OPERATOR}."),
        tools: vec![Arc::new(Lookup(format!("their number is {PHONE}")))],
        model: Some(delegated),
        harness_profile: None,
        interrupt_on: vec![],
        max_depth: 1,
    };
    let registry = Arc::new(AgentRegistry::default());
    let gate = Arc::new(Gate::default());
    let agent = Arc::new(
        Agent::builder(Arc::new(provider))
            .name("main")
            .registry(registry.clone())
            .model_view_boundary(Some(boundary.clone()))
            .middleware(Arc::new(
                SubAgentMiddleware::new(vec![helper]).for_agent("main"),
            ))
            .middleware(gate.clone())
            .build(),
    );
    Delegating {
        session,
        boundary,
        agent,
        gate,
        registry,
        usage,
        delegated_usage,
    }
}

fn chat_call(id: &str, name: &str, arguments: Value) -> ResponseTemplate {
    sse(
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": id, "type": "function",
        "function": {"name": name, "arguments": arguments.to_string()}}]}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 1}}),
    )
}

/// A reply that is given only once a condition of the test holds. `asked`
/// is set when the request arrives.
struct Held {
    asked: Arc<AtomicBool>,
    open: Box<dyn Fn() -> bool + Send + Sync>,
    reply: ResponseTemplate,
}

impl Respond for Held {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.asked.store(true, Ordering::SeqCst);
        for _ in 0..5_000 {
            if (self.open)() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.reply.clone()
    }
}

/// Mounts the reply to the request at `index`, counted from the first.
async fn mount(server: &MockServer, index: u8, reply: impl Respond + 'static) {
    Mock::given(method("POST"))
        .respond_with(reply)
        .up_to_n_times(1)
        .with_priority(index + 1)
        .mount(server)
        .await;
}

/// Role and text of every message of one Chat request.
fn sent(body: &Value) -> Vec<(String, String)> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            (
                message["role"].as_str().unwrap_or_default().to_owned(),
                message["content"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// No request either server received holds a value the detector knows.
async fn nothing_original_left(servers: &[&MockServer]) {
    for server in servers {
        for request in server.received_requests().await.unwrap() {
            let raw = String::from_utf8_lossy(&request.body);
            for original in [EMAIL, PHONE, OPERATOR] {
                assert!(!raw.contains(original), "an original value was sent");
            }
        }
    }
}

#[tokio::test]
async fn a_delegated_agent_works_on_views_and_its_reply_returns_as_model_text() {
    let dir = tempfile::tempdir().unwrap();
    let vault = vault(&dir).await;
    let server = MockServer::start().await;
    let d = delegating(&vault, &server, &server).await;

    let text = format!("who handles {EMAIL}?");
    let node = d.session.admit_raw(&draft(&text)).await.unwrap();
    let placeholder = issued(&node);
    let report = format!("{placeholder} was looked up");
    scripted(
        &server,
        vec![
            chat_call(
                "c1",
                "task",
                json!({"description": format!("look up {placeholder}"), "subagent_type": "helper"}),
            ),
            chat_call("k1", "lookup", json!({"who": placeholder})),
            answer("chat", &[&report]),
            answer("chat", &["all done"]),
        ],
    )
    .await;

    let TurnResult::Done(output) = d
        .agent
        .run_turn_with_view(text.clone(), d.session.source(&node).unwrap())
        .await
        .unwrap()
    else {
        panic!("turn did not finish");
    };
    assert_eq!(output.final_text(), "all done");

    // The record is the real one: the delegated agent's final text is the
    // task's result.
    assert_eq!(
        recorded(&d.agent)[2],
        Msg::Tool {
            tool_call_id: "c1".into(),
            content: report.clone(),
            is_error: false,
        }
    );
    assert_eq!(d.registry.status("helper#0"), Some(AgentStatus::Done));

    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 4);
    // The root agent was told about the delegated agent with a view of its
    // description.
    let system = &sent(&bodies[0])[0].1;
    assert!(system.contains("Launch an ephemeral subagent"));
    assert!(system.contains("- helper: Helps with contacts. Problems go to "));
    // The delegated agent's own system text is a view of its prompt, and its
    // input is the description exactly as the model wrote it.
    let first = sent(&bodies[1]);
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].0, "system");
    assert!(
        first[0]
            .1
            .starts_with("You help with contacts. The operator is ")
    );
    assert_eq!(
        first[1],
        ("user".to_owned(), format!("look up {placeholder}"))
    );
    assert_eq!(bodies[1]["tools"].as_array().unwrap().len(), 1);
    // Its tool's result reached it as a view.
    let second = sent(&bodies[2]);
    assert_eq!(second[3].0, "tool");
    assert!(second[3].1.starts_with("their number is "));
    // Its final text went back to the root agent as the model wrote it. It
    // names a placeholder this conversation issued, which raw text never
    // may: it was sent as model text, not admitted as a new capture.
    let last = sent(&bodies[3]);
    assert_eq!(last[3], ("tool".to_owned(), report));
    nothing_original_left(&[&server]).await;

    // Each lane's requests were counted where they were sent.
    assert_eq!(*d.usage.lock().unwrap(), (8, 3));
    assert_eq!(*d.delegated_usage.lock().unwrap(), (8, 3));
    assert_eq!(d.session.check(), Ok(()));
}

#[tokio::test]
async fn a_background_agent_reports_back_inside_host_framing_as_model_text() {
    let dir = tempfile::tempdir().unwrap();
    let vault = vault(&dir).await;
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegating(&vault, &root, &side).await;

    let text = format!("who handles {EMAIL}?");
    let node = d.session.admit_raw(&draft(&text)).await.unwrap();
    let placeholder = issued(&node);
    let report = format!("report on {placeholder}");
    scripted(&side, vec![answer("chat", &[&report])]).await;
    mount(
        &root,
        0,
        chat_call(
            "c1",
            "task",
            json!({
                "description": format!("check {placeholder}"),
                "subagent_type": "helper",
                "run_in_background": true
            }),
        ),
    )
    .await;
    // The reply that would end the turn is held until the task has ended.
    let registry = d.registry.clone();
    mount(
        &root,
        1,
        Held {
            asked: Arc::default(),
            open: Box::new(move || registry.status("helper#0") == Some(AgentStatus::Done)),
            reply: answer("chat", &["waiting"]),
        },
    )
    .await;
    mount(&root, 2, answer("chat", &["final"])).await;

    d.agent
        .run_turn_with_view(text.clone(), d.session.source(&node).unwrap())
        .await
        .unwrap();

    // The notification is recorded as an ordinary agent records it.
    let framed = format!("[background agent 'helper#0' completed]\n{report}");
    assert_eq!(recorded(&d.agent)[4], Msg::user(framed.clone()));
    // On the wire it is the agent's own framing around the delegated
    // agent's reply as the model wrote it.
    let bodies = bodies(&root).await;
    assert_eq!(bodies.len(), 3);
    assert_eq!(sent(&bodies[2])[5], ("user".to_owned(), framed));
    assert_eq!(
        sent(&super::bodies(&side).await[0])[1],
        ("user".to_owned(), format!("check {placeholder}"))
    );
    nothing_original_left(&[&root, &side]).await;
    assert_eq!(d.session.check(), Ok(()));
}

#[tokio::test]
async fn a_message_reaches_a_running_delegated_agent_as_the_model_wrote_it() {
    let dir = tempfile::tempdir().unwrap();
    let vault = vault(&dir).await;
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegating(&vault, &root, &side).await;

    let text = format!("who handles {EMAIL}?");
    let node = d.session.admit_raw(&draft(&text)).await.unwrap();
    let placeholder = issued(&node);
    let message = format!("also {placeholder}");
    mount(
        &root,
        0,
        chat_call(
            "c1",
            "task",
            json!({"description": "start", "subagent_type": "helper", "run_in_background": true}),
        ),
    )
    .await;
    mount(
        &root,
        1,
        chat_call(
            "c2",
            "send_message",
            json!({"to": "helper#0", "message": message}),
        ),
    )
    .await;
    // The root agent's third reply is held until the task has ended, and
    // the delegated agent's first until the message has been sent.
    let third = Arc::new(AtomicBool::new(false));
    let registry = d.registry.clone();
    mount(
        &root,
        2,
        Held {
            asked: third.clone(),
            open: Box::new(move || registry.status("helper#0") == Some(AgentStatus::Done)),
            reply: answer("chat", &["sent"]),
        },
    )
    .await;
    mount(&root, 3, answer("chat", &["final"])).await;
    mount(
        &side,
        0,
        Held {
            asked: Arc::default(),
            open: Box::new(move || third.load(Ordering::SeqCst)),
            reply: chat_call("k1", "lookup", json!({"who": "them"})),
        },
    )
    .await;
    mount(&side, 1, answer("chat", &["got it"])).await;

    d.agent
        .run_turn_with_view(text.clone(), d.session.source(&node).unwrap())
        .await
        .unwrap();

    assert_eq!(
        recorded(&d.agent)[4],
        Msg::Tool {
            tool_call_id: "c2".into(),
            content: "message sent to helper#0 (delivered to inbox)".into(),
            is_error: false,
        }
    );
    // The delegated agent read the message from its inbox, exactly as the
    // model wrote it.
    let delivered = super::bodies(&side).await;
    assert_eq!(delivered.len(), 2);
    assert!(
        sent(&delivered[1]).contains(&("user".to_owned(), message)),
        "the message is part of the delegated agent's conversation"
    );
    nothing_original_left(&[&root, &side]).await;
    assert_eq!(d.session.check(), Ok(()));
}

#[tokio::test]
async fn a_description_a_person_changed_reaches_the_delegated_agent_as_a_view() {
    let dir = tempfile::tempdir().unwrap();
    let vault = vault(&dir).await;
    let server = MockServer::start().await;
    let d = delegating(&vault, &server, &server).await;

    let text = format!("who handles {EMAIL}?");
    let node = d.session.admit_raw(&draft(&text)).await.unwrap();
    let placeholder = issued(&node);
    scripted(
        &server,
        vec![
            chat_call(
                "c1",
                "task",
                json!({"description": format!("look up {placeholder}"), "subagent_type": "helper"}),
            ),
            answer("chat", &["written"]),
            answer("chat", &["all done"]),
        ],
    )
    .await;
    d.gate.park.store(true, Ordering::SeqCst);
    let TurnResult::Suspended(suspension) = d
        .agent
        .run_turn_with_view(text.clone(), d.session.source(&node).unwrap())
        .await
        .unwrap()
    else {
        panic!("the call was not parked");
    };
    d.gate.park.store(false, Ordering::SeqCst);

    // The person replaces the description with text of their own.
    let call = suspension.request.tool_call.clone();
    let raw = format!(r#"{{"description":"write to {PHONE}","subagent_type":"helper"}}"#);
    let producer = crate::privacy_entry::HumanDecisionProducer::register(
        d.session.clone(),
        Arc::new(move |_| crate::privacy_entry::HumanChoice::Edit(raw.clone())),
    );
    let parked = d.agent.actual_parked_call().unwrap();
    assert_eq!(parked.call(), &call);
    let input = producer.receive(&parked).unwrap();
    let edit = input.action().unwrap().clone();
    let decision = SourceRef::new(input);
    d.agent
        .resume_with_view(edit, "Denied.", decision)
        .await
        .unwrap();

    // That text is raw text from the person. The delegated agent got its
    // view, and the root agent got the delegated agent's reply.
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 3);
    let input = &sent(&bodies[1])[1];
    assert_eq!(input.0, "user");
    assert!(input.1.starts_with("write to ") && !input.1.contains(PHONE));
    assert_eq!(sent(&bodies[2])[3], ("tool".to_owned(), "written".into()));
    nothing_original_left(&[&server]).await;
    assert_eq!(d.session.check(), Ok(()));
}

#[tokio::test]
async fn a_reply_belongs_to_the_lane_of_the_agent_that_received_it() {
    let dir = tempfile::tempdir().unwrap();
    let vault = vault(&dir).await;
    let server = MockServer::start().await;
    let d = delegating(&vault, &server, &server).await;

    // No message/label factory can issue a child or a retained reply.
    assert!(server.received_requests().await.unwrap().is_empty());
    let text = format!("who handles {EMAIL}?");
    let node = d.session.admit_raw(&draft(&text)).await.unwrap();
    scripted(
        &server,
        vec![
            chat_call(
                "c1",
                "task",
                json!({"description":"do the work", "subagent_type":"helper"}),
            ),
            answer("chat", &["a reply"]),
            answer("chat", &["all done"]),
        ],
    )
    .await;
    d.agent
        .run_turn_with_view(text, d.session.source(&node).unwrap())
        .await
        .unwrap();
    // The actual child task return keeps its real immutable model commit.
    let tasks = d.agent.actual_task_returns().unwrap();
    let task = tasks.last().unwrap();
    let commit = task
        .retained_commits()
        .iter()
        .find(|commit| commit.event() == craftsman_harness::provenance::RetainedEvent::ModelReply)
        .unwrap();
    assert_eq!(task.final_text(), Some("a reply"));
    assert!(task.parent_invocation().is_some());
    assert!(
        !commit
            .owner()
            .same_agent(d.agent.actual_retained_commits().unwrap()[0].owner())
    );
    // A genuine child receipt/commit still cannot be adopted by the root.
    assert!(d.boundary.commit_retained_model(commit).await.is_err());
    assert_eq!(d.session.check(), Err(SessionStop::Closed));
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}
