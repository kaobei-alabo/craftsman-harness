//! Real local-file snapshots, scope selection and partial-rewind regressions.
use super::*;

async fn manager(directory: &tempfile::TempDir) -> WorkspaceCheckpointManager {
    let store = Arc::new(
        LibsqlCheckpointer::open_at(directory.path().join("threads.db"))
            .await
            .unwrap(),
    );
    let root = directory.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    WorkspaceCheckpointManager::new(root, store, "scoped-thread")
        .with_warning_sink(Some(Arc::new(|_| {})))
}

fn state() -> AgentState {
    let mut state = AgentState::default();
    state.messages.push(Msg::user("Work with these files."));
    state
}

#[tokio::test]
async fn read_only_turn_never_opens_the_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let manager = manager(&directory).await;
    std::fs::remove_dir(&manager.root).unwrap();
    let mut state = state();
    manager.turn_start(&mut state).await.unwrap();
    {
        let active = manager.before.lock().unwrap().clone().unwrap();
        let snapshots = active.snapshots.lock().await;
        assert!(snapshots.selection.directories.is_empty());
        assert!(snapshots.selection.files.is_empty());
        assert!(snapshots.before.files.is_empty());
    }
    manager.turn_end(&mut state).await.unwrap();
    assert!(manager.list().await.unwrap().is_empty());
    assert!(!manager.root.exists());
}

#[tokio::test]
async fn targeted_edits_keep_first_bytes_across_unrelated_non_git_folders() {
    let directory = tempfile::tempdir().unwrap();
    let manager = manager(&directory).await;
    std::fs::create_dir(manager.root.join("documents")).unwrap();
    std::fs::create_dir(manager.root.join("other-project")).unwrap();
    let first = manager.root.join("documents/report.md");
    let second = manager.root.join("other-project/data.txt");
    std::fs::write(&first, "original report").unwrap();
    std::fs::write(&second, "original data").unwrap();
    let mut state = state();
    manager.turn_start(&mut state).await.unwrap();
    let ticket = manager
        .begin_file_mutation(first.to_str().unwrap(), "edit report".into())
        .await;
    assert!(ticket.is_some());
    {
        let active = ticket.as_ref().unwrap();
        let snapshots = active.snapshots.lock().await;
        assert_eq!(
            snapshots.before.files.keys().cloned().collect::<Vec<_>>(),
            ["documents/report.md"]
        );
        assert!(snapshots.selection.directories.is_empty());
    }
    std::fs::write(&first, "first revision").unwrap();
    manager.finish_mutation(ticket).await;
    let ticket = manager
        .begin_file_mutation(second.to_str().unwrap(), "edit data".into())
        .await;
    std::fs::write(&second, "new data").unwrap();
    manager.finish_mutation(ticket).await;
    let ticket = manager
        .begin_file_mutation(first.to_str().unwrap(), "edit report again".into())
        .await;
    std::fs::write(&first, "second revision").unwrap();
    manager.finish_mutation(ticket).await;
    manager.turn_end(&mut state).await.unwrap();
    let checkpoints = manager.list().await.unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].file_count, 2);
    assert!(checkpoints[0].partial);
    let report = manager.rewind(None, false).await.unwrap();
    assert_eq!(report.restored.len(), 2);
    assert_eq!(std::fs::read_to_string(first).unwrap(), "original report");
    assert_eq!(std::fs::read_to_string(second).unwrap(), "original data");
}

#[tokio::test]
async fn shell_scope_uses_specific_tool_context_and_can_add_another_folder() {
    let directory = tempfile::tempdir().unwrap();
    let manager = manager(&directory).await;
    for name in ["one", "two", "three", "unrelated"] {
        std::fs::create_dir(manager.root.join(name)).unwrap();
        std::fs::write(manager.root.join(name).join("file.txt"), name).unwrap();
    }
    let mut state = state();
    manager.turn_start(&mut state).await.unwrap();
    manager.note_directory(".").await;
    manager.note_file("one/file.txt").await;
    manager.note_directory("two").await;
    let ticket = manager
        .begin_mutation("first command".into())
        .await
        .unwrap();
    {
        let snapshots = ticket.snapshots.lock().await;
        assert_eq!(
            snapshots.selection.directories,
            BTreeSet::from(["one".into(), "two".into()])
        );
        assert_eq!(snapshots.before.files.len(), 2);
        assert!(!snapshots.before.files.contains_key("unrelated/file.txt"));
    }
    std::fs::write(manager.root.join("one/file.txt"), "changed one").unwrap();
    manager.finish_mutation(Some(ticket)).await;
    manager.note_file("three/file.txt").await;
    let ticket = manager
        .begin_mutation("second command".into())
        .await
        .unwrap();
    assert!(
        ticket
            .snapshots
            .lock()
            .await
            .selection
            .directories
            .contains("three")
    );
    std::fs::write(manager.root.join("three/file.txt"), "changed three").unwrap();
    manager.finish_mutation(Some(ticket)).await;
    manager.turn_end(&mut state).await.unwrap();
    assert_eq!(manager.list().await.unwrap()[0].file_count, 2);
    assert_eq!(manager.rewind(None, false).await.unwrap().restored.len(), 2);
    assert_eq!(
        std::fs::read_to_string(manager.root.join("unrelated/file.txt")).unwrap(),
        "unrelated"
    );
}

#[test]
fn git_listing_uses_literal_directory_scopes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .output()
            .unwrap()
            .status
            .success()
    );
    std::fs::create_dir(root.join("folder [one]")).unwrap();
    std::fs::create_dir(root.join("folder o")).unwrap();
    std::fs::write(root.join("folder [one]/note.md"), "document").unwrap();
    std::fs::write(root.join("folder o/secret.txt"), "unrelated").unwrap();
    let snapshot = capture_selection(
        root,
        &SnapshotSelection {
            directories: BTreeSet::from(["folder [one]".into()]),
            ..SnapshotSelection::default()
        },
        0,
    )
    .unwrap();
    assert_eq!(
        snapshot.files.keys().cloned().collect::<Vec<_>>(),
        ["folder [one]/note.md"]
    );
    assert!(!snapshot.covers("folder o/secret.txt"));
}

#[test]
fn snapshot_byte_budget_skips_an_oversized_file_without_dropping_small_files() {
    let directory = tempfile::tempdir().unwrap();
    let large = std::fs::File::create(directory.path().join("large.bin")).unwrap();
    large.set_len(MAX_SNAPSHOT_BYTES + 1).unwrap();
    std::fs::write(directory.path().join("small.txt"), "keep this").unwrap();
    let snapshot = capture_snapshot(directory.path()).unwrap();
    assert!(snapshot.partial);
    assert!(!snapshot.covers("large.bin"));
    assert_eq!(
        snapshot.files["small.txt"].content.as_deref(),
        Some(b"keep this".as_slice())
    );
}

#[test]
fn file_count_budget_prioritizes_explicit_targets() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..=MAX_SCAN_FILES {
        std::fs::write(directory.path().join(format!("{index:04}.txt")), "x").unwrap();
    }
    std::fs::write(directory.path().join("selected.txt"), "selected").unwrap();
    let snapshot = capture_selection(
        directory.path(),
        &SnapshotSelection {
            directories: BTreeSet::from([String::new()]),
            files: BTreeSet::from(["selected.txt".into()]),
            partial: false,
            ..SnapshotSelection::default()
        },
        0,
    )
    .unwrap();
    assert!(snapshot.partial);
    assert_eq!(snapshot.files.len(), MAX_SCAN_FILES);
    assert_eq!(
        snapshot.files["selected.txt"].content.as_deref(),
        Some(b"selected".as_slice())
    );
    assert!(!snapshot.covers("1024.txt"));
}

#[cfg(unix)]
struct RestorePermissions(PathBuf, std::fs::Permissions);

#[test]
fn directory_entry_limit_marks_unvisited_paths_unknown() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..=MAX_SCAN_ENTRIES {
        std::fs::write(directory.path().join(format!("{index:05}.txt")), "").unwrap();
    }
    let listed = enumerate_paths(directory.path(), &BTreeSet::from([String::new()])).unwrap();
    assert!(listed.partial);
    assert!(listed.paths.len() <= MAX_SCAN_ENTRIES);
    assert!(listed.unavailable.contains(""));
    let snapshot = capture_snapshot(directory.path()).unwrap();
    assert!(snapshot.partial);
    assert!(snapshot.files.len() <= MAX_SCAN_FILES);
    assert!(!snapshot.covers("not-observed.txt"));
}

#[test]
fn git_listing_output_limit_observes_the_child_return() {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        "tui::session_bridge::checkpoint_tests::native_checkpoint_ui_fixture",
        "--nocapture",
        "craftsman-checkpoint-ui=listing",
    ]);
    let (output, limited) = bounded_git_listing(command).unwrap().unwrap();
    assert!(limited);
    assert_eq!(output.len() as u64, MAX_GIT_LIST_BYTES + 1);
}

#[cfg(unix)]
impl RestorePermissions {
    fn deny(path: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let prior = std::fs::metadata(path).unwrap().permissions();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0)).unwrap();
        Self(path.to_path_buf(), prior)
    }
}

#[cfg(unix)]
impl Drop for RestorePermissions {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, self.1.clone());
    }
}

#[cfg(unix)]
#[test]
fn inaccessible_subtree_is_unknown_instead_of_a_false_creation_or_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let protected = root.join("protected");
    std::fs::create_dir(&protected).unwrap();
    std::fs::write(protected.join("note.txt"), "protected").unwrap();
    std::fs::write(root.join("visible.txt"), "before").unwrap();
    let before = capture_snapshot(root).unwrap();
    let denied = RestorePermissions::deny(&protected);
    assert_eq!(
        std::fs::read_dir(&protected).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    std::fs::write(root.join("visible.txt"), "after").unwrap();
    let during = capture_snapshot(root).unwrap();
    assert!(during.partial);
    assert!(!during.covers("protected/note.txt"));
    let changes = diff_snapshots(&before, &during);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "visible.txt");
    assert_eq!(changes[0].kind, WorkspaceChangeKind::Modified);
    drop(denied);
    let reopened = capture_snapshot(root).unwrap();
    assert!(diff_snapshots(&during, &reopened).is_empty());
    assert_eq!(
        std::fs::read_to_string(protected.join("note.txt")).unwrap(),
        "protected"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_later_unreadable_file_does_not_delete_the_last_recorded_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let manager = manager(&directory).await;
    let path = manager.root.join("file.txt");
    std::fs::write(&path, "before").unwrap();
    let mut state = state();
    manager.turn_start(&mut state).await.unwrap();
    let ticket = manager.begin_file_mutation("file.txt", "edit".into()).await;
    std::fs::write(&path, "after").unwrap();
    manager.finish_mutation(ticket).await;
    let recorded = manager.list().await.unwrap();
    assert_eq!(recorded.len(), 1);
    let denied = RestorePermissions::deny(&path);
    assert_eq!(
        std::fs::File::open(&path).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    manager.turn_end(&mut state).await.unwrap();
    assert_eq!(manager.list().await.unwrap(), recorded);
    drop(denied);
    assert_eq!(
        manager.rewind(None, false).await.unwrap().restored,
        ["file.txt"]
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "before");
}

#[cfg(unix)]
#[test]
fn operation_not_permitted_is_classified_as_permission_denied() {
    assert_eq!(
        std::io::Error::from_raw_os_error(libc::EPERM).kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

#[test]
fn directory_file_type_changes_are_not_claimed_as_creation_or_deletion() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("was-directory")).unwrap();
    std::fs::write(directory.path().join("was-file"), "before").unwrap();
    let before = capture_snapshot(directory.path()).unwrap();
    std::fs::remove_dir(directory.path().join("was-directory")).unwrap();
    std::fs::write(directory.path().join("was-directory"), "now a file").unwrap();
    std::fs::remove_file(directory.path().join("was-file")).unwrap();
    std::fs::create_dir(directory.path().join("was-file")).unwrap();
    let after = capture_snapshot(directory.path()).unwrap();
    assert!(!before.covers("was-directory"));
    assert!(!after.covers("was-file"));
    assert!(diff_snapshots(&before, &after).is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn partial_observation_preserves_previous_files_while_recording_another_edit() {
    let directory = tempfile::tempdir().unwrap();
    let manager = manager(&directory).await;
    let first = manager.root.join("first.txt");
    let second = manager.root.join("second.txt");
    std::fs::write(&first, "first before").unwrap();
    std::fs::write(&second, "second before").unwrap();
    let mut state = state();
    manager.turn_start(&mut state).await.unwrap();
    let ticket = manager
        .begin_file_mutation("first.txt", "first edit".into())
        .await;
    std::fs::write(&first, "first after").unwrap();
    manager.finish_mutation(ticket).await;
    let denied = RestorePermissions::deny(&first);
    assert_eq!(
        std::fs::File::open(&first).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let ticket = manager
        .begin_file_mutation("second.txt", "second edit".into())
        .await;
    std::fs::write(&second, "second after").unwrap();
    manager.finish_mutation(ticket).await;
    manager.turn_end(&mut state).await.unwrap();
    assert_eq!(manager.list().await.unwrap()[0].file_count, 2);
    drop(denied);
    assert_eq!(manager.rewind(None, false).await.unwrap().restored.len(), 2);
    assert_eq!(std::fs::read_to_string(first).unwrap(), "first before");
    assert_eq!(std::fs::read_to_string(second).unwrap(), "second before");
}

#[tokio::test]
async fn the_live_database_is_never_snapshotted_or_rewound_as_a_workspace_file() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LibsqlCheckpointer::open_at(directory.path().join("threads.db"))
            .await
            .unwrap(),
    );
    let manager =
        WorkspaceCheckpointManager::new(directory.path().to_path_buf(), store, "self-store")
            .with_warning_sink(Some(Arc::new(|_| {})));
    std::fs::write(directory.path().join("document.txt"), "before").unwrap();
    let mut state = state();
    manager.turn_start(&mut state).await.unwrap();
    let ticket = manager
        .begin_mutation("edit document".into())
        .await
        .unwrap();
    assert!(
        ticket
            .snapshots
            .lock()
            .await
            .before
            .files
            .keys()
            .all(|path| !path.starts_with("threads.db"))
    );
    std::fs::write(directory.path().join("document.txt"), "after").unwrap();
    manager.finish_mutation(Some(ticket)).await;
    manager.turn_end(&mut state).await.unwrap();
    assert_eq!(manager.list().await.unwrap()[0].file_count, 1);
    manager
        .store
        .save_workspace_checkpoint(&NewWorkspaceCheckpoint {
            id: "old-self-store".into(),
            thread_id: "self-store".into(),
            turn_seq: 2,
            created: 2,
            summary: "old unsafe record".into(),
            partial: false,
            files: vec![WorkspaceCheckpointFile {
                path: "threads.db".into(),
                kind: WorkspaceChangeKind::Modified,
                before_hash: Some("before".into()),
                after_hash: Some("after".into()),
                before_mode: None,
                after_mode: None,
                before_content: Some(b"invalid database bytes".to_vec()),
                after_content: None,
                restorable: true,
            }],
        })
        .await
        .unwrap();
    let report = manager.rewind(Some("old-self-store"), true).await.unwrap();
    assert!(report.restored.is_empty());
    assert_eq!(report.unrestorable, ["threads.db"]);
    assert_eq!(manager.list().await.unwrap().len(), 2);
}
