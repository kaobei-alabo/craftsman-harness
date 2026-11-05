use std::path::Path;
use std::sync::OnceLock;

pub fn fixture() -> &'static Vec<u8> {
    static BUILT: OnceLock<Vec<u8>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let target = root.join("target/gadget-fixtures/headless-host");
        let output = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "--locked",
                "--release",
                "--target",
                "wasm32-wasip2",
                "--manifest-path",
            ])
            .arg(root.join("crates/craftsman-rs/tests/fixtures/headless-gadget-host/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "headless fixture build failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::read(target.join("wasm32-wasip2/release/headless_gadget_fixture.wasm")).unwrap()
    })
}
