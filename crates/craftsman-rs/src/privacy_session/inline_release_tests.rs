//! What a gate's inline decision keeps and what it lets go of. The gate, the
//! registered UI, the vault, the guard, the Chat provider and the file tools
//! are the real ones, the server is a loopback one and the files are real
//! temporary files. Every case is an ordinary approval: a person answers,
//! does not answer or stops the turn, or the gate's own policy decides.

use super::*;
use crate::hitl::{ApprovalState, ToolPermission};
use crate::privacy_entry::{
    HumanChoice, InlineDecisionInput, InlineDecisionProducer, PrivacyApprovalDecision,
    PrivacyApprovalPrompt, PrivacyApprovalRequest, PrivacyApprovalUi, register_privacy_approval_ui,
};
use craftsman_harness::testing::MemCheckpointer;
use std::sync::Weak;
use std::sync::atomic::{AtomicBool, Ordering};

const BEFORE: &str = "Contact old text";

#[tokio::test]
async fn actual_absent_delegated_channel_releases_only_its_derived_review_and_keeps_the_refusal() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let ground = ground(&vault, &server).await;
    let watched = Arc::new(Watch::default());
    let around = Arc::new(Around::default());
    let store = Arc::new(MemCheckpointer::default());
    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    let plain: Vec<Arc<dyn Tool>> = vec![Arc::new(PlainFile(
        "edit_file",
        ground.dir.path().to_path_buf(),
        around.clone(),
    ))];
    let tools = crate::agent::gated_for_delegation_with_privacy(
        plain,
        &approval,
        (&None, &None),
        ground.dir.path(),
        Some(&ground.review),
        Some(&ground.boundary),
        None,
    );
    let mut builder = Agent::builder(ground.provider.clone())
        .model_view_boundary(Some(ground.boundary.clone()))
        .middleware(watched.clone())
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true);
    for tool in tools {
        builder = builder.tool(tool);
    }
    let agent = Arc::new(builder.build());
    let typed = format!("who handles {EMAIL}?");
    let node = ground.session.admit_raw(&draft(&typed)).await.unwrap();
    let placeholder = issued(&node);
    scripted(&server, vec![file_call("no-channel", "edit_file", json!({
        "file_path":"note.txt", "old_string":BEFORE, "new_string":format!("to {placeholder}"),
    })), answer("chat", &["done"])]).await;
    let source = ground.session.source(&node).unwrap();
    agent
        .run_turn_with_view(typed.clone(), source)
        .await
        .unwrap();
    let refused = "'edit_file' requires user approval, but this delegated run has no interactive approval channel. Do this step directly or use an explicit permission mode.";
    assert_eq!(result_at(&agent, 2), (refused.to_owned(), true));
    assert_eq!(
        std::fs::read_to_string(ground.dir.path().join("note.txt")).unwrap(),
        BEFORE
    );
    assert!(around.steps.lock().unwrap().is_empty());
    let seen = watched.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].held, Err(FileRefusal::Spent));
    assert!(!seen[0].done);
    // Actual terminal facts may still refer to the review metadata. The
    // original preview/file must be gone without discarding those facts.
    let retained = seen[0]
        .operation
        .upgrade()
        .expect("actual review metadata is retained");
    assert!(matches!(
        retained.shown_arguments(),
        Err(FileRefusal::Spent)
    ));
    assert!(!retained.is_done());
    drop(seen);
    let saved = store.saves.lock().unwrap();
    assert!(saved.iter().any(|(_, _, state, _)| {
        state
            .messages
            .iter()
            .any(|message| matches!(message, Msg::User { content } if content == &typed))
    }));
    assert!(saved.iter().any(|(_, _, state, _)| {
        state.messages.iter().any(
        |message| matches!(message, Msg::Tool { content, is_error:true, .. } if content == refused)
    )
    }));
    nothing_original_sent(&server).await;
}

/// A conversation up to the point where its agent is built: everything a
/// gate has to be registered with.
struct Ground {
    session: Arc<PrivacySession>,
    boundary: Arc<ConversationBoundary>,
    provider: Arc<dyn craftsman_harness::ModelProvider>,
    review: Arc<FileReview>,
    policy: Arc<dyn CapturedUserPolicy>,
    dir: tempfile::TempDir,
}

async fn ground(vault: &Vault, server: &MockServer) -> Ground {
    let session = Arc::new(
        PrivacySession::open(
            vault,
            FreshConversationBinding::reserve(&std::env::temp_dir()).unwrap(),
            Detector::from_test_source(LiteralSource::new(&[(EMAIL, "email"), (PHONE, "phone")])),
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
        Arc::new(|_: &str| {}),
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), BEFORE).unwrap();
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    // On Windows a directory is opened with backup semantics, and a
    // link in its place is opened itself and not followed. Without
    // that the system refuses the open of a directory (code 5).
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let actual_directory = options.open(dir.path()).unwrap();
    let policy: Arc<dyn CapturedUserPolicy> = Arc::new(Rule(
        Some(FileDataFormat::PlainText),
        Arc::default(),
        actual_directory,
    ));
    let review = FileReview::new(&policy, dir.path().to_path_buf());
    boundary.review_files(review.clone());
    Ground {
        session,
        boundary,
        provider: Arc::new(provider),
        review,
        policy,
        dir,
    }
}

/// The application's own gate, registered with this conversation.
fn gate(
    ground: &Ground,
    approval: &Arc<Mutex<ApprovalState>>,
    private: Option<Arc<PrivacyApprovalPrompt>>,
) -> Arc<dyn AgentMiddleware> {
    Arc::new(
        crate::hitl_middleware::ApprovalMiddleware::new_at(
            approval.clone(),
            false,
            Some(Arc::new(|_| {})),
            None,
            None,
            ground.dir.path().to_path_buf(),
        )
        .with_privacy_prompt(private)
        .with_privacy_boundary(Some(&ground.boundary)),
    )
}

struct Stage {
    session: Arc<PrivacySession>,
    agent: Arc<Agent>,
    around: Arc<Around>,
    store: Arc<MemCheckpointer>,
    dir: tempfile::TempDir,
    _policy: Arc<dyn CapturedUserPolicy>,
}

/// The agent with `chain` in front of the two file tools, and one turn in
/// which the model edits `note.txt` with the address a person typed.
async fn edits_the_note(
    ground: Ground,
    server: &MockServer,
    chain: Vec<Arc<dyn AgentMiddleware>>,
    desk: Option<&Desk>,
) -> (Stage, Result<TurnResult, AgentError>) {
    let around = Arc::new(Around::default());
    let store = Arc::new(MemCheckpointer::default());
    let mut builder = Agent::builder(ground.provider.clone())
        .model_view_boundary(Some(ground.boundary.clone()))
        .checkpointer(store.clone())
        .durable_turn_checkpoints(true);
    for middleware in chain {
        builder = builder.middleware(middleware);
    }
    for name in ["write_file", "edit_file"] {
        builder = builder.tool(Arc::new(RestoredFileTool::new(
            Arc::new(PlainFile(
                name,
                ground.dir.path().to_path_buf(),
                around.clone(),
            )),
            ground.review.clone(),
        )));
    }
    let agent = Arc::new(builder.build());
    if let Some(desk) = desk {
        *desk.agent.lock().unwrap() = Arc::downgrade(&agent);
    }
    let typed = format!("who handles {EMAIL}?");
    let node = ground.session.admit_raw(&draft(&typed)).await.unwrap();
    let placeholder = issued(&node);
    scripted(
        server,
        vec![
            file_call(
                "c1",
                "edit_file",
                json!({"file_path": "note.txt", "old_string": BEFORE,
                    "new_string": format!("to {placeholder}")}),
            ),
            answer("chat", &["done"]),
        ],
    )
    .await;
    let source = ground.session.source(&node).unwrap();
    let turn = agent.run_turn_with_view(typed, source).await;
    (
        Stage {
            session: ground.session,
            agent,
            around,
            store,
            dir: ground.dir,
            _policy: ground.policy,
        },
        turn,
    )
}

/// Stands in front of the gate and looks at the reviewed operation of the
/// dispatch when everything behind it has returned. It holds the operation
/// only weakly.
#[derive(Default)]
struct Watch {
    /// Attach a decision of its own first, so that the gate's cannot be.
    occupy: AtomicBool,
    seen: Mutex<Vec<Watched>>,
}

struct Watched {
    operation: Weak<ReviewedFile>,
    /// What the operation said of what it holds when the gate, and the tool
    /// if it ran, had returned and the agent still carried the operation.
    held: Result<(), FileRefusal>,
    done: bool,
}

#[async_trait::async_trait]
impl AgentMiddleware for Watch {
    fn name(&self) -> &str {
        "watch"
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
        let reviewed = ReviewedFile::of(&tracked).expect("the edit was derived");
        assert!(reviewed.shown_arguments().is_ok());
        if self.occupy.load(Ordering::SeqCst) {
            tracked
                .note_decision(SourceRef::new(Arc::new("not a decision")))
                .unwrap();
        }
        let outcome = next.run(call, ctx).await;
        self.seen.lock().unwrap().push(Watched {
            operation: Arc::downgrade(&reviewed),
            held: reviewed.shown_arguments().map(|_| ()),
            done: reviewed.is_done(),
        });
        outcome
    }
}

/// What the person at the registered UI does with a request.
#[derive(Clone)]
enum Person {
    Answers(HumanChoice),
    /// Is shown nothing and gives no answer.
    LooksAway,
    /// Answers, and the answer never comes back to the gate.
    Withholds(HumanChoice),
    /// Stops the turn instead of answering.
    StopsTheTurn,
}

/// A receiver that keeps every request and decision it was given, as a
/// frontend's queue may.
struct Desk {
    ui: Mutex<Option<Arc<PrivacyApprovalUi>>>,
    person: Person,
    agent: Mutex<Weak<Agent>>,
    kept: Mutex<Vec<Kept>>,
}

struct Kept {
    request: PrivacyApprovalRequest,
    _decision: Option<PrivacyApprovalDecision>,
    /// Whether anything still holds what the request showed.
    shown: Weak<dyn std::any::Any + Send + Sync>,
    /// The details the person read, when the request was presented.
    read: Option<String>,
}

fn desk(person: Person) -> (Arc<Desk>, Arc<PrivacyApprovalPrompt>) {
    let desk = Arc::new(Desk {
        ui: Mutex::new(None),
        person,
        agent: Mutex::new(Weak::new()),
        kept: Mutex::default(),
    });
    let receiver = desk.clone();
    let (prompt, ui) =
        register_privacy_approval_ui(Arc::new(move |request: PrivacyApprovalRequest| {
            let ui = receiver.ui.lock().unwrap().clone()?;
            let shown = request.shown_handle()?;
            assert!(shown.upgrade().is_some());
            let (decision, read) = match receiver.person.clone() {
                Person::LooksAway => (None, None),
                Person::StopsTheTurn => {
                    let agent = receiver.agent.lock().unwrap().upgrade()?;
                    assert!(agent.request_cancel());
                    (None, None)
                }
                Person::Answers(choice) | Person::Withholds(choice) => {
                    let presentation = ui.present(&request).ok()?;
                    let read = presentation.review().details.to_owned();
                    (ui.decide(presentation, choice).ok(), Some(read))
                }
            };
            receiver.kept.lock().unwrap().push(Kept {
                request,
                _decision: decision.clone(),
                shown,
                read,
            });
            match receiver.person {
                Person::Withholds(_) => None,
                _ => decision,
            }
        }));
    *desk.ui.lock().unwrap() = Some(ui);
    (desk, prompt)
}

fn state(mode: crate::config::PermissionMode) -> Arc<Mutex<ApprovalState>> {
    Arc::new(Mutex::new(ApprovalState {
        mode,
        ..Default::default()
    }))
}

/// The one request the desk kept is spent, and nothing holds what it showed,
/// although the desk still has the request and the decision.
fn the_kept_request_shows_nothing(desk: &Desk, presented: bool) {
    let kept = desk.kept.lock().unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].shown.strong_count(), 0);
    assert!(kept[0].request.shown_handle().is_none());
    assert_eq!(
        kept[0].read.as_ref().map(|read| read.contains(EMAIL)),
        presented.then_some(true)
    );
    let ui = desk.ui.lock().unwrap().clone().unwrap();
    assert!(ui.present(&kept[0].request).is_err());
}

#[tokio::test]
async fn a_decision_that_is_not_an_allow_ends_the_operation_and_nothing_keeps_it() {
    use crate::config::PermissionMode::{AcceptEdits, Manual};
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    // What decides, what the tool result says, and whether the conversation
    // goes on afterwards.
    let refuses = || HumanChoice::Deny("not this file".into());
    let cases: Vec<(&str, Option<Person>, Option<&str>, bool)> = vec![
        (
            "no registered UI",
            None,
            Some("registered private UI"),
            true,
        ),
        (
            "no answer",
            Some(Person::LooksAway),
            Some("no actual answered approval"),
            true,
        ),
        (
            "a refusal",
            Some(Person::Answers(refuses())),
            Some("not this file"),
            true,
        ),
        (
            "the turn is stopped",
            Some(Person::StopsTheTurn),
            None,
            true,
        ),
        (
            "the gate's decision cannot be attached",
            None,
            Some(""),
            true,
        ),
        (
            "an answer that never returns",
            Some(Person::Withholds(refuses())),
            None,
            false,
        ),
        (
            "arguments that are not JSON",
            Some(Person::Answers(HumanChoice::Edit(
                "{\"file_path\": ".into(),
            ))),
            None,
            false,
        ),
    ];
    for (case, person, says, goes_on) in cases {
        let server = MockServer::start().await;
        let ground = ground(&vault, &server).await;
        let cannot_attach = case == "the gate's decision cannot be attached";
        let approval = state(if cannot_attach { AcceptEdits } else { Manual });
        let asked = person.map(desk);
        let watch = Arc::new(Watch::default());
        watch.occupy.store(cannot_attach, Ordering::SeqCst);
        let chain = vec![
            watch.clone() as Arc<dyn AgentMiddleware>,
            gate(
                &ground,
                &approval,
                asked.as_ref().map(|(_, prompt)| prompt.clone()),
            ),
        ];
        let (stage, turn) = edits_the_note(
            ground,
            &server,
            chain,
            asked.as_ref().map(|(desk, _)| &**desk),
        )
        .await;

        // Nothing was written and the tool was not reached.
        assert_eq!(
            std::fs::read_to_string(stage.dir.path().join("note.txt")).unwrap(),
            BEFORE,
            "{case}"
        );
        assert!(stage.around.steps.lock().unwrap().is_empty(), "{case}");
        assert!(
            stage.agent.actual_terminal_outcomes().unwrap().is_empty(),
            "{case}"
        );
        // When the gate returned, the operation had let go of the content
        // and of the file it had opened for the edit. Where the conversation
        // ended instead, it says so.
        let seen = watch.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{case}");
        assert_eq!(
            seen[0].held,
            Err(match goes_on {
                true => FileRefusal::Spent,
                false => FileRefusal::Stopped(SessionStop::Closed),
            }),
            "{case}"
        );
        assert!(!seen[0].done, "{case}");
        // And after the dispatch nothing refers to the operation at all: not
        // the gate, not a decision, not a request.
        assert!(seen[0].operation.upgrade().is_none(), "{case}");
        if let Some((desk, _)) = &asked {
            let presented = !matches!(desk.person, Person::LooksAway | Person::StopsTheTurn);
            the_kept_request_shows_nothing(desk, presented);
        }

        // The answer, the result and the conversation are what they were.
        assert_eq!(stage.session.check().is_ok(), goes_on, "{case}");
        assert_eq!(turn.is_ok(), goes_on, "{case}");
        if let Some(says) = says {
            let (content, is_error) = result_at(&stage.agent, 2);
            assert!(is_error && content.contains(says), "{case}: {content}");
        }
        if case == "the turn is stopped" {
            let Ok(TurnResult::Done(output)) = &turn else {
                panic!("a stopped turn ends")
            };
            assert_eq!(output.final_text(), "Stopped by the user.");
        }
        if goes_on {
            assert_eq!(
                stage.store.saves.lock().unwrap().last().unwrap().2.messages,
                recorded(&stage.agent),
                "{case}"
            );
        }
        nothing_original_sent(&server).await;
    }
}

#[tokio::test]
async fn an_allowed_operation_is_kept_until_the_file_tool_has_written() {
    use crate::config::PermissionMode::{AcceptEdits, Manual};
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let edited = json!({"file_path": "note.txt", "old_string": BEFORE,
        "new_string": "written by a person"})
    .to_string();
    let cases = [
        ("the policy allows", None),
        ("a person approves", Some(HumanChoice::Approve)),
        ("a person edits", Some(HumanChoice::Edit(edited.clone()))),
    ];
    for (case, choice) in cases {
        let server = MockServer::start().await;
        let ground = ground(&vault, &server).await;
        let approval = state(if choice.is_some() {
            Manual
        } else {
            AcceptEdits
        });
        let person_edits = matches!(choice, Some(HumanChoice::Edit(_)));
        let asked = choice.map(|choice| desk(Person::Answers(choice)));
        let watch = Arc::new(Watch::default());
        let chain = vec![
            watch.clone() as Arc<dyn AgentMiddleware>,
            gate(
                &ground,
                &approval,
                asked.as_ref().map(|(_, prompt)| prompt.clone()),
            ),
        ];
        let (stage, turn) = edits_the_note(ground, &server, chain, None).await;
        turn.unwrap();

        // The file tool ran once and wrote: the real value where the model
        // wrote the placeholder, or the person's own text.
        assert_eq!(
            std::fs::read_to_string(stage.dir.path().join("note.txt")).unwrap(),
            match person_edits {
                true => "written by a person".to_owned(),
                false => format!("to {EMAIL}"),
            },
            "{case}"
        );
        assert_eq!(
            *stage.around.steps.lock().unwrap(),
            ["began note.txt", "finished note.txt"],
            "{case}"
        );
        // The gate did not end the operation. The file tool did: with its
        // write, or because a person's arguments replaced what was derived.
        let seen = watch.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{case}");
        assert_eq!(seen[0].done, !person_edits, "{case}");
        assert_eq!(seen[0].held, Err(FileRefusal::Spent), "{case}");

        // The decision is still the one the registered gate made, with the
        // person's answer as it was typed.
        let outcomes = stage.agent.actual_terminal_outcomes().unwrap();
        assert_eq!(outcomes.len(), 1, "{case}");
        let invocation = outcomes[0].invocation();
        let input = invocation
            .decision_source()
            .unwrap()
            .downcast::<InlineDecisionInput>()
            .unwrap();
        assert!(input.registered_session().is_some(), "{case}");
        assert_eq!(invocation.human_edited(), person_edits, "{case}");
        assert_eq!(
            input.raw_edit(),
            person_edits.then_some(edited.as_str()),
            "{case}"
        );
        if let Some((desk, _)) = &asked {
            the_kept_request_shows_nothing(desk, true);
        }
        // The record and its save are the agent's own.
        assert_eq!(
            stage.store.saves.lock().unwrap().last().unwrap().2.messages,
            recorded(&stage.agent),
            "{case}"
        );
        assert!(!result_at(&stage.agent, 2).1, "{case}");
        assert!(stage.session.check().is_ok(), "{case}");
        nothing_original_sent(&server).await;
    }
}

/// The refusal an extra receive was answered with.
fn refusal<T>(answer: Result<T, AgentError>) -> String {
    answer
        .err()
        .expect("an extra receive is refused")
        .to_string()
}

/// A gate of the fixtures that asks registered producers itself, so that a
/// case can ask twice, or ask a producer of another conversation.
struct Asks {
    own: Arc<InlineDecisionProducer>,
    foreign: Arc<InlineDecisionProducer>,
    /// What each extra receive answered, and whether the operation still
    /// held its content afterwards.
    refusals: Mutex<Vec<(String, bool)>>,
    /// The input of a decision that was not an allow, held only weakly.
    denied: Mutex<Option<Weak<InlineDecisionInput>>>,
}

#[async_trait::async_trait]
impl AgentMiddleware for Asks {
    fn name(&self) -> &str {
        "asks"
    }

    fn view_support(&self) -> ViewSupport {
        ViewSupport::Preserving
    }

    async fn around_tool(
        &self,
        _: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let tracked = next.tracked().expect("the agent tracks this dispatch");
        let reviewed = ReviewedFile::of(&tracked).expect("the edit was derived");
        let refused = |refusal: String| {
            self.refusals
                .lock()
                .unwrap()
                .push((refusal, reviewed.shown_arguments().is_ok()));
        };
        // Another conversation's gate, before this one has decided.
        refused(refusal(self.foreign.receive_inline(&tracked, None).await));
        let resolution = self.own.receive_inline(&tracked, None).await?;
        assert!(resolution.input.registered_session().is_some());
        if resolution.permission != ToolPermission::Allow {
            *self.denied.lock().unwrap() = Some(Arc::downgrade(&resolution.input));
            return Ok(ToolOutcome::err("Denied by the policy."));
        }
        // The same plan again, and the other gate again, after the allow.
        refused(refusal(self.own.receive_inline(&tracked, None).await));
        refused(refusal(self.foreign.receive_inline(&tracked, None).await));
        next.run(resolution.call, ctx).await
    }
}

#[tokio::test]
async fn a_second_or_foreign_receive_ends_nothing_and_a_denied_input_is_not_kept() {
    use crate::config::PermissionMode::{AcceptEdits, Plan};
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    for mode in [AcceptEdits, Plan] {
        let (server, other) = (MockServer::start().await, MockServer::start().await);
        let ground = ground(&vault, &server).await;
        let elsewhere = self::ground(&vault, &other).await;
        let register = |conversation: &Ground| {
            InlineDecisionProducer::register(
                &conversation.boundary,
                state(mode),
                conversation.dir.path().to_path_buf(),
                None,
                None,
            )
        };
        let asks = Arc::new(Asks {
            own: register(&ground),
            foreign: register(&elsewhere),
            refusals: Mutex::default(),
            denied: Mutex::default(),
        });
        let chain = vec![asks.clone() as Arc<dyn AgentMiddleware>];
        let (stage, turn) = edits_the_note(ground, &server, chain, None).await;
        turn.unwrap();
        let refusals = asks.refusals.lock().unwrap();
        let not_approved = FileRefusal::NotApproved.to_string();
        let on_disk = std::fs::read_to_string(stage.dir.path().join("note.txt")).unwrap();
        if mode == AcceptEdits {
            // The other conversation's gate was refused twice and a second
            // receive of the same plan once, as spent. None of them ended
            // the operation the first receive had allowed: it still held its
            // content, and the file tool wrote it.
            assert_eq!(refusals.len(), 3);
            assert!(refusals[0].0.contains(&not_approved));
            assert!(refusals[1].0.contains(&FileRefusal::Spent.to_string()));
            assert!(refusals[2].0.contains(&not_approved));
            assert!(refusals.iter().all(|(_, held)| *held));
            assert_eq!(on_disk, format!("to {EMAIL}"));
            assert_eq!(
                *stage.around.steps.lock().unwrap(),
                ["began note.txt", "finished note.txt"]
            );
        } else {
            // A denial by the policy: the gate knew the input while the
            // dispatch held it, and keeps nothing of it afterwards.
            assert_eq!(refusals.len(), 1);
            assert!(refusals[0].0.contains(&not_approved) && refusals[0].1);
            assert_eq!(on_disk, BEFORE);
            let denied = asks.denied.lock().unwrap().clone().unwrap();
            assert!(denied.upgrade().is_none());
            assert!(stage.around.steps.lock().unwrap().is_empty());
        }
        assert!(stage.session.check().is_ok() && elsewhere.session.check().is_ok());
        nothing_original_sent(&server).await;
    }
}
