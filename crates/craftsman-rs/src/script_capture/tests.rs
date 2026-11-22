//! Substantial marked actual-builder/native/HTTP/SQLite evidence. The helper
//! returns before config/environment/artifact work without one registered marker.

use super::*;
use crate::config::Config;
use base64::{Engine, engine::general_purpose::STANDARD};
use craftsman_harness::{Checkpointer, Msg, TurnResult};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Condvar;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "script_capture::tests::normal_session_capture_fixture";
const MARKER: &str = "__craftsman_session_capture__:";
const DONE: &str = "actual normal-session capture and historical preview assertions completed";
const RAW: &[u8] = &[0, 255, 128, b'\r', b'\n', b'Z'];
pub(crate) const ANSWER: &str = "ordinary normal-session read completed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Case {
    Capture,
    Policy,
    Races,
    Boundaries,
    Lifetime,
    Crud,
    Bounds,
    Worker,
    Acp,
    None,
    Rejection,
}
impl Case {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "capture" => Self::Capture,
            "policy" => Self::Policy,
            "races" => Self::Races,
            "boundaries" => Self::Boundaries,
            "lifetime" => Self::Lifetime,
            "crud" => Self::Crud,
            "bounds" => Self::Bounds,
            "worker" => Self::Worker,
            "acp" => Self::Acp,
            "none" => Self::None,
            "rejection" => Self::Rejection,
            _ => return None,
        })
    }
}

/// No bool/path/permission/hash constructor exists, including in test callers.
pub(crate) struct FixtureMarker {
    case: Case,
}
impl FixtureMarker {
    pub(crate) fn case(&self) -> Case {
        self.case
    }
}
fn marked(arguments: impl Iterator<Item = String>) -> Option<FixtureMarker> {
    let mut markers = arguments.filter_map(|arg| arg.strip_prefix(MARKER).map(str::to_owned));
    let name = markers.next()?;
    if markers.next().is_some() {
        return None;
    }
    Some(FixtureMarker {
        case: Case::from_name(&name)?,
    })
}
pub(crate) fn selected_marker() -> Option<FixtureMarker> {
    marked(std::env::args()).filter(|marker| marker.case != Case::None)
}

pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    pub(crate) cfg: Config,
    pub(crate) server: MockServer,
    config_path: PathBuf,
    config_sha: [u8; 32],
}

pub(crate) async fn fixture(_marker: &FixtureMarker) -> Fixture {
    let root = std::env::current_dir().unwrap();
    let server = MockServer::start().await;
    for folder in ["project-a", "project-b/inventory"] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    std::fs::write(root.join("project-a/raw.bin"), RAW).unwrap();
    std::fs::write(root.join("project-a/empty.bin"), []).unwrap();
    std::fs::write(root.join("project-a/value.txt"), b"ordinary source read\n").unwrap();
    std::fs::write(
        root.join("project-b/uncaptured.bin"),
        vec![7; MAX_FILE_BYTES + 1],
    )
    .unwrap();
    // A non-Git multi-project session, larger than the selected target ceiling.
    // New capture never inventories these unrelated files.
    for index in 0..256 {
        std::fs::write(
            root.join(format!("project-b/inventory/{index}.txt")),
            b"unselected",
        )
        .unwrap();
    }
    let config_path = crate::config::config_dir().join("config.json");
    let body = serde_json::to_vec_pretty(&json!({
        "base_url":format!("{}/v1",server.uri()),"api_key":"owned-capture-fixture-key",
        "model":"capture-fixture","harness_profile":"general","voice":{"enabled":false},
        "gadgets":{"enabled":false},"mcp_servers":[],"lsp_servers":{},"permission_mode":"manual",
        "script_capture":{"enabled":true,"root":"../outside-project"},
    }))
    .unwrap();
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&config_path)
        .unwrap();
    std::io::Write::write_all(&mut file, &body).unwrap();
    drop(file);
    let cfg = Config::load().unwrap().unwrap();
    assert_eq!(cfg.model.as_deref(), Some("capture-fixture"));
    assert!(!root.join(".git").exists());
    Fixture {
        root,
        cfg,
        server,
        config_path,
        config_sha: Sha256::digest(body).into(),
    }
}
impl Fixture {
    pub(crate) fn config_preserved(&self) {
        let hash: [u8; 32] = Sha256::digest(std::fs::read(&self.config_path).unwrap()).into();
        assert_eq!(
            hash, self.config_sha,
            "actual loaded config bytes remain exact"
        );
    }
    pub(crate) fn originals_preserved(&self) {
        assert_eq!(
            std::fs::read(self.root.join("project-a/raw.bin")).unwrap(),
            RAW
        );
        assert_eq!(
            std::fs::read(self.root.join("project-a/empty.bin")).unwrap(),
            b""
        );
        assert_eq!(
            std::fs::read(self.root.join("project-a/value.txt")).unwrap(),
            b"ordinary source read\n"
        );
        assert!(!self.root.join("project-a/create.bin").exists());
        self.config_preserved();
    }
}

pub(crate) async fn ordinary_turn(session: &Session, fixture: &Fixture) {
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert!(body["tools"].as_array().unwrap().iter().all(|tool| {
                !tool["function"]["name"].as_str().unwrap().contains("script")
            }));
            let has_result = body["messages"].as_array().unwrap().iter()
                .any(|message| message["role"] == "tool");
            let delta = if has_result { json!({"content":ANSWER}) } else {
                json!({"tool_calls":[{"index":0,"id":"normal-read-call","type":"function",
                    "function":{"name":"read_file","arguments":json!({
                        "file_path":"project-a/value.txt","offset":0,"limit":10
                    }).to_string()}}]})
            };
            let value = json!({"choices":[{"delta":delta,"finish_reason":if has_result{"stop"}else{"tool_calls"}}],
                "usage":{"prompt_tokens":3,"completion_tokens":2}});
            ResponseTemplate::new(200).set_body_raw(format!("data: {value}\n\ndata: [DONE]\n\n"),"text/event-stream")
        }).expect(2).mount(&fixture.server).await;
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        session
            .agent
            .run_turn("Read only the named ordinary source file."),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(result, TurnResult::Done(ref output) if output.final_text() == ANSWER));
    assert_eq!(*session.usage.lock().unwrap(), (6, 4));
    let state = session.agent.state.lock().unwrap().clone();
    assert!(state.messages.iter().any(|message| matches!(message,
        Msg::Tool { content, is_error: false, .. } if content.contains("ordinary source read"))));
    let (_, saved, suspension) = session
        .checkpointer
        .as_ref()
        .unwrap()
        .load_latest(&session.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert!(suspension.is_none());
    assert_eq!(saved.messages, state.messages);
    assert!(
        !saved
            .custom
            .keys()
            .any(|key| key.contains("script") || key.contains("capture"))
    );
    assert_eq!(fixture.server.received_requests().await.unwrap().len(), 2);
    fixture.originals_preserved();
}

fn capture(session: &Session) -> &SessionCapture {
    session
        .script_capture
        .as_ref()
        .expect("actual marked selection")
}
fn candidates(changes: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({"changes":changes})).unwrap()
}
fn file(target: &str, bytes: &[u8]) -> Value {
    json!({"target":target,"after":{"kind":"file","bytes_base64":STANDARD.encode(bytes)}})
}
fn absent(target: &str) -> Value {
    json!({"target":target,"after":{"kind":"absent"}})
}

pub(crate) async fn capture_case(session: &Session, fixture: &Fixture) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let events = seen.clone();
    capture(session).set_hook(Arc::new(move |stage, name| {
        if stage == native::Stage::BeforeRead {
            events.lock().unwrap().push(name.to_owned());
        }
    }));
    let snapshots = capture(session)
        .capture(
            session,
            &[
                "project-a/raw.bin",
                "project-a/empty.bin",
                "project-a/create.bin",
            ],
        )
        .await
        .unwrap();
    capture(session).clear_hook();
    assert_eq!(snapshots.target_count(session).unwrap(), 3);
    assert_eq!(
        snapshots
            .original_bytes(session, "project-a/raw.bin")
            .unwrap(),
        Some(RAW)
    );
    assert_eq!(
        snapshots
            .original_bytes(session, "project-a/empty.bin")
            .unwrap(),
        Some(b"".as_slice())
    );
    assert_eq!(
        snapshots
            .original_bytes(session, "project-a/create.bin")
            .unwrap(),
        None
    );
    assert_eq!(
        snapshots
            .original_bytes(session, "project-b/uncaptured.bin")
            .unwrap_err(),
        CaptureError::Candidate
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["project-a/raw.bin", "project-a/empty.bin"]
    );
    let facts = snapshots
        .historical_facts(session, "project-a/raw.bin")
        .unwrap();
    assert_eq!(facts["before"]["bytes_base64"], STANDARD.encode(RAW));
    assert_eq!(facts["before"]["whole_bytes_and_eof"], true);
    for pair in [
        ("path_before", "path_after"),
        ("handle_before", "handle_after"),
        ("named_after", "named_final"),
    ] {
        assert_eq!(facts["before"][pair.0]["size"], json!(RAW.len()));
        assert_eq!(facts["before"][pair.1]["size"], json!(RAW.len()));
        assert!(facts["before"][pair.0]["modified"].is_object());
        assert!(facts["before"][pair.1]["accessed"].is_object());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let actual = std::fs::metadata(fixture.root.join("project-a/raw.bin")).unwrap();
        let raw = &facts["before"]["path_final"]["unix"];
        assert_eq!(raw["dev"], actual.dev());
        assert_eq!(raw["ino"], actual.ino());
        assert_eq!(raw["mtime_nsec"], actual.mtime_nsec());
        assert_eq!(raw["ctime_nsec"], actual.ctime_nsec());
        assert_eq!(raw["mode"], actual.mode());
        assert_eq!(raw["nlink"], 1);
        for field in [
            "uid",
            "gid",
            "rdev",
            "size",
            "atime",
            "atime_nsec",
            "mtime",
            "ctime",
            "blksize",
            "blocks",
        ] {
            assert!(raw.get(field).is_some(), "full native field {field}");
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let actual = std::fs::File::open(fixture.root.join("project-a/raw.bin"))
            .unwrap()
            .metadata()
            .unwrap();
        assert_eq!(
            facts["before"]["path_final"]["windows"]["creation_time_100ns"],
            actual.creation_time()
        );
        assert_eq!(
            facts["before"]["handle_after"]["windows"]["write_time_100ns"],
            actual.last_write_time()
        );
        assert!(facts["before"]["handle_before"]["change_time_100ns"].is_i64());
        assert_eq!(
            facts["before"]["handle_before"]["change_time_100ns"],
            facts["before"]["handle_after"]["change_time_100ns"]
        );
        assert_eq!(
            facts["before"]["handle_before"]["identity"],
            facts["before"]["named_final"]["identity"]
        );
        assert_eq!(facts["before"]["handle_after"]["nlink"], 1);
    }
    let digest = snapshots.capture_digest(session).unwrap();
    assert_ne!(digest, [0; 32]);
    let same = capture(session)
        .capture(
            session,
            &[
                "project-a/raw.bin",
                "project-a/empty.bin",
                "project-a/create.bin",
            ],
        )
        .await
        .unwrap();
    assert_ne!(
        digest,
        same.capture_digest(session).unwrap(),
        "actual capture serial is bound"
    );
    ordinary_turn(session, fixture).await;
}

pub(crate) async fn policy_case(session: &Session, fixture: &Fixture) {
    let before_reads = Arc::new(Mutex::new(0usize));
    let reads = before_reads.clone();
    capture(session).set_hook(Arc::new(move |stage, _| {
        if stage == native::Stage::BeforeRead {
            *reads.lock().unwrap() += 1;
        }
    }));
    for target in [
        "/etc/passwd",
        "../outside-project/data",
        "project-a/../raw.bin",
        "./project-a/raw.bin",
        "project-a//raw.bin",
        "C:/device",
        "\\\\host\\share",
        "project-a/NUL",
        "project-a/COM\u{00b9}.txt",
        "project-a/CON .txt",
        "project-a/LPT0",
        "project-a/name.",
        "project-a/name ",
        "project-a/a:b",
        "project-a/a?b",
        "project-a/\u{202e}secret",
    ] {
        assert_eq!(
            capture(session).capture(session, &[target]).await.err(),
            Some(CaptureError::Target),
            "{target}"
        );
    }
    for target in [
        "actual-settings/config.json",
        "actual-settings/new-key",
        "project-a/.env",
        "project-a/.env.production",
        "project-a/.ssh/key",
        "project-a/.craftsman/data",
        "project-a/target/image",
        "project-a/node_modules/package",
        "project-a/.npmrc",
        "project-a/.craftsman-exec-tmp-foreign",
        "project-a/credentials.json",
    ] {
        assert_eq!(
            capture(session).capture(session, &[target]).await.err(),
            Some(CaptureError::Denied),
            "{target}"
        );
    }
    assert_eq!(
        capture(session)
            .capture(session, &["missing-parent/leaf.bin"])
            .await
            .err(),
        Some(CaptureError::Parent)
    );
    assert_eq!(
        capture(session)
            .capture(session, &["project-a"])
            .await
            .err(),
        Some(CaptureError::Unsupported)
    );
    for targets in [
        vec!["project-a/raw.bin", "project-a/raw.bin"],
        vec!["project-a/raw.bin", "PROJECT-A/RAW.BIN"],
        vec!["project-a/\u{00e9}.bin", "project-a/e\u{0301}.bin"],
    ] {
        assert_eq!(
            capture(session).capture(session, &targets).await.err(),
            Some(CaptureError::Target)
        );
    }
    std::fs::hard_link(
        fixture.root.join("project-a/raw.bin"),
        fixture.root.join("project-a/linked.bin"),
    )
    .unwrap();
    assert_eq!(
        capture(session)
            .capture(session, &["project-a/linked.bin"])
            .await
            .err(),
        Some(CaptureError::Unsupported)
    );
    std::fs::remove_file(fixture.root.join("project-a/linked.bin")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("raw.bin", fixture.root.join("project-a/link.bin")).unwrap();
        symlink(
            fixture.root.join("project-a"),
            fixture.root.join("parent-link"),
        )
        .unwrap();
        assert_eq!(
            capture(session)
                .capture(session, &["project-a/link.bin"])
                .await
                .err(),
            Some(CaptureError::Unsupported)
        );
        assert_eq!(
            capture(session)
                .capture(session, &["parent-link/raw.bin"])
                .await
                .err(),
            Some(CaptureError::Unsupported)
        );
        use std::os::unix::ffi::OsStrExt;
        let fifo = fixture.root.join("project-a/pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(
            capture(session)
                .capture(session, &["project-a/pipe"])
                .await
                .err(),
            Some(CaptureError::Unsupported)
        );
        assert_eq!(
            capture(session)
                .capture(session, &["project-a/good", "project-a/pipe"])
                .await
                .err(),
            Some(CaptureError::Unsupported)
        );
    }
    assert_eq!(
        *before_reads.lock().unwrap(),
        0,
        "all refused sets return before content copies"
    );
    capture(session).clear_hook();
    fixture.originals_preserved();
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

pub(crate) async fn crud_case(session: &Session, fixture: &Fixture) {
    let snapshots = capture(session)
        .capture(
            session,
            &[
                "project-a/raw.bin",
                "project-a/empty.bin",
                "project-a/create.bin",
                "project-a/value.txt",
            ],
        )
        .await
        .unwrap();
    let preview = capture(session)
        .preview(
            session,
            &snapshots,
            &candidates(vec![
                file("project-a/create.bin", b""),
                file("project-a/raw.bin", b""),
                absent("project-a/empty.bin"),
            ]),
        )
        .unwrap();
    assert_eq!(
        preview.operations(session).unwrap(),
        vec![
            ("project-a/create.bin".into(), Operation::Create),
            ("project-a/raw.bin".into(), Operation::Update),
            ("project-a/empty.bin".into(), Operation::Delete),
        ]
    );
    let frame: Value =
        serde_json::from_slice(preview.complete_historical_preview(session).unwrap()).unwrap();
    assert_eq!(frame["status"], "historical-prospective");
    assert_eq!(frame["scope"]["root"], fixture.root.to_str().unwrap());
    assert_eq!(
        frame["scope"]["config_origin"],
        fixture.config_path.parent().unwrap().to_str().unwrap()
    );
    assert_eq!(frame["operations"].as_array().unwrap().len(), 3);
    assert_eq!(frame["operations"][0]["after"]["bytes_base64"], "");
    assert_eq!(
        frame["operations"][1]["original"]["before"]["bytes_base64"],
        STANDARD.encode(RAW)
    );
    assert_eq!(
        frame["operations"][2]["original"]["before"]["bytes_base64"],
        ""
    );
    assert_ne!(preview.plan_digest(session).unwrap(), [0; 32]);
    let noops = capture(session)
        .preview(
            session,
            &snapshots,
            &candidates(vec![
                file("project-a/raw.bin", RAW),
                file("project-a/empty.bin", b""),
                absent("project-a/create.bin"),
            ]),
        )
        .unwrap();
    assert!(
        noops.operations(session).unwrap().is_empty(),
        "restoration/transient creation is a no-op"
    );
    assert!(
        capture(session)
            .preview(session, &snapshots, &candidates(vec![]))
            .unwrap()
            .operations(session)
            .unwrap()
            .is_empty(),
        "omission never deletes"
    );
    let binary = [0, 254, 253, b'\n'];
    let binary_plan = capture(session)
        .preview(
            session,
            &snapshots,
            &candidates(vec![
                file("project-a/raw.bin", &binary),
                file("project-a/empty.bin", &binary),
            ]),
        )
        .unwrap();
    let binary_frame: Value =
        serde_json::from_slice(binary_plan.complete_historical_preview(session).unwrap()).unwrap();
    assert_eq!(
        binary_frame["operations"][0]["after"]["bytes_base64"],
        STANDARD.encode(binary)
    );
    assert_eq!(binary_plan.operations(session).unwrap().len(), 2);
    for changes in [
        vec![absent("project-b/uncaptured.bin")],
        vec![file("project-b/uncaptured.bin", b"")],
        vec![file("new-uncaptured.bin", b"")],
        vec![absent("project-a")],
        vec![file("project-a/raw.bin", b"a"), absent("project-a/raw.bin")],
        vec![
            file("project-a/raw.bin", b"a"),
            file("PROJECT-A/RAW.BIN", b"a"),
        ],
        vec![file("../outside-project/data", b"a")],
    ] {
        assert_eq!(
            capture(session)
                .preview(session, &snapshots, &candidates(changes))
                .err(),
            Some(CaptureError::Candidate)
        );
    }
    let malformed: Vec<Vec<u8>> = vec![
        br#"{"changes":[],"before_hash":"trusted"}"#.to_vec(),
        br#"{"changes":[],"permission":"allow","source":"approved"}"#.to_vec(),
        br#"{"changes":[],"changes":[]}"#.to_vec(),
        br#"{"changes":[{"target":"project-a/raw.bin","target":"project-a/empty.bin","after":{"kind":"absent"}}]}"#.to_vec(),
        br#"{"changes":[{"target":"project-a/raw.bin","after":{"kind":"absent","kind":"file","bytes_base64":""}}]}"#.to_vec(),
        br#"{"changes":[{"target":"project-a/raw.bin","after":{"kind":"file","bytes_base64":"Zh=="}}]}"#.to_vec(),
        br#"{"changes":[{"target":"project-a/raw.bin","after":{"kind":"file","bytes_base64":"","mode":511}}]}"#.to_vec(),
        br#"{"changes":[{"target":"project-a/raw.bin","after":{"kind":"file","bytes_base64":"","size":0}}]}"#.to_vec(),
        br#"{"changes":[{"target":"project-a/raw.bin","after":{"kind":"file","bytes":[0,255]}}]}"#.to_vec(),
        br#"{"changes":[{"target":"project-a/raw.bin","after":{"kind":"directory"}}]}"#.to_vec(),
        br#"{"changes":[{"target":"project-a/raw.bin","after":{"kind":"absent","source":"host"}}]}"#.to_vec(),
        vec![255,0],
    ];
    for raw in malformed {
        assert_eq!(
            capture(session).preview(session, &snapshots, &raw).err(),
            Some(CaptureError::Candidate),
            "malformed synthetic candidate: {}",
            String::from_utf8_lossy(&raw)
        );
    }
    // Current external mutation does not get silently rebased. The plan keeps
    // its exact HISTORICAL original and makes no current-staleness claim.
    std::fs::write(fixture.root.join("project-a/raw.bin"), b"external now").unwrap();
    let historical = capture(session)
        .preview(
            session,
            &snapshots,
            &candidates(vec![absent("project-a/raw.bin")]),
        )
        .unwrap();
    let old: Value =
        serde_json::from_slice(historical.complete_historical_preview(session).unwrap()).unwrap();
    assert_eq!(
        old["operations"][0]["original"]["before"]["bytes_base64"],
        STANDARD.encode(RAW)
    );
    assert_eq!(
        std::fs::read(fixture.root.join("project-a/raw.bin")).unwrap(),
        b"external now"
    );
    std::fs::write(fixture.root.join("project-a/raw.bin"), RAW).unwrap();
    fixture.originals_preserved();
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

pub(crate) async fn bounds_case(session: &Session, fixture: &Fixture) {
    let seen = Arc::new(Mutex::new(0));
    let reads = seen.clone();
    capture(session).set_hook(Arc::new(move |stage, _| {
        if stage == native::Stage::BeforeRead {
            *reads.lock().unwrap() += 1;
        }
    }));
    let names: Vec<_> = (0..129).map(|i| format!("new-{i}.bin")).collect();
    let refs: Vec<_> = names.iter().map(String::as_str).collect();
    assert_eq!(
        capture(session).capture(session, &refs).await.err(),
        Some(CaptureError::Bounds)
    );
    let oversized = vec![b'P'; MAX_FILE_BYTES + 1];
    std::fs::write(fixture.root.join("project-a/oversized.bin"), oversized).unwrap();
    assert_eq!(
        capture(session)
            .capture(session, &["project-a/oversized.bin"])
            .await
            .err(),
        Some(CaptureError::Bounds)
    );
    for index in 0..4 {
        std::fs::write(
            fixture.root.join(format!("project-a/full-{index}.bin")),
            vec![index as u8; MAX_FILE_BYTES],
        )
        .unwrap();
    }
    let full: Vec<_> = (0..4).map(|i| format!("project-a/full-{i}.bin")).collect();
    let refs: Vec<_> = full.iter().map(String::as_str).collect();
    assert_eq!(
        capture(session).capture(session, &refs).await.err(),
        Some(CaptureError::Bounds)
    );
    assert_eq!(
        *seen.lock().unwrap(),
        0,
        "whole set bytes are admitted before any content copy"
    );
    capture(session).clear_hook();
    let admitted = capture(session).capture(session, &refs[..3]).await.unwrap();
    for target in &full[..3] {
        assert_eq!(
            admitted
                .original_bytes(session, target)
                .unwrap()
                .unwrap()
                .len(),
            MAX_FILE_BYTES
        );
    }
    let too_large = candidates(vec![file(&full[0], &vec![0; MAX_FILE_BYTES + 1])]);
    assert_eq!(
        capture(session)
            .preview(session, &admitted, &too_large)
            .err(),
        Some(CaptureError::Bounds)
    );
    let count_over = candidates((0..129).map(|_| absent(&full[0])).collect());
    assert_eq!(
        capture(session)
            .preview(session, &admitted, &count_over)
            .err(),
        Some(CaptureError::Candidate)
    );
    let mut names: Vec<_> = (0..4).map(|i| format!("project-a/absent-{i}")).collect();
    names.extend(full[..3].iter().cloned());
    let refs: Vec<_> = names.iter().map(String::as_str).collect();
    let originals = capture(session).capture(session, &refs).await.unwrap();
    let candidates_over = candidates(
        names[..4]
            .iter()
            .map(|target| file(target, &vec![0; MAX_FILE_BYTES]))
            .collect(),
    );
    assert_eq!(
        capture(session)
            .preview(session, &originals, &candidates_over)
            .err(),
        Some(CaptureError::Bounds)
    );
    assert_eq!(
        capture(session)
            .preview(session, &originals, &vec![b' '; MAX_PREVIEW_BYTES + 1])
            .err(),
        Some(CaptureError::Bounds)
    );
    // Use full actual native evidence and binary originals, rather than fake
    // large metadata. Complete encoded overflow must refuse the WHOLE plan.
    let parent = (0..6).map(|_| "d").collect::<Vec<_>>().join("/");
    std::fs::create_dir_all(fixture.root.join(&parent)).unwrap();
    let mut many = Vec::new();
    for index in 0..128 {
        let leaf = format!("{index:03}{}", "x".repeat(215));
        let target = format!("{parent}/{leaf}");
        std::fs::write(fixture.root.join(&target), vec![1; 1536]).unwrap();
        many.push(target);
    }
    let refs: Vec<_> = many.iter().map(String::as_str).collect();
    let originals = capture(session).capture(session, &refs).await.unwrap();
    let large = candidates(
        many.iter()
            .map(|target| file(target, &vec![2; 1536]))
            .collect(),
    );
    assert_eq!(
        capture(session).preview(session, &originals, &large).err(),
        Some(CaptureError::Bounds)
    );
    for target in &many {
        assert_eq!(
            std::fs::read(fixture.root.join(target)).unwrap(),
            vec![1; 1536]
        );
    }
    fixture.originals_preserved();
}

pub(crate) async fn races_case(session: &Session, fixture: &Fixture) {
    for stage in [
        native::Stage::BeforeRead,
        native::Stage::BeforeEof,
        native::Stage::AfterRead,
    ] {
        let target = fixture.root.join("project-a/raw.bin");
        let once = Arc::new(AtomicBool::new(false));
        let mutated = once.clone();
        capture(session).set_hook(Arc::new(move |observed, name| {
            if observed == stage
                && name == "project-a/raw.bin"
                && !mutated.swap(true, Ordering::SeqCst)
            {
                if stage == native::Stage::BeforeEof {
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&target)
                        .unwrap();
                    std::io::Write::write_all(&mut file, b"+").unwrap();
                } else {
                    std::fs::write(&target, [0, 254, 128, b'\r', b'\n', b'Z']).unwrap();
                }
            }
        }));
        assert_eq!(
            capture(session)
                .capture(session, &["project-a/raw.bin"])
                .await
                .err(),
            Some(CaptureError::Changed)
        );
        assert!(once.load(Ordering::SeqCst));
        capture(session).clear_hook();
        std::fs::write(fixture.root.join("project-a/raw.bin"), RAW).unwrap();
    }
    let absent_leaf = fixture.root.join("project-a/create.bin");
    capture(session).set_hook(Arc::new(move |stage, _| {
        if stage == native::Stage::Admitted {
            std::fs::write(&absent_leaf, b"raced create").unwrap();
        }
    }));
    assert_eq!(
        capture(session)
            .capture(session, &["project-a/create.bin"])
            .await
            .err(),
        Some(CaptureError::Changed)
    );
    capture(session).clear_hook();
    assert_eq!(
        std::fs::read(fixture.root.join("project-a/create.bin")).unwrap(),
        b"raced create"
    );
    std::fs::remove_file(fixture.root.join("project-a/create.bin")).unwrap();
    let root = fixture.root.clone();
    let moved = Arc::new(Mutex::new(None));
    let outcome = moved.clone();
    capture(session).set_hook(Arc::new(move |stage, name| {
        if stage == native::Stage::BeforeRead && name == "project-a/raw.bin" {
            let result = std::fs::rename(root.join("project-a"), root.join("retired-project-a"));
            let code = result.as_ref().err().and_then(std::io::Error::raw_os_error);
            *outcome.lock().unwrap() = Some((result.is_ok(), code));
            if result.is_ok() {
                std::fs::create_dir(root.join("project-a")).unwrap();
                std::fs::write(root.join("project-a/raw.bin"), RAW).unwrap();
            }
        }
    }));
    let result = capture(session)
        .capture(session, &["project-a/raw.bin"])
        .await;
    capture(session).clear_hook();
    let (moved, code) = moved.lock().unwrap().unwrap();
    if moved {
        assert_eq!(result.err(), Some(CaptureError::Changed));
        std::fs::remove_file(fixture.root.join("project-a/raw.bin")).unwrap();
        std::fs::remove_dir(fixture.root.join("project-a")).unwrap();
        std::fs::rename(
            fixture.root.join("retired-project-a"),
            fixture.root.join("project-a"),
        )
        .unwrap();
    } else {
        #[cfg(windows)]
        {
            assert!(
                matches!(code, Some(5 | 32)),
                "only actual native sharing/access refusal is supported"
            );
            assert_eq!(
                result
                    .unwrap()
                    .original_bytes(session, "project-a/raw.bin")
                    .unwrap(),
                Some(RAW)
            );
        }
        #[cfg(not(windows))]
        panic!("owned parent rename unexpectedly refused: {code:?}");
    }
    fixture.originals_preserved();
}

pub(crate) async fn boundaries_case(session: &Session, fixture: &mut Fixture) {
    let context = capture(session);
    let originals = context
        .capture(session, &["project-a/raw.bin"])
        .await
        .unwrap();
    let before = context
        .preview(session, &originals, br#"{"changes":[]}"#)
        .unwrap();
    let frame: Value =
        serde_json::from_slice(before.complete_historical_preview(session).unwrap()).unwrap();
    assert_eq!(frame["scope"]["root"], fixture.root.to_str().unwrap());
    let config = fixture.config_path.parent().unwrap().to_path_buf();
    let moved_config = fixture.root.join("moved-settings");
    let outcome = std::fs::rename(&config, &moved_config);
    if outcome.is_ok() {
        std::fs::create_dir(&config).unwrap();
        std::fs::write(config.join("config.json"), b"foreign origin stand-in").unwrap();
        let reads = Arc::new(Mutex::new(0usize));
        let seen = reads.clone();
        context.set_hook(Arc::new(move |stage, _| {
            if stage == native::Stage::BeforeRead {
                *seen.lock().unwrap() += 1;
            }
        }));
        assert_eq!(
            context.capture(session, &["project-a/raw.bin"]).await.err(),
            Some(CaptureError::Changed)
        );
        assert_eq!(
            context
                .capture(session, &["moved-settings/config.json"])
                .await
                .err(),
            Some(CaptureError::Changed)
        );
        assert_eq!(
            *reads.lock().unwrap(),
            0,
            "a rebound actual config origin refuses before bytes"
        );
        context.clear_hook();
        assert_eq!(
            std::fs::read(config.join("config.json")).unwrap(),
            b"foreign origin stand-in"
        );
        std::fs::rename(&config, fixture.root.join("foreign-settings")).unwrap();
        std::fs::rename(&moved_config, &config).unwrap();
        fixture.config_preserved();
    } else {
        #[cfg(windows)]
        {
            let code = outcome.unwrap_err().raw_os_error();
            assert!(matches!(code, Some(5 | 32)));
            assert_eq!(
                context
                    .capture(session, &["project-a/raw.bin"])
                    .await
                    .unwrap()
                    .original_bytes(session, "project-a/raw.bin")
                    .unwrap(),
                Some(RAW)
            );
            fixture.config_preserved();
        }
        #[cfg(not(windows))]
        panic!("actual owned config rename refused: {:?}", outcome.err());
    }
    let old_root = fixture.root.clone();
    let moved_root = old_root.parent().unwrap().join("moved-workspace");
    let outcome = std::fs::rename(&old_root, &moved_root);
    if outcome.is_ok() {
        std::fs::create_dir_all(old_root.join("project-a")).unwrap();
        std::fs::write(
            old_root.join("project-a/raw.bin"),
            b"foreign workspace stand-in",
        )
        .unwrap();
        let reads = Arc::new(Mutex::new(0usize));
        let seen = reads.clone();
        context.set_hook(Arc::new(move |stage, _| {
            if stage == native::Stage::BeforeRead {
                *seen.lock().unwrap() += 1;
            }
        }));
        assert_eq!(
            context.capture(session, &["project-a/raw.bin"]).await.err(),
            Some(CaptureError::Changed)
        );
        assert_eq!(
            *reads.lock().unwrap(),
            0,
            "a rebound actual working folder is never ambiently reopened"
        );
        context.clear_hook();
        assert_eq!(
            std::fs::read(old_root.join("project-a/raw.bin")).unwrap(),
            b"foreign workspace stand-in"
        );
        fixture.root = moved_root;
        fixture.config_path = fixture.root.join("actual-settings/config.json");
    } else {
        #[cfg(windows)]
        {
            let code = outcome.unwrap_err().raw_os_error();
            assert!(matches!(code, Some(5 | 32)));
            assert_eq!(
                context
                    .capture(session, &["project-a/raw.bin"])
                    .await
                    .unwrap()
                    .original_bytes(session, "project-a/raw.bin")
                    .unwrap(),
                Some(RAW)
            );
        }
        #[cfg(not(windows))]
        panic!("actual owned workspace rename refused: {:?}", outcome.err());
    }
    fixture.originals_preserved();
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

pub(crate) async fn worker_case(session: &Session, fixture: &Fixture) {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let blocked = gate.clone();
    let ready = Arc::new(tokio::sync::Notify::new());
    let entered = ready.clone();
    capture(session).set_hook(Arc::new(move |stage, _| {
        if stage == native::Stage::Admitted {
            entered.notify_one();
            let (lock, changed) = &*blocked;
            let mut release = lock.lock().unwrap();
            while !*release {
                release = changed.wait(release).unwrap();
            }
        }
    }));
    let mut read = Box::pin(capture(session).capture(session, &["project-a/raw.bin"]));
    tokio::select! {
        result = &mut read => panic!("held actual native read returned early: {:?}", result.err()),
        _ = ready.notified() => {}
    }
    assert_eq!(
        capture(session)
            .capture(session, &["project-a/raw.bin"])
            .await
            .err(),
        Some(CaptureError::Busy)
    );
    drop(read); // The caller future disappears; the owner retains the real join.
    let mut close = Box::pin(session.close_script_capture());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut close)
            .await
            .is_err(),
        "close must await the actual held worker return"
    );
    {
        let (lock, changed) = &*gate;
        *lock.lock().unwrap() = true;
        changed.notify_one();
    }
    tokio::time::timeout(Duration::from_secs(5), close)
        .await
        .unwrap();
    assert_eq!(
        capture(session)
            .capture(session, &["project-a/raw.bin"])
            .await
            .err(),
        Some(CaptureError::Revoked)
    );
    capture(session).clear_hook();
    fixture.originals_preserved();
}

async fn run_case(name: &str, additional: &[&str]) {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let config = workspace.join("actual-settings");
    std::fs::create_dir_all(&config).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let config = config.canonicalize().unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--ignored", "--exact", HELPER, "--nocapture"]);
    if name != "unmarked" {
        command.arg(format!("{MARKER}{name}"));
    }
    command
        .args(additional)
        .current_dir(&workspace)
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
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("actual capture fixture did not return within its bounded native lifetime");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let out = std::str::from_utf8(&out).unwrap();
    let err = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "actual fixture failed\n{out}\n{err}");
    let selected = Case::from_name(name).is_some() && additional.is_empty();
    assert_eq!(
        err.lines().filter(|line| *line == DONE).count(),
        usize::from(selected)
    );
    if !selected {
        assert!(
            !config.join("config.json").exists(),
            "unmarked/unknown/duplicate helper returns before config work"
        );
        assert!(!config.join("threads.db").exists());
        assert!(!workspace.join("project-a").exists());
    }
}

#[test]
#[ignore = "native subprocess only; exact registered marker required before config/environment/artifact work"]
fn normal_session_capture_fixture() {
    let Some(marker) = marked(std::env::args()) else {
        return;
    };
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            if marker.case == Case::Acp {
                crate::acp_serve::script_capture_fixture(&marker).await;
            } else {
                crate::tui::script_capture_fixture(&marker).await;
            }
        });
    eprintln!("{DONE}");
}

#[tokio::test]
async fn actual_tui_capture_retains_missing_empty_binary_native_facts_and_normal_http_sqlite() {
    run_case("capture", &[]).await;
}
#[tokio::test]
async fn actual_scope_denies_sensitive_alias_link_device_and_missing_parent_before_copy() {
    run_case("policy", &[]).await;
}
#[tokio::test]
async fn actual_native_mutations_eof_absence_and_parent_rebind_refuse_whole_capture() {
    run_case("races", &[]).await;
}
#[tokio::test]
async fn actual_retained_root_and_config_origin_identity_rebinds_refuse_foreign_bytes() {
    run_case("boundaries", &[]).await;
}
#[tokio::test]
async fn actual_generations_foreign_equal_sessions_transfer_load_rebuild_and_close_revoke() {
    run_case("lifetime", &[]).await;
}
#[tokio::test]
async fn actual_binary_empty_net_crud_rejects_malicious_candidates_without_real_apply() {
    run_case("crud", &[]).await;
}
#[tokio::test]
async fn actual_whole_count_byte_path_and_encoded_preview_bounds_preserve_originals() {
    run_case("bounds", &[]).await;
}
#[tokio::test]
async fn dropped_capture_wait_retains_real_native_worker_through_actual_close_return() {
    run_case("worker", &[]).await;
}
#[tokio::test]
async fn actual_acp_selection_preserves_nondurable_policy_http_sqlite_and_teardown() {
    run_case("acp", &[]).await;
}
#[tokio::test]
async fn actual_none_frontends_ignore_forged_config_selection_and_keep_old_behavior() {
    run_case("none", &[]).await;
}
#[tokio::test]
async fn actual_unowned_foreign_and_prepared_builders_reject_selection_before_effects() {
    run_case("rejection", &[]).await;
}
#[tokio::test]
async fn unmarked_unknown_duplicate_helpers_return_before_config_environment_artifacts() {
    run_case("unmarked", &[]).await;
    run_case("unknown", &[]).await;
    run_case("capture", &["__craftsman_session_capture__:capture"]).await;
}

#[path = "original_windows_probe.rs"]
pub(crate) mod original_windows_probe;
