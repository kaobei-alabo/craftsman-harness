use super::*;
use crate::exec::construction::{InputPreparationError, Options, PreparedInput, SessionSelection};
use crate::exec::permissions::{InvocationPermissions, OnUnapproved, Selectors};
use crate::exec::runtime::RuntimeError;
use craftsman_harness::{Checkpointer, Msg};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let f = Self {
            _root: root,
            workspace,
            config,
        };
        f.configure("http://127.0.0.1:1", "synthetic-input-fixture-key");
        f
    }
    fn configure(&self, url: &str, key: &str) {
        std::fs::write(
            self.config.join("config.json"),
            json!({"model":"synthetic-model","api_key":key,"base_url":url}).to_string(),
        )
        .unwrap();
    }
    fn options(&self, session: SessionSelection) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session,
            selectors: Selectors::default(),
        }
    }
    fn file(&self, name: &str, bytes: impl AsRef<[u8]>) {
        let target = self.workspace.join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(target, bytes).unwrap();
    }
    fn no_store(&self) {
        assert!(!self.config.join("threads.db").exists());
        assert!(!self.config.join("sessions-rs").exists());
        assert!(!self.config.join("gadgets/.sdk").exists());
    }
    fn filesystem(&self) -> ConfinedFs {
        ConfinedFs::new(
            &self.workspace,
            &self.config,
            Arc::new(InvocationPermissions::from_explicit(Selectors::default(), &[]).unwrap()),
        )
        .unwrap()
    }
}
fn literal(prompt: &[u8]) -> Sources<'_> {
    Sources {
        prompt: Some(prompt),
        prompt_file: None,
        files: &[],
    }
}
fn selection_error(sources: Sources<'_>, expected: InputError) {
    assert_eq!(Selected::select(sources).err(), Some(expected));
}
async fn rejection(f: &Fixture, sources: Sources<'_>, expected: InputError) {
    let error = PreparedInput::prepare(f.options(SessionSelection::Fresh), sources)
        .await
        .err()
        .unwrap();
    assert_eq!(error, InputPreparationError::Input(expected));
    f.no_store();
}
async fn admitted(
    f: &Fixture,
    sources: Sources<'_>,
) -> (crate::exec::construction::Prepared, Admitted) {
    PreparedInput::prepare(f.options(SessionSelection::Ephemeral), sources)
        .await
        .unwrap()
        .into_parts()
}
fn response(events: &[Value]) -> ResponseTemplate {
    let mut body = String::new();
    for event in events {
        body.push_str(&format!("data: {event}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "text/event-stream")
}
async fn answer(server: &MockServer, key: &str) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", format!("Bearer {key}")))
        .respond_with(response(&[
            json!({"choices":[{"delta":{"content":"done"}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":2}}),
        ]))
        .mount(server)
        .await;
}

#[test]
fn exactly_one_instruction_source_is_required_and_attachments_are_not_instructions() {
    selection_error(
        Sources {
            prompt: None,
            prompt_file: None,
            files: &[],
        },
        InputError::MissingSource,
    );
    selection_error(
        Sources {
            prompt: None,
            prompt_file: None,
            files: &["data.txt"],
        },
        InputError::MissingSource,
    );
    selection_error(
        Sources {
            prompt: Some(b"goal"),
            prompt_file: Some("goal.txt"),
            files: &[],
        },
        InputError::ConflictingSources,
    );
}

#[test]
fn explicit_stdin_is_refused_not_silently_read_or_reinterpreted_as_a_prompt() {
    selection_error(literal(b"-"), InputError::StdinUnavailable);
    selection_error(
        Sources {
            prompt: Some(b"-"),
            prompt_file: Some("goal.txt"),
            files: &[],
        },
        InputError::ConflictingSources,
    );
    assert!(Selected::select(literal(b" - ")).is_ok());
    assert!(
        Selected::select(Sources {
            prompt: None,
            prompt_file: Some("-"),
            files: &[]
        })
        .is_ok()
    );
}

#[test]
fn literal_utf8_and_whitespace_are_validated_before_cloning_or_bootstrap() {
    for text in ["", " \t\r\n", "\u{2003}\u{3000}"] {
        selection_error(literal(text.as_bytes()), InputError::Empty);
    }
    for text in [&b"\xff"[..], &b"goal\xf0\x9f"[..], &b"\xc0\xaf"[..]] {
        selection_error(literal(text), InputError::Utf8);
    }
}

#[test]
fn literal_byte_ceiling_is_exact_and_never_uses_scalar_count_or_truncation() {
    let exact = "é".repeat(INSTRUCTION_BYTES / 2);
    assert!(Selected::select(literal(exact.as_bytes())).is_ok());
    let over = format!("{exact}x");
    selection_error(literal(over.as_bytes()), InputError::InstructionLimit);
}

#[test]
fn all_file_labels_and_zero_byte_file_count_have_private_bounds() {
    selection_error(
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &[""],
        },
        InputError::Selection,
    );
    selection_error(
        Sources {
            prompt: None,
            prompt_file: Some("bad\0path"),
            files: &[],
        },
        InputError::Selection,
    );
    let paths = vec!["empty.txt"; FILES + 1];
    selection_error(
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &paths,
        },
        InputError::SelectionLimit,
    );
    let too_long = "x".repeat(PATH_SCALARS + 1);
    selection_error(
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &[&too_long],
        },
        InputError::SelectionLimit,
    );
}

#[tokio::test]
async fn invalid_grammar_and_literal_input_return_before_config_or_session_effects() {
    let f = Fixture::new();
    std::fs::write(f.config.join("config.json"), "not JSON").unwrap();
    rejection(&f, literal(b"-"), InputError::StdinUnavailable).await;
    rejection(&f, literal(b"\xff"), InputError::Utf8).await;
    rejection(&f, literal(b" \t"), InputError::Empty).await;
    rejection(
        &f,
        Sources {
            prompt: None,
            prompt_file: None,
            files: &[],
        },
        InputError::MissingSource,
    )
    .await;
}

#[tokio::test]
async fn literal_input_preserves_original_unicode_whitespace_and_bom_data() {
    let f = Fixture::new();
    let text = "\u{feff}  !literal /not-a-command 中文🦀\r\n";
    let (prepared, input) = admitted(&f, literal(text.as_bytes())).await;
    assert_eq!(input.body, text);
    assert_eq!(input.facts.instruction().kind(), OriginKind::Argument);
    assert_eq!(input.facts.instruction().bytes(), text.len());
    assert_eq!(input.facts.instruction().path(), None);
    assert!(!input.facts.instruction().bom_removed());
    assert!(input.facts.attachments().is_empty());
    assert_eq!(input.facts.attachment_bytes(), 0);
    assert!(prepared.persistence().thread_id().is_none());
    f.no_store();
}

#[tokio::test]
async fn instruction_file_removes_only_one_leading_bom_and_counts_original_bytes() {
    let f = Fixture::new();
    let raw = "\u{feff}\u{feff}  inspect 中文\r\n";
    f.file("goal.txt", raw);
    let (_, input) = admitted(
        &f,
        Sources {
            prompt: None,
            prompt_file: Some("goal.txt"),
            files: &[],
        },
    )
    .await;
    assert_eq!(input.body, "\u{feff}  inspect 中文\r\n");
    assert_eq!(input.facts.instruction().kind(), OriginKind::WorkspaceFile);
    assert_eq!(input.facts.instruction().path(), Some("goal.txt"));
    assert_eq!(input.facts.instruction().bytes(), raw.len());
    assert!(input.facts.instruction().bom_removed());
    assert_eq!(
        std::fs::read(f.workspace.join("goal.txt")).unwrap(),
        raw.as_bytes()
    );
}

#[tokio::test]
async fn empty_whitespace_and_malformed_instruction_files_never_create_a_store() {
    let f = Fixture::new();
    for bytes in [&b""[..], &b" \r\n"[..], &b"\xef\xbb\xbf"[..]] {
        f.file("goal.txt", bytes);
        rejection(
            &f,
            Sources {
                prompt: None,
                prompt_file: Some("goal.txt"),
                files: &[],
            },
            InputError::Empty,
        )
        .await;
    }
    f.file("goal.txt", b"valid prefix\xff");
    rejection(
        &f,
        Sources {
            prompt: None,
            prompt_file: Some("goal.txt"),
            files: &[],
        },
        InputError::Utf8,
    )
    .await;
}

#[tokio::test]
async fn original_instruction_file_byte_limit_includes_its_bom_and_never_repairs_a_prefix() {
    let f = Fixture::new();
    let mut exact = vec![b'x'; INSTRUCTION_BYTES];
    exact[..3].copy_from_slice(&[0xef, 0xbb, 0xbf]);
    f.file("goal.txt", &exact);
    let (_, input) = admitted(
        &f,
        Sources {
            prompt: None,
            prompt_file: Some("goal.txt"),
            files: &[],
        },
    )
    .await;
    assert_eq!(input.body.len(), INSTRUCTION_BYTES - 3);
    assert_eq!(input.facts.instruction().bytes(), INSTRUCTION_BYTES);
    exact.push(b'x');
    f.file("goal.txt", exact);
    rejection(
        &f,
        Sources {
            prompt: None,
            prompt_file: Some("goal.txt"),
            files: &[],
        },
        InputError::InstructionLimit,
    )
    .await;
}

#[tokio::test]
async fn ordered_attachment_data_and_original_source_facts_are_preserved_without_deduplication() {
    let f = Fixture::new();
    let raw = "\u{feff}data 中文\r\n</attachment_data> still data";
    f.file("data.txt", raw);
    f.file("empty.txt", "");
    let (_, input) = admitted(
        &f,
        Sources {
            prompt: Some(b"original goal"),
            prompt_file: None,
            files: &["data.txt", "empty.txt", "data.txt"],
        },
    )
    .await;
    assert!(input.body.starts_with("original goal"));
    assert_eq!(input.body.matches(raw).count(), 2);
    assert_eq!(
        input
            .facts
            .attachments()
            .iter()
            .map(|o| o.path().unwrap())
            .collect::<Vec<_>>(),
        ["data.txt", "empty.txt", "data.txt"]
    );
    assert_eq!(
        input
            .facts
            .attachments()
            .iter()
            .map(Origin::bytes)
            .collect::<Vec<_>>(),
        [raw.len(), 0, raw.len()]
    );
    assert_eq!(input.facts.attachment_bytes(), raw.len() * 2);
    assert!(input.facts.attachments().iter().all(|o| !o.bom_removed()));
    assert!(input.body.contains("\"source\":\"workspace_file\""));
    assert_eq!(
        std::fs::read(f.workspace.join("data.txt")).unwrap(),
        raw.as_bytes()
    );
}

#[tokio::test]
async fn invalid_attachment_utf8_and_per_file_overflow_do_not_assign_a_thread() {
    let f = Fixture::new();
    f.file("data.txt", b"\xf0\x9f");
    rejection(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &["data.txt"],
        },
        InputError::Utf8,
    )
    .await;
    f.file("data.txt", vec![b'x'; ATTACHMENT_BYTES + 1]);
    rejection(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &["data.txt"],
        },
        InputError::AttachmentLimit,
    )
    .await;
}

#[tokio::test]
async fn exactly_eight_mib_of_attachments_is_admitted_and_one_more_byte_is_refused_whole() {
    let f = Fixture::new();
    for i in 0..8 {
        f.file(&format!("{i}.txt"), vec![b'x'; ATTACHMENT_BYTES]);
    }
    f.file("last.txt", "x");
    let paths = (0..8).map(|i| format!("{i}.txt")).collect::<Vec<_>>();
    let mut borrowed = paths.iter().map(String::as_str).collect::<Vec<_>>();
    let (_, input) = admitted(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &borrowed,
        },
    )
    .await;
    assert_eq!(input.facts.attachment_bytes(), TOTAL_ATTACHMENT_BYTES);
    assert_eq!(input.facts.attachments().len(), 8);
    assert!(input.body.len() <= MESSAGE_BYTES);
    borrowed.push("last.txt");
    rejection(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &borrowed,
        },
        InputError::AttachmentTotal,
    )
    .await;
}

#[tokio::test]
async fn an_empty_attachment_is_valid_when_the_actual_aggregate_is_exactly_exhausted() {
    let f = Fixture::new();
    f.file("full.txt", vec![b'x'; ATTACHMENT_BYTES]);
    f.file("empty.txt", "");
    let mut paths = vec!["full.txt"; 8];
    paths.push("empty.txt");
    let (_, input) = admitted(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &paths,
        },
    )
    .await;
    assert_eq!(input.facts.attachment_bytes(), TOTAL_ATTACHMENT_BYTES);
    assert_eq!(input.facts.attachments().last().unwrap().bytes(), 0);
}

#[tokio::test]
async fn a_nonzero_remaining_aggregate_limit_is_enforced_before_retaining_a_larger_file() {
    let f = Fixture::new();
    f.file("full.txt", vec![b'x'; ATTACHMENT_BYTES]);
    f.file("almost.txt", vec![b'x'; ATTACHMENT_BYTES - 1]);
    f.file("last.txt", "xx");
    let mut paths = vec!["full.txt"; 7];
    paths.extend(["almost.txt", "last.txt"]);
    rejection(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &paths,
        },
        InputError::AttachmentTotal,
    )
    .await;
}

#[tokio::test]
async fn default_confined_authority_still_denies_config_credentials_and_outside_input_paths() {
    let f = Fixture::new();
    f.file(".env", "must not become input");
    for file in [".env", "../selected-config/config.json", "missing.txt", "."] {
        rejection(
            &f,
            Sources {
                prompt: None,
                prompt_file: Some(file),
                files: &[],
            },
            InputError::Source,
        )
        .await;
    }
    let absolute = f.config.join("config.json");
    rejection(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &[absolute.to_str().unwrap()],
        },
        InputError::Source,
    )
    .await;
}

#[tokio::test]
async fn explicit_input_selection_never_overrides_a_tool_or_file_deny() {
    let f = Fixture::new();
    f.file("data.txt", "data");
    for deny in ["tool:read_file", "path:data.txt"] {
        let mut options = f.options(SessionSelection::Fresh);
        options.selectors.deny.push(deny.into());
        let error = PreparedInput::prepare(
            options,
            Sources {
                prompt: Some(b"goal"),
                prompt_file: None,
                files: &["data.txt"],
            },
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error, InputPreparationError::Input(InputError::Source));
        f.no_store();
    }
}

#[tokio::test]
async fn input_must_be_a_regular_single_link_file_not_a_directory_or_hard_link() {
    let f = Fixture::new();
    f.file("data.txt", "data");
    std::fs::create_dir(f.workspace.join("directory")).unwrap();
    rejection(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &["directory"],
        },
        InputError::Source,
    )
    .await;
    std::fs::hard_link(f.workspace.join("data.txt"), f.workspace.join("linked.txt")).unwrap();
    rejection(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &["linked.txt"],
        },
        InputError::Source,
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn input_rejects_leaf_and_parent_symlinks_without_creating_session_storage() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.file("real/data.txt", "data");
    symlink("real/data.txt", f.workspace.join("leaf.txt")).unwrap();
    symlink("real", f.workspace.join("parent")).unwrap();
    for file in ["leaf.txt", "parent/data.txt"] {
        rejection(
            &f,
            Sources {
                prompt: Some(b"goal"),
                prompt_file: None,
                files: &[file],
            },
            InputError::Source,
        )
        .await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn file_input_uses_the_retained_root_after_the_ambient_path_is_replaced() {
    let f = Fixture::new();
    f.file("goal.txt", "ORIGINAL captured authority");
    let fs = f.filesystem();
    let moved = f.workspace.with_file_name("retained-workspace");
    std::fs::rename(&f.workspace, &moved).unwrap();
    std::fs::create_dir(&f.workspace).unwrap();
    f.file("goal.txt", "ambient replacement must not be read");
    let input = Selected::select(Sources {
        prompt: None,
        prompt_file: Some("goal.txt"),
        files: &[],
    })
    .unwrap()
    .load(&fs)
    .await
    .unwrap();
    assert_eq!(input.body, "ORIGINAL captured authority");
    f.no_store();
}

#[cfg(unix)]
#[tokio::test]
async fn attachment_labels_are_json_encoded_without_aliasing_actual_io_paths() {
    let f = Fixture::new();
    let label = "quoted\"中文.txt";
    f.file(label, "ORIGINAL body");
    let (_, input) = admitted(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &[label],
        },
    )
    .await;
    let line = input.body.lines().find(|s| s.starts_with('{')).unwrap();
    let header: Value = serde_json::from_str(line).unwrap();
    assert_eq!(header["path"], label);
    assert_eq!(header["bytes"], "ORIGINAL body".len());
    assert_eq!(input.facts.attachments()[0].path(), Some(label));
    assert_eq!(
        std::fs::read(f.workspace.join(label)).unwrap(),
        b"ORIGINAL body"
    );
}

#[test]
fn header_encoding_cannot_turn_label_controls_into_new_header_lines() {
    let label = "quoted\"\n\0中文";
    let encoded = serde_json::to_string(&Header {
        source: "workspace_file",
        path: label,
        bytes: 0,
        index: 0,
    })
    .unwrap();
    assert!(!encoded.contains('\n') && !encoded.contains('\0'));
    let value: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(value["path"], label);
}

#[cfg(unix)]
#[tokio::test]
async fn existing_confined_control_path_denials_are_not_bypassed_by_input_selection() {
    let f = Fixture::new();
    let label = "control\nname.txt";
    f.file(label, "data");
    rejection(
        &f,
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &[label],
        },
        InputError::Source,
    )
    .await;
}

#[tokio::test]
async fn bounded_input_read_cannot_widen_the_input_ceiling_or_change_the_old_raw_read_bound() {
    let f = Fixture::new();
    f.file("data.txt", vec![b'x'; INSTRUCTION_BYTES + 1]);
    let fs = f.filesystem();
    assert_eq!(
        fs.read_input_raw("data.txt", INSTRUCTION_BYTES).await.err(),
        Some(FsError::InputTooLarge)
    );
    assert_eq!(
        fs.read_input_raw("data.txt", usize::MAX).await.err(),
        Some(FsError::InputTooLarge)
    );
    assert_eq!(
        fs.read_raw("data.txt").await.unwrap().len(),
        INSTRUCTION_BYTES + 1
    );
    f.file("empty.txt", "");
    assert!(fs.read_input_raw("empty.txt", 0).await.unwrap().is_empty());
    assert_eq!(
        fs.read_input_raw("data.txt", 0).await.err(),
        Some(FsError::InputTooLarge)
    );
}

#[tokio::test]
async fn fresh_thread_creation_follows_complete_input_and_framing_admission() {
    let f = Fixture::new();
    f.file("data.txt", "accepted original data");
    let (prepared, input) = PreparedInput::prepare(
        f.options(SessionSelection::Fresh),
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &["data.txt"],
        },
    )
    .await
    .unwrap()
    .into_parts();
    assert!(input.body.contains("accepted original data"));
    assert!(prepared.initial_state().messages.is_empty());
    let rows = prepared
        .persistence()
        .store()
        .unwrap()
        .list_threads()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, prepared.persistence().thread_id().unwrap());
    assert!(!f.config.join("sessions-rs").exists());
}

#[test]
fn input_error_text_never_echoes_original_paths_bytes_or_os_detail() {
    let errors = [
        InputError::MissingSource,
        InputError::ConflictingSources,
        InputError::Empty,
        InputError::InstructionLimit,
        InputError::AttachmentLimit,
        InputError::AttachmentTotal,
        InputError::SelectionLimit,
        InputError::Selection,
        InputError::Utf8,
        InputError::Source,
        InputError::StdinUnavailable,
        InputError::Framing,
    ];
    for error in errors {
        let text = error.to_string();
        assert!(!text.contains("synthetic-input-fixture-key"));
        assert!(!text.contains("/private/secret-source"));
        assert!(text.is_ascii());
    }
}

#[tokio::test]
async fn the_actual_shared_builder_receives_literal_input_as_model_data_not_a_command_route() {
    let f = Fixture::new();
    let server = MockServer::start().await;
    let key = "synthetic-input-fixture-key";
    f.configure(&server.uri(), key);
    answer(&server, key).await;
    let prompt = "!not-native /not-a-slash-command 中文\r\n";
    let invocation = Invocation::prepare(
        f.options(SessionSelection::Ephemeral),
        literal(prompt.as_bytes()),
    )
    .await
    .unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(invocation.prepared().persistence().thread_id().is_none());
    let run: Run = invocation.run().await;
    assert!(run.outcome().is_ok());
    assert!(
        run.retained_state()
            .messages
            .iter()
            .any(|m| matches!(m, Msg::User { content } if content == prompt))
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let request: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(
        request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "user" && m["content"] == prompt)
    );
    assert_eq!(run.facts().instruction().bytes(), prompt.len());
    f.no_store();
}

#[tokio::test]
async fn original_input_snapshot_and_captured_provider_auth_do_not_reload_later_file_or_config_changes()
 {
    let f = Fixture::new();
    let server = MockServer::start().await;
    let key = "original-input-provider-credential";
    f.configure(&server.uri(), key);
    answer(&server, key).await;
    let original = format!("ORIGINAL data {key} 中文\r\n");
    f.file("data.txt", &original);
    let invocation = Invocation::prepare(
        f.options(SessionSelection::Ephemeral),
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &["data.txt"],
        },
    )
    .await
    .unwrap();
    f.file("data.txt", "LATER source replacement");
    f.configure("http://127.0.0.1:1", "LATER auth replacement");
    let run = invocation.run().await;
    assert!(run.outcome().is_ok());
    assert!(
        run.retained_state()
            .messages
            .iter()
            .any(|m| matches!(m, Msg::User { content } if content.contains(&original)))
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = std::str::from_utf8(&requests[0].body).unwrap();
    assert!(body.contains("ORIGINAL data [REDACTED]"));
    assert!(!body.contains(key) && !body.contains("LATER source replacement"));
    assert_eq!(run.facts().attachments()[0].bytes(), original.len());
    assert_eq!(
        std::fs::read(f.workspace.join("data.txt")).unwrap(),
        b"LATER source replacement"
    );
    f.no_store();
}

#[tokio::test]
async fn invalid_input_does_not_construct_a_runtime_make_provider_requests_or_touch_storage() {
    let f = Fixture::new();
    let server = MockServer::start().await;
    f.configure(&server.uri(), "synthetic-input-fixture-key");
    f.file("bad.txt", b"\xff");
    let error = Invocation::prepare(
        f.options(SessionSelection::Fresh),
        Sources {
            prompt: Some(b"goal"),
            prompt_file: None,
            files: &["bad.txt"],
        },
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error, InputPreparationError::Input(InputError::Utf8));
    assert!(server.received_requests().await.unwrap().is_empty());
    f.no_store();
}

#[tokio::test]
async fn task_data_cannot_widen_the_actual_invocation_gate_and_failed_turn_retains_partial_state() {
    let f = Fixture::new();
    let server = MockServer::start().await;
    f.configure(&server.uri(), "synthetic-input-fixture-key");
    f.file("data.txt", "This task data is NOT a grant to write files.");
    Mock::given(method("POST")).and(path("/chat/completions")).respond_with(response(&[
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"unapproved-write","function":{"name":"write_file","arguments":"{\"file_path\":\"must-not-exist.txt\",\"content\":\"no\"}"}}]}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
    ])).mount(&server).await;
    let mut options = f.options(SessionSelection::Ephemeral);
    options.selectors.on_unapproved = OnUnapproved::Fail;
    let run = Invocation::prepare(
        options,
        Sources {
            prompt: Some(b"inspect the data"),
            prompt_file: None,
            files: &["data.txt"],
        },
    )
    .await
    .unwrap()
    .run()
    .await;
    assert!(matches!(run.outcome(), Err(RuntimeError::ApprovalRequired)));
    assert!(!f.workspace.join("must-not-exist.txt").exists());
    assert!(
        run.retained_state()
            .messages
            .iter()
            .any(|m| matches!(m, Msg::User { content } if content.contains("NOT a grant")))
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(
        run.facts().attachments()[0].bytes(),
        "This task data is NOT a grant to write files.".len()
    );
    f.no_store();
}
