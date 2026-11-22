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

/// The selection a user's policy makes for a live conversation. It leaves
/// nothing out: every part of the session must have a source path.
fn selected_by_policy(session: &Arc<PrivacySession>) -> PrivacyViewSelection {
    PrivacyViewSelection::from_user_policy(Arc::new(FixturePolicy::default()), session.clone())
        .expect("a live fixture conversation")
}

/// The session a fixture run asks for. The run's own changes to the extras
/// are applied here, so that the build itself is one function for all runs.
fn build<'a>(
    cfg: &'a Config,
    directory: &std::path::Path,
    selected: Option<&Arc<PrivacySession>>,
    thread_id: Option<String>,
    activity: &Arc<Mutex<Vec<String>>>,
    alter: impl FnOnce(&mut SessionExtras),
) -> impl std::future::Future<Output = Option<Session>> + 'a {
    let mut extras = extras_for(directory, selected, activity);
    alter(&mut extras);
    build_session(cfg, thread_id, ApprovalState::default(), extras)
}

/// As [`build`], with the approval state the session starts from.
fn build_with<'a>(
    cfg: &'a Config,
    directory: &std::path::Path,
    selected: Option<&Arc<PrivacySession>>,
    activity: &Arc<Mutex<Vec<String>>>,
    approval: ApprovalState,
) -> impl std::future::Future<Output = Option<Session>> + 'a {
    let extras = extras_for(directory, selected, activity);
    build_session(cfg, None, approval, extras)
}

fn extras_for(
    directory: &std::path::Path,
    selected: Option<&Arc<PrivacySession>>,
    activity: &Arc<Mutex<Vec<String>>>,
) -> SessionExtras {
    let activity = activity.clone();
    SessionExtras {
        privacy_view: selected.map(selected_by_policy),
        workspace_root: Some(directory.to_path_buf()),
        token_sink: Some(Arc::new(|_| {})),
        activity_sink: Some(Arc::new(move |line: &str| {
            activity.lock().unwrap().push(line.to_owned())
        })),
        ..Default::default()
    }
}

async fn build_session(
    cfg: &Config,
    thread_id: Option<String>,
    approval: ApprovalState,
    extras: SessionExtras,
) -> Option<Session> {
    build_session_ext(
        cfg,
        Arc::new(Mutex::new(approval)),
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
    let on_a_pty = cfg!(unix) && selected.len() == 1 && selected[0] == "terminal";
    if selected.len() != 1 || !(selected[0] == "chat" || on_a_pty) {
        return;
    }
    let directory = std::env::current_dir().unwrap();
    let vault_root = directory.parent().unwrap().join("vault");
    std::fs::create_dir(&vault_root).unwrap();
    let (vault, _) = open(&vault_root, "unused-thread").await;
    #[cfg(unix)]
    if on_a_pty {
        terminal_on_a_pty(&directory, &vault).await;
        eprintln!("{DONE}");
        return;
    }
    let activity = Arc::new(Mutex::new(Vec::new()));
    conversation(&directory, &vault, &activity).await;
    repair(&directory, &vault, &activity).await;
    normal_parts(&directory, &vault, &activity).await;
    instruction_files(&directory, &vault, &activity).await;
    repeated_arguments(&directory, &vault, &activity).await;
    compaction_lane(&directory, &vault, &activity).await;
    delegation(&directory, &vault, &activity).await;
    restoration(&directory, &vault, &activity).await;
    delegated_restoration(&directory, &vault, &activity).await;
    automatic_policy(&directory, &vault, &activity).await;
    terminal_display(&directory, &vault).await;
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
    // Every part of the configured session has a source path: nothing was
    // left out and nothing was refused.
    assert!(
        !activity
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("This private conversation "))
    );
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
        // The delegation tools are among them, as in an ordinary session.
        for name in ["read_file", "task", "send_message"] {
            assert!(
                body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["function"]["name"] == name),
                "{name}"
            );
        }
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
        // for byte, the section of the delegated-agent middleware included.
        assert!(selected.contains("\n\nLaunch an ephemeral subagent"));
        assert_eq!(selected, ordinary);
    }
    plain.lsp.shutdown().await;
    session.lsp.shutdown().await;
}

/// The global and the project instruction file in a built session. Each is
/// captured on its own where it is read, so each has its own placeholder, and
/// the section is still the text an ordinary session sends.
async fn instruction_files(
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    const GLOBAL: &str = "carol@example.invalid";
    const PROJECT: &str = "dave@example.invalid";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(answer("chat", &["fixture answer"]))
        .mount(&server)
        .await;
    let cfg = config("chat", &server);
    let global = cfg.agent_dir().join("AGENTS.md");
    let project = directory.join(".craftsman").join("AGENTS.md");
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    // Space around both files, which the section trims.
    std::fs::write(&global, format!("  Global rule: ask {GLOBAL}.\n\n")).unwrap();
    std::fs::write(&project, format!("Project rule: copy {PROJECT}.\n  ")).unwrap();
    let system_of = |body: &str| -> String {
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        body["messages"][0]["content"].as_str().unwrap().to_owned()
    };

    let owner = privacy(vault, directory).await;
    let session = build(&cfg, directory, Some(&owner), None, activity, |_| {})
        .await
        .expect("a session with instruction files is built");
    let view = session.privacy_view.as_ref().unwrap();
    view.turn(&session.agent, "hello").await.unwrap();
    let selected = system_of(&sent(&server).await[0]);
    assert!(!selected.contains(GLOBAL) && !selected.contains(PROJECT));
    // Each file's value has its own placeholder, inside the section's own
    // headings.
    let between = |text: &str, before: &str, after: &str| -> String {
        text.split(before)
            .nth(1)
            .and_then(|rest| rest.split(after).next())
            .unwrap_or_else(|| panic!("{before} is in the system text"))
            .to_owned()
    };
    let for_global = between(&selected, "\n# AGENTS.md\n\nGlobal rule: ask ", ".\n");
    let for_project = between(
        &selected,
        "\n# Project AGENTS.md (./.craftsman/AGENTS.md)\n\nProject rule: copy ",
        ".\n",
    );
    // The memory file an earlier run of the fixture stored has a value too.
    let for_memory = between(&selected, "Office contact: ", "\n");
    assert_ne!(for_global, for_project);
    assert!(for_global != GLOBAL && for_project != PROJECT);
    session.lsp.shutdown().await;

    // An ordinary session sends the same system text, byte for byte, with
    // the two values where the placeholders are.
    let plain_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(answer("chat", &["fixture answer"]))
        .mount(&plain_server)
        .await;
    let plain_cfg = config("chat", &plain_server);
    let plain = build(&plain_cfg, directory, None, None, activity, |_| {})
        .await
        .unwrap();
    plain.agent.run_turn("hello").await.unwrap();
    let ordinary = system_of(&sent(&plain_server).await[0]);
    assert!(ordinary.contains(GLOBAL) && ordinary.contains(PROJECT));
    assert_eq!(
        selected
            .replace(&for_global, GLOBAL)
            .replace(&for_project, PROJECT)
            .replace(&for_memory, "bob@example.invalid"),
        ordinary
    );
    plain.lsp.shutdown().await;
    assert!(owner.check().is_ok());
    // The later runs of the fixture have no instruction files.
    std::fs::remove_file(&global).unwrap();
    std::fs::remove_file(&project).unwrap();
}

/// A reply made from the placeholder a request carries for the address in
/// its input.
struct WithPlaceholder(Box<dyn Fn(&str) -> ResponseTemplate + Send + Sync>);

impl wiremock::Respond for WithPlaceholder {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let input = content_at(request, 1);
        let placeholder = input
            .strip_prefix("Who handles ")
            .and_then(|rest| rest.strip_suffix('?'))
            .expect("the input carries the placeholder");
        (self.0)(placeholder)
    }
}

/// Host tools whose outcome repeats a string of the call's arguments. The
/// outcome goes out with that string as text of the call, so a placeholder
/// the model wrote there does not end the conversation, and the rest of the
/// outcome is the tool's own text.
async fn repeated_arguments(
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let note = directory.join("written-note.txt");
    let server = MockServer::start().await;
    mount_once(
        &server,
        1,
        Mock::given(method("POST")).respond_with(WithPlaceholder(Box::new(|placeholder| {
            reply(
                "chat",
                0,
                Some((
                    "call-glob",
                    "glob",
                    &json!({"pattern": format!("*{placeholder}*")}),
                )),
                "",
            )
        }))),
    )
    .await;
    mount_once(
        &server,
        2,
        Mock::given(method("POST")).respond_with(reply(
            "chat",
            1,
            Some((
                "call-write",
                "write_file",
                &json!({"file_path": note, "content": "kept"}),
            )),
            "",
        )),
    )
    .await;
    Mock::given(method("POST"))
        .respond_with(answer("chat", &["fixture answer"]))
        .mount(&server)
        .await;
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    // File changes are approved for the whole session, as a user can choose.
    let approval = ApprovalState {
        auto_approve: true,
        ..Default::default()
    };
    let session = build_with(&cfg, directory, Some(&owner), activity, approval)
        .await
        .unwrap();
    let view = session.privacy_view.as_ref().unwrap();
    let typed = format!("Who handles {EMAIL}?");
    let TurnResult::Done(output) = view.turn(&session.agent, &typed).await.unwrap() else {
        panic!("the turn finishes");
    };
    assert_eq!(output.final_text(), "fixture answer");

    let requests = sent(&server).await;
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|body| !body.contains(EMAIL)));
    let message = |body: &str, index: usize| -> String {
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        body["messages"][index]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let placeholder = message(&requests[0], 1)
        .strip_prefix("Who handles ")
        .and_then(|rest| rest.strip_suffix('?'))
        .unwrap()
        .to_owned();
    // The pattern came back as the model wrote it, inside the tool's own
    // words. As one raw result it would have been refused: raw text may not
    // hold a placeholder this conversation issued.
    assert_eq!(
        message(&requests[1], 3),
        format!("No files match pattern '*{placeholder}*'")
    );
    // The written path came back the same way, and the file is written.
    assert_eq!(
        message(&requests[2], 5),
        format!("Updated file {}", note.display())
    );
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "kept");
    // Both results are held piece by piece, not as one admitted text, and
    // the record holds what the tools returned.
    let sources = session.agent.recorded_sources().unwrap();
    for at in [2, 4] {
        let source = sources[at].as_ref().unwrap();
        assert!(owner.admitted(source).is_none());
        assert_eq!(owner.actual_result(source).unwrap().reached().len(), 1);
    }
    let messages = session.agent.state.lock().unwrap().messages.clone();
    assert!(
        matches!(&messages[2], Msg::Tool { content, is_error: false, .. }
        if *content == format!("No files match pattern '*{placeholder}*'"))
    );
    assert!(owner.check().is_ok());
    session.lsp.shutdown().await;
    std::fs::remove_file(&note).unwrap();
}

/// Manual compaction in a built session: the builder's own summarizer lane
/// is asked with views, the summary replaces the summarized messages, and
/// the conversation continues from it.
async fn compaction_lane(
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let server = MockServer::start().await;
    for (priority, text) in ["one", "two", "SUMMARY of the work", "three"]
        .into_iter()
        .enumerate()
    {
        Mock::given(method("POST"))
            .respond_with(answer("chat", &[text]))
            .up_to_n_times(1)
            .with_priority(priority as u8 + 1)
            .mount(&server)
            .await;
    }
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    // The owned terminal's durable checkpoints, which idle compaction needs.
    let session = build(&cfg, directory, Some(&owner), None, activity, |extras| {
        extras.cancellation_session = Some(craftsman_harness::SessionCancellation::default())
    })
    .await
    .unwrap();
    let view = session.privacy_view.as_ref().unwrap();
    let first = format!("Write to {EMAIL} about the plan.");
    view.turn(&session.agent, &first).await.unwrap();
    view.turn(&session.agent, "And then?").await.unwrap();
    let before = session.agent.state.lock().unwrap().messages.clone();
    assert_eq!(before.len(), 4);

    let counts = view
        .compact(&session.agent, &session.compaction)
        .await
        .unwrap();
    assert_eq!(counts, Some((4, 4)));
    // The record: the summary in the place of the first input, then the
    // kept messages as they were.
    let after = session.agent.state.lock().unwrap().messages.clone();
    assert_eq!(
        after[0],
        Msg::user("<conversation_summary>\nSUMMARY of the work\n</conversation_summary>")
    );
    assert_eq!(after[1..], before[1..]);
    // The summarizer was asked on the same wire with the view of the input
    // it summarized and the application's own instruction, and no tools.
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 3);
    assert!(!requests[2].contains(EMAIL));
    let asked: serde_json::Value = serde_json::from_str(&requests[2]).unwrap();
    let asked_messages = asked["messages"].as_array().unwrap();
    assert_eq!(asked_messages.len(), 3);
    assert!(
        asked_messages[0]["content"]
            .as_str()
            .unwrap()
            .starts_with("You are compacting an ongoing agent conversation")
    );
    assert!(
        asked_messages[1]["content"]
            .as_str()
            .unwrap()
            .starts_with("Write to ")
    );
    assert_eq!(
        asked_messages[2]["content"],
        "Summarize the conversation above per your instructions, as a continuation summary."
    );
    assert!(asked.get("tools").is_none());
    // Every recorded message still has its source, and the conversation
    // goes on from the summary.
    assert!(
        session
            .agent
            .recorded_sources()
            .unwrap()
            .iter()
            .all(Option::is_some)
    );
    let TurnResult::Done(output) = view.turn(&session.agent, "Go on.").await.unwrap() else {
        panic!("the conversation continues");
    };
    assert_eq!(output.final_text(), "three");
    let requests = sent(&server).await;
    let next: serde_json::Value = serde_json::from_str(&requests[3]).unwrap();
    assert_eq!(
        next["messages"][1]["content"],
        "<conversation_summary>\nSUMMARY of the work\n</conversation_summary>"
    );
    assert!(!requests[3].contains(EMAIL));
    assert!(owner.check().is_ok());
    session.lsp.shutdown().await;
}

/// The text of the message at `index` of the request a server received.
fn content_at(request: &wiremock::Request, index: usize) -> String {
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    body["messages"][index]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The root agent's reply to its first request: a task for the built-in
/// delegated agent, whose description repeats what the request's input says.
struct AsksForATask {
    background: bool,
}

impl wiremock::Respond for AsksForATask {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let description = format!("Work on this: {}", content_at(request, 1));
        reply(
            "chat",
            0,
            Some((
                "call-task",
                "task",
                &json!({
                    "description": description,
                    "subagent_type": "general-purpose",
                    "run_in_background": self.background
                }),
            )),
            "",
        )
    }
}

/// The delegated agent's final reply: it repeats its own input.
struct Reports;

impl wiremock::Respond for Reports {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        answer("chat", &[&format!("Report. {}", content_at(request, 1))])
    }
}

/// Mounts `mock` for one request, at `priority` among the server's mocks.
async fn mount_once(server: &MockServer, priority: u8, mock: Mock) {
    mock.up_to_n_times(1)
        .with_priority(priority)
        .mount(server)
        .await;
}

/// Whether a request is the root agent's: only it is offered the task tool.
fn from_root(request: &wiremock::Request) -> bool {
    String::from_utf8_lossy(&request.body).contains("\"name\":\"task\"")
}

/// Delegation in a built session: the builder's own delegated agent, its
/// provider, its gated tools and the registry. A foreground task whose agent
/// is refused a write and then reads a file, and a background task that
/// reports back through a notification.
async fn delegation(
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let contact = directory.join("contact.txt");
    std::fs::write(&contact, format!("Their number is {PHONE}.\n")).unwrap();
    let refused = directory.join("refused.txt");
    let typed = format!("Ask a helper who handles {EMAIL}.");
    let tool_names = |body: &str| -> Vec<String> {
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let message = |body: &str, index: usize| -> serde_json::Value {
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        body["messages"][index].clone()
    };

    // In the foreground, one request after another.
    let server = MockServer::start().await;
    mount_once(
        &server,
        1,
        Mock::given(method("POST")).respond_with(AsksForATask { background: false }),
    )
    .await;
    mount_once(
        &server,
        2,
        Mock::given(method("POST")).respond_with(reply(
            "chat",
            1,
            Some((
                "call-write",
                "write_file",
                &json!({"file_path": refused, "content": "x"}),
            )),
            "",
        )),
    )
    .await;
    mount_once(
        &server,
        3,
        Mock::given(method("POST")).respond_with(asks_to_read(&contact)),
    )
    .await;
    mount_once(
        &server,
        4,
        Mock::given(method("POST")).respond_with(Reports),
    )
    .await;
    mount_once(
        &server,
        5,
        Mock::given(method("POST")).respond_with(answer("chat", &["fixture answer"])),
    )
    .await;
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    let session = build(&cfg, directory, Some(&owner), None, activity, |_| {})
        .await
        .expect("a session with delegated agents is built");
    let view = session.privacy_view.as_ref().unwrap();
    let TurnResult::Done(output) = view.turn(&session.agent, &typed).await.unwrap() else {
        panic!("the turn finishes");
    };
    assert_eq!(output.final_text(), "fixture answer");

    // Five requests, each sent once, and no original in any of them.
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 5);
    for body in &requests {
        assert!(!body.contains(EMAIL) && !body.contains(PHONE));
    }
    // The delegated agent has its own system text and tools, without the
    // delegation tools, and its input is the description as the model wrote
    // it: the view the root agent was given, never the original.
    let description = message(&requests[1], 1)["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(description.starts_with("Work on this: Ask a helper who handles "));
    assert_ne!(
        message(&requests[1], 0)["content"],
        message(&requests[0], 0)["content"]
    );
    assert!(
        !message(&requests[1], 0)["content"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    let delegated_tools = tool_names(&requests[1]);
    assert!(delegated_tools.contains(&"read_file".to_owned()));
    assert!(!delegated_tools.contains(&"task".to_owned()));
    assert!(!delegated_tools.contains(&"send_message".to_owned()));
    // The write was refused by the delegated agent's own gate and never ran;
    // the refusal is what its model was told.
    assert!(!refused.exists());
    assert!(
        message(&requests[2], 3)["content"]
            .as_str()
            .unwrap()
            .contains(
                "requires user approval, but this delegated run has no interactive approval channel"
            )
    );
    // The file it read reached it as a view.
    let read = message(&requests[3], 5)["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(read.contains("Their number is ") && !read.contains(PHONE));
    // What it ended with is the task's result, in the record and on the
    // wire, exactly as the model wrote it.
    let report = format!("Report. {description}");
    assert_eq!(message(&requests[4], 3)["content"], report.as_str());
    let messages = session.agent.state.lock().unwrap().messages.clone();
    assert!(
        matches!(&messages[2], Msg::Tool { content, is_error: false, .. }
        if *content == report)
    );
    let sources = session.agent.recorded_sources().unwrap();
    let result = owner
        .actual_result(sources[2].as_ref().unwrap())
        .expect("the result has its record");
    assert_eq!(result.tool(), "task");
    assert_eq!(result.reached().len(), 1);
    assert_eq!(
        session.registry.status("general-purpose#0"),
        Some(craftsman_harness::AgentStatus::Done)
    );
    // The root agent's count holds its own two requests only.
    assert_eq!(session.usage.lock().unwrap().0, 6);
    assert!(owner.check().is_ok());
    session.lsp.shutdown().await;

    // In the background. Both agents ask the same server, so each reply is
    // tied to the agent it is for.
    let server = MockServer::start().await;
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    let mut session = build(&cfg, directory, Some(&owner), None, activity, |_| {})
        .await
        .unwrap();
    Mock::given(method("POST"))
        .and(from_root)
        .respond_with(AsksForATask { background: true })
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(|request: &wiremock::Request| !from_root(request))
        .respond_with(Reports)
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(from_root)
        .respond_with(answer("chat", &["noted"]))
        .with_priority(2)
        .mount(&server)
        .await;
    let view = session.privacy_view.as_ref().unwrap();
    view.turn(&session.agent, &typed).await.unwrap();
    // The task ends on its own. Whichever turn is active when its
    // notification arrives records it; the next turn finds it recorded.
    for _ in 0..30_000 {
        if session.registry.status("general-purpose#0")
            == Some(craftsman_harness::AgentStatus::Done)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(
        session.registry.status("general-purpose#0"),
        Some(craftsman_harness::AgentStatus::Done)
    );
    view.turn(&session.agent, "And then?").await.unwrap();

    let requests = sent(&server).await;
    for body in &requests {
        assert!(!body.contains(EMAIL) && !body.contains(PHONE));
    }
    // The delegated agent was asked once.
    assert_eq!(
        requests
            .iter()
            .filter(|body| !body.contains("\"name\":\"task\""))
            .count(),
        1
    );
    // The notification is recorded once, as an ordinary session records it,
    // and went out as the agent's framing around the delegated agent's
    // reply, exactly as the model wrote it.
    let messages = session.agent.state.lock().unwrap().messages.clone();
    let framed: Vec<&String> = messages
        .iter()
        .filter_map(|message| match message {
            Msg::User { content }
                if content.starts_with("[background agent 'general-purpose#0' completed]\n") =>
            {
                Some(content)
            }
            _ => None,
        })
        .collect();
    assert_eq!(framed.len(), 1);
    let framed = framed[0].clone();
    assert!(framed.starts_with(
        "[background agent 'general-purpose#0' completed]\nReport. Work on this: Ask a helper who handles "
    ));
    let last: serde_json::Value = serde_json::from_str(requests.last().unwrap()).unwrap();
    assert!(
        last["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|sent| sent["role"] == "user" && sent["content"] == framed.as_str())
    );
    assert!(
        session
            .agent
            .recorded_sources()
            .unwrap()
            .iter()
            .all(Option::is_some)
    );
    // The frontend's own receiver saw the same notification, as it always
    // does.
    let shown = session.notifications.drain();
    assert_eq!(shown.len(), 1);
    assert!(
        matches!(&shown[0].kind, craftsman_harness::NotificationKind::Completed(text)
        if framed.ends_with(text.as_str()))
    );
    assert!(owner.check().is_ok());
    session.lsp.shutdown().await;
}

/// A user's policy with a rule for file content: plain text, for every file.
struct PlainTextFiles;

impl crate::privacy_session::CapturedUserPolicy for PlainTextFiles {
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
    ) -> Option<crate::privacy_restore::FileDataFormat> {
        // The rule is asked about a resolved destination, never a spelling.
        assert!(destination.is_absolute());
        Some(crate::privacy_restore::FileDataFormat::PlainText)
    }
}

/// A reply made from the placeholder the request carries for the address in
/// its input.
struct FromInput(Box<dyn Fn(&str) -> ResponseTemplate + Send + Sync>);

impl wiremock::Respond for FromInput {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let input = content_at(request, 1);
        let placeholder = input
            .strip_prefix("Who handles ")
            .and_then(|rest| rest.strip_suffix('?'))
            .expect("the input carries the placeholder");
        (self.0)(placeholder)
    }
}

/// Restoration in a built session: the approval gate shows a person the
/// real content of a file write, the file gets that content, the recorded
/// call keeps the placeholder, and the person reads the reply with the value.
async fn restoration(
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let typed = format!("Who handles {EMAIL}?");
    // What the approval prompt was shown, and what it answers.
    let shown: Arc<Mutex<Vec<String>>> = Arc::default();
    let approve = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let run = |file: &'static str, with_rule: bool| {
        let (shown, approve, typed) = (shown.clone(), approve.clone(), typed.clone());
        let activity = activity.clone();
        async move {
            let note = directory.join(file);
            let server = MockServer::start().await;
            let path = note.clone();
            mount_once(
                &server,
                1,
                Mock::given(method("POST")).respond_with(FromInput(Box::new(move |placeholder| {
                    reply(
                        "chat",
                        0,
                        Some((
                            "call-write",
                            "write_file",
                            &json!({"file_path": path, "content": format!("Contact {placeholder} today.")}),
                        )),
                        "",
                    )
                }))),
            )
            .await;
            Mock::given(method("POST"))
                .respond_with(FromInput(Box::new(|placeholder| {
                    answer("chat", &[&format!("Saved it for {placeholder}.")])
                })))
                .mount(&server)
                .await;
            let cfg = config("chat", &server);
            let owner = privacy(vault, directory).await;
            let policy: Arc<dyn crate::privacy_session::CapturedUserPolicy> = match with_rule {
                true => Arc::new(PlainTextFiles),
                false => Arc::new(FixturePolicy::default()),
            };
            let selection = PrivacyViewSelection::from_user_policy(policy, owner.clone()).unwrap();
            let prompt: ApprovalPrompt = Arc::new(move |_, _, details| {
                shown.lock().unwrap().push(details.to_owned());
                match approve.load(std::sync::atomic::Ordering::SeqCst) {
                    true => ApprovalPromptResult::Approve,
                    false => ApprovalPromptResult::Reject("not this one".into()),
                }
            });
            let session = build(&cfg, directory, None, None, &activity, |extras| {
                extras.privacy_view = Some(selection);
                extras.approval_prompt = Some(prompt);
            })
            .await
            .expect("a session with a file rule is built");
            let view = session.privacy_view.as_ref().unwrap();
            let TurnResult::Done(_) = view.turn(&session.agent, &typed).await.unwrap() else {
                panic!("the turn finishes");
            };
            let requests = sent(&server).await;
            assert!(requests.iter().all(|body| !body.contains(EMAIL)));
            let messages = session.agent.state.lock().unwrap().messages.clone();
            let sources = session.agent.recorded_sources().unwrap();
            // The recorded call still holds the placeholder.
            let recorded_content = match &messages[1] {
                Msg::Assistant { tool_calls, .. } => {
                    tool_calls[0].args["content"].as_str().unwrap().to_owned()
                }
                _ => panic!("the call is recorded"),
            };
            assert!(recorded_content.contains("pv1-") && !recorded_content.contains(EMAIL));
            let result = match &messages[2] {
                Msg::Tool { content, .. } => content.clone(),
                _ => panic!("the result is recorded"),
            };
            // A person reads the reply with the value; the record keeps the
            // placeholder.
            let human = owner.human_reply(sources[3].as_ref().unwrap()).unwrap();
            assert_eq!(
                human.read(str::to_owned).unwrap(),
                format!("Saved it for {EMAIL}.")
            );
            assert!(matches!(&messages[3], Msg::Assistant { content, .. }
                if content.contains("pv1-") && !content.contains(EMAIL)));
            assert!(owner.check().is_ok());
            let undone = undo_last_write(&session, &note).await;
            session.lsp.shutdown().await;
            (note, result, undone)
        }
    };

    // Approved by the person: the prompt showed the real content, and the
    // file holds it. The file tool itself ran around the write: its
    // workspace checkpoint knows the write and takes it back.
    let (note, result, undone) = run("restored-note.txt", true).await;
    assert_eq!(undone, Some(format!("Contact {EMAIL} today.")));
    assert!(!note.exists());
    assert_eq!(result, format!("Updated file {}", note.display()));
    let details = shown.lock().unwrap().clone();
    assert_eq!(details.len(), 1);
    assert!(details[0].contains(&format!("Contact {EMAIL} today.")));
    assert!(!details[0].contains("pv1-"));

    // Rejected by the person: nothing is written.
    approve.store(false, std::sync::atomic::Ordering::SeqCst);
    let (note, result, undone) = run("rejected-note.txt", true).await;
    assert!(!note.exists() && undone.is_none());
    assert!(result.contains("not this one"), "{result}");
    assert_eq!(shown.lock().unwrap().len(), 2);

    // No rule in the user's policy: the tool refuses before anyone is
    // asked, and nothing is written.
    approve.store(true, std::sync::atomic::Ordering::SeqCst);
    let (note, result, undone) = run("unruled-note.txt", false).await;
    assert!(!note.exists() && undone.is_none());
    assert_eq!(
        result,
        crate::privacy_session::FileRefusal::NoRule.to_string()
    );
    assert_eq!(shown.lock().unwrap().len(), 2);
}

/// What the file at `note` holds, if the last workspace checkpoint of the
/// session is the file tool's own for a write to it and taking that
/// checkpoint back removes the file again.
async fn undo_last_write(session: &Session, note: &std::path::Path) -> Option<String> {
    let written = std::fs::read_to_string(note).ok()?;
    let workspace = session.workspace_checkpoints.as_ref()?;
    let known = workspace.list().await.unwrap();
    let last = known.first()?;
    assert_eq!(last.file_count, 1);
    let report = workspace.rewind(Some(&last.id), false).await.unwrap();
    assert!(report.conflicts.is_empty() && report.unrestorable.is_empty());
    assert_eq!(report.restored.len(), 1);
    assert!(note.ends_with(&report.restored[0]));
    (!note.exists()).then_some(written)
}

/// The delegated agent's first reply: it asks to write its own input, as
/// its request carries it, to a file.
struct WritesItsInput(std::path::PathBuf);

impl wiremock::Respond for WritesItsInput {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        reply(
            "chat",
            1,
            Some((
                "call-note",
                "write_file",
                &json!({"file_path": self.0, "content": content_at(request, 1)}),
            )),
            "",
        )
    }
}

/// Restoration for a delegated agent in a built session: the builder's own
/// delegated agent writes a file whose content names an address and a
/// telephone alias. Its own gate shows a person the real content, the real
/// file tool writes it with its workspace checkpoint, and the delegated
/// agent's call keeps the placeholders.
async fn delegated_restoration(
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let typed = format!("Note that {EMAIL} answers {PHONE}.");
    let shown: Arc<Mutex<Vec<String>>> = Arc::default();
    let approve = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let run = |file: &'static str, with_rule: bool| {
        let (shown, approve, typed) = (shown.clone(), approve.clone(), typed.clone());
        let activity = activity.clone();
        async move {
            let note = directory.join(file);
            let server = MockServer::start().await;
            mount_once(
                &server,
                1,
                Mock::given(method("POST")).respond_with(AsksForATask { background: false }),
            )
            .await;
            mount_once(
                &server,
                2,
                Mock::given(method("POST")).respond_with(WritesItsInput(note.clone())),
            )
            .await;
            mount_once(
                &server,
                3,
                Mock::given(method("POST")).respond_with(answer("chat", &["Noted."])),
            )
            .await;
            mount_once(
                &server,
                4,
                Mock::given(method("POST")).respond_with(answer("chat", &["fixture answer"])),
            )
            .await;
            let cfg = config("chat", &server);
            let owner = privacy(vault, directory).await;
            let policy: Arc<dyn crate::privacy_session::CapturedUserPolicy> = match with_rule {
                true => Arc::new(PlainTextFiles),
                false => Arc::new(FixturePolicy::default()),
            };
            let selection = PrivacyViewSelection::from_user_policy(policy, owner.clone()).unwrap();
            let prompt: ApprovalPrompt = Arc::new(move |_, _, details| {
                shown.lock().unwrap().push(details.to_owned());
                match approve.load(std::sync::atomic::Ordering::SeqCst) {
                    true => ApprovalPromptResult::Approve,
                    false => ApprovalPromptResult::Reject("not this one".into()),
                }
            });
            let session = build(&cfg, directory, None, None, &activity, |extras| {
                extras.privacy_view = Some(selection);
                extras.approval_prompt = Some(prompt);
            })
            .await
            .expect("a session with delegated agents and a file rule is built");
            let view = session.privacy_view.as_ref().unwrap();
            let TurnResult::Done(_) = view.turn(&session.agent, &typed).await.unwrap() else {
                panic!("the turn finishes");
            };
            let requests = sent(&server).await;
            assert_eq!(requests.len(), 4);
            for body in &requests {
                assert!(!body.contains(EMAIL) && !body.contains(PHONE));
            }
            // What the delegated agent's model was told about its write,
            // and the call it wrote, which keeps what it wrote.
            let third: serde_json::Value = serde_json::from_str(&requests[2]).unwrap();
            let told = third["messages"][3]["content"].as_str().unwrap().to_owned();
            let call = third["messages"][2]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(call.contains("pv1-"));
            assert!(owner.check().is_ok());
            let undone = undo_last_write(&session, &note).await;
            session.lsp.shutdown().await;
            (note, told, undone)
        }
    };

    // Approved at the delegated agent's gate: the person was shown the real
    // content, the address and the telephone number, and the file held it.
    // The file tool's workspace checkpoint knows the write and takes it
    // back.
    let (note, told, undone) = run("delegated-note.txt", true).await;
    let written = format!("Work on this: Note that {EMAIL} answers {PHONE}.");
    assert_eq!(undone, Some(written.clone()));
    assert!(!note.exists());
    assert_eq!(told, format!("Updated file {}", note.display()));
    let details = shown.lock().unwrap().clone();
    assert_eq!(details.len(), 1);
    assert!(details[0].contains(&written) && !details[0].contains("pv1-"));

    // Refused by the person: nothing is written.
    approve.store(false, std::sync::atomic::Ordering::SeqCst);
    let (note, told, undone) = run("delegated-rejected.txt", true).await;
    assert!(!note.exists() && undone.is_none());
    assert!(told.contains("not this one"), "{told}");
    assert_eq!(shown.lock().unwrap().len(), 2);

    // No rule in the user's policy: refused before the gate asks anyone.
    // Neither alias reaches the file as it is.
    approve.store(true, std::sync::atomic::Ordering::SeqCst);
    let (note, told, undone) = run("delegated-unruled.txt", false).await;
    assert!(!note.exists() && undone.is_none());
    assert_eq!(
        told,
        crate::privacy_session::FileRefusal::NoRule.to_string()
    );
    assert_eq!(shown.lock().unwrap().len(), 2);
}

/// The gates' automatic policy in a built session. A person approves the
/// first write with "always", which is the gate's existing switch. After
/// that the policy allows file changes: the root agent's next write and a
/// delegated agent's write are decided by that policy on their real content,
/// nobody is asked again, and the files hold the value.
async fn automatic_policy(
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let typed = format!("Who handles {EMAIL}?");
    let files = [
        "always-first.txt",
        "always-second.txt",
        "always-delegated.txt",
    ]
    .map(|name| directory.join(name));
    let server = MockServer::start().await;
    let writes = |path: std::path::PathBuf| {
        FromInput(Box::new(move |placeholder| {
            reply(
                "chat",
                0,
                Some((
                    "call-write",
                    "write_file",
                    &json!({"file_path": path, "content": format!("Contact {placeholder} today.")}),
                )),
                "",
            )
        }))
    };
    let answers = || answer("chat", &["fixture answer"]);
    let post = || Mock::given(method("POST"));
    mount_once(&server, 1, post().respond_with(writes(files[0].clone()))).await;
    mount_once(&server, 2, post().respond_with(answers())).await;
    mount_once(&server, 3, post().respond_with(writes(files[1].clone()))).await;
    mount_once(&server, 4, post().respond_with(answers())).await;
    mount_once(
        &server,
        5,
        post().respond_with(AsksForATask { background: false }),
    )
    .await;
    mount_once(
        &server,
        6,
        post().respond_with(WritesItsInput(files[2].clone())),
    )
    .await;
    mount_once(&server, 7, post().respond_with(answer("chat", &["Noted."]))).await;
    mount_once(&server, 8, post().respond_with(answers())).await;
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    let selection =
        PrivacyViewSelection::from_user_policy(Arc::new(PlainTextFiles), owner.clone()).unwrap();
    let shown: Arc<Mutex<Vec<String>>> = Arc::default();
    let asked = shown.clone();
    let prompt: ApprovalPrompt = Arc::new(move |_, _, details| {
        asked.lock().unwrap().push(details.to_owned());
        ApprovalPromptResult::ApproveAlways
    });
    let session = build(&cfg, directory, None, None, activity, |extras| {
        extras.privacy_view = Some(selection);
        extras.approval_prompt = Some(prompt);
    })
    .await
    .expect("a session with a file rule is built");
    let view = session.privacy_view.as_ref().unwrap();
    for input in [typed.as_str(), "And again?", "And with a helper?"] {
        let TurnResult::Done(_) = view.turn(&session.agent, input).await.unwrap() else {
            panic!("the turn finishes");
        };
    }
    // The person was asked once, about the real content of the first file.
    let details = shown.lock().unwrap().clone();
    assert_eq!(details.len(), 1);
    assert!(details[0].contains(&format!("Contact {EMAIL} today.")));
    // All three files hold the value: the first by the person's decision,
    // the other two by the policy's.
    let content = format!("Contact {EMAIL} today.");
    assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), content);
    assert_eq!(std::fs::read_to_string(&files[1]).unwrap(), content);
    assert_eq!(
        std::fs::read_to_string(&files[2]).unwrap(),
        format!("Work on this: Who handles {EMAIL}?")
    );
    // The root agent's two writes each carried a decision to their terminal.
    let terminals = session.agent.actual_terminal_outcomes().unwrap();
    let decided: Vec<bool> = terminals
        .iter()
        .filter(|outcome| outcome.invocation().call().name == "write_file")
        .map(|outcome| outcome.invocation().decision_source().is_some())
        .collect();
    assert_eq!(decided, [true, true]);
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 8);
    for body in &requests {
        assert!(!body.contains(EMAIL));
    }
    assert!(owner.check().is_ok());
    session.lsp.shutdown().await;
}

/// A streamed reply whose placeholder arrives in two pieces, between text
/// that is not ASCII.
fn streams_in_pieces(placeholder: &str) -> ResponseTemplate {
    let (first, second) = placeholder.split_at(placeholder.len() / 2);
    let chunk = |text: &str| json!({"choices": [{"delta": {"content": text}}]});
    let end = json!({"choices": [{"delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2}});
    ResponseTemplate::new(200).set_body_raw(
        format!(
            "data: {}\n\ndata: {}\n\ndata: {end}\n\ndata: [DONE]\n\n",
            chunk(&format!("Écris à {first}")),
            chunk(&format!("{second} — merci.")),
        ),
        "text/event-stream",
    )
}

/// The terminal's own sinks with a privacy conversation: the reply is held
/// while it streams, drawn once from the committed reply as a person may
/// read it, and never drawn for a turn that is stopping or that failed. The
/// record and every request keep the placeholder.
async fn terminal_display(directory: &std::path::Path, vault: &Vault) {
    let typed = format!("Who handles {EMAIL}?");
    let server = MockServer::start().await;
    for turn in 1..=3 {
        mount_once(
            &server,
            turn,
            Mock::given(method("POST")).respond_with(FromInput(Box::new(streams_in_pieces))),
        )
        .await;
    }
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("fixture failure"))
        .mount(&server)
        .await;
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    let mut terminal =
        crate::tui::PrivacyTerminal::build(&cfg, directory, selected_by_policy(&owner)).await;
    let shown = format!("Écris à {EMAIL} — merci.");

    // The first turn: both pieces were held, and the terminal was given the
    // committed reply once, with the value.
    let recorded = terminal.turn(&typed, false).await.unwrap();
    assert!(recorded.contains("pv1-") && !recorded.contains(EMAIL));
    let drawn = terminal.drawn();
    assert_eq!(drawn.replies, [shown.clone()]);
    assert_eq!(drawn.deltas, 0);
    assert_eq!(drawn.screen.matches(EMAIL).count(), 1);
    assert!(drawn.screen.contains("Écris à") && !drawn.screen.contains("pv1-"));

    // The second turn is drawn once more, as its own item. What the person
    // was shown did not go back to the model: no request holds the value.
    terminal.turn("And again?", false).await.unwrap();
    let drawn = terminal.drawn();
    assert_eq!(drawn.replies, [shown.clone(), shown.clone()]);
    assert_eq!(drawn.deltas, 0);

    // A turn that is stopping draws nothing more, although its reply was
    // committed and recorded.
    let recorded = terminal.turn("And once more?", true).await.unwrap();
    assert!(recorded.contains("pv1-"));
    assert_eq!(terminal.drawn().replies.len(), 2);

    // A view of the first reply, made while the conversation is live.
    let first_reply = terminal.session().agent.recorded_sources().unwrap()[1]
        .clone()
        .unwrap();
    let late = owner.human_reply(&first_reply).unwrap();

    // A turn whose model request fails draws no reply.
    assert!(terminal.turn("And now?", false).await.is_err());
    let drawn = terminal.drawn();
    assert_eq!(drawn.replies.len(), 2);
    assert_eq!(drawn.deltas, 0);
    assert!(!drawn.screen.contains("pv1-"));

    // The record keeps what the model wrote, and no request held a value.
    for message in terminal
        .session()
        .agent
        .state
        .lock()
        .unwrap()
        .messages
        .iter()
    {
        if let Msg::Assistant { content, .. } = message {
            assert!(content.contains("pv1-") && !content.contains(EMAIL));
        }
    }
    for body in sent(&server).await {
        assert!(!body.contains(EMAIL));
    }

    // Once the conversation has stopped, a view that was made earlier is
    // not read any more, and a reply without a view shows nothing of
    // itself: a fixed line stands where the text would be.
    owner.stop();
    use crate::privacy_session::{HumanRefusal, HumanReply};
    terminal.show(HumanReply::Text(late));
    terminal.show(HumanReply::Withheld(HumanRefusal::NotAReply));
    let drawn = terminal.drawn();
    assert_eq!(drawn.replies.len(), 2);
    let fixed = "A reply of this private conversation could not be shown.";
    assert_eq!(
        drawn.notices.iter().filter(|line| *line == fixed).count(),
        2
    );
    assert_eq!(drawn.screen.matches(EMAIL).count(), 2);
    terminal.session().lsp.shutdown().await;
}

/// The same conversation drawn on the terminal this process was started
/// on: one turn, then one frame by the application's own draw.
#[cfg(unix)]
async fn terminal_on_a_pty(directory: &std::path::Path, vault: &Vault) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(FromInput(Box::new(streams_in_pieces)))
        .mount(&server)
        .await;
    let cfg = config("chat", &server);
    let owner = privacy(vault, directory).await;
    let mut terminal =
        crate::tui::PrivacyTerminal::build(&cfg, directory, selected_by_policy(&owner)).await;
    let recorded = terminal
        .turn(&format!("Who handles {EMAIL}?"), false)
        .await
        .unwrap();
    assert!(recorded.contains("pv1-") && !recorded.contains(EMAIL));
    terminal.draw_on_terminal(&cfg).unwrap();
    for body in sent(&server).await {
        assert!(!body.contains(EMAIL));
    }
    terminal.session().lsp.shutdown().await;
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
    let late = selected_by_policy(&ended);
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
        let selected = selected_by_policy(&owner);
        let report = selected.report();
        assert!(
            build(&cfg, directory, None, thread, activity, |extras| {
                extras.privacy_view = Some(selected);
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

    // The selection a policy makes leaves nothing out. With the ordinary
    // configuration every installed part has a source path, so the session
    // is built with all of them and nothing is reported.
    let selected = selected_by_policy(&owner);
    let report = selected.report();
    let built = build(&cfg, directory, None, None, activity, |extras| {
        extras.privacy_view = Some(selected)
    })
    .await
    .expect("the ordinary configuration is built");
    assert_eq!(report.refusal(), None);
    built.lsp.shutdown().await;
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

/// Everything a child wrote to the terminal it was started on: a newly
/// owned PTY, 40 rows of 120 columns, which is its input and its output.
#[cfg(unix)]
async fn run_on_a_pty(
    arguments: &[String],
    directory: &std::path::Path,
    home: &std::path::Path,
) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let (mut master, mut slave) = (-1, -1);
    let mut size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    assert_eq!(opened, 0);
    let mut master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .env("HOME", home)
        .env("CRAFTSMAN_CONFIG_DIR", home.join("config"))
        .env("TERM", "xterm-256color")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave))
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let mut child = command.spawn().unwrap();
    // The child holds the only other ends of the PTY now.
    drop(command);
    let drawn = std::thread::spawn(move || {
        let mut drawn = Vec::new();
        let mut piece = [0u8; 4096];
        while let Ok(count) = master.read(&mut piece) {
            if count == 0 || drawn.len() > 1 << 20 {
                break;
            }
            drawn.extend_from_slice(&piece[..count]);
        }
        drawn
    });
    let mut stderr = child.stderr.take().unwrap().take(65537);
    let mut error = Vec::new();
    let finished = tokio::time::timeout(Duration::from_secs(180), async {
        tokio::try_join!(child.wait(), stderr.read_to_end(&mut error))
    })
    .await;
    let Ok(Ok((status, _))) = finished else {
        let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
        panic!("private terminal fixture exceeded its bound");
    };
    (status, drawn.join().unwrap(), error)
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_terminal_draws_a_committed_reply_once_on_a_pty_through_the_safe_backend() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(temp.path().join("config")).unwrap();
    let (status, drawn, error) =
        run_on_a_pty(&[format!("{MARKER}terminal")], &workspace, temp.path()).await;
    let error = String::from_utf8_lossy(&error);
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
    // What reached the terminal: the reply as a person may read it, once,
    // and no placeholder.
    let drawn = String::from_utf8_lossy(&drawn);
    assert_eq!(drawn.matches(EMAIL).count(), 1, "{drawn:?}");
    assert!(drawn.contains("Écris à") && drawn.contains("merci."));
    assert!(!drawn.contains("pv1-") && !drawn.contains("privacy.invalid"));
    // The status reports beside it are fixed words: nothing of the reply.
    let reports: Vec<&str> = drawn.split("\u{1b}]7501;").skip(1).collect();
    assert!(!reports.is_empty());
    for report in reports {
        let report = report.split("\u{1b}\\").next().unwrap();
        assert!(
            !report.contains('@') && !report.contains("merci"),
            "{report:?}"
        );
    }
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
