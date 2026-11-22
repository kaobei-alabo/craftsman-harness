//! Marked actual shared-builder/loopback-HTTP proof for the private send guard.
//! Real vault mappings come from host-supplied detector reports in an owned
//! child directory. No detector, masking, restoration, key source, frontend or
//! model service is involved, and no selection exists outside these fixtures.
use super::*;
use crate::privacy_send::tests::{EMAIL, answer, guard_for, mask, open};
use crate::privacy_send::{SendGuard, SendRefusal};
use crate::privacy_vault::Category;
use craftsman_harness::{Msg, TurnResult};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "agent::privacy_send_tests::privacy_send_builder_fixture";
const MARKER: &str = "craftsman-privacy-send=";
const DONE: &str = "actual builder, loopback HTTP and private send guard assertions completed";
const CLEAN: &str = "An ordinary question with no personal data.";
const CONTINUATION: &str = "This conversation is bound to a private send guard. Reopening or transferring it is not available.";

fn config(protocol: &str, server: &MockServer) -> Config {
    let name = match protocol {
        "chat" | "lanes" => {
            return serde_json::from_value(json!({"base_url":server.uri(),"api_key":"fixture-key","model":"fixture-model","harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},"mcp_servers":[],"lsp_servers":{}})).unwrap();
        }
        "responses" => "openai_responses",
        "claude" => "anthropic_messages",
        _ => "gemini_generate_content",
    };
    serde_json::from_value(json!({"model":"test/fixture-model","providers":{"test":{"protocol":name,"base_url":server.uri(),"api_key":"fixture-key","models":{"fixture-model":{"settings":{"reasoning":null}}}}},"harness_profile":"general","voice":{"enabled":false},"gadgets":{"enabled":false},"mcp_servers":[],"lsp_servers":{}})).unwrap()
}

async fn build(
    cfg: &Config,
    directory: &std::path::Path,
    guard: Option<Arc<SendGuard>>,
    thread_id: Option<String>,
    activity: Arc<Mutex<Vec<String>>>,
) -> Option<Session> {
    build_session_ext(
        cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        thread_id,
        SessionExtras {
            send_guard: guard,
            workspace_root: Some(directory.to_path_buf()),
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(move |line: &str| {
                activity.lock().unwrap().push(line.to_owned())
            })),
            ..Default::default()
        },
    )
    .await
}

fn refused(result: Result<TurnResult, AgentError>) {
    match result {
        Err(AgentError::Model(text)) => {
            assert_eq!(text, SendRefusal::KnownOriginal.to_string());
            assert!(!text.contains(EMAIL));
        }
        _ => panic!("the guarded turn must end with the fixed private refusal"),
    }
}

fn answered(result: Result<TurnResult, AgentError>, expected: &str) {
    let Ok(TurnResult::Done(output)) = result else {
        panic!("the turn must complete")
    };
    assert_eq!(output.final_text(), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned private-send fixture; unmarked/unknown startup returns before config access"]
async fn privacy_send_builder_fixture() {
    let selected: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if selected.len() != 1
        || !matches!(
            selected[0].as_str(),
            "chat" | "responses" | "claude" | "gemini" | "lanes"
        )
    {
        return;
    }
    let mode = selected[0].as_str();
    let directory = std::env::current_dir().unwrap();
    let vault_root = directory.parent().unwrap().join("vault");
    std::fs::create_dir(&vault_root).unwrap();
    let (_vault, mapping) = open(&vault_root, "builder-thread").await;
    let masked = mask(&mapping, &[(EMAIL, Category::Email)]).await;
    let guard = guard_for(&mapping, &masked);
    let dirty = format!("Please write to {EMAIL} about the fixture.");
    let server = MockServer::start().await;
    let activity = Arc::new(Mutex::new(Vec::new()));
    if mode == "lanes" {
        lanes(&directory, &server, guard, &dirty, activity).await;
    } else {
        protocol(mode, &directory, &server, guard, &dirty, activity).await;
    }
    eprintln!("{DONE}");
}

/// One wire: identical clean bytes with and without a selection, an absent
/// selection that still sends everything, and a selected session that sends
/// nothing for a known original and refuses reopen and transfer.
async fn protocol(
    protocol: &str,
    directory: &std::path::Path,
    server: &MockServer,
    guard: Arc<SendGuard>,
    dirty: &str,
    activity: Arc<Mutex<Vec<String>>>,
) {
    Mock::given(method("POST"))
        .respond_with(answer(protocol, &["fixture answer"]))
        .mount(server)
        .await;
    let cfg = config(protocol, server);
    let mut absent = build(&cfg, directory, None, None, activity.clone())
        .await
        .unwrap();
    let mut bound = build(&cfg, directory, Some(guard.clone()), None, activity.clone())
        .await
        .unwrap();
    assert!(absent.send_guard.is_none());
    assert!(Arc::ptr_eq(bound.send_guard.as_ref().unwrap(), &guard));

    answered(absent.agent.run_turn(CLEAN).await, "fixture answer");
    answered(bound.agent.run_turn(CLEAN).await, "fixture answer");
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].body, received[1].body);
    assert_eq!(received[0].url, received[1].url);
    for name in ["content-type", "accept"] {
        assert_eq!(received[0].headers.get(name), received[1].headers.get(name));
        assert!(received[1].headers.get(name).is_some());
    }
    assert_eq!(*absent.usage.lock().unwrap(), (3, 2));
    assert_eq!(*bound.usage.lock().unwrap(), (3, 2));

    // Absent selection: the old path sends the same text unchanged.
    answered(absent.agent.run_turn(dirty).await, "fixture answer");
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 3);
    assert!(
        received[2]
            .body
            .windows(EMAIL.len())
            .any(|window| window == EMAIL.as_bytes())
    );
    assert_eq!(*absent.usage.lock().unwrap(), (6, 4));

    // Selected: nothing leaves, and no usage is invented.
    refused(bound.agent.run_turn(dirty).await);
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert_eq!(*bound.usage.lock().unwrap(), (3, 2));

    // Reopen and transfer are refused before any state is touched.
    let before = bound.agent.state.lock().unwrap().messages.clone();
    let thread = bound.thread_id.clone();
    for error in [
        bound.load_thread(&thread).await.err(),
        absent.inherit_conversation(&bound, None).err(),
        bound.inherit_conversation(&absent, None).err(),
    ] {
        match error {
            Some(AgentError::State(text)) => assert_eq!(text, CONTINUATION),
            _ => panic!("a bound conversation must refuse reopen and transfer"),
        }
    }
    assert_eq!(bound.agent.state.lock().unwrap().messages, before);
    assert_eq!(bound.thread_id, thread);
    assert_eq!(absent.agent.state.lock().unwrap().messages.len(), 4);
    activity.lock().unwrap().clear();
    assert!(
        build(
            &cfg,
            directory,
            Some(guard.clone()),
            Some(thread.clone()),
            activity.clone()
        )
        .await
        .is_none()
    );
    guard.close();
    assert!(
        build(&cfg, directory, Some(guard), None, activity.clone())
            .await
            .is_none()
    );
    assert_eq!(
        *activity.lock().unwrap(),
        ["The private send guard is not available for this session."; 2]
    );
    // An absent selection still reopens its own stored conversation.
    let own = absent.thread_id.clone();
    assert!(absent.load_thread(&own).await.is_ok());
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    absent.lsp.shutdown().await;
    bound.lsp.shutdown().await;
}

/// The compaction and delegated clients of one selected session carry the same
/// guard as its main client.
async fn lanes(
    directory: &std::path::Path,
    server: &MockServer,
    guard: Arc<SendGuard>,
    dirty: &str,
    activity: Arc<Mutex<Vec<String>>>,
) {
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let task = json!({"description":dirty,"subagent_type":"general-purpose"});
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            match observed.fetch_add(1, Ordering::SeqCst) {
                0 => answer("chat", &["fixture summary"]),
                1 => {
                    let delta = json!({"tool_calls":[{"index":0,"id":"call-task","type":"function","function":{"name":"task","arguments":task.to_string()}}]});
                    let body = json!({"choices":[{"delta":delta,"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
                    ResponseTemplate::new(200).set_body_raw(
                        format!("data: {body}\n\ndata: [DONE]\n\n"),
                        "text/event-stream",
                    )
                }
                _ => answer("chat", &["a request that must never be sent"]),
            }
        })
        .mount(server)
        .await;
    let cfg = config("lanes", server);
    let session = build(&cfg, directory, Some(guard), None, activity)
        .await
        .unwrap();

    // Compaction client: a clean transcript is summarized over the wire; the
    // same transcript holding a known original is refused, and the loopback
    // server receives no further request.
    let mut transcript: Vec<Msg> = (0..5)
        .flat_map(|turn| {
            [
                Msg::user(format!("ordinary request {turn}")),
                Msg::assistant(format!("ordinary result {turn}")),
            ]
        })
        .collect();
    let compacted = session
        .compaction
        .compact_now(&transcript)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(&compacted[0], Msg::User { content } if content.contains("fixture summary")));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    transcript[2] = Msg::user(dirty);
    match session.compaction.compact_now(&transcript).await {
        Err(AgentError::Model(text)) => {
            assert_eq!(text, SendRefusal::KnownOriginal.to_string())
        }
        _ => panic!("the compaction client must carry the selected guard"),
    }
    assert_eq!(requests.load(Ordering::SeqCst), 1);

    // Delegated client: the main model's own clean request is sent once and
    // answers with a delegation whose task text holds the known original. The
    // child's request and the main follow-up are both refused, and neither
    // reaches the loopback server. The first request and its usage remain.
    refused(session.agent.run_turn(CLEAN).await);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    let state = session.agent.state.lock().unwrap().clone();
    let delegated = state
        .messages
        .iter()
        .find_map(|message| match message {
            Msg::Tool {
                tool_call_id,
                content,
                is_error: true,
            } if tool_call_id == "call-task" => Some(content.clone()),
            _ => None,
        })
        .expect("the delegated call returned its own refusal");
    assert!(delegated.contains(&SendRefusal::KnownOriginal.to_string()));
    assert!(!delegated.contains(EMAIL));
    assert_eq!(*session.usage.lock().unwrap(), (3, 2));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    session.lsp.shutdown().await;
}

async fn run_case(mode: &str) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let config = temp.path().join("config");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}{mode}"))
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", temp.path())
        .env("USERPROFILE", temp.path())
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
        Ok::<_, std::io::Error>((status, out, err))
    })
    .await;
    let (status, out, err) = match result {
        Ok(Ok(value)) => value,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("private send fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
}

#[tokio::test]
async fn actual_chat_builder_seals_clean_bytes_and_sends_nothing_for_a_known_original() {
    run_case("chat").await;
}
#[tokio::test]
async fn actual_responses_builder_seals_clean_bytes_and_sends_nothing_for_a_known_original() {
    run_case("responses").await;
}
#[tokio::test]
async fn actual_claude_builder_seals_clean_bytes_and_sends_nothing_for_a_known_original() {
    run_case("claude").await;
}
#[tokio::test]
async fn actual_gemini_builder_seals_clean_bytes_and_sends_nothing_for_a_known_original() {
    run_case("gemini").await;
}
#[tokio::test]
async fn actual_compaction_and_delegated_clients_share_the_selected_guard() {
    run_case("lanes").await;
}

#[tokio::test]
async fn unmarked_or_unknown_private_send_fixture_returns_before_any_access() {
    // The helper body itself: no marker, an unknown marker and a duplicate all
    // return before a vault, server, configuration or directory is touched.
    let temp = tempfile::tempdir().unwrap();
    for arguments in [
        vec![],
        vec![format!("{MARKER}unknown")],
        vec![format!("{MARKER}chat"), format!("{MARKER}chat")],
    ] {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", HELPER, "--nocapture"])
            .args(&arguments)
            .current_dir(temp.path())
            .env_clear()
            .env("HOME", temp.path())
            .env("USERPROFILE", temp.path())
            .env("CRAFTSMAN_CONFIG_DIR", temp.path())
            .envs(std::env::var_os("PATH").map(|path| ("PATH", path)))
            .envs(std::env::var_os("SystemRoot").map(|root| ("SystemRoot", root)))
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(DONE));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn the_private_prepared_construction_refuses_a_selection_before_effects() {
    use crate::exec::construction::{Options, Prepared, SessionSelection};
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("work");
    let config = root.path().join("selected");
    let vault_root = root.path().join("vault");
    for directory in [&workspace, &config, &vault_root] {
        std::fs::create_dir(directory).unwrap();
    }
    let server = MockServer::start().await;
    std::fs::write(
        config.join("config.json"),
        json!({"model":"synthetic","api_key":"synthetic-secret","base_url":server.uri()})
            .to_string(),
    )
    .unwrap();
    let prepared = Prepared::prepare_with_environment_for_tests(
        Options {
            workspace,
            config_dir: config.clone(),
            session: SessionSelection::Ephemeral,
            trust_workspace: false,
            selectors: Default::default(),
        },
        BTreeMap::new(),
    )
    .await
    .unwrap();
    let (_vault, mapping) = open(&vault_root, "prepared-thread").await;
    let masked = mask(&mapping, &[(EMAIL, Category::Email)]).await;
    let gate = Arc::new(crate::exec::runtime::InvocationGate::captured(&prepared));
    let meter = crate::exec::meter::Meter::from_scope(RequestMeterScope {
        gate: gate.clone(),
        limit: Default::default(),
        token_limit: None,
    });
    let activity = Arc::new(Mutex::new(Vec::new()));
    let observed = activity.clone();
    // The same typed core the private runtime uses, with its own valid gate
    // and meter: only the selection makes it refuse.
    let refused = build_session_with_profile(
        SessionConstruction::Headless {
            prepared: &prepared,
            gate: &gate,
        },
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        true,
        false,
        None,
        SessionExtras {
            send_guard: Some(guard_for(&mapping, &masked)),
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(move |line: &str| {
                observed.lock().unwrap().push(line.to_owned())
            })),
            ..Default::default()
        },
        Some(meter),
    )
    .await;
    assert!(refused.is_none());
    assert_eq!(
        *activity.lock().unwrap(),
        ["The private send guard is not available for this session."]
    );
    assert!(!config.join("threads.db").exists());
    assert!(server.received_requests().await.unwrap().is_empty());
}
