//! Newly owned native children; no parent environment or installed gadgets.
use super::*;
use craftsman_harness::{SessionCancellation, ToolErrorKind};
use std::io::Write;
const MARKER: &str = "craftsman-owned-native-v1=";
const CASE: &str = "tools::owned_cancellation_tests::native_child";

pub(crate) fn marker_directory() -> Option<PathBuf> {
    let args: Vec<_> = std::env::args_os().collect();
    let values: Vec<_> = args
        .windows(2)
        .filter(|pair| pair[0] == "--skip")
        .filter_map(|pair| pair[1].to_str()?.strip_prefix(MARKER).map(PathBuf::from))
        .collect();
    if values.len() != 1 {
        return None;
    }
    let path = values[0].clone();
    if !path.is_absolute() {
        return None;
    }
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return None;
    }
    if std::fs::read(path.join("plan")).ok()?.as_slice() != b"owned-native-v1" {
        return None;
    }
    Some(path)
}

fn quote(text: &str) -> String {
    if cfg!(windows) {
        format!("'{}'", text.replace('\'', "''"))
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}
pub(crate) fn command(directory: &Path) -> String {
    command_for(directory, CASE)
}

fn command_for(directory: &Path, case: &str) -> String {
    let binary = std::env::current_exe().unwrap();
    let marker = format!("{MARKER}{}", directory.to_str().unwrap());
    // The trailing operation keeps the shell as an actual parent; the native
    // helper is its descendant, like Cargo beneath the authoring CLI.
    format!(
        "{}{} --ignored --exact {} --nocapture --skip {}; {}",
        if cfg!(windows) { "& " } else { "" },
        quote(binary.to_str().unwrap()),
        quote(case),
        quote(&marker),
        if cfg!(windows) { "[void]0" } else { ":" }
    )
}

#[cfg(feature = "gadgets")]
#[path = "../../tests/common/gadget_fixture.rs"]
mod gadget_fixture;

#[cfg(feature = "gadgets")]
#[tokio::test]
#[ignore = "owned Cargo build fixture; rejects unmarked runs before configuration"]
async fn gadget_build_child() {
    let Some(directory) = marker_directory() else {
        return;
    };
    let cfg = crate::config::Config::load().unwrap().unwrap();
    crate::gadgets::authoring::build(&cfg, &directory, "cancel-build", false)
        .await
        .unwrap();
}

#[cfg(feature = "gadgets")]
#[tokio::test]
async fn cancelling_a_real_cargo_gadget_build_preserves_the_source_built_component() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("plan"), b"owned-native-v1").unwrap();
    let config = directory.path().join("config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(
        config.join("config.json"),
        serde_json::json!({"shell":{"timeout_secs":240}}).to_string(),
    )
    .unwrap();
    let created =
        crate::gadgets::authoring::scaffold(&config, directory.path(), "cancel-build", false)
            .unwrap();
    let source = PathBuf::from(created["source"].as_str().unwrap());
    let previous = gadget_fixture::fixtures().0.clone();
    std::fs::write(source.join("gadget.wasm"), &previous).unwrap();
    let ready = directory.path().join("ready");
    let late = directory.path().join("late");
    std::fs::write(source.join("build.rs"), format!("fn main() {{ std::fs::write({:?}, std::process::id().to_string()).unwrap(); std::thread::sleep(std::time::Duration::from_millis(1500)); std::fs::write({:?}, b\"build script completed\").unwrap(); }}", ready.to_str().unwrap(), late.to_str().unwrap())).unwrap();
    let helper = command_for(
        directory.path(),
        "tools::owned_cancellation_tests::gadget_build_child",
    );
    let command = if cfg!(windows) {
        format!(
            "$env:CRAFTSMAN_CONFIG_DIR={}; {}",
            quote(config.to_str().unwrap()),
            helper
        )
    } else {
        format!(
            "export CRAFTSMAN_CONFIG_DIR={}; {}",
            quote(config.to_str().unwrap()),
            helper
        )
    };
    let runner = ShellRunner::new(
        directory.path().into(),
        ShellSettings {
            timeout_secs: 300,
            ..Default::default()
        },
    );
    let turn = SessionCancellation::new().new_turn();
    let running_turn = turn.clone();
    let task =
        tokio::spawn(async move { running_turn.scope(runner.run_outcome(&command, None)).await });
    // The original G3 cold-build budget is retained; this marker is emitted by
    // the real compiled build script, not by the launcher or parent.
    tokio::time::timeout(Duration::from_secs(240), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Cargo must actually reach the build script");
    turn.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Cancelled));
    assert!(!late.exists());
    assert_eq!(std::fs::read(source.join("gadget.wasm")).unwrap(), previous);
    assert!(!config.join("gadgets/.grants.json").exists());
}

#[test]
#[ignore = "owned native fixture; returns before environment/config when unmarked"]
fn native_child() {
    let Some(directory) = marker_directory() else {
        return;
    };
    std::io::stdout().write_all(b"native-early-out\n").unwrap();
    std::io::stdout().flush().unwrap();
    std::io::stderr().write_all(b"native-early-err\n").unwrap();
    std::io::stderr().flush().unwrap();
    std::fs::write(directory.join("ready"), std::process::id().to_string()).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    std::fs::write(directory.join("late"), b"actual child reached completion").unwrap();
    std::io::stdout().write_all(b"native-late-out\n").unwrap();
    std::io::stdout().flush().unwrap();
}

pub(crate) async fn ready(directory: &Path) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !directory.join("ready").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the actual native helper must reach its ready marker");
}

async fn cancelled_native(format: OutputFormat) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("plan"), b"owned-native-v1").unwrap();
    let settings = ShellSettings {
        output_format: format,
        timeout_secs: 30,
        ..Default::default()
    };
    assert!(!settings.process_group);
    let runner = ShellRunner::new(directory.path().into(), settings);
    let turn = SessionCancellation::new().new_turn();
    let cmd = command(directory.path());
    let task_turn = turn.clone();
    let running =
        tokio::spawn(async move { task_turn.scope(runner.run_outcome(&cmd, None)).await });
    ready(directory.path()).await;
    turn.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(8), running)
        .await
        .unwrap()
        .unwrap();
    assert!(outcome.is_error);
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Cancelled));
    assert!(outcome.meta.exit_code.is_none());
    assert!(outcome.meta.duration.is_some());
    assert!(outcome.meta.bytes.unwrap() >= 32);
    assert!(
        !directory.path().join("late").exists(),
        "the descendant must not complete during the owned drain"
    );
    assert!(!outcome.content.contains("timed out"));
}

#[tokio::test]
async fn scoped_legacy_command_cancels_its_descendant_and_keeps_typed_actual_facts() {
    cancelled_native(OutputFormat::Legacy).await;
}

#[tokio::test]
async fn scoped_head_tail_command_cancels_its_descendant_and_keeps_typed_actual_facts() {
    cancelled_native(OutputFormat::HeadTail).await;
}

#[tokio::test]
async fn cancelled_before_spawn_has_no_fabricated_process_status_or_byte_count() {
    let turn = SessionCancellation::new().new_turn();
    turn.cancel();
    let runner = ShellRunner::new(PathBuf::from("."), ShellSettings::default());
    let outcome = turn
        .scope(runner.run_outcome("must-not-execute", None))
        .await;
    assert_eq!(outcome.meta.error_kind, Some(ToolErrorKind::Cancelled));
    assert!(outcome.meta.exit_code.is_none());
    assert!(outcome.meta.bytes.is_none());
}

#[tokio::test]
async fn a_new_turn_after_stop_executes_normally_under_the_same_session() {
    let session = SessionCancellation::new();
    session.new_turn().cancel();
    let runner = ShellRunner::new(PathBuf::from("."), ShellSettings::default());
    let command = if cfg!(windows) {
        "[Console]::Write('fresh')"
    } else {
        "printf fresh"
    };
    let outcome = session
        .new_turn()
        .scope(runner.run_outcome(command, None))
        .await;
    assert!(!outcome.is_error);
    assert_eq!(outcome.meta.exit_code, Some(0));
    assert_eq!(outcome.content, "fresh");
}
