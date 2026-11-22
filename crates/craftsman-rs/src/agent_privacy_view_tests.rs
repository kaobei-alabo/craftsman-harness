//! Marked native fixture for the shared builder with a selected privacy
//! conversation: the actual builder, tools, approval gate, checkpoint store
//! and Chat client against a loopback server, with a real vault and guard.
//! The detector is fixed host reply rules. No frontend selects any of this.

use super::*;
use crate::privacy_entry::PrivateEntryError;
use crate::privacy_hook::Detector;
use crate::privacy_hook::tests::{LiteralSource, PHONE};
use crate::privacy_send::tests::{EMAIL, answer, open};
use crate::privacy_session::wire_fixtures::{config, reply};
use crate::privacy_session::{
    AdmitRefusal, BuildRefusal, FixturePolicy, FreshConversationBinding, PrivacySession,
    PrivacyViewSelection, RawOrigin, SessionStop,
};
use crate::privacy_vault::Vault;
use craftsman_harness::{Msg, TurnResult};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "agent::privacy_view_tests::privacy_view_builder_fixture";
const MARKER: &str = "craftsman-privacy-view=";
const DONE: &str =
    "actual builder, loopback HTTP, checkpoint store and private view assertions completed";

async fn privacy(vault: &Vault, directory: &std::path::Path) -> Arc<PrivacySession> {
    let source = LiteralSource::new(&[(EMAIL, "email"), (PHONE, "phone")]);
    // The system prompt and tool text are the application's own.
    source.with_floor(true);
    Arc::new(
        PrivacySession::open(
            vault,
            FreshConversationBinding::reserve(directory).unwrap(),
            Detector::from_test_source(source),
        )
        .await
        .unwrap(),
    )
}

async fn build(
    cfg: &Config,
    directory: &std::path::Path,
    selected: Option<&Arc<PrivacySession>>,
    thread_id: Option<String>,
    activity: &Arc<Mutex<Vec<String>>>,
    alter: impl FnOnce(&mut SessionExtras),
) -> Option<Session> {
    let activity = activity.clone();
    let mut extras = SessionExtras {
        privacy_view: selected
            .map(|session| PrivacyViewSelection::fixture_limited_set(session.clone())),
        workspace_root: Some(directory.to_path_buf()),
        token_sink: Some(Arc::new(|_| {})),
        activity_sink: Some(Arc::new(move |line: &str| {
            activity.lock().unwrap().push(line.to_owned())
        })),
        ..Default::default()
    };
    alter(&mut extras);
    build_session_ext(
        cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        thread_id,
        extras,
    )
    .await
}

fn asks_to_read(path: &std::path::Path) -> ResponseTemplate {
    reply(
        "chat",
        0,
        Some(("call-read", "read_file", &json!({"file_path": path}))),
        "",
    )
}

async fn sent(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| String::from_utf8(request.body.clone()).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned private-view fixture; unmarked/unknown startup returns before config access"]
async fn privacy_view_builder_fixture() {
    let selected: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if selected.len() != 1 || selected[0] != "chat" {
        return;
    }
    let directory = std::env::current_dir().unwrap();
    let vault_root = directory.parent().unwrap().join("vault");
    std::fs::create_dir(&vault_root).unwrap();
    let (vault, _) = open(&vault_root, "unused-thread").await;
    let activity = Arc::new(Mutex::new(Vec::new()));
    conversation(&directory, &vault, &activity).await;
    repair(&directory, &vault, &activity).await;
    normal_parts(&directory, &vault, &activity).await;
    refusals(&directory, &vault, &activity).await;
    eprintln!("{DONE}");
}

/// One real conversation through the shared builder, then its end.
async fn conversation(
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let notes = directory.join("notes.txt");
    std::fs::write(&notes, format!("Their number is {PHONE}.\n")).unwrap();
    let server = MockServer::start().await;
    for (priority, reply) in [asks_to_read(&notes), answer("chat", &["fixture answer"])]
        .into_iter()
        .enumerate()
    {
        Mock::given(method("POST"))
            .respond_with(reply)
            .up_to_n_times(1)
            .with_priority(priority as u8 + 1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .respond_with(answer("chat", &["second answer"]))
        .mount(&server)
        .await;
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    let mut session = build(&cfg, directory, Some(&owner), None, activity, |_| {})
        .await
        .expect("a selected Chat session is built");

    // The session is the reserved conversation: one id for the agent, the
    // checkpoint store and the privacy owner, and the owner's own guard.
    assert_eq!(session.thread_id, owner.thread_id());
    let view = session.privacy_view.as_ref().expect("the view is kept");
    assert!(Arc::ptr_eq(view.session(), &owner));
    assert!(Arc::ptr_eq(
        session.send_guard.as_ref().unwrap(),
        &owner.send_guard()
    ));
    // The parts that have no source path are named, not dropped silently.
    let named = activity
        .lock()
        .unwrap()
        .iter()
        .find(|line| line.starts_with("This private conversation runs without: "))
        .cloned()
        .expect("the frontend is told what is left out");
    assert!(named.contains("compaction"), "{named}");
    // The protected call repair, the todo list, memory and the reminders are
    // not among them: they run, tracked.
    for kept in ["patch_tool_calls", "todos", "memory", "system_reminders"] {
        assert!(!named.contains(kept), "{named}");
    }
    // The untyped entry is refused before anything is recorded.
    assert!(session.agent.run_turn("untyped").await.is_err());
    assert!(session.agent.state.lock().unwrap().messages.is_empty());

    let typed = format!("Who handles {EMAIL}? The notes are in notes.txt.");
    let TurnResult::Done(output) = view.turn(&session.agent, &typed).await.unwrap() else {
        panic!("the turn finishes");
    };
    assert_eq!(output.final_text(), "fixture answer");

    // What was typed and what the tool returned are recorded as they are.
    let messages = session.agent.state.lock().unwrap().messages.clone();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0], Msg::user(typed.clone()));
    let Msg::Tool {
        content, is_error, ..
    } = &messages[2]
    else {
        panic!("the tool result is recorded");
    };
    assert!(!is_error && content.contains(PHONE), "{content}");
    // Neither original left the process, in either request, and the system
    // text and the tools went out.
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 2);
    for body in &requests {
        assert!(!body.contains(EMAIL) && !body.contains(PHONE));
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(!body["messages"][0]["content"].as_str().unwrap().is_empty());
        assert!(
            body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "read_file")
        );
    }
    let second: serde_json::Value = serde_json::from_str(&requests[1]).unwrap();
    assert_eq!(second["messages"][2]["tool_calls"][0]["id"], "call-read");
    assert!(
        second["messages"][3]["content"]
            .as_str()
            .unwrap()
            .contains("Their number is ")
    );
    // The host can reach what ran: the tool's own outcome is the recorded one.
    let sources = session.agent.recorded_sources().unwrap();
    let result = owner
        .actual_result(sources[2].as_ref().unwrap())
        .expect("the result has its record");
    assert_eq!(result.tool(), "read_file");
    assert_eq!(result.reached().len(), 1);
    assert_eq!(
        owner
            .admitted(sources[2].as_ref().unwrap())
            .unwrap()
            .origin(),
        RawOrigin::ToolResult
    );
    assert_eq!(session.usage.lock().unwrap().0, 6);

    // The real record is in the checkpoint store under the reserved id. An
    // ordinary session can read it back exactly; nothing in it is redacted.
    let mut plain = build(&cfg, directory, None, None, activity, |_| {})
        .await
        .unwrap();
    assert!(plain.privacy_view.is_none());
    assert!(plain.load_thread(owner.thread_id()).await.unwrap());
    assert_eq!(plain.agent.state.lock().unwrap().messages, messages);
    plain.lsp.shutdown().await;

    // A draft that cannot be admitted is turned away unchanged. Nothing is
    // recorded or sent, and the conversation continues.
    let issued = second["messages"][1]["content"]
        .as_str()
        .unwrap()
        .strip_prefix("Who handles ")
        .unwrap()
        .split('?')
        .next()
        .unwrap()
        .to_owned();
    assert!(matches!(
        view.turn(&session.agent, &format!("and {issued} again"))
            .await,
        Err(PrivateEntryError::Refused(AdmitRefusal::IssuedPlaceholder))
    ));
    assert_eq!(session.agent.state.lock().unwrap().messages.len(), 4);
    assert_eq!(sent(&server).await.len(), 2);
    let TurnResult::Done(output) = view.turn(&session.agent, "thanks").await.unwrap() else {
        panic!("the conversation continues");
    };
    assert_eq!(output.final_text(), "second answer");

    // Reopening another thread is refused, and rebinding closes the owner
    // instead of moving it. Nothing further is recorded or sent.
    assert!(session.load_thread("another-thread").await.is_err());
    session.set_thread_id("another-thread");
    assert_eq!(session.thread_id, owner.thread_id());
    assert_eq!(owner.check(), Err(SessionStop::Closed));
    let view = session.privacy_view.as_ref().unwrap();
    assert!(matches!(
        view.turn(&session.agent, "after the end").await,
        Err(PrivateEntryError::Stopped(_))
    ));
    assert_eq!(session.agent.state.lock().unwrap().messages.len(), 6);
    assert_eq!(sent(&server).await.len(), 3);

    // A new conversation is a new owner, a new id and a new scope.
    let next = privacy(vault, directory).await;
    let fresh = build(&cfg, directory, Some(&next), None, activity, |_| {})
        .await
        .unwrap();
    assert_ne!(fresh.thread_id, session.thread_id);
    assert_eq!(fresh.thread_id, next.thread_id());
    let fresh_view = fresh.privacy_view.as_ref().unwrap();
    fresh_view.turn(&fresh.agent, &typed).await.unwrap();
    let requests = sent(&server).await;
    let body: serde_json::Value = serde_json::from_str(requests.last().unwrap()).unwrap();
    let reissued = body["messages"][1]["content"].as_str().unwrap();
    assert!(!reissued.contains(EMAIL) && !reissued.contains(&issued));
    fresh.lsp.shutdown().await;
    session.lsp.shutdown().await;
}

/// The protected call repair in a built session: a reply whose call arguments
/// are not a JSON object is repaired as always, and the conversation keeps
/// what was received and what the repair changed.
async fn repair(directory: &std::path::Path, vault: &Vault, activity: &Arc<Mutex<Vec<String>>>) {
    let server = MockServer::start().await;
    let malformed = "notes.txt{";
    let body = json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-read","type":"function","function":{"name":"read_file","arguments":malformed}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!("data: {body}\n\ndata: [DONE]\n\n"),
            "text/event-stream",
        ))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(answer("chat", &["fixture answer"]))
        .mount(&server)
        .await;
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    let session = build(&cfg, directory, Some(&owner), None, activity, |_| {})
        .await
        .unwrap();
    let view = session.privacy_view.as_ref().unwrap();
    let TurnResult::Done(output) = view.turn(&session.agent, "read the notes").await.unwrap()
    else {
        panic!("the turn finishes");
    };
    assert_eq!(output.final_text(), "fixture answer");
    // The record holds the repaired call and the tool's real answer to it.
    let repaired = json!({"_raw": malformed});
    let messages = session.agent.state.lock().unwrap().messages.clone();
    assert!(matches!(&messages[1], Msg::Assistant { tool_calls, .. }
        if tool_calls[0].args == repaired));
    assert!(matches!(&messages[2], Msg::Tool { .. }));
    // The conversation keeps the reply as received and the recorded change.
    let sources = session.agent.recorded_sources().unwrap();
    let reply = owner.committed_model(sources[1].as_ref().unwrap()).unwrap();
    assert!(
        matches!(reply.received_message().unwrap(), Msg::Assistant { tool_calls, .. }
        if tool_calls[0].args == serde_json::Value::String(malformed.into()))
    );
    let edits = reply.edits().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].by, "patch_tool_calls");
    assert_eq!(edits[0].replaced, repaired);
    // The tool was given the repaired call, and the next request carries it.
    let result = owner.actual_result(sources[2].as_ref().unwrap()).unwrap();
    assert_eq!(result.reached()[0].call.args, repaired);
    let requests = sent(&server).await;
    let second: serde_json::Value = serde_json::from_str(&requests[1]).unwrap();
    assert_eq!(
        second["messages"][2]["tool_calls"][0]["function"]["arguments"],
        repaired.to_string()
    );
    session.lsp.shutdown().await;
}

/// The todo list, the memory section and the system prompt in a built
/// session: each is added as parts, and the request carries the same text an
/// ordinary session sends, with the one supported value replaced.
async fn normal_parts(
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    use craftsman_harness::Backend;
    const CONTACT: &str = "bob@example.invalid";
    let script = || async {
        let server = MockServer::start().await;
        let arguments =
            json!({"todos": [{"content": "review notes", "status": "in_progress"}]}).to_string();
        let body = json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-todos","type":"function","function":{"name":"write_todos","arguments":arguments}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {body}\n\ndata: [DONE]\n\n"),
                "text/event-stream",
            ))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(answer("chat", &["fixture answer"]))
            .mount(&server)
            .await;
        server
    };
    let system_of = |body: &str| -> String {
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        body["messages"][0]["content"].as_str().unwrap().to_owned()
    };

    let server = script().await;
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    let session = build(&cfg, directory, Some(&owner), None, activity, |_| {})
        .await
        .unwrap();
    // What the memory store holds is read at the start of the turn.
    session
        .memories
        .as_ref()
        .expect("the memory store opened")
        .write("/memories/AGENTS.md", &format!("Office contact: {CONTACT}"))
        .await
        .unwrap();
    let view = session.privacy_view.as_ref().unwrap();
    let TurnResult::Done(output) = view.turn(&session.agent, "plan the work").await.unwrap() else {
        panic!("the turn finishes");
    };
    assert_eq!(output.final_text(), "fixture answer");
    assert_eq!(session.agent.state.lock().unwrap().todos.len(), 1);
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 2);
    let first = system_of(&requests[0]);
    let second = system_of(&requests[1]);
    // The memory file's content went out as its view, inside the section
    // the memory middleware always writes.
    for system in [&first, &second] {
        assert!(!system.contains(CONTACT));
        assert!(system.contains("<agent_memory>\n/memories/AGENTS.md\nOffice contact: "));
        assert!(system.contains("## `write_todos`"));
    }
    // After the call the todo the model wrote is in the system text, between
    // the application's own heading and label.
    assert!(!first.contains("Current todo list:"));
    assert!(second.contains("Current todo list:\n- [InProgress] review notes\n"));

    // An ordinary session with the same script sends the same system text
    // for these parts, byte for byte, except for the one value.
    let plain_server = script().await;
    let plain_cfg = config("chat", &plain_server);
    let plain = build(&plain_cfg, directory, None, None, activity, |_| {})
        .await
        .unwrap();
    plain.agent.run_turn("plan the work").await.unwrap();
    let plain_requests = sent(&plain_server).await;
    let issued = second
        .split("Office contact: ")
        .nth(1)
        .unwrap()
        .split('\n')
        .next()
        .unwrap();
    assert_ne!(issued, CONTACT);
    for (selected, ordinary) in [&first, &second].into_iter().zip(&plain_requests) {
        let (selected, ordinary) = (selected.replace(issued, CONTACT), system_of(ordinary));
        // Everything the selected session sends is the ordinary text, byte
        // for byte. The ordinary session goes on with the section of the
        // delegated-agent middleware, which the fixture leaves out because
        // it has no source path yet.
        let rest = ordinary
            .strip_prefix(selected.as_str())
            .unwrap_or_else(|| panic!("not the ordinary text: {selected}"));
        assert!(
            rest.starts_with("\n\nLaunch an ephemeral subagent"),
            "{rest}"
        );
    }
    plain.lsp.shutdown().await;
    session.lsp.shutdown().await;
}

/// Selections that are refused before a session exists, each with its
/// typed reason for the host that made the selection.
async fn refusals(directory: &std::path::Path, vault: &Vault, activity: &Arc<Mutex<Vec<String>>>) {
    let server = MockServer::start().await;
    let cfg = config("chat", &server);
    let fixed = "The private conversation view is not available for this session.";
    let count = |line: &str| {
        activity
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| *seen == line)
            .count()
    };
    let owner = privacy(vault, directory).await;
    let policy = |disabled: bool, inspectable_items: bool| {
        Arc::new(FixturePolicy {
            disabled,
            inspectable_items,
        })
    };

    // A policy that does not enable privacy for the workspace makes no
    // selection, and neither does a conversation that has ended.
    assert_eq!(
        PrivacyViewSelection::from_user_policy(policy(true, false), owner.clone()).err(),
        Some(BuildRefusal::PolicyDisabled)
    );
    let ended = privacy(vault, directory).await;
    let late = PrivacyViewSelection::fixture_limited_set(ended.clone());
    ended.stop();
    assert_eq!(
        PrivacyViewSelection::from_user_policy(policy(false, false), ended.clone()).err(),
        Some(BuildRefusal::Ended)
    );

    // A caller's thread id, a separate guard, a requested part that must not
    // be left out, and an owner that ended after the selection was made.
    let other = privacy(vault, directory).await;
    let guard = other.send_guard();
    type Alter = Box<dyn FnOnce(&mut SessionExtras)>;
    let cases: Vec<(Option<String>, Alter, BuildRefusal)> = vec![
        (
            Some("caller-thread".into()),
            Box::new(|_| {}),
            BuildRefusal::NotFresh,
        ),
        (
            None,
            Box::new(move |extras| extras.send_guard = Some(guard)),
            BuildRefusal::NotFresh,
        ),
        (
            None,
            Box::new(|extras| extras.verification = true),
            BuildRefusal::RequestedPart,
        ),
        (
            None,
            Box::new(|extras| extras.context_compilation = true),
            BuildRefusal::RequestedPart,
        ),
        (
            None,
            Box::new(|extras| extras.failure_controls = true),
            BuildRefusal::RequestedPart,
        ),
    ];
    for (thread, alter, expected) in cases {
        let selection = PrivacyViewSelection::fixture_limited_set(owner.clone());
        let report = selection.report();
        assert!(
            build(&cfg, directory, None, thread, activity, |extras| {
                extras.privacy_view = Some(selection);
                alter(extras)
            })
            .await
            .is_none()
        );
        assert_eq!(report.refusal(), Some(expected));
    }
    let report = late.report();
    assert!(
        build(&cfg, directory, None, None, activity, |extras| {
            extras.privacy_view = Some(late)
        })
        .await
        .is_none()
    );
    assert_eq!(report.refusal(), Some(BuildRefusal::Ended));
    assert_eq!(count(fixed), 6);
    // None of them ended the conversation that was offered.
    assert!(owner.check().is_ok());

    // A policy that requires inspectable provider items refuses a native
    // wire, which sends back items that cannot be inspected.
    let native = config("responses", &server);
    let selection =
        PrivacyViewSelection::from_user_policy(policy(false, true), owner.clone()).unwrap();
    let report = selection.report();
    assert!(
        build(&native, directory, None, None, activity, |extras| {
            extras.privacy_view = Some(selection)
        })
        .await
        .is_none()
    );
    assert_eq!(report.refusal(), Some(BuildRefusal::OpaqueProviderItems));
    assert_eq!(
        count(
            "This private conversation must be able to inspect what a provider is sent. A native wire sends back items that cannot be inspected."
        ),
        1
    );

    // The selection a policy makes leaves nothing out. Every installed part
    // must have a source path, and the ones that have none are named: to the
    // frontend in one line, and to the host as a typed list. The protected
    // call repair, the approval gate and the result size limit are not among
    // them.
    let selection =
        PrivacyViewSelection::from_user_policy(policy(false, false), owner.clone()).unwrap();
    let report = selection.report();
    assert!(
        build(&cfg, directory, None, None, activity, |extras| {
            extras.privacy_view = Some(selection)
        })
        .await
        .is_none()
    );
    let Some(BuildRefusal::MissingSourcePaths(missing)) = report.refusal() else {
        panic!("the host is told which parts have no source path");
    };
    assert!(
        missing.iter().any(|part| part == "compaction"),
        "{missing:?}"
    );
    for kept in [
        "patch_tool_calls",
        "hitl",
        "result_budgets",
        "todos",
        "memory",
        "system_reminders",
    ] {
        assert!(!missing.iter().any(|part| part == kept), "{missing:?}");
    }
    let line = activity
        .lock()
        .unwrap()
        .iter()
        .find(|line| {
            line.starts_with(
                "This private conversation cannot be built. No source path exists for: ",
            )
        })
        .cloned()
        .expect("the frontend is told why");
    assert!(
        line.ends_with(&format!("{}.", missing.join(", "))),
        "{line}"
    );
    assert!(owner.check().is_ok());
    assert!(server.received_requests().await.unwrap().is_empty());
}

async fn run_fixture(
    arguments: &[String],
    directory: &std::path::Path,
    home: &std::path::Path,
) -> std::process::Output {
    let config = home.join("config");
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("CRAFTSMAN_CONFIG_DIR", config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = command.spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap().take(65537);
    let mut stderr = child.stderr.take().unwrap().take(65537);
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>(std::process::Output {
            status,
            stdout: out,
            stderr: err,
        })
    })
    .await;
    match result {
        Ok(Ok(output)) => output,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("private view fixture exceeded its bound");
        }
    }
}

#[tokio::test]
async fn actual_builder_runs_a_selected_privacy_conversation_on_the_chat_wire() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(temp.path().join("config")).unwrap();
    let output = run_fixture(&[format!("{MARKER}chat")], &workspace, temp.path()).await;
    assert!(output.stdout.len() <= 65536 && output.stderr.len() <= 65536);
    let error = std::str::from_utf8(&output.stderr).unwrap();
    assert!(output.status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
}

#[tokio::test]
async fn unmarked_or_unknown_private_view_fixture_returns_before_any_access() {
    let temp = tempfile::tempdir().unwrap();
    for arguments in [
        vec![],
        vec![format!("{MARKER}unknown")],
        vec![format!("{MARKER}chat"), format!("{MARKER}chat")],
    ] {
        let output = run_fixture(&arguments, temp.path(), temp.path()).await;
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(DONE));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
