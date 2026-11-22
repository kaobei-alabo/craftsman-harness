//! What a person reads and what reaches a file in a privacy conversation:
//! the human view of a committed reply, the hold on a streamed reply, and
//! file writes and edits derived, reviewed and run with the real values.
//! The vault, the guard and the Chat provider are real, the server is a
//! loopback one, and the files are real temporary files.

use super::*;
use crate::privacy_restore::{FileDataFormat, RestoreError};

/// A user's rule for file content: one data format for every file, or none.
/// It records every destination it was asked about.
struct Rule(Option<FileDataFormat>, Arc<Mutex<Vec<std::path::PathBuf>>>);

impl CapturedUserPolicy for Rule {
    fn enables_privacy_in(&self, _: &str) -> bool {
        true
    }

    fn requires_inspectable_provider_items(&self) -> bool {
        false
    }

    fn restorable_file_data(
        &self,
        _: &str,
        destination: &std::path::Path,
    ) -> Option<FileDataFormat> {
        self.1.lock().unwrap().push(destination.to_owned());
        self.0
    }
}

/// What a file tool did around its write, in order, and whether it reports
/// what a checker says about the file afterwards.
#[derive(Default)]
struct Around {
    steps: Mutex<Vec<String>>,
    diagnose: std::sync::atomic::AtomicBool,
}

/// A file tool shaped as the application's own: it notes the file for a
/// checkpoint, writes, finishes the checkpoint and reports diagnostics. Its
/// write is the derived one when there is one, and its arguments otherwise.
struct PlainFile(&'static str, std::path::PathBuf, Arc<Around>);

#[async_trait::async_trait]
impl Tool for PlainFile {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.0.into(),
            description: "Write or edit a file.".into(),
            parameters: json!({"type": "object"}),
        }
    }

    async fn call(&self, args: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let path = args["file_path"].as_str().unwrap_or_default();
        let step = |what: &str| self.2.steps.lock().unwrap().push(format!("{what} {path}"));
        step("began");
        let written = match crate::privacy_session::authorized_write() {
            Some(written) => written.map(|_| ()).map_err(|refusal| refusal.to_string()),
            None => {
                let text = args["content"]
                    .as_str()
                    .or(args["new_string"].as_str())
                    .unwrap_or_default();
                std::fs::write(self.1.join(path), text).map_err(|error| error.to_string())
            }
        };
        step("finished");
        if let Err(refusal) = written {
            return Ok(ToolOutcome::err(refusal));
        }
        let mut message = format!("Updated file {path}");
        if self.2.diagnose.load(std::sync::atomic::Ordering::SeqCst) {
            // A checker quotes the file as it is on disk now.
            let on_disk = std::fs::read_to_string(self.1.join(path)).unwrap();
            step("checked");
            message.push_str(&format!(
                "\n\nDiagnostics:\nline 1: unknown word in '{on_disk}'"
            ));
        }
        Ok(ToolOutcome::ok(message))
    }
}

/// What the reviewer does with a reviewed operation.
#[derive(Clone, Copy, PartialEq)]
enum Decide {
    /// A person looks at the preview and approves.
    Approve,
    /// A person looks at the preview and refuses.
    Deny,
    /// Somebody looks, nothing is decided, and the call is let through.
    LookOnly,
    /// Nothing is decided here; the decision of another operation is
    /// handed to the agent in its place.
    CarryAnother,
    /// The call is parked for a person.
    Park,
    /// Approve, then change the destination before the tool runs.
    ApproveThenDisturb,
    /// The gate's automatic policy decides, in the reviewer's own state.
    Policy,
}

/// What the reviewer was shown.
#[derive(Default, Clone, PartialEq, Debug)]
struct Shown {
    create: bool,
    path: std::path::PathBuf,
    old: Option<String>,
    new: String,
    arguments: Value,
}

impl Shown {
    fn of(preview: &FilePreview<'_>) -> Self {
        Self {
            create: preview.change == FileChange::Create,
            path: preview.path.to_owned(),
            old: preview.old.map(str::to_owned),
            new: preview.new.to_owned(),
            arguments: preview.arguments.clone(),
        }
    }
}

/// The review before a file tool runs, as a gate does it: take the
/// operation the agent carries for this dispatch, look, decide.
struct Reviewer {
    decide: Arc<Mutex<Decide>>,
    shown: Mutex<Vec<Shown>>,
    refusals: Mutex<Vec<FileRefusal>>,
    /// The decision of some other operation, for [`Decide::CarryAnother`].
    another: Mutex<Option<SourceRef>>,
    /// The policy state of this gate, for [`Decide::Policy`].
    approval: Arc<Mutex<crate::hitl::ApprovalState>>,
    source: Arc<crate::privacy_entry::InlineDecisionProducer>,
    dir: std::path::PathBuf,
}

#[async_trait::async_trait]
impl AgentMiddleware for Reviewer {
    fn name(&self) -> &str {
        "reviewer"
    }

    fn view_support(&self) -> ViewSupport {
        ViewSupport::Preserving
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let tracked = next.tracked().expect("the agent tracks this dispatch");
        // The plan the agent issued for this dispatch is the plan of this
        // call, and no terminal exists yet.
        assert_eq!(tracked.planned().unwrap().call(), &call);
        assert!(tracked.invocation().is_none());
        let Some(reviewed) = ReviewedFile::of(&tracked) else {
            return next.run(call, ctx).await;
        };
        let derived = match reviewed.derived() {
            Ok(derived) => derived,
            Err(refusal) => {
                assert!(tracked.shown().is_none());
                self.refusals.lock().unwrap().push(refusal.clone());
                return Ok(ToolOutcome::err(refusal.to_string()));
            }
        };
        let look = |preview: &FilePreview<'_>| {
            // The agent carries the digest of exactly this preview.
            assert_eq!(tracked.shown(), Some(preview.digest));
            self.shown.lock().unwrap().push(Shown::of(preview));
        };
        let decide = *self.decide.lock().unwrap();
        match decide {
            Decide::Approve | Decide::ApproveThenDisturb => {
                derived.preview(|preview| look(&preview)).unwrap();
                let resolution = self.source.receive_inline(&tracked, None).await.unwrap();
                assert_eq!(resolution.permission, crate::hitl::ToolPermission::Allow);
                if decide == Decide::ApproveThenDisturb {
                    let path = call.args["file_path"].as_str().unwrap();
                    std::fs::write(self.dir.join(path), "changed by someone else").unwrap();
                }
            }
            Decide::Deny => {
                derived.preview(|preview| look(&preview)).unwrap();
                let resolution = self.source.receive_inline(&tracked, None).await.unwrap();
                assert!(matches!(
                    resolution.permission,
                    crate::hitl::ToolPermission::Deny(_)
                ));
                return Ok(ToolOutcome::err("Denied by the reviewer."));
            }
            Decide::LookOnly => derived.preview(|preview| look(&preview)).unwrap(),
            Decide::CarryAnother => {
                derived.preview(|preview| look(&preview)).unwrap();
                let another = self.another.lock().unwrap().clone().unwrap();
                tracked.note_decision(another).unwrap();
            }
            Decide::Park => {
                return Err(AgentError::Suspended(Box::new(Suspension {
                    frames: vec![],
                    request: HitlRequest {
                        tool_call: call,
                        summary: "write the file?".into(),
                    },
                })));
            }
            Decide::Policy => {
                derived.preview(|preview| look(&preview)).unwrap();
                match self
                    .source
                    .receive_inline(&tracked, None)
                    .await
                    .unwrap()
                    .permission
                {
                    crate::hitl::ToolPermission::Allow => {}
                    crate::hitl::ToolPermission::Deny(reason) => {
                        return Ok(ToolOutcome::err(reason));
                    }
                    crate::hitl::ToolPermission::Prompt => {
                        return Ok(ToolOutcome::err("The policy asks a person."));
                    }
                }
            }
        }
        next.run(call, ctx).await
    }
}

struct Files {
    session: Arc<PrivacySession>,
    agent: Arc<Agent>,
    reviewer: Arc<Reviewer>,
    /// The user's policy. The fixture is its owner, as a session is; the
    /// review and the tools only refer to it.
    policy: Option<Arc<dyn CapturedUserPolicy>>,
    /// Every destination the policy was asked about.
    asked: Arc<Mutex<Vec<std::path::PathBuf>>>,
    around: Arc<Around>,
    dir: tempfile::TempDir,
    placeholder: String,
    typed: String,
    source: SourceRef,
}

/// One conversation with the two file tools behind a reviewer, and the
/// placeholder the conversation issued for the address in its input.
async fn files(vault: &Vault, server: &MockServer, rule: Option<FileDataFormat>) -> Files {
    files_with(vault, server, rule, Arc::new(|_: &str| {})).await
}

/// As [`files`], with the sink the provider streams a reply to.
async fn files_with(
    vault: &Vault,
    server: &MockServer,
    rule: Option<FileDataFormat>,
    sink: crate::agent::TokenSink,
) -> Files {
    set_up(vault, server, rule, sink, None).await
}

/// A delegated agent's side of a conversation: the server its model is on,
/// what its gate can ask, and the policy state of that gate.
struct Helper<'a> {
    side: &'a MockServer,
    prompt: Option<crate::agent::ApprovalPrompt>,
    approval: crate::hitl::ApprovalState,
    private: Option<Arc<crate::privacy_entry::PrivacyApprovalPrompt>>,
}

async fn set_up(
    vault: &Vault,
    server: &MockServer,
    rule: Option<FileDataFormat>,
    sink: crate::agent::TokenSink,
    helper: Option<Helper<'_>>,
) -> Files {
    set_up_with_main_gate(vault, server, rule, sink, helper, None).await
}

struct MainGate {
    approval: Arc<Mutex<crate::hitl::ApprovalState>>,
    legacy: Option<crate::agent::ApprovalPrompt>,
    private: Option<Arc<crate::privacy_entry::PrivacyApprovalPrompt>>,
    store: Arc<craftsman_harness::testing::MemCheckpointer>,
}

async fn set_up_with_main_gate(
    vault: &Vault,
    server: &MockServer,
    rule: Option<FileDataFormat>,
    sink: crate::agent::TokenSink,
    helper: Option<Helper<'_>>,
    main: Option<MainGate>,
) -> Files {
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
    let system = session
        .admit_raw(&CapturedRaw::from_producer(
            RawOrigin::HostStatic,
            "You are a careful assistant.",
        ))
        .await
        .unwrap();
    let boundary = session.boundary(&[system]).unwrap();
    let provider = ViewChatProvider::new(
        client(server),
        session.clone(),
        Arc::new(Mutex::new((0, 0))),
        sink,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let asked: Arc<Mutex<Vec<std::path::PathBuf>>> = Arc::default();
    let policy: Arc<dyn CapturedUserPolicy> = Arc::new(Rule(rule, asked.clone()));
    // The boundary of the conversation derives planned writes from now on,
    // for this agent and every agent it delegates to.
    let review = FileReview::new(&policy, dir.path().to_path_buf());
    boundary.review_files(review.clone());
    let decision = Arc::new(Mutex::new(Decide::Approve));
    let approval = Arc::new(Mutex::new(crate::hitl::ApprovalState::default()));
    let received_decision = decision.clone();
    let source = crate::privacy_entry::InlineDecisionProducer::register_reader(
        &boundary,
        approval.clone(),
        dir.path().to_path_buf(),
        Arc::new(move |_, _, _, _| match *received_decision.lock().unwrap() {
            Decide::Deny => {
                crate::privacy_entry::HumanChoice::Deny("Denied by the reviewer.".into())
            }
            Decide::Policy => crate::privacy_entry::HumanChoice::Absent,
            _ => crate::privacy_entry::HumanChoice::Approve,
        }),
    );
    let reviewer = Arc::new(Reviewer {
        decide: decision,
        shown: Mutex::default(),
        refusals: Mutex::default(),
        another: Mutex::default(),
        approval,
        source,
        dir: dir.path().to_path_buf(),
    });
    let mut builder =
        Agent::builder(Arc::new(provider)).model_view_boundary(Some(boundary.clone()));
    builder = match main {
        None => builder.middleware(reviewer.clone()),
        Some(main) => builder
            .middleware(Arc::new(
                crate::hitl_middleware::ApprovalMiddleware::new_at(
                    main.approval,
                    false,
                    Some(Arc::new(|_| {})),
                    None,
                    main.legacy,
                    dir.path().to_path_buf(),
                )
                .with_privacy_prompt(main.private)
                .with_privacy_boundary(Some(&boundary)),
            ))
            .checkpointer(main.store)
            .durable_turn_checkpoints(true),
    };
    let around = Arc::new(Around::default());
    let mut plain: Vec<Arc<dyn Tool>> = Vec::new();
    for name in ["write_file", "edit_file"] {
        plain.push(Arc::new(PlainFile(
            name,
            dir.path().to_path_buf(),
            around.clone(),
        )));
    }
    if let Some(helper) = helper {
        // A delegated agent with the two file tools as the application
        // gives them to one: the same tools behind the agent's own gate.
        let delegated = delegated_provider(
            client(helper.side),
            session.clone(),
            Arc::new(Mutex::new((0, 0))),
            Arc::new(|_: &str| {}),
        )
        .await
        .unwrap();
        let bound_prompt = helper.private.or_else(|| {
            helper
                .prompt
                .as_ref()
                .map(|read| bound_test_ui(read.clone()))
        });
        let spec = SubAgentSpec {
            name: "helper".into(),
            description: "Writes notes.".into(),
            system_prompt: "You write notes.".into(),
            tools: crate::agent::gated_for_delegation_with_privacy(
                plain.clone(),
                &Arc::new(Mutex::new(helper.approval)),
                (&helper.prompt, &None),
                dir.path(),
                Some(&review),
                Some(&boundary),
                bound_prompt,
            ),
            model: Some(delegated),
            harness_profile: None,
            interrupt_on: vec![],
            max_depth: 1,
        };
        builder = builder.name("main").middleware(Arc::new(
            SubAgentMiddleware::new(vec![spec]).for_agent("main"),
        ));
    }
    for tool in plain {
        builder = builder.tool(Arc::new(RestoredFileTool::new(tool, review.clone())));
    }
    let agent = Arc::new(builder.build());
    let typed = format!("who handles {EMAIL}?");
    let node = session.admit_raw(&draft(&typed)).await.unwrap();
    Files {
        placeholder: issued(&node),
        source: session.source(&node).unwrap(),
        session,
        agent,
        reviewer,
        policy: Some(policy),
        asked,
        around,
        dir,
        typed,
    }
}

fn file_call(id: &str, name: &str, arguments: Value) -> ResponseTemplate {
    sse(
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": id, "type": "function",
        "function": {"name": name, "arguments": arguments.to_string()}}]}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 1}}),
    )
}

/// The tool result recorded at `index`.
fn result_at(agent: &Agent, index: usize) -> (String, bool) {
    match &recorded(agent)[index] {
        Msg::Tool {
            content, is_error, ..
        } => (content.clone(), *is_error),
        other => panic!("not a tool result: {other:?}"),
    }
}

/// The invocation the agent issued at the terminal of its last tool call.
fn last_invocation(agent: &Agent) -> craftsman_harness::provenance::ActualToolInvocation {
    agent
        .actual_terminal_outcomes()
        .unwrap()
        .last()
        .unwrap()
        .invocation()
        .clone()
}

/// A person's approval of the parked call, from a reader that shows the
/// person what the application's own reader shows: the call the agent
/// carried, with the operation's preview taken under the digest the agent
/// handed over. `seen` is what the person was shown.
fn person_reads(
    session: &Arc<PrivacySession>,
    agent: &Agent,
    seen: &Arc<Mutex<Vec<Value>>>,
) -> SourceRef {
    let seen = seen.clone();
    let producer = crate::privacy_entry::HumanDecisionProducer::register(
        session.clone(),
        Arc::new(move |parked, shown| match call_to_show(parked, shown) {
            Some(call) => {
                seen.lock().unwrap().push(call.args);
                crate::privacy_entry::HumanChoice::Approve
            }
            None => crate::privacy_entry::HumanChoice::Absent,
        }),
    );
    let parked = agent.actual_parked_call().unwrap();
    SourceRef::new(producer.receive(&parked).unwrap())
}

async fn nothing_original_sent(server: &MockServer) {
    for request in server.received_requests().await.unwrap() {
        let raw = String::from_utf8_lossy(&request.body);
        assert!(!raw.contains(EMAIL) && !raw.contains(PHONE));
    }
}

#[tokio::test]
async fn a_person_reads_the_values_of_a_committed_reply_and_the_record_keeps_placeholders() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let f = files(&vault, &server, None).await;
    let p = &f.placeholder;
    // The reply names the placeholder twice as a token, once inside a longer
    // token, and names an alias nobody issued here, around other scripts.
    let unknown = "pv1-00000000000000000000000000000000@privacy.invalid";
    let reply = format!("Größe 日本: write to {p}, then {p}. Not x{p}y, not {unknown}.");
    scripted(
        &server,
        vec![answer("chat", &[&reply]), answer("chat", &["again"])],
    )
    .await;
    f.agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();

    let sources = f.agent.recorded_sources().unwrap();
    let human = f.session.human_reply(sources[1].as_ref().unwrap()).unwrap();
    assert_eq!(
        human.read(str::to_owned).unwrap(),
        format!("Größe 日本: write to {EMAIL}, then {EMAIL}. Not x{p}y, not {unknown}.")
    );
    assert_eq!((human.restored(), human.unresolved()), (2, 1));
    // The view is nobody's record: the recorded reply still holds the
    // placeholders, and so does the next request.
    assert_eq!(recorded(&f.agent)[1], Msg::assistant(reply.clone()));
    let node = f.session.admit_raw(&draft("and then?")).await.unwrap();
    f.agent
        .run_turn_with_view("and then?", f.session.source(&node).unwrap())
        .await
        .unwrap();
    let second = &bodies(&server).await[1];
    assert_eq!(second["messages"][2]["content"], reply.as_str());
    nothing_original_sent(&server).await;

    // The source of an input is not a reply, and a reply of another
    // conversation is not this one's.
    assert_eq!(
        f.session.human_reply(sources[0].as_ref().unwrap()).err(),
        Some(HumanRefusal::NotAReply)
    );
    let other = files(&vault, &server, None).await;
    assert_eq!(
        other
            .session
            .human_reply(sources[1].as_ref().unwrap())
            .err(),
        Some(HumanRefusal::NotAReply)
    );
    // A stopped conversation gives no view, and a view already made cannot
    // be read any more.
    f.session.stop();
    assert!(matches!(
        f.session.human_reply(sources[1].as_ref().unwrap()),
        Err(HumanRefusal::Stopped(_))
    ));
    assert!(human.read(str::len).is_err());
}

#[tokio::test]
async fn a_streamed_reply_is_held_until_it_is_committed_and_then_shown_once() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    // The conversation's provider streams into the hold, not to a terminal.
    let hold = HeldStream::new();
    let f = files_with(&vault, &server, None, hold.sink()).await;
    let p = &f.placeholder;
    // The provider streams the placeholder in two pieces.
    let (first, second) = p.split_at(p.len() / 2);
    let chunk = |text: &str| json!({"choices": [{"delta": {"content": text}}]});
    let end = json!({"choices": [{"delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2}});
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: {end}\n\ndata: [DONE]\n\n",
        chunk(&format!("ask {first}")),
        chunk(&format!("{second} now")),
    );
    scripted(
        &server,
        vec![ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")],
    )
    .await;
    let agent = &f.agent;
    agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();

    // Both pieces are held, and nothing of them was shown anywhere.
    assert_eq!(hold.held_bytes(), format!("ask {p} now").len());
    let source = agent.recorded_sources().unwrap()[1].clone().unwrap();
    let (human, streamed) = hold.committed(&f.session, &source).unwrap();
    assert_eq!(streamed, Streamed::Whole);
    assert_eq!(
        human.read(str::to_owned).unwrap(),
        format!("ask {EMAIL} now")
    );
    assert_eq!(hold.held_bytes(), 0);

    // More than the hold keeps: the view still comes from the committed
    // reply, and the hold says that the stream is not what it kept.
    let sink = hold.sink();
    for _ in 0..300 {
        sink(&"x".repeat(1024));
    }
    assert_eq!(hold.held_bytes(), 0);
    let (human, streamed) = hold.committed(&f.session, &source).unwrap();
    assert_eq!(streamed, Streamed::Overflowed);
    assert_eq!(human.restored(), 1);
    // A cancelled turn drops what was held.
    sink("partial pv1-");
    hold.discard();
    assert_eq!(hold.held_bytes(), 0);
    assert_eq!(
        hold.committed(&f.session, &source).unwrap().1,
        Streamed::Nothing
    );
    nothing_original_sent(&server).await;
}

#[tokio::test]
async fn a_created_file_holds_the_real_value_and_the_call_keeps_the_placeholder() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
    let p = &f.placeholder;
    let arguments = json!({"file_path": "note.txt", "content": format!("Contact {p} today.\n")});
    scripted(
        &server,
        vec![
            file_call("c1", "write_file", arguments.clone()),
            answer("chat", &["done"]),
        ],
    )
    .await;
    f.agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();

    // The reviewer saw the real content of a file that does not exist yet.
    let shown = f.reviewer.shown.lock().unwrap().clone();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].create && shown[0].old.is_none());
    assert_eq!(shown[0].new, format!("Contact {EMAIL} today.\n"));
    assert_eq!(shown[0].arguments["file_path"], "note.txt");
    // That content is what was written, through the derived operation and
    // not by the ordinary tool.
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("note.txt")).unwrap(),
        format!("Contact {EMAIL} today.\n")
    );
    assert_eq!(
        result_at(&f.agent, 2),
        ("Updated file note.txt".to_owned(), false)
    );
    // The recorded call and the next request still hold the placeholder.
    assert!(
        matches!(&recorded(&f.agent)[1], Msg::Assistant { tool_calls, .. }
        if tool_calls[0].args == arguments)
    );
    let second = &bodies(&server).await[1];
    assert!(
        second["messages"][2]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap()
            .contains(p.as_str())
    );
    nothing_original_sent(&server).await;
    // The policy was asked about the file the operation is bound to, as the
    // host resolved it: when it was derived and again at the write. It was
    // never asked about the path as the model spelled it.
    let bound = f.dir.path().canonicalize().unwrap().join("note.txt");
    assert_eq!(shown[0].path, bound);
    assert_eq!(*f.asked.lock().unwrap(), [bound.clone(), bound]);
    // The terminal carried the plan, the source and the digest of this
    // operation and a decision the operation issued itself. The operation
    // ran once, is over and holds nothing any more.
    let invocation = last_invocation(&f.agent);
    assert!(invocation.shown().is_some() && invocation.decision_source().is_some());
    let reviewed = invocation
        .review_source()
        .and_then(|source| source.downcast::<ReviewedFile>())
        .expect("the terminal carries the reviewed operation");
    assert!(reviewed.is_done());
    assert_eq!(reviewed.shown_arguments().err(), Some(FileRefusal::Spent));
    assert!(f.session.check().is_ok());
}

#[tokio::test]
async fn an_edit_is_shown_with_the_real_old_and_new_file_and_a_json_file_stays_json() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let f = files(&vault, &server, Some(FileDataFormat::JsonDocument)).await;
    let p = &f.placeholder;
    let path = f.dir.path().join("contacts.json");
    let before = format!("{{\"owner\": \"{EMAIL}\", \"note\": \"none\"}}\n");
    std::fs::write(&path, &before).unwrap();
    scripted(
        &server,
        vec![
            file_call(
                "c1",
                "edit_file",
                json!({
                    "file_path": "contacts.json",
                    "old_string": format!("\"owner\": \"{p}\", \"note\": \"none\""),
                    "new_string": format!("\"owner\": \"{p}\", \"note\": \"ask {p}\"")
                }),
            ),
            answer("chat", &["done"]),
        ],
    )
    .await;
    f.agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();

    let after = format!("{{\"owner\": \"{EMAIL}\", \"note\": \"ask {EMAIL}\"}}\n");
    let shown = f.reviewer.shown.lock().unwrap().clone();
    assert!(!shown[0].create);
    assert_eq!(shown[0].old.as_deref(), Some(before.as_str()));
    assert_eq!(shown[0].new, after);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), after);
    assert_eq!(
        result_at(&f.agent, 2),
        ("Updated file contacts.json".to_owned(), false)
    );
    nothing_original_sent(&server).await;
}

#[tokio::test]
async fn what_cannot_be_derived_or_was_not_approved_is_refused_before_the_file_is_touched() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let unknown = "pv1-00000000000000000000000000000000@privacy.invalid";
    let content = |p: &str| json!({"file_path": "note.txt", "content": format!("to {p}")});
    type Case = (
        Option<FileDataFormat>,
        Decide,
        Box<dyn Fn(&str) -> Value>,
        Option<FileRefusal>,
    );
    let cases: Vec<Case> = vec![
        // No rule of the user's policy for this file.
        (
            None,
            Decide::Approve,
            Box::new(content),
            Some(FileRefusal::NoRule),
        ),
        // A placeholder in the path, which is never restored.
        (
            Some(FileDataFormat::PlainText),
            Decide::Approve,
            Box::new(|p| json!({"file_path": format!("{p}.txt"), "content": "x"})),
            Some(FileRefusal::Excluded("another argument of the call")),
        ),
        // An alias this conversation did not issue.
        (
            Some(FileDataFormat::PlainText),
            Decide::Approve,
            Box::new(
                move |p| json!({"file_path": "note.txt", "content": format!("{p} {unknown}")}),
            ),
            Some(FileRefusal::Restore(RestoreError::UnknownAlias)),
        ),
        // Content that is not the JSON document the rule names.
        (
            Some(FileDataFormat::JsonDocument),
            Decide::Approve,
            Box::new(content),
            Some(FileRefusal::Restore(RestoreError::Input)),
        ),
        // Derived and shown, then denied.
        (
            Some(FileDataFormat::PlainText),
            Decide::Deny,
            Box::new(content),
            None,
        ),
        // Derived and shown, and let through without any decision.
        (
            Some(FileDataFormat::PlainText),
            Decide::LookOnly,
            Box::new(content),
            None,
        ),
    ];
    for (rule, decide, arguments, refusal) in cases {
        let server = MockServer::start().await;
        let f = files(&vault, &server, rule).await;
        *f.reviewer.decide.lock().unwrap() = decide;
        scripted(
            &server,
            vec![
                file_call("c1", "write_file", arguments(&f.placeholder)),
                answer("chat", &["done"]),
            ],
        )
        .await;
        f.agent
            .run_turn_with_view(f.typed.clone(), f.source.clone())
            .await
            .unwrap();
        // The tool's answer is the refusal, nothing exists on disk, the
        // call is recorded as the model wrote it and the conversation goes
        // on.
        let (text, is_error) = result_at(&f.agent, 2);
        assert!(is_error, "{text}");
        assert_eq!(
            f.reviewer.refusals.lock().unwrap().first().cloned(),
            refusal
        );
        if let Some(refusal) = &refusal {
            assert_eq!(text, refusal.to_string());
        }
        assert_eq!(std::fs::read_dir(f.dir.path()).unwrap().count(), 0);
        assert!(f.session.check().is_ok());
        assert_eq!(recorded(&f.agent).len(), 4);
        nothing_original_sent(&server).await;
    }
}

#[tokio::test]
async fn a_file_that_changed_after_it_was_shown_is_not_written() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    // An edit: someone rewrites the file after the approval.
    let server = MockServer::start().await;
    let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
    let p = &f.placeholder;
    let path = f.dir.path().join("note.txt");
    std::fs::write(&path, "owner: nobody\n").unwrap();
    *f.reviewer.decide.lock().unwrap() = Decide::ApproveThenDisturb;
    scripted(
        &server,
        vec![
            file_call(
                "c1",
                "edit_file",
                json!({"file_path": "note.txt", "old_string": "nobody", "new_string": p}),
            ),
            answer("chat", &["done"]),
        ],
    )
    .await;
    f.agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();
    assert_eq!(
        result_at(&f.agent, 2),
        (FileRefusal::Stale.to_string(), true)
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "changed by someone else"
    );

    // A create: the file appears after the approval.
    let server = MockServer::start().await;
    let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
    *f.reviewer.decide.lock().unwrap() = Decide::ApproveThenDisturb;
    scripted(
        &server,
        vec![
            file_call(
                "c1",
                "write_file",
                json!({"file_path": "note.txt", "content": format!("to {}", f.placeholder)}),
            ),
            answer("chat", &["done"]),
        ],
    )
    .await;
    f.agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();
    assert_eq!(
        result_at(&f.agent, 2),
        (FileRefusal::Stale.to_string(), true)
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("note.txt")).unwrap(),
        "changed by someone else"
    );
    assert!(f.session.check().is_ok());
}

#[tokio::test]
async fn a_parked_write_runs_after_the_person_saw_the_real_content_and_only_as_shown() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
    let p = &f.placeholder;
    *f.reviewer.decide.lock().unwrap() = Decide::Park;
    scripted(
        &server,
        vec![
            file_call(
                "c1",
                "write_file",
                json!({"file_path": "note.txt", "content": format!("to {p}")}),
            ),
            answer("chat", &["done"]),
        ],
    )
    .await;
    let suspended = f
        .agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();
    assert!(matches!(suspended, TurnResult::Suspended(_)));
    // The call is parked. The agent carries the operation and its digest
    // with the parked call, and nothing is on disk.
    let parked = f.agent.actual_parked_call().unwrap();
    let reviewed = ReviewedFile::of_parked(&parked).expect("the parked call carries its operation");
    assert!(parked.shown().is_some());
    assert!(!f.dir.path().join("note.txt").exists());

    // A reader that asks a person without showing the operation: its
    // approval is a real input of that person, with the digest the agent
    // carried, but nobody was shown what will be written. The tool refuses,
    // and the operation is over.
    let decision = fixture_human_input(&f.session, &f.agent, &HitlAction::Approve);
    assert_eq!(
        decision
            .downcast::<crate::privacy_entry::HumanDecisionInput>()
            .unwrap()
            .shown(),
        parked.shown()
    );
    f.agent
        .resume_with_view(HitlAction::Approve, "Denied.", decision)
        .await
        .unwrap();
    assert_eq!(
        result_at(&f.agent, 2),
        (FileRefusal::NotApproved.to_string(), true)
    );
    assert!(!f.dir.path().join("note.txt").exists());
    assert!(!reviewed.is_done());
    assert_eq!(reviewed.shown_arguments().err(), Some(FileRefusal::Spent));

    // The same again with a reader that shows the person the preview under
    // the digest the agent handed over: the resumed call writes the real
    // content.
    let server = MockServer::start().await;
    let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
    *f.reviewer.decide.lock().unwrap() = Decide::Park;
    scripted(
        &server,
        vec![
            file_call(
                "c1",
                "write_file",
                json!({"file_path": "note.txt", "content": format!("to {}", f.placeholder)}),
            ),
            answer("chat", &["done"]),
        ],
    )
    .await;
    f.agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();
    let parked = f.agent.actual_parked_call().unwrap();
    let reviewed = ReviewedFile::of_parked(&parked).unwrap();
    // Under a digest that is not this operation's, or under none, a reader
    // is shown nothing.
    for wrong in [Some([7; 32]), None] {
        assert_eq!(
            reviewed.shown_to_reader(wrong, |_| ()).err(),
            Some(FileRefusal::NotApproved)
        );
    }
    let seen = Arc::default();
    let decision = person_reads(&f.session, &f.agent, &seen);
    f.agent
        .resume_with_view(HitlAction::Approve, "Denied.", decision)
        .await
        .unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        [json!({"file_path": "note.txt", "content": format!("to {EMAIL}")})]
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("note.txt")).unwrap(),
        format!("to {EMAIL}")
    );
    assert_eq!(
        result_at(&f.agent, 2),
        ("Updated file note.txt".to_owned(), false)
    );
    // It ran once, as the plan that was parked.
    let invocation = last_invocation(&f.agent);
    assert!(invocation.resumed() && invocation.was_planned_as(parked.planned()));
    assert!(reviewed.is_done());
    assert_eq!(reviewed.shown_arguments().err(), Some(FileRefusal::Spent));
    nothing_original_sent(&server).await;
}

#[tokio::test]
async fn a_derived_write_runs_inside_the_file_tool_and_its_diagnostics_reach_the_model_as_a_view() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
    f.around
        .diagnose
        .store(true, std::sync::atomic::Ordering::SeqCst);
    scripted(
        &server,
        vec![
            file_call(
                "c1",
                "write_file",
                json!({"file_path": "note.txt", "content": format!("to {}", f.placeholder)}),
            ),
            answer("chat", &["done"]),
        ],
    )
    .await;
    f.agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();
    // The tool itself ran: it began its checkpoint, the derived write went
    // to disk inside it, it finished the checkpoint and then checked the
    // file as it is on disk.
    assert_eq!(
        *f.around.steps.lock().unwrap(),
        ["began note.txt", "finished note.txt", "checked note.txt"]
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("note.txt")).unwrap(),
        format!("to {EMAIL}")
    );
    // The recorded result is what the tool returned, with what its checker
    // quoted from the file.
    assert_eq!(
        result_at(&f.agent, 2),
        (
            format!("Updated file note.txt\n\nDiagnostics:\nline 1: unknown word in 'to {EMAIL}'"),
            false
        )
    );
    // The model is told the same with the placeholder, and no request holds
    // the value.
    let told = bodies(&server).await[1]["messages"][3]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        told,
        format!(
            "Updated file note.txt\n\nDiagnostics:\nline 1: unknown word in 'to {}'",
            f.placeholder
        )
    );
    nothing_original_sent(&server).await;
}

#[tokio::test]
async fn arguments_a_person_edited_run_as_that_person_wrote_them() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    for with_alias in [false, true] {
        let server = MockServer::start().await;
        let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
        // An alias this conversation issued for a telephone number. It has
        // no marker.
        let number = f
            .session
            .admit_raw(&draft(&format!("who handles {PHONE}?")))
            .await
            .unwrap();
        let alias = issued(&number);
        assert!(!alias.contains("pv1-"));
        let edited = match with_alias {
            true => format!("call {alias}"),
            false => "to the person".to_owned(),
        };
        *f.reviewer.decide.lock().unwrap() = Decide::Park;
        scripted(
            &server,
            vec![
                file_call(
                    "c1",
                    "write_file",
                    json!({"file_path": "note.txt", "content": format!("to {}", f.placeholder)}),
                ),
                answer("chat", &["done"]),
            ],
        )
        .await;
        f.agent
            .run_turn_with_view(f.typed.clone(), f.source.clone())
            .await
            .unwrap();
        let reviewed = ReviewedFile::of_parked(&f.agent.actual_parked_call().unwrap()).unwrap();
        // The person writes other arguments instead.
        let action = HitlAction::ApproveEdited {
            args: json!({"file_path": "note.txt", "content": edited}),
        };
        let note = f.dir.path().join("note.txt");
        if with_alias {
            // A person's text that holds an alias of this conversation has
            // no model view, with a marker or without one. The conversation
            // stops when the call is resumed, before the file tool runs,
            // and the alias is never written to a file as if it were a
            // telephone number.
            let decision = fixture_human_input(&f.session, &f.agent, &action);
            let resumed = f.agent.resume_with_view(action, "Denied.", decision).await;
            assert!(resumed.is_err());
            assert!(f.session.check().is_err());
            assert!(!note.exists());
            assert!(f.around.steps.lock().unwrap().is_empty());
            assert!(matches!(
                reviewed.shown_arguments(),
                Err(FileRefusal::Stopped(_))
            ));
            continue;
        }
        // What was derived from the model's call is over, nothing is
        // restored into the person's text, and it is written as it is by
        // the file tool's own write.
        let decision = fixture_human_input(&f.session, &f.agent, &action);
        f.agent
            .resume_with_view(action, "Denied.", decision)
            .await
            .unwrap();
        assert!(!reviewed.is_done());
        assert_eq!(reviewed.shown_arguments().err(), Some(FileRefusal::Spent));
        assert_eq!(std::fs::read_to_string(&note).unwrap(), edited);
        assert_eq!(
            result_at(&f.agent, 2),
            ("Updated file note.txt".to_owned(), false)
        );
        assert_eq!(
            *f.around.steps.lock().unwrap(),
            ["began note.txt", "finished note.txt"]
        );
        nothing_original_sent(&server).await;
    }
}

use craftsman_harness::SubAgentSpec;
use craftsman_harness::builtin::SubAgentMiddleware;

/// What a delegated agent's gate was shown, and what it answers.
#[derive(Default)]
struct GateSeat {
    shown: Mutex<Vec<String>>,
    deny: std::sync::atomic::AtomicBool,
}

struct DelegatedFiles {
    f: Files,
    seat: Arc<GateSeat>,
    /// The alias the conversation issued for the telephone number.
    number: String,
}

/// One conversation whose root agent delegates to `helper`, which has the
/// two file tools behind its own gate. `channel` says whether that gate can
/// ask a person.
async fn delegated_files(
    vault: &Vault,
    root: &MockServer,
    side: &MockServer,
    rule: Option<FileDataFormat>,
    channel: bool,
    approval: crate::hitl::ApprovalState,
) -> DelegatedFiles {
    let seat = Arc::new(GateSeat::default());
    let asked = seat.clone();
    let prompt: crate::agent::ApprovalPrompt = Arc::new(move |_, _, details| {
        asked.shown.lock().unwrap().push(details.to_owned());
        match asked.deny.load(std::sync::atomic::Ordering::SeqCst) {
            true => crate::agent::ApprovalPromptResult::Reject("not this one".into()),
            false => crate::agent::ApprovalPromptResult::Approve,
        }
    });
    let helper = Helper {
        side,
        prompt: channel.then_some(prompt),
        approval,
        private: None,
    };
    let f = set_up(vault, root, rule, Arc::new(|_: &str| {}), Some(helper)).await;
    let number = f
        .session
        .admit_raw(&draft(&format!("who handles {PHONE}?")))
        .await
        .unwrap();
    DelegatedFiles {
        number: issued(&number),
        seat,
        f,
    }
}

/// The root agent asks `helper` for a note, and `helper` asks to write
/// `content` to `note.txt`.
async fn delegates_a_note(root: &MockServer, side: &MockServer, content: &str) {
    scripted(
        root,
        vec![
            file_call(
                "t1",
                "task",
                json!({"description": "write the note", "subagent_type": "helper"}),
            ),
            answer("chat", &["done"]),
        ],
    )
    .await;
    scripted(
        side,
        vec![
            file_call(
                "w1",
                "write_file",
                json!({"file_path": "note.txt", "content": content}),
            ),
            answer("chat", &["saved"]),
        ],
    )
    .await;
}

/// What the delegated agent's model was told about its write.
async fn told_the_helper(side: &MockServer) -> String {
    bodies(side).await[1]["messages"][3]["content"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A synthetic actual receiver/modal/choice adapter for local source tests.
/// Its production counterpart is A's real TUI owner, not an ordinary callback.
fn bound_test_ui(
    read: crate::agent::ApprovalPrompt,
) -> Arc<crate::privacy_entry::PrivacyApprovalPrompt> {
    bound_test_choice_ui(Arc::new(move |review| {
        match read(&review.call.name, review.summary, review.details) {
            crate::agent::ApprovalPromptResult::Approve => {
                crate::privacy_entry::HumanChoice::Approve
            }
            crate::agent::ApprovalPromptResult::ApproveAlways => {
                crate::privacy_entry::HumanChoice::ApproveAlways
            }
            crate::agent::ApprovalPromptResult::Reject(raw) => {
                crate::privacy_entry::HumanChoice::Deny(raw)
            }
        }
    }))
}

fn bound_test_choice_ui(
    read: Arc<
        dyn for<'a> Fn(
                crate::privacy_entry::PrivacyApprovalView<'a>,
            ) -> crate::privacy_entry::HumanChoice
            + Send
            + Sync,
    >,
) -> Arc<crate::privacy_entry::PrivacyApprovalPrompt> {
    use crate::privacy_entry::{
        PrivacyApprovalDecision, PrivacyApprovalPresentation, PrivacyApprovalRequest,
        PrivacyApprovalUi, PrivacyApprovalView, register_privacy_approval_ui,
    };
    let owner = Arc::new(Mutex::new(None::<Arc<PrivacyApprovalUi>>));
    let receiver = owner.clone();
    let (prompt, ui) =
        register_privacy_approval_ui(Arc::new(move |request: PrivacyApprovalRequest| {
            let ui = receiver.lock().unwrap().clone()?;
            let presentation: PrivacyApprovalPresentation = ui.present(&request).ok()?;
            let review: PrivacyApprovalView<'_> = presentation.review();
            let choice = read(review);
            let decision: PrivacyApprovalDecision = ui.decide(presentation, choice).ok()?;
            Some(decision)
        }));
    *owner.lock().unwrap() = Some(ui);
    prompt
}

#[tokio::test]
async fn a_delegated_agents_write_is_derived_in_its_own_lane_and_decided_by_its_own_gate() {
    let dir = tempfile::tempdir().unwrap();
    let vault = vault(&dir).await;

    // Approved by the person at the delegated agent's gate.
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegated_files(
        &vault,
        &root,
        &side,
        Some(FileDataFormat::PlainText),
        true,
        Default::default(),
    )
    .await;
    let (p, n) = (d.f.placeholder.clone(), d.number.clone());
    assert!(!n.contains("pv1-"));
    delegates_a_note(&root, &side, &format!("Contact {p} or call {n}.")).await;
    d.f.agent
        .run_turn_with_view(d.f.typed.clone(), d.f.source.clone())
        .await
        .unwrap();
    let note = d.f.dir.path().join("note.txt");
    assert_eq!(
        std::fs::read_to_string(&note).unwrap(),
        format!("Contact {EMAIL} or call {PHONE}.")
    );
    // The gate asked once, about the content that was written. It was shown
    // no placeholder, with a marker or without one.
    let shown = d.seat.shown.lock().unwrap().clone();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].contains(&format!("Contact {EMAIL} or call {PHONE}.")));
    assert!(!shown[0].contains(&p) && !shown[0].contains(&n));
    // The agent's own file tool ran around the write.
    assert_eq!(
        *d.f.around.steps.lock().unwrap(),
        ["began note.txt", "finished note.txt"]
    );
    // The delegated agent's call stays as its model wrote it, and its model
    // is told the tool's own words.
    assert_eq!(told_the_helper(&side).await, "Updated file note.txt");
    let call = bodies(&side).await[1]["messages"][2]["tool_calls"][0]["function"]["arguments"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(call.contains(&p) && call.contains(&n));
    for server in [&root, &side] {
        nothing_original_sent(server).await;
    }

    // Refused by the person: nothing is written, and the file tool never
    // ran.
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegated_files(
        &vault,
        &root,
        &side,
        Some(FileDataFormat::PlainText),
        true,
        Default::default(),
    )
    .await;
    d.seat.deny.store(true, std::sync::atomic::Ordering::SeqCst);
    delegates_a_note(&root, &side, &format!("Contact {}.", d.f.placeholder)).await;
    d.f.agent
        .run_turn_with_view(d.f.typed.clone(), d.f.source.clone())
        .await
        .unwrap();
    assert!(!d.f.dir.path().join("note.txt").exists());
    assert_eq!(d.seat.shown.lock().unwrap().len(), 1);
    assert!(d.f.around.steps.lock().unwrap().is_empty());
    assert!(told_the_helper(&side).await.contains("not this one"));

    // A gate that cannot ask anyone refuses, as for any delegated write.
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegated_files(
        &vault,
        &root,
        &side,
        Some(FileDataFormat::PlainText),
        false,
        Default::default(),
    )
    .await;
    delegates_a_note(&root, &side, &format!("Contact {}.", d.f.placeholder)).await;
    d.f.agent
        .run_turn_with_view(d.f.typed.clone(), d.f.source.clone())
        .await
        .unwrap();
    assert!(!d.f.dir.path().join("note.txt").exists());
    assert!(d.f.around.steps.lock().unwrap().is_empty());
    assert!(
        told_the_helper(&side)
            .await
            .contains("requires user approval")
    );

    // No rule of the user's policy for the file: refused before the gate is
    // asked. The call names only the telephone alias, which has no marker;
    // it is not written to the file as it is.
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegated_files(&vault, &root, &side, None, true, Default::default()).await;
    delegates_a_note(&root, &side, &format!("Call {}.", d.number)).await;
    d.f.agent
        .run_turn_with_view(d.f.typed.clone(), d.f.source.clone())
        .await
        .unwrap();
    assert!(!d.f.dir.path().join("note.txt").exists());
    assert!(d.seat.shown.lock().unwrap().is_empty());
    assert!(d.f.around.steps.lock().unwrap().is_empty());
    assert_eq!(
        told_the_helper(&side).await,
        FileRefusal::NoRule.to_string()
    );

    // The gate's automatic policy allows file changes: nobody is asked, the
    // policy's decision on the real content is carried in the delegated
    // lane, and the file holds the values. In plan mode the same policy
    // denies, and nothing is written.
    for (mode, written) in [
        (crate::config::PermissionMode::Manual, true),
        (crate::config::PermissionMode::Plan, false),
    ] {
        let (root, side) = (MockServer::start().await, MockServer::start().await);
        let state = crate::hitl::ApprovalState {
            mode,
            auto_approve: true,
            ..Default::default()
        };
        let d = delegated_files(
            &vault,
            &root,
            &side,
            Some(FileDataFormat::PlainText),
            true,
            state,
        )
        .await;
        delegates_a_note(&root, &side, &format!("Call {}.", d.number)).await;
        d.f.agent
            .run_turn_with_view(d.f.typed.clone(), d.f.source.clone())
            .await
            .unwrap();
        let note = d.f.dir.path().join("note.txt");
        assert!(d.seat.shown.lock().unwrap().is_empty());
        match written {
            true => {
                assert_eq!(
                    std::fs::read_to_string(&note).unwrap(),
                    format!("Call {PHONE}.")
                );
                assert_eq!(told_the_helper(&side).await, "Updated file note.txt");
            }
            false => {
                assert!(!note.exists());
                assert!(d.f.around.steps.lock().unwrap().is_empty());
            }
        }
        for server in [&root, &side] {
            nothing_original_sent(server).await;
        }
    }

    // A call that names no placeholder is the delegated agent's ordinary
    // write: asked about and written as the model wrote it.
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegated_files(&vault, &root, &side, None, true, Default::default()).await;
    delegates_a_note(&root, &side, "Nothing private.").await;
    d.f.agent
        .run_turn_with_view(d.f.typed.clone(), d.f.source.clone())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(d.f.dir.path().join("note.txt")).unwrap(),
        "Nothing private."
    );
    assert_eq!(d.seat.shown.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_decision_of_another_operation_approves_nothing_whatever_the_calls_id() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
    let p = f.placeholder.clone();
    let writes = |path: &str| {
        file_call(
            "c1",
            "write_file",
            json!({"file_path": path, "content": format!("to {p}")}),
        )
    };
    scripted(
        &server,
        vec![
            writes("first.txt"),
            answer("chat", &["done"]),
            // The second call carries the id of the first.
            writes("second.txt"),
            answer("chat", &["done"]),
        ],
    )
    .await;
    f.agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("first.txt")).unwrap(),
        format!("to {EMAIL}")
    );
    // The decision the first operation issued, which its terminal carried
    // and the file tool used.
    let first = last_invocation(&f.agent);
    let used = first.decision_source().cloned().unwrap();

    // In the next turn nothing is decided on the new operation. The agent is
    // handed the first operation's decision instead and carries it to the
    // terminal of the second call.
    *f.reviewer.another.lock().unwrap() = Some(used.clone());
    *f.reviewer.decide.lock().unwrap() = Decide::CarryAnother;
    let again = f.session.admit_raw(&draft("and once more?")).await.unwrap();
    f.agent
        .run_turn_with_view("and once more?", f.session.source(&again).unwrap())
        .await
        .unwrap();
    let second = last_invocation(&f.agent);
    assert!(
        second
            .decision_source()
            .is_some_and(|carried| carried.is(&used))
    );
    // It is a different plan of the same agent for a call with the same id,
    // with its own operation and its own digest.
    assert_eq!(second.call().id, first.call().id);
    assert!(!second.was_planned_as(first.planned()));
    assert_ne!(second.shown(), first.shown());
    // The file tool refused: that decision is not one of the operation that
    // was derived for this plan. Nothing was written.
    let results: Vec<(String, bool)> = recorded(&f.agent)
        .iter()
        .filter_map(|message| match message {
            Msg::Tool {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        [
            ("Updated file first.txt".to_owned(), false),
            (FileRefusal::NotApproved.to_string(), true)
        ]
    );
    assert!(!f.dir.path().join("second.txt").exists());
    assert!(f.session.check().is_ok());
    nothing_original_sent(&server).await;
}

#[tokio::test]
async fn a_gates_automatic_policy_decides_on_what_will_be_written_and_is_checked_at_the_file() {
    use crate::config::PermissionMode;
    use crate::hitl::ApprovalState;
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let auto = ApprovalState {
        auto_approve: true,
        ..Default::default()
    };
    let plan = ApprovalState {
        mode: PermissionMode::Plan,
        auto_approve: true,
        ..Default::default()
    };
    for (state, written) in [
        (auto, true),
        (plan, false),
        (ApprovalState::default(), false),
    ] {
        let server = MockServer::start().await;
        let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
        *f.reviewer.approval.lock().unwrap() = state;
        *f.reviewer.decide.lock().unwrap() = Decide::Policy;
        scripted(
            &server,
            vec![
                file_call(
                    "c1",
                    "write_file",
                    json!({"file_path": "note.txt", "content": format!("to {}", f.placeholder)}),
                ),
                answer("chat", &["done"]),
            ],
        )
        .await;
        f.agent
            .run_turn_with_view(f.typed.clone(), f.source.clone())
            .await
            .unwrap();
        let note = f.dir.path().join("note.txt");
        let (text, is_error) = result_at(&f.agent, 2);
        match written {
            // The policy allowed what will really be written. Its decision
            // was recorded where it was taken, carried to the terminal and
            // accepted by the file tool.
            true => {
                assert_eq!(
                    std::fs::read_to_string(&note).unwrap(),
                    format!("to {EMAIL}")
                );
                assert_eq!((text.as_str(), is_error), ("Updated file note.txt", false));
                assert!(last_invocation(&f.agent).decision_source().is_some());
            }
            // The policy denied, or asks a person and nobody was asked: the
            // gate's own answer, no decision, no file.
            false => {
                assert!(is_error && !note.exists(), "{text}");
                assert!(f.agent.actual_terminal_outcomes().unwrap().is_empty());
            }
        }
        assert!(f.around.steps.lock().unwrap().is_empty() != written);
        assert!(f.session.check().is_ok());
        nothing_original_sent(&server).await;
    }
}

#[tokio::test]
async fn nothing_but_the_session_keeps_the_users_policy_and_without_it_there_is_no_rule() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let mut f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
    // The fixture stands where a session stands: it is the policy's only
    // owner. The review, the boundary and the file tools of the agent that
    // was built refer to it and keep nothing of it alive.
    let policy = f.policy.take().unwrap();
    assert_eq!(Arc::strong_count(&policy), 1);
    drop(policy);
    scripted(
        &server,
        vec![
            file_call(
                "c1",
                "write_file",
                json!({"file_path": "note.txt", "content": format!("to {}", f.placeholder)}),
            ),
            answer("chat", &["done"]),
        ],
    )
    .await;
    f.agent
        .run_turn_with_view(f.typed.clone(), f.source.clone())
        .await
        .unwrap();
    // Without the policy there is no rule: the write is refused before
    // anyone is asked, and the placeholder is not written as it is.
    assert_eq!(
        result_at(&f.agent, 2),
        (FileRefusal::NoRule.to_string(), true)
    );
    assert_eq!(
        f.reviewer.refusals.lock().unwrap().first(),
        Some(&FileRefusal::NoRule)
    );
    assert!(f.reviewer.shown.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_dir(f.dir.path()).unwrap().count(), 0);
    assert!(f.session.check().is_ok());
    nothing_original_sent(&server).await;
}

#[tokio::test]
async fn an_absent_or_unparsable_answer_writes_nothing_and_keeps_the_call_as_it_is() {
    use crate::privacy_entry::{HumanChoice, HumanDecisionProducer};
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    for unparsable in [false, true] {
        let server = MockServer::start().await;
        let f = files(&vault, &server, Some(FileDataFormat::PlainText)).await;
        *f.reviewer.decide.lock().unwrap() = Decide::Park;
        let arguments =
            json!({"file_path": "note.txt", "content": format!("to {}", f.placeholder)});
        scripted(
            &server,
            vec![
                file_call("c1", "write_file", arguments.clone()),
                answer("chat", &["done"]),
            ],
        )
        .await;
        f.agent
            .run_turn_with_view(f.typed.clone(), f.source.clone())
            .await
            .unwrap();
        // The person is shown the operation and gives no answer, or types
        // arguments that are not arguments.
        let choice = match unparsable {
            true => HumanChoice::Edit("{\"file_path\": ".into()),
            false => HumanChoice::Absent,
        };
        let producer = HumanDecisionProducer::register(
            f.session.clone(),
            Arc::new(move |parked, shown| {
                assert!(call_to_show(parked, shown).is_some());
                choice.clone()
            }),
        );
        let parked = f.agent.actual_parked_call().unwrap();
        let received = producer.receive(&parked);
        match unparsable {
            // No answer is no decision: there is nothing to resume with,
            // the call stays parked and the conversation goes on.
            false => {
                let input = received.unwrap();
                assert!(input.was_absent() && input.action().is_none());
                assert!(f.agent.actual_parked_call().is_some());
                assert!(f.session.check().is_ok());
            }
            // What was typed is kept as it was typed, and the conversation
            // stops there.
            true => {
                assert!(received.is_err());
                assert!(f.session.check().is_err());
            }
        }
        // Nothing ran and nothing was written. The call is recorded as the
        // model wrote it and has no result.
        assert_eq!(std::fs::read_dir(f.dir.path()).unwrap().count(), 0);
        assert!(f.around.steps.lock().unwrap().is_empty());
        assert!(f.agent.actual_terminal_outcomes().unwrap().is_empty());
        let messages = recorded(&f.agent);
        assert_eq!(messages.len(), 2);
        assert!(matches!(&messages[1], Msg::Assistant { tool_calls, .. }
            if tool_calls[0].args == arguments));
        nothing_original_sent(&server).await;
    }
}

#[tokio::test]
async fn registered_main_gate_keeps_actual_choice_policy_effect_result_and_save() {
    use craftsman_harness::testing::MemCheckpointer;
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    // Actual approval, Always, denial, automatic permission, and legacy-only
    // refusal. No callback/budget fault or replay scenario is injected.
    for case in 0..6 {
        let server = MockServer::start().await;
        let state = Arc::new(Mutex::new(crate::hitl::ApprovalState {
            auto_approve: case == 3,
            ..Default::default()
        }));
        let read_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = read_count.clone();
        let policy = state.clone();
        let callback: crate::agent::ApprovalPrompt = Arc::new(move |_, _, details| {
            assert!(policy.try_lock().is_ok());
            assert!(details.contains(EMAIL));
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match case {
                1 => crate::agent::ApprovalPromptResult::ApproveAlways,
                2 => crate::agent::ApprovalPromptResult::Reject("actual person declined".into()),
                _ => crate::agent::ApprovalPromptResult::Approve,
            }
        });
        let mut absent_owner = None;
        let private = if case == 5 {
            let (prompt, ui) =
                crate::privacy_entry::register_privacy_approval_ui(Arc::new(|_| None));
            absent_owner = Some(ui);
            Some(prompt)
        } else {
            (case < 3).then(|| bound_test_ui(callback.clone()))
        };
        let store = Arc::new(MemCheckpointer::default());
        let f = set_up_with_main_gate(
            &vault,
            &server,
            Some(FileDataFormat::PlainText),
            Arc::new(|_| {}),
            None,
            Some(MainGate {
                approval: state.clone(),
                legacy: Some(callback),
                private,
                store: store.clone(),
            }),
        )
        .await;
        let mut script = vec![file_call(
            "c1",
            "write_file",
            json!({
                "file_path": "note.txt", "content": format!("Contact {}.", f.placeholder)
            }),
        )];
        if case == 1 {
            script.push(file_call(
                "c2",
                "write_file",
                json!({
                    "file_path": "second.txt", "content": format!("Contact {}.", f.placeholder)
                }),
            ));
        }
        script.push(answer("chat", &["done"]));
        scripted(&server, script).await;
        f.agent
            .run_turn_with_view(f.typed.clone(), f.source.clone())
            .await
            .unwrap();
        let writes = matches!(case, 0 | 1 | 3);
        if writes {
            assert_eq!(
                std::fs::read_to_string(f.dir.path().join("note.txt")).unwrap(),
                format!("Contact {EMAIL}.")
            );
            let invocation = last_invocation(&f.agent);
            let input = invocation
                .decision_source()
                .unwrap()
                .downcast::<crate::privacy_entry::InlineDecisionInput>()
                .unwrap();
            assert!(input.registered_session().is_some() && !input.edited());
            assert_eq!(input.action(), Some(HitlAction::Approve));
        } else {
            assert!(!f.dir.path().join("note.txt").exists());
            assert!(f.agent.actual_terminal_outcomes().unwrap().is_empty());
            assert!(result_at(&f.agent, 2).1);
        }
        assert_eq!(
            read_count.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(case < 3)
        );
        assert_eq!(state.lock().unwrap().auto_approve, matches!(case, 1 | 3));
        if case == 1 {
            assert_eq!(
                std::fs::read_to_string(f.dir.path().join("second.txt")).unwrap(),
                format!("Contact {EMAIL}.")
            );
        }
        if case == 4 {
            assert!(result_at(&f.agent, 2).0.contains("registered private UI"));
        }
        if case == 5 {
            assert!(absent_owner.is_some());
            assert!(
                result_at(&f.agent, 2)
                    .0
                    .contains("no actual answered approval")
            );
        }
        let state = f.agent.state.lock().unwrap().clone();
        assert_eq!(
            store.saves.lock().unwrap().last().unwrap().2.messages,
            state.messages
        );
        assert!(
            state
                .messages
                .iter()
                .any(|msg| matches!(msg, Msg::Assistant { tool_calls, .. }
            if tool_calls.iter().any(|call| call.args.to_string().contains(&f.placeholder))))
        );
        assert!(f.session.check().is_ok());
        nothing_original_sent(&server).await;
    }
}

#[tokio::test]
async fn registered_inline_raw_edits_keep_human_origin_on_main_and_delegated_calls() {
    use crate::privacy_entry::{HumanChoice, InlineDecisionInput};
    use craftsman_harness::testing::MemCheckpointer;
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    for delegated in [false, true] {
        for equal_json in [false, true] {
            let root_server = MockServer::start().await;
            let side = MockServer::start().await;
            let raw = if equal_json {
                json!({"file_path":"note.txt","content":"ordinary same bytes"}).to_string()
            } else {
                format!(" {{ \"file_path\": \"note.txt\", \"content\": \"Typed {EMAIL}.\" }} ")
            };
            let entered = raw.clone();
            let private =
                bound_test_choice_ui(Arc::new(move |_| HumanChoice::Edit(entered.clone())));
            let store = Arc::new(MemCheckpointer::default());
            let f = if delegated {
                set_up_with_main_gate(
                    &vault,
                    &root_server,
                    Some(FileDataFormat::PlainText),
                    Arc::new(|_| {}),
                    Some(Helper {
                        side: &side,
                        prompt: None,
                        approval: Default::default(),
                        private: Some(private),
                    }),
                    None,
                )
                .await
            } else {
                set_up_with_main_gate(
                    &vault,
                    &root_server,
                    Some(FileDataFormat::PlainText),
                    Arc::new(|_| {}),
                    None,
                    Some(MainGate {
                        approval: Arc::new(Mutex::new(Default::default())),
                        legacy: None,
                        private: Some(private),
                        store: store.clone(),
                    }),
                )
                .await
            };
            let original = json!({"file_path":"note.txt","content": if equal_json {
                "ordinary same bytes".to_owned()
            } else { format!("Originally {}.", f.placeholder) }});
            if delegated {
                delegates_a_note(&root_server, &side, original["content"].as_str().unwrap()).await;
            } else {
                scripted(
                    &root_server,
                    vec![
                        file_call("c1", "write_file", original.clone()),
                        answer("chat", &["done"]),
                    ],
                )
                .await;
            }
            f.agent
                .run_turn_with_view(f.typed.clone(), f.source.clone())
                .await
                .unwrap();
            assert_eq!(
                std::fs::read_to_string(f.dir.path().join("note.txt")).unwrap(),
                if equal_json {
                    "ordinary same bytes".to_owned()
                } else {
                    format!("Typed {EMAIL}.")
                }
            );
            assert_eq!(
                *f.around.steps.lock().unwrap(),
                ["began note.txt", "finished note.txt"]
            );
            let all = if delegated {
                let returned = f.agent.actual_task_returns().unwrap();
                assert_eq!(returned.len(), 1);
                assert!(returned[0].failure().is_none());
                returned[0].terminal_outcomes().to_vec()
            } else {
                f.agent.actual_terminal_outcomes().unwrap()
            };
            let actual = all
                .iter()
                .find(|event| {
                    event.invocation().call().name == "write_file"
                        && (!delegated || event.invocation().parent().is_some())
                })
                .unwrap();
            let invocation = actual.invocation();
            assert!(invocation.human_edited());
            let input = invocation
                .decision_source()
                .unwrap()
                .downcast::<InlineDecisionInput>()
                .unwrap();
            assert_eq!(input.raw_edit(), Some(raw.as_str()));
            assert!(input.edited());
            assert_eq!(
                invocation.call().args,
                serde_json::from_str::<Value>(&raw).unwrap()
            );
            assert_eq!(invocation.planned().call().args, original);
            if equal_json {
                assert_eq!(invocation.call().args, invocation.planned().call().args);
            }
            if !delegated {
                assert_eq!(
                    store.saves.lock().unwrap().last().unwrap().2.messages,
                    recorded(&f.agent)
                );
            }
            nothing_original_sent(&root_server).await;
            if delegated {
                nothing_original_sent(&side).await;
            }
        }
    }
}
