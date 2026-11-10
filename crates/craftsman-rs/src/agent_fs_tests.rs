//! Filesystem tests use the actual shared builder's scope/gate and synthetic
//! selected configuration. No arbitrary filesystem/seed/latch constructor.
use super::*;
use crate::exec::confined_fs::TextFs;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::permissions::{Mode, OnUnapproved, Selectors};
use crate::exec::runtime::InvocationGate;
use craftsman_harness::ToolErrorKind;
use std::time::{Duration, Instant};

struct Fixture {
    root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
}
struct Harness {
    filesystem: Arc<TextFs>,
    shell: Arc<crate::tools::ShellRunner>,
    gate: Arc<InvocationGate>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let config = root.path().join("selected");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        Self {
            root,
            workspace,
            config,
        }
    }
    fn put(&self, name: &str, content: impl AsRef<[u8]>) {
        let path = self.workspace.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    async fn harness(&self, extra: Value, selectors: Selectors) -> Harness {
        let mut config = json!({"model":"synthetic","api_key":"synthetic-secret",
            "shell":{"output_format":"head_tail","timeout_secs":30,"process_group":true}});
        config
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        std::fs::write(self.config.join("config.json"), config.to_string()).unwrap();
        let prepared = Prepared::prepare_with_environment_for_tests(
            Options {
                workspace: self.workspace.clone(),
                config_dir: self.config.clone(),
                trust_workspace: false,
                session: SessionSelection::Ephemeral,
                selectors,
            },
            BTreeMap::new(),
        )
        .await
        .unwrap();
        let (_, gate) = build_prepared_session(&prepared).await.unwrap();
        let harness = Harness {
            filesystem: Arc::new(TextFs::from_scope(FilesystemScope {
                prepared: &prepared,
                gate: gate.clone(),
            })),
            shell: Arc::new(crate::tools::ShellRunner::headless(ShellScope {
                prepared: &prepared,
                gate: gate.clone(),
            })),
            gate,
        };
        harness
    }
}
fn allow() -> Selectors {
    Selectors {
        allow: vec![
            "tool:write_file".into(),
            "tool:edit_file".into(),
            "tool:execute".into(),
        ],
        ..Default::default()
    }
}

#[tokio::test]
async fn whole_original_multiline_coverage_precedes_every_page_and_original_number() {
    let f = Fixture::new();
    let input = "before\nfirst\nmiddle\nlast\nafter\n";
    f.put("lines.txt", input);
    let h = f
        .harness(json!({"search_api_key":"first\nmiddle\nlast"}), allow())
        .await;
    for page in 0..5 {
        let outcome = h
            .filesystem
            .call(
                "read_file",
                json!({"file_path":"lines.txt","offset":page,"limit":1}),
            )
            .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert_eq!(
            outcome.content,
            format!(
                "{:>6}\t{}",
                page + 1,
                ["before", "[REDACTED]", "[REDACTED]", "[REDACTED]", "after"][page]
            )
        );
        assert_eq!(outcome.meta.bytes, Some(input.len() as u64));
        assert!(!outcome.meta.truncated && outcome.meta.duration.is_some());
    }
    assert_eq!(
        h.filesystem.read("lines.txt", 2, 1).await.unwrap(),
        "     3\t[REDACTED]"
    );
}
#[tokio::test]
async fn source_crlf_and_unicode_are_masked_before_original_pagination() {
    let f = Fixture::new();
    f.put("unicode.txt", "开头\r\n钥匙🙂\r\n尾巴\r\nlast\r");
    let h = f
        .harness(json!({"search_api_key":"钥匙🙂\r\n尾巴"}), allow())
        .await;
    assert_eq!(
        h.filesystem.read("unicode.txt", 1, 2).await.unwrap(),
        "     2\t[REDACTED]\n     3\t[REDACTED]"
    );
    assert_eq!(
        h.filesystem.read("unicode.txt", 3, 1).await.unwrap(),
        "     4\tlast\r"
    );
}
#[tokio::test]
async fn line_framing_credentials_do_not_change_original_source_positions_or_raw_bytes() {
    let f = Fixture::new();
    f.put("scalar.txt", "a\nb\nc\nd\ne\nf\ng");
    let h = f.harness(json!({"api_key":"7"}), allow()).await;
    let result = h
        .filesystem
        .call(
            "read_file",
            json!({"file_path":"scalar.txt","offset":6,"limit":1}),
        )
        .await;
    assert!(!result.is_error && !result.content.contains('7'));
    assert!(result.content.ends_with("\tg"));
    assert_eq!(result.meta.bytes, Some(13));
}
#[tokio::test]
async fn full_file_read_bound_and_output_cut_are_independent_and_do_not_keep_key_tails() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    for before in [98_950, 98_960, 98_970, 98_980, 99_000] {
        let input = format!(
            "{}synthetic-secret{}",
            "q".repeat(before),
            "z".repeat(100_000)
        );
        f.put("large.txt", &input);
        let result = h
            .filesystem
            .call("read_file", json!({"file_path":"large.txt","limit":1}))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.meta.truncated && result.content.len() <= 100_000);
        assert!(!result.content.contains("synthetic") && !result.content.contains("secret"));
        assert_eq!(result.meta.bytes, Some(input.len() as u64));
    }
    f.put("full-bound.txt", vec![b'q'; 10 * 1024 * 1024]);
    let result = h
        .filesystem
        .call("read_file", json!({"file_path":"full-bound.txt","limit":1}))
        .await;
    assert!(!result.is_error && result.meta.truncated);
    assert_eq!(result.meta.bytes, Some(10 * 1024 * 1024));
    let file = std::fs::File::create(f.workspace.join("oversize.txt")).unwrap();
    file.set_len(10 * 1024 * 1024 + 1).unwrap();
    let result = h
        .filesystem
        .call("read_file", json!({"file_path":"oversize.txt"}))
        .await;
    assert_eq!(result.meta.error_kind, Some(ToolErrorKind::OutputLimit));
    assert_eq!(result.meta.bytes, None);
}
#[tokio::test]
async fn mutations_use_original_paths_content_and_edit_matching_not_view_substitution() {
    let f = Fixture::new();
    let h = f.harness(json!({}), allow()).await;
    let path = "synthetic-secret.txt";
    let result = h
        .filesystem
        .call(
            "write_file",
            json!({"file_path":path,"content":"synthetic-secret"}),
        )
        .await;
    assert_eq!(result.content, "Updated file [REDACTED].txt");
    assert!(!result.is_error);
    assert_eq!(
        std::fs::read(f.workspace.join(path)).unwrap(),
        b"synthetic-secret"
    );
    assert!(!f.workspace.join("[REDACTED].txt").exists());
    let result=h.filesystem.call("edit_file", json!({"file_path":path,"old_string":"synthetic-secret","new_string":"new synthetic-secret"})).await;
    assert!(!result.is_error && !result.content.contains("synthetic-secret"));
    assert_eq!(
        std::fs::read(f.workspace.join(path)).unwrap(),
        b"new synthetic-secret"
    );
    assert_eq!(
        h.filesystem.read(path, 0, 1).await.unwrap(),
        "     1\tnew [REDACTED]"
    );
    let error = h
        .filesystem
        .call(
            "write_file",
            json!({"file_path":path,"content":"overwrite"}),
        )
        .await;
    assert_eq!(error.meta.error_kind, Some(ToolErrorKind::DeniedCapability));
    assert_eq!(
        std::fs::read(f.workspace.join(path)).unwrap(),
        b"new synthetic-secret"
    );
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}
#[tokio::test]
async fn listing_and_globbing_match_original_names_while_only_returning_safe_labels() {
    let f = Fixture::new();
    f.put("synthetic-secret.txt", "abc");
    f.put("folder/synthetic-secret.rs", "data");
    let h = f.harness(json!({}), allow()).await;
    let entries = h.filesystem.ls(".").await.unwrap();
    assert_eq!(
        entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
        ["folder/", "[REDACTED].txt"]
    );
    assert_eq!(entries[1].size, 3);
    assert_eq!(
        h.filesystem.glob("synthetic-secret.*").await.unwrap(),
        ["[REDACTED].txt"]
    );
    assert!(h.filesystem.glob("[REDACTED].*").await.unwrap().is_empty());
    let result = h
        .filesystem
        .call("glob", json!({"pattern":"absent-synthetic-secret"}))
        .await;
    assert_eq!(result.content, "No files match pattern 'absent-[REDACTED]'");
    assert!(!result.is_error);
}
#[tokio::test]
async fn duplicate_safe_name_labels_fail_closed_and_never_bind_an_alias_to_original_io() {
    let f = Fixture::new();
    f.put("synthetic-secret.txt", "original");
    f.put("[REDACTED].txt", "literal");
    let h = f.harness(json!({}), allow()).await;
    for name in ["ls", "glob"] {
        let args = if name == "ls" {
            json!({"path":"."})
        } else {
            json!({"pattern":"*"})
        };
        let out = h.filesystem.call(name, args).await;
        assert!(out.is_error);
        assert_eq!(out.content, "[REDACTED]");
        assert_eq!(out.meta.error_kind, Some(ToolErrorKind::Failed));
        assert!(out.meta.truncated);
    }
    // An explicitly requested literal marker filename is still that original
    // file; there is no reverse mapping to the redacted-name source.
    assert_eq!(
        h.filesystem.read("[REDACTED].txt", 0, 1).await.unwrap(),
        "     1\tliteral"
    );
}
#[tokio::test]
async fn source_body_replacement_collisions_fail_before_page_or_formatter_cuts() {
    let f = Fixture::new();
    f.put("collision.txt", "xsecret");
    let h = f
        .harness(
            json!({"api_key":"secret","search_api_key":"x[REDACTED]"}),
            allow(),
        )
        .await;
    let result = h
        .filesystem
        .call("read_file", json!({"file_path":"collision.txt"}))
        .await;
    assert!(result.is_error);
    assert_eq!(result.content, "[REDACTED]");
    assert_eq!(result.meta.error_kind, Some(ToolErrorKind::Failed));
    assert!(result.meta.truncated);
    assert_eq!(result.meta.bytes, Some(7));
}
#[tokio::test]
async fn grep_matches_original_lines_and_masks_whole_multiline_source_before_hit_selection() {
    let f = Fixture::new();
    let input = "before\nfirst\nneedle\nlast\nafter";
    f.put("synthetic-secret.txt", input);
    let h = f
        .harness(json!({"search_api_key":"first\nneedle\nlast"}), allow())
        .await;
    let out = h
        .filesystem
        .call(
            "grep",
            json!({"pattern":"needle","path":"synthetic-secret.txt"}),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "[REDACTED].txt:3:[REDACTED]\n");
    assert_eq!(out.meta.bytes, Some(input.len() as u64));
    assert!(!out.meta.truncated);
    let out = h
        .filesystem
        .call(
            "grep",
            json!({"pattern":"[REDACTED]","path":"synthetic-secret.txt"}),
        )
        .await;
    assert_eq!(out.content, "No matches found for '[REDACTED]'");
    assert!(!out.is_error);
}
#[tokio::test]
async fn grep_charges_binary_invalid_and_nonmatching_source_bytes_against_original_budget() {
    let f = Fixture::new();
    f.put("binary.bin", [0, 255, 1]);
    f.put("invalid.bin", [255, 254]);
    f.put("text.txt", "no match");
    let h = f.harness(json!({}), allow()).await;
    let out = h.filesystem.call("grep", json!({"pattern":"absent"})).await;
    assert!(!out.is_error);
    assert_eq!(out.meta.bytes, Some(13));
    for index in 0..7 {
        let file = std::fs::File::create(f.workspace.join(format!("large-{index}.bin"))).unwrap();
        file.set_len(10 * 1024 * 1024).unwrap();
    }
    let out = h.filesystem.call("grep", json!({"pattern":"absent"})).await;
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::OutputLimit));
    assert!(out.meta.bytes.unwrap() > 64 * 1024 * 1024 && out.meta.truncated);
}
#[tokio::test]
async fn unsafe_array_overflow_fails_but_text_listing_can_publish_a_bounded_safe_prefix() {
    let f = Fixture::new();
    let mut observed = 0;
    for index in 0..500 {
        let name = format!("{}{index:04}.txt", "qz".repeat(30));
        observed += name.len();
        f.put(&name, "");
    }
    let h = f.harness(json!({"api_key":"q"}), allow()).await;
    assert!(h.filesystem.ls(".").await.is_err());
    let out = h.filesystem.call("ls", json!({"path":"."})).await;
    assert!(!out.is_error && out.meta.truncated && out.content.len() <= 100_000);
    assert!(!out.content.contains('q'));
    assert_eq!(out.meta.bytes, Some(observed as u64));
}
#[tokio::test]
async fn fixed_diagnostics_and_invalid_source_utf8_preserve_typed_errors_without_echoing_data() {
    let f = Fixture::new();
    f.put("invalid.bin", [255]);
    let h = f.harness(json!({"api_key":"workspace"}), allow()).await;
    for (args, kind) in [
        (json!({"file_path":"missing"}), ToolErrorKind::NotFound),
        (
            json!({"file_path":"invalid.bin"}),
            ToolErrorKind::InvalidArgs,
        ),
        (
            json!({"file_path":"../outside"}),
            ToolErrorKind::InvalidArgs,
        ),
    ] {
        let out = h.filesystem.call("read_file", args).await;
        assert!(out.is_error);
        assert_eq!(out.meta.error_kind, Some(kind));
        assert!(!out.content.contains("workspace"));
    }
    assert!(!h.filesystem.supports_execution());
    assert!(
        !h.filesystem
            .execute("must-not-spawn")
            .await
            .unwrap_err()
            .to_string()
            .contains("workspace")
    );
}
#[tokio::test]
async fn immutable_seeds_do_not_reload_replaced_config_or_mask_original_mutation_payloads() {
    let f = Fixture::new();
    f.put("values.txt", "synthetic-secret replacement-secret");
    let h = f.harness(json!({}), allow()).await;
    std::fs::write(
        f.config.join("config.json"),
        json!({"model":"synthetic","api_key":"replacement-secret"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        h.filesystem.read("values.txt", 0, 1).await.unwrap(),
        "     1\t[REDACTED] replacement-secret"
    );
    h.filesystem
        .write("new.txt", "synthetic-secret")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(f.workspace.join("new.txt")).unwrap(),
        b"synthetic-secret"
    );
}
#[tokio::test]
async fn denied_and_plan_operations_do_not_create_parents_or_latch_false_approval_failures() {
    let f = Fixture::new();
    for selectors in [
        Selectors {
            mode: Mode::Plan,
            ..allow()
        },
        Selectors {
            deny: vec!["path:blocked.txt".into()],
            ..allow()
        },
    ] {
        let h = f.harness(json!({}), selectors).await;
        let out = h
            .filesystem
            .call(
                "write_file",
                json!({"file_path":"blocked.txt","content":"no"}),
            )
            .await;
        assert!(out.is_error);
        assert_eq!(out.meta.error_kind, Some(ToolErrorKind::DeniedCapability));
        assert!(!h.gate.approval_required());
        assert!(!f.workspace.join("blocked.txt").exists());
    }
}
#[tokio::test]
async fn typed_protected_write_failure_seals_even_if_a_direct_backend_caller_catches_it() {
    let f = Fixture::new();
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let h = f.harness(json!({}), selectors).await;
    let _caught = h
        .filesystem
        .write("build.rs", "must not run")
        .await
        .unwrap_err();
    assert!(h.gate.approval_required());
    let later = h
        .filesystem
        .call(
            "write_file",
            json!({"file_path":"later/file.txt","content":"no"}),
        )
        .await;
    assert_eq!(later.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert!(!f.workspace.join("build.rs").exists() && !f.workspace.join("later").exists());
    assert!(h.filesystem.read("no", 0, 1).await.is_err());
}
#[cfg(unix)]
#[tokio::test]
async fn scoped_text_io_uses_retained_authority_after_ambient_root_rebinding() {
    let f = Fixture::new();
    f.put("identity.txt", "synthetic-secret original");
    let h = f.harness(json!({}), allow()).await;
    let moved = f.root.path().join("retained");
    std::fs::rename(&f.workspace, &moved).unwrap();
    std::fs::create_dir(&f.workspace).unwrap();
    f.put("identity.txt", "replacement must not be read");
    assert_eq!(
        h.filesystem.read("identity.txt", 0, 1).await.unwrap(),
        "     1\t[REDACTED] original"
    );
    h.filesystem
        .write("new.txt", "original authority")
        .await
        .unwrap();
    assert!(moved.join("new.txt").exists() && !f.workspace.join("new.txt").exists());
}
#[tokio::test]
async fn same_typed_filesystem_failure_interrupts_an_actually_started_scoped_shell() {
    let f = Fixture::new();
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let h = f.harness(json!({}), selectors).await;
    let command = if cfg!(windows) {
        // Flush ORIGINAL output before the actual ready-file signal, without
        // cold cmdlet/provider startup inside the unchanged process deadline.
        "[Console]::Out.Write('synthetic-secret'); [Console]::Out.Flush(); [IO.File]::WriteAllText('ready.txt','ready'); [Threading.Thread]::Sleep(30000)"
    } else {
        "printf synthetic-secret; printf ready > ready.txt; sleep 30"
    };
    let task = tokio::spawn({
        let shell = h.shell.clone();
        async move { shell.run_outcome(command, None).await }
    });
    tokio::time::timeout(Duration::from_secs(20), async {
        while !f.workspace.join("ready.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .expect("real shell startup before stop measurement");
    let at = Instant::now();
    let out = h
        .filesystem
        .call("write_file", json!({"file_path":"build.rs","content":"no"}))
        .await;
    assert_eq!(out.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    let stopped = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert!(at.elapsed() < Duration::from_secs(10));
    assert_eq!(stopped.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert_eq!(stopped.content, "[REDACTED]");
    assert_eq!(stopped.meta.bytes, Some(16)); // ORIGINAL bytes actually observed.
    assert!(!f.workspace.join("build.rs").exists());
}
