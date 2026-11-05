//! Offline pin/config guard for the reviewed48.x security patch. This never
//! compiles/executes a guest or claims absence of every current/future advisory.
use std::fs;
use std::path::Path;

fn field<'a>(package: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key} = \"");
    package
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .and_then(|rest| rest.strip_suffix('"'))
}
fn manifest() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap()
}
fn lock() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock")).unwrap()
}

#[test]
fn direct_engine_and_wasi_use_the_same_reviewed_security_patch_without_capability_widening() {
    let source = manifest();
    for (name, features) in [
        (
            "wasmtime",
            "[\"anyhow\", \"std\", \"runtime\", \"cranelift\", \"component-model\", \"async\", \"cache\", \"parallel-compilation\"]",
        ),
        ("wasmtime-wasi", "[\"p2\"]"),
    ] {
        let prefix = format!("{name} = ");
        let line = source.lines().find(|l| l.starts_with(&prefix)).unwrap();
        assert!(
            line.contains("version = \"=48.0.8\""),
            "engine patch must change only by a reviewed security/compatibility PR: {line}"
        );
        assert!(line.contains("default-features = false") && line.contains("optional = true"));
        assert!(
            line.contains(&format!("features = {features}")),
            "imports/features must not silently widen in a patch"
        );
    }
}
#[test]
fn every_resolved_wasmtime_pulley_package_matches_the_reviewed_patch_and_registry_source() {
    let source = lock();
    let mut count = 0;
    for block in source.split("[[package]]").skip(1) {
        let Some(name) = field(block, "name") else {
            continue;
        };
        if name.starts_with("wasmtime") || name.starts_with("pulley-") {
            count += 1;
            assert_eq!(
                field(block, "version"),
                Some("48.0.8"),
                "mismatched or vulnerable runtime package: {name}"
            );
            assert_eq!(
                field(block, "source"),
                Some("registry+https://github.com/rust-lang/crates.io-index")
            );
            assert!(field(block, "checksum").is_some());
        }
    }
    assert_eq!(
        count, 17,
        "review any runtime-family/lock package addition or removal explicitly"
    );
}
#[test]
fn gadget_model_visibility_is_not_enabled_by_the_engine_security_patch() {
    let source = manifest();
    assert!(source.contains(
        "gadgets = [\"dep:craftsman-gadget\", \"dep:wasmtime\", \"dep:wasmtime-wasi\", \"dep:toml\"]"
    ));
    let config =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gadgets/cli.rs"))
            .unwrap();
    assert!(
        config.contains("pub fn enabled") && config.contains("unwrap_or(false)"),
        "security patch is not model visibility/default-on approval"
    );
}
