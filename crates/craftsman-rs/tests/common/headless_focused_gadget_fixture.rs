//! Source inputs for focused guest tests, not compiled-component/approval caches.
use std::path::Path;
use std::sync::OnceLock;

pub struct Inputs {
    pub original: &'static [u8],
    pub focused: Vec<u8>,
}

/// Build BOTH source inputs before returning any headless fixture. Otherwise a
/// late first-use Cargo/LTO build can compete with another test's already-active
/// guest budget. Wasmtime compilation still happens through the real host load,
/// under its unchanged 30s budget; no native-code cache or guest warmup is added.
pub fn inputs() -> &'static Inputs {
    static BUILT: OnceLock<Inputs> = OnceLock::new();
    BUILT.get_or_init(|| {
        let original = super::fixture::fixture().as_slice();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let target = root.join("target/gadget-fixtures/headless-focused");
        let output = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "--locked",
                "--release",
                "--target",
                "wasm32-wasip2",
                "--manifest-path",
            ])
            .arg(root.join("crates/craftsman-rs/tests/fixtures/headless-gadget-focused/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "focused headless fixture source build failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let focused = std::fs::read(
            target.join("wasm32-wasip2/release/headless_gadget_focused_fixture.wasm"),
        )
        .unwrap();
        assert!(
            focused.len() < original.len(),
            "focused test source must not grow beyond the broad input"
        );
        Inputs { original, focused }
    })
}
