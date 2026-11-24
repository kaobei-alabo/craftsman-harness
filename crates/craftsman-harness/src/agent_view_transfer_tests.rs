//! The owned retirement and adoption of a selected agent's conversation.
//! Real agents, a real tool round and real saves; the host is a test double
//! that only labels its sources.

use super::*;
use crate::provenance::{
    ActualConversationTransfer, ActualModelInput, ActualRetainedCommit, BoxedBoundaryFuture,
    ConversationAdopter, ModelDelivery, ModelViewBoundary, ObservedToolResult, PartFrom,
    RetainedEvent, SourceRef, TrackedModelRequest,
};
use crate::testing::{MemCheckpointer, NoopTool};
use crate::{FileData, ModelRequest, ModelResponse, Todo, TodoStatus, ToolDef, Usage};
use std::sync::atomic::AtomicBool;

/// One source of the test host: the conversation it was made for, what it
/// stands beside, and the retired source it was adopted from.
struct Held {
    conversation: usize,
    what: String,
    original: Option<SourceRef>,
}

struct Host {
    conversation: usize,
    closed: AtomicBool,
    /// The reply each call-origin part of a system section named, per section.
    sections: Mutex<Vec<Vec<SourceRef>>>,
}

impl Host {
    fn new(conversation: usize) -> Arc<Self> {
        Arc::new(Self {
            conversation,
            closed: AtomicBool::new(false),
            sections: Mutex::new(Vec::new()),
        })
    }
    fn held(&self, what: impl Into<String>) -> SourceRef {
        SourceRef::new(Arc::new(Held {
            conversation: self.conversation,
            what: what.into(),
            original: None,
        }))
    }
}

#[async_trait::async_trait]
impl ModelViewBoundary for Host {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    async fn system_sources(&self) -> Result<Vec<SourceRef>, AgentError> {
        Ok(Vec::new())
    }
    async fn tool_source(&self, definition: &ToolDef) -> Result<SourceRef, AgentError> {
        Ok(self.held(format!("tool {}", definition.name)))
    }
    async fn commit_model(&self, _: SourceRef, _: &Msg) -> Result<SourceRef, AgentError> {
        Ok(self.held("reply"))
    }
    async fn system_addition(&self, parts: &[crate::SourcePart]) -> Result<SourceRef, AgentError> {
        let replies = parts
            .iter()
            .filter_map(|part| match &part.from {
                PartFrom::Call { reply, .. } => Some(reply.clone()),
                _ => None,
            })
            .collect();
        self.sections.lock().unwrap().push(replies);
        Ok(self.held("section"))
    }
    async fn observe_tool_result(
        &self,
        result: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError> {
        Ok(self.held(format!("result {}", result.call_id)))
    }
}

/// What one request carried: the recorded messages, the source beside each,
/// the event of each, and for an adopted one the event of its original.
struct Seen {
    messages: Vec<Msg>,
    sources: Vec<SourceRef>,
    events: Vec<RetainedEvent>,
    originals: Vec<Option<RetainedEvent>>,
}

struct Wire {
    script: Mutex<std::vec::IntoIter<Msg>>,
    seen: Mutex<Vec<Seen>>,
}

impl Wire {
    fn new(script: Vec<Msg>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into_iter()),
            seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl ModelProvider for Wire {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        Err(AgentError::Model("the recorded text was asked for".into()))
    }
    fn complete_with_view<'a, 'f>(
        &'a self,
        request: TrackedModelRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ModelDelivery, AgentError>> + Send + 'f>,
    >
    where
        'a: 'f,
        Self: 'f,
    {
        Box::pin(async move {
            let commits: Vec<&ActualRetainedCommit> = request
                .message_inputs()
                .iter()
                .map(|input| match input {
                    ActualModelInput::Retained(commit) => commit,
                    _ => panic!("every message of these requests is a retained one"),
                })
                .collect();
            self.seen.lock().unwrap().push(Seen {
                messages: request.recorded().messages.clone(),
                sources: request.message_sources().to_vec(),
                events: commits.iter().map(|commit| commit.event()).collect(),
                originals: commits
                    .iter()
                    .map(|commit| {
                        commit
                            .adopted()
                            .map(|adopted| adopted.original().commit().event())
                    })
                    .collect(),
            });
            match self.script.lock().unwrap().next() {
                Some(message) => Ok(ModelDelivery::new(
                    ModelResponse {
                        message,
                        usage: Usage::default(),
                    },
                    SourceRef::new(Arc::new(())),
                )),
                None => Err(AgentError::Model("the scripted wire is exhausted".into())),
            }
        })
    }
}

/// The adopting host: it accepts only sources of the conversation it knows
/// as retired, and labels a new source for each.
struct Adopter {
    host: Arc<Host>,
    retired: usize,
    refuse: bool,
    leave_one_out: bool,
}

impl ConversationAdopter for Adopter {
    fn adopted_sources<'a, 'b, 'f>(
        &'a self,
        adopted: &'b [ActualRetainedCommit],
    ) -> BoxedBoundaryFuture<'f, Vec<SourceRef>>
    where
        'a: 'f,
        'b: 'f,
        Self: 'f,
    {
        Box::pin(async move {
            if self.refuse {
                return Err(AgentError::State("the test host refused".into()));
            }
            let mut sources = Vec::new();
            for commit in adopted {
                assert_eq!(commit.event(), RetainedEvent::Adopted);
                let adoption = commit.adopted().expect("an adopted commit");
                let original = adoption.original();
                assert_eq!(commit.message(), original.commit().message());
                let held = original
                    .source()
                    .downcast::<Held>()
                    .expect("a source of the retired host");
                assert_eq!(held.conversation, self.retired);
                sources.push(SourceRef::new(Arc::new(Held {
                    conversation: self.host.conversation,
                    what: format!("adopted {}", held.what),
                    original: Some(original.source().clone()),
                })));
            }
            if self.leave_one_out {
                sources.pop();
            }
            Ok(sources)
        })
    }
}

fn agent(
    wire: &Arc<Wire>,
    host: Option<&Arc<Host>>,
    saves: &Arc<MemCheckpointer>,
    thread: &str,
) -> Agent {
    Agent::builder(wire.clone())
        .tool(Arc::new(NoopTool::new("noop")))
        .model_view_boundary(host.map(|host| host.clone() as Arc<dyn ModelViewBoundary>))
        .checkpointer(saves.clone())
        .thread_id(thread)
        .build()
}

fn call_then_answer() -> Vec<Msg> {
    vec![
        Msg::Assistant {
            content: "calling".into(),
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                name: "noop".into(),
                args: serde_json::json!({"value": "x"}),
            }],
        },
        Msg::assistant("first answer"),
    ]
}

/// A retired conversation of four messages on the thread `thread`.
async fn retired(
    saves: &Arc<MemCheckpointer>,
    thread: &str,
) -> (Agent, Arc<Host>, ActualConversationTransfer) {
    let host = Host::new(1);
    let old = agent(&Wire::new(call_then_answer()), Some(&host), saves, thread);
    old.run_turn_with_view("first input", host.held("input"))
        .await
        .expect("the first turn");
    let transfer = old
        .retire_conversation_with_view()
        .await
        .expect("an idle selected conversation retires");
    (old, host, transfer)
}

fn sequences(saves: &MemCheckpointer, thread: &str) -> Vec<u64> {
    saves
        .saves
        .lock()
        .unwrap()
        .iter()
        .filter(|(saved, ..)| saved == thread)
        .map(|(_, seq, ..)| *seq)
        .collect()
}

#[tokio::test]
async fn a_retired_conversation_is_adopted_with_its_roles_results_state_and_save_sequence() {
    let saves = Arc::new(MemCheckpointer::default());
    let host = Host::new(1);
    let old = agent(
        &Wire::new(call_then_answer()),
        Some(&host),
        &saves,
        "thread",
    );
    old.run_turn_with_view("first input", host.held("input"))
        .await
        .expect("the first turn");
    {
        let mut state = old.state.lock().unwrap();
        state.custom.insert("kept".into(), serde_json::json!(7));
        state.files.insert("note.txt".into(), FileData::text("x"));
    }
    let before = sequences(&saves, "thread");
    assert!(!before.is_empty());

    let transfer = old
        .retire_conversation_with_view()
        .await
        .expect("an idle selected conversation retires");
    // The retirement saved nothing and closed the old view for good.
    assert_eq!(sequences(&saves, "thread"), before);
    assert!(host.closed.load(Ordering::SeqCst));
    assert!(
        old.run_turn_with_view("late", host.held("input"))
            .await
            .is_err()
    );
    assert!(old.retire_conversation_with_view().await.is_err());
    let originals: Vec<RetainedEvent> = transfer
        .messages()
        .iter()
        .map(|message| message.commit().event())
        .collect();
    assert_eq!(
        originals,
        [
            RetainedEvent::Input,
            RetainedEvent::ModelReply,
            RetainedEvent::ToolResult,
            RetainedEvent::ModelReply
        ]
    );
    assert!(
        transfer
            .owner()
            .same_agent(transfer.messages()[0].commit().owner())
    );

    let fresh_host = Host::new(2);
    let wire = Wire::new(vec![Msg::assistant("second answer")]);
    let fresh = agent(&wire, Some(&fresh_host), &saves, "thread");
    let adopter = Adopter {
        host: fresh_host.clone(),
        retired: 1,
        refuse: false,
        leave_one_out: false,
    };
    fresh
        .adopt_conversation_with_view(&transfer, &adopter)
        .await
        .expect("a fresh selected agent adopts");
    assert!(transfer.is_adopted());
    // Adoption saved nothing either.
    assert_eq!(sequences(&saves, "thread"), before);
    {
        let state = fresh.state.lock().unwrap();
        assert_eq!(state.messages, transfer.state().messages);
        assert_eq!(state.custom.get("kept"), Some(&serde_json::json!(7)));
        assert_eq!(state.files.get("note.txt"), Some(&FileData::text("x")));
        assert!(matches!(&state.messages[1],
            Msg::Assistant { tool_calls, .. } if tool_calls[0].id == "call-1"));
        assert!(matches!(&state.messages[2],
            Msg::Tool { tool_call_id, is_error: false, .. } if tool_call_id == "call-1"));
    }

    fresh
        .run_turn_with_view("second input", fresh_host.held("input"))
        .await
        .expect("the adopted conversation takes its next turn");
    let seen = wire.seen.lock().unwrap();
    let request = &seen[0];
    assert_eq!(request.messages.len(), 5);
    assert_eq!(request.messages[..4], transfer.state().messages[..]);
    assert_eq!(request.events[..4], [RetainedEvent::Adopted; 4]);
    assert_eq!(request.events[4], RetainedEvent::Input);
    assert_eq!(
        request.originals,
        [
            Some(RetainedEvent::Input),
            Some(RetainedEvent::ModelReply),
            Some(RetainedEvent::ToolResult),
            Some(RetainedEvent::ModelReply),
            None
        ]
    );
    for (index, source) in request.sources[..4].iter().enumerate() {
        let held = source
            .downcast::<Held>()
            .expect("the adopting host's source");
        assert_eq!(held.conversation, 2);
        assert!(held.what.starts_with("adopted "));
        let original = held.original.as_ref().expect("the retired source");
        assert!(original.is(transfer.messages()[index].source()));
    }
    // One thread, one unbroken save sequence across both agents.
    let after = sequences(&saves, "thread");
    assert!(after.len() > before.len());
    assert_eq!(after, (0..after.len() as u64).collect::<Vec<_>>());
    let last = saves.saves.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.2.messages.len(), 6);
    assert_eq!(fresh.actual_sourced_messages().unwrap().len(), 6);
}

#[tokio::test]
async fn adoption_takes_a_fresh_agent_and_a_whole_answer_and_happens_once() {
    let saves = Arc::new(MemCheckpointer::default());
    let (_old, _old_host, transfer) = retired(&saves, "thread").await;
    let host = Host::new(2);
    let adopter = |refuse, leave_one_out| Adopter {
        host: host.clone(),
        retired: 1,
        refuse,
        leave_one_out,
    };

    // A host that refuses, or answers for fewer messages, adopts nothing.
    let fresh = agent(&Wire::new(Vec::new()), Some(&host), &saves, "thread");
    for failing in [adopter(true, false), adopter(false, true)] {
        assert!(
            fresh
                .adopt_conversation_with_view(&transfer, &failing)
                .await
                .is_err()
        );
        assert!(!transfer.is_adopted());
        assert!(fresh.state.lock().unwrap().messages.is_empty());
    }

    // An agent that already recorded a turn is not fresh.
    let used_host = Host::new(3);
    let used = agent(
        &Wire::new(vec![Msg::assistant("own")]),
        Some(&used_host),
        &Arc::new(MemCheckpointer::default()),
        "other",
    );
    used.run_turn_with_view("own input", used_host.held("input"))
        .await
        .expect("its own turn");
    let for_used = Adopter {
        host: used_host.clone(),
        retired: 1,
        refuse: false,
        leave_one_out: false,
    };
    assert!(
        used.adopt_conversation_with_view(&transfer, &for_used)
            .await
            .is_err()
    );
    assert_eq!(used.state.lock().unwrap().messages.len(), 2);

    // An agent without a source view has no adoption at all.
    let plain = agent(&Wire::new(Vec::new()), None, &saves, "thread");
    assert!(
        plain
            .adopt_conversation_with_view(&transfer, &adopter(false, false))
            .await
            .is_err()
    );
    assert!(!transfer.is_adopted());

    // The refusals left the transfer whole: the fresh agent adopts it now,
    // and nobody adopts it a second time.
    fresh
        .adopt_conversation_with_view(&transfer, &adopter(false, false))
        .await
        .expect("the fresh agent adopts after the refusals");
    assert_eq!(fresh.state.lock().unwrap().messages.len(), 4);
    let second_host = Host::new(4);
    let second = agent(&Wire::new(Vec::new()), Some(&second_host), &saves, "thread");
    let again = Adopter {
        host: second_host.clone(),
        retired: 1,
        refuse: false,
        leave_one_out: false,
    };
    assert!(
        second
            .adopt_conversation_with_view(&transfer, &again)
            .await
            .is_err()
    );
    assert!(second.state.lock().unwrap().messages.is_empty());
}

#[tokio::test]
async fn under_another_thread_the_adopting_agent_keeps_its_own_save_sequence() {
    let saves = Arc::new(MemCheckpointer::default());
    let (_old, _old_host, transfer) = retired(&saves, "thread").await;
    let old_saves = sequences(&saves, "thread");
    let host = Host::new(2);
    let wire = Wire::new(vec![Msg::assistant("second answer")]);
    let fresh = agent(&wire, Some(&host), &saves, "successor");
    let adopter = Adopter {
        host: host.clone(),
        retired: 1,
        refuse: false,
        leave_one_out: false,
    };
    fresh
        .adopt_conversation_with_view(&transfer, &adopter)
        .await
        .expect("adoption under another thread");
    fresh
        .run_turn_with_view("second input", host.held("input"))
        .await
        .expect("the next turn");
    assert_eq!(sequences(&saves, "thread"), old_saves);
    let own = sequences(&saves, "successor");
    assert_eq!(own, (0..own.len() as u64).collect::<Vec<_>>());
    // The first save under the new thread already holds the whole record.
    let first = saves
        .saves
        .lock()
        .unwrap()
        .iter()
        .find(|(thread, ..)| thread == "successor")
        .cloned()
        .unwrap();
    assert!(first.2.messages.len() >= 5);
}

#[tokio::test]
async fn the_ordinary_entries_still_refuse_and_a_refused_retirement_keeps_the_conversation() {
    let saves = Arc::new(MemCheckpointer::default());
    let host = Host::new(1);
    let old = agent(
        &Wire::new(vec![Msg::assistant("one"), Msg::assistant("two")]),
        Some(&host),
        &saves,
        "thread",
    );
    old.run_turn_with_view("first input", host.held("input"))
        .await
        .expect("the first turn");

    // The ordinary transfer and reopen are refused for a selected agent, in
    // both directions, exactly as before.
    let plain = agent(&Wire::new(Vec::new()), None, &saves, "thread");
    assert!(plain.inherit_conversation(&old).is_err());
    assert!(old.inherit_conversation(&plain).is_err());
    assert!(old.load_thread("thread").await.is_err());
    // An agent without a source view has no retirement.
    assert!(plain.retire_conversation_with_view().await.is_err());

    // A todo list with no recorded origin has no successor: refused, and
    // the conversation goes on in its own view.
    old.state.lock().unwrap().todos.push(Todo {
        content: "kept".into(),
        status: TodoStatus::Pending,
    });
    let refusal = old
        .retire_conversation_with_view()
        .await
        .expect_err("a todo list without an origin is not carried");
    assert!(refusal.to_string().contains("todo list"));
    assert!(!host.closed.load(Ordering::SeqCst));
    old.run_turn_with_view("second input", host.held("input"))
        .await
        .expect("the conversation was kept");
    old.state.lock().unwrap().todos.clear();
    let transfer = old
        .retire_conversation_with_view()
        .await
        .expect("without the todo list it retires");
    assert_eq!(transfer.messages().len(), 4);
    assert_eq!(transfer.thread_id(), "thread");
}

#[tokio::test]
async fn a_todo_list_follows_its_reply_into_the_adopted_conversation() {
    let saves = Arc::new(MemCheckpointer::default());
    let with_todos = |wire: &Arc<Wire>, host: &Arc<Host>| {
        Agent::builder(wire.clone())
            .middleware(Arc::new(crate::builtin::TodoListMiddleware::new()))
            .model_view_boundary(Some(host.clone() as Arc<dyn ModelViewBoundary>))
            .checkpointer(saves.clone())
            .thread_id("thread")
            .build()
    };
    let host = Host::new(1);
    let old = with_todos(
        &Wire::new(vec![
            Msg::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "write_todos".into(),
                    args: serde_json::json!({"todos": [{"content": "one step", "status": "pending"}]}),
                }],
            },
            Msg::assistant("planned"),
        ]),
        &host,
    );
    old.run_turn_with_view("plan it", host.held("input"))
        .await
        .expect("the planning turn");
    assert_eq!(old.state.lock().unwrap().todos.len(), 1);
    // In the old conversation the stored todo names the old reply.
    let named = host.sections.lock().unwrap().last().cloned().unwrap();
    assert_eq!(named.len(), 1);
    let transfer = old
        .retire_conversation_with_view()
        .await
        .expect("a todo list whose reply is recorded is carried");
    assert!(named[0].is(transfer.messages()[1].source()));

    let fresh_host = Host::new(2);
    let wire = Wire::new(vec![Msg::assistant("continuing")]);
    let fresh = with_todos(&wire, &fresh_host);
    let adopter = Adopter {
        host: fresh_host.clone(),
        retired: 1,
        refuse: false,
        leave_one_out: false,
    };
    fresh
        .adopt_conversation_with_view(&transfer, &adopter)
        .await
        .expect("adoption with a todo list");
    assert_eq!(fresh.state.lock().unwrap().todos, transfer.state().todos);
    fresh
        .run_turn_with_view("go on", fresh_host.held("input"))
        .await
        .expect("the todo section has a source in the new conversation");
    // In the new conversation the same todo names the adopted reply's new
    // source, and nothing of the retired conversation.
    let seen = wire.seen.lock().unwrap();
    let named = fresh_host.sections.lock().unwrap().last().cloned().unwrap();
    assert_eq!(named.len(), 1);
    assert!(named[0].is(&seen[0].sources[1]));
    let held = named[0].downcast::<Held>().unwrap();
    assert_eq!(
        (held.conversation, held.what.as_str()),
        (2, "adopted reply")
    );
}
