//! The real, model-free migration CLI is read-only for originals/config and
//! does not need Rust, an SDK installation, a grant or a component execution.
#![cfg(feature = "gadgets")]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

struct Home {
    _root: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    workspace: PathBuf,
}
impl Home {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let config = home.join("custom-config");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            config.join("config.json"),
            json!({"gadgets":{"enabled":false}}).to_string(),
        )
        .unwrap();
        Self {
            _root: root,
            home,
            config,
            workspace,
        }
    }
    fn command(&self, args: &[&str]) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_craftsman"));
        command
            .args(args)
            .current_dir(&self.workspace)
            .env("HOME", &self.home)
            .env("CRAFTSMAN_CONFIG_DIR", &self.config)
            .env("RUSTUP_HOME", self.home.join("empty-rustup"))
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.env("USERPROFILE", &self.home);
        command
    }
}
async fn run(command: &mut tokio::process::Command) -> std::process::Output {
    tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .expect("model-free migration timed out")
        .unwrap()
}
fn report(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)))
}
fn plugin(base: &std::path::Path, prompt: &str) -> Vec<u8> {
    let root = base.join("plugins");
    std::fs::create_dir_all(&root).unwrap();
    let bytes = json!({"version":1,"name":"review-flow","prompt":prompt,
        "commands":[{"name":"review","prompt":"Review {$ARGS}"}]})
    .to_string()
    .into_bytes();
    std::fs::write(root.join("review.json"), &bytes).unwrap();
    bytes
}

#[tokio::test]
async fn cli_dry_run_preserves_config_originals_and_existing_gadgets() {
    let home = Home::new();
    let original = plugin(&home.config, "Legacy rules");
    let config = std::fs::read(home.config.join("config.json")).unwrap();
    let preview = run(&mut home.command(&["gadgets", "migrate", "--dry-run"])).await;
    assert!(preview.status.success());
    assert_eq!(report(&preview)["migrations"][0]["status"], "would-migrate");
    assert!(!home.config.join("gadgets").exists());
    let installed = run(&mut home.command(&["gadgets", "migrate"])).await;
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert_eq!(report(&installed)["migrations"][0]["status"], "migrated");
    let target = home.config.join("gadgets/review-flow/gadget.toml");
    let before = std::fs::read(&target).unwrap();
    let repeated = run(&mut home.command(&["gadgets", "migrate"])).await;
    assert!(repeated.status.success());
    assert_eq!(
        report(&repeated)["migrations"][0]["status"],
        "already-migrated"
    );
    assert_eq!(std::fs::read(&target).unwrap(), before);
    std::fs::write(
        &target,
        "id='review-flow'\nversion='1.0.0'\nprompt='User changes'\n",
    )
    .unwrap();
    let changed = std::fs::read(&target).unwrap();
    let conflict = run(&mut home.command(&["gadgets", "migrate"])).await;
    assert!(!conflict.status.success());
    assert_eq!(report(&conflict)["complete"], false);
    assert_eq!(report(&conflict)["migrations"][0]["status"], "conflict");
    assert_eq!(std::fs::read(&target).unwrap(), changed);
    assert_eq!(
        std::fs::read(home.config.join("plugins/review.json")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(home.config.join("config.json")).unwrap(),
        config
    );
    assert!(!home.config.join("gadgets/.grants.json").exists());
    assert!(!home.config.join("gadgets/.sdk").exists());
    assert!(!home.config.join("gadgets/review-flow/gadget.wasm").exists());
    assert!(!home.home.join("empty-rustup").exists());
}

#[tokio::test]
async fn cli_requires_explicit_project_migration_and_keeps_both_originals() {
    let home = Home::new();
    let user_original = plugin(&home.config, "User rules");
    let project_original = plugin(&home.workspace.join(".craftsman"), "Project rules");
    assert!(
        run(&mut home.command(&["gadgets", "migrate"]))
            .await
            .status
            .success()
    );
    assert!(!home.workspace.join(".craftsman/gadgets").exists());
    let project = run(&mut home.command(&["gadgets", "migrate", "--project"])).await;
    assert!(project.status.success());
    assert_eq!(report(&project)["project"], true);
    assert!(
        home.workspace
            .join(".craftsman/gadgets/review-flow/gadget.toml")
            .is_file()
    );
    assert_eq!(
        std::fs::read(home.config.join("plugins/review.json")).unwrap(),
        user_original
    );
    assert_eq!(
        std::fs::read(home.workspace.join(".craftsman/plugins/review.json")).unwrap(),
        project_original
    );
}
