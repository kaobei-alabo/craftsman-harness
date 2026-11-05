use super::super::manifest::{Capabilities, Manifest};
use super::*;
use crate::exec::construction::{Options, Prepared, SessionSelection};
use crate::exec::permissions::{Mode, OnUnapproved, Selectors};
use crate::exec::runtime::Runtime;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    bundle: PathBuf,
}
impl Fixture {
    fn new(caps: Capabilities) -> Self {
        Self::with_input(caps, focused_fixture::inputs().original)
    }
    fn focused(caps: Capabilities) -> Self {
        Self::with_input(caps, &focused_fixture::inputs().focused)
    }
    fn with_input(caps: Capabilities, component: &[u8]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let config = workspace.join("selected-config");
        let bundle = config.join("gadgets/demo");
        std::fs::create_dir_all(&bundle).unwrap();
        write(&workspace, "safe.txt", b"safe bytes");
        write(&workspace, "Cargo.toml", b"ambient WASI must not read this");
        write(&config,"config.json",br#"{"model":"synthetic","api_key":"synthetic-key","gadgets":{"enabled":true},"auto_approve":true}"#);
        let manifest = Manifest {
            id: "demo".into(),
            version: "0.1.0".into(),
            description: "fixture".into(),
            capabilities: caps,
            prompt: None,
            commands: vec![],
            agents: vec![],
            migration: None,
        };
        write(
            &bundle,
            "gadget.toml",
            toml::to_string(&manifest).unwrap().as_bytes(),
        );
        write(&bundle, "gadget.wasm", component);
        Self {
            root,
            workspace,
            config,
            bundle,
        }
    }
    async fn prepared(&self, selectors: Selectors, trust: bool) -> Prepared {
        Prepared::prepare_with_environment_for_tests(
            Options {
                workspace: self.workspace.clone(),
                config_dir: self.config.clone(),
                trust_workspace: trust,
                session: SessionSelection::Ephemeral,
                selectors,
            },
            std::collections::BTreeMap::new(),
        )
        .await
        .unwrap()
    }
    async fn host(&self, selectors: Selectors) -> GuestHost {
        let p = self.prepared(selectors, false).await;
        let runtime = Runtime::build(p).await.unwrap();
        GuestHost::from_scope(
            runtime.guest_scope(),
            vec!["demo".into()],
            GuestLimits::default(),
        )
        .await
        .unwrap()
    }
    async fn state(&self, selectors: Selectors, phase: Phase) -> (Prepared, HeadlessState) {
        let p = self.prepared(selectors, false).await;
        let gate = Arc::new(InvocationGate::new(p.permissions().clone()));
        let sources = SourceAuthority::from_prepared(&p, vec!["demo".into()])
            .await
            .unwrap();
        let source = sources.authorize(SourceKind::User, "demo").await.unwrap();
        let state = HeadlessState::from_source(
            &source,
            gate,
            phase,
            Arc::new(CallBudget::new(Duration::from_secs(30))),
            256 * 1024 * 1024,
            http::client().unwrap(),
            p.credentials().redactor(),
        )
        .unwrap();
        (p, state)
    }
}
fn write(base: &Path, name: &str, bytes: &[u8]) {
    let path = base.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
fn caps(read: bool, write: bool, http: bool) -> Capabilities {
    Capabilities {
        read: if read {
            BTreeSet::from(["workspace".into()])
        } else {
            BTreeSet::new()
        },
        write: if write {
            BTreeSet::from(["workspace".into()])
        } else {
            BTreeSet::new()
        },
        http: if http {
            BTreeSet::from(["example.com".into()])
        } else {
            BTreeSet::new()
        },
    }
}
fn allow(names: &[&str]) -> Selectors {
    Selectors {
        allow: names.iter().map(|n| format!("tool:{n}")).collect(),
        ..Default::default()
    }
}
fn fail() -> Selectors {
    Selectors {
        on_unapproved: OnUnapproved::Fail,
        ..Default::default()
    }
}
fn kind<T>(result: Result<T, GadgetError>) -> ErrorKind {
    result.err().unwrap().kind
}
fn request(url: &str) -> Request {
    Request {
        method: "GET".into(),
        url: url.into(),
        headers: vec![],
        body: None,
    }
}
fn meta_error(state: &HeadlessState) -> ErrorKind {
    state.last_failure.unwrap()
}

#[tokio::test]
async fn owned_source_guest_reads_binary_and_creates_without_overwrite_or_ambient_writers() {
    let f = Fixture::focused(caps(true, true, false));
    let host = f.host(allow(&["write_file"])).await;
    let bytes = vec![0, 255, 13, 10, 195, 40];
    let created = host
        .call(
            SourceKind::User,
            "demo",
            "write_bytes",
            json!({"path":"new/binary.bin","bytes":bytes}),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&created.json).unwrap(),
        json!({"written":true})
    );
    let result = host
        .call(
            SourceKind::User,
            "demo",
            "read_bytes",
            json!({"path":"new/binary.bin"}),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.json).unwrap(),
        json!(bytes)
    );
    assert_eq!(
        std::fs::read(f.workspace.join("new/binary.bin")).unwrap(),
        bytes
    );
    assert_eq!(
        kind(
            host.call(
                SourceKind::User,
                "demo",
                "write_bytes",
                json!({"path":"new/binary.bin","bytes":[1]})
            )
            .await
        ),
        ErrorKind::DeniedCapability
    );
    assert_eq!(result.approval.id, "demo");
    assert_eq!(result.network_calls, 0);
    assert!(!f.config.join("gadgets/.grants.json").exists());
    assert!(!f.config.join("gadgets/.grants.lock").exists());
    assert!(!f.config.join("gadgets/.sdk").exists());
    assert!(!f.config.join("threads.db").exists());
    assert!(!f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn malicious_describe_sees_empty_read_write_http_even_after_full_source_approval() {
    let f = Fixture::new(caps(true, true, true));
    let host = f.host(allow(&["write_file", "web_fetch"])).await;
    let description = host.describe(SourceKind::User, "demo").await.unwrap();
    assert_eq!(description.functions.len(), 19);
    assert!(
        description
            .functions
            .iter()
            .all(|func| func.description == "read=false;write=false;http=false")
    );
    assert!(!f.workspace.join("describe-write.txt").exists());
    let (_, mut state) = f
        .state(allow(&["write_file", "web_fetch"]), Phase::Describe)
        .await;
    assert!(state.read_file("safe.txt".into()).await.is_err());
    assert!(state.list_dir(".".into()).await.is_err());
    assert!(
        state
            .write_file("describe-write.txt".into(), vec![0])
            .await
            .is_err()
    );
    assert!(
        state
            .http_request(request("https://example.com/"))
            .await
            .is_err()
    );
    assert_eq!(state.http.network_calls, 0);
    assert!(!state.gate.approval_required());
}

#[tokio::test]
async fn unapproved_source_does_not_compile_instantiate_or_describe_and_fail_seals() {
    let f = Fixture::new(caps(true, true, false));
    write(&f.bundle, "gadget.wasm", b"not a component");
    for policy in [OnUnapproved::Deny, OnUnapproved::Fail] {
        let p = f
            .prepared(
                Selectors {
                    on_unapproved: policy,
                    ..Default::default()
                },
                false,
            )
            .await;
        let runtime = Runtime::build(p).await.unwrap();
        let gate = runtime.guest_scope().gate();
        let host = GuestHost::from_scope(runtime.guest_scope(), vec![], GuestLimits::default())
            .await
            .unwrap();
        assert_eq!(
            kind(host.describe(SourceKind::User, "demo").await),
            ErrorKind::NeedsApproval
        );
        assert_eq!(host.compiled.load(Ordering::Relaxed), 0);
        assert_eq!(gate.approval_required(), policy == OnUnapproved::Fail);
        assert!(!f.workspace.join("describe-write.txt").exists());
    }
}

#[tokio::test]
async fn invalid_approved_bytes_fail_with_fixed_diagnostics_not_guest_or_parser_text() {
    let f = Fixture::new(caps(false, false, false));
    write(&f.bundle, "gadget.wasm", b"SECRET-BYTES-not-a-component");
    let host = f.host(Selectors::default()).await;
    let err = host.describe(SourceKind::User, "demo").await.err().unwrap();
    assert_eq!(err.kind, ErrorKind::InvalidGadget);
    assert!(!err.message.contains("SECRET") && !err.message.contains(f.config.to_str().unwrap()));
}

#[tokio::test]
async fn mismatched_prepared_gate_or_approved_source_cannot_mix_invocation_authority() {
    let a = Fixture::new(caps(true, true, false));
    let b = Fixture::new(caps(true, true, false));
    let p = a.prepared(Selectors::default(), false).await;
    let other = b.prepared(allow(&["write_file"]), false).await;
    let gate = Arc::new(InvocationGate::new(other.permissions().clone()));
    // A production factory has no Prepared/gate-pair constructor. Its opaque
    // scope comes ONLY from Runtime. The store still rejects cross-source policy.
    let sources = SourceAuthority::from_prepared(&p, vec!["demo".into()])
        .await
        .unwrap();
    let source = sources.authorize(SourceKind::User, "demo").await.unwrap();
    assert_eq!(
        kind(HeadlessState::from_source(
            &source,
            gate,
            Phase::Call,
            Arc::new(CallBudget::new(Duration::from_secs(30))),
            256 * 1024 * 1024,
            http::client().unwrap(),
            p.credentials().redactor()
        )),
        ErrorKind::InvalidArgs
    );
    assert!(!a.workspace.join("created.bin").exists());
    assert!(!b.workspace.join("created.bin").exists());
}

#[tokio::test]
async fn invocation_and_source_grants_do_not_approve_host_writes_or_network() {
    let f = Fixture::new(caps(true, true, true));
    let (_, mut state) = f.state(Selectors::default(), Phase::Call).await;
    assert!(
        state
            .write_file("unapproved.txt".into(), b"no".to_vec())
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::NeedsApproval);
    assert!(
        state
            .http_request(request("https://example.com/"))
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::NeedsApproval);
    assert_eq!(state.http.network_calls, 0);
    assert!(!state.gate.approval_required());
    assert!(!f.workspace.join("unapproved.txt").exists());
}

#[tokio::test]
async fn catching_unapproved_write_then_read_succeeds_only_under_deny_policy() {
    let f = Fixture::new(caps(true, true, false));
    let host = f.host(Selectors::default()).await;
    let result = host
        .call(SourceKind::User, "demo", "catch_write_then_read", json!({}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.json).unwrap(),
        json!({"caught":true,"later_read":true})
    );
    assert!(!host.gate.approval_required());
    assert!(!f.workspace.join("unapproved.txt").exists());
    let host = f.host(fail()).await;
    assert_eq!(
        kind(
            host.call(SourceKind::User, "demo", "catch_write_then_read", json!({}))
                .await
        ),
        ErrorKind::NeedsApproval
    );
    assert!(host.gate.approval_required());
    assert_eq!(
        kind(
            host.call(SourceKind::User, "demo", "read_bytes", json!({}))
                .await
        ),
        ErrorKind::NeedsApproval
    );
}

#[tokio::test]
async fn caught_import_followed_by_cpu_spin_or_wasi_sleep_stops_on_latch_not_deadline() {
    let f = Fixture::new(caps(true, true, false));
    for function in ["catch_write_then_spin", "catch_write_then_sleep"] {
        let host = f.host(fail()).await;
        // Cold source/Wasmtime compilation and instantiation are bounded setup,
        // NOT the prompt-stop measurement. Keep production's 30s default intact.
        let loading = Arc::new(CallBudget::new(Duration::from_secs(120)));
        let (source, component) = host.load(SourceKind::User, "demo", &loading).await.unwrap();
        let mut store = host.store(&source, Phase::Call, loading.clone()).unwrap();
        let gadget = host
            .guarded(&loading, async {
                bindings::Gadget::instantiate_async(&mut store, &component, &host.linker)
                    .await
                    .map_err(|_| error(ErrorKind::InvalidGadget, "fixture instantiation failed"))
            })
            .await
            .unwrap();
        assert!(!host.gate.approval_required());
        assert!(store.data().last_failure.is_none());
        let budget = Arc::new(CallBudget::new(host.limits.timeout));
        store.data_mut().set_budget_for_tests(budget.clone());
        store.set_epoch_deadline(1);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            host.guarded(&budget, async {
                gadget
                    .craftsman_gadget_guest()
                    .call_call(&mut store, function, "{}")
                    .await
                    .map_err(|_| error(ErrorKind::Trap, "fixture export trapped"))
            }),
        )
        .await
        .expect("caught import must seal promptly AFTER guest readiness");
        let err = classify(result.err().unwrap(), store.data());
        assert_eq!(err.kind, ErrorKind::NeedsApproval);
        assert!(host.gate.approval_required());
        assert_eq!(store.data().last_failure, Some(ErrorKind::NeedsApproval));
        assert!(
            !budget.expired(),
            "approval stop must precede the guest deadline"
        );
        assert!(
            !store
                .data()
                .capture
                .snapshot_for_tests()
                .0
                .contains("must-not-be-logged-after-fail")
        );
        assert!(!f.workspace.join("unapproved.txt").exists());
    }
}

#[tokio::test]
async fn explicit_denies_plan_and_hard_paths_remain_final_without_approval_latching() {
    let f = Fixture::new(caps(true, true, true));
    let mut selectors = allow(&["write_file", "web_fetch"]);
    selectors.mode = Mode::Plan;
    selectors.on_unapproved = OnUnapproved::Fail;
    let (_, mut state) = f.state(selectors, Phase::Call).await;
    assert!(state.write_file("plan.txt".into(), vec![0]).await.is_err());
    assert_eq!(meta_error(&state), ErrorKind::DeniedCapability);
    let mut post = request("https://example.com/");
    post.method = "POST".into();
    assert!(state.http_request(post).await.is_err());
    assert_eq!(meta_error(&state), ErrorKind::DeniedCapability);
    assert!(!state.gate.approval_required());
    let mut selectors = allow(&["write_file", "web_fetch"]);
    selectors.deny = vec!["tool:write_file".into(), "tool:web_fetch".into()];
    selectors.on_unapproved = OnUnapproved::Fail;
    let (_, mut state) = f.state(selectors, Phase::Call).await;
    assert!(
        state
            .write_file("denied.txt".into(), vec![0])
            .await
            .is_err()
    );
    assert!(
        state
            .http_request(request("https://example.com/"))
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::DeniedCapability);
    assert!(!state.gate.approval_required());
    let (_, mut state) = f.state(fail(), Phase::Call).await;
    for path in [
        "selected-config/config.json",
        ".env-secret",
        ".git/config",
        ".craftsman/config.json",
    ] {
        assert!(state.write_file(path.into(), vec![0]).await.is_err());
        assert_eq!(meta_error(&state), ErrorKind::DeniedCapability);
        assert!(!state.gate.approval_required());
    }
}

#[tokio::test]
async fn exact_protected_write_needs_path_and_tool_and_keeps_create_only_semantics() {
    let f = Fixture::new(caps(true, true, false));
    let mut selectors = allow(&["write_file"]);
    selectors
        .allow
        .push("path:.github/workflows/checked.yml".into());
    let (_, mut state) = f.state(selectors, Phase::Call).await;
    state
        .write_file(".github/workflows/checked.yml".into(), b"checked".to_vec())
        .await
        .unwrap();
    assert!(
        state
            .write_file(".github/workflows/other.yml".into(), vec![0])
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::NeedsApproval);
    assert!(
        state
            .write_file(".github/workflows/checked.yml".into(), vec![0])
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::DeniedCapability);
    assert_eq!(
        std::fs::read(f.workspace.join(".github/workflows/checked.yml")).unwrap(),
        b"checked"
    );
}

#[tokio::test]
async fn lists_are_structured_and_filter_credentials_config_and_runtime_artifacts() {
    let f = Fixture::new(caps(true, false, false));
    write(&f.workspace, ".env-synthetic", b"must-hide");
    write(&f.workspace, ".craftsman-exec-tmp-synthetic", b"must-hide");
    write(&f.workspace, "data/bytes.bin", b"bytes");
    let (_, mut state) = f.state(Selectors::default(), Phase::Call).await;
    let names: Vec<_> = state
        .list_dir(".".into())
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["Cargo.toml", "data", "safe.txt"]);
    let entries = state.list_dir("data".into()).await.unwrap();
    assert_eq!(entries[0].name, "bytes.bin");
    assert_eq!(entries[0].size, 5);
    assert!(!entries[0].is_dir);
    assert!(
        state
            .read_file("selected-config/config.json".into())
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::DeniedCapability);
}

#[tokio::test]
async fn wasi_has_no_inherited_environment_stdio_preopens_or_sockets() {
    let f = Fixture::new(caps(true, true, true));
    let host = f.host(allow(&["write_file", "web_fetch"])).await;
    let result = host
        .call(SourceKind::User, "demo", "ambient", json!({}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.json).unwrap(),
        json!({"env":null,"file":false,"socket":false})
    );
    assert_eq!(result.network_calls, 0);
}

#[tokio::test]
async fn misleading_guest_failure_text_is_not_a_typed_approval_or_raw_diagnostic() {
    let f = Fixture::new(caps(false, false, false));
    let host = f.host(fail()).await;
    let err = host
        .call(SourceKind::User, "demo", "literal_error", json!({}))
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(!err.message.contains("SECRET") && !err.message.contains("NeedsApproval"));
    assert!(!host.gate.approval_required());
}

#[tokio::test]
async fn guest_json_and_logs_are_bounded_redacted_memory_only_not_public_producers() {
    let f = Fixture::new(caps(false, false, false));
    let host = f.host(Selectors::default()).await;
    let result = host
        .call(SourceKind::User, "demo", "output", json!({}))
        .await
        .unwrap();
    assert_eq!(result.logs.len(), super::super::host::MAX_LOG_BYTES);
    assert!(result.logs_truncated);
    assert!(result.logs.contains('x'));
    assert_eq!(
        kind(host.call(SourceKind::User, "demo", "huge", json!({})).await),
        ErrorKind::OutputLimit
    );
    assert!(!f.config.join("sessions-rs").exists());
    assert!(!f.config.join("threads.db").exists());
}

#[tokio::test]
async fn memory_exhaustion_has_typed_fixed_classification() {
    let f = Fixture::new(caps(false, false, false));
    let host = f.host(Selectors::default()).await;
    assert_eq!(
        kind(
            host.call(SourceKind::User, "demo", "allocate", json!({}))
                .await
        ),
        ErrorKind::OutOfMemory
    );
}

#[tokio::test]
async fn expired_budget_never_polls_io_and_active_wait_is_bounded() {
    let gate = Arc::new(InvocationGate::new(Arc::new(
        crate::exec::permissions::InvocationPermissions::from_explicit(Selectors::default(), &[])
            .unwrap(),
    )));
    let calls = AtomicUsize::new(0);
    let expired = CallBudget::new(Duration::ZERO);
    assert_eq!(
        kind(
            bounded(&gate, &expired, async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
        ),
        ErrorKind::Timeout
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let budget = CallBudget::new(Duration::from_millis(100));
    assert_eq!(
        kind(
            bounded(&gate, &budget, async {
                std::future::pending::<Result<(), GadgetError>>().await
            })
            .await
        ),
        ErrorKind::Timeout
    );
}

#[tokio::test]
async fn expired_import_does_not_create_paths_or_start_network() {
    let f = Fixture::new(caps(true, true, true));
    let (_, mut state) = f
        .state(allow(&["write_file", "web_fetch"]), Phase::Call)
        .await;
    state.set_budget_for_tests(Arc::new(CallBudget::new(Duration::ZERO)));
    assert!(
        state
            .write_file("late/path/file".into(), vec![0])
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::Timeout);
    assert!(
        state
            .http_request(request("https://example.com/"))
            .await
            .is_err()
    );
    assert_eq!(state.http.network_calls, 0);
    assert!(!f.workspace.join("late").exists());
}

#[tokio::test]
async fn wasm_cpu_and_wasi_sleep_have_real_post_instantiation_deadlines() {
    let f = Fixture::focused(caps(false, false, false));
    for function in ["spin", "sleep"] {
        let host = f.host(Selectors::default()).await;
        let loading = Arc::new(CallBudget::new(Duration::from_secs(30)));
        let (source, component) = host.load(SourceKind::User, "demo", &loading).await.unwrap();
        let mut store = host.store(&source, Phase::Call, loading.clone()).unwrap();
        let gadget = bindings::Gadget::instantiate_async(&mut store, &component, &host.linker)
            .await
            .unwrap();
        // Start a short budget ONLY after cold compilation/instantiation, so this
        // test cannot pass merely because the compiler consumed the deadline.
        let budget = Arc::new(CallBudget::new(Duration::from_secs(1)));
        store.data_mut().set_budget_for_tests(budget.clone());
        store.set_epoch_deadline(1);
        let err = tokio::time::timeout(
            Duration::from_secs(5),
            host.guarded(&budget, async {
                gadget
                    .craftsman_gadget_guest()
                    .call_call(&mut store, function, "{}")
                    .await
                    .map_err(|_| error(ErrorKind::Trap, "fixture trapped"))
            }),
        )
        .await
        .unwrap()
        .err()
        .unwrap();
        assert_eq!(classify(err, store.data()).kind, ErrorKind::Timeout);
        assert!(
            store
                .data()
                .capture
                .snapshot_for_tests()
                .0
                .contains(&format!("{function}-entered"))
        );
    }
}

#[tokio::test]
async fn approval_signal_wakes_active_wait_without_claiming_blocking_work_rollback() {
    let gate = Arc::new(InvocationGate::new(Arc::new(
        crate::exec::permissions::InvocationPermissions::from_explicit(fail(), &[]).unwrap(),
    )));
    let started = Arc::new(tokio::sync::Notify::new());
    let done = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let notified = started.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    let worker = tokio::task::spawn_blocking({
        let started = started.clone();
        let done = done.clone();
        let release = release.clone();
        let calls = calls.clone();
        move || {
            started.notify_waiters();
            let (lock, cv) = &*release;
            let mut ready = lock.lock().unwrap();
            while !*ready {
                ready = cv.wait(ready).unwrap();
            }
            calls.fetch_add(1, Ordering::SeqCst);
            done.notify_waiters();
        }
    });
    let task = tokio::spawn({
        let gate = gate.clone();
        async move {
            bounded(&gate, &CallBudget::new(Duration::from_secs(30)), async {
                worker
                    .await
                    .map_err(|_| error(ErrorKind::Io, "worker failed"))?;
                Ok(())
            })
            .await
        }
    });
    notified.await;
    gate.observe_error_kind(craftsman_harness::ToolErrorKind::NeedsApproval);
    assert_eq!(
        kind(
            tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap()
        ),
        ErrorKind::NeedsApproval
    );
    let completed = done.notified();
    tokio::pin!(completed);
    completed.as_mut().enable();
    {
        let (lock, cv) = &*release;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
    tokio::time::timeout(Duration::from_secs(3), completed)
        .await
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "started blocking work survives dropped waiter"
    );
}

#[tokio::test]
async fn https_policy_requires_declared_exact_host_and_explicit_network_permission() {
    let f = Fixture::new(caps(false, false, true));
    let (_, mut state) = f.state(allow(&["web_fetch"]), Phase::Call).await;
    let sent = Arc::new(AtomicUsize::new(0));
    state.http.reply = Some(Arc::new({
        let sent = sent.clone();
        move |_| {
            sent.fetch_add(1, Ordering::SeqCst);
            Ok(Response {
                status: 200,
                headers: vec![],
                body: vec![0, 255],
            })
        }
    }));
    for url in [
        "http://example.com/",
        "https://other.example/",
        "https://example.com:444/",
        "https://user:secret@example.com/",
        "file:///secret",
    ] {
        assert!(state.http_request(request(url)).await.is_err());
        assert_eq!(meta_error(&state), ErrorKind::DeniedCapability);
    }
    assert_eq!(sent.load(Ordering::SeqCst), 0);
    assert_eq!(state.http.network_calls, 0);
    let response = state
        .http_request(request("https://example.com/"))
        .await
        .unwrap();
    assert_eq!(response.body, vec![0, 255]);
    assert_eq!(state.http.network_calls, 1);
    let (_, mut state) = f.state(fail(), Phase::Call).await;
    assert!(
        state
            .http_request(request("https://example.com/"))
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::NeedsApproval);
    assert!(state.gate.approval_required());
    assert_eq!(state.http.network_calls, 0);
}

#[tokio::test]
async fn https_request_header_body_and_count_bounds_are_checked_before_admission() {
    let f = Fixture::new(caps(false, false, true));
    let (_, mut state) = f.state(allow(&["web_fetch"]), Phase::Call).await;
    state.http.reply = Some(Arc::new(|_| {
        Ok(Response {
            status: 200,
            headers: vec![],
            body: vec![],
        })
    }));
    let mut body = request("https://example.com/");
    body.body = Some(vec![0; MAX_JSON_BYTES + 1]);
    let mut headers = request("https://example.com/");
    headers.headers = vec![("x".into(), "y".into()); 65];
    let mut bytes = request("https://example.com/");
    bytes.headers = vec![("x".into(), "x".repeat(64 * 1024))];
    let mut url = request("https://example.com/");
    url.url = "x".repeat(16 * 1024 + 1);
    for req in [body, headers, bytes, url] {
        assert!(state.http_request(req).await.is_err());
        assert_eq!(meta_error(&state), ErrorKind::OutputLimit);
    }
    for header in [
        "host",
        "Connection",
        "proxy-authorization",
        "transfer-encoding",
        "content-length",
    ] {
        let mut req = request("https://example.com/");
        req.headers.push((header.into(), "x".into()));
        assert!(state.http_request(req).await.is_err());
        assert_eq!(meta_error(&state), ErrorKind::DeniedCapability);
    }
    assert_eq!(state.http.network_calls, 0);
    for _ in 0..64 {
        state
            .http_request(request("https://example.com/"))
            .await
            .unwrap();
    }
    assert!(
        state
            .http_request(request("https://example.com/"))
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::OutputLimit);
    assert_eq!(state.http.network_calls, 64);
}

#[tokio::test]
async fn https_response_bounds_and_failed_attempt_accounting_use_synthetic_transport() {
    let f = Fixture::new(caps(false, false, true));
    let (_, mut state) = f.state(allow(&["web_fetch"]), Phase::Call).await;
    for case in 0..3 {
        state.http.reply = Some(Arc::new(move |_| {
            Ok(match case {
                0 => Response {
                    status: 200,
                    headers: vec![],
                    body: vec![0; 10 * 1024 * 1024 + 1],
                },
                1 => Response {
                    status: 200,
                    headers: vec![("x".into(), "y".into()); 65],
                    body: vec![],
                },
                _ => Response {
                    status: 200,
                    headers: vec![("x".into(), "y".repeat(64 * 1024))],
                    body: vec![],
                },
            })
        }));
        assert!(
            state
                .http_request(request("https://example.com/"))
                .await
                .is_err()
        );
        assert_eq!(meta_error(&state), ErrorKind::OutputLimit);
    }
    state.http.reply = Some(Arc::new(|_| {
        Err(error(ErrorKind::Failed, "synthetic HTTPS failure"))
    }));
    assert!(
        state
            .http_request(request("https://example.com/"))
            .await
            .is_err()
    );
    assert_eq!(meta_error(&state), ErrorKind::Failed);
    assert_eq!(
        state.http.network_calls, 4,
        "failed/oversized admitted requests still count"
    );
}

#[test]
fn description_and_argument_schemas_cannot_fetch_external_http_file_or_config_resources() {
    for uri in [
        "https://example.com/schema",
        "file:///config/secret",
        "relative/schema",
    ] {
        assert_eq!(
            kind(validator(&json!({"$ref":uri}))),
            ErrorKind::InvalidGadget
        );
    }
    let local = json!({"$defs":{"text":{"type":"string"}},"type":"object","properties":{"x":{"$ref":"#/$defs/text"}}});
    assert!(validator(&local).unwrap().is_valid(&json!({"x":"ok"})));
    assert!(!validator(&local).unwrap().is_valid(&json!({"x":0})));
    for text in [
        "SECRET-invalid-json",
        r#"{"functions":[{"name":"bad name","description":"secret","input_schema":{}}]}"#,
        r#"{"functions":[{"name":"demo","description":"valid","input_schema":{"$ref":"https://example.com/private"}}]}"#,
    ] {
        let err = parse_description(text).err().unwrap();
        assert_eq!(err.kind, ErrorKind::InvalidGadget);
        assert!(!err.message.contains("SECRET"));
    }
}

#[tokio::test]
async fn invalid_limits_opt_in_and_project_trust_fail_before_any_guest_execution() {
    let f = Fixture::new(caps(true, false, false));
    let p = f.prepared(Selectors::default(), false).await;
    let runtime = Runtime::build(p).await.unwrap();
    for limits in [
        GuestLimits {
            timeout: Duration::ZERO,
            ..Default::default()
        },
        GuestLimits {
            timeout: Duration::from_secs(301),
            ..Default::default()
        },
        GuestLimits {
            memory_bytes: 1024,
            ..Default::default()
        },
        GuestLimits {
            memory_bytes: 257 * 1024 * 1024,
            ..Default::default()
        },
    ] {
        assert_eq!(
            kind(GuestHost::from_scope(runtime.guest_scope(), vec!["demo".into()], limits).await),
            ErrorKind::InvalidArgs
        );
    }
    let host = f.host(fail()).await;
    assert_eq!(
        kind(host.describe(SourceKind::Project, "demo").await),
        ErrorKind::NeedsApproval
    );
    assert_eq!(host.compiled.load(Ordering::Relaxed), 0);
    write(
        &f.config,
        "config.json",
        br#"{"model":"synthetic","gadgets":{"enabled":false}}"#,
    );
    let p = f.prepared(Selectors::default(), false).await;
    let runtime = Runtime::build(p).await.unwrap();
    assert_eq!(
        kind(
            GuestHost::from_scope(
                runtime.guest_scope(),
                vec!["demo".into()],
                GuestLimits::default()
            )
            .await
        ),
        ErrorKind::DeniedCapability
    );
}

#[cfg(unix)]
#[tokio::test]
async fn source_owned_filesystem_survives_config_and_workspace_rebinding_without_ambient_reopen() {
    let f = Fixture::new(caps(true, true, false));
    let p = f.prepared(allow(&["write_file"]), false).await;
    let runtime = Runtime::build(p).await.unwrap();
    let host = GuestHost::from_scope(
        runtime.guest_scope(),
        vec!["demo".into()],
        GuestLimits::default(),
    )
    .await
    .unwrap();
    let moved = f.root.path().join("moved-work");
    std::fs::rename(&f.workspace, &moved).unwrap();
    write(&f.workspace, "safe.txt", b"wrong replacement");
    write(
        &f.config,
        "gadgets/demo/gadget.wasm",
        b"malicious replacement",
    );
    let result = host
        .call(SourceKind::User, "demo", "read_bytes", json!({}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.json).unwrap(),
        json!(b"safe bytes".to_vec())
    );
    host.call(
        SourceKind::User,
        "demo",
        "write_bytes",
        json!({"bytes":[0,255]}),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(moved.join("created.bin")).unwrap(), [0, 255]);
    assert!(!f.workspace.join("created.bin").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn guest_imports_reject_symlink_hardlink_and_fifo_without_leaking_outside_bytes() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(caps(true, false, false));
    let outside = f.root.path().join("outside");
    std::fs::write(&outside, b"outside bytes").unwrap();
    symlink(&outside, f.workspace.join("link")).unwrap();
    std::fs::hard_link(&outside, f.workspace.join("hard")).unwrap();
    let fifo = std::ffi::CString::new(f.workspace.join("fifo").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let (_, mut state) = f.state(Selectors::default(), Phase::Call).await;
    for name in ["link", "hard", "fifo"] {
        let err = tokio::time::timeout(Duration::from_secs(3), state.read_file(name.into()))
            .await
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(meta_error(&state), ErrorKind::DeniedCapability);
        assert!(!err.contains("outside bytes"));
    }
}

fn credentials(f: &Fixture, key: &str, extra: Value) {
    let mut cfg =
        json!({"model":"synthetic","api_key":key,"gadgets":{"enabled":true},"auto_approve":true});
    cfg.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    write(&f.config, "config.json", cfg.to_string().as_bytes());
}

#[tokio::test]
async fn actual_guest_json_redacts_unicode_escaped_values_and_member_names_before_encoding() {
    let f = Fixture::new(caps(false, false, false));
    let key = "synthetic-secret\"\n钥匙😀";
    credentials(&f, key, json!({}));
    let host = f.host(Selectors::default()).await;
    let value = json!({key:{"nested":[key,42,true,null]},"ordinary":"value"});
    let result = host
        .call(SourceKind::User, "demo", "echo", json!({"value":value}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.json).unwrap(),
        json!({"[REDACTED]":{"nested":["[REDACTED]",42,true,null]},"ordinary":"value"})
    );
    assert!(result.logs.is_empty() && !result.logs_truncated);
    assert_eq!(result.approval.id, "demo");
    assert_eq!(result.network_calls, 0);
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn actual_guest_execution_uses_original_path_and_payload_not_redacted_authorization_input() {
    let f = Fixture::new(caps(true, true, false));
    let key = "synthetic-secret-payload";
    credentials(&f, key, json!({}));
    let host = f.host(allow(&["write_file"])).await;
    let path = format!("data/{key}.bin");
    let result = host
        .call(
            SourceKind::User,
            "demo",
            "write_and_echo",
            json!({"path":path,"payload":key}),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(f.workspace.join(&path)).unwrap(),
        key.as_bytes()
    );
    assert!(!f.workspace.join("data/[REDACTED].bin").exists());
    assert_eq!(
        serde_json::from_str::<Value>(&result.json).unwrap(),
        json!({"path":"data/[REDACTED].bin","payload":"[REDACTED]"})
    );
    assert_eq!(result.approval.id, "demo");
    assert!(result.duration > Duration::ZERO);
    assert_eq!(result.network_calls, 0);
    assert!(
        !f.config.join("gadgets/.grants.json").exists() && !f.config.join("gadgets/.sdk").exists()
    );
    assert!(!f.config.join("sessions-rs").exists() && !f.config.join("threads.db").exists());
}

#[tokio::test]
async fn guest_host_uses_the_prepared_frozen_seeds_not_a_later_config_reopen() {
    let f = Fixture::new(caps(false, false, false));
    let original = "original-synthetic-secret";
    let replacement = "replacement-synthetic-secret";
    credentials(&f, original, json!({}));
    let prepared = f.prepared(Selectors::default(), false).await;
    let runtime = Runtime::build(prepared).await.unwrap();
    credentials(&f, replacement, json!({}));
    let host = GuestHost::from_scope(
        runtime.guest_scope(),
        vec!["demo".into()],
        GuestLimits::default(),
    )
    .await
    .unwrap();
    let result = host
        .call(
            SourceKind::User,
            "demo",
            "echo",
            json!({"value":[original,replacement]}),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.json).unwrap(),
        json!(["[REDACTED]", replacement])
    );
}

#[tokio::test]
async fn actual_wasip2_stdout_stderr_and_log_imports_are_one_redacted_ordered_stream() {
    let f = Fixture::new(caps(false, false, false));
    credentials(&f, "secret-value", json!({}));
    let host = f.host(Selectors::default()).await;
    let result = host
        .call(
            SourceKind::User,
            "demo",
            "logs",
            json!({"parts":[
                {"channel":"stdout","text":"before sec"},
                {"channel":"stderr","text":"ret-value after"},
                {"channel":"log","text":"secret-value"}
            ]}),
        )
        .await
        .unwrap();
    assert_eq!(
        result.logs,
        "before [REDACTED] after[Level::Info] [REDACTED]\n"
    );
    assert!(!result.logs_truncated);
    assert_eq!(
        serde_json::from_str::<Value>(&result.json).unwrap(),
        json!({"done":true})
    );
}

#[tokio::test]
async fn actual_guest_key_crossing_original_64k_cut_is_redacted_before_retention() {
    let f = Fixture::new(caps(false, false, false));
    let key = "fixture-secret-value";
    credentials(&f, key, json!({}));
    let host = f.host(Selectors::default()).await;
    let result = host
        .call(
            SourceKind::User,
            "demo",
            "logs",
            json!({"parts":[
                {"channel":"stdout","text":"q".repeat(output::MAX_LOG_BYTES-8)},
                {"channel":"stderr","text":key},
                {"channel":"stdout","text":" trailing"}
            ]}),
        )
        .await
        .unwrap();
    assert!(result.logs.len() <= output::MAX_LOG_BYTES);
    assert!(result.logs_truncated && !result.logs.contains("fixture"));
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn unsafe_guest_json_collision_scalar_or_generated_literal_is_a_fixed_nonapproval_failure() {
    for (key, extra, value) in [
        (
            "secret-one",
            json!({"search_api_key":"secret-two"}),
            json!({"secret-one":1,"secret-two":2}),
        ),
        ("12345", json!({}), json!(12345)),
        (
            "secret",
            json!({"search_api_key":"x[REDACTED]"}),
            json!("xsecret"),
        ),
    ] {
        let f = Fixture::new(caps(false, false, false));
        credentials(&f, key, extra);
        let host = f.host(fail()).await;
        let e = host
            .call(SourceKind::User, "demo", "echo", json!({"value":value}))
            .await
            .err()
            .unwrap();
        assert_eq!(e.kind, ErrorKind::InvalidGadget);
        assert!(!e.message.contains(key));
        assert!(!host.gate.approval_required());
        assert_eq!(e.network_calls, 0);
        assert!(
            host.call(
                SourceKind::User,
                "demo",
                "echo",
                json!({"value":{"ordinary":"later"}})
            )
            .await
            .is_ok()
        );
    }
}

#[tokio::test]
async fn invalid_wasi_bytes_or_oversized_import_logs_cannot_publish_an_otherwise_successful_json() {
    let f = Fixture::new(caps(false, false, false));
    for (function, kind) in [
        ("invalid_stdout", ErrorKind::InvalidGadget),
        ("huge_log", ErrorKind::OutputLimit),
    ] {
        let host = f.host(fail()).await;
        let e = host
            .call(SourceKind::User, "demo", function, json!({}))
            .await
            .err()
            .unwrap();
        assert_eq!(e.kind, kind);
        assert!(e.logs_truncated && !e.message.contains("must_not_publish"));
        assert!(!host.gate.approval_required());
    }
}

#[tokio::test]
async fn actual_guest_failure_discards_held_secret_suffix_and_preserves_typed_error_metadata() {
    let f = Fixture::new(caps(false, false, false));
    credentials(&f, "secret-value", json!({}));
    let host = f.host(fail()).await;
    let e = host
        .call(
            SourceKind::User,
            "demo",
            "logs_then_error",
            json!({"text":"secret-"}),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(
        e.logs_truncated
            && !e.message.contains("secret")
            && !e.message.contains("SECRET-GUEST-CONTENT")
    );
    assert_eq!(e.network_calls, 0);
    assert!(!host.gate.approval_required());
}

#[tokio::test]
async fn actual_caught_import_stops_direct_wasip2_writes_and_preserves_the_same_runtime_latch() {
    let f = Fixture::new(caps(true, true, false));
    let host = f.host(fail()).await;
    let loading = Arc::new(CallBudget::new(Duration::from_secs(120)));
    let (source, component) = host.load(SourceKind::User, "demo", &loading).await.unwrap();
    let mut store = host.store(&source, Phase::Call, loading.clone()).unwrap();
    let gadget = host
        .guarded(&loading, async {
            bindings::Gadget::instantiate_async(&mut store, &component, &host.linker)
                .await
                .map_err(|_| error(ErrorKind::InvalidGadget, "fixture instantiation failed"))
        })
        .await
        .unwrap();
    let budget = Arc::new(CallBudget::new(host.limits.timeout));
    store.data_mut().set_budget_for_tests(budget.clone());
    store.set_epoch_deadline(1);
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        host.guarded(&budget, async {
            gadget
                .craftsman_gadget_guest()
                .call_call(&mut store, "post_fail_stdio", "{}")
                .await
                .map_err(|_| error(ErrorKind::Trap, "fixture export trapped"))
        }),
    )
    .await
    .expect("post-readiness typed stop must stay prompt");
    let e = classify(result.err().unwrap(), store.data());
    assert_eq!(e.kind, ErrorKind::NeedsApproval);
    assert!(host.gate.approval_required() && !budget.expired());
    let logs = store.data().capture.snapshot_for_tests().0;
    assert!(logs.contains("before-fail-entry"));
    assert!(!logs.contains("must-not-be-logged-after-fail"));
    assert!(!f.workspace.join("unapproved.txt").exists());
}

#[tokio::test]
async fn focused_source_retains_real_empty_capability_describe_probes_and_exact_exports() {
    let f = Fixture::focused(caps(true, true, true));
    let host = f.host(allow(&["write_file", "web_fetch"])).await;
    assert_eq!(host.limits.timeout, Duration::from_secs(30));
    let description = host.describe(SourceKind::User, "demo").await.unwrap();
    let names = description
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["read_bytes", "sleep", "spin", "write_bytes"]);
    assert!(
        description
            .functions
            .iter()
            .all(|function| function.description == "read=false;write=false;http=false")
    );
    assert_eq!(host.compiled.load(Ordering::Relaxed), 1);
    assert!(!f.workspace.join("describe-write.txt").exists());
    assert!(!f.config.join("gadgets/.grants.json").exists());
    assert!(!f.config.join("gadgets/.sdk").exists());
    assert!(!f.config.join("threads.db").exists());
}

#[tokio::test]
async fn focused_source_uses_original_dynamic_binary_arguments_and_preserves_denied_overwrite() {
    let f = Fixture::focused(caps(true, true, false));
    let host = f.host(allow(&["write_file"])).await;
    assert_eq!(host.limits.timeout, Duration::from_secs(30));
    let original = (0..=255u8).rev().collect::<Vec<_>>();
    let result = host
        .call(
            SourceKind::User,
            "demo",
            "write_bytes",
            json!({"path":"different.bin","bytes":original}),
        )
        .await
        .unwrap();
    assert_eq!(result.approval.id, "demo");
    assert_eq!(result.network_calls, 0);
    let read = host
        .call(
            SourceKind::User,
            "demo",
            "read_bytes",
            json!({"path":"different.bin"}),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<u8>>(&read.json).unwrap(),
        original
    );
    assert_eq!(
        kind(
            host.call(
                SourceKind::User,
                "demo",
                "write_bytes",
                json!({"path":"different.bin","bytes":[1,2,3]})
            )
            .await
        ),
        ErrorKind::DeniedCapability
    );
    assert_eq!(
        std::fs::read(f.workspace.join("different.bin")).unwrap(),
        original
    );
    assert!(!host.gate.approval_required());
    assert_eq!(host.compiled.load(Ordering::Relaxed), 3);
    assert!(!f.config.join("gadgets/.grants.json").exists());
    assert!(!f.config.join("gadgets/.sdk").exists());
    assert!(!f.config.join("threads.db").exists());
}

#[tokio::test]
#[ignore = "explicit compile-cost probe only; argv marker required before fixture/config work"]
async fn focused_fixture_compile_cost_probe() {
    const MARKER: &str = "--craftsman-private-focused-compile-probe";
    if !std::env::args().any(|argument| argument == MARKER) {
        return;
    }
    for focused in [false, true] {
        let fixture = if focused {
            Fixture::focused(caps(false, false, false))
        } else {
            Fixture::new(caps(false, false, false))
        };
        let host = fixture.host(Selectors::default()).await;
        assert_eq!(host.limits.timeout, Duration::from_secs(30));
        let budget = Arc::new(CallBudget::new(host.limits.timeout));
        let start = std::time::Instant::now();
        let (source, _component) = host.load(SourceKind::User, "demo", &budget).await.unwrap();
        assert!(!budget.expired());
        assert_eq!(host.compiled.load(Ordering::Relaxed), 1);
        println!(
            "FOCUSED_COMPILE_PROBE focused={focused} original_bytes={} elapsed_ms={}",
            source.component_bytes().len(),
            start.elapsed().as_millis()
        );
    }
}
