//! Actual connection ownership, waiter cleanup and native WebSocket handoff.
use super::*;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(1);
const CLEANUP_WAIT: Duration = Duration::from_secs(5);

fn fixture() -> (AppState, Arc<StubDesktopDriver>) {
    let bridge = Arc::new(DesktopBridge::new());
    let driver = Arc::new(StubDesktopDriver {
        bridge: bridge.clone(),
        workspace: Mutex::new("/stub/main".into()),
        current_thread: Mutex::new("t-main".into()),
        threads: Mutex::new(Vec::new()),
        turn_active: AtomicBool::new(false),
        blocked_turn_started: None,
        blocked_turn_release: None,
        configuration_error: None,
        voice_available: true,
        voice_start_generation: AtomicU64::new(0),
        voice_cancelled: AtomicBool::new(false),
    });
    (
        AppState {
            runner: driver.clone(),
            bridge,
        },
        driver,
    )
}

struct CleanupProbe {
    inner: Arc<StubDesktopDriver>,
    entered: std_mpsc::Sender<()>,
    release: Mutex<Option<std_mpsc::Receiver<()>>>,
}

#[async_trait]
impl DesktopDriver for CleanupProbe {
    async fn ready(&self) -> bool {
        self.inner.ready().await
    }
    async fn current_thread(&self) -> String {
        self.inner.current_thread().await
    }
    async fn current_model(&self) -> String {
        self.inner.current_model().await
    }
    async fn current_workspace(&self) -> String {
        self.inner.current_workspace().await
    }
    async fn run_turn(&self, prompt: String, thread: String) -> Result<(), String> {
        self.inner.run_turn(prompt, thread).await
    }
    async fn set_model(&self, model: String) -> (bool, Option<String>) {
        self.inner.set_model(model).await
    }
    async fn history(&self, thread: String) -> Vec<Value> {
        self.inner.history(thread).await
    }
    async fn list_threads(&self, offset: usize, limit: usize) -> Result<ThreadPage, String> {
        self.inner.list_threads(offset, limit).await
    }
    async fn new_thread(&self) -> Result<String, String> {
        self.inner.new_thread().await
    }
    async fn delete_thread(&self, thread: String) -> Result<String, String> {
        self.inner.delete_thread(thread).await
    }
    async fn list_models(&self) -> Result<Vec<String>, String> {
        self.inner.list_models().await
    }
    fn steer(&self, message: String) -> bool {
        self.inner.steer(message)
    }
    fn cancel_voice(&self) -> bool {
        self.inner.cancel_voice()
    }
    fn cancel(&self) -> bool {
        let release = self.release.lock().unwrap().take();
        if let Some(release) = release {
            self.entered.send(()).unwrap();
            release.recv_timeout(CLEANUP_WAIT).unwrap();
        }
        self.inner.cancel()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_slot_stays_owned_until_actual_old_cleanup_finishes() {
    let (mut state, driver) = fixture();
    let (entered, enter_rx) = std_mpsc::channel();
    let (release, release_rx) = std_mpsc::channel();
    state.runner = Arc::new(CleanupProbe {
        inner: driver.clone(),
        entered,
        release: Mutex::new(Some(release_rx)),
    });
    let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
    let first = DesktopConnection::attach(&state, first_tx).await.unwrap();
    let generation = first.generation;
    driver.turn_active.store(true, Ordering::SeqCst);
    state
        .bridge
        .begin_turn("original-turn".into(), generation)
        .unwrap();
    let waiting_bridge = state.bridge.clone();
    let waiter = tokio::spawn(async move {
        waiting_bridge.request_approval("execute", "original build", "original command")
    });
    let frame = tokio::time::timeout(WAIT, first_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame["type"], "permission_request");
    let dropping = std::thread::spawn(move || drop(first));
    tokio::task::spawn_blocking(move || enter_rx.recv_timeout(CLEANUP_WAIT).unwrap())
        .await
        .unwrap();
    assert_eq!(
        state
            .bridge
            .slot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation,
        generation
    );
    assert!(state.bridge.connection_owner.try_lock().is_err());
    assert!(!waiter.is_finished());
    assert_eq!(state.bridge.approvals.lock().unwrap().len(), 1);

    let (next_tx, _next_rx) = tokio::sync::mpsc::unbounded_channel();
    let next = DesktopConnection::attach(&state, next_tx);
    tokio::pin!(next);
    assert!(futures_util::poll!(&mut next).is_pending());
    release.send(()).unwrap();
    tokio::task::spawn_blocking(move || dropping.join().unwrap())
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(WAIT, waiter).await.unwrap().unwrap(),
        ApprovalPromptResult::Reject(String::new())
    );
    let next = tokio::time::timeout(WAIT, next).await.unwrap().unwrap();
    assert!(next.generation > generation);
    assert!(!driver.turn_active.load(Ordering::SeqCst));
    assert!(driver.voice_cancelled.load(Ordering::SeqCst));
    assert!(!state.bridge.detach(generation));
    assert_eq!(
        state
            .bridge
            .slot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation,
        next.generation
    );
    drop(next);
}

#[tokio::test]
async fn live_owner_is_not_replaced_when_admission_handoff_expires() {
    let (state, driver) = fixture();
    let (first_tx, _first_rx) = tokio::sync::mpsc::unbounded_channel();
    let first = DesktopConnection::attach(&state, first_tx).await.unwrap();
    let (second_tx, _second_rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(
        tokio::time::timeout(WAIT, DesktopConnection::attach(&state, second_tx))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        state
            .bridge
            .slot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation,
        first.generation
    );
    assert!(!driver.voice_cancelled.load(Ordering::SeqCst));
    drop(first);
    assert!(driver.voice_cancelled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn racing_successors_admit_one_generation_at_a_time() {
    let (state, _) = fixture();
    let (first_tx, _first_rx) = tokio::sync::mpsc::unbounded_channel();
    let first = DesktopConnection::attach(&state, first_tx).await.unwrap();
    let (second_tx, _second_rx) = tokio::sync::mpsc::unbounded_channel();
    let (third_tx, _third_rx) = tokio::sync::mpsc::unbounded_channel();
    let second = DesktopConnection::attach(&state, second_tx);
    let third = DesktopConnection::attach(&state, third_tx);
    tokio::pin!(second, third);
    assert!(futures_util::poll!(&mut second).is_pending());
    assert!(futures_util::poll!(&mut third).is_pending());
    let first_generation = first.generation;
    drop(first);
    let second = tokio::time::timeout(WAIT, second).await.unwrap().unwrap();
    assert!(second.generation > first_generation);
    assert!(futures_util::poll!(&mut third).is_pending());
    let second_generation = second.generation;
    drop(second);
    let third = tokio::time::timeout(WAIT, third).await.unwrap().unwrap();
    assert!(third.generation > second_generation);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborting_an_owned_reader_releases_real_question_waiters_and_admission() {
    let (state, driver) = fixture();
    let (frames, mut receive) = tokio::sync::mpsc::unbounded_channel();
    let connection = DesktopConnection::attach(&state, frames).await.unwrap();
    let generation = connection.generation;
    state
        .bridge
        .begin_turn("question-turn".into(), connection.generation)
        .unwrap();
    driver.turn_active.store(true, Ordering::SeqCst);
    let bridge = state.bridge.clone();
    let question = tokio::spawn(async move { bridge.request_answer("original question") });
    assert_eq!(
        tokio::time::timeout(WAIT, receive.recv())
            .await
            .unwrap()
            .unwrap()["type"],
        "ask_user"
    );
    let (started, start_rx) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        let _connection = connection;
        started.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    start_rx.await.unwrap();
    reader.abort();
    assert!(
        tokio::time::timeout(WAIT, reader)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    assert_eq!(
        tokio::time::timeout(WAIT, question).await.unwrap().unwrap(),
        None
    );
    assert!(!driver.turn_active.load(Ordering::SeqCst));
    assert!(driver.voice_cancelled.load(Ordering::SeqCst));
    // Connection cleanup requests a stop; it does not fabricate completion of
    // the original turn. Its context remains until the runner actually returns.
    assert!(state.bridge.current_turn().is_some_and(|turn| {
        turn.connection_generation == generation && turn.thread_id == "question-turn"
    }));
    let (next, _receive) = tokio::sync::mpsc::unbounded_channel();
    assert!(DesktopConnection::attach(&state, next).await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_close_handshake_releases_the_slot_before_reconnect_hello() {
    let url = spawn_stub_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(url.clone()).await.unwrap();
    assert_eq!(recv_json(&mut socket).await["type"], "hello");
    assert_eq!(recv_json(&mut socket).await["type"], "state_snapshot");
    socket.close(None).await.unwrap();
    let reply = tokio::time::timeout(WAIT, socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        reply,
        tokio_tungstenite::tungstenite::Message::Close(_)
    ));
    let (mut next, _) = tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(url))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recv_json(&mut next).await["type"], "hello");
    assert_eq!(recv_json(&mut next).await["type"], "state_snapshot");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_live_client_rejects_a_second_client_and_remains_usable() {
    let url = spawn_stub_server().await;
    let (mut first, _) = tokio_tungstenite::connect_async(url.clone()).await.unwrap();
    assert_eq!(recv_json(&mut first).await["type"], "hello");
    assert_eq!(recv_json(&mut first).await["type"], "state_snapshot");
    let (mut second, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let rejection = recv_json(&mut second).await;
    assert_eq!(rejection["type"], "operation_error");
    assert_eq!(rejection["code"], "client_already_connected");
    first
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"get_state", "operation_id":"original-client"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let response = recv_json(&mut first).await;
    assert_eq!(response["type"], "state_snapshot");
    assert_eq!(response["operation_id"], "original-client");
}

#[tokio::test]
async fn stuck_async_writer_is_aborted_and_its_owned_resources_are_released() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let owned = Dropped(dropped.clone());
    let (started, receive) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        let _owned = owned;
        started.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    receive.await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), finish_writer(writer))
        .await
        .expect("writer cleanup must be bounded");
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn dropped_reader_writer_guard_cancels_an_in_flight_async_write_task() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let owned = Dropped(dropped.clone());
    let (started, receive) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        let _owned = owned;
        started.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    let guard = WriterGuard(writer.abort_handle());
    receive.await.unwrap();
    drop(guard);
    assert!(
        tokio::time::timeout(WAIT, writer)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    assert!(dropped.load(Ordering::SeqCst));
}
