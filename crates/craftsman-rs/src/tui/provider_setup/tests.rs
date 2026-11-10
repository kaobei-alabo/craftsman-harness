use super::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

fn config() -> Config {
    serde_json::from_value(json!({"model":"a/original","providers":{
        "a":{"protocol":"openai_responses","base_url":"http://127.0.0.1:1/v1","api_key":"original-key","models":{"original":{"settings":{"reasoning":{"type":"effort","value":"high"}},"reasoning_controls":{"type":"effort","values":["high"]}}}},
        "b":{"protocol":"anthropic_messages","api_key":"${UNSELECTED_SETUP_CREDENTIAL}","models":{"other":{"settings":{"reasoning":null}}}}
    },"base_url":"http://legacy.invalid/v1","api_key":"legacy-key","retained-user-field":{"value":7}})).unwrap()
}

#[test]
fn scoped_candidate_keeps_other_connections_models_settings_and_unknown_user_fields() {
    let cfg = config();
    let updated = candidate(
        &cfg,
        "a",
        "https://example.com/v1/".into(),
        "new-key".into(),
        "original".into(),
    )
    .unwrap();
    assert_eq!(updated.providers["b"], cfg.providers["b"]);
    assert_eq!(updated.providers["a"].models, cfg.providers["a"].models);
    assert_eq!(updated.extra, cfg.extra);
    assert_eq!(updated.base_url, cfg.base_url);
    assert_eq!(updated.api_key, cfg.api_key);
    assert_eq!(updated.providers["a"].base_url, "https://example.com/v1");
    assert_eq!(updated.providers["a"].api_key, "new-key");
    let new = candidate(
        &cfg,
        "a",
        "https://example.com/v1".into(),
        "new-key".into(),
        "org/new-model".into(),
    )
    .unwrap();
    assert_eq!(new.model.as_deref(), Some("a/org/new-model"));
    assert_eq!(
        new.providers["a"].models["original"],
        cfg.providers["a"].models["original"]
    );
    assert!(
        new.providers["a"].models["org/new-model"]
            .settings
            .is_empty()
    );
    assert_eq!(cfg.model.as_deref(), Some("a/original"));
}

#[test]
fn invalid_candidate_refuses_before_mutating_current_config_or_resolving_other_credentials() {
    let cfg = config();
    let original = serde_json::to_value(&cfg).unwrap();
    for (name, url, key, model) in [
        ("missing", "https://example.com", "key".into(), "m"),
        ("a", "https://user:secret@example.com", "key".into(), "m"),
        ("a", "file:///tmp/provider", "key".into(), "m"),
        ("a", "https://example.com", "key".into(), "model with space"),
        ("a", "https://example.com", "x".repeat(16385), "m"),
    ] {
        assert!(candidate(&cfg, name, url.into(), key, model.into()).is_err());
    }
    assert_eq!(serde_json::to_value(&cfg).unwrap(), original);
    let mut full = cfg.clone();
    full.providers.get_mut("a").unwrap().models = (0..511)
        .map(|n| (format!("m{n}"), CatalogEntry::default()))
        .collect();
    assert!(
        candidate(
            &full,
            "a",
            "https://example.com".into(),
            "key".into(),
            "new".into()
        )
        .is_err()
    );
}

#[test]
fn provider_setup_masks_credentials_preserves_drafts_and_refuses_whole_oversized_paste() {
    let cfg = config();
    let mut app = App::new(&cfg);
    show(&cfg, &mut app);
    let Some(Modal::Selection(picker)) = &app.modal else {
        panic!("connection picker missing")
    };
    assert_eq!(picker.items.len(), 2);
    assert_eq!(
        picker.items[0].enter_action,
        Some(AppAction::EditProvider("a".into()))
    );
    edit(&cfg, "a", &mut app);
    let Some(Modal::Setup(modal)) = &mut app.modal else {
        panic!("provider setup missing")
    };
    modal.field = 1;
    assert_eq!(modal.fields[1], "original-key");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(130, 35)).unwrap();
    terminal
        .draw(|frame| crate::tui::modals::render_modal(frame, app.modal.as_ref().unwrap()))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(!rendered.contains("original-key"));
    app.running = true;
    crate::tui::input::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
    let Some(Modal::Setup(modal)) = &app.modal else {
        panic!("setup draft was discarded by cancellation")
    };
    assert_eq!(modal.fields[1], "original-key");
    app.running = false;
    crate::tui::input::handle_terminal_event(&mut app, Event::Paste("x".repeat(16385)));
    let Some(Modal::Setup(modal)) = &app.modal else {
        panic!("draft lost")
    };
    assert_eq!(modal.fields[1], "original-key");
    crate::tui::modals::handle_modal_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    crate::tui::input::handle_terminal_event(&mut app, Event::Paste("new-key\r\n".into()));
    let Some(Modal::Setup(modal)) = &mut app.modal else {
        panic!("draft lost")
    };
    assert_eq!(modal.fields[1], "new-key");
    modal.field = 2;
    modal.fields[2] = "org/new-model".into();
    crate::tui::modals::handle_modal_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(
        matches!(&app.pending_action,Some(AppAction::SaveProviderSetup{provider,model,api_key,..})if provider=="a"&&model=="org/new-model"&&api_key=="new-key")
    );
}

const HELPER: &str = "tui::provider_setup::tests::owned_provider_setup_fixture";
const MARKER: &str = "craftsman-provider-setup-fixture=";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned provider setup fixture; unmarked returns before path/config access"]
async fn owned_provider_setup_fixture() {
    use crate::tui::session_bridge::UiBridge;
    use craftsman_harness::Checkpointer;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let markers: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if markers.len() != 1 {
        return;
    }
    let directory = std::path::PathBuf::from(&markers[0]);
    if !directory.is_absolute() {
        return;
    }
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    Mock::given(method("POST")).and(path("/first/responses")).and(header("authorization","Bearer original-key")).respond_with(move|request:&wiremock::Request|{
        let body:Value=serde_json::from_slice(&request.body).unwrap();assert_eq!(body["model"],"original");assert_eq!(body["reasoning"]["effort"],"high");
        let items=if counter.fetch_add(1,Ordering::SeqCst)==0 {vec![json!({"type":"reasoning","id":"setup-reasoning","encrypted_content":"original-opaque-setup","summary":[]}),json!({"type":"function_call","call_id":"setup-write","name":"write_file","arguments":json!({"file_path":"proof.txt","content":"one setup effect"}).to_string()})]}else{vec![json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"original task completed"}]})]};
        ResponseTemplate::new(200).set_body_raw(format!("data: {}\n\n",json!({"type":"response.completed","response":{"status":"completed","output":items,"usage":{"input_tokens":3,"output_tokens":2}}})),"text/event-stream")
    }).expect(2).mount(&server).await;
    let counter = requests.clone();
    Mock::given(method("POST")).and(path("/second/responses")).and(header("authorization","Bearer new-key")).respond_with(move|request:&wiremock::Request|{
        let body:Value=serde_json::from_slice(&request.body).unwrap();assert_eq!(body["model"],"org/new-model");assert!(body.get("reasoning").is_none());
        let text=serde_json::to_string(&body).unwrap();assert!(text.contains("original task completed")&&text.contains("setup-write"));assert!(!text.contains("original-opaque-setup"));counter.fetch_add(1,Ordering::SeqCst);
        ResponseTemplate::new(200).set_body_raw(format!("data: {}\n\n",json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"new connection continued"}]}],"usage":{"input_tokens":3,"output_tokens":2}}})),"text/event-stream")
    }).expect(1).mount(&server).await;
    let mut cfg = config();
    cfg.providers.get_mut("a").unwrap().base_url = format!("{}/first", server.uri());
    cfg.save().unwrap();
    let approval = Arc::new(Mutex::new(ApprovalState {
        auto_approve: true,
        mode: crate::config::PermissionMode::BypassPermissions,
        ..Default::default()
    }));
    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let root = craftsman_harness::SessionCancellation::new();
    let mut slot = AgentSlot::spawn(&cfg, approval.clone(), UiBridge { tx }, root.clone());
    let mut app = App::new(&cfg);
    let previous = slot.get().await.as_ref().unwrap();
    previous
        .agent
        .run_turn("Write proof.txt once, then retain the result.")
        .await
        .unwrap();
    let thread = previous.thread_id.clone();
    let sequence = previous
        .checkpointer
        .as_ref()
        .unwrap()
        .load_latest(&thread)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(*previous.usage.lock().unwrap(), (6, 4));
    let modified = std::fs::metadata(directory.join("proof.txt"))
        .unwrap()
        .modified()
        .unwrap();
    let original_cfg = serde_json::to_value(&cfg).unwrap();
    let config_path = crate::config::config_path();
    let backup = config_path.with_file_name("before-failure.json");
    std::fs::rename(&config_path, &backup).unwrap();
    std::fs::create_dir(&config_path).unwrap();
    save(
        "a",
        format!("{}/second", server.uri()),
        "new-key".into(),
        "org/new-model".into(),
        &mut cfg,
        &mut slot,
        &approval,
        &mut app,
    );
    assert_eq!(serde_json::to_value(&cfg).unwrap(), original_cfg);
    assert!(!slot.is_pending());
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert!(config_path.is_dir());
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&backup).unwrap()).unwrap(),
        original_cfg
    );
    std::fs::remove_dir(&config_path).unwrap();
    std::fs::rename(&backup, &config_path).unwrap();
    save(
        "a",
        format!("{}/second", server.uri()),
        "new-key".into(),
        "org/new-model".into(),
        &mut cfg,
        &mut slot,
        &approval,
        &mut app,
    );
    let next = slot.get().await.as_ref().unwrap();
    assert_eq!(next.thread_id, thread);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert_eq!(*next.usage.lock().unwrap(), (6, 4));
    next.agent
        .run_turn("Continue using the newly configured connection.")
        .await
        .unwrap();
    assert!(
        next.checkpointer
            .as_ref()
            .unwrap()
            .load_latest(&thread)
            .await
            .unwrap()
            .unwrap()
            .0
            > sequence
    );
    assert_eq!(*next.usage.lock().unwrap(), (9, 6));
    assert_eq!(
        std::fs::read(directory.join("proof.txt")).unwrap(),
        b"one setup effect"
    );
    assert_eq!(
        std::fs::metadata(directory.join("proof.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    assert_eq!(saved["model"], "a/org/new-model");
    assert_eq!(
        saved["providers"]["b"]["api_key"],
        "${UNSELECTED_SETUP_CREDENTIAL}"
    );
    assert_eq!(saved["retained-user-field"]["value"], 7);
    assert_eq!(
        cfg.providers["a"].models["original"].settings["reasoning"]["value"],
        "high"
    );
    root.cancel();
    slot.shutdown().await;
    server.verify().await;
    eprintln!(
        "actual provider setup save failure, durable transfer, HTTP continuation and no replay assertions completed"
    );
}

#[tokio::test]
async fn actual_provider_setup_preserves_file_history_usage_and_config_across_failed_and_successful_saves()
 {
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
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
        .arg(format!("{MARKER}{}", root.path().to_str().unwrap()))
        .current_dir(root.path())
        .env_clear()
        .env("CRAFTSMAN_CONFIG_DIR", config)
        .env("HOME", root.path())
        .env("USERPROFILE", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(30), async {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut stdout = stdout.take(65537);
        let mut stderr = stderr.take(65537);
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    })
    .await;
    let (status, out, err) = match outcome {
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned setup fixture failed its execution/cleanup bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    assert!(status.success(), "{}", String::from_utf8_lossy(&err));
    assert_eq!(String::from_utf8(err).unwrap().lines().filter(|line|*line=="actual provider setup save failure, durable transfer, HTTP continuation and no replay assertions completed").count(),1);
}
