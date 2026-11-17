//! Credential-free native HTTP/builder/ACP failure boundaries.
//! These fixtures do not identify a historical peer failure or change runtime policy.
use super::*;
use craftsman_harness::{AgentError, AgentState, Checkpointer, Msg, TurnResult};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, oneshot};

const HELPER: &str =
    "agent::provider_failure_boundary_tests::native_provider_failure_boundary_fixture";
const MARKER: &str = "craftsman-provider-failure-boundary=";
const OBSERVATION: &str = "provider-boundary-observation ";
const DONE: &str =
    "actual provider HTTP, approved file, usage and failure-boundary assertions completed";
const OUTPUT_LIMIT: u64 = 64 * 1024;
const REQUEST_LIMIT: u64 = 64 * 1024;
const FRAME_LIMIT: u64 = 256 * 1024;
const MAX_REQUESTS: usize = 24;
const INPUT: &str = "Exercise the selected provider boundary and retain the approved file result.";
const FOLLOWUP: &str = "Explicit follow-up; retain the earlier file and do not repeat its write.";
const FILE: &str = "approved.txt";
const CONTENT: &str = "original approved provider-boundary effect\n";
const FINAL: &str = "completed on exactly the twentieth response";
const FOLLOWUP_FINAL: &str = "explicit follow-up completed without another write";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    CreatedEof,
    AdmissionRefusal,
    TwentiethControl,
}
impl Scenario {
    fn complete_responses(self) -> usize {
        match self {
            Self::CreatedEof => 1,
            Self::AdmissionRefusal | Self::TwentiethControl => 20,
        }
    }
    fn initial_requests(self) -> usize {
        match self {
            Self::CreatedEof => 2,
            Self::AdmissionRefusal => 21,
            Self::TwentiethControl => 20,
        }
    }
    fn tool_results(self) -> usize {
        match self {
            Self::CreatedEof => 1,
            Self::AdmissionRefusal => 20,
            Self::TwentiethControl => 19,
        }
    }
    fn error(self) -> &'static str {
        match self {
            Self::CreatedEof => "Native model stream ended before completion.",
            Self::AdmissionRefusal => "Native model endpoint returned HTTP 402.",
            Self::TwentiethControl => panic!("the twentieth response must complete normally"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Case {
    Builder(Scenario, bool),
    Acp(Scenario),
}
impl Case {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "builder-eof-default" => Self::Builder(Scenario::CreatedEof, false),
            "builder-eof-owned" => Self::Builder(Scenario::CreatedEof, true),
            "builder-refusal-default" => Self::Builder(Scenario::AdmissionRefusal, false),
            "builder-refusal-owned" => Self::Builder(Scenario::AdmissionRefusal, true),
            "builder-control" => Self::Builder(Scenario::TwentiethControl, false),
            "acp-eof" => Self::Acp(Scenario::CreatedEof),
            "acp-refusal" => Self::Acp(Scenario::AdmissionRefusal),
            "acp-control" => Self::Acp(Scenario::TwentiethControl),
            _ => return None,
        })
    }
}

fn selected(arguments: impl IntoIterator<Item = OsString>) -> Option<Case> {
    let mut selection = None;
    for argument in arguments {
        let Some(value) = argument.to_str().and_then(|arg| arg.strip_prefix(MARKER)) else {
            continue;
        };
        if selection.is_some() {
            return None;
        }
        selection = Some(Case::parse(value)?);
    }
    selection
}

fn usage(ordinal: usize) -> (u64, u64) {
    let ordinal = u64::try_from(ordinal).unwrap();
    (10 + ordinal, 20 + ordinal)
}

#[derive(Clone)]
struct Exchange {
    ordinal: usize,
    request: Value,
    status: u16,
    created: bool,
    completed_usage: Option<(u64, u64)>,
    response_bytes_written: usize,
}

struct HttpFixture {
    uri: String,
    exchanges: Arc<Mutex<Vec<Exchange>>>,
    changed: Arc<Notify>,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl HttpFixture {
    async fn start(scenario: Scenario) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let uri = format!("http://{}/v1", listener.local_addr().unwrap());
        let exchanges = Arc::new(Mutex::new(Vec::new()));
        let observed = exchanges.clone();
        let changed = Arc::new(Notify::new());
        let completed = changed.clone();
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            for ordinal in 1..=MAX_REQUESTS {
                let (stream, _) = tokio::select! {
                    _ = &mut stopped => return,
                    accepted = listener.accept() => accepted.unwrap(),
                };
                let (mut stream, request) =
                    tokio::time::timeout(Duration::from_secs(5), read_request(stream))
                        .await
                        .expect("the fixture received no bounded complete native HTTP request");
                let (status, body, created, completed_usage) = reply(scenario, ordinal);
                let reason = match status {
                    200 => "OK",
                    402 => "Payment Required",
                    _ => "Internal Server Error",
                };
                // A real connection-close body has genuine EOF. In the created-only
                // case there is no content length, terminal SSE event or timer cut.
                let header = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
                    if status == 200 {
                        "text/event-stream"
                    } else {
                        "application/json"
                    }
                );
                let bytes = format!("{header}{body}");
                stream.write_all(bytes.as_bytes()).await.unwrap();
                stream.flush().await.unwrap();
                stream.shutdown().await.unwrap();
                // Record only after the actual writes/flush/FIN return successfully.
                observed.lock().unwrap().push(Exchange {
                    ordinal,
                    request,
                    status,
                    created,
                    completed_usage,
                    response_bytes_written: bytes.len(),
                });
                completed.notify_waiters();
            }
            panic!("native provider fixture exceeded its request bound");
        });
        Self {
            uri,
            exchanges,
            changed,
            stop,
            task,
        }
    }

    async fn snapshot(&self, expected: usize) -> Vec<Exchange> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let snapshot = self.exchanges.lock().unwrap().clone();
                if snapshot.len() >= expected {
                    return snapshot;
                }
                notified.await;
            }
        })
        .await
        .expect("the actual HTTP writes and FIN did not complete within the fixture bound")
    }

    async fn finish(self) -> Vec<Exchange> {
        let _ = self.stop.send(());
        self.task.await.unwrap();
        self.exchanges.lock().unwrap().clone()
    }
}

async fn read_request(stream: TcpStream) -> (TcpStream, Value) {
    let mut reader = BufReader::new(stream.take(REQUEST_LIMIT + 1));
    let mut headers = Vec::new();
    loop {
        let before = headers.len();
        let count = reader.read_until(b'\n', &mut headers).await.unwrap();
        assert!(count > 0 && headers.len() <= 8192, "bounded HTTP headers");
        if &headers[before..] == b"\r\n" {
            break;
        }
    }
    let headers = std::str::from_utf8(&headers).unwrap();
    let mut lines = headers.split("\r\n");
    assert_eq!(lines.next(), Some("POST /v1/responses HTTP/1.1"));
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Bearer synthetic-provider-boundary-key")
    );
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert!(!headers.contains_key("transfer-encoding"));
    let size: usize = headers["content-length"].parse().unwrap();
    assert!(size <= usize::try_from(REQUEST_LIMIT).unwrap() - 8192);
    let mut body = vec![0; size];
    reader.read_exact(&mut body).await.unwrap();
    assert!(
        reader.buffer().is_empty(),
        "no pipelined request or extra body bytes"
    );
    let request: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["model"], "provider-boundary-fixture");
    assert_eq!(request["stream"], true);
    assert!(
        !request["tools"].as_array().unwrap().is_empty(),
        "every observed request must be the actual main builder, not a hidden compaction call"
    );
    (reader.into_inner().into_inner(), request)
}

fn reply(scenario: Scenario, ordinal: usize) -> (u16, String, bool, Option<(u64, u64)>) {
    if scenario == Scenario::AdmissionRefusal && ordinal > 20 {
        // Fixture-only admission: twenty complete requests are admitted; later
        // requests remain refused, including an explicit follow-up. No reset.
        return (
            402,
            json!({"error":{"message":"synthetic admission limit reached; private refusal body"}})
                .to_string(),
            false,
            None,
        );
    }
    let created = json!({
        "type":"response.created",
        "response":{"id":format!("provider-boundary-response-{ordinal}"),
                    "status":"in_progress","output":[],"usage":null}
    });
    let prefix = format!("event: response.created\ndata: {created}\n\n");
    if scenario == Scenario::CreatedEof && ordinal == 2 {
        return (200, prefix, true, None);
    }
    if scenario == Scenario::TwentiethControl && ordinal > 20 {
        return (
            500,
            json!({"error":{"message":"unexpected extra request"}}).to_string(),
            false,
            None,
        );
    }
    let final_text = if scenario == Scenario::TwentiethControl && ordinal == 20 {
        Some(FINAL)
    } else if scenario == Scenario::CreatedEof && ordinal > 2 {
        Some(FOLLOWUP_FINAL)
    } else {
        None
    };
    let output = if let Some(text) = final_text {
        json!([{"type":"message","id":format!("provider-boundary-message-{ordinal}"),
                "role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":text,"annotations":[]}]}])
    } else {
        let (name, args) = if ordinal == 1 {
            ("write_file", json!({"file_path":FILE,"content":CONTENT}))
        } else {
            ("read_file", json!({"file_path":FILE,"offset":0,"limit":10}))
        };
        json!([{"type":"function_call","id":format!("provider-boundary-function-{ordinal}"),
                "call_id":format!("provider-boundary-call-{ordinal:02}"),
                "name":name,"arguments":args.to_string(),"status":"completed"}])
    };
    let reported = usage(ordinal);
    let completed = json!({
        "type":"response.completed",
        "response":{"id":format!("provider-boundary-response-{ordinal}"),"status":"completed",
                    "output":output,"usage":{"input_tokens":reported.0,"output_tokens":reported.1}}
    });
    (
        200,
        format!("{prefix}event: response.completed\ndata: {completed}\n\n"),
        true,
        Some(reported),
    )
}

fn config(directory: &Path, uri: &str) -> (Config, PathBuf, [u8; 32]) {
    let selected = PathBuf::from(std::env::var_os("CRAFTSMAN_CONFIG_DIR").unwrap());
    assert!(selected.is_absolute() && !selected.starts_with(directory));
    let file = selected.join("config.json");
    let value = json!({
        "model":"test/provider-boundary-fixture",
        "providers":{"test":{"protocol":"openai_responses","base_url":uri,
            "api_key":"synthetic-provider-boundary-key",
            "models":{"provider-boundary-fixture":{"settings":{
                "reasoning":null,"generation":{"max_output_tokens":8192}
            }}}}},
        "permission_mode":"manual","auto_approve":false,"harness_profile":"general",
        "voice":{"enabled":false},"gadgets":{"enabled":false},
        "mcp_servers":[],"lsp_servers":{}
    });
    let bytes = serde_json::to_vec(&value).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    (
        serde_json::from_value(value).unwrap(),
        file,
        Sha256::digest(bytes).into(),
    )
}

fn tool_results(state: &AgentState) -> Vec<(&str, &str)> {
    state
        .messages
        .iter()
        .filter_map(|message| match message {
            Msg::Tool {
                tool_call_id,
                content,
                is_error,
            } => {
                assert!(!is_error, "the earlier actual file operations succeeded");
                Some((tool_call_id.as_str(), content.as_str()))
            }
            _ => None,
        })
        .collect()
}

fn assert_retained(state: &AgentState, expected_tools: usize) {
    assert_eq!(state.messages.first(), Some(&Msg::user(INPUT)));
    let calls: Vec<_> = state
        .messages
        .iter()
        .filter_map(|message| match message {
            Msg::Assistant { tool_calls, .. } => Some(tool_calls),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(calls.len(), expected_tools);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.name == "write_file")
            .count(),
        1
    );
    assert_eq!(calls[0].args, json!({"file_path":FILE,"content":CONTENT}));
    let results = tool_results(state);
    assert_eq!(results.len(), expected_tools);
    for (offset, (id, content)) in results.iter().enumerate() {
        assert_eq!(*id, format!("provider-boundary-call-{:02}", offset + 1));
        if offset == 0 {
            assert!(content.contains("Updated file") && content.contains(FILE));
        } else {
            assert!(content.contains(CONTENT.trim_end()));
        }
    }
}

fn reported_usage(exchanges: &[Exchange]) -> (u64, u64) {
    exchanges
        .iter()
        .filter_map(|exchange| exchange.completed_usage)
        .fold((0, 0), |sum, usage| (sum.0 + usage.0, sum.1 + usage.1))
}

fn http_observation(exchanges: &[Exchange]) -> Value {
    let usage = reported_usage(exchanges);
    json!({
        "http_exchanges":exchanges.len(),
        "written_http_statuses":exchanges.iter().map(|exchange| exchange.status).collect::<Vec<_>>(),
        "written_created_events":exchanges.iter().filter(|exchange| exchange.created).count(),
        "written_completed_events":exchanges.iter().filter(|exchange| exchange.completed_usage.is_some()).count(),
        "unreported_usage_ordinals":exchanges.iter().filter(|exchange| exchange.completed_usage.is_none()).map(|exchange| exchange.ordinal).collect::<Vec<_>>(),
        "completed_response_reported_usage":{"input_tokens":usage.0,"output_tokens":usage.1}
    })
}

fn assert_http(exchanges: &[Exchange], scenario: Scenario, followup: bool) {
    assert_eq!(
        exchanges.len(),
        scenario.initial_requests() + usize::from(followup)
    );
    for (index, exchange) in exchanges.iter().enumerate() {
        assert_eq!(exchange.ordinal, index + 1);
        assert!(exchange.response_bytes_written > 0);
        let expected_status = if scenario == Scenario::AdmissionRefusal && exchange.ordinal > 20 {
            402
        } else {
            200
        };
        assert_eq!(exchange.status, expected_status);
        assert_eq!(exchange.created, expected_status == 200);
        let expected_usage = if expected_status == 402
            || scenario == Scenario::CreatedEof && exchange.ordinal == 2
        {
            None
        } else {
            Some(usage(exchange.ordinal))
        };
        assert_eq!(exchange.completed_usage, expected_usage);
        let results: Vec<_> = exchange.request["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .collect();
        assert_eq!(
            results.len(),
            index.min(scenario.tool_results()),
            "the next real request must contain every earlier accepted file result"
        );
        if let Some(result) = results.first() {
            assert_eq!(result["call_id"], "provider-boundary-call-01");
            assert!(result["output"].as_str().unwrap().contains("Updated file"));
        }
    }
}

fn assert_model_error(result: Result<TurnResult, AgentError>, scenario: Scenario) {
    match result {
        Err(AgentError::Model(message)) => assert_eq!(message, scenario.error()),
        other => panic!("expected the actual fixed model failure, got {other:?}"),
    }
}

async fn builder_case(directory: &Path, scenario: Scenario, owned: bool) {
    let server = HttpFixture::start(scenario).await;
    let (cfg, cfg_path, cfg_hash) = config(directory, &server.uri);
    let approvals = Arc::new(AtomicUsize::new(0));
    let approved = approvals.clone();
    let text = Arc::new(Mutex::new(String::new()));
    let actual_text = text.clone();
    let session = build_session_ext(
        &cfg,
        Arc::new(Mutex::new(ApprovalState::default())),
        "",
        false,
        false,
        None,
        SessionExtras {
            workspace_root: Some(directory.to_path_buf()),
            cancellation_session: owned.then(craftsman_harness::SessionCancellation::default),
            token_sink: Some(Arc::new(move |chunk| {
                actual_text.lock().unwrap().push_str(chunk)
            })),
            activity_sink: Some(Arc::new(|_| {})),
            call_approval_prompt: Some(Arc::new(move |call, _, _| {
                assert_eq!(call.name, "write_file");
                assert_eq!(call.id, "provider-boundary-call-01");
                assert_eq!(call.args, json!({"file_path":FILE,"content":CONTENT}));
                approved.fetch_add(1, Ordering::SeqCst);
                ApprovalPromptResult::Approve
            })),
            ..SessionExtras::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(session.agent.durable_turn_checkpoints_enabled(), owned);
    let result = session.agent.run_turn(INPUT).await;
    if scenario == Scenario::TwentiethControl {
        let TurnResult::Done(output) = result.unwrap() else {
            panic!("the actual twentieth response must finish without suspension");
        };
        assert_eq!(output.final_text(), FINAL);
        assert_eq!(*text.lock().unwrap(), FINAL);
    } else {
        assert_model_error(result, scenario);
        assert!(
            text.lock().unwrap().is_empty(),
            "no fabricated answer on model failure"
        );
    }
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read_to_string(directory.join(FILE)).unwrap(),
        CONTENT
    );
    assert_retained(
        &session.agent.state.lock().unwrap(),
        scenario.tool_results(),
    );
    let initial = server.snapshot(scenario.initial_requests()).await;
    assert_http(&initial, scenario, false);
    assert_eq!(*session.usage.lock().unwrap(), reported_usage(&initial));
    assert_eq!(
        initial
            .iter()
            .filter(|exchange| exchange.completed_usage.is_some())
            .count(),
        scenario.complete_responses()
    );
    let (seq, stored, suspension) = session
        .checkpointer
        .as_ref()
        .unwrap()
        .load_latest(&session.thread_id)
        .await
        .unwrap()
        .unwrap();
    let expected_seq = if owned {
        3 * scenario.complete_responses() + 1
    } else if scenario == Scenario::TwentiethControl {
        scenario.complete_responses()
    } else {
        scenario.complete_responses() - 1
    };
    assert_eq!(seq, u64::try_from(expected_seq).unwrap());
    assert!(suspension.is_none());
    assert_eq!(
        stored.custom.contains_key("_craftsman_durable_turn_v1"),
        owned
    );
    let expected_saved = if owned || scenario == Scenario::TwentiethControl {
        scenario.tool_results()
    } else {
        scenario.tool_results() - 1
    };
    assert_eq!(tool_results(&stored).len(), expected_saved);
    if owned {
        assert_eq!(
            stored.custom["_craftsman_durable_turn_v1"]["in_progress"],
            true
        );
        assert_retained(&stored, scenario.tool_results());
    }
    let followup = scenario != Scenario::TwentiethControl;
    if followup {
        // A real explicit new turn is different from an automatic retry. The
        // refusal fixture keeps the SAME twenty-admission ceiling in this turn.
        let result = session.agent.run_turn(FOLLOWUP).await;
        if scenario == Scenario::CreatedEof {
            let TurnResult::Done(output) = result.unwrap() else {
                panic!("the explicit EOF follow-up must complete normally");
            };
            assert_eq!(output.final_text(), FOLLOWUP_FINAL);
        } else {
            assert_model_error(result, scenario);
        }
        assert_retained(
            &session.agent.state.lock().unwrap(),
            scenario.tool_results(),
        );
        assert_eq!(approvals.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(directory.join(FILE)).unwrap(),
            CONTENT
        );
    }
    session.lsp.shutdown().await;
    let exchanges = server.finish().await;
    assert_http(&exchanges, scenario, followup);
    assert_eq!(*session.usage.lock().unwrap(), reported_usage(&exchanges));
    let actual_cfg_hash: [u8; 32] = Sha256::digest(std::fs::read(cfg_path).unwrap()).into();
    assert_eq!(actual_cfg_hash, cfg_hash);
    eprintln!(
        "{OBSERVATION}{}",
        json!({"frontend":"builder","scenario":format!("{scenario:?}"),
            "owned_durable":owned,"http":http_observation(&exchanges),
            "approved_writes":approvals.load(Ordering::SeqCst),
            "live_tool_results":tool_results(&session.agent.state.lock().unwrap()).len(),
            "checkpoint_sequence_before_followup":seq,
            "saved_tool_results_before_followup":tool_results(&stored).len(),
            "explicit_followup":followup,
            "actual_main_usage":*session.usage.lock().unwrap()})
    );
}

struct AcpClient {
    writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    reader: BufReader<tokio::io::Take<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    frames: usize,
}
impl AcpClient {
    fn start(cfg: Config, directory: &Path) -> Self {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(client);
        let (server_reader, server_writer) = tokio::io::split(server);
        let root = directory.to_path_buf();
        let task = tokio::spawn(async move {
            crate::acp_serve::run_with_io(
                cfg,
                BufReader::new(server_reader),
                server_writer,
                Some(root),
            )
            .await
        });
        Self {
            writer,
            reader: BufReader::new(reader.take(FRAME_LIMIT + 1)),
            task,
            frames: 0,
        }
    }
    async fn send(&mut self, value: Value) {
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(bytes.len() <= 8192);
        self.writer.write_all(&bytes).await.unwrap();
        self.writer.write_all(b"\n").await.unwrap();
        self.writer.flush().await.unwrap();
    }
    async fn next(&mut self) -> Value {
        let mut bytes = Vec::new();
        assert!(self.reader.read_until(b'\n', &mut bytes).await.unwrap() > 0);
        self.frames += 1;
        assert!(self.frames <= 256 && bytes.len() <= 16 * 1024);
        serde_json::from_slice(&bytes).unwrap()
    }
    async fn until(&mut self, id: &str, approve: bool) -> (Value, Vec<Value>) {
        let mut frames = Vec::new();
        loop {
            let frame = self.next().await;
            if frame["method"] == "session/request_permission" {
                assert!(approve, "history replay must not request another approval");
                assert_eq!(frame["params"]["_meta"]["craftsman"]["tool"], "write_file");
                assert!(
                    frame["params"]["options"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|option| option["optionId"] == "allow-once"
                            && option["kind"] == "allow_once")
                );
                self.send(json!({"jsonrpc":"2.0","id":frame["id"],
                    "result":{"outcome":{"outcome":"selected","optionId":"allow-once"}}}))
                    .await;
            }
            if frame["id"] == id && frame.get("method").is_none() {
                return (frame, frames);
            }
            frames.push(frame);
        }
    }
    async fn finish(mut self) {
        self.writer.shutdown().await.unwrap();
        loop {
            let mut bytes = Vec::new();
            if self.reader.read_until(b'\n', &mut bytes).await.unwrap() == 0 {
                break;
            }
            self.frames += 1;
            assert!(self.frames <= 256 && bytes.len() <= 16 * 1024);
            let _: Value = serde_json::from_slice(&bytes).unwrap();
        }
        self.task.await.unwrap().unwrap();
    }
}

fn completed_file_results(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|frame| {
            let update = &frame["params"]["update"];
            if update["sessionUpdate"] != "tool_call_update" || update["status"] != "completed" {
                return None;
            }
            Some(
                update["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|item| item["content"]["text"].as_str())
                    .collect::<String>(),
            )
        })
        .collect()
}

async fn acp_case(directory: &Path, scenario: Scenario) {
    let server = HttpFixture::start(scenario).await;
    let (cfg, cfg_path, cfg_hash) = config(directory, &server.uri);
    let mut client = AcpClient::start(cfg, directory);
    client
        .send(json!({"jsonrpc":"2.0","id":"init","method":"initialize",
        "params":{"protocolVersion":1}}))
        .await;
    let (initialized, frames) = client.until("init", false).await;
    assert!(frames.is_empty());
    assert_eq!(initialized["result"]["protocolVersion"], 1);
    assert_eq!(
        initialized["result"]["_meta"]["craftsman"]["extensions"],
        json!([
            "_craftsman/session/request_input",
            "_craftsman/session/set_model",
            "_craftsman/session/list"
        ])
    );
    let thread = "provider-boundary-acp";
    client
        .send(json!({"jsonrpc":"2.0","id":"new","method":"session/new",
        "params":{"cwd":directory,"mcpServers":[],"_meta":{"craftsman":{"threadId":thread}}}}))
        .await;
    let (created, _) = client.until("new", false).await;
    assert_eq!(created["result"]["sessionId"], thread);
    client
        .send(
            json!({"jsonrpc":"2.0","id":"prompt","method":"session/prompt",
        "params":{"sessionId":thread,"prompt":[{"type":"text","text":INPUT}]}}),
        )
        .await;
    let (finished, frames) = client.until("prompt", true).await;
    assert_eq!(finished["jsonrpc"], "2.0");
    assert_eq!(finished["id"], "prompt");
    if scenario == Scenario::TwentiethControl {
        assert!(finished.get("error").is_none());
        assert_eq!(finished["result"]["stopReason"], "end_turn");
        assert_eq!(
            finished["result"]["_meta"]["craftsman"]["usage"],
            json!({"inputTokens":410,"outputTokens":610})
        );
    } else {
        assert_eq!(
            finished,
            json!({"jsonrpc":"2.0","id":"prompt",
                "error":{"code":-32603,"message":"agent turn failed"}})
        );
    }
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame["method"] == "session/request_permission")
            .count(),
        1
    );
    let actual_results = completed_file_results(&frames);
    assert_eq!(actual_results.len(), scenario.tool_results());
    assert!(actual_results[0].contains("Updated file") && actual_results[0].contains(FILE));
    assert_eq!(
        std::fs::read_to_string(directory.join(FILE)).unwrap(),
        CONTENT
    );
    let initial = server.snapshot(scenario.initial_requests()).await;
    assert_http(&initial, scenario, false);

    // A cached live ACP load replays actual results, independently of the
    // non-durable disk progress policy. It must not sample or approve tools.
    client
        .send(json!({"jsonrpc":"2.0","id":"load","method":"session/load",
        "params":{"sessionId":thread,"cwd":directory,"mcpServers":[]}}))
        .await;
    let (loaded, replay) = client.until("load", false).await;
    assert!(loaded["result"].is_object());
    assert_eq!(completed_file_results(&replay), actual_results);
    assert_eq!(
        server.snapshot(scenario.initial_requests()).await.len(),
        scenario.initial_requests()
    );
    client.finish().await;
    let exchanges = server.finish().await;
    assert_http(&exchanges, scenario, false);
    let store = LibsqlCheckpointer::open_for_workspace(directory)
        .await
        .unwrap();
    let (_, stored, suspension) = store.load_latest(thread).await.unwrap().unwrap();
    assert!(suspension.is_none());
    assert!(!stored.custom.contains_key("_craftsman_durable_turn_v1"));
    let expected_saved = if scenario == Scenario::TwentiethControl {
        scenario.tool_results()
    } else {
        scenario.tool_results() - 1
    };
    assert_eq!(tool_results(&stored).len(), expected_saved);
    let actual_cfg_hash: [u8; 32] = Sha256::digest(std::fs::read(cfg_path).unwrap()).into();
    assert_eq!(actual_cfg_hash, cfg_hash);
    eprintln!(
        "{OBSERVATION}{}",
        json!({"frontend":"acp","scenario":format!("{scenario:?}"),
            "owned_durable":false,"http":http_observation(&exchanges),
            "approved_writes":frames.iter().filter(|frame|frame["method"]=="session/request_permission").count(),
            "cached_replay_results":completed_file_results(&replay).len(),
            "saved_tool_results":tool_results(&stored).len(),
            "prompt_response":finished})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned provider-boundary fixture; unmarked/unknown startup returns before environment/config/workspace access"]
async fn native_provider_failure_boundary_fixture() {
    let Some(case) = selected(std::env::args_os()) else {
        return;
    };
    let directory = std::env::current_dir().unwrap();
    assert!(directory.is_absolute());
    match case {
        Case::Builder(scenario, owned) => builder_case(&directory, scenario, owned).await,
        Case::Acp(scenario) => acp_case(&directory, scenario).await,
    }
    eprintln!("{DONE}");
}

async fn run_arguments(arguments: &[String], execute: bool) {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("workspace");
    let config = root.path().join("config");
    std::fs::create_dir(&directory).unwrap();
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .args(arguments)
        .current_dir(&directory)
        .env_clear()
        .env("CRAFTSMAN_CONFIG_DIR", &config)
        .env("HOME", root.path())
        .env("USERPROFILE", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
    let mut child = command.spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap().take(OUTPUT_LIMIT + 1);
    let mut stderr = child.stderr.take().unwrap().take(OUTPUT_LIMIT + 1);
    let result = tokio::time::timeout(Duration::from_secs(20), async {
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
            panic!("actual provider-boundary fixture exceeded its bounded native execution");
        }
    };
    assert!(out.len() as u64 <= OUTPUT_LIMIT && err.len() as u64 <= OUTPUT_LIMIT);
    let out = std::str::from_utf8(&out).unwrap();
    let err = std::str::from_utf8(&err).unwrap();
    assert!(
        status.success(),
        "native assertions failed\nstdout: {out}\nstderr: {err}"
    );
    assert_eq!(
        err.lines().filter(|line| *line == DONE).count(),
        usize::from(execute)
    );
    let observations: Vec<_> = err
        .lines()
        .filter_map(|line| line.strip_prefix(OBSERVATION))
        .collect();
    assert_eq!(observations.len(), usize::from(execute));
    for observation in observations {
        let value: Value = serde_json::from_str(observation).unwrap();
        assert_eq!(value["approved_writes"], 1);
        eprintln!("{OBSERVATION}{value}");
    }
    if !execute {
        assert!(!config.join("config.json").exists() && !config.join("threads.db").exists());
        assert!(std::fs::read_dir(directory).unwrap().next().is_none());
    }
}

async fn run_case(mode: &str) {
    run_arguments(&[format!("{MARKER}{mode}")], true).await;
}

#[tokio::test]
async fn actual_builder_created_only_http_eof_retains_file_and_default_progress_policy() {
    run_case("builder-eof-default").await;
}
#[tokio::test]
async fn actual_owned_builder_created_only_http_eof_retains_durable_file_results() {
    run_case("builder-eof-owned").await;
}
#[tokio::test]
async fn actual_builder_http_402_after_twenty_admissions_keeps_original_results_and_usage() {
    run_case("builder-refusal-default").await;
}
#[tokio::test]
async fn actual_owned_builder_http_402_keeps_durable_results_and_same_admission_ceiling() {
    run_case("builder-refusal-owned").await;
}
#[tokio::test]
async fn actual_builder_exactly_twentieth_http_response_completes_without_extra_request() {
    run_case("builder-control").await;
}
#[tokio::test]
async fn actual_acp_created_only_http_eof_keeps_standard_error_and_live_file_result() {
    run_case("acp-eof").await;
}
#[tokio::test]
async fn actual_acp_http_402_keeps_standard_error_and_replays_real_results_without_work() {
    run_case("acp-refusal").await;
}
#[tokio::test]
async fn actual_acp_exactly_twentieth_http_response_reports_only_completed_usage() {
    run_case("acp-control").await;
}
#[tokio::test]
async fn unmarked_unknown_and_duplicate_provider_helpers_return_before_configuration_access() {
    for arguments in [
        Vec::new(),
        vec![format!("{MARKER}unknown")],
        vec![
            format!("{MARKER}builder-eof-default"),
            format!("{MARKER}builder-eof-default"),
        ],
    ] {
        assert!(selected(arguments.iter().map(OsString::from)).is_none());
        run_arguments(&arguments, false).await;
    }
}
