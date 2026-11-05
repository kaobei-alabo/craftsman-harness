use super::*;
use crate::exec::permissions::{Mode, Selectors};

struct Fixture {
    root: tempfile::TempDir,
    home: tempfile::TempDir,
    fs: ConfinedFs,
}

fn policy(allow: &[&str], deny: &[&str], mode: Mode) -> Arc<InvocationPermissions> {
    Arc::new(
        InvocationPermissions::from_explicit(
            Selectors {
                mode,
                allow: allow.iter().map(|s| (*s).to_owned()).collect(),
                deny: deny.iter().map(|s| (*s).to_owned()).collect(),
                ..Selectors::default()
            },
            &[],
        )
        .unwrap(),
    )
}

impl Fixture {
    fn new(allow: &[&str], deny: &[&str]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let fs = ConfinedFs::new(
            root.path(),
            &home.path().join("config"),
            policy(allow, deny, Mode::Manual),
        )
        .unwrap();
        Self { root, home, fs }
    }
}

#[tokio::test]
async fn create_read_edit_list_glob_grep_round_trip() {
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    f.fs.write("src/a.rs", "one\ntwo").await.unwrap();
    assert_eq!(f.fs.read("src/a.rs", 1, 100).await.unwrap(), "     2\ttwo");
    f.fs.edit("src/a.rs", "two", "three", false).await.unwrap();
    assert_eq!(f.fs.ls("src").await.unwrap()[0].path, "a.rs");
    assert_eq!(f.fs.glob("src/*.rs").await.unwrap(), ["src/a.rs"]);
    assert_eq!(
        f.fs.grep("three", None).await.unwrap(),
        "src/a.rs:2:three\n"
    );
    assert!(
        f.fs.write("src/a.rs", "overwrite")
            .await
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    assert!(!f.fs.supports_execution());
    assert!(f.fs.execute("echo no").await.is_err());
}

fn failed<T: std::fmt::Debug>(result: std::result::Result<T, AgentError>, error: FsError) {
    assert_eq!(
        result.unwrap_err().to_string(),
        AgentError::from(error).to_string()
    );
}

#[tokio::test]
async fn absolute_inside_paths_are_capability_relative_and_escapes_never_run() {
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    let inside =
        f.fs.root()
            .join("inside.txt")
            .to_string_lossy()
            .into_owned();
    f.fs.write(&inside, "inside").await.unwrap();
    assert!(f.fs.read(&inside, 0, 10).await.unwrap().contains("inside"));
    let outside = f.home.path().join("outside.txt");
    std::fs::write(&outside, "OUTSIDE-SECRET").unwrap();
    let path = outside.to_string_lossy();
    failed(f.fs.read(&path, 0, 10).await, FsError::OutsideRoot);
    failed(f.fs.write(&path, "changed").await, FsError::OutsideRoot);
    failed(
        f.fs.edit(&path, "SECRET", "changed", false).await,
        FsError::OutsideRoot,
    );
    failed(f.fs.ls(&path).await, FsError::OutsideRoot);
    failed(f.fs.grep("SECRET", Some(&path)).await, FsError::OutsideRoot);
    for path in [
        "../outside.txt",
        "nested/../../outside.txt",
        "C:drive-relative",
        "safe\u{9b}",
        "NUL.txt",
        "a./x",
        "a /x",
    ] {
        failed(f.fs.read(path, 0, 10).await, FsError::InvalidPath);
        failed(f.fs.write(path, "changed").await, FsError::InvalidPath);
    }
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "OUTSIDE-SECRET");
    assert!(f.fs.glob("../*").await.is_err());
    assert!(!f.root.path().join("nested").exists());
}

#[tokio::test]
async fn missing_tool_permission_and_plan_cannot_create_parents() {
    let f = Fixture::new(&[], &[]);
    failed(
        f.fs.write("new/sub/file.txt", "no").await,
        FsError::Unapproved,
    );
    assert!(!f.root.path().join("new").exists());
    let fs = ConfinedFs::new(
        f.root.path(),
        &f.home.path().join("cfg"),
        policy(
            &["tool:write_file", "tool:edit_file", "path:Cargo.toml"],
            &[],
            Mode::Plan,
        ),
    )
    .unwrap();
    failed(fs.write("Cargo.toml", "no").await, FsError::ToolDenied);
    failed(fs.write("src/new.txt", "no").await, FsError::ToolDenied);
    assert!(!f.root.path().join("src").exists());
}

#[tokio::test]
async fn credentials_config_and_internal_artifacts_are_hidden_from_every_operation() {
    let root = tempfile::tempdir().unwrap();
    let cfg = root.path().join("user_config");
    std::fs::create_dir(&cfg).unwrap();
    std::fs::write(cfg.join("token.txt"), "SYNTHETIC-SECRET").unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    for path in [".env", "src/.ENV.local", ".craftsman-exec-tmp-spy"] {
        std::fs::write(root.path().join(path), "SYNTHETIC-SECRET").unwrap();
    }
    std::fs::write(root.path().join("ok.txt"), "public needle").unwrap();
    let fs = ConfinedFs::new(
        root.path(),
        &cfg,
        policy(&["tool:write_file", "tool:edit_file"], &[], Mode::Manual),
    )
    .unwrap();
    for path in [
        "user_config/token.txt",
        ".env",
        "src/.ENV.local",
        ".craftsman-exec-tmp-spy",
    ] {
        failed(fs.read(path, 0, 10).await, FsError::HardDenied);
        failed(fs.write(path, "no").await, FsError::HardDenied);
        failed(
            fs.edit(path, "SECRET", "no", false).await,
            FsError::HardDenied,
        );
        failed(fs.grep("SECRET", Some(path)).await, FsError::HardDenied);
    }
    failed(fs.ls("user_config").await, FsError::HardDenied);
    let names = fs
        .ls(".")
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect::<Vec<_>>();
    assert_eq!(names, ["ok.txt", "src/"]);
    assert_eq!(fs.glob("**").await.unwrap(), ["ok.txt"]);
    assert!(
        !fs.grep("SECRET", None)
            .await
            .unwrap()
            .contains("SYNTHETIC-SECRET")
    );
    assert_eq!(
        fs.grep("needle", Some("ok.txt")).await.unwrap(),
        "ok.txt:1:public needle\n"
    );
    std::fs::rename(&cfg, root.path().join("alias")).unwrap();
    failed(fs.read("alias/token.txt", 0, 10).await, FsError::HardDenied);
    assert!(!fs.ls(".").await.unwrap().iter().any(|f| f.path == "alias/"));
}

#[tokio::test]
async fn hard_denied_git_and_craftsman_writes_cannot_be_overridden() {
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    for path in [
        ".git/config",
        ".CRAFTSMAN/gadgets/.grants.json",
        "nested/.git/config",
    ] {
        failed(f.fs.write(path, "no").await, FsError::HardDenied);
    }
    assert!(!f.root.path().join(".git").exists());
    assert!(!f.root.path().join("nested").exists());
}

#[tokio::test]
async fn protected_ci_build_and_automation_writes_require_both_exact_path_and_tool() {
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    for path in [
        ".github/workflows/ci.yml",
        ".gitlab-ci.yml",
        "azure-pipelines.yml",
        "Jenkinsfile",
        "build.rs",
        "build.sh",
        "build.ps1",
        "install.sh",
        "install.ps1",
        "Makefile",
        "GNUmakefile",
        "CMakeLists.txt",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        ".superset/config.json",
        ".vscode/tasks.json",
        ".devcontainer/devcontainer.json",
    ] {
        failed(f.fs.write(path, "no").await, FsError::ProtectedWrite);
    }
    assert!(std::fs::read_dir(f.root.path()).unwrap().next().is_none());
    let fs = ConfinedFs::new(
        f.root.path(),
        &f.home.path().join("cfg"),
        policy(
            &[
                "tool:write_file",
                "tool:edit_file",
                "path:Cargo.toml",
                "path:.github/workflows/ci.yml",
            ],
            &[],
            Mode::Manual,
        ),
    )
    .unwrap();
    fs.write("Cargo.toml", "before").await.unwrap();
    fs.edit("Cargo.toml", "before", "after", false)
        .await
        .unwrap();
    fs.write(".github/workflows/ci.yml", "allowed")
        .await
        .unwrap();
    failed(
        fs.write("nested/Cargo.toml", "no").await,
        FsError::ProtectedWrite,
    );
    failed(
        fs.write(".github/workflows/other.yml", "no").await,
        FsError::ProtectedWrite,
    );
    let fs = ConfinedFs::new(
        f.root.path(),
        &f.home.path().join("cfg"),
        policy(&["path:Cargo.toml"], &[], Mode::Manual),
    )
    .unwrap();
    failed(
        fs.edit("Cargo.toml", "after", "no", false).await,
        FsError::Unapproved,
    );
}

#[tokio::test]
async fn explicit_tool_and_case_conservative_path_denies_win_over_allows() {
    let f = Fixture::new(
        &["tool:write_file", "tool:edit_file", "path:Cargo.toml"],
        &["path:cargo.toml", "path:secret.txt"],
    );
    std::fs::write(f.root.path().join("secret.txt"), "needle SECRET").unwrap();
    failed(f.fs.write("Cargo.toml", "no").await, FsError::PathDenied);
    failed(f.fs.read("secret.txt", 0, 10).await, FsError::PathDenied);
    assert!(f.fs.glob("**").await.unwrap().is_empty());
    assert!(f.fs.ls(".").await.unwrap().is_empty());
    assert!(
        !f.fs
            .grep("SECRET", None)
            .await
            .unwrap()
            .contains("needle SECRET")
    );
    let fs = ConfinedFs::new(
        f.root.path(),
        &f.home.path().join("cfg"),
        policy(
            &["tool:write_file", "path:Cargo.toml"],
            &[
                "tool:write_file",
                "tool:ls",
                "tool:glob",
                "tool:grep",
                "tool:read_file",
            ],
            Mode::Manual,
        ),
    )
    .unwrap();
    failed(fs.write("Cargo.toml", "no").await, FsError::ToolDenied);
    failed(fs.read("secret.txt", 0, 10).await, FsError::ToolDenied);
    failed(fs.ls(".").await, FsError::ToolDenied);
    failed(fs.glob("**").await, FsError::ToolDenied);
    failed(fs.grep("x", None).await, FsError::ToolDenied);
}

#[test]
fn selectors_cannot_name_directories_or_ambiguous_configuration_scopes() {
    let f = Fixture::new(&[], &[]);
    std::fs::create_dir(f.root.path().join("folder")).unwrap();
    for allow in [true, false] {
        let p = if allow {
            policy(&["path:folder"], &[], Mode::Manual)
        } else {
            policy(&[], &["path:folder"], Mode::Manual)
        };
        assert!(matches!(
            ConfinedFs::new(f.root.path(), &f.home.path().join("cfg"), p),
            Err(FsError::InvalidSelector)
        ));
    }
    assert!(matches!(
        ConfinedFs::new(f.root.path(), f.root.path(), policy(&[], &[], Mode::Manual)),
        Err(FsError::InvalidScope)
    ));
    assert!(matches!(
        ConfinedFs::new(
            f.root.path(),
            &f.root.path().join("missing/../cfg"),
            policy(&[], &[], Mode::Manual)
        ),
        Err(FsError::InvalidScope)
    ));
    assert!(
        ConfinedFs::new(
            f.root.path(),
            &f.root.path().join("missing/cfg"),
            policy(&["path:missing/cfg/key.json"], &[], Mode::Manual)
        )
        .is_err()
    );
    assert!(
        ConfinedFs::new(
            Path::new("."),
            &f.home.path().join("cfg"),
            policy(&[], &[], Mode::Manual)
        )
        .is_err()
    );
}

#[tokio::test]
async fn future_denied_file_names_cannot_be_turned_into_parent_directories() {
    let f = Fixture::new(&["tool:write_file"], &["path:future"]);
    failed(
        f.fs.write("future/sub/file.txt", "no").await,
        FsError::PathDenied,
    );
    assert!(!f.root.path().join("future").exists());
}

#[tokio::test]
async fn hard_link_aliases_are_never_read_edited_or_discovered() {
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    let secret = f.home.path().join("outside.txt");
    std::fs::write(&secret, "SYNTHETIC-OUTSIDE-SECRET").unwrap();
    std::fs::hard_link(&secret, f.root.path().join("alias.txt")).unwrap();
    failed(f.fs.read("alias.txt", 0, 10).await, FsError::HardLink);
    failed(
        f.fs.edit("alias.txt", "SECRET", "no", false).await,
        FsError::HardLink,
    );
    failed(
        f.fs.grep("SECRET", Some("alias.txt")).await,
        FsError::HardLink,
    );
    failed(f.fs.write("alias.txt", "no").await, FsError::Exists);
    assert!(f.fs.ls(".").await.unwrap().is_empty());
    assert!(f.fs.glob("**").await.unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(secret).unwrap(),
        "SYNTHETIC-OUTSIDE-SECRET"
    );
}

#[tokio::test]
async fn edit_requires_nonempty_unique_existing_text_and_preserves_failed_targets() {
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    f.fs.write("edit.txt", "a a").await.unwrap();
    failed(
        f.fs.edit("edit.txt", "", "x", false).await,
        FsError::EmptyEdit,
    );
    failed(
        f.fs.edit("edit.txt", "absent", "x", true).await,
        FsError::NoMatch,
    );
    failed(
        f.fs.edit("edit.txt", "a", "x", false).await,
        FsError::NonUnique,
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("edit.txt")).unwrap(),
        "a a"
    );
    f.fs.edit("edit.txt", "a", "x", true).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("edit.txt")).unwrap(),
        "x x"
    );
    assert!(
        !f.fs
            .ls(".")
            .await
            .unwrap()
            .iter()
            .any(|e| e.path.starts_with(TEMP_PREFIX))
    );
}

#[tokio::test]
async fn own_concurrent_writes_and_edits_do_not_overwrite_each_other() {
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    let (a, b) = tokio::join!(
        f.fs.write("same.txt", "first"),
        f.fs.write("same.txt", "second")
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    f.fs.write("edit.txt", "original").await.unwrap();
    let (a, b) = tokio::join!(
        f.fs.edit("edit.txt", "original", "first", false),
        f.fs.edit("edit.txt", "original", "second", false)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        std::fs::read_to_string(f.root.path().join("edit.txt"))
            .unwrap()
            .as_str(),
        "first" | "second"
    ));
}

#[tokio::test]
async fn detected_external_edit_and_temporary_rebinding_are_not_published() {
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    f.fs.write("edit.txt", "original").await.unwrap();
    let path = f.fs.root().join("edit.txt");
    *f.fs.inner.hook.lock().unwrap() = Some(Arc::new(move |stage, target| {
        if stage == "publish" && target == "edit.txt" {
            std::fs::write(&path, "external writer").unwrap();
        }
    }));
    failed(
        f.fs.edit("edit.txt", "original", "ours", false).await,
        FsError::Changed,
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("edit.txt")).unwrap(),
        "external writer"
    );
    let root = f.fs.root().to_path_buf();
    *f.fs.inner.hook.lock().unwrap() = Some(Arc::new(move |stage, target| {
        if stage == "publish" && target == "new.txt" {
            let path = std::fs::read_dir(&root)
                .unwrap()
                .filter_map(|e| e.ok())
                .find(|e| e.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
                .unwrap()
                .path();
            std::fs::remove_file(&path).unwrap();
            std::fs::write(path, "attacker replacement").unwrap();
        }
    }));
    failed(f.fs.write("new.txt", "ours").await, FsError::Changed);
    assert!(!f.root.path().join("new.txt").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn filesystem_work_does_not_block_the_async_runtime_thread() {
    use std::sync::Condvar;
    let f = Fixture::new(&[], &[]);
    std::fs::write(f.root.path().join("read.txt"), "text").unwrap();
    let ready = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let signal = ready.clone();
    let blocked = release.clone();
    *f.fs.inner.hook.lock().unwrap() = Some(Arc::new(move |stage, _| {
        if stage == "read" {
            signal.notify_one();
            let (lock, cv) = &*blocked;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = cv.wait(released).unwrap();
            }
        }
    }));
    let fs = f.fs.clone();
    let task = tokio::spawn(async move { fs.read("read.txt", 0, 10).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), ready.notified())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let (lock, cv) = &*release;
    *lock.lock().unwrap() = true;
    cv.notify_all();
    assert!(task.await.unwrap().unwrap().contains("text"));
}

#[tokio::test]
async fn bounds_reject_oversized_files_and_names_before_mutation_and_bound_rendering() {
    let f = Fixture::new(&["tool:write_file"], &[]);
    let large = "x".repeat(MAX_FILE_BYTES + 1);
    failed(
        f.fs.write("new/huge.txt", &large).await,
        FsError::FileTooLarge,
    );
    assert!(!f.root.path().join("new").exists());
    std::fs::write(f.root.path().join("large.txt"), &large).unwrap();
    failed(f.fs.read("large.txt", 0, 1).await, FsError::FileTooLarge);
    failed(f.fs.glob(&"*".repeat(257)).await, FsError::InvalidPath);
    let path = vec!["p"; MAX_DEPTH + 1].join("/");
    failed(f.fs.write(&path, "no").await, FsError::InvalidPath);
    let line = "界".repeat(MAX_OUTPUT_BYTES);
    assert_eq!(
        render_page(&line, 0, 1),
        crate::tools::clamp(cat_n(&line, 0, 1))
    );
    let lines = "a\n".repeat(MAX_OUTPUT_BYTES);
    assert_eq!(
        render_page(&lines, 0, usize::MAX),
        crate::tools::clamp(cat_n(&lines, 0, usize::MAX))
    );
    assert!(render_page(&line, 0, 1).len() < MAX_OUTPUT_BYTES + 64);
}

#[tokio::test]
async fn read_pagination_empty_binary_and_grep_results_are_truthful() {
    let f = Fixture::new(&["tool:write_file"], &[]);
    f.fs.write("empty.txt", "").await.unwrap();
    assert_eq!(
        f.fs.read("empty.txt", usize::MAX, 10).await.unwrap(),
        cat_n("", 0, 10)
    );
    f.fs.write("text.txt", "a\nb\nc").await.unwrap();
    assert_eq!(
        f.fs.read("text.txt", 1, 1).await.unwrap(),
        cat_n("a\nb\nc", 1, 1)
    );
    failed(f.fs.read("text.txt", 3, 10).await, FsError::Offset);
    std::fs::write(f.root.path().join("binary.bin"), [0xff, 0, 1]).unwrap();
    failed(f.fs.read("binary.bin", 0, 10).await, FsError::InvalidUtf8);
    assert!(
        !f.fs
            .grep("absent", None)
            .await
            .unwrap()
            .contains("binary.bin")
    );
    let line = "long-match".repeat(MAX_OUTPUT_BYTES);
    f.fs.write("matched.txt", &line).await.unwrap();
    assert!(
        f.fs.grep("long-match", Some("matched.txt"))
            .await
            .unwrap()
            .ends_with("… [output truncated]")
    );
    let no_matches =
        f.fs.grep(&"x".repeat(MAX_OUTPUT_BYTES), None)
            .await
            .unwrap();
    assert!(no_matches.len() < MAX_OUTPUT_BYTES + 64);
    assert!(no_matches.ends_with("… [output truncated]"));
}

#[cfg(unix)]
#[tokio::test]
async fn final_parent_inside_and_dangling_symlinks_never_reach_data_or_writes() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    std::fs::write(f.root.path().join("inside.txt"), "inside").unwrap();
    std::fs::write(f.home.path().join("outside.txt"), "SYNTHETIC-SECRET").unwrap();
    symlink(f.home.path(), f.root.path().join("linked")).unwrap();
    symlink("inside.txt", f.root.path().join("inside-link")).unwrap();
    symlink(
        f.home.path().join("outside.txt"),
        f.root.path().join("outside-link"),
    )
    .unwrap();
    symlink("missing", f.root.path().join("dangling")).unwrap();
    for path in [
        "inside-link",
        "outside-link",
        "dangling",
        "linked/outside.txt",
    ] {
        assert!(f.fs.read(path, 0, 10).await.is_err());
        assert!(f.fs.edit(path, "SECRET", "changed", false).await.is_err());
    }
    assert!(f.fs.write("linked/new.txt", "no").await.is_err());
    assert!(f.fs.ls("linked").await.is_err());
    assert!(f.fs.grep("SECRET", Some("linked")).await.is_err());
    assert_eq!(f.fs.glob("**").await.unwrap(), ["inside.txt"]);
    assert_eq!(
        f.fs.ls(".")
            .await
            .unwrap()
            .iter()
            .map(|e| e.path.as_str())
            .collect::<Vec<_>>(),
        ["inside.txt"]
    );
    assert!(
        !f.fs
            .grep("SECRET", None)
            .await
            .unwrap()
            .contains("SYNTHETIC-SECRET")
    );
    assert!(!f.home.path().join("new.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn parent_swap_after_walk_is_rejected_before_read_or_creation() {
    use std::os::unix::fs::symlink;
    for write in [false, true] {
        let f = Fixture::new(&["tool:write_file"], &[]);
        std::fs::create_dir(f.root.path().join("parent")).unwrap();
        std::fs::write(f.root.path().join("parent/file.txt"), "inside").unwrap();
        std::fs::write(f.home.path().join("file.txt"), "SYNTHETIC-OUTSIDE-SECRET").unwrap();
        let root = f.fs.root().to_path_buf();
        let outside = f.home.path().to_path_buf();
        *f.fs.inner.hook.lock().unwrap() = Some(Arc::new(move |stage, target| {
            if stage == "parent" && target.starts_with("parent/") {
                std::fs::rename(root.join("parent"), root.join("old-parent")).unwrap();
                symlink(&outside, root.join("parent")).unwrap();
            }
        }));
        if write {
            failed(f.fs.write("parent/new.txt", "no").await, FsError::Changed);
        } else {
            failed(f.fs.read("parent/file.txt", 0, 10).await, FsError::Changed);
        }
        assert!(!f.home.path().join("new.txt").exists());
        assert_eq!(
            std::fs::read_to_string(f.home.path().join("file.txt")).unwrap(),
            "SYNTHETIC-OUTSIDE-SECRET"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn final_swap_after_open_and_before_edit_publication_is_not_followed() {
    use std::os::unix::fs::symlink;
    for edit in [false, true] {
        let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
        std::fs::write(f.root.path().join("target.txt"), "inside").unwrap();
        let outside = f.home.path().join("secret.txt");
        std::fs::write(&outside, "SYNTHETIC-SECRET").unwrap();
        let target = f.fs.root().join("target.txt");
        let original = outside.clone();
        *f.fs.inner.hook.lock().unwrap() = Some(Arc::new(move |stage, path| {
            if path == "target.txt" && stage == if edit { "publish" } else { "read" } {
                std::fs::remove_file(&target).unwrap();
                symlink(&original, &target).unwrap();
            }
        }));
        if edit {
            failed(
                f.fs.edit("target.txt", "inside", "ours", false).await,
                FsError::Changed,
            );
        } else {
            failed(f.fs.read("target.txt", 0, 10).await, FsError::Changed);
        }
        assert_eq!(
            std::fs::read_to_string(outside).unwrap(),
            "SYNTHETIC-SECRET"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_fifo_swapped_in_after_metadata_check_does_not_hang_the_worker() {
    use std::ffi::CString;
    use std::sync::atomic::AtomicBool;
    let f = Fixture::new(&[], &[]);
    let path = f.fs.root().join("race.txt");
    std::fs::write(&path, "regular").unwrap();
    let once = Arc::new(AtomicBool::new(false));
    let switched = once.clone();
    let fifo = path.clone();
    *f.fs.inner.hook.lock().unwrap() = Some(Arc::new(move |stage, target| {
        if stage == "open" && target == "race.txt" && !switched.swap(true, Ordering::SeqCst) {
            std::fs::remove_file(&fifo).unwrap();
            let name = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        }
    }));
    // A safety watchdog opens a writer if a regression accidentally uses a
    // blocking open. Even a failed timeout test then releases the worker.
    let done = Arc::new(AtomicBool::new(false));
    let stopped = done.clone();
    let watchdog = std::thread::spawn(move || {
        for _ in 0..30 {
            if stopped.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let name = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let fd = unsafe { libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK) };
        if fd >= 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            unsafe { libc::close(fd) };
        }
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        f.fs.read("race.txt", 0, 10),
    )
    .await;
    if result.is_ok() {
        done.store(true, Ordering::SeqCst);
    }
    watchdog.join().unwrap();
    failed(
        result.expect("FIFO open must be nonblocking"),
        FsError::NotFile,
    );
}

#[cfg(unix)]
#[tokio::test]
async fn root_path_rebinding_does_not_rebind_the_retained_directory_authority() {
    use std::os::unix::fs::symlink;
    let base = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = base.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("file.txt"), "authorized").unwrap();
    std::fs::write(outside.path().join("file.txt"), "OUTSIDE-SECRET").unwrap();
    let fs = ConfinedFs::new(
        &root,
        &base.path().join("cfg"),
        policy(&[], &[], Mode::Manual),
    )
    .unwrap();
    std::fs::rename(&root, base.path().join("original-root")).unwrap();
    symlink(outside.path(), &root).unwrap();
    assert_eq!(
        fs.read("file.txt", 0, 10).await.unwrap(),
        "     1\tauthorized"
    );
}

#[tokio::test]
async fn normalized_unicode_denies_do_not_grant_different_protected_files() {
    let f = Fixture::new(
        &["tool:write_file", "path:.github/café.yml"],
        &["path:café.txt"],
    );
    // Denies conservatively cover NFC/NFD and case aliases on every platform.
    failed(
        f.fs.write("cafe\u{301}.txt", "no").await,
        FsError::PathDenied,
    );
    failed(f.fs.write("CAFÉ.txt", "no").await, FsError::PathDenied);
    // An exact write allow does not widen through normalization on systems
    // where the NFC/NFD spellings may name distinct files.
    failed(
        f.fs.write(".github/cafe\u{301}.yml", "no").await,
        FsError::ProtectedWrite,
    );
    f.fs.write(".github/café.yml", "allowed").await.unwrap();
}

#[tokio::test]
async fn nonexisting_config_boundaries_cover_normalized_unicode_aliases() {
    let root = tempfile::tempdir().unwrap();
    let fs = ConfinedFs::new(
        root.path(),
        &root.path().join("配置-café"),
        policy(&["tool:write_file"], &[], Mode::Manual),
    )
    .unwrap();
    failed(
        fs.write("配置-cafe\u{301}/token.json", "no").await,
        FsError::HardDenied,
    );
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

struct BoundFileTool {
    backend: Arc<dyn Backend>,
    name: &'static str,
}

#[async_trait]
impl craftsman_harness::Tool for BoundFileTool {
    fn def(&self) -> craftsman_harness::ToolDef {
        craftsman_harness::ToolDef {
            name: self.name.into(),
            description: "test filesystem operation".into(),
            parameters: json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        args: serde_json::Value,
        _: &mut craftsman_harness::ToolCtx<'_>,
    ) -> std::result::Result<craftsman_harness::ToolOutcome, AgentError> {
        match self.name {
            "write_file" => {
                self.backend
                    .write(
                        args["file_path"].as_str().unwrap(),
                        args["content"].as_str().unwrap(),
                    )
                    .await?
            }
            "read_file" => {
                return self
                    .backend
                    .read(args["file_path"].as_str().unwrap(), 0, 100)
                    .await
                    .map(craftsman_harness::ToolOutcome::ok);
            }
            _ => unreachable!(),
        }
        Ok(craftsman_harness::ToolOutcome::ok("written"))
    }
}

fn assistant_call(id: &str, tool: &str, args: serde_json::Value) -> craftsman_harness::Msg {
    craftsman_harness::Msg::Assistant {
        content: String::new(),
        tool_calls: vec![craftsman_harness::ToolCall {
            id: id.into(),
            name: tool.into(),
            args,
        }],
    }
}

#[tokio::test]
async fn main_and_delegated_calls_share_backend_enforcement_without_an_approval_prompt() {
    use craftsman_harness::builtin::SubAgentMiddleware;
    use craftsman_harness::testing::ScriptedModel;
    use craftsman_harness::{Agent, Msg, SubAgentSpec};
    let f = Fixture::new(&["tool:write_file"], &[]);
    let backend: Arc<dyn Backend> = Arc::new(f.fs.clone());
    let target = f
        .home
        .path()
        .join("must-not-exist.txt")
        .to_string_lossy()
        .into_owned();
    let child = Arc::new(ScriptedModel::new(vec![
        assistant_call(
            "child-write",
            "write_file",
            json!({"file_path":target,"content":"no"}),
        ),
        assistant_call(
            "child-protected",
            "write_file",
            json!({"file_path":"Cargo.toml","content":"no"}),
        ),
        Msg::assistant("child saw denials"),
    ]));
    let parent = Arc::new(ScriptedModel::new(vec![
        assistant_call(
            "main-protected",
            "write_file",
            json!({"file_path":".craftsman/grants.json","content":"no"}),
        ),
        assistant_call(
            "task-1",
            "task",
            json!({"description":"try the same write","subagent_type":"writer"}),
        ),
        Msg::assistant("done"),
    ]));
    let tool: Arc<dyn craftsman_harness::Tool> = Arc::new(BoundFileTool {
        backend: backend.clone(),
        name: "write_file",
    });
    let spec = SubAgentSpec {
        name: "writer".into(),
        description: "test".into(),
        system_prompt: String::new(),
        tools: vec![tool.clone()],
        model: Some(child.clone()),
        harness_profile: None,
        interrupt_on: vec![],
        max_depth: 1,
    };
    let agent = Agent::builder(parent.clone())
        .name("main")
        .backend(backend)
        .tool(tool)
        .middleware(Arc::new(SubAgentMiddleware::new(vec![spec])))
        .build();
    agent.run_turn("try it").await.unwrap();
    assert!(!f.home.path().join("must-not-exist.txt").exists());
    assert!(!f.root.path().join("Cargo.toml").exists());
    assert!(!f.root.path().join(".craftsman").exists());
    let requests = child.requests.lock().unwrap();
    let last = requests.last().unwrap();
    assert!(last.messages.iter().any(|m| matches!(m,Msg::Tool {is_error:true,content,..} if content.contains("outside the canonical workspace"))));
    assert!(last.messages.iter().any(
        |m| matches!(m,Msg::Tool {is_error:true,content,..} if content.contains("protected writes"))
    ));
}

#[tokio::test]
async fn grep_aggregate_read_budget_is_not_evaded_by_many_nonmatching_files() {
    let f = Fixture::new(&[], &[]);
    for index in 0..7 {
        let file = std::fs::File::create(f.root.path().join(format!("large-{index}.bin"))).unwrap();
        file.set_len(MAX_FILE_BYTES as u64).unwrap();
    }
    failed(f.fs.grep("no matches", None).await, FsError::OutputLimit);
}

#[cfg(unix)]
#[tokio::test]
async fn creation_is_private_and_edits_drop_special_unix_permission_bits() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(&["tool:write_file", "tool:edit_file"], &[]);
    f.fs.write("new/file.txt", "before").await.unwrap();
    let dir = std::fs::metadata(f.root.path().join("new")).unwrap();
    let file = std::fs::metadata(f.root.path().join("new/file.txt")).unwrap();
    assert_eq!(dir.permissions().mode() & 0o077, 0);
    assert_eq!(file.permissions().mode() & 0o077, 0);
    std::fs::set_permissions(
        f.root.path().join("new/file.txt"),
        std::fs::Permissions::from_mode(0o6751),
    )
    .unwrap();
    f.fs.edit("new/file.txt", "before", "after", false)
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(f.root.path().join("new/file.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o751
    );
}

#[tokio::test]
async fn raw_host_io_preserves_binary_bytes_without_text_framing() {
    let f = Fixture::new(&["tool:write_file"], &[]);
    let bytes = [0, 0xff, b'\r', b'\n', 0xc3, 0x28];
    f.fs.write_raw("binary.bin", &bytes).await.unwrap();
    assert_eq!(f.fs.read_raw("binary.bin").await.unwrap(), bytes);
    assert_eq!(
        std::fs::read(f.root.path().join("binary.bin")).unwrap(),
        bytes
    );
    failed(f.fs.read("binary.bin", 0, 10).await, FsError::InvalidUtf8);
    assert_eq!(
        f.fs.write_raw("binary.bin", b"replace").await.unwrap_err(),
        FsError::Exists
    );
    assert_eq!(f.fs.list_raw(".").await.unwrap()[0].path, "binary.bin");
}

#[tokio::test]
async fn raw_host_denials_are_typed_and_cannot_create_unapproved_parents() {
    let f = Fixture::new(&[], &[]);
    assert_eq!(
        f.fs.write_raw("new/sub/a.bin", b"no").await.unwrap_err(),
        FsError::Unapproved
    );
    assert!(!f.root.path().join("new").exists());
    assert_eq!(
        f.fs.write_raw(".env", b"no").await.unwrap_err(),
        FsError::HardDenied
    );
    let outside = f.home.path().join("SECRET-PATH-MUST-NOT-ECHO");
    std::fs::write(&outside, b"secret").unwrap();
    let error = f.fs.read_raw(outside.to_str().unwrap()).await.unwrap_err();
    assert_eq!(error, FsError::OutsideRoot);
    assert!(!error.to_string().contains("SECRET-PATH-MUST-NOT-ECHO"));
    assert_eq!(
        f.fs.read_raw("missing").await.unwrap_err(),
        FsError::NotFound
    );
}

#[tokio::test]
async fn raw_host_write_respects_plan_and_exact_protected_overrides() {
    let f = Fixture::new(&["tool:write_file", "path:Cargo.toml"], &[]);
    f.fs.write_raw("Cargo.toml", b"[package]\n").await.unwrap();
    assert_eq!(
        f.fs.write_raw("build.rs", b"no").await.unwrap_err(),
        FsError::ProtectedWrite
    );
    assert_eq!(
        f.fs.write_raw(".git/config", b"no").await.unwrap_err(),
        FsError::HardDenied
    );
    let plan = ConfinedFs::new(
        f.root.path(),
        &f.home.path().join("cfg"),
        policy(&["tool:write_file"], &[], Mode::Plan),
    )
    .unwrap();
    assert_eq!(
        plan.write_raw("plan/sub/a", b"no").await.unwrap_err(),
        FsError::ToolDenied
    );
    assert!(!f.root.path().join("plan").exists());
}

#[tokio::test]
async fn raw_host_listing_counts_denied_candidates_before_its_1024_entry_bound() {
    let f = Fixture::new(&[], &[]);
    for i in 0..1024 {
        std::fs::write(f.root.path().join(format!(".env-{i:04}")), b"hidden").unwrap();
    }
    assert!(f.fs.list_raw(".").await.unwrap().is_empty());
    std::fs::write(f.root.path().join(".env-overflow"), b"hidden").unwrap();
    assert_eq!(f.fs.list_raw(".").await.unwrap_err(), FsError::OutputLimit);
    // Ordinary filesystem ls keeps its established larger traversal bound.
    assert!(f.fs.ls(".").await.unwrap().is_empty());
}

#[tokio::test]
async fn raw_host_file_bounds_reject_without_truncating_or_creating_parents() {
    let f = Fixture::new(&["tool:write_file"], &[]);
    let bytes = vec![0xff; MAX_FILE_BYTES];
    f.fs.write_raw("at-bound.bin", &bytes).await.unwrap();
    assert_eq!(f.fs.read_raw("at-bound.bin").await.unwrap(), bytes);
    let too_large = vec![0; MAX_FILE_BYTES + 1];
    assert_eq!(
        f.fs.write_raw("oversize/child.bin", &too_large)
            .await
            .unwrap_err(),
        FsError::FileTooLarge
    );
    assert!(!f.root.path().join("oversize").exists());
    std::fs::write(f.root.path().join("oversize.bin"), too_large).unwrap();
    assert_eq!(
        f.fs.read_raw("oversize.bin").await.unwrap_err(),
        FsError::FileTooLarge
    );
}

#[cfg(unix)]
#[tokio::test]
async fn raw_host_operations_retain_root_authority_after_path_rebinding() {
    let f = Fixture::new(&["tool:write_file"], &[]);
    std::fs::write(f.root.path().join("identity.bin"), [0xff, 1]).unwrap();
    let reported = f.fs.root().to_owned();
    let moved = f.home.path().join("retained");
    std::fs::rename(&reported, &moved).unwrap();
    std::fs::create_dir(&reported).unwrap();
    std::fs::write(reported.join("identity.bin"), b"replacement").unwrap();
    assert_eq!(f.fs.read_raw("identity.bin").await.unwrap(), [0xff, 1]);
    f.fs.write_raw("new.bin", &[0, 0xfe]).await.unwrap();
    assert_eq!(std::fs::read(moved.join("new.bin")).unwrap(), [0, 0xfe]);
    assert!(!reported.join("new.bin").exists());
}
