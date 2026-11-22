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
const CHECK_HELPER: &str = "agent::privacy_view_tests::privacy_view_check";
const CHECK_MARKER: &str = "craftsman-privacy-check=";
const WIRES: [&str; 4] = ["chat", "responses", "claude", "gemini"];

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
    if selected.len() != 1
        || !(matches!(
            selected[0].as_str(),
            "chat" | "controls" | "verification" | "scheduled" | "stopped"
        ) || on_a_pty)
    {
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
    // The configured controllers of a session, on every wire: the context
    // and failure controller, and with it the verification controller.
    if selected[0] != "chat" {
        for wire in WIRES {
            match selected[0].as_str() {
                "controls" => controls(wire, &directory, &vault, &activity).await,
                "verification" => verification(wire, false, &directory, &vault, &activity).await,
                "scheduled" => verification(wire, true, &directory, &vault, &activity).await,
                _ => {
                    stopped(wire, true, &directory, &vault, &activity).await;
                    stopped(wire, false, &directory, &vault, &activity).await;
                }
            }
        }
        eprintln!("{DONE}");
        return;
    }
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

/// The declared check of the controls case: a real subprocess that reads
/// the one file its marker names and leaves with a real status.
#[test]
#[ignore = "owned check of the private-view fixture; an unmarked or unknown start returns before any access"]
fn privacy_view_check() {
    let named: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(CHECK_MARKER))
                .map(str::to_owned)
        })
        .collect();
    if named.len() != 1
        || !WIRES.iter().any(|wire| {
            ["asked", "scheduled", "denied", "failed"]
                .iter()
                .any(|case| named[0] == format!("{case}-{wire}/value.txt"))
        })
    {
        return;
    }
    let value = std::fs::read_to_string(&named[0]).unwrap();
    println!("checked value: {value}");
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    if !value.starts_with("fixed") {
        std::process::exit(7);
    }
}

/// The command that runs [`privacy_view_check`] on `file` in the shell the
/// session's own runner uses, with the check's real exit status.
fn check_command(file: &str) -> String {
    let binary = std::env::current_exe().unwrap();
    let binary = binary.to_str().unwrap();
    #[cfg(windows)]
    {
        format!(
            "$craftsman_check = [System.Diagnostics.Process]::new(); $craftsman_check.StartInfo.FileName = '{}'; $craftsman_check.StartInfo.Arguments = '--ignored --exact {CHECK_HELPER} --nocapture {CHECK_MARKER}{file}'; $craftsman_check.StartInfo.WorkingDirectory = [Environment]::CurrentDirectory; $craftsman_check.StartInfo.UseShellExecute = $false; $craftsman_check.StartInfo.RedirectStandardOutput = $true; $craftsman_check.StartInfo.RedirectStandardError = $true; [void]$craftsman_check.Start(); $craftsman_check_stdout = $craftsman_check.StandardOutput.ReadToEnd(); $craftsman_check_stderr = $craftsman_check.StandardError.ReadToEnd(); $craftsman_check.WaitForExit(); $craftsman_check_exit = $craftsman_check.ExitCode; $craftsman_check.Dispose(); [Console]::Out.Write($craftsman_check_stdout); [Console]::Error.Write($craftsman_check_stderr); [Console]::Out.Flush(); [Console]::Error.Flush(); [Environment]::Exit($craftsman_check_exit)",
            binary.replace("'", "''")
        )
    }
    #[cfg(not(windows))]
    {
        format!(
            "'{}' --ignored --exact {CHECK_HELPER} --nocapture {CHECK_MARKER}{file}",
            binary.replace('\'', "'\\''")
        )
    }
}

/// The configured context and failure controller in a built privacy
/// conversation on one wire: the session's own builder makes the original
/// controller from the user's settings and installs it behind the
/// conversation's adapter.
///
/// A person asks about a file. The model reads it, asks twice for a file
/// that is not there, and answers. The controller records the input and the
/// observed path under its own keys; the model is given the same context as
/// parts, each seen as its own source is seen, and the failure controls'
/// advisory after the same error came back twice. Nothing is paused.
async fn controls(
    wire: &'static str,
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let folder = format!("noted-{wire}");
    std::fs::create_dir(directory.join(&folder)).unwrap();
    let notes = format!("{folder}/notes.txt");
    let missing = format!("{folder}/missing.txt");
    std::fs::write(
        directory.join(&notes),
        format!("Their number is {PHONE}.\n"),
    )
    .unwrap();
    let server = MockServer::start().await;
    let read = |file: &str| json!({"file_path": directory.join(file)});
    let scripted = [
        reply(
            wire,
            0,
            Some(("call-notes", "read_file", &read(&notes))),
            "",
        ),
        reply(
            wire,
            1,
            Some(("call-first", "read_file", &read(&missing))),
            "",
        ),
        reply(
            wire,
            2,
            Some(("call-again", "read_file", &read(&missing))),
            "",
        ),
        reply(wire, 3, None, "nobody else"),
    ];
    for (place, scripted) in scripted.into_iter().enumerate() {
        mount_once(
            &server,
            place as u8 + 1,
            Mock::given(method("POST")).respond_with(scripted),
        )
        .await;
    }
    Mock::given(method("POST"))
        .respond_with(reply(wire, 4, None, "welcome"))
        .mount(&server)
        .await;
    let mut cfg = config(wire, &server);
    cfg.extra.insert("context".into(), json!({"enabled": true}));
    cfg.extra
        .insert("failure_control".into(), json!({"repeat_limit": 5}));
    let owner = privacy(vault, directory).await;
    let session = build(&cfg, directory, Some(&owner), None, activity, |extras| {
        extras.verification = true;
        extras.context_compilation = true;
        extras.failure_controls = true;
        extras.task_workflow = true;
        // The owned terminal's durable checkpoints, which idle compaction
        // needs.
        extras.cancellation_session = Some(craftsman_harness::SessionCancellation::default());
    })
    .await
    .unwrap_or_else(|| panic!("the configured controller is built on {wire}"));
    // The session keeps the original controller, as any session does, and
    // no verification controller: none is declared.
    let original = session.controls.clone().expect("the original controller");
    assert!(session.verification.is_none());
    let view = session.privacy_view.as_ref().unwrap();
    let typed = format!("Who handles {EMAIL}? See the notes.");
    let TurnResult::Done(output) = view.turn(&session.agent, &typed).await.unwrap() else {
        panic!("the turn finishes on {wire}");
    };
    assert_eq!(output.final_text(), "nobody else", "{wire}");

    // The record is exact.
    let messages = session.agent.state.lock().unwrap().messages.clone();
    assert_eq!(messages.len(), 8, "{wire}: {messages:?}");
    assert_eq!(messages[0], Msg::user(typed.clone()));
    assert!(
        matches!(&messages[2], Msg::Tool { content, is_error, .. }
            if content.contains(PHONE) && !is_error),
        "{wire}"
    );
    let errors: Vec<&String> = [&messages[4], &messages[6]]
        .into_iter()
        .map(|message| match message {
            Msg::Tool {
                content,
                is_error: true,
                ..
            } => content,
            other => panic!("a missing file is an error on {wire}: {other:?}"),
        })
        .collect();
    assert_eq!(errors[0], errors[1], "{wire}");

    // The original controller kept its own bookkeeping beside the record:
    // the context record with the input as typed and the path as the call
    // wrote it, and the stamp of the failure controls. Nothing else of the
    // record changed for it.
    let custom = session.agent.state.lock().unwrap().custom.clone();
    let mut keys: Vec<&str> = custom
        .keys()
        .map(String::as_str)
        .filter(|key| key.starts_with("_craftsman_"))
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, {
        let mut own = original.own_keys();
        own.sort_unstable();
        own
    });
    assert_eq!(
        keys,
        ["_craftsman_context_v1", "_craftsman_failure_input_v1"]
    );
    let record = &custom["_craftsman_context_v1"];
    assert_eq!(record["initial_input"]["text"], json!(typed));
    let noted = directory.join(&notes);
    assert_eq!(
        record["files"][0]["selector"],
        json!(noted.to_str().unwrap()),
        "{wire}"
    );
    assert_eq!(
        custom["_craftsman_failure_input_v1"]["stamp"]["revision"],
        json!(original.recorded_input().unwrap().revision)
    );
    // The failure controls saw the same error twice and paused nothing.
    assert!(original.confirmed_pause().is_none());

    // The agent's own events: one accepted input, for the adapter the
    // conversation made, and a source beside every recorded message.
    let events = session.agent.actual_boundary_inputs().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].in_callback());
    let sources = session.agent.recorded_sources().unwrap();
    assert!(sources.len() == 8 && sources.iter().all(Option::is_some));

    let requests = sent(&server).await;
    assert_eq!(requests.len(), 4, "{wire}");
    for body in &requests {
        // Nothing private left the process, and the section quotes the
        // input as the input itself is seen.
        assert!(!body.contains(EMAIL) && !body.contains(PHONE), "{wire}");
        assert!(body.contains("Task context from earlier user inputs"));
        assert!(body.contains("Initial request (event 1):"), "{wire}");
        assert!(body.matches("Who handles ").count() >= 2, "{wire}");
        // Digests and sizes of recorded text are no part of it.
        assert!(!body.contains("original_bytes") && !body.contains("result_digest"));
    }
    // A path is named once the observer saw the call that wrote it.
    assert!(!requests[0].contains("File observations"));
    for body in &requests[1..] {
        assert!(
            body.contains("File observations") && body.contains(" | read | completed=true | "),
            "{wire}"
        );
    }
    // The advisory is the application's own fixed text, after the second
    // identical error and not before.
    let advisory = "returned the same error repeatedly";
    assert!(requests[..3].iter().all(|body| !body.contains(advisory)));
    assert!(requests[3].contains(advisory), "{wire}");

    // The conversation continues: a second input is accepted by the same
    // controller and quoted beside the first.
    let TurnResult::Done(output) = view.turn(&session.agent, "Thanks.").await.unwrap() else {
        panic!("the second turn finishes on {wire}");
    };
    assert_eq!(output.final_text(), "welcome");
    assert_eq!(session.agent.actual_boundary_inputs().unwrap().len(), 2);
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 5);
    let last = requests.last().unwrap();
    assert!(
        last.contains("Initial request (event 1):") && last.contains("Recent user input (event")
    );
    assert!(!last.contains(EMAIL) && !last.contains(PHONE));
    assert!(last.contains("known_compactions\\\":0"), "{wire}");
    assert!(owner.check().is_ok(), "{wire}");

    // A manual compaction through the conversation's typed entry. The
    // agent tells the installed controller of the real replacement, once,
    // before its final save, and the original controller records it under
    // its own key: one known compaction, and every observed file uncertain.
    let before = session.agent.state.lock().unwrap().messages.clone();
    assert_eq!(before.len(), 10);
    let counts = view
        .compact(&session.agent, &session.compaction)
        .await
        .unwrap_or_else(|error| panic!("the conversation is compacted on {wire}: {error}"))
        .expect("something was replaced");
    let after = session.agent.state.lock().unwrap().messages.clone();
    assert_eq!(counts, (before.len(), after.len()), "{wire}");
    assert!(matches!(&after[0], Msg::User { content }
        if content == "<conversation_summary>\nwelcome\n</conversation_summary>"));
    assert!(after[1..].iter().all(|kept| before.contains(kept)));
    let compactions = session.agent.actual_idle_compactions().unwrap();
    assert_eq!(compactions.len(), 1);
    assert!(!compactions[0].in_callback());
    assert_eq!(compactions[0].before().len(), before.len());
    assert_eq!(compactions[0].after().len(), after.len());
    let custom = session.agent.state.lock().unwrap().custom.clone();
    let record = &custom["_craftsman_context_v1"];
    assert_eq!(record["compactions"], json!(1), "{wire}");
    assert_eq!(record["uncertain_compactions"], json!(0));
    assert_eq!(record["initial_input"]["text"], json!(typed));
    let files = record["files"].as_array().unwrap();
    assert!(!files.is_empty());
    assert!(
        files.iter().all(|file| file["freshness"] == "uncertain"),
        "{wire}"
    );
    let sources = session.agent.recorded_sources().unwrap();
    assert!(sources.len() == after.len() && sources.iter().all(Option::is_some));
    assert!(original.confirmed_pause().is_none());

    // The conversation goes on from the summary. The first input is no
    // longer a recorded message, and the section still quotes it from the
    // source of its accepted-input event, as its own view.
    let TurnResult::Done(output) = view.turn(&session.agent, "Go on.").await.unwrap() else {
        panic!("the conversation continues on {wire}");
    };
    assert_eq!(output.final_text(), "welcome");
    let requests = sent(&server).await;
    // The summarizer's request, and the request of the new turn.
    assert_eq!(requests.len(), 7, "{wire}");
    for body in &requests[5..] {
        assert!(!body.contains(EMAIL) && !body.contains(PHONE), "{wire}");
    }
    let last = requests.last().unwrap();
    assert!(last.contains("known_compactions\\\":1"), "{wire}");
    assert!(last.contains("Initial request (event 1):"), "{wire}");
    assert!(last.contains("Who handles "), "{wire}");
    assert!(last.contains("conversation_summary"), "{wire}");
    assert_eq!(session.agent.actual_boundary_inputs().unwrap().len(), 3);
    assert!(owner.check().is_ok(), "{wire}");
    assert!(
        !activity
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("This private conversation ")),
        "{wire}"
    );
    session.lsp.shutdown().await;
}

/// The verification controller stops a turn in a built privacy conversation
/// on one wire, after the model answered and the controller scheduled its
/// check: because a person turned the check down (`denied`), or because the
/// real check failed and no repair round is allowed.
///
/// The turn returns the controller's own refusal. The agent's fixed note of
/// it is recorded with its source, and so is everything before it; the
/// check's real result is recorded exactly. A turn that ends with an error
/// ends the conversation, so nothing of it is sent to a model afterwards.
async fn stopped(
    wire: &'static str,
    denied: bool,
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let folder = match denied {
        true => format!("denied-{wire}"),
        false => format!("failed-{wire}"),
    };
    std::fs::create_dir(directory.join(&folder)).unwrap();
    let value = format!("{folder}/value.txt");
    let note = format!("{folder}/note.txt");
    std::fs::write(directory.join(&value), format!("broken for {PHONE}")).unwrap();
    std::fs::write(directory.join(&note), "draft").unwrap();
    let server = MockServer::start().await;
    // A denied check would have passed: the model fixes the value. A failed
    // check finds the value as it was: the model edits another source.
    let edit = match denied {
        true => json!({"file_path": value, "old_string": "broken", "new_string": "fixed"}),
        false => json!({"file_path": note, "old_string": "draft", "new_string": "final"}),
    };
    let scripted = [
        reply(wire, 0, Some(("call-edit", "edit_file", &edit)), ""),
        reply(wire, 1, None, "edited"),
    ];
    for (place, scripted) in scripted.into_iter().enumerate() {
        mount_once(
            &server,
            place as u8 + 1,
            Mock::given(method("POST")).respond_with(scripted),
        )
        .await;
    }
    Mock::given(method("POST"))
        .respond_with(reply(wire, 2, None, "never asked"))
        .mount(&server)
        .await;
    let mut cfg = config(wire, &server);
    cfg.extra.insert(
        "verification".into(),
        json!({"commands": [check_command(&value)], "paths": [folder], "max_repair_rounds": 0}),
    );
    cfg.extra.insert("context".into(), json!({"enabled": true}));
    cfg.extra
        .insert("failure_control".into(), json!({"repeat_limit": 5}));
    let owner = privacy(vault, directory).await;
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let prompt: ApprovalPrompt = {
        let asked = asked.clone();
        Arc::new(move |tool, _, _| {
            asked.lock().unwrap().push(tool.to_owned());
            match denied && tool == "verify" {
                true => ApprovalPromptResult::Reject("not yet".into()),
                false => ApprovalPromptResult::Approve,
            }
        })
    };
    let session = build(&cfg, directory, Some(&owner), None, activity, |extras| {
        extras.verification = true;
        extras.context_compilation = true;
        extras.failure_controls = true;
        extras.task_workflow = true;
        extras.approval_prompt = Some(prompt);
    })
    .await
    .unwrap_or_else(|| panic!("the configured controllers are built on {wire}"));
    let view = session.privacy_view.as_ref().unwrap();
    let typed = format!("Finish the work for {EMAIL}.");

    // The turn returns the original controller's own refusal.
    let reason = match view.turn(&session.agent, &typed).await {
        Err(PrivateEntryError::Agent(AgentError::State(reason))) => reason,
        other => panic!(
            "the controller stops the turn on {wire}: {:?}",
            other.map(|_| ())
        ),
    };
    let expected = match denied {
        true => {
            "Required verification did not run; review its permissions or policy before continuing."
        }
        false => {
            "Verification still failed after the allowed repair rounds. Completed work and actual check results are retained."
        }
    };
    assert_eq!(reason, expected, "{wire}");
    assert_eq!(*asked.lock().unwrap(), ["edit_file", "verify"], "{wire}");

    // The record is exact and complete: the input, the edit and its result,
    // the answer, the call the controller scheduled, what came back for it,
    // and the agent's fixed note.
    let messages = session.agent.state.lock().unwrap().messages.clone();
    assert_eq!(messages.len(), 7, "{wire}: {messages:?}");
    assert_eq!(messages[0], Msg::user(typed.clone()));
    assert!(
        matches!(&messages[4], Msg::Assistant { content, tool_calls }
        if content == "Running the declared completion check."
            && tool_calls.len() == 1 && tool_calls[0].name == "verify")
    );
    let Msg::Tool {
        content: evidence,
        is_error: true,
        ..
    } = &messages[5]
    else {
        panic!("the check did not pass on {wire}: {:?}", messages[5]);
    };
    match denied {
        // The person's own words, and no run of the check.
        true => assert_eq!(evidence, "Tool call denied by the user. Feedback: not yet"),
        // The real command, its real status and its real output.
        false => assert!(
            evidence.starts_with(&format!(
                "Verification did not pass.\nCheck: {}\nExit: 7\n",
                check_command(&value)
            )) && evidence.contains(&format!("checked value: broken for {PHONE}\n")),
            "{wire}: {evidence}"
        ),
    }
    assert_eq!(
        messages[6],
        Msg::assistant(
            "The declared completion check did not accept this turn. Completed effects and check results are retained."
        )
    );
    // What was done stays done.
    assert_eq!(
        std::fs::read_to_string(directory.join(&value)).unwrap(),
        match denied {
            true => format!("fixed for {PHONE}"),
            false => format!("broken for {PHONE}"),
        }
    );
    assert_eq!(
        std::fs::read_to_string(directory.join(&note)).unwrap(),
        if denied { "draft" } else { "final" }
    );

    // Every recorded message has its source. The note's is the agent's own
    // event for it: fixed text of the host, and no reply of a model.
    let sources = session.agent.recorded_sources().unwrap();
    assert_eq!(sources.len(), 7);
    assert!(sources.iter().all(Option::is_some), "{wire}");
    let noted = sources[6].as_ref().unwrap();
    assert_eq!(
        owner.admitted(noted).unwrap().origin(),
        RawOrigin::HostStatic
    );
    assert!(owner.committed_model(noted).is_none());
    let stops = session
        .agent
        .actual_retained_commits()
        .unwrap()
        .iter()
        .filter(|commit| {
            commit.event()
                == craftsman_harness::provenance::RetainedEvent::Framework(
                    craftsman_harness::provenance::FrameworkCommit::CompletionStop,
                )
        })
        .count();
    assert_eq!(stops, 1, "{wire}");
    assert_eq!(
        session.agent.actual_host_scheduled_calls().unwrap().len(),
        1
    );
    let result = owner
        .actual_result(sources[5].as_ref().unwrap())
        .expect("the check's result has its record");
    assert_eq!(result.reached().len(), usize::from(!denied));
    assert!(result.operation().is_none());
    // The one failure the agent kept for this turn is the refusal itself:
    // no source was missing.
    let failures = session.agent.actual_entry_failures().unwrap();
    assert_eq!(failures.len(), 1, "{wire}: {failures:?}");
    assert!(failures[0].contains(expected), "{wire}: {failures:?}");

    // Two requests were sent, both before the check, and neither holds a
    // private value. The check's result never went to a model.
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 2, "{wire}");
    for body in &requests {
        assert!(!body.contains(EMAIL) && !body.contains(PHONE), "{wire}");
        assert!(!body.contains("Verification did not pass") && !body.contains("not yet"));
    }

    // A turn that ended with an error ends the conversation. Nothing more
    // is recorded or sent.
    assert!(owner.check().is_err(), "{wire}");
    assert!(matches!(
        view.turn(&session.agent, "And now?").await,
        Err(PrivateEntryError::Stopped(_))
    ));
    assert_eq!(session.agent.state.lock().unwrap().messages.len(), 7);
    assert_eq!(sent(&server).await.len(), 2);
    session.lsp.shutdown().await;
}

/// The configured verification, context and failure controllers in a built
/// privacy conversation on one wire, with the terminal's kind of extras: the
/// session's own builder makes the original controllers from the user's
/// settings and installs them behind the conversation's adapters.
///
/// A person asks for a fix. The model edits a source file and a protected
/// file. Then the declared check runs: either the model asks for it itself
/// before it answers, or it answers and the verification controller
/// schedules the check (`scheduled`). A person reviews the protected change
/// as it really is, the real command runs, and the model is asked once more
/// with the check's result. Everything recorded is exact; everything sent is
/// a view.
async fn verification(
    wire: &'static str,
    scheduled: bool,
    directory: &std::path::Path,
    vault: &Vault,
    activity: &Arc<Mutex<Vec<String>>>,
) {
    let folder = match scheduled {
        true => format!("scheduled-{wire}"),
        false => format!("asked-{wire}"),
    };
    std::fs::create_dir(directory.join(&folder)).unwrap();
    let value = format!("{folder}/value.txt");
    let guard = format!("{folder}/guard.txt");
    std::fs::write(directory.join(&value), format!("broken for {PHONE}")).unwrap();
    std::fs::write(
        directory.join(&guard),
        format!("guard original for {PHONE}"),
    )
    .unwrap();
    let server = MockServer::start().await;
    let edit = |file: &str, old: &str, new: &str| json!({"file_path": file, "old_string": old, "new_string": new});
    let scripted = [
        reply(
            wire,
            0,
            Some(("call-value", "edit_file", &edit(&value, "broken", "fixed"))),
            "",
        ),
        reply(
            wire,
            1,
            Some((
                "call-guard",
                "edit_file",
                &edit(&guard, "original", "updated"),
            )),
            "",
        ),
        match scheduled {
            true => reply(wire, 2, None, "edited"),
            false => reply(wire, 2, Some(("call-verify", "verify", &json!({}))), ""),
        },
        reply(wire, 3, None, "verified"),
    ];
    for (place, scripted) in scripted.into_iter().enumerate() {
        mount_once(
            &server,
            place as u8 + 1,
            Mock::given(method("POST")).respond_with(scripted),
        )
        .await;
    }
    Mock::given(method("POST"))
        .respond_with(reply(wire, 4, None, "welcome"))
        .mount(&server)
        .await;
    let mut cfg = config(wire, &server);
    let command = check_command(&value);
    cfg.extra.insert(
        "verification".into(),
        json!({"commands": [command], "paths": [folder], "protected_paths": [guard]}),
    );
    cfg.extra.insert("context".into(), json!({"enabled": true}));
    cfg.extra.insert(
        "failure_control".into(),
        json!({"repeat_limit": 5, "test_review_signals": true}),
    );
    let owner = privacy(vault, directory).await;
    // What a person was asked to approve, with the details they were shown.
    let asked: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let prompt: ApprovalPrompt = {
        let asked = asked.clone();
        Arc::new(move |tool, _, details| {
            asked
                .lock()
                .unwrap()
                .push((tool.to_owned(), details.to_owned()));
            ApprovalPromptResult::Approve
        })
    };
    let session = build(&cfg, directory, Some(&owner), None, activity, |extras| {
        extras.verification = true;
        extras.context_compilation = true;
        extras.failure_controls = true;
        extras.task_workflow = true;
        extras.approval_prompt = Some(prompt);
    })
    .await
    .unwrap_or_else(|| panic!("the configured controllers are built on {wire}"));
    // The session keeps the original controllers, as any session does.
    assert!(session.verification.is_some() && session.controls.is_some());
    let view = session.privacy_view.as_ref().unwrap();
    let typed = format!("Fix the value for {EMAIL}.");
    let TurnResult::Done(output) = view.turn(&session.agent, &typed).await.unwrap() else {
        panic!("the turn finishes on {wire}");
    };
    assert_eq!(output.final_text(), "verified", "{wire}");

    // The record is exact: the input as typed, both edits, the call for the
    // check, the check's real result and the answer that followed it. A
    // scheduled check comes after the model's answer, in the agent's own
    // message with the call the controller directed.
    let messages = session.agent.state.lock().unwrap().messages.clone();
    let carrier = messages.len() - 3;
    assert_eq!(
        carrier,
        if scheduled { 6 } else { 5 },
        "{wire}: {messages:?}"
    );
    assert_eq!(messages[0], Msg::user(typed.clone()));
    let check = match &messages[carrier] {
        Msg::Assistant {
            content,
            tool_calls,
        } => {
            if scheduled {
                assert_eq!(content, "Running the declared completion check.");
            }
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].name, "verify");
            assert_eq!(tool_calls[0].args, json!({}));
            tool_calls[0].id.clone()
        }
        other => panic!("the call for the check is recorded on {wire}: {other:?}"),
    };
    let Msg::Tool {
        tool_call_id,
        content: evidence,
        is_error,
    } = &messages[carrier + 1]
    else {
        panic!("the check's result is recorded on {wire}");
    };
    assert_eq!(tool_call_id, &check);
    assert!(!is_error, "{wire}: {evidence}");
    // The real command, its real status and its real output, which quotes
    // the source it checked.
    assert!(
        evidence.starts_with(&format!(
            "Verification passed.\nCheck: {}\nExit: 0\n",
            check_command(&value)
        )) && evidence.contains(&format!("checked value: fixed for {PHONE}\n")),
        "{wire}: {evidence}"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join(&value)).unwrap(),
        format!("fixed for {PHONE}")
    );
    assert_eq!(
        std::fs::read_to_string(directory.join(&guard)).unwrap(),
        format!("guard updated for {PHONE}")
    );

    // A person was asked three times: for each edit, and for the check with
    // the protected file as it really was and is. Nothing was waived.
    let asked = asked.lock().unwrap().clone();
    let tools: Vec<&str> = asked.iter().map(|(tool, _)| tool.as_str()).collect();
    assert_eq!(tools, ["edit_file", "edit_file", "verify"], "{wire}");
    let review = &asked[2].1;
    assert!(
        review.contains(&format!("Protected file: {guard}"))
            && review.contains(&format!("guard original for {PHONE}"))
            && review.contains(&format!("guard updated for {PHONE}")),
        "{wire}: {review}"
    );

    // The controllers kept their own bookkeeping, and only that, beside the
    // record: the context record with the input as typed, and the stamp of
    // the failure controls.
    let custom = session.agent.state.lock().unwrap().custom.clone();
    let mut keys: Vec<&str> = custom
        .keys()
        .map(String::as_str)
        .filter(|key| key.starts_with("_craftsman_"))
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["_craftsman_context_v1", "_craftsman_failure_input_v1"],
        "{wire}"
    );
    assert_eq!(
        custom["_craftsman_context_v1"]["initial_input"]["text"],
        json!(typed)
    );
    let observed: Vec<&str> = custom["_craftsman_context_v1"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["selector"].as_str().unwrap())
        .collect();
    assert_eq!(observed, [value.as_str(), guard.as_str()], "{wire}");

    // The agent's own events: one accepted input for the installed
    // controller and, for a scheduled check, one call the installed
    // completion controller scheduled. Every recorded message has its
    // source, the agent's message for that call included.
    assert_eq!(session.agent.actual_boundary_inputs().unwrap().len(), 1);
    let hosted = session.agent.actual_host_scheduled_calls().unwrap();
    assert_eq!(hosted.len(), usize::from(scheduled));
    if scheduled {
        assert_eq!(hosted[0].call().id, check);
    }
    let sources = session.agent.recorded_sources().unwrap();
    assert_eq!(sources.len(), messages.len());
    assert!(sources.iter().all(Option::is_some), "{wire}");
    // A scheduled call ran as that and has no operation of a reply. A call
    // the model asked for has the operation of its reply.
    let result = owner
        .actual_result(sources[carrier + 1].as_ref().unwrap())
        .expect("the check's result has its record");
    assert_eq!(result.tool(), "verify");
    assert_eq!(result.reached().len(), 1);
    assert_eq!(result.operation().is_none(), scheduled);
    // The agent's own event for a scheduled call is accepted once, by the
    // conversation it was scheduled in, and by no other.
    if scheduled {
        use craftsman_harness::provenance::ModelViewBoundary;
        assert!(
            view.boundary()
                .observe_host_scheduled(&hosted[0])
                .await
                .is_err()
        );
        let other = privacy(vault, directory).await;
        let foreign = other.boundary(&[]).unwrap();
        assert!(foreign.observe_host_scheduled(&hosted[0]).await.is_err());
        assert!(owner.check().is_ok() && other.check().is_ok());
    }

    // Nothing private left the process in any request.
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 4, "{wire}");
    for body in &requests {
        assert!(!body.contains(EMAIL) && !body.contains(PHONE), "{wire}");
        // Every request carries the context section with the first input
        // quoted as its own view, and the note of declared verification.
        assert!(body.contains("Task context from earlier user inputs"));
        assert!(body.contains("Initial request (event 1):"), "{wire}");
        assert!(body.matches("Fix the value for ").count() >= 2, "{wire}");
        assert!(body.contains("Declared verification is enabled."));
        // Digests and sizes of recorded text are no part of that section.
        assert!(!body.contains("original_bytes") && !body.contains("result_digest"));
    }
    // After the first edit the section names the path as the call wrote it.
    assert!(!requests[0].contains("File observations"));
    assert!(requests[1].contains("File observations") && requests[1].contains(" | edit | "));
    // The request after the check holds the call for it and the check's
    // result as a view: the real output with the number replaced.
    let after = &requests[3];
    assert_eq!(
        after.contains("Running the declared completion check."),
        scheduled
    );
    assert!(after.contains(&check) && after.contains("verify"), "{wire}");
    assert!(after.contains("Verification passed.") && after.contains("checked value: fixed for "));
    for earlier in &requests[..3] {
        assert!(!earlier.contains("Verification passed."));
    }
    assert!(owner.check().is_ok(), "{wire}");

    // The conversation continues: a second input is accepted by the same
    // controller and quoted beside the first.
    let TurnResult::Done(output) = view.turn(&session.agent, "Thanks.").await.unwrap() else {
        panic!("the second turn finishes on {wire}");
    };
    assert_eq!(output.final_text(), "welcome");
    assert_eq!(session.agent.actual_boundary_inputs().unwrap().len(), 2);
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 5);
    let last = requests.last().unwrap();
    assert!(
        last.contains("Initial request (event 1):") && last.contains("Recent user input (event")
    );
    assert!(!last.contains(EMAIL) && !last.contains(PHONE));
    assert!(owner.check().is_ok(), "{wire}");
    assert!(
        !activity
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("This private conversation ")),
        "{wire}"
    );
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
    let late = selected_by_policy(&ended);
    ended.stop();
    assert_eq!(
        PrivacyViewSelection::from_user_policy(policy(false, false), ended.clone()).err(),
        Some(BuildRefusal::Ended)
    );

    // A caller's thread id, a separate guard, and an owner that ended after
    // the selection was made. Declared verification, context compilation and
    // failure controls are requested parts that have their adapters now: the
    // `controls` case builds and runs them, and they are not refused here.
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
    assert_eq!(count(fixed), 3);
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

    // The three parts a terminal always requests are not refused either.
    // With nothing declared for them in the user's settings there is no
    // controller to install, exactly as in an ordinary session.
    let requested = privacy(vault, directory).await;
    let selected = selected_by_policy(&requested);
    let report = selected.report();
    let built = build(&cfg, directory, None, None, activity, |extras| {
        extras.privacy_view = Some(selected);
        extras.verification = true;
        extras.context_compilation = true;
        extras.failure_controls = true;
    })
    .await
    .expect("the requested parts are built");
    assert_eq!(report.refusal(), None);
    assert!(built.verification.is_none() && built.controls.is_none());
    built.lsp.shutdown().await;
    assert!(requested.check().is_ok());
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

async fn run_marked(mode: &str) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(temp.path().join("config")).unwrap();
    let output = run_fixture(&[format!("{MARKER}{mode}")], &workspace, temp.path()).await;
    assert!(output.stdout.len() <= 65536 && output.stderr.len() <= 65536);
    let error = std::str::from_utf8(&output.stderr).unwrap();
    assert!(output.status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
}

#[tokio::test]
async fn actual_builder_runs_context_and_failure_controls_in_a_privacy_conversation_on_four_wires()
{
    run_marked("controls").await;
}

#[tokio::test]
async fn actual_builder_runs_declared_verification_in_a_privacy_conversation_on_four_wires() {
    run_marked("verification").await;
}

/// The check the verification controller schedules itself, after the
/// model's answer.
#[tokio::test]
async fn actual_builder_runs_the_check_the_verification_controller_schedules_on_four_wires() {
    run_marked("scheduled").await;
}

/// A stop by the verification controller: a check a person turned down, and
/// a check that failed with no repair round left.
#[tokio::test]
async fn actual_builder_ends_a_privacy_conversation_at_a_verification_stop_on_four_wires() {
    run_marked("stopped").await;
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
