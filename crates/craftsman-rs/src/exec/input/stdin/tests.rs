use super::*;
use crate::exec::construction::{PreparedStdinInput, SessionSelection};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::runtime::RuntimeError;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "native-input-synthetic-secret";
struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    server: MockServer,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let workspace = base.join("workspace");
        let config = base.join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let f = Self {
            _root: root,
            workspace,
            config,
            server: MockServer::start().await,
        };
        f.configure(KEY);
        f
    }
    fn configure(&self, key: &str) {
        std::fs::write(
            self.config.join("config.json"),
            json!({"model":"synthetic-model","api_key":key,"base_url":self.server.uri()})
                .to_string(),
        )
        .unwrap();
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
    fn no_store(&self) {
        for name in ["threads.db", "sessions-rs", "gadgets/.sdk"] {
            assert!(!self.config.join(name).exists());
        }
    }
    async fn no_effects(&self) {
        self.no_store();
        assert!(self.server.received_requests().await.unwrap().is_empty());
    }
    async fn respond(&self, events: &[Value]) {
        let mut body = String::new();
        for event in events {
            body.push_str(&format!("data: {event}\n\n"));
        }
        body.push_str("data: [DONE]\n\n");
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .mount(&self.server)
            .await;
    }
    async fn answer(&self) {
        self.respond(&[json!({"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]})])
            .await;
    }
}
fn selected(files: &[&str]) -> Selected {
    Selected::select(Sources {
        prompt: Some(b"-"),
        prompt_file: None,
        files,
    })
    .unwrap()
}
fn named<'a>(files: &'a [&'a str]) -> Sources<'a> {
    Sources {
        prompt: Some(b"-"),
        prompt_file: None,
        files,
    }
}
fn queued(bytes: &[u8]) -> (Pipe, File) {
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    writer.write_all(bytes).unwrap();
    drop(writer);
    (Pipe::owned_for_tests(reader).unwrap(), retained)
}
fn limits(first: u64, total: u64) -> Limits {
    Limits::tightening(
        super::super::INSTRUCTION_BYTES,
        Duration::from_millis(first),
        Duration::from_millis(total),
    )
}
async fn admit(
    f: &Fixture,
    bytes: &[u8],
    files: &[&str],
) -> Result<(Prepared, Admitted), StdinPreparationError> {
    let (pipe, _) = queued(bytes);
    Ok(PreparedStdinInput::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        named(files),
        pipe,
        Limits::default(),
    )
    .await?
    .into_parts())
}
#[test]
fn only_the_exact_positional_marker_mints_native_selection() {
    assert!(Selected::select(named(&[])).is_ok());
    for prompt in [&b"goal"[..], &b" - "[..], &b""[..], &b"\xff"[..]] {
        assert_eq!(
            Selected::select(Sources {
                prompt: Some(prompt),
                prompt_file: None,
                files: &[]
            })
            .err(),
            Some(Error::NotSelected)
        );
    }
    for file in ["-", "goal.txt"] {
        assert_eq!(
            Selected::select(Sources {
                prompt: None,
                prompt_file: Some(file),
                files: &[]
            })
            .err(),
            Some(Error::NotSelected)
        );
    }
}
#[test]
fn source_conflicts_and_missing_instruction_precede_native_selection() {
    assert_eq!(
        Selected::select(Sources {
            prompt: None,
            prompt_file: None,
            files: &["data"]
        })
        .err(),
        Some(Error::Input(InputError::MissingSource))
    );
    assert_eq!(
        Selected::select(Sources {
            prompt: Some(b"-"),
            prompt_file: Some("goal"),
            files: &[]
        })
        .err(),
        Some(Error::Input(InputError::ConflictingSources))
    );
}
#[test]
fn complete_attachment_selection_is_bounded_before_capture() {
    for file in ["", "a\0b"] {
        assert_eq!(
            Selected::select(named(&[file])).err(),
            Some(Error::Input(InputError::Selection))
        );
    }
    let long = "x".repeat(super::super::PATH_SCALARS + 1);
    assert_eq!(
        Selected::select(named(&[&long])).err(),
        Some(Error::Input(InputError::SelectionLimit))
    );
    assert_eq!(
        Selected::select(named(&["a"; 129])).err(),
        Some(Error::Input(InputError::SelectionLimit))
    );
    assert!(Selected::select(named(&["a"; 128])).is_ok());
}
#[test]
fn defaults_are_exact_and_test_bounds_can_only_tighten() {
    let l = Limits::default();
    assert_eq!(l.first, Duration::from_secs(10));
    assert_eq!(l.total, Duration::from_secs(30));
    assert_eq!(l.maximum, 1024 * 1024);
    assert!(l.valid());
    for l in [
        Limits::tightening(0, FIRST, TOTAL),
        Limits::tightening(1024 * 1024 + 1, FIRST, TOTAL),
        Limits::tightening(1, Duration::ZERO, TOTAL),
        Limits::tightening(1, FIRST + Duration::from_secs(1), TOTAL),
        Limits::tightening(1, FIRST, TOTAL + Duration::from_secs(1)),
        limits(100, 50),
    ] {
        assert!(!l.valid());
    }
}
#[test]
fn legacy_argument_file_and_guarded_file_routes_still_refuse_stdin() {
    assert_eq!(
        super::super::Selected::select(named(&[])).err(),
        Some(InputError::StdinUnavailable)
    );
    assert!(
        super::super::Selected::select(Sources {
            prompt: None,
            prompt_file: Some("-"),
            files: &[]
        })
        .is_ok()
    );
}
#[tokio::test]
async fn native_pipe_capture_preserves_whole_utf8_bom_and_crlf_bytes() {
    let bytes = "\u{feff}中文 🦀\r\n!literal /model\n".as_bytes();
    let (pipe, retained) = queued(bytes);
    assert!(stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    assert_eq!(
        capture(pipe, Instant::now(), Limits::default())
            .await
            .unwrap(),
        bytes
    );
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
}
#[tokio::test]
async fn stdin_origin_counts_original_bytes_before_one_bom_is_removed() {
    let f = Fixture::new().await;
    let bytes = "\u{feff}instruction\r\n".as_bytes();
    let (_, input) = admit(&f, bytes, &[]).await.unwrap();
    assert_eq!(input.body, "instruction\r\n");
    assert_eq!(input.facts.instruction().kind(), OriginKind::Stdin);
    assert_eq!(input.facts.instruction().bytes(), bytes.len());
    assert!(input.facts.instruction().bom_removed());
    assert!(input.facts.instruction().path().is_none());
    f.no_effects().await;
}
#[tokio::test]
async fn only_one_stdin_encoding_preamble_is_removed() {
    let f = Fixture::new().await;
    let bytes = "\u{feff}\u{feff}text".as_bytes();
    let (_, input) = admit(&f, bytes, &[]).await.unwrap();
    assert_eq!(input.body, "\u{feff}text");
    assert_eq!(input.facts.instruction().bytes(), bytes.len());
    f.no_effects().await;
}
#[tokio::test]
async fn empty_whitespace_and_bom_only_eof_fail_before_store_or_traffic() {
    let f = Fixture::new().await;
    for bytes in [
        &b""[..],
        &b" \t\r\n"[..],
        "\u{2003}\u{3000}".as_bytes(),
        "\u{feff}".as_bytes(),
    ] {
        assert_eq!(
            admit(&f, bytes, &[]).await.err(),
            Some(StdinPreparationError::Input(Error::Input(
                InputError::Empty
            )))
        );
        f.no_effects().await;
    }
}
#[tokio::test]
async fn malformed_whole_stdin_utf8_never_falls_back_to_a_lossy_prefix() {
    let f = Fixture::new().await;
    for bytes in [&b"\xff"[..], &b"good\xf0\x9f"[..], &b"\xc0\xaf"[..]] {
        assert_eq!(
            admit(&f, bytes, &[]).await.err(),
            Some(StdinPreparationError::Input(Error::Input(InputError::Utf8)))
        );
        f.no_effects().await;
    }
}
#[tokio::test]
async fn native_utf8_may_be_split_across_actual_reads_but_only_eof_finishes() {
    let f = Fixture::new().await;
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let task = tokio::spawn(async move {
        writer.write_all(&[0xf0, 0x9f]).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        writer.write_all(&[0xa6, 0x80]).unwrap();
    });
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let input = selected(&[])
        .load_for_tests(&f.options_fs(), pipe, limits(200, 500))
        .await
        .unwrap();
    task.await.unwrap();
    assert_eq!(input.body, "🦀");
    assert_eq!(input.facts.instruction().bytes(), 4);
    f.no_effects().await;
}
#[tokio::test]
async fn exact_one_mib_original_stdin_is_admitted_without_raising_source_caps() {
    let f = Fixture::new().await;
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let bytes = "é".repeat(1024 * 1024 / 2).into_bytes();
    let expected = bytes.clone();
    let task = tokio::task::spawn_blocking(move || writer.write_all(&bytes));
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let (_, input) = PreparedStdinInput::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        named(&[]),
        pipe,
        Limits::default(),
    )
    .await
    .unwrap()
    .into_parts();
    task.await.unwrap().unwrap();
    assert_eq!(input.body.as_bytes(), expected);
    assert_eq!(input.facts.instruction().bytes(), 1024 * 1024);
    f.no_effects().await;
}
#[tokio::test]
async fn original_stream_overflow_discards_the_entire_prefix_before_session() {
    let f = Fixture::new().await;
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    let task = tokio::task::spawn_blocking(move || writer.write_all(&vec![b'x'; 1024 * 1024 + 1]));
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    assert_eq!(
        PreparedStdinInput::prepare_for_tests(
            f.options(SessionSelection::Fresh),
            named(&[]),
            pipe,
            Limits::default()
        )
        .await
        .err(),
        Some(StdinPreparationError::Input(Error::Input(
            InputError::InstructionLimit
        )))
    );
    task.await.unwrap().unwrap();
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    f.no_effects().await;
}
#[tokio::test]
async fn a_tightened_native_byte_cap_refuses_instead_of_clipping() {
    for (n, ok) in [(16, true), (17, false)] {
        let (pipe, retained) = queued(&vec![b'x'; n]);
        let result = capture(pipe, Instant::now(), Limits::tightening(16, FIRST, TOTAL)).await;
        if ok {
            assert_eq!(result.unwrap().len(), 16);
        } else {
            assert_eq!(
                result.err(),
                Some(Error::Input(InputError::InstructionLimit))
            );
        }
        assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    }
}
#[tokio::test]
async fn idle_open_pipe_hits_real_first_byte_deadline_without_waiting_for_eof() {
    let (reader, _writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let started = Instant::now();
    assert_eq!(
        capture(pipe, started, limits(50, 200)).await.err(),
        Some(Error::FirstByteDeadline)
    );
    assert!(started.elapsed() >= Duration::from_millis(50));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
}
#[tokio::test]
async fn one_byte_and_idle_open_pipe_still_hits_absolute_total_deadline() {
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    writer.write_all(b"x").unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let started = Instant::now();
    assert_eq!(
        capture(pipe, started, limits(30, 100)).await.err(),
        Some(Error::TotalDeadline)
    );
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(writer);
}
#[tokio::test]
async fn repeated_trickle_bytes_never_reset_the_total_read_clock() {
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    writer.write_all(b"x").unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let task = tokio::spawn(async move {
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(15)).await;
            if writer.write_all(b"x").is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    assert_eq!(
        capture(pipe, started, limits(40, 120)).await.err(),
        Some(Error::TotalDeadline)
    );
    assert!(started.elapsed() >= Duration::from_millis(120));
    assert!(started.elapsed() < Duration::from_secs(2));
    task.await.unwrap();
}
#[tokio::test]
async fn expired_before_capture_refuses_even_already_available_data() {
    let (pipe, retained) = queued(b"late");
    assert_eq!(
        capture(
            pipe,
            Instant::now() - Duration::from_secs(1),
            limits(20, 50)
        )
        .await
        .err(),
        Some(Error::FirstByteDeadline)
    );
    let mut pipe = Pipe::owned_for_tests(retained).unwrap();
    let mut bytes = [0; 8];
    assert_eq!(pipe.read(&mut bytes).unwrap(), Read::Bytes(4));
    assert_eq!(&bytes[..4], b"late");
    pipe.finish().unwrap();
}
#[tokio::test]
async fn timeout_restores_mode_and_has_no_background_reader_for_late_bytes() {
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    assert_eq!(
        capture(pipe, Instant::now(), limits(40, 100)).await.err(),
        Some(Error::FirstByteDeadline)
    );
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    writer.write_all(b"after timeout").unwrap();
    let mut probe = Pipe::owned_for_tests(retained).unwrap();
    let mut bytes = [0; 32];
    assert_eq!(probe.read(&mut bytes).unwrap(), Read::Bytes(13));
    assert_eq!(&bytes[..13], b"after timeout");
    probe.finish().unwrap();
}
#[tokio::test]
async fn dropping_the_waiting_future_closes_ownership_and_restores_mode() {
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let task = tokio::spawn(capture(pipe, Instant::now(), Limits::default()));
    tokio::time::sleep(Duration::from_millis(30)).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    writer.write_all(b"after drop").unwrap();
    let mut probe = Pipe::owned_for_tests(retained).unwrap();
    let mut bytes = [0; 32];
    assert_eq!(probe.read(&mut bytes).unwrap(), Read::Bytes(10));
    assert_eq!(&bytes[..10], b"after drop");
    probe.finish().unwrap();
}
#[tokio::test]
async fn already_nonblocking_pipe_mode_is_preserved_not_forced_to_blocking() {
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    stdin_pipe::set_nonblocking_for_tests(&reader, true).unwrap();
    let retained = reader.try_clone().unwrap();
    writer.write_all(b"x").unwrap();
    drop(writer);
    assert_eq!(
        capture(
            Pipe::owned_for_tests(reader).unwrap(),
            Instant::now(),
            Limits::default()
        )
        .await
        .unwrap(),
        b"x"
    );
    assert!(stdin_pipe::nonblocking_for_tests(&retained).unwrap());
}
#[test]
fn foreign_mode_restoration_is_refused_without_clobbering_it() {
    let (reader, _writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    let mut pipe = Pipe::owned_for_tests(reader).unwrap();
    stdin_pipe::set_nonblocking_for_tests(&retained, false).unwrap();
    assert_eq!(
        pipe.read(&mut [0; 1]).err(),
        Some(stdin_pipe::Error::Changed)
    );
    pipe.finish().unwrap();
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
}
#[test]
fn ordinary_files_and_invalid_native_buffers_are_not_read_as_pipe_stdin() {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"must not read").unwrap();
    assert_eq!(
        Pipe::owned_for_tests(file).err().map(|e| e),
        Some(stdin_pipe::Error::Unsupported)
    );
    let (reader, _writer) = stdin_pipe::pair_for_tests().unwrap();
    let mut pipe = Pipe::owned_for_tests(reader).unwrap();
    assert_eq!(
        pipe.read(&mut []).err(),
        Some(stdin_pipe::Error::Unsupported)
    );
    assert_eq!(
        pipe.read(&mut vec![0; stdin_pipe::STEP + 1]).err(),
        Some(stdin_pipe::Error::Unsupported)
    );
    pipe.finish().unwrap();
}
#[tokio::test]
async fn native_read_and_eof_attempts_share_one_finite_call_budget() {
    for (n, ok) in [(CALLS - 1, true), (CALLS, false)] {
        let (pipe, retained) = queued(&vec![b'x'; n]);
        let pipe = pipe.tighten_read_size_for_tests(1);
        let result = capture(pipe, Instant::now(), Limits::default()).await;
        if ok {
            assert_eq!(result.unwrap().len(), n);
        } else {
            assert_eq!(result.err(), Some(Error::ReadCalls));
        }
        assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    }
}
#[tokio::test]
async fn stdin_attachment_data_and_labels_reuse_original_complete_framing() {
    let f = Fixture::new().await;
    std::fs::write(f.workspace.join("data.txt"), "\u{feff}data\r\n!literal").unwrap();
    let (_, input) = admit(&f, b"goal", &["data.txt", "data.txt"]).await.unwrap();
    assert!(
        input
            .body
            .starts_with("goal\n\n<craftsman_input_attachments>")
    );
    assert!(input.body.contains("\u{feff}data\r\n!literal"));
    assert_eq!(input.facts.attachments().len(), 2);
    assert_eq!(
        input.facts.attachment_bytes(),
        2 * "\u{feff}data\r\n!literal".len()
    );
    assert_eq!(input.facts.instruction().kind(), OriginKind::Stdin);
    f.no_effects().await;
}
#[tokio::test]
async fn missing_or_denied_attachment_after_native_eof_never_opens_session() {
    let f = Fixture::new().await;
    for file in ["missing.txt", "../selected-config/config.json"] {
        assert_eq!(
            admit(&f, b"goal", &[file]).await.err(),
            Some(StdinPreparationError::Input(Error::Input(
                InputError::Source
            )))
        );
        f.no_effects().await;
    }
}
#[tokio::test]
async fn unadmitted_stdin_cannot_inspect_an_invalid_existing_store() {
    let f = Fixture::new().await;
    std::fs::write(f.config.join("threads.db"), b"legacy sentinel").unwrap();
    let (pipe, _) = queued(b"\xff");
    assert_eq!(
        PreparedStdinInput::prepare_for_tests(
            f.options(SessionSelection::Fresh),
            named(&[]),
            pipe,
            Limits::default()
        )
        .await
        .err(),
        Some(StdinPreparationError::Input(Error::Input(InputError::Utf8)))
    );
    assert_eq!(
        std::fs::read(f.config.join("threads.db")).unwrap(),
        b"legacy sentinel"
    );
    assert!(f.server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn actual_native_invocation_is_model_only_and_does_not_reopen_capabilities() {
    let f = Fixture::new().await;
    f.answer().await;
    let (pipe, _) = queued(b"!touch forbidden\r\n/model literal");
    let invocation = Invocation::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        named(&[]),
        pipe,
        Limits::default(),
    )
    .await
    .unwrap();
    f.no_effects().await;
    let run = invocation.run().await;
    assert!(run.outcome().is_ok());
    assert_eq!(run.facts().instruction().kind(), OriginKind::Stdin);
    assert!(!f.workspace.join("forbidden").exists());
    let requests = f.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        String::from_utf8(requests[0].body.clone())
            .unwrap()
            .contains("!touch forbidden")
    );
    f.no_store();
}
#[tokio::test]
async fn native_input_success_can_use_existing_actual_terminal_views_only() {
    let f = Fixture::new().await;
    f.answer().await;
    let (pipe, _) = queued(b"goal");
    let run = Invocation::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        named(&[]),
        pipe,
        Limits::default(),
    )
    .await
    .unwrap()
    .run()
    .await;
    assert_eq!(run.facts().instruction().bytes(), 4);
    let terminal = run.terminal();
    let view = terminal.human(1024).unwrap();
    assert_eq!(view.answer().unwrap(), Some(&b"done\n"[..]));
    f.no_store();
}
#[tokio::test]
async fn actual_typed_approval_failure_keeps_partial_run_without_native_spill() {
    let f = Fixture::new().await;
    f.respond(&[json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"write","type":"function","function":{"name":"write_file","arguments":"{\"file_path\":\"build.rs\",\"content\":\"data\"}"}}]},"finish_reason":"tool_calls"}]})]).await;
    let (pipe, _) = queued(b"goal");
    let mut options = f.options(SessionSelection::Ephemeral);
    options.selectors.allow = vec!["tool:write_file".into()];
    let run = Invocation::prepare_for_tests(options, named(&[]), pipe, Limits::default())
        .await
        .unwrap()
        .run()
        .await;
    assert!(matches!(run.outcome(), Err(RuntimeError::ApprovalRequired)));
    assert_eq!(run.facts().instruction().kind(), OriginKind::Stdin);
    assert!(!f.workspace.join("build.rs").exists());
    f.no_store();
}
#[tokio::test]
async fn all_new_native_errors_have_fixed_diagnostics_without_raw_source() {
    for error in [
        Error::NotSelected,
        Error::FirstByteDeadline,
        Error::TotalDeadline,
        Error::ReadCalls,
        Error::Resource,
        Error::Input(InputError::Utf8),
        Error::Native(stdin_pipe::Error::Io),
    ] {
        assert_eq!(error.to_string(), "[REDACTED]");
        assert_eq!(
            StdinPreparationError::Input(error).to_string(),
            "[REDACTED]"
        );
    }
}

impl Fixture {
    fn options_fs(&self) -> super::super::ConfinedFs {
        super::super::ConfinedFs::new(
            &self.workspace,
            &self.config,
            std::sync::Arc::new(
                crate::exec::permissions::InvocationPermissions::from_explicit(
                    Selectors::default(),
                    &[],
                )
                .unwrap(),
            ),
        )
        .unwrap()
    }
}

const HELPER: &str = "exec::input::stdin::tests::native_positional_stdin_helper";
const MARKER: &str = "--craftsman-private-native-stdin-fixture";
const NATIVE_BYTES: &[u8] = b"synthetic native positional input\r\n";
#[tokio::test]
#[ignore = "native subprocess only; exact argv marker required before stdin inspection"]
async fn native_positional_stdin_helper() {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(index) = args.iter().position(|arg| arg == MARKER) else {
        return;
    };
    let Some(scenario) = args.get(index + 1) else {
        return;
    };
    if !matches!(
        scenario.as_str(),
        "capture" | "not-selected" | "bad-config" | "idle" | "busy"
    ) {
        return;
    }
    let f = Fixture::new().await;
    if scenario == "not-selected" {
        assert_eq!(
            Invocation::prepare(
                f.options(SessionSelection::Fresh),
                Sources {
                    prompt: Some(b"literal"),
                    prompt_file: None,
                    files: &[]
                }
            )
            .await
            .err()
            .map(|e| e),
            Some(StdinPreparationError::Input(Error::NotSelected))
        );
        f.no_effects().await;
    } else if scenario == "bad-config" {
        std::fs::write(f.config.join("config.json"), b"invalid config").unwrap();
        assert_eq!(
            Invocation::prepare(f.options(SessionSelection::Fresh), named(&[]))
                .await
                .err()
                .map(|e| e),
            Some(StdinPreparationError::Preparation(
                crate::exec::construction::PreparationError::Config
            ))
        );
        f.no_store();
        f.configure(KEY);
    } else if scenario == "busy" {
        let selection = selected(&[]);
        let first = Pipe::stdin(&selection).unwrap();
        assert_eq!(Pipe::stdin(&selection).err(), Some(stdin_pipe::Error::Busy));
        // The rejected second owner never consumed bytes; finish the first
        // lease without reading, then the actual invocation gets the full source.
        first.finish().unwrap();
    } else if scenario == "idle" {
        let selection = selected(&[]);
        let pipe = Pipe::stdin(&selection).unwrap();
        assert_eq!(
            capture(pipe, Instant::now(), limits(80, 200)).await.err(),
            Some(Error::FirstByteDeadline)
        );
        println!("NATIVE_STDIN_IDLE_DEADLINE");
        return;
    }
    f.answer().await;
    let invocation = Invocation::prepare(f.options(SessionSelection::Ephemeral), named(&[]))
        .await
        .unwrap();
    f.no_effects().await;
    let run = invocation.run().await;
    assert!(run.outcome().is_ok());
    assert_eq!(run.facts().instruction().kind(), OriginKind::Stdin);
    assert_eq!(run.facts().instruction().bytes(), NATIVE_BYTES.len());
    let requests = f.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        String::from_utf8(requests[0].body.clone())
            .unwrap()
            .contains("synthetic native positional input")
    );
    f.no_store();
    // Fixed fixture status only; never print consumed source/paths/error bodies.
    println!("NATIVE_STDIN_CAPTURE_COMPLETE");
}
async fn subprocess(scenario: &str, with_marker: bool, write: bool) -> std::process::Output {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        HELPER,
        "--nocapture",
        "--test-threads=1",
    ]);
    if with_marker {
        // libtest consumes its own options; the marker belongs after the argv
        // separator and remains visible to only this exact ignored helper.
        command.args(["--", MARKER, scenario]);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let input = child.stdin.take().unwrap();
    // Hold the writer open for idle/unmarked tests; no EOF can fake readiness.
    let mut input = Some(input);
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
    let output = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output())
        .await
        .expect("native helper startup/capture finishes; no widened production read clock")
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
async fn actual_os_stdin_uses_the_production_prepared_runtime_chain() {
    let output = subprocess("capture", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("NATIVE_STDIN_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn unselected_argument_refuses_without_consuming_actual_stdin() {
    let output = subprocess("not-selected", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("NATIVE_STDIN_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn invalid_bootstrap_refuses_without_consuming_actual_stdin() {
    let output = subprocess("bad-config", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("NATIVE_STDIN_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn actual_idle_stdin_pipe_hits_deadline_instead_of_requiring_writer_eof() {
    let output = subprocess("idle", true, false).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("NATIVE_STDIN_IDLE_DEADLINE"));
}
#[tokio::test]
async fn unmarked_ignored_native_helper_returns_without_inspecting_stdin() {
    let started = Instant::now();
    let output = subprocess("capture", false, false).await;
    assert!(!String::from_utf8_lossy(&output.stdout).contains("NATIVE_STDIN_CAPTURE_COMPLETE"));
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[tokio::test]
async fn a_second_actual_stdin_owner_is_refused_without_consuming_source() {
    let output = subprocess("busy", true, true).await;
    assert!(String::from_utf8_lossy(&output.stdout).contains("NATIVE_STDIN_CAPTURE_COMPLETE"));
}
#[tokio::test]
async fn both_native_deadlines_precede_fresh_session_and_provider_effects() {
    let f = Fixture::new().await;
    for (bytes, expected) in [
        (&b""[..], Error::FirstByteDeadline),
        (&b"x"[..], Error::TotalDeadline),
    ] {
        let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
        writer.write_all(bytes).unwrap();
        let retained = reader.try_clone().unwrap();
        let pipe = Pipe::owned_for_tests(reader).unwrap();
        let error = PreparedStdinInput::prepare_for_tests(
            f.options(SessionSelection::Fresh),
            named(&[]),
            pipe,
            limits(40, 100),
        )
        .await
        .err();
        assert_eq!(error, Some(StdinPreparationError::Input(expected)));
        assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
        f.no_effects().await;
        drop(writer);
    }
}
#[tokio::test]
async fn invalid_private_limits_refuse_without_a_native_data_read() {
    let (pipe, retained) = queued(b"untouched");
    assert_eq!(
        capture(pipe, Instant::now(), Limits::tightening(0, FIRST, TOTAL))
            .await
            .err(),
        Some(Error::Resource)
    );
    let mut probe = Pipe::owned_for_tests(retained).unwrap();
    let mut buffer = [0; 16];
    assert_eq!(probe.read(&mut buffer).unwrap(), Read::Bytes(9));
    assert_eq!(&buffer[..9], b"untouched");
    probe.finish().unwrap();
}

#[test]
fn empty_open_native_pipe_is_pending_and_only_writer_closure_is_eof() {
    let (reader, mut writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    let mut pipe = Pipe::owned_for_tests(reader).unwrap();
    let mut buffer = [0; 16];
    assert_eq!(pipe.read(&mut buffer).unwrap(), Read::Pending);
    writer.write_all(b"x").unwrap();
    assert_eq!(pipe.read(&mut buffer).unwrap(), Read::Bytes(1));
    assert_eq!(buffer[0], b'x');
    assert_eq!(pipe.read(&mut buffer).unwrap(), Read::Pending);
    drop(writer);
    assert_eq!(pipe.read(&mut buffer).unwrap(), Read::End);
    pipe.finish().unwrap();
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
}
#[test]
fn native_write_end_is_refused_before_any_read_or_mode_lease() {
    let (_reader, writer) = stdin_pipe::pair_for_tests().unwrap();
    let retained = writer.try_clone().unwrap();
    assert_eq!(
        Pipe::owned_for_tests(writer).err(),
        Some(stdin_pipe::Error::Unsupported)
    );
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
}

fn read_only_queued(bytes: &[u8]) -> (Pipe, File) {
    let (reader, mut writer) = stdin_pipe::read_only_pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    writer.write_all(bytes).unwrap();
    drop(writer);
    (Pipe::owned_for_tests(reader).unwrap(), retained)
}
#[test]
fn read_only_native_pipe_observes_pending_bytes_and_actual_eof_without_upgrading_rights() {
    let (reader, mut writer) = stdin_pipe::read_only_pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    let mut pipe = Pipe::owned_for_tests(reader).unwrap();
    let mut buffer = [0; 16];
    assert_eq!(pipe.read(&mut buffer).unwrap(), Read::Pending);
    writer.write_all(b"x").unwrap();
    assert_eq!(pipe.read(&mut buffer).unwrap(), Read::Bytes(1));
    assert_eq!(buffer[0], b'x');
    assert_eq!(pipe.read(&mut buffer).unwrap(), Read::Pending);
    drop(writer);
    assert_eq!(pipe.read(&mut buffer).unwrap(), Read::End);
    pipe.finish().unwrap();
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
}
#[tokio::test]
async fn read_only_native_eof_admits_whole_utf8_original_bom_facts_or_refuses_before_session() {
    let f = Fixture::new().await;
    let bytes = "\u{feff}中文 🦀\r\n".as_bytes();
    let (pipe, retained) = read_only_queued(bytes);
    let (_, input) = PreparedStdinInput::prepare_for_tests(
        f.options(SessionSelection::Ephemeral),
        named(&[]),
        pipe,
        Limits::default(),
    )
    .await
    .unwrap()
    .into_parts();
    assert_eq!(input.body, "中文 🦀\r\n");
    assert_eq!(input.facts.instruction().kind(), OriginKind::Stdin);
    assert_eq!(input.facts.instruction().bytes(), bytes.len());
    assert!(input.facts.instruction().bom_removed());
    assert!(input.facts.instruction().path().is_none());
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    f.no_effects().await;
    for (bytes, expected) in [
        (&b""[..], InputError::Empty),
        (&b"\xff"[..], InputError::Utf8),
        ("\u{feff}".as_bytes(), InputError::Empty),
    ] {
        let (pipe, retained) = read_only_queued(bytes);
        assert_eq!(
            PreparedStdinInput::prepare_for_tests(
                f.options(SessionSelection::Fresh),
                named(&[]),
                pipe,
                Limits::default()
            )
            .await
            .err(),
            Some(StdinPreparationError::Input(Error::Input(expected)))
        );
        assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
        f.no_effects().await;
    }
}
#[tokio::test]
async fn read_only_idle_and_partial_source_keep_both_absolute_deadlines_before_session() {
    let f = Fixture::new().await;
    for (bytes, expected) in [
        (&b""[..], Error::FirstByteDeadline),
        (&b"x"[..], Error::TotalDeadline),
    ] {
        let (reader, mut writer) = stdin_pipe::read_only_pair_for_tests().unwrap();
        writer.write_all(bytes).unwrap();
        let retained = reader.try_clone().unwrap();
        let pipe = Pipe::owned_for_tests(reader).unwrap();
        let started = Instant::now();
        assert_eq!(
            PreparedStdinInput::prepare_for_tests(
                f.options(SessionSelection::Fresh),
                named(&[]),
                pipe,
                limits(80, 200)
            )
            .await
            .err(),
            Some(StdinPreparationError::Input(expected))
        );
        assert!(
            started.elapsed()
                >= if bytes.is_empty() {
                    Duration::from_millis(80)
                } else {
                    Duration::from_millis(200)
                }
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
        f.no_effects().await;
        drop(writer);
    }
}
#[tokio::test]
async fn read_only_native_trickle_never_restarts_total_clock() {
    let (reader, mut writer) = stdin_pipe::read_only_pair_for_tests().unwrap();
    writer.write_all(b"x").unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let task = tokio::spawn(async move {
        for _ in 0..15 {
            tokio::time::sleep(Duration::from_millis(15)).await;
            if writer.write_all(b"x").is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    assert_eq!(
        capture(pipe, started, limits(80, 200)).await.err(),
        Some(Error::TotalDeadline)
    );
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert!(started.elapsed() < Duration::from_secs(2));
    task.await.unwrap();
}
#[tokio::test]
async fn dropping_read_only_native_capture_preserves_later_actual_bytes() {
    let (reader, mut writer) = stdin_pipe::read_only_pair_for_tests().unwrap();
    let retained = reader.try_clone().unwrap();
    let pipe = Pipe::owned_for_tests(reader).unwrap();
    let task = tokio::spawn(capture(pipe, Instant::now(), Limits::default()));
    tokio::time::sleep(Duration::from_millis(30)).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    let late = b"late readonly bytes";
    writer.write_all(late).unwrap();
    let mut pipe = Pipe::owned_for_tests(retained).unwrap();
    let mut buffer = [0; 32];
    assert_eq!(pipe.read(&mut buffer).unwrap(), Read::Bytes(late.len()));
    assert_eq!(&buffer[..late.len()], late);
    pipe.finish().unwrap();
}
#[tokio::test]
async fn read_only_native_exact_cap_overflow_and_eof_keep_original_admission() {
    let f = Fixture::new().await;
    for (n, success) in [
        (super::super::INSTRUCTION_BYTES, true),
        (super::super::INSTRUCTION_BYTES + 1, false),
    ] {
        let (reader, mut writer) = stdin_pipe::read_only_pair_for_tests().unwrap();
        let retained = reader.try_clone().unwrap();
        let task = tokio::task::spawn_blocking(move || writer.write_all(&vec![b'x'; n]));
        let pipe = Pipe::owned_for_tests(reader).unwrap();
        let session = if success {
            SessionSelection::Ephemeral
        } else {
            SessionSelection::Fresh
        };
        let result = PreparedStdinInput::prepare_for_tests(
            f.options(session),
            named(&[]),
            pipe,
            Limits::default(),
        )
        .await;
        if success {
            let (_, input) = result.unwrap().into_parts();
            assert_eq!(input.body.len(), n);
            assert!(input.body.bytes().all(|b| b == b'x'));
            assert_eq!(input.facts.instruction().bytes(), n);
        } else {
            assert_eq!(
                result.err(),
                Some(StdinPreparationError::Input(Error::Input(
                    InputError::InstructionLimit
                )))
            );
        }
        task.await.unwrap().unwrap();
        assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
        f.no_effects().await;
    }
}
#[tokio::test]
async fn read_only_native_read_calls_include_original_data_and_actual_eof() {
    for (n, success) in [(CALLS - 1, true), (CALLS, false)] {
        let (pipe, retained) = read_only_queued(&vec![b'x'; n]);
        let result = capture(
            pipe.tighten_read_size_for_tests(1),
            Instant::now(),
            Limits::default(),
        )
        .await;
        if success {
            assert_eq!(result.unwrap().len(), n);
        } else {
            assert_eq!(result.err(), Some(Error::ReadCalls));
        }
        assert!(!stdin_pipe::nonblocking_for_tests(&retained).unwrap());
    }
}
