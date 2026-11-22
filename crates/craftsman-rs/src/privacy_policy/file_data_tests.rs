use super::*;
use serde_json::{Value, json};
use std::path::Path;

fn user_policy(workspace: &Path, destination: &Path) -> Value {
    json!({"privacy": {
        "enabled": true,
        "detector": {
            "gadget": "privacy-detector",
            "function": "detect",
            "component_sha256": "a".repeat(64),
            "manifest_sha256": "b".repeat(64)
        },
        "file_data": [{
            "workspace": workspace,
            "destination": destination,
            "format": "plain_text",
            "operations": ["write_file", "edit_file"]
        }]
    }})
}

#[test]
fn file_data_policy_requires_explicit_location_representation_and_operations() {
    let workspace = Path::new(if cfg!(windows) {
        "C:/owned/workspace"
    } else {
        "/owned/workspace"
    });
    let proposed = user_policy(workspace, &workspace.join("data.txt"));
    assert!(matches!(
        parse_user_source(&serde_json::to_vec(&proposed).unwrap()),
        Ok(Policy::Proposed(_))
    ));
    // An extension alone, a relative/model-proposed location, an execution
    // permission, and a narrower unsupported operation set cannot authorize
    // restoration. Even disabled malformed policy fails instead of vanishing.
    for (field, replacement) in [
        ("workspace", json!("owned/workspace")),
        ("destination", json!("/outside/data.txt")),
        ("destination", json!(workspace)),
        ("destination", json!(workspace.join("../data.txt"))),
        ("destination", json!(workspace.join("./data.txt"))),
        ("destination", json!("/owned/workspace/bad\nname")),
        ("format", json!(".json")),
        ("format", json!("source_code")),
        ("operations", json!(["write_file"])),
        ("operations", json!(["edit_file", "execute"])),
        ("operations", json!(["write_file", "write_file"])),
        ("capabilities", json!(["file_write"])),
    ] {
        for enabled in [true, false] {
            let mut bad = proposed.clone();
            bad["privacy"]["enabled"] = json!(enabled);
            bad["privacy"]["file_data"][0][field] = replacement.clone();
            assert!(
                matches!(
                    parse_user_source(&serde_json::to_vec(&bad).unwrap()),
                    Err(PolicyError::Invalid)
                ),
                "{field}"
            );
        }
    }
    let mut duplicate = proposed.clone();
    duplicate["privacy"]["file_data"] = json!([
        proposed["privacy"]["file_data"][0],
        proposed["privacy"]["file_data"][0]
    ]);
    assert!(matches!(
        parse_user_source(&serde_json::to_vec(&duplicate).unwrap()),
        Err(PolicyError::Invalid)
    ));
    let mut too_many = proposed.clone();
    too_many["privacy"]["file_data"] = Value::Array(
        (0..17)
            .map(|index| {
                let mut rule = proposed["privacy"]["file_data"][0].clone();
                rule["destination"] = json!(workspace.join(format!("file-{index}")));
                rule
            })
            .collect(),
    );
    assert!(matches!(
        parse_user_source(&serde_json::to_vec(&too_many).unwrap()),
        Err(PolicyError::Invalid)
    ));
    let mut no_detector = proposed;
    no_detector["privacy"]
        .as_object_mut()
        .unwrap()
        .remove("detector");
    no_detector["privacy"]["enabled"] = json!(false);
    assert!(matches!(
        parse_user_source(&serde_json::to_vec(&no_detector).unwrap()),
        Err(PolicyError::Invalid)
    ));
}

#[test]
fn explicit_contact_application_is_strict_and_does_not_broaden_generic_format() {
    let workspace = Path::new(if cfg!(windows) {
        "C:/owned/workspace"
    } else {
        "/owned/workspace"
    });
    let generic = user_policy(workspace, &workspace.join("record.txt"));
    assert!(matches!(
        parse_user_source(&serde_json::to_vec(&generic).unwrap()),
        Ok(Policy::Proposed(_))
    ));
    for (format, application) in [
        ("plain_text", "contact_lines_v1"),
        ("json_document", "contact_json_v1"),
    ] {
        let mut value = generic.clone();
        value["privacy"]["file_data"][0]["format"] = json!(format);
        value["privacy"]["file_data"][0]["application"] = json!(application);
        assert!(matches!(
            parse_user_source(&serde_json::to_vec(&value).unwrap()),
            Ok(Policy::Proposed(_))
        ));
        value["privacy"]["file_data"][0]["format"] = json!(if format == "plain_text" {
            "json_document"
        } else {
            "plain_text"
        });
        assert!(matches!(
            parse_user_source(&serde_json::to_vec(&value).unwrap()),
            Err(PolicyError::Invalid)
        ));
    }
    for field in ["purpose", "schema", "pointer", "capability"] {
        let mut bad = generic.clone();
        bad["privacy"]["file_data"][0][field] = json!("contact_data");
        assert!(matches!(
            parse_user_source(&serde_json::to_vec(&bad).unwrap()),
            Err(PolicyError::Invalid)
        ));
    }
    let mut unknown = generic;
    unknown["privacy"]["file_data"][0]["application"] = json!("generic_json");
    assert!(matches!(
        parse_user_source(&serde_json::to_vec(&unknown).unwrap()),
        Err(PolicyError::Invalid)
    ));
}

#[cfg(unix)]
#[test]
fn actual_configured_root_excludes_innocuously_named_contact_data_destinations() {
    let root_owner = private_directory();
    let workspace_owner = private_directory();
    let root = root_owner.path().canonicalize().unwrap();
    let workspace = workspace_owner.path().canonicalize().unwrap();
    create_config(
        &root,
        &user_policy(&workspace, &workspace.join("record.txt")),
    );
    let captured =
        CapturedPolicy::capture_from_root(ConfigRoot::capture_owned_test_root(&root).unwrap())
            .unwrap();
    assert!(!captured.permits_data_destination(&root.join("innocent-contact-record.txt")));
    assert!(!captured.permits_data_destination(&root.join("other-name.json")));
    for path in [
        "AGENTS.md",
        ".craftsman/AGENTS.md",
        "instructions/owner.txt",
        "schema/contact.json",
        "src/contact.json",
        "data/credentials.json",
        "data/profile.schema.json",
        "scripts/owner.txt",
    ] {
        assert!(
            !captured.permits_data_destination(&workspace.join(path)),
            "{path}"
        );
    }
    assert!(captured.permits_data_destination(&workspace.join("record.txt")));
    // Not excluded is only an eligibility predicate. No FileDataRequest,
    // application/schema/range grant or retained final effect exists here.
}

#[cfg(unix)]
fn private_directory() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[cfg(unix)]
fn create_config(root: &Path, value: &Value) {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("config.json"))
        .unwrap();
    file.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
    file.sync_all().unwrap();
}

#[cfg(unix)]
#[test]
fn captured_file_rule_binds_user_source_workspace_and_real_parent_identity() {
    use crate::privacy_restore::FileDataFormat;
    let root_owner = private_directory();
    let workspace_owner = private_directory();
    let root = root_owner.path().canonicalize().unwrap();
    let workspace = workspace_owner.path().canonicalize().unwrap();
    let parent = workspace.join("data");
    std::fs::create_dir(&parent).unwrap();
    let destination = parent.join("answer.json");
    std::fs::write(&destination, b"{\"data\":\"retained original\"}").unwrap();
    let mut policy = user_policy(&workspace, &destination);
    policy["privacy"]["file_data"][0]["format"] = json!("json_document");
    create_config(&root, &policy);
    let captured =
        CapturedPolicy::capture_from_root(ConfigRoot::capture_owned_test_root(&root).unwrap())
            .unwrap();
    let detector = captured.proposed_detector().unwrap().unwrap();
    assert!(matches!(
        detector.file_data_format(&workspace, &destination),
        Some(FileDataFormat::JsonDocument)
    ));
    assert!(
        detector
            .file_data_format(&workspace, &parent.join("other.json"))
            .is_none()
    );
    assert!(detector.file_data_format(&root, &destination).is_none());
    let mut merged = crate::config::Config::default();
    merged.extra.insert(
        "privacy".into(),
        json!({"file_data": [{
            "destination": parent.join("other.json"), "format": "plain_text"
        }]}),
    );
    assert!(
        detector
            .file_data_format(&workspace, &parent.join("other.json"))
            .is_none()
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"{\"data\":\"retained original\"}"
    );
    // The same absolute text cannot renew a replaced directory's authority.
    std::fs::rename(&parent, workspace.join("retained-old-data")).unwrap();
    std::fs::create_dir(&parent).unwrap();
    assert_eq!(captured.recheck(), Err(PolicyError::Source));
    assert!(
        detector
            .file_data_format(&workspace, &destination)
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn captured_file_rule_refuses_symlinks_absent_parents_and_replaced_user_config() {
    use std::os::unix::fs::symlink;
    let root_owner = private_directory();
    let workspace_owner = private_directory();
    let outside_owner = private_directory();
    let root = root_owner.path().canonicalize().unwrap();
    let workspace = workspace_owner.path().canonicalize().unwrap();
    let linked = workspace.join("data");
    symlink(outside_owner.path(), &linked).unwrap();
    let value = user_policy(&workspace, &linked.join("answer.txt"));
    create_config(&root, &value);
    assert!(matches!(
        CapturedPolicy::capture_from_root(ConfigRoot::capture_owned_test_root(&root).unwrap()),
        Err(PolicyError::Source)
    ));
    // Only test-owned files are removed; no partial key/vault is adopted.
    std::fs::remove_file(root.join("config.json")).unwrap();
    let absent = workspace.join("missing-parent/answer.txt");
    let mut value = user_policy(&workspace, &absent);
    create_config(&root, &value);
    assert!(matches!(
        CapturedPolicy::capture_from_root(ConfigRoot::capture_owned_test_root(&root).unwrap()),
        Err(PolicyError::Source)
    ));
    std::fs::remove_file(root.join("config.json")).unwrap();
    value["privacy"]["enabled"] = json!(false);
    create_config(&root, &value);
    let disabled =
        CapturedPolicy::capture_from_root(ConfigRoot::capture_owned_test_root(&root).unwrap())
            .unwrap();
    assert!(disabled.require_available().is_ok());
    assert!(disabled.proposed_detector().unwrap().is_none());
    std::fs::rename(root.join("config.json"), root.join("retained-old-config")).unwrap();
    create_config(&root, &value);
    assert_eq!(disabled.recheck(), Err(PolicyError::Source));
}
