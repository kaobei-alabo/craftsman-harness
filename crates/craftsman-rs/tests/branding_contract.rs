//! First-party identity contracts; no provider, user-config or guest execution.

use serde_json::Value;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

#[test]
fn workspace_packages_and_launchers_use_craftsman_identity() {
    assert_eq!(env!("CARGO_PKG_NAME"), "craftsman-rs");
    let workspace = include_str!("../../../Cargo.toml");
    for member in [
        "crates/craftsman-rs",
        "crates/craftsman-harness",
        "crates/craftsman-gadget",
        "crates/craftsman-gadget-macros",
    ] {
        assert!(workspace.contains(&format!("\"{member}\"")));
    }
    assert_eq!(
        Path::new(env!("CARGO_BIN_EXE_craftsman"))
            .file_stem()
            .unwrap(),
        "craftsman"
    );
    assert_eq!(
        Path::new(env!("CARGO_BIN_EXE_craftsman-rs"))
            .file_stem()
            .unwrap(),
        "craftsman-rs"
    );
}

#[test]
fn configuration_and_embedded_gadget_contract_use_current_namespaces() {
    let config = include_str!("../src/config.rs");
    assert!(config.contains("CRAFTSMAN_CONFIG_DIR"));
    assert!(config.contains("CRAFTSMAN_PLATFORM_TOKEN"));
    assert!(config.contains(".join(\".craftsman\")"));
    let wit = include_str!("../../craftsman-gadget/wit/gadget.wit");
    assert!(wit.contains("package craftsman:gadget@0.1.0;"));
    let authoring = include_str!("../src/gadgets/authoring.rs");
    assert!(authoring.contains("crates/craftsman-gadget/Cargo.toml"));
    assert!(authoring.contains("crates/craftsman-gadget-macros/Cargo.toml"));
    assert!(authoring.contains("use craftsman_gadget::{gadget_call, GadgetError};"));
}

#[test]
fn trace_and_headless_schema_resources_use_current_identity() {
    for schema in [
        include_str!("../../../docs/schemas/trace-v1.schema.json"),
        include_str!("../../../docs/schemas/exec-contract-v1.schema.json"),
        include_str!("../../../docs/schemas/exec-result-v1.schema.json"),
        include_str!("../../../docs/schemas/exec-events-v1.schema.json"),
    ] {
        let value: Value = serde_json::from_str(schema).unwrap();
        assert!(
            value["$id"]
                .as_str()
                .unwrap()
                .starts_with("urn:craftsman-harness:")
        );
    }
    let trace: Value =
        serde_json::from_str(include_str!("../../../docs/schemas/trace-v1.schema.json")).unwrap();
    assert_eq!(
        trace["properties"]["schema"]["const"],
        "craftsman-harness/trace@1"
    );

    // Serialized wire vectors must migrate their decoded identities too, not
    // merely their surrounding human-readable JSON fixture labels.
    let vectors: Value =
        serde_json::from_str(include_str!("../../../docs/schemas/exec-v1.vectors.json")).unwrap();
    for case in vectors["wire"].as_array().unwrap() {
        if case["valid"] != true {
            continue;
        }
        let hex = case["hex"].as_str().unwrap();
        assert_eq!(hex.len() % 2, 0);
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect();
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let record: Value = serde_json::from_slice(line).unwrap();
            assert!(
                record["schema"]
                    .as_str()
                    .unwrap()
                    .starts_with("craftsman-harness/")
            );
            if let Some(meta) = record.get("_meta") {
                assert!(meta.get("craftsman").is_some());
            }
        }
    }
}

#[tokio::test]
async fn cli_help_and_version_use_current_name_before_any_config_effect() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("must-remain-absent");
    for argument in ["--version", "--help"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_craftsman"));
        command
            .arg(argument)
            .env_clear()
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("CRAFTSMAN_CONFIG_DIR", &config)
            .kill_on_drop(true);
        // Windows needs its original native system location, not user config.
        #[cfg(windows)]
        for key in ["SYSTEMROOT", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .expect("identity-only CLI invocation exceeded its own new bound")
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        if argument == "--version" {
            assert_eq!(stdout, format!("craftsman {}\n", env!("CARGO_PKG_VERSION")));
        } else {
            assert!(stdout.contains("Usage: craftsman"));
            assert!(stdout.contains("Craftsman platform chat client"));
        }
        assert!(output.stderr.is_empty());
        assert!(!config.exists(), "help/version must not initialize config");
    }
}
