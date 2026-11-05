//! The framework crate must stay free of app-layer dependencies (HTTP, UI,
//! platform SDK). If one creeps into `craftsman-harness/Cargo.toml`, this fails —
//! `craftsman-harness` is meant to be a pure, reusable library that the app
//! (`craftsman-rs`) wires to concrete I/O via the ModelProvider/Backend traits.

#[test]
fn framework_has_no_app_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    for banned in ["reqwest", "craftsman-sdk", "rustyline", "clap", "crossterm"] {
        assert!(
            !manifest.contains(banned),
            "craftsman-harness must not depend on `{banned}` — keep app I/O in craftsman-rs"
        );
    }
}
