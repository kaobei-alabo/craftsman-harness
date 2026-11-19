//! Actual normal-session controls for C1 identity and C2 return-stop fences.
use super::*;
use craftsman_harness::{Agent, SessionCancellation};
use std::future::Future;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;
use tokio::io::AsyncReadExt;

const TARGET: &str = "project-a/raw.bin";
const RAW: &[u8] = &[0, 255, 128, b'\r', b'\n', b'Z'];
const HELPER: &str =
    "tui::session_bridge::script_capture_tests::actual_agent_binding_and_capture_stop_fixture";
const MARKER: &str = "__craftsman_binding_stop_repair__:";
const DONE: &str = "actual Agent binding and per-capture return stop assertions completed";

#[derive(Clone, Copy)]
pub(crate) enum Case {
    Identity,
    AfterNativeCheck,
    ReceiverWake,
    StoppedWork,
    Historical,
    PreviewReturn,
}
impl Case {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "identity" => Self::Identity,
            "post-join" => Self::AfterNativeCheck,
            "receiver-wake" => Self::ReceiverWake,
            "stopped-work" => Self::StoppedWork,
            "historical" => Self::Historical,
            "preview-return" => Self::PreviewReturn,
            _ => return None,
        })
    }
}
pub(crate) fn selected(args: impl Iterator<Item = String>) -> Option<Case> {
    let mut markers = args.filter_map(|arg| arg.strip_prefix(MARKER).map(str::to_owned));
    let name = markers.next()?;
    if markers.next().is_some() {
        return None;
    }
    Case::parse(&name)
}

struct CancelAtReceiverWake {
    turn: TurnCancellation,
    reached: Arc<tokio::sync::Notify>,
    once: AtomicBool,
}
impl Wake for CancelAtReceiverWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if !self.once.swap(true, Ordering::AcqRel) {
            self.turn.cancel();
            self.reached.notify_one();
        }
    }
}

pub(crate) async fn run(case: Case, session: &mut Session, owner: SessionCancellation) {
    match case {
        Case::Identity => identity_case(session).await,
        Case::AfterNativeCheck => post_join_case(session, &owner).await,
        Case::ReceiverWake => receiver_case(session, &owner).await,
        Case::StoppedWork => stopped_work_case(session, &owner).await,
        Case::Historical => historical_case(session, &owner).await,
        Case::PreviewReturn => preview_return_case(session, &owner).await,
    }
}

async fn identity_case(session: &mut Session) {
    let originals = session
        .script_capture
        .as_ref()
        .unwrap()
        .capture(session, &[TARGET])
        .await
        .unwrap();
    let completed_preview = session
        .script_capture
        .as_ref()
        .unwrap()
        .preview(session, &originals, br#"{"changes":[]}"#)
        .unwrap();
    let context = session.script_capture.as_ref().unwrap().context.clone();
    let state = session.agent.state.clone();
    let backend = context.backend.clone();
    let ask_user = context.identity_tool.clone();
    let model = Arc::new(craftsman_harness::testing::ScriptedModel::new(vec![]));
    let other = std::env::current_dir().unwrap().join("different-backend");
    std::fs::create_dir(&other).unwrap();
    let mut observations = Vec::new();
    let copied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_copies = copied.clone();
    session
        .script_capture
        .as_ref()
        .unwrap()
        .set_hook(Arc::new(move |stage, _| {
            if stage == native::Stage::BeforeRead {
                observed_copies.fetch_add(1, Ordering::AcqRel);
            }
        }));
    for kind in [
        "shared-every-retained-handle",
        "default-backend",
        "different-backend",
        "missing-tool",
    ] {
        let mut builder = Agent::builder(model.clone())
            .name("main")
            .state(state.clone())
            .registry(session.registry.clone())
            .thread_id(session.thread_id.clone());
        if kind != "missing-tool" {
            builder = builder.tool(ask_user.clone());
        }
        if kind == "shared-every-retained-handle" {
            builder = builder.backend(backend.clone());
        }
        if kind == "different-backend" {
            builder = builder.backend(Arc::new(crate::backend_fs::RealFs::new(other.clone())));
        }
        let foreign = builder.build();
        assert!(Arc::ptr_eq(&foreign.state, &state));
        assert_eq!(
            foreign.registered_tool_matches("ask_user", &ask_user),
            kind != "missing-tool"
        );
        let real = std::mem::replace(&mut session.agent, foreign);
        let old_error = originals.original_bytes(session, TARGET).err();
        let read_error = session
            .script_capture
            .as_ref()
            .unwrap()
            .capture(session, &[TARGET])
            .await
            .err();
        let preview_error = session
            .script_capture
            .as_ref()
            .unwrap()
            .preview(session, &originals, br#"{"changes":[]}"#)
            .err();
        let completed_error = completed_preview.complete_historical_preview(session).err();
        observations.push(json!({
            "case":kind,"old_original_error":old_error.map(|e|format!("{e:?}")),
            "new_capture_error":read_error.map(|e|format!("{e:?}")),
            "new_preview_error":preview_error.map(|e|format!("{e:?}")),
            "accepted_preview_readout_error":completed_error.map(|e|format!("{e:?}")),
            "fresh_agent_rejected":old_error==Some(CaptureError::Foreign)
                && read_error==Some(CaptureError::Foreign)
                && preview_error==Some(CaptureError::Foreign)
                && completed_error==Some(CaptureError::Foreign),
        }));
        session.agent = real;
    }
    println!(
        "BINDING_STOP_OBSERVATIONS={}",
        json!({"version":1,"controls":observations})
    );
    session.script_capture.as_ref().unwrap().clear_hook();
    assert!(
        observations
            .iter()
            .all(|row| row["fresh_agent_rejected"] == true),
        "fresh Agent sharing state/backend/registry/tool cannot use the selected owner"
    );
    assert_eq!(
        copied.load(Ordering::Acquire),
        0,
        "foreign instances return before native content copying"
    );
    assert!(model.requests.lock().unwrap().is_empty());
    assert_eq!(
        originals.original_bytes(session, TARGET).unwrap(),
        Some(RAW)
    );
    let real = session.agent.clone();
    let moved = std::mem::replace(&mut session.agent, real);
    assert_eq!(
        originals.original_bytes(session, TARGET).unwrap(),
        Some(RAW),
        "clones and moves retain the actual built Agent identity"
    );
    session.agent = moved;
}

async fn post_join_case(session: &Session, owner: &SessionCancellation) {
    let turn = owner.new_turn();
    let stopped = turn.clone();
    let reached = Arc::new(AtomicBool::new(false));
    let observed = reached.clone();
    session
        .script_capture
        .as_ref()
        .unwrap()
        .set_hook(Arc::new(move |stage, _| {
            if stage == native::Stage::AfterNativeCheck {
                stopped.cancel();
                observed.store(true, Ordering::Release);
            }
        }));
    let result = turn
        .scope(
            session
                .script_capture
                .as_ref()
                .unwrap()
                .capture(session, &[TARGET]),
        )
        .await;
    session.script_capture.as_ref().unwrap().clear_hook();
    assert!(reached.load(Ordering::Acquire));
    assert!(turn.is_cancelled() && !owner.is_cancelled());
    let error = result.err();
    println!(
        "BINDING_STOP_OBSERVATIONS={}",
        json!({
            "version":1,"case":"actual-stop-after-native-check-before-publication",
            "hook_after_real_native_join":true,"owner_stopped":owner.is_cancelled(),
            "turn_stopped":turn.is_cancelled(),"return_error":error.map(|e|format!("{e:?}")),
        })
    );
    assert_eq!(error, Some(CaptureError::Revoked));
    assert_completed_refused_native(session);
}

async fn receiver_case(session: &Session, owner: &SessionCancellation) {
    let turn = owner.new_turn();
    let ready = Arc::new(tokio::sync::Notify::new());
    let wake = Arc::new(CancelAtReceiverWake {
        turn: turn.clone(),
        reached: ready.clone(),
        once: AtomicBool::new(false),
    });
    let waker = Waker::from(wake.clone());
    let mut scope = Box::pin(
        turn.scope(
            session
                .script_capture
                .as_ref()
                .unwrap()
                .capture(session, &[TARGET]),
        ),
    );
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(scope.as_mut().poll(&mut cx), Poll::Pending));
    tokio::time::timeout(Duration::from_secs(5), ready.notified())
        .await
        .unwrap();
    assert!(wake.once.load(Ordering::Acquire));
    assert!(turn.is_cancelled() && !owner.is_cancelled());
    let result = tokio::time::timeout(Duration::from_secs(5), scope)
        .await
        .unwrap();
    let error = result.err();
    println!(
        "BINDING_STOP_OBSERVATIONS={}",
        json!({
            "version":1,"case":"actual-stop-on-receiver-wake-before-resumption",
            "publication_hook_used":false,"turn_stopped":turn.is_cancelled(),
            "return_error":error.map(|e|format!("{e:?}")),
        })
    );
    assert_eq!(error, Some(CaptureError::Revoked));
    assert_completed_refused_native(session);
}

async fn stopped_work_case(session: &Session, owner: &SessionCancellation) {
    let originals = session
        .script_capture
        .as_ref()
        .unwrap()
        .capture(session, &[TARGET])
        .await
        .unwrap();
    let before_serial = session
        .script_capture
        .as_ref()
        .unwrap()
        .context
        .lifetime
        .lock()
        .unwrap()
        .serial;
    let turn = owner.new_turn();
    turn.cancel();
    let error = turn
        .scope(
            session
                .script_capture
                .as_ref()
                .unwrap()
                .capture(session, &[TARGET]),
        )
        .await
        .err();
    let preview_error = turn
        .scope(async {
            session
                .script_capture
                .as_ref()
                .unwrap()
                .preview(session, &originals, &vec![b' '; MAX_PREVIEW_BYTES + 1])
                .err()
        })
        .await;
    println!(
        "BINDING_STOP_OBSERVATIONS={}",
        json!({
            "version":1,"case":"stopped-turn-new-capture-and-new-preview",
            "capture_error":error.map(|e|format!("{e:?}")),
            "new_preview_error":preview_error.map(|e|format!("{e:?}")),
        })
    );
    assert_eq!(error, Some(CaptureError::Revoked));
    assert_eq!(
        preview_error,
        Some(CaptureError::Revoked),
        "stop wins before candidate decoding/bounds work"
    );
    assert_eq!(
        session
            .script_capture
            .as_ref()
            .unwrap()
            .context
            .lifetime
            .lock()
            .unwrap()
            .serial,
        before_serial
    );
}

async fn historical_case(session: &Session, owner: &SessionCancellation) {
    let turn = owner.new_turn();
    let originals = turn
        .scope(
            session
                .script_capture
                .as_ref()
                .unwrap()
                .capture(session, &[TARGET]),
        )
        .await
        .unwrap();
    let preview = turn
        .scope(async {
            session
                .script_capture
                .as_ref()
                .unwrap()
                .preview(session, &originals, br#"{"changes":[]}"#)
                .unwrap()
        })
        .await;
    turn.cancel();
    assert!(!owner.is_cancelled());
    turn.scope(async {
        assert_eq!(
            originals.original_bytes(session, TARGET).unwrap(),
            Some(RAW)
        );
        assert_eq!(originals.target_count(session).unwrap(), 1);
        assert_eq!(
            originals.historical_facts(session, TARGET).unwrap()["before"]["whole_bytes_and_eof"],
            true
        );
        assert!(
            !preview
                .complete_historical_preview(session)
                .unwrap()
                .is_empty()
        );
        assert!(preview.operations(session).unwrap().is_empty());
    })
    .await;
    let new_preview = turn
        .scope(async {
            session
                .script_capture
                .as_ref()
                .unwrap()
                .preview(session, &originals, br#"{"changes":[]}"#)
                .err()
        })
        .await;
    assert_eq!(
        new_preview,
        Some(CaptureError::Revoked),
        "historical readout never permits new work"
    );
}

fn assert_completed_refused_native(session: &Session) {
    let context = &session.script_capture.as_ref().unwrap().context;
    let lifetime = context.lifetime.lock().unwrap();
    let evidence = lifetime
        .refused_native
        .as_ref()
        .expect("actual completed native facts retained privately");
    assert!(evidence.frame.len() <= MAX_PREVIEW_BYTES);
    let frame: Value = serde_json::from_slice(&evidence.frame).unwrap();
    assert_eq!(frame["capture"], lifetime.serial);
    assert_eq!(frame["targets"][0]["target"], TARGET);
    assert_eq!(
        frame["targets"][0]["before"]["bytes_base64"],
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, RAW)
    );
    assert_eq!(frame["targets"][0]["before"]["whole_bytes_and_eof"], true);
    for field in [
        "path_before",
        "path_after",
        "path_final",
        "handle_before",
        "handle_after",
        "named_after",
        "named_final",
    ] {
        assert!(
            frame["targets"][0]["before"][field].is_object(),
            "full native field {field} retained"
        );
    }
}

async fn preview_return_case(session: &Session, owner: &SessionCancellation) {
    let originals = session
        .script_capture
        .as_ref()
        .unwrap()
        .capture(session, &[TARGET])
        .await
        .unwrap();
    let turn = owner.new_turn();
    let stopping = turn.clone();
    let reached = Arc::new(AtomicBool::new(false));
    let observed = reached.clone();
    session
        .script_capture
        .as_ref()
        .unwrap()
        .set_hook(Arc::new(move |stage, _| {
            if stage == native::Stage::AfterPreview {
                stopping.cancel();
                observed.store(true, Ordering::Release);
            }
        }));
    let preview = turn
        .scope(async {
            session
                .script_capture
                .as_ref()
                .unwrap()
                .preview(session, &originals, br#"{"changes":[]}"#)
                .err()
        })
        .await;
    session.script_capture.as_ref().unwrap().clear_hook();
    assert!(reached.load(Ordering::Acquire) && turn.is_cancelled() && !owner.is_cancelled());
    assert_eq!(
        preview,
        Some(CaptureError::Revoked),
        "actual stop after derivation refuses new preview return"
    );
    assert_eq!(
        originals.original_bytes(session, TARGET).unwrap(),
        Some(RAW)
    );
}

async fn run_case(case: &str, markers: &[&str]) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("work");
    let config = root.join("actual-settings");
    std::fs::create_dir_all(&config).unwrap();
    let root = root.canonicalize().unwrap();
    let config = config.canonicalize().unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        HELPER,
        "--nocapture",
        "__craftsman_session_capture__:capture",
    ]);
    if case != "unmarked" {
        command.arg(format!("{MARKER}{case}"));
    }
    command
        .args(markers)
        .current_dir(&root)
        .env_clear()
        .env("CRAFTSMAN_CONFIG_DIR", &config)
        .env("HOME", directory.path())
        .env("USERPROFILE", directory.path())
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
    let result = tokio::time::timeout(Duration::from_secs(120), async {
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
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned native binding/stop fixture did not return within its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let out = std::str::from_utf8(&out).unwrap();
    let err = std::str::from_utf8(&err).unwrap();
    print!("{out}");
    assert!(
        status.success(),
        "actual native binding/stop control failed\n{out}\n{err}"
    );
    let actual = Case::parse(case).is_some() && markers.is_empty();
    assert_eq!(
        err.lines().filter(|line| *line == DONE).count(),
        usize::from(actual)
    );
    if !actual {
        assert!(!config.join("config.json").exists());
        assert!(!config.join("threads.db").exists());
        assert!(!root.join("project-a").exists());
    }
}

#[tokio::test]
async fn fresh_agent_sharing_retained_state_backend_registry_tool_rejects_capture_and_data() {
    run_case("identity", &[]).await;
}
#[tokio::test]
async fn actual_per_capture_stop_after_native_check_refuses_successful_publication_return() {
    run_case("post-join", &[]).await;
}
#[tokio::test]
async fn actual_per_capture_stop_on_receiver_wake_refuses_successful_return() {
    run_case("receiver-wake", &[]).await;
}
#[tokio::test]
async fn stopped_turn_refuses_new_capture_and_new_preview_before_work() {
    run_case("stopped-work", &[]).await;
}
#[tokio::test]
async fn accepted_completed_snapshots_and_previews_remain_historical_in_stopped_turn() {
    run_case("historical", &[]).await;
}
#[tokio::test]
async fn actual_stop_after_pure_derivation_refuses_new_preview_return_keeps_historical_originals() {
    run_case("preview-return", &[]).await;
}
#[tokio::test]
async fn unmarked_unknown_duplicate_binding_stop_helpers_return_before_owned_config_work() {
    run_case("unmarked", &[]).await;
    run_case("unknown", &[]).await;
    run_case("identity", &["__craftsman_binding_stop_repair__:identity"]).await;
}
