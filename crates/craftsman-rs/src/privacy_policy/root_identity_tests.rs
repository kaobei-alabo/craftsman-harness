//! The configured root is refused as a data location by the directory its
//! owner holds, whatever a rule calls it. Every directory, source file and
//! rule here is a real one: nothing builds a request or a grant.
use super::*;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::PathBuf;

/// A directory only its owner can enter, as a configured root must be.
fn private_child(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    path
}

/// The device and inode `path` names, when it names a directory itself and
/// not a link to one.
fn native_identity(path: &Path) -> Option<(u64, u64)> {
    let found = std::fs::symlink_metadata(path).ok()?;
    found.is_dir().then(|| (found.dev(), found.ino()))
}

/// Other spellings under which this machine reaches `directory` with no
/// symbolic link on the way: its name in another letter case, where the
/// volume ignores case, and the data-volume path that macOS gives every
/// directory. A machine with neither has no such spelling, and this returns
/// none instead of inventing one.
fn other_spellings(directory: &Path) -> Vec<(&'static str, PathBuf)> {
    let actual = native_identity(directory);
    let name = directory.file_name().unwrap().to_str().unwrap();
    [
        (
            "letter case",
            directory.with_file_name(name.to_ascii_lowercase()),
        ),
        (
            "data volume",
            Path::new("/System/Volumes/Data").join(directory.strip_prefix("/").unwrap()),
        ),
    ]
    .into_iter()
    .filter(|(_, spelled)| {
        spelled != directory
            && native_identity(spelled) == actual
            && spelled
                .ancestors()
                .all(|part| native_identity(part).is_some())
    })
    .collect()
}

/// A user policy with one rule for each destination, all in `workspace`.
fn policy_for(workspace: &Path, destinations: &[&Path]) -> Value {
    let mut policy = user_policy(workspace, destinations[0]);
    let first = policy["privacy"]["file_data"][0].clone();
    policy["privacy"]["file_data"] = Value::Array(
        destinations
            .iter()
            .map(|destination| {
                let mut rule = first.clone();
                rule["destination"] = json!(destination);
                rule
            })
            .collect(),
    );
    policy
}

/// Puts `policy` in place of the root's own test `config.json`.
fn replace_config(root: &Path, policy: &Value) {
    let _ = std::fs::remove_file(root.join("config.json"));
    create_config(root, policy);
}

fn capture(root: &Path) -> Result<CapturedPolicy> {
    CapturedPolicy::capture_from_root(ConfigRoot::capture_owned_test_root(root).unwrap())
}

/// The steps `CapturedPolicy::capture` takes, stopped before its last
/// recheck, so that the lookup can be asked about a rule which that recheck
/// refuses first.
fn captured_before_the_last_check(root: &Path) -> CapturedPolicy {
    let held = ConfigRoot::capture_owned_test_root(root).unwrap();
    let (bytes, source) = held.read_user_config().unwrap().unwrap();
    let Ok(Policy::Proposed(mut proposed)) = parse_user_source(&bytes) else {
        panic!("the test policy is an enabled one");
    };
    for rule in &mut proposed.file_rules {
        rule.capture().unwrap();
    }
    CapturedPolicy {
        policy: Policy::Proposed(proposed),
        root: held,
        source: Some(source),
    }
}

/// The rule of a policy that has one, holding no directory yet.
fn rule_for(workspace: &Path, destination: &Path) -> CapturedFileRule {
    let source = serde_json::to_vec(&user_policy(workspace, destination)).unwrap();
    let Ok(Policy::Proposed(proposed)) = parse_user_source(&source) else {
        panic!("the test policy is an enabled one");
    };
    proposed.file_rules.into_iter().next().unwrap()
}

#[test]
fn another_spelling_of_the_configured_root_is_refused_by_the_directory_it_holds() {
    let owner = private_directory();
    let parent = owner.path().canonicalize().unwrap();
    let root = private_child(&parent, "Custom-User-Root");
    std::fs::create_dir(root.join("people")).unwrap();
    let mut exercised = Vec::new();
    for (form, spelled) in other_spellings(&root) {
        let workspace = spelled.parent().unwrap();
        let beside = workspace.join("contact-record.txt");
        // The spelling alone refuses nothing: a file beside the root, named
        // through the same spelling of its parent, is data as it was before.
        replace_config(&root, &policy_for(workspace, &[&beside]));
        let Ok(control) = capture(&root) else {
            println!("ROOT-SPELLING not exercised, its directories do not open: {form}");
            continue;
        };
        assert!(control.permits_data_destination(&beside), "{form}");
        assert_eq!(control.recheck(), Ok(()), "{form}");
        drop(control);
        for inside in ["contact-record.txt", "people/contact-record.txt"] {
            let destination = spelled.join(inside);
            // Neither name check sees the root in this spelling.
            assert!(!destination.starts_with(&root), "{form} {inside}");
            assert!(!file_data::protected_role(&destination), "{form} {inside}");
            // The actual loader refuses the policy that names it as data.
            replace_config(&root, &policy_for(workspace, &[&destination]));
            assert!(
                matches!(capture(&root), Err(PolicyError::Invalid)),
                "{form} {inside}"
            );
            // The lookup refuses it too, for that rule and not for the rule
            // beside it, when it is asked before the recheck could refuse.
            replace_config(&root, &policy_for(workspace, &[&destination, &beside]));
            let stopped = captured_before_the_last_check(&root);
            assert!(
                !stopped.permits_data_destination(&destination),
                "{form} {inside}"
            );
            assert!(stopped.permits_data_destination(&beside), "{form} {inside}");
            assert_eq!(
                stopped.recheck(),
                Err(PolicyError::Invalid),
                "{form} {inside}"
            );
        }
        exercised.push(form);
    }
    // A machine whose volumes keep letter case and have no second path to a
    // directory has no other spelling to try. The moved-root case below does
    // not depend on one.
    println!(
        "ROOT-SPELLINGS-EXERCISED: {}",
        if exercised.is_empty() {
            "none on this machine".to_owned()
        } else {
            exercised.join(", ")
        }
    );
}

#[test]
fn a_moved_configured_root_is_still_the_root_under_its_new_name() {
    let owner = private_directory();
    let parent = owner.path().canonicalize().unwrap();
    let root = private_child(&parent, "user-root");
    std::fs::create_dir(root.join("people")).unwrap();
    std::fs::create_dir(parent.join("notes")).unwrap();
    let held = ConfigRoot::capture_owned_test_root(&root).unwrap();
    // A rule that holds no directory yet is refused, not waved through.
    let mut neighbour = rule_for(&parent, &parent.join("notes/record.txt"));
    assert!(neighbour.reaches_root(&held));
    neighbour.capture().unwrap();
    assert!(!neighbour.reaches_root(&held));
    let mut beside = rule_for(&parent, &parent.join("record.txt"));
    beside.capture().unwrap();
    assert!(!beside.reaches_root(&held));
    // The root directory is given a name that reads like data. Its owner
    // still holds that same directory, and so does a rule that leads there,
    // directly or one directory further in.
    let moved = parent.join("moved-data");
    std::fs::rename(&root, &moved).unwrap();
    for inside in ["record.txt", "people/record.txt"] {
        let destination = moved.join(inside);
        assert!(!destination.starts_with(&root), "{inside}");
        assert!(!file_data::protected_role(&destination), "{inside}");
        let mut rule = rule_for(&parent, &destination);
        rule.capture().unwrap();
        assert!(rule.reaches_root(&held), "{inside}");
    }
    // A different directory that now has the root's old name is not the held
    // one. This answer is about the directory; that name stays refused by
    // the name check and by the root's own recheck.
    let replacement = private_child(&parent, "user-root");
    let mut renamed_over = rule_for(&parent, &replacement.join("record.txt"));
    renamed_over.capture().unwrap();
    assert!(!renamed_over.reaches_root(&held));
    assert!(!neighbour.reaches_root(&held));
    assert!(!beside.reaches_root(&held));
}

#[test]
fn data_beside_the_configured_root_stays_allowed_and_data_inside_it_is_refused_at_capture() {
    let owner = private_directory();
    let parent = owner.path().canonicalize().unwrap();
    let root = private_child(&parent, "user-root");
    std::fs::create_dir(root.join("people")).unwrap();
    std::fs::create_dir(parent.join("notes")).unwrap();
    // The workspace contains the root. A file beside the root and a file in
    // a neighbouring directory pass through the root's parent, not the root.
    let beside = parent.join("record.txt");
    let neighbour = parent.join("notes/record.txt");
    replace_config(&root, &policy_for(&parent, &[&beside, &neighbour]));
    let captured = capture(&root).unwrap();
    assert_eq!(captured.recheck(), Ok(()));
    assert!(captured.permits_data_destination(&beside));
    assert!(captured.permits_data_destination(&neighbour));
    let detector = captured.proposed_detector().unwrap().unwrap();
    assert_eq!(detector.file_rules.len(), 2);
    for rule in &detector.file_rules {
        assert!(!rule.reaches_root(captured.root()));
    }
    drop(captured);
    // Inside the root under its own spelling, a rule used to be captured and
    // refused at each lookup by name. It is now no policy at all, also when
    // an allowed rule stands beside it.
    for inside in ["record.txt", "people/record.txt"] {
        let destination = root.join(inside);
        replace_config(&root, &policy_for(&parent, &[&beside, &destination]));
        assert!(
            matches!(capture(&root), Err(PolicyError::Invalid)),
            "{inside}"
        );
    }
}
