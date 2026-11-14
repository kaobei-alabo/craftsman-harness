use super::*;

fn config(value: Value) -> Config {
    let mut cfg = Config::default();
    cfg.extra.insert("verification".into(), value);
    cfg
}

#[test]
fn empty_policy_does_no_source_scan_and_explicit_policy_requires_scopes() {
    let missing = Path::new("/definitely-unavailable-verification-fixture");
    assert!(
        VerificationPlan::load(&config(json!({"commands":[]})), missing)
            .unwrap()
            .is_none()
    );
    assert!(VerificationPlan::load(&config(json!({"commands":["check"]})), missing).is_err());
    let plan=VerificationPlan::load(&config(json!({"commands":["check"],"paths":["project-a/src","project-b/document.txt"],"protected_paths":["project-a/src/tests.rs"]})),missing).unwrap().unwrap();
    assert_eq!(plan.paths, vec!["project-a/src", "project-b/document.txt"]);
    assert_eq!(plan.protected, vec!["project-a/src/tests.rs"]);
}

#[test]
fn invalid_policy_refuses_unknown_authority_and_unbounded_or_ambiguous_paths() {
    for value in [
        json!({"commands":["check"],"paths":["src"],"env_pass":["TOKEN"]}),
        json!({"commands":["check"],"paths":["../src"]}),
        json!({"commands":["check"],"paths":["src/./tests"]}),
        json!({"commands":["check"],"paths":["src//tests"]}),
        json!({"commands":["check"],"paths":["src"],"protected_paths":["other"]}),
        json!({"commands":["check"],"paths":["src"],"max_repair_rounds":4}),
        json!({"commands":[7],"paths":["src"]}),
        json!({"commands":["x".repeat(4097)],"paths":["src"]}),
    ] {
        assert!(VerificationPlan::load(&config(value), Path::new(".")).is_err());
    }
}

#[test]
fn declared_non_git_scopes_ignore_unrelated_projects_and_bind_actual_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("project-a")).unwrap();
    std::fs::create_dir(root.join("project-b")).unwrap();
    std::fs::write(root.join("project-a/value"), "before").unwrap();
    std::fs::write(root.join("project-b/value"), "unrelated").unwrap();
    let paths = vec!["project-a".into()];
    let excluded = BTreeSet::new();
    let before = workspace_checkpoint::observe_source(root, &paths, &excluded).unwrap();
    assert!(before.complete());
    std::fs::write(root.join("project-b/value"), "changed elsewhere").unwrap();
    let same = workspace_checkpoint::observe_source(root, &paths, &excluded).unwrap();
    assert_eq!(before.digest(), same.digest());
    std::fs::write(root.join("project-a/value"), "after").unwrap();
    let after = workspace_checkpoint::observe_source(root, &paths, &excluded).unwrap();
    assert_ne!(before.digest(), after.digest());
}

#[test]
fn protected_change_review_is_bound_to_actual_before_and_after_contents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("tests")).unwrap();
    std::fs::write(root.join("tests/public.txt"), "assert original").unwrap();
    let paths = vec!["tests".into()];
    let before = workspace_checkpoint::observe_source(root, &paths, &BTreeSet::new()).unwrap();
    std::fs::write(root.join("tests/public.txt"), "assert changed").unwrap();
    let after = workspace_checkpoint::observe_source(root, &paths, &BTreeSet::new()).unwrap();
    let (digest, detail) = before.protected_review(&after, &paths).unwrap();
    assert!(detail.contains("assert original") && detail.contains("assert changed"));
    std::fs::write(root.join("tests/public.txt"), "another change").unwrap();
    let newer = workspace_checkpoint::observe_source(root, &paths, &BTreeSet::new()).unwrap();
    assert_ne!(digest, before.protected_review(&newer, &paths).unwrap().0);
    let (_, restored) = newer.protected_contents(&paths).unwrap();
    assert!(restored.contains("baseline is unavailable"));
}

#[test]
fn unsupported_source_or_excessive_coverage_cannot_be_claimed_complete() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("src")).unwrap();
    for i in 0..1025 {
        std::fs::write(root.join("src").join(format!("file-{i:04}")), "x").unwrap();
    }
    let image =
        workspace_checkpoint::observe_source(root, &["src".into()], &BTreeSet::new()).unwrap();
    assert!(!image.complete());
}

#[test]
fn git_ignore_changes_do_not_hide_protected_source_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let status = std::process::Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null", "init", "--quiet"])
        .arg(root)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::create_dir(root.join("tests")).unwrap();
    std::fs::write(root.join("tests/public.txt"), "test original").unwrap();
    let paths = vec!["tests".into()];
    let before = workspace_checkpoint::observe_source(root, &paths, &BTreeSet::new()).unwrap();
    std::fs::write(root.join(".gitignore"), "tests/\n").unwrap();
    std::fs::write(root.join("tests/public.txt"), "test modified").unwrap();
    let after = workspace_checkpoint::observe_source(root, &paths, &BTreeSet::new()).unwrap();
    assert!(before.complete() && after.complete());
    let (_, review) = before.protected_review(&after, &paths).unwrap();
    assert!(review.contains("test original") && review.contains("test modified"));
}

#[test]
fn application_state_and_project_policy_are_excluded_from_source_observations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::create_dir(root.join(".craftsman")).unwrap();
    std::fs::write(root.join("src/file"), "source").unwrap();
    std::fs::write(root.join(".craftsman/config.json"), "synthetic secret").unwrap();
    let excluded = BTreeSet::from([".craftsman".into()]);
    let before = workspace_checkpoint::observe_source(root, &[String::new()], &excluded).unwrap();
    std::fs::write(root.join(".craftsman/config.json"), "changed secret").unwrap();
    let after = workspace_checkpoint::observe_source(root, &[String::new()], &excluded).unwrap();
    assert_eq!(before.digest(), after.digest());
    assert!(
        workspace_checkpoint::observe_source(root, &[".craftsman/config.json".into()], &excluded)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn project_policy_and_scoped_sources_never_follow_symlinked_parents() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(outside.path().join("verification.json"), "{}").unwrap();
    symlink(outside.path(), root.join(".craftsman")).unwrap();
    assert!(workspace_checkpoint::read_verification_policy(root, POLICY_FILE).is_err());
    std::fs::write(outside.path().join("file"), "outside").unwrap();
    symlink(outside.path(), root.join("src")).unwrap();
    assert!(
        workspace_checkpoint::observe_source(root, &["src/file".into()], &BTreeSet::new()).is_err()
    );
}
