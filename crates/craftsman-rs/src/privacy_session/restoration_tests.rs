//! What a person reads and what reaches a file in a privacy conversation:
//! the human view of a committed reply, the hold on a streamed reply, and
//! file writes and edits derived, reviewed and run with the real values.
//! The vault, the guard and the Chat provider are real, the server is a
//! loopback one, and the files are real temporary files.

use super::*;
use crate::privacy_restore::{FileDataFormat, RestoreError};

/// A user's rule for file content: one data format for every file, or none.
struct Rule(Option<FileDataFormat>);

impl CapturedUserPolicy for Rule {
    fn enables_privacy_in(&self, _: &str) -> bool {
        true
    }

    fn requires_inspectable_provider_items(&self) -> bool {
        false
    }

    fn restorable_file_data(&self, _: &str, _: &str) -> Option<FileDataFormat> {
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

/// What the reviewer does with a derived operation.
#[derive(Clone, Copy, PartialEq)]
enum Decide {
    Approve,
    ApproveSomethingElse,
    Deny,
    Park,
    /// Approve, then change the destination before the tool runs.
    ApproveThenDisturb,
}

/// What the reviewer was shown.
#[derive(Default, Clone, PartialEq, Debug)]
struct Shown {
    create: bool,
    old: Option<String>,
    new: String,
    arguments: Value,
}

/// The review before a file tool runs: derive, look, decide.
struct Reviewer {
    review: Arc<FileReview>,
    decide: Mutex<Decide>,
    shown: Mutex<Vec<Shown>>,
    refusals: Mutex<Vec<FileRefusal>>,
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
        let derived = match self.review.propose(&call).await {
            Ok(derived) => derived,
            Err(refusal) => {
                self.refusals.lock().unwrap().push(refusal.clone());
                return Ok(ToolOutcome::err(refusal.to_string()));
            }
        };
        let Some(derived) = derived else {
            return next.run(call, ctx).await;
        };
        let digest = derived
            .preview(|preview| {
                self.shown.lock().unwrap().push(Shown {
                    create: preview.change == FileChange::Create,
                    old: preview.old.map(str::to_owned),
                    new: preview.new.to_owned(),
                    arguments: preview.arguments.clone(),
                });
                preview.digest
            })
            .unwrap();
        let decide = *self.decide.lock().unwrap();
        match decide {
            Decide::Approve => derived.approve(digest).unwrap(),
            Decide::ApproveSomethingElse => {
                assert_eq!(derived.approve([7; 32]), Err(FileRefusal::NotApproved));
            }
            Decide::Deny => {
                derived.refuse();
                return Ok(ToolOutcome::err("Denied by the reviewer."));
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
            Decide::ApproveThenDisturb => {
                derived.approve(digest).unwrap();
                let path = call.args["file_path"].as_str().unwrap();
                std::fs::write(self.dir.join(path), "changed by someone else").unwrap();
            }
        }
        next.run(call, ctx).await
    }
}

struct Files {
    session: Arc<PrivacySession>,
    agent: Arc<Agent>,
    reviewer: Arc<Reviewer>,
    review: Arc<FileReview>,
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
/// and what its gate can ask.
struct Helper<'a> {
    side: &'a MockServer,
    prompt: Option<crate::agent::ApprovalPrompt>,
}

async fn set_up(
    vault: &Vault,
    server: &MockServer,
    rule: Option<FileDataFormat>,
    sink: crate::agent::TokenSink,
    helper: Option<Helper<'_>>,
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
    let review = FileReview::new(
        session.clone(),
        boundary.clone(),
        Arc::new(Rule(rule)),
        dir.path().to_path_buf(),
    );
    let reviewer = Arc::new(Reviewer {
        review: review.clone(),
        decide: Mutex::new(Decide::Approve),
        shown: Mutex::default(),
        refusals: Mutex::default(),
        dir: dir.path().to_path_buf(),
    });
    let mut builder = Agent::builder(Arc::new(provider))
        .model_view_boundary(Some(boundary.clone()))
        .middleware(reviewer.clone());
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
        // gives them to one: derived before the agent's own gate and
        // recorded behind it.
        let delegated = delegated_provider(
            client(helper.side),
            session.clone(),
            Arc::new(Mutex::new((0, 0))),
            Arc::new(|_: &str| {}),
        )
        .await
        .unwrap();
        let spec = SubAgentSpec {
            name: "helper".into(),
            description: "Writes notes.".into(),
            system_prompt: "You write notes.".into(),
            tools: crate::agent::gated_for_delegation(
                plain.clone(),
                &Arc::new(Mutex::new(crate::hitl::ApprovalState::default())),
                (&helper.prompt, &None),
                dir.path(),
                Some(&review),
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
        review,
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
    // The operation ran once and is gone.
    assert!(f.review.waiting("c1").is_none());
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
        // Derived and shown, then a decision about something else.
        (
            Some(FileDataFormat::PlainText),
            Decide::ApproveSomethingElse,
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
    // The call is parked. What it will write is kept for the person to
    // look at, and nothing is on disk.
    let waiting = f.review.waiting("c1").expect("the derived operation waits");
    let digest = waiting
        .preview(|preview| {
            assert_eq!(preview.new, format!("to {EMAIL}"));
            preview.digest
        })
        .unwrap();
    assert!(!f.dir.path().join("note.txt").exists());

    // Resumed without the decision being bound to what was shown: the tool
    // refuses, and the operation is spent.
    let decision = fixture_human_input(&f.session, &f.agent, &HitlAction::Approve);
    f.agent
        .resume_with_view(HitlAction::Approve, "Denied.", decision)
        .await
        .unwrap();
    assert_eq!(
        result_at(&f.agent, 2),
        (FileRefusal::NotApproved.to_string(), true)
    );
    assert!(!f.dir.path().join("note.txt").exists());
    assert_eq!(waiting.approve(digest), Err(FileRefusal::NotApproved));
    assert_eq!(waiting.execute(None), Err(FileRefusal::NotApproved));

    // The same again, with the decision bound: the resumed call writes the
    // real content.
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
    let waiting = f.review.waiting("c1").unwrap();
    let digest = waiting.preview(|preview| preview.digest).unwrap();
    waiting.approve(digest).unwrap();
    let decision = fixture_human_input(&f.session, &f.agent, &HitlAction::Approve);
    f.agent
        .resume_with_view(HitlAction::Approve, "Denied.", decision)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("note.txt")).unwrap(),
        format!("to {EMAIL}")
    );
    assert_eq!(
        result_at(&f.agent, 2),
        ("Updated file note.txt".to_owned(), false)
    );
    // It ran once.
    assert_eq!(waiting.execute(None), Err(FileRefusal::Spent));
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
        let waiting = f.review.waiting("c1").unwrap();
        let digest = waiting.preview(|preview| preview.digest).unwrap();
        waiting.approve(digest).unwrap();
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
                waiting.execute(None),
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
        assert_eq!(waiting.execute(None), Err(FileRefusal::NotApproved));
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

#[tokio::test]
async fn a_delegated_agents_write_is_derived_in_its_own_lane_and_decided_by_its_own_gate() {
    let dir = tempfile::tempdir().unwrap();
    let vault = vault(&dir).await;

    // Approved by the person at the delegated agent's gate.
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegated_files(&vault, &root, &side, Some(FileDataFormat::PlainText), true).await;
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
    // The root agent's review holds nothing of it.
    assert!(d.f.review.waiting("w1").is_none());
    for server in [&root, &side] {
        nothing_original_sent(server).await;
    }

    // Refused by the person: nothing is written, and the file tool never
    // ran.
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegated_files(&vault, &root, &side, Some(FileDataFormat::PlainText), true).await;
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
    let d = delegated_files(&vault, &root, &side, Some(FileDataFormat::PlainText), false).await;
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
    let d = delegated_files(&vault, &root, &side, None, true).await;
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

    // A call that names no placeholder is the delegated agent's ordinary
    // write: asked about and written as the model wrote it.
    let (root, side) = (MockServer::start().await, MockServer::start().await);
    let d = delegated_files(&vault, &root, &side, None, true).await;
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
