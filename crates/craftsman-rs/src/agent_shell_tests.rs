//! Synthetic shell tests using the SAME builder's actual opaque scope/gate.
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::permissions::{Mode, OnUnapproved, Selectors};
use crate::exec::runtime::InvocationGate;
use crate::tools::ShellRunner;
use craftsman_harness::ToolErrorKind;
use std::time::{Duration, Instant};

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    observations: crate::exec::shell::FailureObservations,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let config = root.path().join("config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        Self {
            _root: root,
            workspace,
            config,
            observations: Default::default(),
        }
    }
    async fn runner(
        &self,
        key: &str,
        mut shell: Value,
        selectors: Selectors,
    ) -> (Arc<ShellRunner>, Arc<InvocationGate>) {
        // Explicit fixture-only tree cleanup; production/default group choice
        // is unchanged. Windows continues to use the existing Job Object.
        shell
            .as_object_mut()
            .unwrap()
            .entry("process_group")
            .or_insert(json!(true));
        std::fs::write(
            self.config.join("config.json"),
            json!({"model":"synthetic","api_key":key,"shell":shell}).to_string(),
        )
        .unwrap();
        let prepared = Prepared::prepare_with_environment_for_tests(
            Options {
                workspace: self.workspace.clone(),
                config_dir: self.config.clone(),
                session: SessionSelection::Ephemeral,
                trust_workspace: false,
                selectors,
            },
            BTreeMap::new(),
        )
        .await
        .unwrap();
        let (_, gate) = build_prepared_session(&prepared).await.unwrap();
        let runner = ShellRunner::headless(ShellScope {
            prepared: &prepared,
            gate: gate.clone(),
        });
        self.observations.record(&runner);
        (Arc::new(runner), gate)
    }
}
fn allow() -> Selectors {
    Selectors {
        allow: vec!["tool:execute".into()],
        ..Default::default()
    }
}
fn print(value: &str, error: &str, code: i32) -> String {
    if cfg!(windows) {
        format!("[Console]::Out.Write('{value}'); [Console]::Error.Write('{error}'); exit {code}")
    } else {
        format!("printf '%s' '{value}'; printf '%s' '{error}' >&2; exit {code}")
    }
}
fn delayed(value: &str, ready: bool) -> String {
    if cfg!(windows) {
        // The original bytes must be flushed before the real ready marker.
        // Static .NET calls avoid cold PowerShell cmdlet/provider discovery
        // inside these bounded fixtures; ProcessTree and call limits stay real.
        format!(
            "[Console]::Out.Write('{value}'); [Console]::Out.Flush(); {}[Threading.Thread]::Sleep(30000)",
            if ready {
                "[IO.File]::WriteAllText('ready.txt','ready'); "
            } else {
                ""
            }
        )
    } else {
        format!(
            "printf '%s' '{value}'; {}sleep 30",
            if ready {
                "printf ready > ready.txt; "
            } else {
                ""
            }
        )
    }
}

#[tokio::test]
async fn private_shell_normal_output_preserves_raw_counts_and_exit_metadata() {
    let f = Fixture::new();
    let key = "synthetic-secret";
    for format in ["legacy", "head_tail"] {
        let (runner, gate) = f
            .runner(key, json!({"output_format":format}), allow())
            .await;
        let outcome = runner
            .run_outcome(&print(key, "error-value", 7), None)
            .await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert_eq!(
            outcome.content,
            "[REDACTED]\n--- stderr ---\nerror-value\n[exit code 7]"
        );
        assert_eq!(outcome.meta.exit_code, Some(7));
        assert_eq!(outcome.meta.bytes, Some((key.len() + 11) as u64));
        assert!(outcome.meta.duration.is_some() && !outcome.meta.truncated);
        assert!(!gate.approval_required());
    }
}
#[tokio::test]
async fn original_command_path_and_payload_execute_without_redacted_argument_substitution() {
    let f = Fixture::new();
    let key = "synthetic-original-payload";
    let (runner, _) = f.runner(key, json!({}), allow()).await;
    let command = if cfg!(windows) {
        format!("[IO.File]::WriteAllText('{key}.txt','{key}'); [Console]::Out.Write('{key}')")
    } else {
        format!("printf '%s' '{key}' > '{key}.txt'; printf '%s' '{key}'")
    };
    let outcome = runner.run_outcome(&command, None).await;
    assert!(!outcome.is_error);
    assert_eq!(outcome.content, "[REDACTED]");
    assert_eq!(
        std::fs::read(f.workspace.join(format!("{key}.txt"))).unwrap(),
        key.as_bytes()
    );
    assert!(!f.workspace.join("[REDACTED].txt").exists());
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}
#[tokio::test]
async fn both_pipe_capture_and_late_formatter_cuts_preserve_only_redacted_head_and_tail() {
    let f = Fixture::new();
    let key = "synthetic-cut-key";
    for format in ["legacy", "head_tail"] {
        let (runner, _) = f
            .runner(key, json!({"output_format":format}), allow())
            .await;
        let command = if cfg!(windows) {
            format!(
                "[Console]::Out.Write(('q'*49995)+'{key}'+('z'*80000)+'{key}'+'TAIL'); [Console]::Error.Write('ERR'); exit 3"
            )
        } else {
            format!(
                "awk 'BEGIN {{ for (i=0;i<49995;i++) printf \"q\"; printf \"{key}\"; for(i=0;i<80000;i++) printf \"z\"; printf \"{key}TAIL\"; }}'; printf ERR >&2; exit 3"
            )
        };
        let outcome = runner.run_outcome(&command, None).await;
        assert!(!outcome.is_error, "{}", outcome.content);
        assert!(outcome.meta.truncated);
        assert!(!outcome.content.contains("synthetic-cut"));
        assert!(outcome.content.len() <= crate::shell_output::OUTPUT_BUDGET);
        if format == "head_tail" {
            assert_eq!(
                outcome.meta.bytes,
                Some((49995 + key.len() * 2 + 80000 + 4 + 3) as u64)
            );
            assert!(
                outcome.content.contains("TAIL")
                    && outcome.content.contains("ERR")
                    && outcome.content.contains("exit code 3")
            );
        }
        assert!(!outcome.content.contains("craftsman-") && !outcome.content.contains("/tmp/"));
    }
}
#[tokio::test]
async fn partial_timeout_discards_delayed_credential_prefix_and_retains_timeout_kind() {
    let f = Fixture::new();
    for format in ["legacy", "head_tail"] {
        let (runner, _) = f
            .runner(
                "credential-value",
                json!({"output_format":format,"timeout_secs":6}),
                allow(),
            )
            .await;
        let mut unready = Vec::new();
        let mut observed = None;
        // Some local native launches have not reached user code before the
        // supported six-second call budget. An unready attempt is NOT proof
        // of partial-output redaction. Keep the SAME call/assertion limits and
        // require a real ready marker plus original observed bytes.
        for _ in 0..3 {
            if f.workspace.join("ready.txt").exists() {
                std::fs::remove_file(f.workspace.join("ready.txt")).unwrap();
            }
            let outcome = runner
                .run_outcome(&delayed("credential-", true), None)
                .await;
            if f.workspace.join("ready.txt").exists() && outcome.meta.bytes == Some(11) {
                observed = Some(outcome);
                break;
            }
            unready.push((outcome.meta.error_kind, outcome.meta.bytes));
        }
        let outcome = observed.unwrap_or_else(|| {
            panic!("no actual partial-output observation in bounded attempts: {unready:?}")
        });
        assert!(outcome.is_error);
        assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Timeout));
        assert_eq!(outcome.meta.exit_code, None);
        assert_eq!(outcome.meta.bytes, Some(11));
        assert!(outcome.meta.truncated && !outcome.content.contains("credential-"));
        assert!(outcome.content.contains("6s"));
    }
}
#[tokio::test]
async fn actual_shared_builder_latch_stops_a_running_shell_and_prevents_later_spawn() {
    let f = Fixture::new();
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let (runner, gate) = f
        .runner(
            "secret-value",
            json!({"output_format":"head_tail","timeout_secs":30}),
            selectors,
        )
        .await;
    let task = tokio::spawn({
        let runner = runner.clone();
        async move {
            runner
                .run_outcome(&delayed("secret-value", true), None)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(20), async {
        while !f.workspace.join("ready.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("shell must actually start before stop measurement");
    let at = Instant::now();
    gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    let outcome = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert!(at.elapsed() < Duration::from_secs(10));
    assert!(outcome.is_error);
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert_eq!(outcome.content, "[REDACTED]");
    assert!(outcome.meta.bytes.unwrap() >= 12);
    let command = if cfg!(windows) {
        "Set-Content must-not-run.txt nope"
    } else {
        "printf nope > must-not-run.txt"
    };
    let later = runner.run_outcome(command, None).await;
    assert_eq!(later.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert!(!f.workspace.join("must-not-run.txt").exists());
}
#[tokio::test]
async fn plan_and_unapproved_calls_do_not_spawn_or_forge_completion() {
    let f = Fixture::new();
    let command = if cfg!(windows) {
        "Set-Content denied.txt nope"
    } else {
        "printf nope > denied.txt"
    };
    for (selectors, kind) in [
        (Selectors::default(), ToolErrorKind::NeedsApproval),
        (
            Selectors {
                mode: Mode::Plan,
                ..allow()
            },
            ToolErrorKind::DeniedCapability,
        ),
    ] {
        let (runner, gate) = f.runner("synthetic-key", json!({}), selectors).await;
        let outcome = runner.run_outcome(command, None).await;
        assert!(outcome.is_error);
        assert_eq!(outcome.meta.error_kind, Some(kind));
        assert_eq!(outcome.meta.bytes, None);
        assert!(!gate.approval_required());
        assert!(!f.workspace.join("denied.txt").exists());
    }
}
#[tokio::test]
async fn unsafe_post_capture_cleaning_is_rejected_before_any_process_effect() {
    let f = Fixture::new();
    let (runner, _) = f
        .runner("synthetic-key", json!({"clean_output":true}), allow())
        .await;
    let command = if cfg!(windows) {
        "Set-Content denied.txt nope"
    } else {
        "printf nope > denied.txt"
    };
    let outcome = runner.run_outcome(command, None).await;
    assert!(outcome.is_error);
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::InvalidArgs));
    assert_eq!(outcome.content, "[REDACTED]");
    assert!(!f.workspace.join("denied.txt").exists());
    assert_eq!(outcome.meta.bytes, None);
}
#[tokio::test]
async fn scalar_credential_in_exit_text_is_redacted_without_changing_exit_status() {
    let f = Fixture::new();
    let (runner, _) = f.runner("7", json!({}), allow()).await;
    let outcome = runner.run_outcome(&print("", "", 7), None).await;
    assert!(!outcome.is_error && !outcome.content.contains('7'));
    assert_eq!(outcome.meta.exit_code, Some(7));
    assert_eq!(outcome.meta.bytes, Some(0));
}
#[tokio::test]
async fn invalid_pipe_utf8_stops_as_a_typed_output_failure_not_the_later_deadline() {
    let f = Fixture::new();
    let (runner, _) = f
        .runner(
            "synthetic-key",
            json!({"timeout_secs":15,"output_format":"head_tail"}),
            allow(),
        )
        .await;
    let command = if cfg!(windows) {
        "$s=[Console]::OpenStandardOutput(); $s.WriteByte(255); $s.Flush(); Start-Sleep -Seconds 30"
    } else {
        "printf '\\377'; sleep 30"
    };
    let outcome = runner.run_outcome(command, None).await;
    assert!(outcome.is_error);
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Failed));
    assert!(outcome.meta.bytes.unwrap() >= 1);
    assert_eq!(outcome.content, "[REDACTED]");
}
#[tokio::test]
async fn failed_spawn_uses_fixed_redacted_text_not_os_paths_or_arguments() {
    let f = Fixture::new();
    let (runner, _) = f.runner("synthetic-key", json!({}), allow()).await;
    std::fs::remove_dir(&f.workspace).unwrap();
    let outcome = runner.run_outcome("must-not-be-echoed-command", None).await;
    assert!(outcome.is_error);
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Spawn));
    assert_eq!(outcome.content, "[REDACTED]");
    assert_eq!(outcome.meta.bytes, None);
    assert_eq!(outcome.meta.exit_code, None);
}
#[tokio::test]
async fn frozen_shell_seeds_do_not_reopen_modified_configuration() {
    let f = Fixture::new();
    let (runner, _) = f.runner("original-secret", json!({}), allow()).await;
    std::fs::write(
        f.config.join("config.json"),
        json!({"model":"synthetic","api_key":"replacement-secret"}).to_string(),
    )
    .unwrap();
    let outcome = runner
        .run_outcome(&print("original-secret replacement-secret", "", 0), None)
        .await;
    assert!(!outcome.is_error);
    assert_eq!(outcome.content, "[REDACTED] replacement-secret");
}

#[tokio::test]
async fn empty_private_shell_command_has_fixed_typed_metadata_without_a_spawn() {
    let f = Fixture::new();
    let (runner, gate) = f.runner("command", json!({}), allow()).await;
    assert!(runner.has_headless_scope());
    let outcome = runner.run_outcome("", None).await;
    assert!(outcome.is_error);
    assert_eq!(outcome.content, "[REDACTED]");
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::InvalidArgs));
    assert_eq!(outcome.meta.bytes, None);
    assert_eq!(outcome.meta.exit_code, None);
    assert!(outcome.meta.duration.is_some());
    assert!(!gate.approval_required());
    assert_eq!(std::fs::read_dir(&f.workspace).unwrap().count(), 0);
}

#[tokio::test]
async fn frontend_empty_shell_command_preserves_its_legacy_validation_text() {
    for shell in [
        None,
        Some(Arc::new(ShellRunner::new(
            PathBuf::from("."),
            crate::shell::ShellSettings::default(),
        ))),
    ] {
        let mut state = AgentState::default();
        let backend: Arc<dyn Backend> = Arc::new(craftsman_harness::StateFs::new(Arc::new(
            Mutex::new(AgentState::default()),
        )));
        let tool = HostTool {
            kind: HostKind::Execute,
            backend: backend.clone(),
            lsp: None,
            workspace_checkpoints: None,
            shell,
        };
        let (notifications, _receiver) = craftsman_harness::notification_channel();
        let mut ctx = ToolCtx {
            state: &mut state,
            backend,
            registry: Arc::new(AgentRegistry::default()),
            notifications,
        };
        let outcome = tool.call(json!({"command":""}), &mut ctx).await.unwrap();
        assert!(outcome.is_error);
        assert_eq!(outcome.content, "Error: empty command");
        assert_eq!(outcome.meta.error_kind, None);
        assert_eq!(outcome.meta.bytes, None);
        assert_eq!(outcome.meta.exit_code, None);
    }
}

#[tokio::test]
async fn test_only_launch_observations_come_from_the_actual_builder_without_source_text() {
    let f = Fixture::new();
    let key = "synthetic-private-observation-key";
    let (runner, _) = f
        .runner(key, json!({"output_format":"head_tail"}), allow())
        .await;
    let view = runner.observation_view_for_tests().unwrap();
    let outcome = runner.run_outcome(&print(key, "safe-error", 0), None).await;
    assert!(!outcome.is_error, "{}", outcome.content);
    let report = view.report();
    assert!(report.contains("calls=1"), "{report}");
    assert!(report.contains("launch_return_us=Some("), "{report}");
    assert!(report.contains("pid=Some("), "{report}");
    assert!(report.contains("stdout_first_poll_us=Some("), "{report}");
    assert!(report.contains("stderr_first_poll_us=Some("), "{report}");
    assert!(report.contains("scope_end_us=Some("), "{report}");
    assert!(!report.contains(key));
    assert!(!report.contains(f.workspace.to_str().unwrap()));
    assert!(!report.contains("safe-error"));
}

#[tokio::test]
async fn test_only_denied_call_observations_do_not_fabricate_a_launch_or_pipe_poll() {
    let f = Fixture::new();
    let (runner, _) = f
        .runner(
            "synthetic-key",
            json!({}),
            Selectors {
                mode: Mode::Plan,
                ..allow()
            },
        )
        .await;
    let view = runner.observation_view_for_tests().unwrap();
    let outcome = runner.run_outcome("must-not-execute", None).await;
    assert!(outcome.is_error);
    let report = view.report();
    assert!(report.contains("calls=1"), "{report}");
    assert!(report.contains("launch_enter_us=None"), "{report}");
    assert!(report.contains("launch_return_us=None"), "{report}");
    assert!(report.contains("pid=None"), "{report}");
    assert!(report.contains("stdout_first_poll_us=None"), "{report}");
    assert!(report.contains("stderr_first_poll_us=None"), "{report}");
    assert!(!report.contains("must-not-execute"));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_task_snapshot_uses_the_active_actual_builder_child_and_not_a_released_pid() {
    let f = Fixture::new();
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let key = "synthetic-native-snapshot-key";
    let (runner, gate) = f
        .runner(
            key,
            json!({"output_format":"head_tail","timeout_secs":30}),
            selectors,
        )
        .await;
    let view = runner.observation_view_for_tests().unwrap();
    let task = tokio::spawn({
        let runner = runner.clone();
        async move {
            runner
                .run_outcome(&delayed("synthetic-native-output", true), None)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(20), async {
        while !f.workspace.join("ready.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual native snapshot fixture must write its real ready marker");
    let report = view.failure_report();
    assert!(report.contains("native_process=owned-child"), "{report}");
    assert!(report.contains("native_task=observed"), "{report}");
    assert!(
        report.contains("native_threads=observed count=1"),
        "{report}"
    );
    assert!(report.contains("native_thread=0"), "{report}");
    assert!(report.contains("scope_end_us=None"), "{report}");
    assert!(!report.contains(key));
    assert!(!report.contains(f.workspace.to_str().unwrap()));
    assert!(!report.contains("synthetic-native-output"));
    gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    let outcome = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert!(outcome.meta.bytes.unwrap() >= 23);
    let released = view.failure_report();
    assert!(released.contains("scope_end_us=Some("), "{released}");
    assert!(!released.contains("native_process="), "{released}");
    assert!(!released.contains("native_task="), "{released}");
    assert!(!released.contains("native_thread="), "{released}");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_execution_observations_match_the_actual_retained_workspace_without_paths() {
    native_execution_observation_fixture(false).await;
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_execution_observations_do_not_rebind_expected_identity_to_a_reused_path() {
    native_execution_observation_fixture(true).await;
}
#[cfg(target_os = "macos")]
async fn native_execution_observation_fixture(rebind: bool) {
    let f = Fixture::new();
    let key = "synthetic-execution-observation-key";
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let (runner, gate) = f
        .runner(
            key,
            json!({"output_format":"head_tail","timeout_secs":30}),
            selectors,
        )
        .await;
    let view = runner.observation_view_for_tests().unwrap();
    if rebind {
        // OWNED fixture namespace only: original metadata must come from the
        // retained invocation handle, not a later ambient stat of this name.
        std::fs::rename(&f.workspace, f._root.path().join("original-workspace")).unwrap();
        std::fs::create_dir(&f.workspace).unwrap();
    }
    let task = tokio::spawn({
        let runner = runner.clone();
        async move {
            runner
                .run_outcome(&delayed("synthetic-native-output", true), None)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(20), async {
        while !f.workspace.join("ready.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("execution observation fixture requires a genuine ready child");
    let report = view.failure_report();
    assert!(
        report.contains("native_execution=observed executable_class="),
        "{report}"
    );
    let expected = if rebind {
        "native_cwd=different-from-invocation-workspace"
    } else {
        "native_cwd=matches-invocation-workspace"
    };
    assert!(report.contains(expected), "{report}");
    assert!(!report.contains(key));
    assert!(!report.contains("synthetic-native-output"));
    assert!(!report.contains(f.workspace.to_str().unwrap()));
    assert!(!report.contains("/bin/"));
    assert!(!report.contains("original-workspace"));
    gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    let outcome = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert!(outcome.meta.bytes.unwrap() >= 23);
    assert!(view.failure_report().contains("scope_end_us=Some("));
    assert!(!view.failure_report().contains("native_execution="));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn pre_kill_timeout_snapshot_survives_the_actual_owned_child_release_without_pid_lookup() {
    let f = Fixture::new();
    let (runner, _) = f
        .runner(
            "synthetic-pre-stop-key",
            json!({"output_format":"head_tail","timeout_secs":1}),
            allow(),
        )
        .await;
    let view = runner.observation_view_for_tests().unwrap();
    let outcome = runner.run_outcome("sleep 30", None).await;
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Timeout));
    assert_eq!(outcome.meta.bytes, Some(0));
    assert!(!view.failure_report().contains("native_process="));
    let report = view.teardown_report();
    assert!(report.contains("before_stop_decision=timeout"), "{report}");
    assert!(
        report.contains("before_stop_tree_pid_matches_recorded=true"),
        "{report}"
    );
    assert!(
        report.contains("native_snapshot_phase=before-stop native_snapshot_query_count=1"),
        "{report}"
    );
    assert!(report.contains("native_process=owned-child"), "{report}");
    assert!(
        report.contains("before_stop_stdout_bytes=0 before_stop_stdout_eof=false"),
        "{report}"
    );
    assert!(report.contains("scope_end_us=Some("), "{report}");
    assert_eq!(report, view.teardown_report());
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn pre_kill_approval_snapshot_keeps_real_bytes_and_actual_before_stop_kind() {
    pre_kill_approval_snapshot_fixture(false).await;
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn active_view_snapshot_is_not_queried_again_or_relabelled_fresh_before_stop() {
    pre_kill_approval_snapshot_fixture(true).await;
}
#[cfg(target_os = "macos")]
async fn pre_kill_approval_snapshot_fixture(prior_view: bool) {
    let f = Fixture::new();
    let mut selectors = allow();
    selectors.on_unapproved = OnUnapproved::Fail;
    let (runner, gate) = f
        .runner(
            "synthetic-pre-stop-key",
            json!({"output_format":"head_tail","timeout_secs":30}),
            selectors,
        )
        .await;
    let view = runner.observation_view_for_tests().unwrap();
    let task = tokio::spawn({
        let runner = runner.clone();
        async move {
            runner
                .run_outcome(&delayed("synthetic-native-output", true), None)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(20), async {
        while !f.workspace.join("ready.txt").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pre-stop fixture requires a real ready child");
    if prior_view {
        assert!(view.failure_report().contains("native_process=owned-child"));
    }
    gate.observe_error_kind(ToolErrorKind::NeedsApproval);
    let outcome = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::NeedsApproval));
    assert!(outcome.meta.bytes.unwrap() >= 23);
    assert!(!view.failure_report().contains("native_process="));
    let report = view.teardown_report();
    assert!(
        report.contains("before_stop_decision=needs-approval"),
        "{report}"
    );
    assert!(
        report.contains("before_stop_tree_pid_matches_recorded=true"),
        "{report}"
    );
    let phase = if prior_view {
        "native_snapshot_phase=active-view"
    } else {
        "native_snapshot_phase=before-stop"
    };
    assert!(report.contains(phase), "{report}");
    assert!(report.contains("native_snapshot_query_count=1"), "{report}");
    assert!(!report.contains("synthetic-pre-stop-key"));
    assert!(!report.contains("synthetic-native-output"));
    assert!(!report.contains(f.workspace.to_str().unwrap()));
    assert_eq!(report, view.teardown_report());
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn successful_normal_shell_does_not_invent_a_stop_or_native_snapshot() {
    let f = Fixture::new();
    let (runner, _) = f
        .runner(
            "synthetic-pre-stop-key",
            json!({"output_format":"head_tail"}),
            allow(),
        )
        .await;
    let view = runner.observation_view_for_tests().unwrap();
    let outcome = runner.run_outcome(&print("safe", "", 0), None).await;
    assert!(!outcome.is_error);
    assert_eq!(outcome.meta.bytes, Some(4));
    let report = view.teardown_report();
    assert!(report.contains("scope_end_us=Some("));
    assert!(!report.contains("before_stop_"));
    assert!(!report.contains("native_snapshot"));
    assert!(!report.contains("native_process="));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_scheduling_projection_uses_saved_actual_pre_kill_child_facts_without_requery() {
    let f = Fixture::new();
    let (runner, _) = f
        .runner(
            "synthetic-scheduling-key",
            json!({"output_format":"head_tail", "timeout_secs":1}),
            allow(),
        )
        .await;
    let view = runner.observation_view_for_tests().unwrap();
    let outcome = runner.run_outcome("sleep 30", None).await;
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Timeout));
    assert_eq!(outcome.meta.bytes, Some(0));
    assert!(!view.failure_report().contains("native_scheduling="));
    let report = view.teardown_report();
    assert!(report.contains("scope_end_us=Some("), "{report}");
    assert!(
        report.contains("native_snapshot_phase=before-stop native_snapshot_query_count=1"),
        "{report}"
    );
    assert!(
        report.contains("native_scheduling=observed native_scheduling_source=initial-task-info"),
        "{report}"
    );
    for key in [
        "nice_native=",
        "task_thread_policy_native=",
        "task_priority_native=",
        "native_thread_scheduling=0",
        "current_priority_native=",
        "priority_native=",
        "max_priority_native=",
    ] {
        assert!(report.contains(key), "{report}");
    }
    let flags: u32 = report
        .split_whitespace()
        .find_map(|v| v.strip_prefix("kernel_flags="))
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        report.contains(&format!("proc_exec_flag={}", flags & 0x4000 != 0)),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "proc_unclassified_bits_native={}",
            flags & !0x7fff
        )),
        "{report}"
    );
    assert!(!report.contains("synthetic-scheduling-key"));
    assert!(!report.contains(f.workspace.to_str().unwrap()));
    assert_eq!(report, view.teardown_report());
}

// Additive original-poll/selection diagnostic cases; no changed older fixture.
fn original_progress_counter(line: &str, field: &str) -> u64 {
    let marker = format!("{field}=Some(");
    let rest = line.split_once(&marker).unwrap().1;
    rest.split_once(')').unwrap().0.parse().unwrap()
}

fn progress_print(value: &str, error: &str, code: i32) -> String {
    if cfg!(windows) {
        // This observer fixture needs actual pipe EOF and a native exit status,
        // without making managed PowerShell teardown part of its observation.
        // Keep the original shell/Job owner and bounds; flush actual bytes first.
        format!(
            "[Console]::Out.Write('{value}'); [Console]::Error.Write('{error}'); [Console]::Out.Flush(); [Console]::Error.Flush(); [Environment]::Exit({code})"
        )
    } else {
        print(value, error, code)
    }
}

#[tokio::test]
async fn actual_scoped_shell_poll_progress_retains_reader_return_then_wait_and_original_output() {
    let f = Fixture::new();
    let key = "synthetic-progress-key";
    for format in ["legacy", "head_tail"] {
        let (runner, _) = f
            .runner(
                key,
                json!({"output_format":format,"timeout_secs":15}),
                allow(),
            )
            .await;
        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            runner.run_outcome(&progress_print(key, "", 7), None),
        )
        .await
        .expect("bounded new original-poll fixture");
        assert!(!outcome.is_error, "{}", outcome.content);
        assert_eq!(outcome.content, "[REDACTED]\n[exit code 7]");
        assert_eq!(outcome.meta.bytes, Some(key.len() as u64));
        assert_eq!(outcome.meta.exit_code, Some(7));
        let view = runner.observation_view_for_tests().unwrap();
        let progress = view.poll_progress_report_for_tests();
        assert!(
            progress.contains("calls=1 omitted=0 recorder_loss=false clock=call"),
            "{progress}"
        );
        assert!(progress.contains("readers_result=returned-ok readers_phase_loss=false"));
        assert!(progress.contains("wait_result=returned-status wait_phase_loss=false"));
        assert!(!progress.contains("before-stop-history") && !progress.contains(key));
        let selection = progress
            .lines()
            .find(|line| line.contains("readers_enter_us="))
            .unwrap();
        assert!(
            original_progress_counter(selection, "readers_enter_us")
                <= original_progress_counter(selection, "readers_return_us")
        );
        assert!(
            original_progress_counter(selection, "readers_return_us")
                <= original_progress_counter(selection, "wait_enter_us")
        );
        assert!(
            original_progress_counter(selection, "wait_enter_us")
                <= original_progress_counter(selection, "wait_return_us")
        );
        let stdout = progress
            .lines()
            .find(|line| line.contains("stdout_polls="))
            .unwrap();
        assert!(original_progress_counter(stdout, "stdout_polls") >= 2);
        assert!(stdout.contains("stdout_last_poll_result=eof stdout_count_loss=false"));
        let stderr = progress
            .lines()
            .find(|line| line.contains("stderr_polls="))
            .unwrap();
        assert!(original_progress_counter(stderr, "stderr_polls") >= 1);
        assert!(stderr.contains("stderr_last_poll_result=eof stderr_count_loss=false"));
        assert!(view.report().contains("scope_end_us=Some("));
    }
}

#[cfg(windows)]
#[tokio::test]
async fn explicit_progress_exit_keeps_real_stream_bytes_status_and_reader_wait_phases() {
    let f = Fixture::new();
    let key = "synthetic-native-progress-key";
    let error = "actual-native-progress-error";
    for format in ["legacy", "head_tail"] {
        let (runner, _) = f
            .runner(
                key,
                json!({"output_format":format,"timeout_secs":15}),
                allow(),
            )
            .await;
        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            runner.run_outcome(&progress_print(key, error, 11), None),
        )
        .await
        .expect("bounded native-exit progress fixture");
        assert!(!outcome.is_error, "{}", outcome.content);
        assert!(outcome.content.contains("[REDACTED]"));
        assert!(outcome.content.contains(error));
        assert!(outcome.content.contains("[exit code 11]"));
        assert_eq!(outcome.meta.bytes, Some((key.len() + error.len()) as u64));
        assert_eq!(outcome.meta.exit_code, Some(11));
        let report = runner
            .observation_view_for_tests()
            .unwrap()
            .poll_progress_report_for_tests();
        assert!(
            report.contains("readers_result=returned-ok readers_phase_loss=false"),
            "{report}"
        );
        assert!(
            report.contains("wait_result=returned-status wait_phase_loss=false"),
            "{report}"
        );
        assert!(!report.contains(key));
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn actual_scoped_timeout_poll_progress_keeps_pre_drain_history_separate_without_new_pid_queries()
 {
    let f = Fixture::new();
    for format in ["legacy", "head_tail"] {
        let (runner, _) = f
            .runner(
                "synthetic-progress-timeout-key",
                json!({"output_format":format,"timeout_secs":1}),
                allow(),
            )
            .await;
        // This requires genuine zero-byte timeout facts, NOT startup/readiness.
        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            runner.run_outcome("sleep 30", None),
        )
        .await
        .expect("bounded new original-poll timeout fixture");
        assert!(outcome.is_error);
        assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Timeout));
        assert_eq!(outcome.meta.bytes, Some(0));
        assert_eq!(outcome.meta.exit_code, None);
        let view = runner.observation_view_for_tests().unwrap();
        let progress = view.poll_progress_report_for_tests();
        let before = progress
            .lines()
            .find(|line| {
                line.contains("poll_progress_phase=before-stop-history")
                    && line.contains("stdout_polls=")
            })
            .unwrap();
        let before_polls = original_progress_counter(before, "stdout_polls");
        assert!(before_polls >= 1);
        assert_eq!(
            before_polls,
            original_progress_counter(before, "stdout_pending_polls")
        );
        assert!(before.contains("stdout_last_poll_result=pending stdout_count_loss=false"));
        let current = progress
            .lines()
            .find(|line| {
                line.contains("poll_progress_phase=current-history")
                    && line.contains("stdout_polls=")
            })
            .unwrap();
        assert!(original_progress_counter(current, "stdout_polls") >= before_polls);
        if format == "head_tail" {
            assert!(current.contains("stdout_last_poll_result=eof stdout_count_loss=false"));
        } else {
            assert!(current.contains("stdout_last_poll_result=pending stdout_count_loss=false"));
        }
        assert!(progress.contains("readers_result=timeout-branch readers_phase_loss=false"));
        assert!(progress.contains(
            "wait_enter_us=None wait_return_us=None wait_result=not-returned wait_phase_loss=false"
        ));
        let saved = view.teardown_report();
        assert!(saved.contains("before_stop_decision=timeout"));
        assert!(saved.contains("native_snapshot_phase=before-stop native_snapshot_query_count=1 native_snapshot_clock=call"));
        assert_eq!(view.poll_progress_report_for_tests(), progress);
        assert_eq!(view.teardown_report(), saved);
        assert!(!view.failure_report().contains("native_process="));
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn actual_scoped_zero_byte_timeout_keeps_current_child_pipe_metadata_on_same_saved_ticket() {
    let f = Fixture::new();
    let (runner, _) = f
        .runner(
            "synthetic-current-pipe-key",
            json!({"output_format":"head_tail","timeout_secs":1}),
            allow(),
        )
        .await;
    let view = runner.observation_view_for_tests().unwrap();
    // A genuine zero-output timeout, NOT a readiness/emission/startup proof.
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        runner.run_outcome("sleep 30", None),
    )
    .await
    .expect("one actual scoped call within the original outer bound");
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Timeout));
    assert_eq!(outcome.meta.bytes, Some(0));
    assert_eq!(outcome.meta.exit_code, None);
    assert!(outcome.meta.duration.unwrap() >= Duration::from_secs(1));
    let report = view.teardown_report();
    assert!(report.contains("before_stop_decision=timeout"), "{report}");
    assert!(
        report.contains("native_snapshot_phase=before-stop native_snapshot_query_count=1"),
        "{report}"
    );
    assert!(
        report.contains("native_child_pipe=stdout current_child_fd=1 buffer_bytes_native=0"),
        "{report}"
    );
    assert!(
        report.contains("native_child_pipe=stderr current_child_fd=2 buffer_bytes_native=0"),
        "{report}"
    );
    assert!(report.contains("scope_end_us=Some("), "{report}");
    assert!(
        !report.contains("synthetic-current-pipe-key")
            && !report.contains(f.workspace.to_str().unwrap())
    );
    assert_eq!(report, view.teardown_report()); // cached history, no new PID/FD query
}
