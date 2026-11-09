use super::*;
use crate::tui::session_bridge::UiBridge;
use craftsman_harness::Checkpointer;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn effort_only_selection_keeps_the_model_and_unrelated_configuration() {
    let cfg:Config=serde_json::from_value(json!({"model":"a","api_key":"synthetic-key","extra-kept":{"nested":true},"model_settings":{"b":{"reasoning_effort":"low"}}})).unwrap();
    let selected = selected_config(
        &cfg,
        ModelSelection {
            model: None,
            effort: Some(Some(ReasoningEffort::High)),
        },
    )
    .unwrap();
    assert_eq!(selected.model, cfg.model);
    assert_eq!(selected.api_key, cfg.api_key);
    assert_eq!(selected.extra["extra-kept"], cfg.extra["extra-kept"]);
    assert_eq!(
        selected.extra["model_settings"]["b"],
        cfg.extra["model_settings"]["b"]
    );
    assert_eq!(
        crate::model_controls::display_effort(&selected).as_deref(),
        Some("high")
    );
    assert!(!cfg.extra["model_settings"].get("a").is_some());
}

#[test]
fn configured_model_picker_carries_effort_choices_and_close_does_not_apply_them() {
    let cfg: Config = serde_json::from_value(json!({"model":"a"})).unwrap();
    let mut app = App::new(&cfg);
    show_model_catalog(
        &mut app,
        vec![CatalogModel {
            id: "a".into(),
            label: "Coding model".into(),
            reasoning_efforts: vec![ReasoningEffort::Low, ReasoningEffort::High],
        }],
        Some("a"),
    );
    let Some(Modal::Selection(picker)) = &app.modal else {
        panic!("catalog picker missing");
    };
    assert_eq!(picker.items.len(), 3);
    assert!(
        matches!(&picker.items[2].enter_action,Some(AppAction::SelectModelEffort {model,effort:Some(ReasoningEffort::High)}) if model=="a")
    );
    app.modal = None;
    assert_eq!(cfg.model.as_deref(), Some("a"));
    assert!(
        crate::model_controls::selected_effort(&cfg, "a")
            .unwrap()
            .is_none()
    );
}

const HELPER: &str = "tui::model_commands::tests::terminal_model_fixture";
#[test]
fn cancelled_catalog_delivery_does_not_open_a_late_model_picker() {
    let cfg: Config = serde_json::from_value(json!({"model":"a"})).unwrap();
    let mut app = App::new(&cfg);
    app.running = true;
    app.cancel_requested = true;
    crate::tui::session_bridge::handle_ui_event(
        &mut app,
        crate::tui::session_bridge::UiEvent::ModelCatalogLoaded {
            current: Some("a".into()),
            result: Ok((vec![CatalogModel::named("a".into())], true)),
        },
        None,
    );
    assert!(app.modal.is_none());
    assert!(!app.running);
    assert!(!app.cancel_requested);
}

const MARKER: &str = "craftsman-model-controls-fixture-v1=";

fn marker_directory() -> Option<PathBuf> {
    let args: Vec<_> = std::env::args_os().collect();
    let marked: Vec<_> = args
        .iter()
        .filter_map(|arg| arg.to_str().and_then(|arg| arg.strip_prefix(MARKER)))
        .collect();
    if marked.len() != 1 {
        return None;
    }
    let directory = PathBuf::from(marked[0]);
    directory.is_absolute().then_some(directory)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned config/model fixture; refuses unmarked startup before path/config access"]
async fn terminal_model_fixture() {
    let Some(directory) = marker_directory() else {
        return;
    };
    let server = MockServer::start().await;
    let samples = Arc::new(AtomicUsize::new(0));
    let sample_count = samples.clone();
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(move |request:&wiremock::Request| {
        let body:Value=serde_json::from_slice(&request.body).unwrap();
        let index=sample_count.fetch_add(1,Ordering::SeqCst);
        let (delta,reason)=if index==0 {
            assert_eq!(body["model"],"model-a");
            (json!({"tool_calls":[{"index":0,"id":"actual-write","type":"function","function":{"name":"write_file","arguments":json!({"file_path":"proof.txt","content":"one actual write"}).to_string()}}]}),"tool_calls")
        } else if index==1 {
            assert_eq!(body["model"],"model-a"); (json!({"content":"remember-alpha"}),"stop")
        } else {
            assert_eq!(body["model"],"model-b");assert_eq!(body["reasoning_effort"],"high");
            assert!(body["messages"].as_array().unwrap().iter().any(|message|message["content"]=="remember-alpha"));
            assert!(body["messages"].as_array().unwrap().iter().any(|message|message["tool_call_id"]=="actual-write"));
            (json!({"content":"continued-beta"}),"stop")
        };
        let event=json!({"choices":[{"delta":delta,"finish_reason":reason}],"usage":{"prompt_tokens":5,"completion_tokens":2}});
        ResponseTemplate::new(200).set_body_raw(format!("data: {event}\n\ndata: [DONE]\n\n"),"text/event-stream")
    }).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":[{"id":"model-b","reasoning_efforts":["low","high"]},{"id":"model-a"}]}),
        ))
        .mount(&server)
        .await;
    let mut cfg:Config=serde_json::from_value(json!({"base_url":format!("{}/v1",server.uri()),"api_key":"synthetic-model-key","model":"model-a","harness_profile":"general","model_catalog":[{"id":"model-b","reasoning_efforts":["low","high"]}]})).unwrap();
    let approval = Arc::new(Mutex::new(ApprovalState {
        auto_approve: true,
        mode: crate::config::PermissionMode::BypassPermissions,
        ..Default::default()
    }));
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let root = craftsman_harness::SessionCancellation::new();
    let mut slot = AgentSlot::spawn(&cfg, approval.clone(), UiBridge { tx }, root.clone());
    let old = slot.get().await.as_ref().unwrap();
    old.agent
        .run_turn("Write the fixture, then remember alpha.")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(directory.join("proof.txt")).unwrap(),
        "one actual write"
    );
    let thread = old.thread_id.clone();
    let usage = *old.usage.lock().unwrap();
    let before = old
        .checkpointer
        .as_ref()
        .unwrap()
        .load_latest(&thread)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(usage, (10, 4));
    assert_eq!(samples.load(Ordering::SeqCst), 2);
    let mut app = App::new(&cfg);
    select_model(
        &mut cfg,
        ModelSelection {
            model: Some("model-b".into()),
            effort: Some(Some(ReasoningEffort::High)),
        },
        &mut slot,
        &approval,
        &mut app,
    );
    let new = slot.get().await.as_ref().unwrap();
    assert_eq!(new.thread_id, thread);
    assert_eq!(*new.usage.lock().unwrap(), usage);
    assert_eq!(
        samples.load(Ordering::SeqCst),
        2,
        "reconfiguration must not sample or replay"
    );
    new.agent
        .run_turn("Continue the same conversation.")
        .await
        .unwrap();
    let after = new
        .checkpointer
        .as_ref()
        .unwrap()
        .load_latest(&thread)
        .await
        .unwrap()
        .unwrap();
    assert!(after.0 > before);
    assert!(
        after
            .1
            .messages
            .iter()
            .any(|message| message == &craftsman_harness::Msg::assistant("remember-alpha"))
    );
    assert_eq!(*new.usage.lock().unwrap(), (15, 6));
    assert_eq!(samples.load(Ordering::SeqCst), 3);
    assert_eq!(
        std::fs::read_to_string(directory.join("proof.txt")).unwrap(),
        "one actual write"
    );
    let catalog_turn = slot.fetch_model_catalog(cfg.clone());
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let crate::tui::session_bridge::UiEvent::ModelCatalogLoaded { result, .. } = event {
            let (models, available) = result.unwrap();
            assert!(available);
            assert_eq!(
                models
                    .iter()
                    .find(|m| m.id == "model-b")
                    .unwrap()
                    .reasoning_efforts,
                vec![ReasoningEffort::Low, ReasoningEffort::High]
            );
            break;
        }
    }
    assert!(!catalog_turn.is_cancelled());
    // A model catalog request has its own cancellable owner; stopping it must
    // not cancel the active conversation or wait for the delayed HTTP body.
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":[]}))
                .set_delay(Duration::from_secs(20)),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let before_requests = server.received_requests().await.unwrap().len();
    let lookup = slot.fetch_model_catalog(cfg.clone());
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.received_requests().await.unwrap().len() == before_requests {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    lookup.cancel();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let crate::tui::session_bridge::UiEvent::ModelCatalogLoaded { result, .. } = event {
            assert!(result.is_err());
            break;
        }
    }
    assert!(!slot.cancellation().is_cancelled());
    // Initialization failure retains the conversation but closes old sampling;
    // it cannot silently use the old model under the new selection.
    let mut invalid = cfg.clone();
    invalid.base_url.clear();
    invalid.platform = Default::default();
    slot.rebuild(&invalid, approval.clone());
    let retained = slot.get().await.as_ref().unwrap();
    assert_eq!(retained.thread_id, thread);
    assert!(
        retained
            .agent
            .run_turn("must not use the old model")
            .await
            .is_err()
    );
    assert_eq!(samples.load(Ordering::SeqCst), 3);
    slot.rebuild(&cfg, approval.clone());
    let restored = slot.get().await.as_ref().unwrap();
    assert_eq!(restored.thread_id, thread);
    assert!(!slot.cancellation().is_cancelled());
    root.cancel();
    slot.shutdown().await;
    eprintln!("actual terminal model continuation and effort assertions completed");
}

#[tokio::test]
async fn actual_terminal_builder_switches_model_effort_and_preserves_durable_conversation() {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            HELPER,
            "--test-threads=1",
            "--nocapture",
            "--skip",
        ])
        .arg(format!("{MARKER}{}", directory.path().to_str().unwrap()))
        .current_dir(directory.path())
        .env_clear()
        .env("CRAFTSMAN_CONFIG_DIR", &config)
        .env("HOME", directory.path())
        .env("USERPROFILE", directory.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        let mut out = vec![];
        let mut err = vec![];
        let mut out_pipe = stdout.take(65537);
        let mut err_pipe = stderr.take(65537);
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            out_pipe.read_to_end(&mut out),
            err_pipe.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    })
    .await;
    let (status, out, err) = match outcome {
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned terminal model fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let err = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{err}");
    assert_eq!(
        err.lines()
            .filter(|line| *line
                == "actual terminal model continuation and effort assertions completed")
            .count(),
        1
    );
}
