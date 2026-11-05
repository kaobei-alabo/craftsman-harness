//! Embedded gadget authoring resources and process-safe Cargo builds.
//! Native build scripts remain shell operations, not WASM capabilities.

use super::manifest::{MAX_COMPONENT_BYTES, MAX_MANIFEST_BYTES, Manifest, valid_id};
use super::{GadgetRuntime, RuntimeOptions};
use crate::config::Config;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const TOOLCHAIN: &str = include_str!("../../../../rust-toolchain.toml");
const TEMPLATE_LOCK: &str =
    include_str!("../../../craftsman-gadget/tests/fixtures/demo/Cargo.lock");
const SKILL: &str = include_str!("assets/build-gadget/SKILL.md");
const SDK_WORKSPACE: &str = "[workspace]\nresolver = \"2\"\nmembers = [\"crates/craftsman-gadget\", \"crates/craftsman-gadget-macros\"]\n";
const SDK_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", SDK_WORKSPACE),
    (
        "crates/craftsman-gadget/Cargo.toml",
        include_str!("../../../craftsman-gadget/Cargo.toml"),
    ),
    (
        "crates/craftsman-gadget/src/lib.rs",
        include_str!("../../../craftsman-gadget/src/lib.rs"),
    ),
    (
        "crates/craftsman-gadget/src/host.rs",
        include_str!("../../../craftsman-gadget/src/host.rs"),
    ),
    (
        "crates/craftsman-gadget/wit/gadget.wit",
        include_str!("../../../craftsman-gadget/wit/gadget.wit"),
    ),
    (
        "crates/craftsman-gadget-macros/Cargo.toml",
        include_str!("../../../craftsman-gadget-macros/Cargo.toml"),
    ),
    (
        "crates/craftsman-gadget-macros/src/lib.rs",
        include_str!("../../../craftsman-gadget-macros/src/lib.rs"),
    ),
    ("skills/build-gadget/SKILL.md", SKILL),
];
const TEMPLATE_SOURCE: &str = r#"use craftsman_gadget::{gadget_call, GadgetError};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input { text: String }

/// Count whitespace-separated words in a text.
#[gadget_call]
fn count_words(args: Input) -> Result<usize, GadgetError> {
    Ok(args.text.split_whitespace().count())
}
"#;

#[derive(Debug, Clone)]
pub struct EmbeddedSdk {
    pub root: PathBuf,
    pub skill: PathBuf,
}

pub fn pinned_toolchain() -> String {
    let document: toml::Value =
        toml::from_str(TOOLCHAIN).expect("embedded toolchain is valid TOML");
    document["toolchain"]["channel"]
        .as_str()
        .expect("exact toolchain pin")
        .to_owned()
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn sdk_id() -> String {
    let mut digest = Sha256::new();
    for (path, contents) in SDK_FILES {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(contents.as_bytes());
        digest.update([0]);
    }
    digest.update(TOOLCHAIN.as_bytes());
    format!("{}-{:x}", env!("CARGO_PKG_VERSION"), digest.finalize())
}
pub(super) fn directory(parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let path = parent.join(name);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!(
            "authoring directory must not be a symlink or non-directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&path)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}
pub(super) fn regular_bytes(path: &Path, maximum: usize) -> anyhow::Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= maximum as u64,
        "expected a bounded regular file: {}",
        path.display()
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    anyhow::ensure!(file.metadata()?.is_file(), "expected a regular file");
    let mut bytes = Vec::new();
    file.take((maximum + 1) as u64).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= maximum, "file exceeds authoring size limit");
    Ok(bytes)
}
pub(super) fn write_new(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

/// Materialize an immutable, content-addressed SDK. Do not overwrite user
/// modifications or follow planted symlinks. Interrupted creation is resumable.
pub fn ensure_sdk(config: &Path) -> anyhow::Result<EmbeddedSdk> {
    fs::create_dir_all(config)?;
    let config = config.canonicalize()?;
    let gadgets = directory(&config, "gadgets")?;
    let sdks = directory(&gadgets, ".sdk")?;
    let root = directory(&sdks, &sdk_id())?;
    for (name, contents) in SDK_FILES {
        let relative = Path::new(name);
        let mut parent = root.clone();
        for part in relative.parent().unwrap().components() {
            parent = directory(
                &parent,
                part.as_os_str().to_str().expect("embedded UTF-8 path"),
            )?;
        }
        let path = root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(_) => anyhow::ensure!(
                regular_bytes(&path, contents.len())? == contents.as_bytes(),
                "embedded SDK file was modified; preserve/remove that SDK copy before retrying: {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_new(&path, contents.as_bytes())?
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(EmbeddedSdk {
        skill: root.join("skills/build-gadget/SKILL.md"),
        root,
    })
}

pub fn builtin_skill(config: &Path) -> anyhow::Result<crate::skills::Skill> {
    let sdk = ensure_sdk(config)?;
    Ok(crate::skills::Skill {
        name: "build-gadget".into(),
        description:
            "Build a reusable typed WASM gadget when existing tools are missing a local capability."
                .into(),
        path: sdk.skill,
    })
}

pub fn scaffold(config: &Path, workspace: &Path, id: &str, project: bool) -> anyhow::Result<Value> {
    anyhow::ensure!(
        valid_id(id),
        "gadget id must match its directory and use 2-64 lowercase ASCII letters/digits/hyphens"
    );
    let sdk = ensure_sdk(config)?;
    let base = if project {
        let root = workspace.canonicalize()?;
        let craftsman = directory(&root, ".craftsman")?;
        directory(&craftsman, "gadgets")?
    } else {
        config.canonicalize()?.join("gadgets")
    };
    let root = base.join(id);
    anyhow::ensure!(
        fs::symlink_metadata(&root).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "gadget directory already exists; no files were overwritten: {}",
        root.display()
    );
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&root)?;
    let source = directory(&root, "src")?;
    let package = format!("craftsman-gadget-{id}");
    let sdk_path = sdk.root.join("crates/craftsman-gadget");
    let sdk_path = sdk_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("SDK dependency path must be UTF-8"))?;
    let manifest = json!({"package":{"name":package,"version":"0.1.0","edition":"2024","publish":false},
        "lib":{"crate-type":["cdylib"]}, "workspace":{}, "dependencies":{
            "craftsman-gadget":{"path":sdk_path}, "serde":{"version":"1","features":["derive"]}, "serde_json":"1", "schemars":"1"}});
    // Serialize through TOML rather than hand-quoting Windows or spaced paths.
    let manifest: toml::Value = serde_json::from_value(manifest)?;
    write_new(
        &root.join("Cargo.toml"),
        toml::to_string_pretty(&manifest)?.as_bytes(),
    )?;
    write_new(
        &root.join("Cargo.lock"),
        TEMPLATE_LOCK
            .replace(
                "name = \"gadget-fixture\"",
                &format!("name = \"{package}\""),
            )
            .as_bytes(),
    )?;
    write_new(&root.join("rust-toolchain.toml"), TOOLCHAIN.as_bytes())?;
    write_new(&source.join("lib.rs"), TEMPLATE_SOURCE.as_bytes())?;
    let gadget = Manifest {
        id: id.to_owned(),
        version: "0.1.0".into(),
        description: format!("Local {id} gadget."),
        capabilities: Default::default(),
        prompt: None,
        commands: vec![],
        agents: vec![],
        migration: None,
    };
    write_new(
        &root.join("gadget.toml"),
        toml::to_string_pretty(&gadget)?.as_bytes(),
    )?;
    Ok(
        json!({"gadget":id,"source":root,"sdk":sdk.root,"skill":sdk.skill,"toolchain":pinned_toolchain(),"status":"scaffolded"}),
    )
}

fn gadget_root(
    config: &Path,
    workspace: &Path,
    id: &str,
    project: bool,
) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(valid_id(id), "invalid gadget id");
    let base = if project {
        workspace.join(".craftsman/gadgets")
    } else {
        config.join("gadgets")
    };
    let path = base.join(id);
    let meta = fs::symlink_metadata(&path)?;
    anyhow::ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "gadget source must not be a symlink"
    );
    Ok(path.canonicalize()?)
}
fn artifact_name(cargo: &toml::Value) -> anyhow::Result<String> {
    let name = cargo
        .get("lib")
        .and_then(|v| v.get("name"))
        .or_else(|| cargo.get("package").and_then(|v| v.get("name")))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Cargo.toml needs a package/lib name"))?;
    anyhow::ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
        "invalid Cargo library name"
    );
    Ok(name.replace('-', "_"))
}

pub async fn build(
    cfg: &Config,
    workspace: &Path,
    id: &str,
    project: bool,
) -> anyhow::Result<Value> {
    let config = crate::config::config_dir();
    let root = gadget_root(&config, workspace, id, project)?;
    // Verify the shipped SDK copy on every native build too, never silently
    // compile a modified embedded procedural macro or overwrite user changes.
    ensure_sdk(&config)?;
    let text = regular_bytes(&root.join("gadget.toml"), MAX_MANIFEST_BYTES)?;
    Manifest::parse(std::str::from_utf8(&text)?, id)?;
    let text = regular_bytes(&root.join("Cargo.toml"), MAX_MANIFEST_BYTES)?;
    let cargo: toml::Value = toml::from_str(std::str::from_utf8(&text)?)?;
    let name = artifact_name(&cargo)?;
    regular_bytes(&root.join("Cargo.lock"), 1024 * 1024)?;
    let runner =
        crate::tools::ShellRunner::new(root.clone(), crate::shell::ShellSettings::from_config(cfg));
    let command = format!(
        "cargo +{} build --locked --release --lib --target wasm32-wasip2 --manifest-path Cargo.toml --target-dir target --color never",
        pinned_toolchain()
    );
    let outcome = runner.run_outcome(&command, None).await;
    anyhow::ensure!(
        !outcome.is_error && outcome.meta.exit_code == Some(0),
        "gadget build failed; previous component retained: {}",
        outcome.content
    );
    let artifact = root
        .join("target/wasm32-wasip2/release")
        .join(format!("{name}.wasm"));
    let bytes = regular_bytes(&artifact, MAX_COMPONENT_BYTES)?;
    let compiled_hash = hash(&bytes);
    let check_bytes = bytes.clone();
    let check_workspace = workspace.to_path_buf();
    let check_config = config.clone();
    tokio::task::spawn_blocking(move || {
        let runtime = GadgetRuntime::new(
            &check_workspace,
            &check_config,
            RuntimeOptions {
                disk_cache: false,
                ..Default::default()
            },
        )?;
        runtime.validate_component(&check_bytes)
    })
    .await??;
    // Recheck the declaration before publication; no guest export is invoked.
    let current = regular_bytes(&root.join("gadget.toml"), MAX_MANIFEST_BYTES)?;
    Manifest::parse(std::str::from_utf8(&current)?, id)?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = root.join(format!(
        ".gadget.wasm.tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    write_new(&temporary, &bytes)?;
    let published = root.join("gadget.wasm");
    if let Err(error) = fs::rename(&temporary, &published) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(
        json!({"gadget":id,"source":root,"artifact":published,"sha256":compiled_hash,"toolchain":pinned_toolchain(),
        "status":"built","approval":"checked on the next catalog refresh or call","output":outcome.content}),
    )
}

pub async fn doctor(cfg: &Config, workspace: &Path) -> anyhow::Result<Value> {
    let pin = pinned_toolchain();
    let runner = crate::tools::ShellRunner::new(
        workspace.to_path_buf(),
        crate::shell::ShellSettings::from_config(cfg),
    );
    // Query installed toolchains first. Selecting an absent toolchain can
    // trigger rustup auto-installation even for a target-list query.
    let readonly = |command: &str| {
        if cfg!(windows) {
            format!("$env:RUSTUP_AUTO_INSTALL='0'; {command}")
        } else {
            format!("RUSTUP_AUTO_INSTALL=0 {command}")
        }
    };
    let toolchains = runner
        .run_outcome(&readonly("rustup toolchain list"), None)
        .await;
    let present = !toolchains.is_error
        && toolchains.meta.exit_code == Some(0)
        && toolchains
            .content
            .lines()
            .any(|l| l.trim().starts_with(&format!("{pin}-")) || l.trim() == pin);
    let target = if present {
        Some(
            runner
                .run_outcome(
                    &readonly(&format!("rustup target list --installed --toolchain {pin}")),
                    None,
                )
                .await,
        )
    } else {
        None
    };
    let installed = target.as_ref().is_some_and(|r| {
        !r.is_error
            && r.meta.exit_code == Some(0)
            && r.content.lines().any(|l| l.trim() == "wasm32-wasip2")
    });
    let fix = if installed {
        Value::Null
    } else if present {
        json!(format!("rustup target add wasm32-wasip2 --toolchain {pin}"))
    } else {
        json!(format!(
            "rustup toolchain install {pin} --profile minimal --target wasm32-wasip2"
        ))
    };
    Ok(
        json!({"toolchain":pin,"toolchain_installed":present,"wasm_target":"wasm32-wasip2", "target_installed":installed,
        "sdk_embedded":true,"fix":fix,"details":target.map(|t|t.content).unwrap_or(toolchains.content)}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_sdk_is_stable_and_never_overwrites_modified_sources() {
        let config = tempfile::tempdir().unwrap();
        let first = ensure_sdk(config.path()).unwrap();
        let again = ensure_sdk(config.path()).unwrap();
        assert_eq!(first.root, again.root);
        assert!(first.skill.is_file());
        for (name, contents) in SDK_FILES {
            assert_eq!(
                fs::read(first.root.join(name)).unwrap(),
                contents.as_bytes()
            );
        }
        let source = first.root.join("crates/craftsman-gadget/src/lib.rs");
        fs::write(&source, "user changes").unwrap();
        assert!(ensure_sdk(config.path()).is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "user changes");
    }

    #[test]
    fn scaffold_is_locked_standalone_typed_and_create_only() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let created = scaffold(config.path(), workspace.path(), "word-count", false).unwrap();
        let root = PathBuf::from(created["source"].as_str().unwrap());
        let cargo: toml::Value =
            toml::from_str(&fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
        assert_eq!(
            cargo["package"]["name"].as_str(),
            Some("craftsman-gadget-word-count")
        );
        assert!(cargo.get("workspace").is_some());
        assert!(
            Path::new(
                cargo["dependencies"]["craftsman-gadget"]["path"]
                    .as_str()
                    .unwrap()
            )
            .join("wit/gadget.wit")
            .is_file()
        );
        let lock: toml::Value =
            toml::from_str(&fs::read_to_string(root.join("Cargo.lock")).unwrap()).unwrap();
        assert!(
            lock["package"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"].as_str() == Some("craftsman-gadget-word-count"))
        );
        let source = root.join("src/lib.rs");
        let before = fs::read(&source).unwrap();
        assert!(scaffold(config.path(), workspace.path(), "word-count", false).is_err());
        assert_eq!(fs::read(&source).unwrap(), before);
        assert!(scaffold(config.path(), workspace.path(), "../bad", false).is_err());
        assert!(
            !root.join("gadget.wasm").exists(),
            "scaffolding must not run or publish a component"
        );
        let project = scaffold(config.path(), workspace.path(), "project-counter", true).unwrap();
        assert!(
            Path::new(project["source"].as_str().unwrap())
                .starts_with(workspace.path().canonicalize().unwrap())
        );
    }

    #[cfg(unix)]
    #[test]
    fn authoring_does_not_follow_planted_sdk_symlinks() {
        let config = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(config.path().join("gadgets")).unwrap();
        std::os::unix::fs::symlink(outside.path(), config.path().join("gadgets/.sdk")).unwrap();
        assert!(ensure_sdk(config.path()).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
