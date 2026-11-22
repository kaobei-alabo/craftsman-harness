//! Delegated agents through the optional source view boundary: the `task`
//! and `send_message` tools, background completion and the inbox, with
//! scripted providers and a recording boundary per agent. Nothing here
//! contacts a model service.

use super::*;
use crate::builtin::{HitlPolicyMiddleware, SubAgentMiddleware};
use crate::provenance::{
    BoxedBoundaryFuture, FrameworkCommit, ModelDelivery, ModelViewBoundary, ObservedToolResult,
    PartFrom, SourcePart, SourceRef, TrackedModelRequest,
};
use crate::subagent::{AgentRegistry, AgentStatus, SubAgentSpec};
use crate::testing::NoopTool;
use crate::{HitlAction, ModelResponse, ToolDef, Usage};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Tag(String);

fn source(tag: &str) -> SourceRef {
    SourceRef::new(Arc::new(Tag(tag.to_owned())))
}

fn tag(source: &SourceRef) -> String {
    source.downcast::<Tag>().expect("test payload").0.clone()
}

/// Each part as where it came from and its text.
fn describe(parts: &[SourcePart]) -> String {
    parts
        .iter()
        .map(|part| {
            let from = match &part.from {
                PartFrom::Fixed => "fixed".to_owned(),
                PartFrom::Read { what } => format!("read {what}"),
                PartFrom::Call {
                    reply,
                    call_id,
                    pointer,
                } => format!("call {} {call_id} {pointer}", tag(reply)),
                PartFrom::CallJson {
                    reply,
                    call_id,
                    pointer,
                } => format!("json {} {call_id} {pointer}", tag(reply)),
                PartFrom::Source { source } => format!("source {}", tag(source)),
            };
            format!("{from}={}", part.text)
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// What a provider was handed for one request.
struct Seen {
    recorded: ModelRequest,
    system: Vec<String>,
    messages: Vec<String>,
    conversation: Option<String>,
}

type Gate = Box<dyn Fn() -> bool + Send + Sync>;

/// A provider with a view path only. A reply can be held back until a
/// condition of the test holds.
struct Scripted {
    script: Mutex<std::vec::IntoIter<Msg>>,
    seen: Mutex<Vec<Seen>>,
    recorded_path_calls: AtomicUsize,
    gates: Mutex<Vec<(usize, Gate)>>,
}

impl Scripted {
    fn new(script: Vec<Msg>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into_iter()),
            seen: Mutex::new(Vec::new()),
            recorded_path_calls: AtomicUsize::new(0),
            gates: Mutex::new(Vec::new()),
        })
    }

    /// Holds the reply to request `index` until `open` returns true.
    fn hold(&self, index: usize, open: impl Fn() -> bool + Send + Sync + 'static) {
        self.gates.lock().unwrap().push((index, Box::new(open)));
    }

    fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl ModelProvider for Scripted {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, AgentError> {
        self.recorded_path_calls.fetch_add(1, Ordering::SeqCst);
        Err(AgentError::Model("the recorded-text path was used".into()))
    }

    async fn complete_with_view(
        &self,
        req: TrackedModelRequest,
    ) -> Result<ModelDelivery, AgentError> {
        let index = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(Seen {
                recorded: req.recorded().clone(),
                system: req.system_sources().iter().map(tag).collect(),
                messages: req.message_sources().iter().map(tag).collect(),
                conversation: req.conversation().map(tag),
            });
            seen.len() - 1
        };
        for _ in 0..5_000 {
            let open = self
                .gates
                .lock()
                .unwrap()
                .iter()
                .filter(|(held, _)| *held == index)
                .all(|(_, open)| open());
            if open {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        match self.script.lock().unwrap().next() {
            Some(message) => Ok(ModelDelivery::new(
                ModelResponse {
                    message,
                    usage: Usage {
                        input_tokens: 5,
                        output_tokens: 2,
                    },
                },
                source(&format!("receipt:{index}")),
            )),
            None => Err(AgentError::Model("scripted model exhausted".into())),
        }
    }
}

/// The boundary of one agent of a conversation: the root's, or one that a
/// boundary of the same conversation made for a delegated agent. All of them
/// write to one list of events.
struct Family {
    label: String,
    system: Vec<SourceRef>,
    events: Arc<Mutex<Vec<String>>>,
    children: Arc<AtomicUsize>,
    envelopes: Arc<AtomicUsize>,
    without_children: Arc<AtomicBool>,
}

impl Family {
    fn root() -> Arc<Self> {
        Arc::new(Self {
            label: "root".into(),
            system: vec![source("root:system")],
            events: Arc::default(),
            children: Arc::default(),
            envelopes: Arc::default(),
            without_children: Arc::default(),
        })
    }

    fn note(&self, event: String) {
        self.events.lock().unwrap().push(event);
    }

    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ModelViewBoundary for Family {
    async fn system_sources(&self) -> Result<Vec<SourceRef>, AgentError> {
        Ok(self.system.clone())
    }

    async fn tool_source(&self, definition: &ToolDef) -> Result<SourceRef, AgentError> {
        Ok(source(&format!(
            "{}:definition:{}",
            self.label, definition.name
        )))
    }

    async fn commit_model(&self, receipt: SourceRef, _: &Msg) -> Result<SourceRef, AgentError> {
        Ok(source(&format!("{}:model:{}", self.label, tag(&receipt))))
    }

    async fn observe_tool_result(
        &self,
        result: ObservedToolResult<'_>,
    ) -> Result<SourceRef, AgentError> {
        self.note(format!(
            "{} result {} [{}]",
            self.label,
            result.call_id,
            describe(result.parts)
        ));
        Ok(source(&format!("{}:result:{}", self.label, result.call_id)))
    }

    async fn system_addition(&self, parts: &[SourcePart]) -> Result<SourceRef, AgentError> {
        let text: String = parts.iter().map(|part| &*part.text).collect();
        self.note(format!("{} addition {text}", self.label));
        Ok(source(&format!("{}:addition", self.label)))
    }

    async fn host_message(&self, part: &SourcePart) -> Result<SourceRef, AgentError> {
        Ok(source(&format!("{}:host:{}", self.label, part.text)))
    }

    async fn accept_decision(
        &self,
        _: &ToolCall,
        _: &HitlAction,
        _: &SourceRef,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    fn conversation(&self) -> Option<SourceRef> {
        Some(source(&self.label))
    }

    async fn observe_framework_commit(
        &self,
        event: FrameworkCommit,
        _: &Msg,
    ) -> Result<SourceRef, AgentError> {
        Ok(source(&format!("{}:framework:{event:?}", self.label)))
    }

    async fn argument_source(&self, part: &SourcePart) -> Result<SourceRef, AgentError> {
        Ok(source(&format!(
            "{}:argument:{}",
            self.label,
            describe(std::slice::from_ref(part))
        )))
    }

    fn child_boundary<'a, 'b, 'f>(
        &'a self,
        system: &'b [SourcePart],
    ) -> BoxedBoundaryFuture<'f, Arc<dyn ModelViewBoundary>>
    where
        'a: 'f,
        'b: 'f,
        Self: 'f,
    {
        if self.without_children.load(Ordering::SeqCst) {
            return Box::pin(std::future::ready(Err(AgentError::State(
                "This conversation has no source view for a delegated agent.".into(),
            ))));
        }
        let label = format!(
            "{}/child{}",
            self.label,
            self.children.fetch_add(1, Ordering::SeqCst)
        );
        let child: Arc<dyn ModelViewBoundary> = Arc::new(Family {
            system: vec![source(&format!("{label}:system:{}", describe(system)))],
            label,
            events: self.events.clone(),
            children: self.children.clone(),
            envelopes: self.envelopes.clone(),
            without_children: self.without_children.clone(),
        });
        Box::pin(std::future::ready(Ok(child)))
    }

    async fn observe_envelope(
        &self,
        event: FrameworkCommit,
        parts: &[SourcePart],
        committed: &Msg,
    ) -> Result<SourceRef, AgentError> {
        let joined: String = parts.iter().map(|part| &*part.text).collect();
        assert!(matches!(committed, Msg::User { content } if *content == joined));
        self.note(format!(
            "{} envelope {event:?} [{}]",
            self.label,
            describe(parts)
        ));
        Ok(source(&format!(
            "{}:envelope:{}",
            self.label,
            self.envelopes.fetch_add(1, Ordering::SeqCst)
        )))
    }
}

fn call(id: &str, name: &str, args: serde_json::Value) -> Msg {
    Msg::Assistant {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: name.into(),
            args,
        }],
    }
}

fn spec(name: &str, model: Arc<Scripted>) -> SubAgentSpec {
    SubAgentSpec {
        name: name.into(),
        description: format!("the {name} agent"),
        system_prompt: format!("you are {name}"),
        tools: vec![Arc::new(NoopTool::new("look"))],
        model: Some(model),
        harness_profile: None,
        interrupt_on: vec![],
        max_depth: 1,
    }
}

struct Conversation {
    agent: Agent,
    root: Arc<Family>,
    model: Arc<Scripted>,
    registry: Arc<AgentRegistry>,
}

fn conversation(script: Vec<Msg>, specs: Vec<SubAgentSpec>) -> Conversation {
    let root = Family::root();
    let model = Scripted::new(script);
    let registry = Arc::new(AgentRegistry::default());
    let agent = Agent::builder(model.clone())
        .name("main")
        .registry(registry.clone())
        .model_view_boundary(Some(root.clone()))
        .middleware(Arc::new(SubAgentMiddleware::new(specs).for_agent("main")))
        .build();
    Conversation {
        agent,
        root,
        model,
        registry,
    }
}

fn contents(agent: &Agent) -> Vec<String> {
    agent
        .state
        .lock()
        .unwrap()
        .messages
        .iter()
        .map(|message| match message {
            Msg::Assistant { content, .. }
            | Msg::User { content }
            | Msg::System { content }
            | Msg::Tool { content, .. } => content.clone(),
        })
        .collect()
}

#[tokio::test]
async fn a_foreground_task_runs_on_a_boundary_of_the_same_conversation() {
    let child = Scripted::new(vec![Msg::assistant("found [X1] twice")]);
    let c = conversation(
        vec![
            call(
                "c1",
                "task",
                serde_json::json!({"description":"find [X1]","subagent_type":"gp"}),
            ),
            Msg::assistant("done"),
        ],
        vec![spec("gp", child.clone())],
    );

    let out = c
        .agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();
    assert!(matches!(out, TurnResult::Done(out) if out.final_text() == "done"));

    // The delegated agent got the description as its input, with the source
    // the root's boundary issued for that text of the call, and its own
    // system text through its own boundary. It has no system text of its own
    // and was never asked through the recorded-text path.
    let seen = child.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].recorded.system, "");
    assert_eq!(seen[0].recorded.messages, vec![Msg::user("find [X1]")]);
    assert_eq!(
        seen[0].system,
        vec!["root/child0:system:read delegated agent prompt=you are gp"]
    );
    assert_eq!(
        seen[0].messages,
        vec!["root:argument:call root:model:receipt:0 c1 /description=find [X1]"]
    );
    assert_eq!(seen[0].conversation.as_deref(), Some("root/child0"));
    assert_eq!(child.recorded_path_calls.load(Ordering::SeqCst), 0);
    assert_eq!(c.model.recorded_path_calls.load(Ordering::SeqCst), 0);
    drop(seen);

    // What it ended with came back as the result, with the source its own
    // boundary keeps beside that reply.
    assert_eq!(
        contents(&c.agent),
        vec!["go", "", "found [X1] twice", "done"]
    );
    assert!(c.root.events().contains(
        &"root result c1 [source root/child0:model:receipt:0=found [X1] twice]".to_owned()
    ));
    let seen = c.model.seen.lock().unwrap();
    assert_eq!(
        seen[1].messages,
        vec!["input", "root:model:receipt:0", "root:result:c1"]
    );
    assert_eq!(seen[1].conversation.as_deref(), Some("root"));
    drop(seen);
    assert_eq!(c.registry.status("gp#0"), Some(AgentStatus::Done));

    // The section this middleware adds is the ordinary section's text.
    let mut ordinary = AgentState::default();
    SubAgentMiddleware::new(vec![spec("gp", Scripted::new(vec![]))])
        .prepare_model(&mut ordinary)
        .await
        .unwrap();
    assert!(
        c.root
            .events()
            .contains(&format!("root addition {}", ordinary.system_extras[0]))
    );
}

#[tokio::test]
async fn a_background_task_reports_back_inside_the_agents_own_framing() {
    let child = Scripted::new(vec![Msg::assistant("report on [X1]")]);
    let c = conversation(
        vec![
            call(
                "c1",
                "task",
                serde_json::json!({
                    "description":"look into [X1]",
                    "subagent_type":"gp",
                    "run_in_background":true
                }),
            ),
            Msg::assistant("waiting"),
            Msg::assistant("final"),
        ],
        vec![spec("gp", child.clone())],
    );
    // The reply that would end the turn is held until the task has ended.
    let registry = c.registry.clone();
    c.model.hold(1, move || {
        registry.status("gp#0") == Some(AgentStatus::Done)
    });

    c.agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();

    // The recorded conversation is what an ordinary agent records.
    assert_eq!(
        contents(&c.agent),
        vec![
            "go",
            "",
            "Started background agent 'gp#0' (type: gp). You will be notified when it completes.",
            "waiting",
            "[background agent 'gp#0' completed]\nreport on [X1]",
            "final",
        ]
    );
    // The notification was recorded with its parts: the agent's framing, the
    // sender's name, and the delegated agent's reply as that reply.
    assert!(c.root.events().contains(
        &"root envelope Notification [fixed=[background agent '|read agent name=gp#0|fixed=' completed]\n|source root/child0:model:receipt:0=report on [X1]]"
            .to_owned()
    ));
    let seen = c.model.seen.lock().unwrap();
    assert_eq!(
        seen[2].messages,
        vec![
            "input",
            "root:model:receipt:0",
            "root:result:c1",
            "root:model:receipt:1",
            "root:envelope:0",
        ]
    );
    drop(seen);
    assert_eq!(child.recorded_path_calls.load(Ordering::SeqCst), 0);
    assert_eq!(child.requests(), 1);
}

#[tokio::test]
async fn a_message_reaches_a_running_delegated_agent_as_the_text_the_call_wrote() {
    let child = Scripted::new(vec![
        call("s1", "look", serde_json::json!({})),
        Msg::assistant("got it"),
    ]);
    let c = conversation(
        vec![
            call(
                "c1",
                "task",
                serde_json::json!({
                    "description":"start",
                    "subagent_type":"gp",
                    "run_in_background":true
                }),
            ),
            call(
                "c2",
                "send_message",
                serde_json::json!({"to":"gp#0","message":"also [Y2]"}),
            ),
            Msg::assistant("sent"),
            Msg::assistant("final"),
        ],
        vec![spec("gp", child.clone())],
    );
    // The delegated agent's first reply is held until the message is in its
    // inbox, and the root's third reply until the task has ended.
    let root_model = c.model.clone();
    child.hold(0, move || root_model.requests() >= 3);
    let registry = c.registry.clone();
    c.model.hold(2, move || {
        registry.status("gp#0") == Some(AgentStatus::Done)
    });

    c.agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();

    // The delegated agent recorded the message where an ordinary one reads
    // its inbox, with the source the sender's boundary issued for it.
    // The delegated agent recorded the message where an ordinary one reads
    // its inbox, before a round's request, with the source the sender's
    // boundary issued for it.
    let seen = child.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let last = &seen[1];
    let at = last
        .recorded
        .messages
        .iter()
        .position(|message| *message == Msg::user("also [Y2]"))
        .expect("the message is part of the delegated agent's conversation");
    assert_eq!(last.messages[at], "root/child0:envelope:0");
    drop(seen);
    let events = c.root.events();
    assert!(events.contains(
        &"root/child0 envelope InboxMessage [source root:argument:call root:model:receipt:1 c2 /message=also [Y2]=also [Y2]]"
            .to_owned()
    ));
    // The tool's own result names the recipient as the call wrote it.
    assert!(events.contains(
        &"root result c2 [fixed=message sent to |call root:model:receipt:1 c2 /to=gp#0|fixed= (delivered to inbox)]"
            .to_owned()
    ));
    assert_eq!(
        contents(&c.agent)[4],
        "message sent to gp#0 (delivered to inbox)"
    );
    assert_eq!(child.recorded_path_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn names_the_call_wrote_stay_the_calls_own_text_in_a_result() {
    let c = conversation(
        vec![
            call(
                "c1",
                "task",
                serde_json::json!({"description":"x","subagent_type":"[N1]"}),
            ),
            call(
                "c2",
                "send_message",
                serde_json::json!({"to":"[N2]","message":"hello"}),
            ),
            Msg::assistant("done"),
        ],
        vec![spec("gp", Scripted::new(vec![]))],
    );
    c.agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();

    let events = c.root.events();
    assert!(events.contains(
        &"root result c1 [fixed=We cannot invoke subagent |call root:model:receipt:0 c1 /subagent_type=[N1]|fixed= because it does not exist, the only allowed types are |read delegated agents=`gp`]"
            .to_owned()
    ));
    // Nobody has that name, so nothing was delivered.
    assert!(events.contains(
        &"root result c2 [fixed=message sent to |call root:model:receipt:1 c2 /to=[N2]]".to_owned()
    ));
    assert_eq!(
        contents(&c.agent)[2],
        "We cannot invoke subagent [N1] because it does not exist, the only allowed types are `gp`"
    );
    assert_eq!(contents(&c.agent)[4], "message sent to [N2]");
}

#[tokio::test]
async fn a_task_is_not_started_without_a_boundary_for_its_agent() {
    let child = Scripted::new(vec![Msg::assistant("never")]);
    let c = conversation(
        vec![
            call(
                "c1",
                "task",
                serde_json::json!({"description":"x","subagent_type":"gp"}),
            ),
            Msg::assistant("done"),
        ],
        vec![spec("gp", child.clone())],
    );
    c.root.without_children.store(true, Ordering::SeqCst);

    c.agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();

    // The refusal is the recorded result. No agent was made, nothing was
    // registered, and no provider was asked on either path.
    assert!(matches!(
        &c.agent.state.lock().unwrap().messages[2],
        Msg::Tool { content, is_error: true, .. }
            if content.contains("no source view for a delegated agent")
    ));
    assert_eq!(child.requests(), 0);
    assert_eq!(child.recorded_path_calls.load(Ordering::SeqCst), 0);
    assert!(c.registry.list().is_empty());
}

struct Unstated;

#[async_trait::async_trait]
impl AgentMiddleware for Unstated {
    fn name(&self) -> &str {
        "unstated"
    }
}

#[tokio::test]
async fn delegation_makes_no_statement_while_a_delegated_part_makes_none() {
    let stated = SubAgentMiddleware::new(vec![spec("gp", Scripted::new(vec![]))])
        .child_middleware(Arc::new(HitlPolicyMiddleware::new(["look"], "gp")));
    assert_eq!(
        stated.view_support(),
        crate::provenance::ViewSupport::Tracked
    );
    let unstated = SubAgentMiddleware::new(vec![spec("gp", Scripted::new(vec![]))])
        .child_middleware(Arc::new(Unstated));
    assert_eq!(
        unstated.view_support(),
        crate::provenance::ViewSupport::Unadapted
    );

    // An agent with such a middleware is refused before any effect.
    let model = Scripted::new(vec![Msg::assistant("never")]);
    let agent = Agent::builder(model.clone())
        .model_view_boundary(Some(Family::root()))
        .middleware(Arc::new(unstated))
        .build();
    assert!(
        agent
            .run_turn_with_view("go", source("input"))
            .await
            .is_err()
    );
    assert!(agent.state.lock().unwrap().messages.is_empty());
    assert_eq!(model.requests(), 0);
}

#[tokio::test]
async fn a_suspended_background_task_reports_the_parked_call_as_its_replys_text() {
    let child = Scripted::new(vec![call("s1", "look", serde_json::json!({"at":"[P1]"}))]);
    let mut gated = spec("gp", child.clone());
    gated.interrupt_on = vec!["look".into()];
    let c = conversation(
        vec![
            call(
                "c1",
                "task",
                serde_json::json!({
                    "description":"look",
                    "subagent_type":"gp",
                    "run_in_background":true
                }),
            ),
            Msg::assistant("waiting"),
            Msg::assistant("final"),
        ],
        vec![gated],
    );
    let registry = c.registry.clone();
    c.model.hold(1, move || {
        registry.status("gp#0") == Some(AgentStatus::Suspended)
    });

    c.agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();

    assert_eq!(
        contents(&c.agent)[4],
        "[background agent 'gp#0' is waiting for approval]\nlook {\"at\":\"[P1]\"}"
    );
    assert!(c.root.events().contains(
        &"root envelope Notification [fixed=[background agent '|read agent name=gp#0|fixed=' is waiting for approval]\n|read tool name=look|fixed= |json root/child0:model:receipt:0 s1 ={\"at\":\"[P1]\"}]"
            .to_owned()
    ));
    // The gated tool did not run.
    assert_eq!(child.requests(), 1);
}

#[tokio::test]
async fn a_call_a_person_changed_has_no_text_of_the_reply_in_its_result() {
    let root = Family::root();
    let model = Scripted::new(vec![
        call(
            "c1",
            "send_message",
            serde_json::json!({"to":"[N1]","message":"hello"}),
        ),
        Msg::assistant("done"),
    ]);
    let agent = Agent::builder(model.clone())
        .name("main")
        .model_view_boundary(Some(root.clone()))
        .middleware(Arc::new(
            SubAgentMiddleware::new(vec![spec("gp", Scripted::new(vec![]))]).for_agent("main"),
        ))
        .middleware(Arc::new(HitlPolicyMiddleware::new(
            ["send_message"],
            "main",
        )))
        .build();
    let suspended = agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();
    assert!(matches!(suspended, TurnResult::Suspended(_)));

    agent
        .resume_with_view(
            HitlAction::ApproveEdited {
                args: serde_json::json!({"to":"someone else","message":"changed"}),
            },
            "denied",
            source("decision"),
        )
        .await
        .unwrap();

    // The call ran with the person's arguments. Its result says nothing
    // about text of the reply: the recipient's name is not the reply's.
    assert_eq!(contents(&agent)[2], "message sent to someone else");
    assert!(root.events().contains(&"root result c1 []".to_owned()));
}

#[tokio::test]
async fn a_cancelled_background_task_reports_its_own_closing_note() {
    // The delegated agent's provider never answers.
    let child = Scripted::new(vec![Msg::assistant("never")]);
    child.hold(0, || false);
    let root = Family::root();
    let model = Scripted::new(vec![
        call(
            "c1",
            "task",
            serde_json::json!({
                "description":"slow work",
                "subagent_type":"gp",
                "run_in_background":true
            }),
        ),
        Msg::assistant("started"),
        Msg::assistant("noted"),
    ]);
    let registry = Arc::new(AgentRegistry::default());
    let agent = Agent::builder(model.clone())
        .name("main")
        .registry(registry.clone())
        .interrupt_model_on_cancel(true)
        .cancellation_session(crate::SessionCancellation::default())
        .model_view_boundary(Some(root.clone()))
        .middleware(Arc::new(
            SubAgentMiddleware::new(vec![spec("gp", child.clone())]).for_agent("main"),
        ))
        .build();
    agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();

    // The task is cancelled through the registry, as the task view does,
    // while its agent waits for a reply.
    for _ in 0..5_000 {
        if child.requests() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert!(registry.abort("gp#0"));
    for _ in 0..5_000 {
        if registry.status("gp#0") != Some(AgentStatus::Running) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert_eq!(registry.status("gp#0"), Some(AgentStatus::Failed));
    assert_eq!(registry.progress("gp#0").as_deref(), Some("cancelled"));

    // The next turn records what the task ended with: the delegated agent's
    // own closing note, with the source its boundary gave for that note.
    agent
        .run_turn_with_view("and now?", source("second"))
        .await
        .unwrap();
    assert_eq!(
        contents(&agent)[4],
        "[background agent 'gp#0' completed]\nStopped by the user."
    );
    assert!(root.events().contains(
        &"root envelope Notification [fixed=[background agent '|read agent name=gp#0|fixed=' completed]\n|source root/child0:framework:CancelNote=Stopped by the user.]"
            .to_owned()
    ));
    assert_eq!(child.requests(), 1);
}

/// A middleware that adds one note of the host's own before a round, once,
/// and remembers under its own key that it did.
struct Notes;

#[async_trait::async_trait]
impl AgentMiddleware for Notes {
    fn name(&self) -> &str {
        "notes"
    }

    fn view_support(&self) -> crate::provenance::ViewSupport {
        crate::provenance::ViewSupport::Tracked
    }

    async fn prepare_model_with_view(
        &self,
        prepare: &mut crate::provenance::TrackedPrepare<'_>,
    ) -> Result<(), AgentError> {
        if prepare.state().custom.contains_key("notes.seen") {
            return Ok(());
        }
        prepare
            .append_host_message(vec![
                SourcePart::fixed(Arc::from("<note>")),
                SourcePart::read("runtime data", Arc::from("three gadgets")),
                SourcePart::fixed(Arc::from("</note>")),
            ])
            .await?;
        prepare.set_own_custom("notes.seen", serde_json::Value::Bool(true));
        Ok(())
    }
}

#[tokio::test]
async fn a_tracked_middleware_appends_a_host_note_as_parts() {
    let root = Family::root();
    let model = Scripted::new(vec![Msg::assistant("one"), Msg::assistant("two")]);
    let agent = Agent::builder(model.clone())
        .model_view_boundary(Some(root.clone()))
        .middleware(Arc::new(Notes))
        .build();
    agent
        .run_turn_with_view("go", source("input"))
        .await
        .unwrap();

    // The note is recorded where the ordinary hook records it, after the
    // input and before the reply, and went out with the source the boundary
    // gave for its parts.
    assert_eq!(
        contents(&agent),
        vec!["go", "<note>three gadgets</note>", "one"]
    );
    assert!(
        root.events().contains(
            &"root envelope HostNote [fixed=<note>|read runtime data=three gadgets|fixed=</note>]"
                .to_owned()
        )
    );
    assert_eq!(
        model.seen.lock().unwrap()[0].messages,
        vec!["input", "root:envelope:0"]
    );
    assert!(
        agent
            .recorded_sources()
            .unwrap()
            .iter()
            .all(Option::is_some)
    );

    // It is added once: the next turn finds the middleware's own key.
    agent
        .run_turn_with_view("again", source("second"))
        .await
        .unwrap();
    assert_eq!(
        contents(&agent),
        vec!["go", "<note>three gadgets</note>", "one", "again", "two"]
    );
    assert_eq!(
        model.seen.lock().unwrap()[1].messages,
        vec!["input", "root:envelope:0", "root:model:receipt:0", "second"]
    );
}
