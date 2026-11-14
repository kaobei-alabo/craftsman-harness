//! Marked real builder/HTTP/subprocess/approval/SQLite verification proof.
use super::*;
use craftsman_harness::Checkpointer;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HELPER: &str = "agent::verification_tests::native_verification_fixture";
const MARKER: &str = "craftsman-verification=";
const COMMAND_HELPER: &str = "agent::verification_tests::native_verification_command";
const COMMAND_MARKER: &str = "craftsman-verification-command=";
const DONE: &str = "actual completion verification, protected review, subprocess, HTTP and SQLite assertions completed";

#[test]
#[ignore = "native verification command fixture; marker required before path access"]
fn native_verification_command() {
    let modes: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(COMMAND_MARKER))
                .map(str::to_owned)
        })
        .collect();
    if modes.len() != 1 || !matches!(modes[0].as_str(), "check" | "mutate" | "wait") {
        return;
    }
    if modes[0] == "mutate" {
        std::fs::write("src/value.txt", "mutated by check").unwrap();
        println!("actual check changed sources");
        return;
    }
    if modes[0] == "wait" {
        std::fs::write("check-ready", "ready").unwrap();
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    let value = std::fs::read_to_string("src/value.txt").unwrap();
    if value != "fixed" {
        println!("actual check failed: value remains {value}");
        use std::io::Write;
        std::io::stdout().flush().unwrap();
        std::io::stderr().flush().unwrap();
        std::process::exit(7);
    }
    println!("actual check passed: fixed value");
}

pub(super) fn command(mode: &str) -> String {
    let binary = std::env::current_exe().unwrap();
    let binary = binary.to_str().unwrap();
    #[cfg(windows)]
    {
        format!(
            "& '{}' '--ignored' '--exact' '{COMMAND_HELPER}' '--nocapture' '{COMMAND_MARKER}{mode}'; $craftsman_check_exit = $LASTEXITCODE; [Console]::Out.Flush(); [Console]::Error.Flush(); exit $craftsman_check_exit",
            binary.replace("'", "''")
        )
    }
    #[cfg(not(windows))]
    {
        format!(
            "'{}' --ignored --exact {COMMAND_HELPER} --nocapture {COMMAND_MARKER}{mode}",
            binary.replace('\'', "'\\''")
        )
    }
}

fn streamed(delta: Value, tool: bool) -> ResponseTemplate {
    let value = json!({"choices":[{"delta":delta,"finish_reason":if tool{"tool_calls"}else{"stop"}}],"usage":{"prompt_tokens":3,"completion_tokens":2}});
    ResponseTemplate::new(200).set_body_raw(
        format!("data: {value}\n\ndata: [DONE]\n\n"),
        "text/event-stream",
    )
}
fn edit(file: &str, old: &str, new: &str, id: usize) -> Value {
    json!({"tool_calls":[{"index":0,"id":format!("model-{id}"),"type":"function","function":{"name":"edit_file","arguments":json!({"file_path":file,"old_string":old,"new_string":new}).to_string()}}]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "actual verification fixture; unmarked/unknown startup returns before config access"]
async fn native_verification_fixture() {
    let modes: Vec<_> = std::env::args_os()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    if modes.len() != 1
        || !matches!(
            modes[0].as_str(),
            "repair"
                | "protected-deny"
                | "protected-allow"
                | "stale"
                | "mutating-check"
                | "save-fail"
                | "readonly"
                | "legacy"
                | "cancel"
                | "restore"
                | "limit"
                | "project"
                | "policy-stale"
                | "project-bypass"
                | "invalid"
                | "command-status"
                | "continue-failed"
        )
    {
        return;
    }
    let mode = modes[0].clone();
    let root = std::env::current_dir().unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::create_dir(root.join("tests")).unwrap();
    let value = if matches!(
        mode.as_str(),
        "protected-deny" | "protected-allow" | "stale" | "mutating-check" | "cancel"
    ) {
        "fixed"
    } else {
        "broken"
    };
    std::fs::write(root.join("src/value.txt"), value).unwrap();
    std::fs::write(root.join("tests/public.txt"), "test original").unwrap();
    if mode == "command-status" {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", COMMAND_HELPER, "--nocapture"])
            .arg(format!("{COMMAND_MARKER}check"))
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(7),
            "direct marked helper status: {:?}; stdout: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("actual check failed"));
        #[cfg(windows)]
        {
            let output = tokio::process::Command::new("powershell.exe")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &command("check"),
                ])
                .current_dir(&root)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output()
                .await
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(7),
                "direct PowerShell status: {:?}; stdout: {}; stderr: {}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("actual check failed"),
                "direct PowerShell failure output: stdout: {}; stderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let shell = crate::tools::ShellRunner::new(root, crate::shell::ShellSettings::default());
        let result = shell.run_outcome(&command("check"), None).await;
        assert_eq!(
            result.meta.exit_code,
            Some(7),
            "actual shell result: {}",
            result.content
        );
        assert!(
            result.content.contains("actual check failed"),
            "actual shell result: {}",
            result.content
        );
        eprintln!("{DONE}");
        return;
    }
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let scenario = if mode == "continue-failed" {
        String::from("limit")
    } else if mode == "restore" || mode == "project" || mode == "project-bypass" {
        String::from("repair")
    } else {
        mode.clone()
    };
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let index = observed.fetch_add(1, Ordering::SeqCst);
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            if index == 1 && matches!(scenario.as_str(), "repair" | "limit") {
                assert_eq!(
                    std::fs::read_to_string("src/value.txt").unwrap(),
                    "still broken",
                    "first actual edit result: {}",
                    body["messages"]
                );
            }
            if scenario == "readonly" {
                return streamed(json!({"content":"read-only answer"}), false);
            }
            if index == 0 {
                return if matches!(
                    scenario.as_str(),
                    "protected-deny" | "protected-allow" | "stale"
                ) {
                    streamed(
                        edit("tests/public.txt", "test original", "test modified", index),
                        true,
                    )
                } else {
                    streamed(
                        edit(
                            "src/value.txt",
                            if scenario == "mutating-check" || scenario == "cancel" {
                                "fixed"
                            } else {
                                "broken"
                            },
                            if scenario == "mutating-check" || scenario == "cancel" {
                                "fixed before verification"
                            } else {
                                "still broken"
                            },
                            index,
                        ),
                        true,
                    )
                };
            }
            if scenario == "repair" && index == 2 {
                let messages = body["messages"].as_array().unwrap();
                assert!(
                    messages.iter().any(|message| {
                        message["role"] == "tool"
                            && message["content"]
                                .as_str()
                                .is_some_and(|text| text.contains("actual check failed"))
                    }),
                    "actual verification tool results before repair: {}",
                    body["messages"]
                );
                return streamed(edit("src/value.txt", "still broken", "fixed", index), true);
            }
            if scenario == "repair" && (index == 4 || index == 6)
                || scenario == "protected-allow" && index == 2
            {
                assert!(body["messages"].as_array().unwrap().iter().any(|message| {
                    message["role"] == "tool"
                        && message["content"]
                            .as_str()
                            .is_some_and(|text| text.contains("Verification passed."))
                }));
                return streamed(json!({"content":"verified final answer"}), false);
            }
            streamed(json!({"content":"premature answer"}), false)
        })
        .mount(&server)
        .await;
    let mut cfg:Config=serde_json::from_value(json!({"base_url":server.uri(),"api_key":"fixture-key","model":"fixture-model","harness_profile":"general","mcp_servers":[],"lsp_servers":{},"voice":{"enabled":false},"gadgets":{"enabled":false}})).unwrap();
    let selected_command = command(if mode == "mutating-check" {
        "mutate"
    } else if mode == "cancel" {
        "wait"
    } else {
        "check"
    });
    cfg.extra.insert("verification".into(),json!({"commands":[selected_command],"paths":["src","tests"],"protected_paths":["tests"],"max_repair_rounds":if matches!(mode.as_str(),"mutating-check"|"continue-failed"){0}else{3}}));
    if mode == "project" || mode == "policy-stale" || mode == "project-bypass" {
        std::fs::create_dir(root.join(".craftsman")).unwrap();
        let policy = cfg.extra.remove("verification").unwrap();
        std::fs::write(
            root.join(".craftsman/verification.json"),
            serde_json::to_vec(&policy).unwrap(),
        )
        .unwrap();
    }
    let approval = Arc::new(Mutex::new(ApprovalState::default()));
    if matches!(
        mode.as_str(),
        "protected-allow" | "stale" | "project-bypass"
    ) {
        approval.lock().unwrap().mode = crate::config::PermissionMode::BypassPermissions;
    }
    let prompts = Arc::new(AtomicUsize::new(0));
    let verification_prompts = Arc::new(AtomicUsize::new(0));
    let count = prompts.clone();
    let verification_count = verification_prompts.clone();
    let prompt_mode = mode.clone();
    let prompt_root = root.clone();
    if mode == "invalid" {
        cfg.extra.insert(
            "verification".into(),
            json!({"commands":["check"],"paths":["../outside"]}),
        );
    }
    let built = build_session_ext(
        &cfg,
        approval,
        "",
        false,
        false,
        None,
        SessionExtras {
            workspace_root: Some(root.clone()),
            verification: mode != "legacy",
            cancellation_session: Some(craftsman_harness::SessionCancellation::default()),
            interrupt_model_on_cancel: true,
            token_sink: Some(Arc::new(|_| {})),
            activity_sink: Some(Arc::new(|_| {})),
            approval_prompt: Some(Arc::new(move |tool, _, details| {
                count.fetch_add(1, Ordering::SeqCst);
                if tool == "verify" {
                    verification_count.fetch_add(1, Ordering::SeqCst);
                    if matches!(
                        prompt_mode.as_str(),
                        "protected-deny" | "protected-allow" | "stale"
                    ) {
                        assert!(
                            details.contains("test original") && details.contains("test modified")
                        );
                    }
                    if prompt_mode == "protected-deny" {
                        return ApprovalPromptResult::Reject("keep test changes unapproved".into());
                    }
                    if prompt_mode == "policy-stale" {
                        std::fs::write(prompt_root.join(".craftsman/verification.json"), "{}")
                            .unwrap();
                    }
                    if prompt_mode == "stale" {
                        std::fs::write(
                            prompt_root.join("tests/public.txt"),
                            "changed after review",
                        )
                        .unwrap();
                    }
                }
                ApprovalPromptResult::Approve
            })),
            ..SessionExtras::default()
        },
    )
    .await;
    if mode == "invalid" {
        assert!(built.is_none());
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        assert!(!crate::config::config_dir().join("threads.db").exists());
        eprintln!("{DONE}");
        return;
    }
    let mut session = built.unwrap();
    if mode == "save-fail" {
        session.checkpointer.as_ref().unwrap().connection().execute("CREATE TRIGGER refuse_completion_check BEFORE INSERT ON checkpoints WHEN instr(NEW.state_json, '_craftsman_completion_') > 0 BEGIN SELECT RAISE(ABORT, 'synthetic store failure'); END",()).await.unwrap();
    }
    let result = if mode == "cancel" {
        let agent = session.agent.clone();
        let task = tokio::spawn(async move { agent.run_turn("complete the fixture").await });
        tokio::time::timeout(Duration::from_secs(60), async {
            while !root.join("check-ready").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(session.agent.is_turn_active());
        assert!(session.agent.request_cancel());
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
    } else {
        session.agent.run_turn("complete the fixture").await
    };
    if matches!(
        mode.as_str(),
        "repair"
            | "protected-allow"
            | "readonly"
            | "legacy"
            | "cancel"
            | "restore"
            | "project"
            | "project-bypass"
    ) {
        let craftsman_harness::TurnResult::Done(output) = result.unwrap() else {
            panic!("unexpected suspension")
        };
        assert_eq!(
            output.final_text(),
            match mode.as_str() {
                "repair" | "protected-allow" | "restore" | "project" | "project-bypass" =>
                    "verified final answer",
                "readonly" => "read-only answer",
                "cancel" => "Stopped by the user.",
                _ => "premature answer",
            }
        );
    } else {
        let error = result.unwrap_err();
        assert!(matches!(error, AgentError::State(_)));
        if mode == "save-fail" {
            assert!(session.agent.checkpoint_failed());
            assert!(error.to_string().contains("Further work was stopped"));
        }
    }
    assert_eq!(
        requests.load(Ordering::SeqCst),
        match mode.as_str() {
            "repair" | "restore" | "project" | "limit" | "project-bypass" => 5,
            "protected-allow" => 3,
            "readonly" => 1,
            _ => 2,
        }
    );
    assert_eq!(
        verification_prompts.load(Ordering::SeqCst),
        match mode.as_str() {
            "repair" | "restore" | "project" => 2,
            "project-bypass" => 1,
            "limit" => 4,
            "legacy" | "readonly" | "save-fail" => 0,
            _ => 1,
        }
    );
    if mode == "protected-allow" || mode == "stale" {
        assert_eq!(
            prompts.load(Ordering::SeqCst),
            1,
            "protected changes must still ask under bypass mode"
        );
    }
    let state = session.agent.state.lock().unwrap().clone();
    if mode == "repair" || mode == "restore" || mode == "project" || mode == "project-bypass" {
        assert_eq!(
            std::fs::read_to_string(root.join("src/value.txt")).unwrap(),
            "fixed"
        );
        assert!(state.messages.iter().any(|message|matches!(message,craftsman_harness::Msg::Tool{content,..}if content.contains("actual check failed"))));
    }
    if mode == "legacy" {
        assert!(state.messages.iter().all(|message|!matches!(message,craftsman_harness::Msg::Tool{tool_call_id,..}if tool_call_id.starts_with("_craftsman_completion_"))));
    }
    let store = session.checkpointer.as_ref().unwrap();
    let (_, saved, _) = store
        .load_latest(&session.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
        mode == "save-fail"
    );
    if mode == "limit" {
        assert_eq!(state.messages.iter().filter(|message|matches!(message,craftsman_harness::Msg::Tool{content,..}if content.contains("actual check failed"))).count(),4);
    }
    if mode == "continue-failed" {
        let before = session.agent.state.lock().unwrap().clone();
        assert!(before.messages.iter().any(|message|matches!(message,craftsman_harness::Msg::Tool{content,..}if content.contains("actual check failed"))));
        let error = session
            .agent
            .run_turn("Continue from the failed checks without changing files.")
            .await
            .unwrap_err();
        assert!(matches!(error, AgentError::State(_)));
        assert!(error.to_string().contains("Verification still failed"));
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        assert_eq!(verification_prompts.load(Ordering::SeqCst), 2);
        assert_eq!(
            std::fs::read_to_string(root.join("src/value.txt")).unwrap(),
            "still broken"
        );
        let after = session.agent.state.lock().unwrap().clone();
        assert!(after.messages.starts_with(&before.messages));
        assert_eq!(after.messages.iter().filter(|message|matches!(message,craftsman_harness::Msg::Tool{content,..}if content.contains("actual check failed"))).count(),2);
        assert_eq!(after.messages.iter().filter(|message|matches!(message,craftsman_harness::Msg::Tool{tool_call_id,..}if tool_call_id=="model-0")).count(),1);
        let (_, saved, _) = store
            .load_latest(&session.thread_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.messages, after.messages);
        assert_eq!(
            saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
            false
        );
        // Exercise both app transfer call sites' bookkeeping: the TUI keeps
        // its captured thread, while ACP explicitly assigns the existing id.
        for explicit_id in [false, true] {
            let previous = session.agent.state.lock().unwrap().clone();
            let before_requests = requests.load(Ordering::SeqCst);
            let before_prompts = verification_prompts.load(Ordering::SeqCst);
            let continued_prompts = verification_prompts.clone();
            let mut next = build_session_ext(
                &cfg,
                Arc::new(Mutex::new(ApprovalState::default())),
                "",
                false,
                false,
                Some(session.thread_id.clone()),
                SessionExtras {
                    workspace_root: Some(root.clone()),
                    verification: true,
                    cancellation_session: Some(craftsman_harness::SessionCancellation::default()),
                    interrupt_model_on_cancel: true,
                    token_sink: Some(Arc::new(|_| {})),
                    activity_sink: Some(Arc::new(|_| {})),
                    approval_prompt: Some(Arc::new(move |tool, _, _| {
                        assert_eq!(tool, "verify");
                        continued_prompts.fetch_add(1, Ordering::SeqCst);
                        ApprovalPromptResult::Approve
                    })),
                    ..SessionExtras::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(requests.load(Ordering::SeqCst), before_requests);
            session.agent.request_cancel();
            for task in session.registry.take_background_joins() {
                let _ = task.await;
            }
            session.lsp.shutdown().await;
            next.inherit_conversation(&session, explicit_id.then_some(session.thread_id.as_str()))
                .unwrap();
            assert_eq!(next.agent.state.lock().unwrap().messages, previous.messages);
            assert_eq!(*next.usage.lock().unwrap(), *session.usage.lock().unwrap());
            assert_eq!(requests.load(Ordering::SeqCst), before_requests);
            assert_eq!(verification_prompts.load(Ordering::SeqCst), before_prompts);
            assert!(
                next.verification
                    .as_ref()
                    .unwrap()
                    .status()
                    .contains("requires fresh checks")
            );
            let error = next
                .agent
                .run_turn("Continue after selecting a model.")
                .await
                .unwrap_err();
            assert!(matches!(error, AgentError::State(_)));
            assert!(error.to_string().contains("Verification still failed"));
            assert_eq!(requests.load(Ordering::SeqCst), before_requests + 1);
            assert_eq!(
                verification_prompts.load(Ordering::SeqCst),
                before_prompts + 1
            );
            let after = next.agent.state.lock().unwrap().clone();
            assert!(after.messages.starts_with(&previous.messages));
            assert_eq!(after.messages.iter().filter(|message|matches!(message,craftsman_harness::Msg::Tool{tool_call_id,..}if tool_call_id=="model-0")).count(),1);
            let (_, saved, _) = next
                .checkpointer
                .as_ref()
                .unwrap()
                .load_latest(&next.thread_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(saved.messages, after.messages);
            assert_eq!(
                saved.custom["_craftsman_durable_turn_v1"]["in_progress"],
                false
            );
            session = next;
        }
    }
    if mode == "restore" {
        let id = session.thread_id.clone();
        let before_requests = requests.load(Ordering::SeqCst);
        session.load_thread(&id).await.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), before_requests);
        let craftsman_harness::TurnResult::Done(output) = session
            .agent
            .run_turn("Continue from the restored conversation.")
            .await
            .unwrap()
        else {
            panic!("restored suspension");
        };
        assert_eq!(output.final_text(), "verified final answer");
        assert_eq!(requests.load(Ordering::SeqCst), 7);
        assert_eq!(verification_prompts.load(Ordering::SeqCst), 3);
    }
    session.lsp.shutdown().await;
    eprintln!("{DONE}");
}

async fn run_case(mode: &str) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let config = temp.path().join("config");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&config).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}{mode}"))
        .current_dir(&workspace)
        .env_clear()
        .env("HOME", temp.path())
        .env("USERPROFILE", temp.path())
        .env("CRAFTSMAN_CONFIG_DIR", &config)
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
        Ok(Ok(value)) => value,
        _ => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            panic!("owned verification fixture exceeded its bound");
        }
    };
    assert!(out.len() <= 65536 && err.len() <= 65536);
    let error = std::str::from_utf8(&err).unwrap();
    assert!(status.success(), "{error}");
    assert_eq!(error.lines().filter(|line| *line == DONE).count(), 1);
}

#[tokio::test]
async fn actual_failed_checks_feed_repairs_and_accept_only_current_source_results() {
    run_case("repair").await;
}
#[tokio::test]
async fn actual_protected_test_changes_require_review_before_checks_and_completion() {
    run_case("protected-deny").await;
}
#[tokio::test]
async fn actual_protected_review_remains_required_under_bypass_permissions() {
    run_case("protected-allow").await;
}
#[tokio::test]
async fn actual_protected_approval_does_not_cover_later_file_changes() {
    run_case("stale").await;
}
#[tokio::test]
async fn actual_checks_that_change_sources_do_not_create_a_passing_receipt() {
    run_case("mutating-check").await;
}
#[tokio::test]
async fn actual_completion_checkpoint_failure_prevents_check_dispatch() {
    run_case("save-fail").await;
}
#[tokio::test]
async fn actual_readonly_turn_runs_no_automatic_verification() {
    run_case("readonly").await;
}
#[tokio::test]
async fn actual_legacy_builder_ignores_verification_policy_without_opt_in() {
    run_case("legacy").await;
}
#[tokio::test]
async fn actual_owned_verification_cancel_keeps_results_and_stops_running_checks() {
    run_case("cancel").await;
}

#[tokio::test]
async fn actual_verification_repair_limit_stops_after_retained_failures() {
    run_case("limit").await;
}
#[tokio::test]
async fn actual_restored_verification_rechecks_without_load_time_replay() {
    run_case("restore").await;
}
#[tokio::test]
async fn actual_project_verification_policy_runs_only_declared_checks() {
    run_case("project").await;
}
#[tokio::test]
async fn actual_project_policy_changes_refuse_old_scheduled_checks() {
    run_case("policy-stale").await;
}

#[tokio::test]
async fn actual_project_command_approval_is_required_despite_bypass() {
    run_case("project-bypass").await;
}
#[tokio::test]
async fn actual_invalid_verification_policy_refuses_before_store_or_model_effects() {
    run_case("invalid").await;
}

#[tokio::test]
async fn actual_marked_check_command_preserves_native_failure_exit_and_output() {
    run_case("command-status").await;
}

#[tokio::test]
async fn actual_followup_cannot_accept_prior_unverified_changes_without_fresh_checks() {
    run_case("continue-failed").await;
}
