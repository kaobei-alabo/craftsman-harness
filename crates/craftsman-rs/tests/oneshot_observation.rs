//! New parent observations of the unchanged owned legacy scenarios.
//! Original oneshot_contract tests are not included or altered.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::process::{Child, Command};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[path = "fixtures/oneshot_observation.rs"]
mod fixture;
#[cfg(windows)]
#[path = "common/oneshot_observation_windows.rs"]
mod native_process;

const HELPER: &str = "marked_native_oneshot_observation";
const MARKER: &str = "__craftsman_original_oneshot_observation__:";
const CAP: usize = 65536;

#[derive(Clone, Copy)]
enum Case {
    Compaction,
    BrokenStdout,
    FinalNewline,
    Unfinished,
}
impl Case {
    fn name(self) -> &'static str {
        match self {
            Self::Compaction => "compaction",
            Self::BrokenStdout => "broken-stdout",
            Self::FinalNewline => "final-newline",
            Self::Unfinished => "unfinished-sse",
        }
    }
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "compaction" => Self::Compaction,
            "broken-stdout" => Self::BrokenStdout,
            "final-newline" => Self::FinalNewline,
            "unfinished-sse" => Self::Unfinished,
            _ => return None,
        })
    }
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn image_hash(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
fn io_fields(error: &io::Error) -> Value {
    json!({"kind":format!("{:?}",error.kind()),"raw_os_error":error.raw_os_error()})
}
fn create_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?
        .write_all(bytes)
}

struct TraceState {
    events: Vec<Value>,
    bytes: usize,
    failed: bool,
    sink: std::fs::File,
}
#[derive(Clone)]
struct Trace {
    start: Instant,
    state: Arc<Mutex<TraceState>>,
}
impl Trace {
    fn new() -> Self {
        let sink = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open("oneshot-events.jsonl")
            .unwrap();
        Self {
            start: Instant::now(),
            state: Arc::new(Mutex::new(TraceState {
                events: Vec::new(),
                bytes: 0,
                failed: false,
                sink,
            })),
        }
    }
    fn event(&self, name: &'static str, fields: Value) {
        let mut state = self.state.lock().unwrap();
        let record = json!({"ordinal":state.events.len(),"event":name,"elapsed_ns":self.start.elapsed().as_nanos(),"fields":fields});
        let mut bytes = serde_json::to_vec(&record).unwrap();
        bytes.push(b'\n');
        if state.events.len() >= 128 || state.bytes + bytes.len() > CAP {
            state.failed = true;
            return;
        }
        state.bytes += bytes.len();
        state.events.push(record);
        if state.sink.write_all(&bytes).is_err() {
            state.failed = true;
        }
    }
    fn packet(&self) -> Value {
        let s = self.state.lock().unwrap();
        json!({"records":s.events,"record_count":s.events.len(),"encoded_event_bytes":s.bytes,
            "cap_records":128,"cap_bytes":CAP,"diagnostic_failed":s.failed})
    }
}

struct LoggedResponder<R> {
    inner: R,
    trace: Trace,
    calls: Arc<AtomicUsize>,
}
impl<R: Respond> Respond for LoggedResponder<R> {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let ordinal = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.trace.event(
            "loopback_request_received",
            json!({"request_ordinal":ordinal,"body_bytes":request.body.len()}),
        );
        let response = self.inner.respond(request);
        // Wiremock owns the later socket writes. Template construction is not
        // presented as a native SSE flush or a received client response.
        self.trace.event(
            "exact_fixture_response_returned",
            json!({"request_ordinal":ordinal,"native_sse_flush":"UNAVAILABLE_IN_WIREMOCK"}),
        );
        response
    }
}

struct ObservedListener {
    inner: tokio::net::TcpListener,
    trace: Trace,
}
impl ObservedListener {
    async fn accept(&self) -> io::Result<(ObservedStream, std::net::SocketAddr)> {
        self.trace.event("loopback_accept_begin", json!({}));
        let (inner, addr) = self.inner.accept().await?;
        self.trace
            .event("loopback_accept_return", json!({"accepted":true}));
        Ok((
            ObservedStream {
                inner,
                trace: self.trace.clone(),
                read_started: false,
                flush_started: false,
            },
            addr,
        ))
    }
}
struct ObservedStream {
    inner: tokio::net::TcpStream,
    trace: Trace,
    read_started: bool,
    flush_started: bool,
}
impl AsyncRead for ObservedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.read_started {
            self.read_started = true;
            self.trace.event("actual_http_header_read_begin", json!({}));
        }
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(ref returned) = result {
            self.trace.event("actual_http_header_read_return",match returned {
                Ok(())=>json!({"bytes":buf.filled().len()-before,"actual_eof":buf.filled().len()==before}),
                Err(e)=>json!({"error":io_fields(e)}) });
        }
        result
    }
}
impl AsyncWrite for ObservedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(ref returned) = result {
            self.trace.event(
                "actual_fixed_sse_write_return",
                match returned {
                    Ok(n) => json!({"bytes":n,"sha256":hash(&buf[..*n])}),
                    Err(e) => json!({"error":io_fields(e)}),
                },
            );
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.flush_started {
            self.flush_started = true;
            self.trace.event("actual_sse_flush_begin", json!({}));
        }
        let result = Pin::new(&mut self.inner).poll_flush(cx);
        if let Poll::Ready(ref returned) = result {
            self.trace.event(
                "actual_sse_flush_return",
                match returned {
                    Ok(()) => json!({"returned_success":true}),
                    Err(e) => json!({"error":io_fields(e)}),
                },
            );
        }
        result
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[derive(Default)]
struct ReadState {
    bytes: Vec<u8>,
    started: bool,
    eof: Option<bool>,
    error: Option<Value>,
    selected: bool,
}
struct ObservedReader<R> {
    inner: R,
    state: Arc<Mutex<ReadState>>,
    trace: Trace,
    stream: &'static str,
}
impl<R: AsyncRead + Unpin> AsyncRead for ObservedReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let start = {
            let mut s = self.state.lock().unwrap();
            if !s.started && buf.remaining() > 0 {
                s.started = true;
                true
            } else {
                false
            }
        };
        if start {
            self.trace
                .event("actual_parent_read_begin", json!({"stream":self.stream}));
        }
        let before = buf.filled().len();
        let offered = buf.remaining() > 0;
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(ref returned) = result {
            let mut s = self.state.lock().unwrap();
            match returned {
                Ok(()) => {
                    let added = &buf.filled()[before..];
                    s.bytes.extend_from_slice(added);
                    if offered && added.is_empty() {
                        s.eof = Some(true);
                    }
                }
                Err(error) => s.error = Some(io_fields(error)),
            }
        }
        result
    }
}
async fn read_owned<R: AsyncRead + Unpin>(
    reader: Option<R>,
    state: Arc<Mutex<ReadState>>,
    trace: Trace,
    stream: &'static str,
) -> io::Result<Vec<u8>> {
    let Some(inner) = reader else {
        trace.event(
            "parent_reader_unavailable",
            json!({"stream":stream,"actual_eof":null}),
        );
        return Ok(Vec::new());
    };
    state.lock().unwrap().selected = true;
    let mut reader = ObservedReader {
        inner,
        state: state.clone(),
        trace: trace.clone(),
        stream,
    }
    .take((CAP + 1) as u64);
    let mut bytes = Vec::new();
    let result = reader.read_to_end(&mut bytes).await;
    let eof = state.lock().unwrap().eof;
    trace.event("actual_parent_read_return",json!({"stream":stream,"bytes":bytes.len(),"sha256":hash(&bytes),
        "actual_eof":eof,"reached_limit":bytes.len()>CAP,"error":result.as_ref().err().map(io_fields)}));
    result?;
    if bytes.len() > CAP {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(bytes)
}
fn read_packet(state: &Arc<Mutex<ReadState>>) -> Value {
    let s = state.lock().unwrap();
    json!({"selected_actual_reader":s.selected,"read_started":s.started,"captured_bytes":s.bytes.len(),
        "sha256":hash(&s.bytes),"actual_eof":s.eof,"error":s.error,"reached_limit":s.bytes.len()>CAP})
}

#[cfg(windows)]
async fn observe_exit(witness: &native_process::Retained, trace: &Trace) -> io::Result<u32> {
    let mut checks = 0usize;
    loop {
        checks += 1;
        if let Some(code) = witness.observe()? {
            trace.event(
                "actual_owned_child_exit_signaled",
                json!({"exit_code":code,"native_zero_wait_checks":checks}),
            );
            return Ok(code);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct Collected {
    result: Result<io::Result<std::process::Output>, tokio::time::error::Elapsed>,
    witness_failed: bool,
}
async fn collect(mut child: Child, close_stdout: bool, trace: &Trace) -> Collected {
    let pid = child.id();
    if close_stdout {
        let reader = child.stdout.take().unwrap();
        drop(reader);
        trace.event(
            "sole_stdout_reader_dropped",
            json!({"remaining_parent_stdout_readers":0,"child_output_latch":"UNAVAILABLE"}),
        );
    }
    trace.event(
        "native_spawn_return",
        json!({"actual_pid":pid,"recorded_after_required_reader_drop":close_stdout}),
    );
    #[cfg(windows)]
    let witness = native_process::Retained::bind(&child);
    #[cfg(windows)]
    let witness_failed = witness.is_err();
    #[cfg(windows)]
    trace.event("actual_child_handle_retained",match &witness {
        Ok(w)=>json!({"same_access":true,"observed_inherit_bit":w.observed_inherit_bit,"before_wait_with_output_consumes_child":true}),
        Err(e)=>json!({"error":io_fields(e)}) });
    #[cfg(not(windows))]
    let witness_failed = false;
    #[cfg(not(windows))]
    trace.event(
        "independent_child_exit",
        json!({"observation":"UNAVAILABLE_ON_THIS_HOST"}),
    );
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_state = Arc::new(Mutex::new(ReadState::default()));
    let err_state = Arc::new(Mutex::new(ReadState::default()));
    // Transfer only the original sole readers. Their unchanged read_to_end
    // implementation is wrapped for observations; no tee/extra reader/flag or
    // output transformation is introduced. wait_with_output owns the SAME
    // Child, while the outer join owns these actual readers through completion.
    let joined = async {
        trace.event(
            "collector_begin",
            json!({"sole_readers_transferred":true,"wait_with_output":"same_child_status"}),
        );
        let wait = async {
            let status = child.wait_with_output().await;
            trace.event(
                "actual_child_wait_return",
                match &status {
                    Ok(output) => {
                        json!({"exit_code":output.status.code(),"success":output.status.success()})
                    }
                    Err(error) => json!({"error":io_fields(error)}),
                },
            );
            status
        };
        let (status, out, err) = tokio::try_join!(
            wait,
            read_owned(stdout, out_state.clone(), trace.clone(), "stdout"),
            read_owned(stderr, err_state.clone(), trace.clone(), "stderr")
        )?;
        trace.event(
            "collector_return",
            json!({"stdout_bytes":out.len(),"stderr_bytes":err.len()}),
        );
        Ok::<_, io::Error>(std::process::Output {
            status: status.status,
            stdout: out,
            stderr: err,
        })
    };
    #[cfg(windows)]
    let observed = async {
        tokio::pin!(joined);
        if let Ok(w) = &witness {
            tokio::select! {
                result=&mut joined=>result,
                exit=observe_exit(w,trace)=>{if let Err(e)=exit {trace.event("native_exit_observer_error",json!({"error":io_fields(&e)}));} joined.await}
            }
        } else {
            joined.await
        }
    };
    #[cfg(not(windows))]
    let observed = joined;
    let result = tokio::time::timeout(Duration::from_secs(20), observed).await;
    trace.event(
        "original_wait_outcome",
        json!({"seconds":20,"deadline_expired":result.is_err(),
        "io_error":result.as_ref().ok().and_then(|r|r.as_ref().err()).map(io_fields)}),
    );
    if !matches!(result, Ok(Ok(_))) {
        trace.event(
            "original_owned_capture_future_dropped",
            json!({"kill_on_drop_selected":true,"actual_native_kill_return":"UNAVAILABLE"}),
        );
        #[cfg(windows)]
        if let Ok(w) = &witness {
            let cleanup =
                tokio::time::timeout(Duration::from_secs(5), observe_exit(w, trace)).await;
            trace.event("owned_native_cleanup_observation",json!({"bound_seconds":5,"deadline_expired":cleanup.is_err(),
                "actual_exit_code":cleanup.as_ref().ok().and_then(|r|r.as_ref().ok()),
                "io_error":cleanup.as_ref().ok().and_then(|r|r.as_ref().err()).map(io_fields),"descendant_quiescence":"UNAVAILABLE"}));
        }
    }
    let out = out_state.lock().unwrap().bytes.clone();
    let err = err_state.lock().unwrap().bytes.clone();
    create_bytes(Path::new("cli.stdout.raw"), &out).unwrap();
    create_bytes(Path::new("cli.stderr.raw"), &err).unwrap();
    let packet = json!({"actual_stdout":read_packet(&out_state),"actual_stderr":read_packet(&err_state),
        "actual_exit_code":result.as_ref().ok().and_then(|r|r.as_ref().ok()).and_then(|o|o.status.code()),
        "wait_expired":result.is_err(),"witness_failed":witness_failed,
        "child_model_usage":"UNAVAILABLE_FROM_UNINSTRUMENTED_CLI","child_source_stages":"UNAVAILABLE",
        "whole_captured_streams_within_cap":out.len()<=CAP&&err.len()<=CAP,"trace":trace.packet()});
    let bytes = serde_json::to_vec_pretty(&packet).unwrap();
    assert!(bytes.len() <= CAP);
    create_bytes(Path::new("oneshot-capture.json"), &bytes).unwrap();
    Collected {
        result,
        witness_failed,
    }
}

fn spawn(command: &mut Command, trace: &Trace) -> Child {
    trace.event("native_spawn_begin", json!({}));
    let result = command.spawn();
    if let Err(error) = &result {
        trace.event("native_spawn_error", json!({"error":io_fields(error)}));
    }
    result.unwrap()
}
fn summary(case: Case, trace: &Trace, fields: Value) {
    let packet = json!({"case":case.name(),"mode_gadgets":cfg!(feature="gadgets"),"actual_facts":fields,"trace":trace.packet()});
    let bytes = serde_json::to_vec_pretty(&packet).unwrap();
    assert!(bytes.len() <= CAP);
    create_bytes(Path::new("oneshot-final.json"), &bytes).unwrap();
    assert_eq!(
        packet["trace"]["diagnostic_failed"], false,
        "whole diagnostic cap or sink failed"
    );
}

async fn scenario(case: Case, trace: &Trace) {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    match case {
        Case::Compaction => {
            std::fs::write(workspace.path().join("source.txt"), "source evidence").unwrap();
            let main_calls = Arc::new(AtomicUsize::new(0));
            let compaction_calls = Arc::new(AtomicUsize::new(0));
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .respond_with(LoggedResponder {
                    inner: fixture::CompactionAware {
                        main_calls: main_calls.clone(),
                        compaction_calls: compaction_calls.clone(),
                    },
                    trace: trace.clone(),
                    calls: calls.clone(),
                })
                .mount(&server)
                .await;
            fixture::config(
                home.path(),
                &server.uri(),
                json!({"model_profiles":{"mock-model":{"context_window":1}}}),
            );
            let mut command = fixture::command(home.path(), workspace.path());
            command.args(["-m", "read the source"]);
            let collected = collect(spawn(&mut command, trace), false, trace).await;
            summary(
                case,
                trace,
                json!({"actual_requests":server.received_requests().await.unwrap().len(),
                "actual_main_fixture_calls":main_calls.load(Ordering::SeqCst),"actual_compaction_fixture_calls":compaction_calls.load(Ordering::SeqCst),
                "actual_source_file_sha256":hash(&std::fs::read(workspace.path().join("source.txt")).unwrap()),"actual_model_usage":"UNAVAILABLE"}),
            );
            assert!(!collected.witness_failed);
            let output = collected.result.unwrap().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                "main-after-compaction\n"
            );
            assert_eq!(main_calls.load(Ordering::SeqCst), 2);
            assert_eq!(compaction_calls.load(Ordering::SeqCst), 1);
        }
        Case::BrokenStdout => {
            let target = workspace.path().join("must-not-be-dispatched.txt");
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .respond_with(LoggedResponder {
                    inner: fixture::BrokenPipeAware {
                        target: target.to_string_lossy().into_owned(),
                    },
                    trace: trace.clone(),
                    calls: calls.clone(),
                })
                .mount(&server)
                .await;
            fixture::config(home.path(), &server.uri(), json!({}));
            let mut command = fixture::command(home.path(), workspace.path());
            command.args(["-m", "respond then write", "--auto-approve"]);
            let collected = collect(spawn(&mut command, trace), true, trace).await;
            summary(
                case,
                trace,
                json!({"actual_requests":server.received_requests().await.unwrap().len(),"actual_target_exists":target.exists(),
                "actual_target_sha256":std::fs::read(&target).ok().map(|b|hash(&b)),"actual_model_usage":"UNAVAILABLE"}),
            );
            assert!(!collected.witness_failed);
            let output = collected.result.unwrap().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(0), "{stderr}");
            assert!(
                !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
                "{stderr}"
            );
            assert!(
                !target.exists(),
                "output closure must be checked before dispatch"
            );
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        }
        Case::FinalNewline => {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .respond_with(LoggedResponder {
                    inner: fixture::text(""),
                    trace: trace.clone(),
                    calls: calls.clone(),
                })
                .mount(&server)
                .await;
            fixture::config(home.path(), &server.uri(), json!({}));
            let mut command = fixture::command(home.path(), workspace.path());
            command.args(["-m", "return an empty answer"]);
            let collected = collect(spawn(&mut command, trace), true, trace).await;
            summary(
                case,
                trace,
                json!({"actual_requests":server.received_requests().await.unwrap().len(),"actual_model_usage":"UNAVAILABLE"}),
            );
            assert!(!collected.witness_failed);
            let output = collected.result.unwrap().unwrap();
            assert_eq!(
                output.status.code(),
                Some(0),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        }
        Case::Unfinished => {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = fixture::unfinished(ObservedListener {
                inner: listener,
                trace: trace.clone(),
            });
            fixture::config(home.path(), &format!("http://{address}"), json!({}));
            let mut command = fixture::command(home.path(), workspace.path());
            command.args(["-m", "stream indefinitely"]);
            let collected = collect(spawn(&mut command, trace), true, trace).await;
            summary(
                case,
                trace,
                json!({"actual_unfinished_server_task":!server.is_finished(),"actual_model_usage":"UNAVAILABLE"}),
            );
            assert!(!collected.witness_failed);
            let output = collected
                .result
                .expect("a broken stdout pipe must stop even an unfinished provider stream")
                .unwrap();
            assert!(
                !server.is_finished(),
                "the mock provider must still be unfinished"
            );
            server.abort();
            let _ = server.await;
            assert_eq!(
                output.status.code(),
                Some(0),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
#[ignore = "owned native observation only; exact unique marker required before config/environment/path/stdin work"]
fn marked_native_oneshot_observation() {
    let args: Vec<_> = std::env::args().collect();
    if !args.iter().any(|a| a == HELPER) || !args.iter().any(|a| a == "--exact") {
        return;
    }
    let names: Vec<_> = args.iter().filter_map(|a| a.strip_prefix(MARKER)).collect();
    let [name] = names.as_slice() else {
        return;
    };
    let Some(case) = Case::from_name(name) else {
        return;
    };
    let trace = Trace::new();
    trace.event("selected_helper_source",json!({"case":case.name(),"actual_helper_pid":std::process::id(),
        "actual_cli_image_sha256":image_hash(Path::new(env!("CARGO_BIN_EXE_craftsman"))).unwrap(),
        "actual_helper_image_sha256":image_hash(&std::env::current_exe().unwrap()).unwrap(),"mode_gadgets":cfg!(feature="gadgets"),
        "actual_main_source_sha256":hash(include_bytes!("../src/main.rs")),
        "actual_oneshot_io_source_sha256":hash(include_bytes!("../src/oneshot_io.rs")),
        "actual_windows_output_source_sha256":hash(include_bytes!("../src/oneshot_io/windows_probe.rs")),
        "original_fixture_source_sha256":hash(include_bytes!("oneshot_contract.rs")),
        "actual_model_usage":"UNAVAILABLE","credentials_and_bodies":"NOT_RECORDED"}));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(scenario(case, &trace));
    trace.event(
        "all_original_later_verdicts_returned",
        json!({"case":case.name()}),
    );
    eprintln!("owned one-shot observation and original later verdicts completed");
}

async fn run_marked(name: &str, additional: &[&str], selected: bool) {
    let root = tempfile::tempdir().unwrap();
    let physical = root.path().canonicalize().unwrap();
    let image = std::env::current_exe().unwrap();
    let mut command = Command::new(&image);
    command.args(["--ignored", "--exact", HELPER, "--nocapture"]);
    if name != "unmarked" {
        command.arg(format!("{MARKER}{name}"));
    }
    command
        .args(additional)
        .current_dir(&physical)
        .env_clear()
        .env("HOME", &physical)
        .env("USERPROFILE", &physical)
        .env("TMPDIR", &physical)
        .env("TEMP", &physical)
        .env("TMP", &physical)
        .env("CRAFTSMAN_CONFIG_DIR", physical.join("unselected-config"))
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
    let pid = child.id();
    let mut stdout = child.stdout.take().unwrap().take((CAP + 1) as u64);
    let mut stderr = child.stderr.take().unwrap().take((CAP + 1) as u64);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )?;
        Ok::<_, io::Error>(status)
    })
    .await;
    let cleanup = if !matches!(result, Ok(Ok(_))) {
        Some(
            tokio::time::timeout(Duration::from_secs(5), child.kill())
                .await
                .map(|r| r.map(|()| true).map_err(|e| io_fields(&e)))
                .ok(),
        )
    } else {
        None
    };
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/verification/privacy-original-windows-observation/actual/oneshot");
    std::fs::create_dir_all(&base).unwrap();
    let directory = tempfile::Builder::new()
        .prefix(name)
        .tempdir_in(&base)
        .unwrap()
        .keep();
    create_bytes(&directory.join("parent.stdout.raw"), &out).unwrap();
    create_bytes(&directory.join("parent.stderr.raw"), &err).unwrap();
    for entry in std::fs::read_dir(&physical).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            let bytes = std::fs::read(entry.path()).unwrap();
            assert!(bytes.len() <= CAP + 1);
            create_bytes(&directory.join(entry.file_name()), &bytes).unwrap();
        }
    }
    let packet = json!({"case":name,"actual_helper_pid":pid,"actual_helper_image_sha256":image_hash(&image).unwrap(),
        "actual_exit_code":result.as_ref().ok().and_then(|r|r.as_ref().ok()).and_then(|s|s.code()),
        "deadline_expired":result.is_err(),"cleanup":cleanup,"stdout_captured_bytes":out.len(),"stderr_captured_bytes":err.len(),
        "stdout_sha256":hash(&out),"stderr_sha256":hash(&err),"mode_gadgets":cfg!(feature="gadgets"),
        "new_outer_bound_seconds":30,"original_inner_wait_seconds":20,"original_cleanup_seconds":5,"selected":selected});
    create_bytes(
        &directory.join("parent.json"),
        &serde_json::to_vec_pretty(&packet).unwrap(),
    )
    .unwrap();
    assert!(
        out.len() <= CAP && err.len() <= CAP,
        "whole capture exceeds its cap"
    );
    assert!(
        result.unwrap().unwrap().success(),
        "new one-shot observer failed; complete owned packet retained"
    );
    if selected {
        let done = b"owned one-shot observation and original later verdicts completed";
        assert!(err.windows(done.len()).any(|w| w == done));
        assert!(physical.join("oneshot-capture.json").exists());
        assert!(physical.join("oneshot-final.json").exists());
    } else {
        assert!(!physical.join("oneshot-events.jsonl").exists());
        assert!(!physical.join("unselected-config").exists());
        assert_eq!(std::fs::read_dir(&physical).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn actual_compaction_observation_preserves_original_stdout_summary_and_counts() {
    run_marked("compaction", &[], true).await;
}
#[tokio::test]
async fn actual_closed_stdout_observation_preserves_quiet_exit_and_no_tool_effect() {
    run_marked("broken-stdout", &[], true).await;
}
#[tokio::test]
async fn actual_final_newline_observation_preserves_quiet_exit_and_one_request() {
    run_marked("final-newline", &[], true).await;
}
#[tokio::test]
async fn actual_unfinished_sse_observation_preserves_real_open_response_and_quiet_exit() {
    run_marked("unfinished-sse", &[], true).await;
}
#[tokio::test]
async fn unmarked_unknown_duplicate_observers_return_before_owned_work() {
    run_marked("unmarked", &[], false).await;
    run_marked("unknown", &[], false).await;
    run_marked(
        "compaction",
        &["__craftsman_original_oneshot_observation__:compaction"],
        false,
    )
    .await;
}
