//! The production pinned detector path and admission owner, run against the
//! actual source-built detector component. The ignored native entry point
//! returns before any build, configuration or path access unless exactly one
//! registered argv marker selected it. Everything it writes is under its own
//! temporary directory; the stored approval it creates there is a real grant
//! in that isolated store, not fixture metadata.

use super::super::grants::GrantStore;
use super::super::manifest::Capabilities;
use super::super::{ErrorKind, GadgetRuntime, PinnedSource, RuntimeOptions};
use super::{build_fixture, compiler_environment, hash, root};
use crate::privacy_hook::{
    Detector, DetectorPin, HookError, PinnedDetector, ReplyRefusal, Request,
};
use crate::privacy_send::SendRefusal;
use crate::privacy_session::{
    AdmitError, AdmitRefusal, ConversationScope, PrivacySession, SessionStop,
};
use crate::privacy_vault::{HostKey, HostKeySource, Limits, Vault, VaultLocation};
use serde_json::json;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use zeroize::Zeroizing;

const ID: &str = "privacy-detector";
const HELPER: &str = "gadgets::privacy_detector_tests::pinned_hook::pinned_hook_native_fixture";
const MARKER: &str = "craftsman-privacy-hook=";
const EMAIL: &str = "alice@example.invalid";
const PHONE: &str = "+886 912 345 678";

fn selected(arguments: impl IntoIterator<Item = OsString>) -> bool {
    let values: Vec<_> = arguments
        .into_iter()
        .filter_map(|argument| {
            argument
                .to_str()
                .and_then(|argument| argument.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    matches!(values.as_slice(), [value] if value == "suite")
}

struct SyntheticKey;

impl HostKeySource for SyntheticKey {
    fn acquire_privacy_key(&self) -> crate::privacy_vault::Result<HostKey> {
        Ok(HostKey::supplied_by_host(Zeroizing::new([9; 32])))
    }
}

struct Installed {
    workspace: PathBuf,
    config: PathBuf,
    bundle: PathBuf,
    image: Vec<u8>,
    manifest: Vec<u8>,
}

impl Installed {
    fn pin(&self) -> PinnedSource {
        PinnedSource {
            id: ID.to_owned(),
            component_sha256: hash(&self.image),
            manifest_sha256: hash(&self.manifest),
        }
    }

    fn runtime(&self, options: RuntimeOptions) -> GadgetRuntime {
        GadgetRuntime::new(&self.workspace, &self.config, options).unwrap()
    }

    fn restore(&self) {
        std::fs::write(self.bundle.join("gadget.wasm"), &self.image).unwrap();
        std::fs::write(self.bundle.join("gadget.toml"), &self.manifest).unwrap();
    }
}

fn body(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"input": [{"role": "user", "content": text}]})).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "pinned detector fixture; exactly one argv marker required before build/config access"]
async fn pinned_hook_native_fixture() {
    if !selected(std::env::args_os()) {
        println!("PRIVACY-HOOK-FIXTURE:NOT-SELECTED");
        return;
    }
    let root = root();
    let (image, _) = build_fixture(&root);
    let manifest =
        std::fs::read(root.join("crates/craftsman-rs/tests/fixtures/privacy-detector/gadget.toml"))
            .unwrap();
    let home = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let owned = std::fs::canonicalize(home.path()).unwrap();
    let installed = Installed {
        workspace: owned.join("workspace"),
        config: owned.join("owned-config"),
        bundle: owned.join("owned-config/gadgets").join(ID),
        image,
        manifest,
    };
    std::fs::create_dir_all(&installed.workspace).unwrap();
    std::fs::create_dir_all(&installed.bundle).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&installed.config, std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    installed.restore();
    let source = installed.bundle.canonicalize().unwrap();
    let grants = GrantStore::new(&installed.config);
    let none = Capabilities::default();
    let options = RuntimeOptions {
        disk_cache: false,
        ..RuntimeOptions::default()
    };
    let runtime = installed.runtime(options.clone());
    let pin = installed.pin();
    let text = format!("中文🦀 {EMAIL}; {PHONE}; {EMAIL}.");
    let request = Request::from_text(&text).unwrap();
    let arguments = request.to_json();
    let mut refusals = 0;
    macro_rules! refused {
        ($runtime:expr, $pin:expr, $kind:expr) => {{
            let error = $runtime
                .call_pinned($pin, "detect", arguments.clone())
                .await
                .unwrap_err();
            assert_eq!(error.kind, $kind, "{}", error.message);
            // Refused before any compile or describe.
            assert_eq!(
                ($runtime.compiled_count(), $runtime.described_count()),
                (0, 0)
            );
            refusals += 1;
        }};
    }

    // No stored approval.
    refused!(runtime, &pin, ErrorKind::NeedsApproval);
    // An approval of another build at the same path satisfies the ordinary
    // rebuild rule, and is not an approval of these bytes.
    grants.approve(ID, &source, &"1".repeat(64), &none).unwrap();
    assert!(
        grants
            .approved(ID, &source, &pin.component_sha256, &none)
            .unwrap()
    );
    assert!(
        !grants
            .approved_exact(ID, &source, &pin.component_sha256, &none)
            .unwrap()
    );
    refused!(runtime, &pin, ErrorKind::NeedsApproval);
    // An allow-list entry, interactive mode and workspace trust are not one.
    let lenient = installed.runtime(RuntimeOptions {
        allowed_gadgets: BTreeSet::from([ID.to_owned()]),
        interactive: true,
        trust_workspace: true,
        ..options.clone()
    });
    refused!(lenient, &pin, ErrorKind::NeedsApproval);
    drop(lenient);

    // The actual stored approval of these exact bytes at this source.
    grants
        .approve(ID, &source, &pin.component_sha256, &none)
        .unwrap();
    assert!(
        grants
            .approved_exact(ID, &source, &pin.component_sha256, &none)
            .unwrap()
    );

    // A selector that does not name the installed bytes.
    for wrong in [
        PinnedSource {
            component_sha256: "2".repeat(64),
            ..pin.clone()
        },
        PinnedSource {
            manifest_sha256: "3".repeat(64),
            ..pin.clone()
        },
    ] {
        refused!(runtime, &wrong, ErrorKind::InvalidGadget);
    }
    for malformed in [
        PinnedSource {
            component_sha256: pin.component_sha256.to_uppercase(),
            ..pin.clone()
        },
        PinnedSource {
            manifest_sha256: "4".repeat(63),
            ..pin.clone()
        },
        PinnedSource {
            id: "../privacy-detector".to_owned(),
            ..pin.clone()
        },
    ] {
        refused!(runtime, &malformed, ErrorKind::InvalidArgs);
    }
    refused!(
        runtime,
        &PinnedSource {
            id: "another-detector".to_owned(),
            ..pin.clone()
        },
        ErrorKind::NotFound
    );
    // Changed component bytes, then a changed manifest, under the same pin.
    let mut changed = installed.image.clone();
    changed.push(0);
    std::fs::write(installed.bundle.join("gadget.wasm"), &changed).unwrap();
    refused!(runtime, &pin, ErrorKind::InvalidGadget);
    installed.restore();
    let mut described = installed.manifest.clone();
    described.extend_from_slice(b"\n# changed\n");
    std::fs::write(installed.bundle.join("gadget.toml"), &described).unwrap();
    refused!(runtime, &pin, ErrorKind::InvalidGadget);
    // A manifest that declares a capability, with its own matching selector
    // and its own stored approval, is still refused.
    let capable = String::from_utf8(installed.manifest.clone())
        .unwrap()
        .replace("read = []", "read = [\"workspace\"]");
    assert_ne!(capable.as_bytes(), installed.manifest.as_slice());
    std::fs::write(installed.bundle.join("gadget.toml"), &capable).unwrap();
    let reading = Capabilities {
        read: BTreeSet::from(["workspace".to_owned()]),
        ..Capabilities::default()
    };
    grants
        .approve(ID, &source, &pin.component_sha256, &reading)
        .unwrap();
    refused!(
        runtime,
        &PinnedSource {
            manifest_sha256: hash(capable.as_bytes()),
            ..pin.clone()
        },
        ErrorKind::DeniedCapability
    );
    installed.restore();
    // A project gadget of the same id, whatever it contains.
    let shadow = installed.workspace.join(".craftsman/gadgets").join(ID);
    std::fs::create_dir_all(&shadow).unwrap();
    refused!(runtime, &pin, ErrorKind::InvalidGadget);
    std::fs::write(shadow.join("gadget.toml"), &installed.manifest).unwrap();
    std::fs::write(shadow.join("gadget.wasm"), &installed.image).unwrap();
    refused!(runtime, &pin, ErrorKind::InvalidGadget);
    std::fs::remove_dir_all(installed.workspace.join(".craftsman")).unwrap();

    // The approved, pinned, capability-free source runs.
    let result = runtime
        .call_pinned(&pin, "detect", arguments.clone())
        .await
        .unwrap();
    assert_eq!(
        (runtime.compiled_count(), runtime.described_count()),
        (1, 1)
    );
    assert_eq!(result.network_calls, 0);
    let spans = request.validate_reply(&result.json).unwrap();
    assert_eq!(spans.len(), 3);
    assert_eq!(&text[spans[0].start..spans[0].end], EMAIL);
    assert_eq!(&text[spans[1].start..spans[1].end], PHONE);
    assert_eq!(&text[spans[2].start..spans[2].end], EMAIL);
    // A compiled component does not outlive a change of the installed bytes.
    std::fs::write(installed.bundle.join("gadget.wasm"), &changed).unwrap();
    assert_eq!(
        runtime
            .call_pinned(&pin, "detect", arguments.clone())
            .await
            .unwrap_err()
            .kind,
        ErrorKind::InvalidGadget
    );
    refusals += 1;
    installed.restore();
    assert!(
        runtime
            .call_pinned(&pin, "detect", arguments.clone())
            .await
            .is_ok()
    );
    assert_eq!(runtime.compiled_count(), 1);

    // Replies of the actual guest that the host must refuse whole.
    let mut malformed = 0;
    for (mode, expected) in [
        ("bad_hash", ReplyRefusal::Binding),
        ("bad_node_hash", ReplyRefusal::Binding),
        ("unknown_node", ReplyRefusal::Node),
        ("negative", ReplyRefusal::Shape),
        ("scalar_split", ReplyRefusal::Offset),
        ("overflow", ReplyRefusal::Offset),
        ("reversed", ReplyRefusal::Offset),
        ("overlap", ReplyRefusal::Order),
        ("unknown_category", ReplyRefusal::Category),
        ("extra_field", ReplyRefusal::Shape),
        ("count", ReplyRefusal::Count),
        ("oversized", ReplyRefusal::Output),
        ("wrong_category", ReplyRefusal::Category),
    ] {
        let reply = runtime
            .call_pinned(
                &pin,
                "fixture_adversarial",
                json!({"mode": mode, "request": arguments}),
            )
            .await
            .unwrap();
        assert_eq!(
            request.validate_reply(&reply.json).err(),
            Some(expected),
            "{mode}"
        );
        malformed += 1;
    }
    // An empty report is well formed. The host cannot see an omission; the
    // admission owner covers only values its scope already knows.
    let omitted = runtime
        .call_pinned(
            &pin,
            "fixture_adversarial",
            json!({"mode": "omit", "request": arguments}),
        )
        .await
        .unwrap();
    assert!(request.validate_reply(&omitted.json).unwrap().is_empty());
    // A guest failure, an oversized result and an unknown function.
    for (function, input, kind) in [
        (
            "fixture_adversarial",
            json!({"mode": "unknown", "request": arguments}),
            ErrorKind::Failed,
        ),
        (
            "fixture_adversarial",
            json!({"mode": "sdk_oversized", "request": arguments}),
            ErrorKind::OutputLimit,
        ),
        (
            "detect",
            json!({"version": 1, "input_binding": 7, "nodes": []}),
            ErrorKind::InvalidArgs,
        ),
        ("unknown", json!({}), ErrorKind::NotFound),
    ] {
        assert_eq!(
            runtime
                .call_pinned(&pin, function, input)
                .await
                .unwrap_err()
                .kind,
            kind
        );
        malformed += 1;
    }
    // A guest that never returns is stopped by the call budget.
    let brief = installed.runtime(RuntimeOptions {
        timeout: Duration::from_secs(2),
        ..options.clone()
    });
    let started = Instant::now();
    assert_eq!(
        brief
            .call_pinned(
                &pin,
                "fixture_adversarial",
                json!({"mode": "spin", "request": arguments}),
            )
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Timeout
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    malformed += 1;
    drop(brief);

    // The admission owner over the actual detector, vault and guard.
    let vault = Vault::open(
        VaultLocation::under_host_config_root(&installed.config).unwrap(),
        &SyntheticKey,
        Limits::default(),
    )
    .await
    .unwrap();
    let detector = |pin: &PinnedSource, function: &str| {
        Detector::pinned(
            PinnedDetector::new(
                &installed.workspace,
                &installed.config,
                DetectorPin::supplied_by_host(
                    &pin.id,
                    function,
                    &pin.component_sha256,
                    &pin.manifest_sha256,
                )
                .unwrap(),
            )
            .unwrap(),
        )
    };
    assert_eq!(
        DetectorPin::supplied_by_host(ID, "detect", "not-a-hash", &pin.manifest_sha256).err(),
        Some(HookError::Source)
    );
    let scope = || ConversationScope::fresh(&installed.workspace).unwrap();
    let session = PrivacySession::open(&vault, scope(), detector(&pin, "detect"))
        .await
        .unwrap();
    let guard = session.send_guard();
    let admitted = session.admit_raw(&text).await.unwrap();
    let masked = admitted.text().unwrap().to_owned();
    assert_eq!(admitted.replacements(), 3);
    assert!(!masked.contains(EMAIL) && !masked.contains(PHONE));
    assert!(masked.starts_with("中文🦀 pv1-") && masked.ends_with('.'));
    assert_eq!(
        session
            .with_scope_records(|_, records| records.len().unwrap())
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        guard.seal(body(&text), &[]).err(),
        Some(SendRefusal::KnownOriginal)
    );
    assert!(guard.seal(body(&masked), &[]).is_ok());
    // A repeated value reuses its placeholder; admitted text is not raw text.
    let again = session.admit_raw(&format!("re: {EMAIL}")).await.unwrap();
    assert!(masked.contains(again.text().unwrap().strip_prefix("re: ").unwrap()));
    assert_eq!(
        session.admit_raw(&masked).await.err(),
        Some(AdmitError::Refused(AdmitRefusal::IssuedPlaceholder))
    );
    let mut admitted_texts = 2;
    // A text the request cannot carry is refused; the session continues.
    assert_eq!(
        session
            .admit_raw(&format!("{}\n", "x".repeat(70 * 1024)))
            .await
            .err(),
        Some(AdmitError::Refused(AdmitRefusal::TooLarge))
    );
    assert!(session.admit_raw("no value here").await.is_ok());
    admitted_texts += 1;
    // A changed source ends the session: no admission, no send.
    std::fs::write(installed.bundle.join("gadget.wasm"), &changed).unwrap();
    assert_eq!(
        session.admit_raw(&format!("call {PHONE}")).await.err(),
        Some(AdmitError::Stopped(SessionStop::Detector))
    );
    installed.restore();
    assert_eq!(session.check(), Err(SessionStop::Closed));
    assert_eq!(admitted.text(), Err(SessionStop::Closed));
    assert_eq!(
        guard.seal(body("clean"), &[]).err(),
        Some(SendRefusal::Closed)
    );
    refusals += 1;
    // A selector for other bytes, and a detector whose replies are malformed.
    let wrong = PinnedSource {
        component_sha256: "5".repeat(64),
        ..pin.clone()
    };
    let unselected = PrivacySession::open(&vault, scope(), detector(&wrong, "detect"))
        .await
        .unwrap();
    assert_eq!(
        unselected.admit_raw(&text).await.err(),
        Some(AdmitError::Stopped(SessionStop::Detector))
    );
    refusals += 1;
    let unknown = PrivacySession::open(&vault, scope(), detector(&pin, "fixture_adversarial"))
        .await
        .unwrap();
    assert_eq!(
        unknown.admit_raw(&text).await.err(),
        Some(AdmitError::Stopped(SessionStop::Detector))
    );
    refusals += 1;
    assert_eq!(
        unknown.with_scope_records(|_, _| ()).await.err(),
        Some(SessionStop::Closed)
    );
    println!(
        "PRIVACY-HOOK-FIXTURE:COMPLETE refusals={refusals} malformed={malformed} admitted={admitted_texts}"
    );
}

#[test]
fn absent_unknown_duplicate_markers_do_not_select_the_pinned_fixture() {
    let marker = |value: &str| OsString::from(format!("{MARKER}{value}"));
    assert!(!selected([]));
    assert!(!selected([OsString::from("--ignored")]));
    assert!(!selected([marker("")]));
    assert!(!selected([marker("collect")]));
    assert!(!selected([marker("suite"), marker("suite")]));
    assert!(!selected([marker("suite"), marker("other")]));
    assert!(!selected([OsString::from(
        "craftsman-privacy-detector=suite"
    )]));
    assert!(selected([OsString::from("--exact"), marker("suite")]));
}

async fn run_helper(
    marker: Option<&str>,
    home: &Path,
) -> (std::process::ExitStatus, String, String) {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo").into_os_string())
        })
        .expect("explicit readonly Cargo source cache for trusted fixture build");
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--ignored", "--exact", HELPER, "--nocapture"]);
    if let Some(marker) = marker {
        command.arg(format!("{MARKER}{marker}"));
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    compiler_environment::isolated_supervisor_environment(
        command.as_std_mut(),
        home,
        &cargo_home,
        &std::env::var_os("PATH").expect("actual toolchain PATH"),
        compiler_environment::system_root(),
        compiler_environment::host_discovery(),
    );
    let mut child = command.spawn().unwrap();
    let mut stdout_reader = child.stdout.take().unwrap().take(65537);
    let mut stderr_reader = child.stderr.take().unwrap().take(65537);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let completed = tokio::time::timeout(Duration::from_secs(240), async {
        tokio::join!(
            stdout_reader.read_to_end(&mut stdout),
            stderr_reader.read_to_end(&mut stderr),
            child.wait()
        )
    })
    .await;
    let (out_read, err_read, status) = match completed {
        Ok(completed) => completed,
        Err(_) => {
            child.start_kill().unwrap();
            tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .expect("pinned fixture cleanup did not return")
                .unwrap();
            panic!("pinned detector fixture exceeded its setup and call bound");
        }
    };
    out_read.unwrap();
    err_read.unwrap();
    assert!(stdout.len() <= 64 * 1024 && stderr.len() <= 64 * 1024);
    (
        status.unwrap(),
        String::from_utf8(stdout).unwrap(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

#[tokio::test]
async fn an_unmarked_pinned_fixture_returns_before_any_build_or_configuration() {
    let home = tempfile::tempdir().unwrap();
    for marker in [None, Some("collect")] {
        let (status, stdout, _) = run_helper(marker, home.path()).await;
        assert!(status.success());
        assert!(stdout.contains("PRIVACY-HOOK-FIXTURE:NOT-SELECTED"));
        assert!(!stdout.contains("COMPLETE"));
    }
    // Nothing was created in the isolated home.
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn pinned_detector_and_admission_pass_marked_actual_native_runtime() {
    let home = tempfile::tempdir().unwrap();
    let (status, stdout, stderr) = run_helper(Some("suite"), home.path()).await;
    assert!(
        status.success(),
        "pinned detector fixture failed: {stdout} {stderr}"
    );
    assert!(stdout.contains("PRIVACY-HOOK-FIXTURE:COMPLETE refusals=18 malformed=18 admitted=3"));
    assert!(!stdout.contains("NOT-SELECTED"));
}
