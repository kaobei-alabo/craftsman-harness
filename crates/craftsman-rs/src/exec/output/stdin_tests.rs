use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::input::stdin::Limits;
use crate::exec::input::{Sources, StdinFileInvocation};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::platform_os::stdin_pipe::{self, Pipe};
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
const KEY: &str = "composed-native-output-synthetic-secret";
struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    output: PathBuf,
    server: MockServer,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let workspace = base.join("workspace");
        let config = base.join("selected-config");
        let output = base.join("output");
        for directory in [&workspace, &config, &output] {
            std::fs::create_dir(directory).unwrap();
        }
        let f = Self {
            _root: root,
            workspace,
            config,
            output,
            server: MockServer::start().await,
        };
        std::fs::write(
            f.config.join("config.json"),
            json!({"model":"synthetic-model", "api_key":KEY, "base_url":f.server.uri()})
                .to_string(),
        )
        .unwrap();
        f
    }
    fn options(&self, session: SessionSelection) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session,
            selectors: Selectors {
                on_unapproved: OnUnapproved::Fail,
                ..Default::default()
            },
        }
    }
    fn destination(&self, format: Format, max_bytes: usize) -> Destination {
        Destination {
            directory: self.output.clone(),
            file_name: "result.out".into(),
            format,
            max_bytes,
        }
    }
    fn entries(&self) -> Vec<String> {
        let mut names = std::fs::read_dir(&self.output)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        names
    }
    fn no_store(&self) {
        for p in ["threads.db", "sessions-rs", "gadgets/.sdk"] {
            assert!(!self.config.join(p).exists());
        }
    }
    async fn no_effects(&self) {
        self.no_store();
        assert!(self.entries().is_empty());
        assert!(self.server.received_requests().await.unwrap().is_empty());
    }
    async fn respond(&self, text: &str) {
        let event = json!({"choices":[{"delta":{"content":text},"finish_reason":"stop"}]});
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {event}\n\ndata: [DONE]\n\n"),
                "text/event-stream",
            ))
            .mount(&self.server)
            .await;
    }
}
fn sources() -> Sources<'static> {
    Sources {
        prompt: Some(b"-"),
        prompt_file: None,
        files: &[],
    }
}
fn queued(bytes: &[u8]) -> (Pipe, std::fs::File) {
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    writer.write_all(bytes).unwrap();
    drop(writer);
    (Pipe::owned_for_tests(reader).unwrap(), retained)
}
#[tokio::test]
async fn actual_native_input_run_uses_the_existing_guarded_ascii_controller() {
    let f = Fixture::new().await;
    f.respond(&format!("中文 🦀 {KEY}")).await;
    let (pipe, retained) = queued("\u{feff}original goal\r\n".as_bytes());
    let invocation = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
        pipe,
        Limits::default(),
        || {},
    )
    .await
    .unwrap();
    f.no_effects().await;
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    let run = invocation.run().await;
    assert!(run.run_for_tests().outcome().is_ok());
    assert_eq!(
        run.run_for_tests().facts().instruction().kind(),
        crate::exec::input::OriginKind::Stdin
    );
    assert_eq!(
        run.run_for_tests().facts().instruction().bytes(),
        "\u{feff}original goal\r\n".len()
    );
    assert!(run.run_for_tests().facts().instruction().bom_removed());
    assert!(f.entries().is_empty());
    let mut terminal = run.terminal();
    assert!(f.entries().is_empty());
    terminal.publish().unwrap();
    let bytes = std::fs::read(f.output.join("result.out")).unwrap();
    assert!(bytes.is_ascii());
    assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["final_output"], "中文 🦀 [REDACTED]");
    assert!(value.get("usage").is_none() && value.get("status").is_none());
    f.no_store();
}

use super::write::{Event, Signals};
use crate::exec::construction::{
    PreparationError, StdinOutputPreparationError, StdinPreparationError,
};
use crate::exec::input::{FileInvocation, InputError, OriginKind, StdinError};
use crate::exec::runtime::{InvocationGate, RuntimeError};
use crate::exec::terminal::{Error as TerminalError, Terminal};
use craftsman_harness::ToolErrorKind;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

impl Fixture {
    async fn no_execution(&self) {
        self.no_store();
        assert!(self.server.received_requests().await.unwrap().is_empty());
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.output.join("result.out")).unwrap()
    }
    fn value(&self) -> serde_json::Value {
        let bytes = self.bytes();
        assert!(bytes.is_ascii());
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
        serde_json::from_slice(&bytes).unwrap()
    }
    async fn invocation(
        &self,
        bytes: &[u8],
        format: Format,
        maximum: usize,
    ) -> StdinFileInvocation {
        let (pipe, _) = queued(bytes);
        StdinFileInvocation::prepare_for_tests(
            self.options(SessionSelection::Ephemeral),
            sources(),
            self.destination(format, maximum),
            pipe,
            Limits::default(),
            || {},
        )
        .await
        .unwrap()
    }
    async fn run(&self, text: &str, format: Format) -> crate::exec::input::FileRun {
        self.respond(text).await;
        let run = self
            .invocation(b"original goal", format, CAP)
            .await
            .run()
            .await;
        assert!(run.run_for_tests().outcome().is_ok());
        run
    }
}
fn failure(result: Result<(), Failure>, kind: Error, linked: bool) {
    let e = result.unwrap_err();
    assert_eq!(e.kind(), kind);
    assert_eq!(e.link_returned_success(), linked);
    assert_eq!(e.to_string(), "[REDACTED]");
}
fn gate(run: &crate::exec::input::FileRun) -> Arc<InvocationGate> {
    run.run_for_tests().runtime_for_tests().gate_for_tests()
}
fn unread(file: File, expected: &[u8]) {
    assert!(!stdin_pipe::nonblocking_for_tests(&file).unwrap());
    let mut pipe = Pipe::owned_for_tests(file).unwrap();
    let mut buffer = [0; stdin_pipe::STEP];
    assert_eq!(
        pipe.read(&mut buffer).unwrap(),
        stdin_pipe::Read::Bytes(expected.len())
    );
    assert_eq!(&buffer[..expected.len()], expected);
    assert_eq!(pipe.read(&mut buffer).unwrap(), stdin_pipe::Read::End);
    pipe.finish().unwrap();
}
async fn reject_before_read(f: &Fixture, destination: Destination, expected: Error) {
    let bytes = b"must remain unread";
    let (pipe, retained) = queued(bytes);
    let error = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Fresh),
        sources(),
        destination,
        pipe,
        Limits::default(),
        || panic!("invalid destination reached native input phase"),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error, StdinOutputPreparationError::Output(expected));
    unread(retained, bytes);
    f.no_execution().await;
}
fn directory_identity(p: &Path) -> super::path::Identity {
    let dir = cap_std::fs::Dir::open_ambient_dir(p, cap_std::ambient_authority()).unwrap();
    super::path::identity(&dir.dir_metadata().unwrap())
}
fn move_directory(original: &Path, moved: &Path, identity: super::path::Identity) -> bool {
    match std::fs::rename(original, moved) {
        Ok(()) => {
            assert!(!original.exists());
            assert!(directory_identity(moved) == identity);
            true
        }
        #[cfg(windows)]
        Err(e) => {
            assert!(
                matches!(e.raw_os_error(), Some(5 | 32)),
                "unexpected native rename failure: {e:?}"
            );
            assert!(original.is_dir() && !moved.exists());
            assert!(directory_identity(original) == identity);
            false
        }
        #[cfg(not(windows))]
        Err(e) => panic!("native rename failed: {e:?}"),
    }
}
#[tokio::test]
async fn composed_human_preview_keeps_whole_answer_and_private_facts_without_shell_routing() {
    let f = Fixture::new().await;
    f.respond(&format!("中文 {KEY}\r\n##vso[task.x] ::warning::"))
        .await;
    let invocation = f
        .invocation(
            b"!touch forbidden\r\n/model literal",
            Format::HumanPreview,
            CAP,
        )
        .await;
    f.no_effects().await;
    let run = invocation.run().await;
    assert!(!f.workspace.join("forbidden").exists());
    let requests = f.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let value: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(value["messages"].as_array().unwrap().iter().any(|m| {
        m["content"]
            .as_str()
            .is_some_and(|s| s.contains("!touch forbidden"))
    }));
    let mut terminal = run.terminal();
    terminal.publish().unwrap();
    assert_eq!(
        String::from_utf8(f.bytes()).unwrap(),
        "中文 [REDACTED]\\u{000d}\n#\\u{0023}vso[task.x] :\\u{003a}warning:\\u{003a}\ncraftsman: private run return=returned; approval_required=false; final_answer=present\n"
    );
    f.no_store();
}
#[tokio::test]
async fn composed_attachments_keep_original_labels_bytes_and_framing() {
    let f = Fixture::new().await;
    std::fs::write(f.workspace.join("data.txt"), "\u{feff}data\r\n!literal").unwrap();
    f.respond("done").await;
    let (pipe, _) = queued(b"goal");
    let sources = Sources {
        prompt: Some(b"-"),
        prompt_file: None,
        files: &["data.txt", "data.txt"],
    };
    let run = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        sources,
        f.destination(Format::AsciiFragment, CAP),
        pipe,
        Limits::default(),
        || {},
    )
    .await
    .unwrap()
    .run()
    .await;
    let facts = run.run_for_tests().facts();
    assert_eq!(facts.instruction().kind(), OriginKind::Stdin);
    assert_eq!(facts.instruction().bytes(), 4);
    assert_eq!(facts.attachments().len(), 2);
    assert_eq!(
        facts.attachment_bytes(),
        2 * "\u{feff}data\r\n!literal".len()
    );
    let requests = f.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(body["messages"].as_array().unwrap().iter().any(|m| {
        m["content"].as_str().is_some_and(|s| {
            s.starts_with("goal\n\n<craftsman_input_attachments>")
                && s.contains("data.txt")
                && s.contains("\u{feff}data\r\n!literal")
        })
    }));
    run.terminal().publish().unwrap();
    f.no_store();
}
#[tokio::test]
async fn complete_source_conflict_missing_and_attachment_grammar_precede_all_output_or_stdin_io() {
    let f = Fixture::new().await;
    let cases = [
        (
            Sources {
                prompt: None,
                prompt_file: None,
                files: &[],
            },
            StdinError::Input(InputError::MissingSource),
        ),
        (
            Sources {
                prompt: Some(b"-"),
                prompt_file: Some("goal"),
                files: &[],
            },
            StdinError::Input(InputError::ConflictingSources),
        ),
        (
            Sources {
                prompt: Some(b"literal"),
                prompt_file: None,
                files: &[],
            },
            StdinError::NotSelected,
        ),
        (
            Sources {
                prompt: Some(b"-"),
                prompt_file: None,
                files: &[""],
            },
            StdinError::Input(InputError::Selection),
        ),
    ];
    std::fs::write(f.config.join("config.json"), b"invalid config").unwrap();
    for (source, expected) in cases {
        let (pipe, retained) = queued(b"unread");
        let error = StdinFileInvocation::prepare_for_tests(
            f.options(SessionSelection::Fresh),
            source,
            f.destination(Format::AsciiFragment, 0),
            pipe,
            Limits::default(),
            || panic!("invalid selection reached capture"),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            error,
            StdinOutputPreparationError::Stdin(StdinPreparationError::Input(expected))
        );
        unread(retained, b"unread");
        f.no_effects().await;
    }
}
#[tokio::test]
async fn attachment_count_and_path_caps_are_complete_before_native_or_destination_inspection() {
    let f = Fixture::new().await;
    let long = "x".repeat(4097);
    let many = ["data"; 129];
    for files in [&many[..], &[long.as_str()][..]] {
        let (pipe, retained) = queued(b"unread");
        let error = StdinFileInvocation::prepare_for_tests(
            f.options(SessionSelection::Fresh),
            Sources {
                prompt: Some(b"-"),
                prompt_file: None,
                files,
            },
            f.destination(Format::AsciiFragment, CAP),
            pipe,
            Limits::default(),
            || panic!("invalid attachment selection reached capture"),
        )
        .await
        .err();
        assert_eq!(
            error,
            Some(StdinOutputPreparationError::Stdin(
                StdinPreparationError::Input(StdinError::Input(InputError::SelectionLimit))
            ))
        );
        unread(retained, b"unread");
        f.no_effects().await;
    }
}
#[tokio::test]
async fn invalid_output_byte_budgets_precede_config_and_native_capture() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("config.json"), b"invalid config").unwrap();
    for n in [0, CAP + 1] {
        reject_before_read(&f, f.destination(Format::AsciiFragment, n), Error::Limit).await;
        f.no_effects().await;
    }
}
#[tokio::test]
async fn unsafe_original_single_names_are_rejected_without_consuming_input() {
    let f = Fixture::new().await;
    for name in [
        "",
        ".",
        "..",
        "a/b",
        "a\\b",
        "bad\0name",
        ".craftsman-exec-tmp-foreign",
        ".env",
        "build.rs",
    ] {
        let mut d = f.destination(Format::AsciiFragment, CAP);
        d.file_name = name.into();
        reject_before_read(&f, d, Error::Path).await;
        f.no_effects().await;
    }
    let mut d = f.destination(Format::HumanPreview, CAP);
    d.file_name = "x".repeat(256);
    reject_before_read(&f, d, Error::Path).await;
}
#[tokio::test]
async fn relative_parent_and_dot_destination_grammar_never_reads_stdin() {
    let f = Fixture::new().await;
    // Preserve original spellings: Windows verbatim PathBuf::join resolves
    // dot/parent segments before the production parser can reject them.
    let original_segment = |segment: &str| {
        let mut raw = f.output.as_os_str().to_os_string();
        raw.push(std::path::MAIN_SEPARATOR_STR);
        raw.push(segment);
        PathBuf::from(raw)
    };
    for directory in [
        PathBuf::from("relative"),
        original_segment("../output"),
        original_segment("./inner"),
    ] {
        let mut d = f.destination(Format::AsciiFragment, CAP);
        d.directory = directory;
        reject_before_read(&f, d, Error::Path).await;
        f.no_effects().await;
    }
}
#[tokio::test]
async fn retained_workspace_config_and_nested_output_scopes_are_denied_before_stdin() {
    let f = Fixture::new().await;
    std::fs::create_dir(f.workspace.join("child")).unwrap();
    std::fs::create_dir(f.config.join("child")).unwrap();
    for directory in [
        &f.workspace,
        &f.config,
        &f.workspace.join("child"),
        &f.config.join("child"),
    ] {
        let mut d = f.destination(Format::AsciiFragment, CAP);
        d.directory = directory.clone();
        reject_before_read(&f, d, Error::Scope).await;
        f.no_effects().await;
    }
}
#[tokio::test]
async fn existing_final_file_is_preserved_and_does_not_consume_native_bytes_or_inspect_store() {
    let f = Fixture::new().await;
    std::fs::write(f.output.join("result.out"), b"foreign final").unwrap();
    std::fs::write(f.config.join("threads.db"), b"invalid legacy store").unwrap();
    let (pipe, retained) = queued(b"unread");
    let error = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Fresh),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
        pipe,
        Limits::default(),
        || panic!("existing final reached native capture"),
    )
    .await
    .err();
    assert_eq!(
        error,
        Some(StdinOutputPreparationError::Output(Error::Exists))
    );
    unread(retained, b"unread");
    assert_eq!(f.bytes(), b"foreign final");
    assert_eq!(
        std::fs::read(f.config.join("threads.db")).unwrap(),
        b"invalid legacy store"
    );
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn existing_final_directory_is_not_replaced_or_read_as_native_instruction() {
    let f = Fixture::new().await;
    std::fs::create_dir(f.output.join("result.out")).unwrap();
    reject_before_read(&f, f.destination(Format::HumanPreview, CAP), Error::Exists).await;
    assert!(f.output.join("result.out").is_dir());
}
#[tokio::test]
async fn existing_hardlinked_target_is_not_replaced_unlinked_or_used_for_input() {
    let f = Fixture::new().await;
    std::fs::write(f.output.join("foreign"), b"foreign").unwrap();
    std::fs::hard_link(f.output.join("foreign"), f.output.join("result.out")).unwrap();
    reject_before_read(&f, f.destination(Format::AsciiFragment, CAP), Error::Exists).await;
    assert_eq!(f.bytes(), b"foreign");
    assert_eq!(std::fs::read(f.output.join("foreign")).unwrap(), b"foreign");
}
#[tokio::test]
async fn missing_destination_parent_is_not_created_and_native_data_stays_unread() {
    let f = Fixture::new().await;
    let mut d = f.destination(Format::AsciiFragment, CAP);
    d.directory = f.output.join("missing");
    reject_before_read(&f, d, Error::Io(std::io::ErrorKind::NotFound)).await;
    assert!(!f.output.join("missing").exists());
}
#[tokio::test]
async fn invalid_bootstrap_and_credentials_refuse_before_destination_or_input_capture() {
    let f = Fixture::new().await;
    for (config, expected) in [
        (b"invalid json".to_vec(), PreparationError::Config),
        (
            serde_json::json!({"model":"synthetic-model","api_key":"${}","base_url":f.server.uri()})
                .to_string()
                .into_bytes(),
            PreparationError::Credentials,
        ),
    ] {
        std::fs::write(f.config.join("config.json"), config).unwrap();
        let (pipe, retained) = queued(b"unread");
        let error = StdinFileInvocation::prepare_for_tests(
            f.options(SessionSelection::Fresh),
            sources(),
            f.destination(Format::AsciiFragment, CAP),
            pipe,
            Limits::default(),
            || panic!("invalid bootstrap reached capture"),
        )
        .await
        .err();
        assert_eq!(
            error,
            Some(StdinOutputPreparationError::Stdin(
                StdinPreparationError::Preparation(expected)
            ))
        );
        unread(retained, b"unread");
        f.no_effects().await;
    }
}
#[tokio::test]
async fn empty_bom_whitespace_and_malformed_original_utf8_fail_before_session_and_staging() {
    let f = Fixture::new().await;
    for (bytes, expected) in [
        (&b""[..], InputError::Empty),
        (&b" \r\n"[..], InputError::Empty),
        ("\u{feff}".as_bytes(), InputError::Empty),
        (&b"\xff"[..], InputError::Utf8),
        (&b"good\xf0\x9f"[..], InputError::Utf8),
    ] {
        let (pipe, retained) = queued(bytes);
        let error = StdinFileInvocation::prepare_for_tests(
            f.options(SessionSelection::Fresh),
            sources(),
            f.destination(Format::AsciiFragment, CAP),
            pipe,
            Limits::default(),
            || {},
        )
        .await
        .err();
        assert_eq!(
            error,
            Some(StdinOutputPreparationError::Stdin(
                StdinPreparationError::Input(StdinError::Input(expected))
            ))
        );
        assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
        f.no_effects().await;
    }
}
#[tokio::test]
async fn native_byte_overflow_discards_prefix_without_session_store_or_output() {
    let f = Fixture::new().await;
    let (pipe, retained) = queued(b"overflow");
    let error = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Fresh),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
        pipe,
        Limits::tightening(4, Duration::from_secs(10), Duration::from_secs(30)),
        || {},
    )
    .await
    .err();
    assert_eq!(
        error,
        Some(StdinOutputPreparationError::Stdin(
            StdinPreparationError::Input(StdinError::Input(InputError::InstructionLimit))
        ))
    );
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    f.no_effects().await;
}
#[tokio::test]
async fn composed_first_byte_and_absolute_total_deadlines_precede_session_and_output() {
    let f = Fixture::new().await;
    for (bytes, expected) in [
        (&b""[..], StdinError::FirstByteDeadline),
        (&b"x"[..], StdinError::TotalDeadline),
    ] {
        let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
        writer.write_all(bytes).unwrap();
        let retained = reader.try_clone().unwrap();
        let pipe = Pipe::owned_for_tests(reader).unwrap();
        let error = StdinFileInvocation::prepare_for_tests(
            f.options(SessionSelection::Fresh),
            sources(),
            f.destination(Format::AsciiFragment, CAP),
            pipe,
            Limits::tightening(CAP, Duration::from_millis(80), Duration::from_millis(200)),
            || {},
        )
        .await
        .err();
        assert_eq!(
            error,
            Some(StdinOutputPreparationError::Stdin(
                StdinPreparationError::Input(expected)
            ))
        );
        assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
        f.no_effects().await;
        drop(writer);
    }
}
#[tokio::test]
async fn missing_attachment_after_native_eof_never_creates_session_or_output() {
    let f = Fixture::new().await;
    let (pipe, _) = queued(b"goal");
    let error = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Fresh),
        Sources {
            prompt: Some(b"-"),
            prompt_file: None,
            files: &["missing.txt"],
        },
        f.destination(Format::AsciiFragment, CAP),
        pipe,
        Limits::default(),
        || {},
    )
    .await
    .err();
    assert_eq!(
        error,
        Some(StdinOutputPreparationError::Stdin(
            StdinPreparationError::Input(StdinError::Input(InputError::Source))
        ))
    );
    f.no_effects().await;
}
#[tokio::test]
async fn target_planted_during_native_wait_is_rechecked_before_ids_store_and_provider() {
    let f = Fixture::new().await;
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let (ready, wait) = tokio::sync::oneshot::channel();
    let target = f.output.join("result.out");
    let job = tokio::spawn(async move {
        wait.await.unwrap();
        std::fs::write(target, b"foreign during read").unwrap();
        writer.write_all(b"goal").unwrap();
    });
    let error = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Fresh),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
        pipe,
        Limits::default(),
        || ready.send(()).unwrap(),
    )
    .await
    .err();
    job.await.unwrap();
    assert_eq!(
        error,
        Some(StdinOutputPreparationError::Output(Error::Exists))
    );
    assert_eq!(f.bytes(), b"foreign during read");
    f.no_execution().await;
}
#[tokio::test]
async fn post_read_target_failure_precedes_an_invalid_existing_store_without_schema_bypass() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("threads.db"), b"legacy store").unwrap();
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let (ready, wait) = tokio::sync::oneshot::channel();
    let target = f.output.join("result.out");
    let job = tokio::spawn(async move {
        wait.await.unwrap();
        std::fs::write(target, b"foreign").unwrap();
        writer.write_all(b"goal").unwrap();
    });
    let error = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Fresh),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
        pipe,
        Limits::default(),
        || ready.send(()).unwrap(),
    )
    .await
    .err();
    job.await.unwrap();
    assert_eq!(
        error,
        Some(StdinOutputPreparationError::Output(Error::Exists))
    );
    assert_eq!(
        std::fs::read(f.config.join("threads.db")).unwrap(),
        b"legacy store"
    );
    assert_eq!(f.bytes(), b"foreign");
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn native_parent_rename_during_stdin_keeps_actual_retained_authority_or_refuses_changed() {
    let f = Fixture::new().await;
    let original = f.output.clone();
    let moved = f.output.with_file_name("moved-output");
    let id = directory_identity(&original);
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let (ready, wait) = tokio::sync::oneshot::channel();
    let job = tokio::spawn(async move {
        wait.await.unwrap();
        let moved_ok = move_directory(&original, &moved, id);
        if moved_ok {
            std::fs::create_dir(&original).unwrap();
        }
        writer.write_all(b"goal").unwrap();
        moved_ok
    });
    let result = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
        pipe,
        Limits::default(),
        || ready.send(()).unwrap(),
    )
    .await;
    let moved = job.await.unwrap();
    if moved {
        assert_eq!(
            result.err(),
            Some(StdinOutputPreparationError::Output(Error::Changed))
        );
        assert!(f.entries().is_empty());
        f.no_execution().await;
    } else {
        f.respond("done").await;
        let run = result.unwrap().run().await;
        run.terminal().publish().unwrap();
        assert_eq!(f.value()["final_output"], "done");
        f.no_store();
    }
}
#[tokio::test]
async fn dropping_waiting_composed_preparation_leaves_no_output_or_orphan_reader() {
    let f = Fixture::new().await;
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let options = f.options(SessionSelection::Fresh);
    let destination = f.destination(Format::AsciiFragment, CAP);
    let (ready, wait) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        StdinFileInvocation::prepare_for_tests(
            options,
            sources(),
            destination,
            pipe,
            Limits::default(),
            || ready.send(()).unwrap(),
        )
        .await
    });
    wait.await.unwrap();
    tokio::task::yield_now().await;
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    f.no_effects().await;
    writer.write_all(b"late bytes").unwrap();
    drop(writer);
    unread(retained, b"late bytes");
}
#[tokio::test]
async fn original_read_only_native_pipe_can_publish_without_mode_right_escalation() {
    let f = Fixture::new().await;
    f.respond("done").await;
    let (reader, mut writer) = stdin_pipe::read_only_pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    writer
        .write_all("\u{feff}readonly goal\r\n".as_bytes())
        .unwrap();
    drop(writer);
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let run = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        sources(),
        f.destination(Format::HumanPreview, CAP),
        pipe,
        Limits::default(),
        || {},
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(
        run.run_for_tests().facts().instruction().kind(),
        OriginKind::Stdin
    );
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    run.terminal().publish().unwrap();
    assert!(String::from_utf8(f.bytes()).unwrap().starts_with("done\n"));
    f.no_store();
}
#[tokio::test]
async fn actual_typed_approval_required_run_keeps_original_partial_return_for_file_publication() {
    let f = Fixture::new().await;
    let event = serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"protected","type":"function","function":{"name":"write_file","arguments":"{\"file_path\":\"build.rs\",\"content\":\"data\"}"}}]},"finish_reason":"tool_calls"}]});
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!("data: {event}\n\ndata: [DONE]\n\n"),
            "text/event-stream",
        ))
        .mount(&f.server)
        .await;
    let (pipe, _) = queued(b"goal");
    let mut options = f.options(SessionSelection::Ephemeral);
    options.selectors.allow = vec!["tool:write_file".into()];
    let run = StdinFileInvocation::prepare_for_tests(
        options,
        sources(),
        f.destination(Format::AsciiFragment, CAP),
        pipe,
        Limits::default(),
        || {},
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(matches!(
        run.run_for_tests().outcome(),
        Err(RuntimeError::ApprovalRequired)
    ));
    assert!(!f.workspace.join("build.rs").exists());
    run.terminal().publish().unwrap();
    let value = f.value();
    assert_eq!(value["run_return"], "approval_required");
    assert_eq!(value["approval_required"], true);
    assert!(!value["messages"].as_array().unwrap().is_empty());
    f.no_store();
}
#[tokio::test]
async fn actual_failed_turn_publishes_only_the_unchanged_fixed_human_diagnostic() {
    let f = Fixture::new().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_raw(format!("raw {KEY}\u{001b} ##vso[task.x]"), "text/plain"),
        )
        .mount(&f.server)
        .await;
    let run = f
        .invocation(b"goal", Format::HumanPreview, CAP)
        .await
        .run()
        .await;
    assert!(matches!(
        run.run_for_tests().outcome(),
        Err(RuntimeError::Turn(_))
    ));
    run.terminal().publish().unwrap();
    assert_eq!(
        String::from_utf8(f.bytes()).unwrap(),
        "craftsman: private run return=turn_failed; approval_required=false; final_answer=absent\n"
    );
    f.no_store();
}
#[tokio::test]
async fn late_actual_approval_keeps_return_observation_but_closes_execution() {
    let f = Fixture::new().await;
    let run = f.run("done", Format::AsciiFragment).await;
    let g = gate(&run);
    g.observe_error_kind(ToolErrorKind::NeedsApproval);
    run.terminal().publish().unwrap();
    assert_eq!(f.value()["run_return"], "returned");
    assert_eq!(f.value()["approval_required"], true);
    assert!(g.execution_closed());
}
#[tokio::test]
async fn revocation_before_attempt_creates_no_staging_and_never_reopens_execution() {
    let f = Fixture::new().await;
    let run = f.run("done", Format::AsciiFragment).await;
    let g = gate(&run);
    let mut terminal = run.terminal();
    terminal.close();
    failure(
        terminal.publish(),
        Error::Terminal(TerminalError::Closed),
        false,
    );
    assert!(g.execution_closed());
    assert!(f.entries().is_empty());
}
const EVENTS: &[Event] = &[
    Event::BeforeCreate,
    Event::AfterCreate,
    Event::BeforeOpen,
    Event::AfterOpen,
    Event::BeforeWrite,
    Event::AfterWrite,
    Event::BeforeFlush,
    Event::AfterFlush,
    Event::BeforeSync,
    Event::AfterSync,
    Event::BeforeCheck,
    Event::AfterCheck,
    Event::BeforeLink,
    Event::AfterLink,
    Event::BeforeCleanup,
    Event::AfterCleanup,
    Event::Finished,
];
fn linked(event: Event) -> bool {
    matches!(
        event,
        Event::AfterLink | Event::BeforeCleanup | Event::AfterCleanup | Event::Finished
    )
}
#[tokio::test]
async fn composed_owner_late_approval_at_every_io_stage_preserves_only_actual_link_observation() {
    for stage in EVENTS {
        let f = Fixture::new().await;
        let run = f.run("done", Format::AsciiFragment).await;
        let g = gate(&run);
        let mut terminal = run.terminal();
        let mut events = Vec::new();
        let mut hook = |event, _: &Terminal, _: &cap_std::fs::Dir| {
            events.push(event);
            if event == *stage {
                g.observe_error_kind(ToolErrorKind::NeedsApproval);
            }
        };
        failure(
            terminal.publish_for_tests(&mut Signals {
                hook: Some(&mut hook),
                ..Default::default()
            }),
            Error::Terminal(TerminalError::Superseded),
            linked(*stage),
        );
        assert_eq!(events.last(), Some(stage));
        assert!(g.execution_closed() && g.approval_required());
        assert_eq!(
            f.entries(),
            if linked(*stage) {
                vec!["result.out"]
            } else {
                vec![]
            }
        );
    }
}
#[tokio::test]
async fn composed_owner_revocation_at_every_io_stage_stops_following_data_operations() {
    for stage in EVENTS {
        let f = Fixture::new().await;
        let run = f.run("done", Format::AsciiFragment).await;
        let mut terminal = run.terminal();
        let mut events = Vec::new();
        let mut hook = |event, t: &Terminal, _: &cap_std::fs::Dir| {
            events.push(event);
            if event == *stage {
                t.close();
            }
        };
        failure(
            terminal.publish_for_tests(&mut Signals {
                hook: Some(&mut hook),
                ..Default::default()
            }),
            Error::Terminal(TerminalError::Closed),
            linked(*stage),
        );
        assert_eq!(events.last(), Some(stage));
        assert_eq!(
            f.entries(),
            if linked(*stage) {
                vec!["result.out"]
            } else {
                vec![]
            }
        );
    }
}
#[tokio::test]
async fn codec_failure_consumes_the_single_attempt_without_raw_or_format_fallback() {
    let f = Fixture::new().await;
    let run = f.run("whole answer", Format::HumanPreview).await;
    let mut terminal = run.terminal();
    terminal.publish().unwrap();
    let bytes = f.bytes();
    failure(terminal.publish(), Error::AlreadyAttempted, false);
    assert_eq!(f.bytes(), bytes);
    f.no_store();
    let f = Fixture::new().await;
    f.respond("cannot fit").await;
    let mut terminal = f
        .invocation(b"goal", Format::HumanPreview, 1)
        .await
        .run()
        .await
        .terminal();
    assert!(terminal.publish().is_err());
    assert!(f.entries().is_empty());
    failure(terminal.publish(), Error::AlreadyAttempted, false);
    f.no_store();
}
#[tokio::test]
async fn frozen_config_seeds_and_original_destination_do_not_reload_after_native_preparation() {
    let f = Fixture::new().await;
    f.respond(&format!("done {KEY}")).await;
    let (pipe, _) = queued(b"goal");
    let mut d = f.destination(Format::AsciiFragment, CAP);
    d.file_name = format!("{KEY}.out");
    let invocation = StdinFileInvocation::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        sources(),
        d,
        pipe,
        Limits::default(),
        || {},
    )
    .await
    .unwrap();
    std::fs::write(f.config.join("config.json"), b"invalid reload").unwrap();
    invocation.run().await.terminal().publish().unwrap();
    let actual = f.output.join(format!("{KEY}.out"));
    let bytes = std::fs::read(actual).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["final_output"], "done [REDACTED]");
    assert!(!f.output.join("[REDACTED].out").exists());
    f.no_store();
}
#[tokio::test]
async fn final_name_planted_after_staging_is_preserved_without_repair_or_retry() {
    let f = Fixture::new().await;
    let mut terminal = f.run("done", Format::AsciiFragment).await.terminal();
    let target = f.output.join("result.out");
    let mut hook = |event, _: &Terminal, _: &cap_std::fs::Dir| {
        if event == Event::BeforeLink {
            std::fs::write(&target, b"foreign").unwrap();
        }
    };
    failure(
        terminal.publish_for_tests(&mut Signals {
            hook: Some(&mut hook),
            ..Default::default()
        }),
        Error::Exists,
        false,
    );
    assert_eq!(f.bytes(), b"foreign");
    assert_eq!(f.entries(), vec!["result.out"]);
}
#[tokio::test]
async fn owned_cleanup_failure_rechecks_actual_stop_and_never_erases_foreign_entries() {
    for stop in [0u8, 1, 2] {
        let f = Fixture::new().await;
        let run = f.run("done", Format::AsciiFragment).await;
        let g = gate(&run);
        let mut terminal = run.terminal();
        let mut cleanup = false;
        let mut hook = |event, t: &Terminal, dir: &cap_std::fs::Dir| {
            if event == Event::BeforeCleanup {
                dir.rename("record", dir, "original").unwrap();
                dir.write("record", b"foreign cleanup").unwrap();
            } else if event == Event::AfterCleanup {
                cleanup = true;
                if stop == 1 {
                    g.observe_error_kind(ToolErrorKind::NeedsApproval);
                } else if stop == 2 {
                    t.close();
                }
            }
        };
        failure(
            terminal.publish_for_tests(&mut Signals {
                hook: Some(&mut hook),
                ..Default::default()
            }),
            match stop {
                1 => Error::Terminal(TerminalError::Superseded),
                2 => Error::Terminal(TerminalError::Closed),
                _ => Error::Changed,
            },
            true,
        );
        assert!(cleanup && g.execution_closed());
        let entries = f.entries();
        assert_eq!(entries.len(), 2);
        let stage = entries
            .iter()
            .find(|p| p.starts_with(".craftsman-exec-tmp-"))
            .unwrap();
        assert_eq!(
            std::fs::read(f.output.join(stage).join("record")).unwrap(),
            b"foreign cleanup"
        );
        assert_eq!(
            std::fs::read(f.output.join(stage).join("original")).unwrap(),
            f.bytes()
        );
    }
}
#[tokio::test]
async fn legacy_file_invocation_still_refuses_stdin_without_adapting_its_source_body() {
    let f = Fixture::new().await;
    let error = FileInvocation::prepare(
        f.options(SessionSelection::Fresh),
        sources(),
        f.destination(Format::AsciiFragment, CAP),
    )
    .await
    .err();
    assert_eq!(
        error,
        Some(crate::exec::construction::OutputPreparationError::Input(
            crate::exec::construction::InputPreparationError::Input(InputError::StdinUnavailable)
        ))
    );
    f.no_effects().await;
}
#[test]
fn composed_preparation_errors_have_fixed_text_without_path_bytes_or_native_messages() {
    for error in [
        StdinOutputPreparationError::Output(Error::Path),
        StdinOutputPreparationError::Output(Error::Changed),
        StdinOutputPreparationError::Stdin(StdinPreparationError::Input(StdinError::TotalDeadline)),
        StdinOutputPreparationError::Stdin(StdinPreparationError::Preparation(
            PreparationError::Credentials,
        )),
        StdinOutputPreparationError::Stdin(StdinPreparationError::Runtime),
    ] {
        assert_eq!(error.to_string(), "[REDACTED]");
    }
}

use std::process::Stdio;
const HELPER: &str = "exec::output::stdin_tests::native_stdin_file_helper";
const MARKER: &str = "--craftsman-private-stdin-file-fixture";
const NATIVE_BYTES: &[u8] = b"original native stdin file goal\r\n";
#[tokio::test]
#[ignore = "native subprocess only; exact argv marker required before any stdin inspection"]
async fn native_stdin_file_helper() {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(index) = args.iter().position(|a| a == MARKER) else {
        return;
    };
    let Some(scenario) = args.get(index + 1) else {
        return;
    };
    if !matches!(
        scenario.as_str(),
        "capture" | "bad-budget" | "scope" | "bad-config" | "existing" | "idle"
    ) {
        return;
    }
    let f = Fixture::new().await;
    if scenario == "idle" {
        let started = tokio::time::Instant::now();
        assert_eq!(
            StdinFileInvocation::prepare(
                f.options(SessionSelection::Fresh),
                sources(),
                f.destination(Format::AsciiFragment, CAP)
            )
            .await
            .err(),
            Some(StdinOutputPreparationError::Stdin(
                StdinPreparationError::Input(StdinError::FirstByteDeadline)
            ))
        );
        assert!(started.elapsed() >= Duration::from_secs(10));
        assert!(started.elapsed() < Duration::from_secs(60));
        f.no_effects().await;
        println!("STDIN_FILE_IDLE_DEADLINE");
        return;
    }
    if scenario == "bad-budget" {
        assert_eq!(
            StdinFileInvocation::prepare(
                f.options(SessionSelection::Fresh),
                sources(),
                f.destination(Format::AsciiFragment, 0)
            )
            .await
            .err(),
            Some(StdinOutputPreparationError::Output(Error::Limit))
        );
        f.no_effects().await;
    } else if scenario == "scope" {
        let mut d = f.destination(Format::AsciiFragment, CAP);
        d.directory = f.workspace.clone();
        assert_eq!(
            StdinFileInvocation::prepare(f.options(SessionSelection::Fresh), sources(), d)
                .await
                .err(),
            Some(StdinOutputPreparationError::Output(Error::Scope))
        );
        f.no_effects().await;
    } else if scenario == "bad-config" {
        std::fs::write(f.config.join("config.json"), b"invalid json").unwrap();
        assert_eq!(
            StdinFileInvocation::prepare(
                f.options(SessionSelection::Fresh),
                sources(),
                f.destination(Format::AsciiFragment, CAP)
            )
            .await
            .err(),
            Some(StdinOutputPreparationError::Stdin(
                StdinPreparationError::Preparation(PreparationError::Config)
            ))
        );
        f.no_effects().await;
        std::fs::write(
            f.config.join("config.json"),
            serde_json::json!({"model":"synthetic-model","api_key":KEY,"base_url":f.server.uri()})
                .to_string(),
        )
        .unwrap();
    } else if scenario == "existing" {
        std::fs::write(f.output.join("result.out"), b"foreign final").unwrap();
        assert_eq!(
            StdinFileInvocation::prepare(
                f.options(SessionSelection::Fresh),
                sources(),
                f.destination(Format::AsciiFragment, CAP)
            )
            .await
            .err(),
            Some(StdinOutputPreparationError::Output(Error::Exists))
        );
        assert_eq!(f.bytes(), b"foreign final");
        f.no_execution().await;
    }
    f.respond("done").await;
    let mut destination = f.destination(Format::AsciiFragment, CAP);
    destination.file_name = "native.out".into();
    let invocation = StdinFileInvocation::prepare(
        f.options(SessionSelection::Ephemeral),
        sources(),
        destination,
    )
    .await
    .unwrap();
    assert!(!f.output.join("native.out").exists());
    assert!(f.server.received_requests().await.unwrap().is_empty());
    let run = invocation.run().await;
    assert!(run.run_for_tests().outcome().is_ok());
    assert_eq!(
        run.run_for_tests().facts().instruction().kind(),
        OriginKind::Stdin
    );
    assert_eq!(
        run.run_for_tests().facts().instruction().bytes(),
        NATIVE_BYTES.len()
    );
    let requests = f.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(body["messages"].as_array().unwrap().iter().any(|m| {
        m["content"]
            .as_str()
            .is_some_and(|s| s == "original native stdin file goal\r\n")
    }));
    run.terminal().publish().unwrap();
    let bytes = std::fs::read(f.output.join("native.out")).unwrap();
    assert!(bytes.is_ascii());
    assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["final_output"], "done");
    assert!(value.get("status").is_none() && value.get("usage").is_none());
    if scenario == "existing" {
        assert_eq!(f.bytes(), b"foreign final");
    }
    f.no_store();
    println!("STDIN_FILE_CAPTURE_COMPLETE");
}
async fn subprocess(scenario: &str, marker: bool, write: bool) -> std::process::Output {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        HELPER,
        "--nocapture",
        "--test-threads=1",
    ]);
    if marker {
        command.args(["--", MARKER, scenario]);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let mut input = Some(child.stdin.take().unwrap());
    if write {
        use tokio::io::AsyncWriteExt;
        input
            .as_mut()
            .unwrap()
            .write_all(NATIVE_BYTES)
            .await
            .unwrap();
        drop(input.take());
    }
    // Hold writer open for idle/unmarked cases; startup is independent of strict
    // unchanged production10s first-byte/30s total native capture clocks.
    let output = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output())
        .await
        .expect("native helper startup/capture bound; no widened native read clock")
        .unwrap();
    drop(input);
    assert!(
        output.status.success(),
        "native helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
#[tokio::test]
async fn actual_process_stdin_reaches_production_prepared_runtime_and_guarded_file_chain() {
    let output = subprocess("capture", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("STDIN_FILE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn invalid_destination_grammar_does_not_consume_actual_os_stdin() {
    let output = subprocess("bad-budget", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("STDIN_FILE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn forbidden_output_scope_does_not_consume_actual_os_stdin() {
    let output = subprocess("scope", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("STDIN_FILE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn invalid_bootstrap_does_not_consume_actual_os_stdin_before_file_route() {
    let output = subprocess("bad-config", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("STDIN_FILE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn existing_output_does_not_consume_actual_os_stdin_and_foreign_file_survives() {
    let output = subprocess("existing", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("STDIN_FILE_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn actual_idle_process_stdin_preserves_production_first_byte_deadline_before_store() {
    let output = subprocess("idle", true, false).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("STDIN_FILE_IDLE_DEADLINE"));
}
#[tokio::test]
async fn unmarked_ignored_file_helper_never_inspects_or_reads_ambient_stdin() {
    let output = subprocess("capture", false, false).await;
    assert!(!String::from_utf8_lossy(&output.stdout).contains("STDIN_FILE_CAPTURE_COMPLETE"));
}
