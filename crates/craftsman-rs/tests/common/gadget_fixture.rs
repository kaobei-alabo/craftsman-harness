use std::path::Path;
use std::sync::OnceLock;

pub fn fixtures() -> &'static (Vec<u8>, Vec<u8>) {
    static BUILT: OnceLock<(Vec<u8>, Vec<u8>)> = OnceLock::new();
    BUILT.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let target_root = root.join("target/gadget-fixtures");
        let mut built = Vec::new();
        for fixture in ["demo", "demo-v2"] {
            // A cdylib's output filename has no package-version hash. Keep
            // variants in separate Cargo targets so incremental builds cannot
            // mistake a newer variant's artifact for an older one's output.
            let target = target_root.join(fixture);
            let output = std::process::Command::new(env!("CARGO"))
                .arg("build")
                .arg("--locked")
                .arg("--release")
                .arg("--target")
                .arg("wasm32-wasip2")
                .arg("--manifest-path")
                .arg(root.join(format!(
                    "crates/craftsman-gadget/tests/fixtures/{fixture}/Cargo.toml"
                )))
                .arg("--target-dir")
                .arg(&target)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "fixture build failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            built.push(
                std::fs::read(target.join("wasm32-wasip2/release/gadget_fixture.wasm")).unwrap(),
            );
        }
        let pair = (built.remove(0), built.remove(0));
        assert_ne!(
            pair.0, pair.1,
            "rebuild fixtures must be genuinely different components"
        );
        pair
    })
}
