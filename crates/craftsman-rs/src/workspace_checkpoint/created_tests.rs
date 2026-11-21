//! Actual manager/store/transaction controls for one checkpoint's creation time.
use super::*;
use serde_json::json;
use std::time::Duration;

fn file(path: &str, after: &[u8]) -> WorkspaceCheckpointFile {
    WorkspaceCheckpointFile {
        path: path.into(),
        kind: WorkspaceChangeKind::Modified,
        before_hash: Some(format!("{:x}", Sha256::digest(b"before"))),
        after_hash: Some(format!("{:x}", Sha256::digest(after))),
        before_mode: Some(0o644),
        after_mode: Some(0o600),
        before_content: Some(b"before".to_vec()),
        after_content: Some(after.to_vec()),
        restorable: true,
    }
}
fn checkpoint(id: &str, created: i64, after: &[u8]) -> NewWorkspaceCheckpoint {
    NewWorkspaceCheckpoint {
        id: id.into(),
        thread_id: "creation-thread".into(),
        turn_seq: 7,
        created,
        summary: "first contents".into(),
        partial: false,
        files: vec![file("record.txt", after)],
    }
}
fn summary(value: &WorkspaceCheckpointSummary) -> serde_json::Value {
    json!({"id":value.id,"created":value.created,"turn_seq":value.turn_seq,"summary":value.summary,"file_count":value.file_count,"partial":value.partial})
}
fn record(name: &str, value: serde_json::Value) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let directory = root
        .join("target/verification/workspace-created-stability/actual")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&directory).unwrap();
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(format!("{name}.json")))
        .unwrap();
    file.write_all(&serde_json::to_vec_pretty(&value).unwrap())
        .unwrap();
    file.sync_all().unwrap();
}
async fn cross_real_second(first: i64) -> i64 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let now = unix_secs();
            if now > first {
                return now;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned new clock-boundary control did not cross a real second")
}

#[tokio::test]
async fn one_id_keeps_creation_order_while_real_sql_updates_all_current_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("threads.db");
    let store = LibsqlCheckpointer::open_at(path.clone()).await.unwrap();
    store
        .save_workspace_checkpoint(&checkpoint("older", 100, b"first"))
        .await
        .unwrap();
    store
        .save_workspace_checkpoint(&checkpoint("newer", 200, b"newer"))
        .await
        .unwrap();
    let mut update = checkpoint("older", 300, b"updated binary\0\xff");
    update.summary = "updated summary".into();
    update.partial = true;
    update.files.push(file("additional.txt", b"additional"));
    store.save_workspace_checkpoint(&update).await.unwrap();
    let list = store
        .list_workspace_checkpoints("creation-thread")
        .await
        .unwrap();
    let loaded = store
        .load_workspace_checkpoint("creation-thread", Some("older"))
        .await
        .unwrap()
        .unwrap();
    record(
        "same-id-update",
        json!({"proposed_created":300,"actual":summary(&loaded.summary),"order":list.iter().map(summary).collect::<Vec<_>>(),"current_file_paths":loaded.files.iter().map(|f|&f.path).collect::<Vec<_>>()}),
    );
    assert_eq!(loaded.summary.created, 100);
    assert_eq!(
        list.iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["newer", "older"]
    );
    assert_eq!(loaded.summary.turn_seq, 7);
    assert_eq!(loaded.summary.summary, "updated summary");
    assert!(loaded.summary.partial);
    assert_eq!(loaded.summary.file_count, 2);
    let current = loaded
        .files
        .iter()
        .find(|f| f.path == "record.txt")
        .unwrap();
    assert_eq!(
        current.before_content.as_deref(),
        Some(b"before".as_slice())
    );
    assert_eq!(
        current.after_content.as_deref(),
        Some(b"updated binary\0\xff".as_slice())
    );
    assert_eq!(current.before_mode, Some(0o644));
    assert_eq!(current.after_mode, Some(0o600));
    assert!(current.restorable);
    assert_eq!(
        current.after_hash.as_deref(),
        Some(format!("{:x}", Sha256::digest(b"updated binary\0\xff")).as_str())
    );
    drop(store);
    let reopened = LibsqlCheckpointer::open_at(path).await.unwrap();
    assert_eq!(
        reopened
            .list_workspace_checkpoints("creation-thread")
            .await
            .unwrap(),
        list
    );
}

#[tokio::test]
async fn a_failed_update_rolls_back_files_and_metadata_without_rebinding_creation() {
    let directory = tempfile::tempdir().unwrap();
    let store = LibsqlCheckpointer::open_at(directory.path().join("threads.db"))
        .await
        .unwrap();
    store
        .save_workspace_checkpoint(&checkpoint("retained", 100, b"first"))
        .await
        .unwrap();
    let prior = store
        .list_workspace_checkpoints("creation-thread")
        .await
        .unwrap();
    store.connection().execute("CREATE TRIGGER refuse_owned_file BEFORE INSERT ON workspace_checkpoint_files WHEN NEW.path = 'refused.txt' BEGIN SELECT RAISE(ABORT, 'owned insert failure'); END",()).await.unwrap();
    let mut update = checkpoint("retained", 500, b"uncommitted");
    update.summary = "uncommitted summary".into();
    update.files = vec![file("refused.txt", b"uncommitted")];
    assert!(store.save_workspace_checkpoint(&update).await.is_err());
    assert_eq!(
        store
            .list_workspace_checkpoints("creation-thread")
            .await
            .unwrap(),
        prior
    );
    let retained = store
        .load_workspace_checkpoint("creation-thread", Some("retained"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.files.len(), 1);
    assert_eq!(retained.files[0].path, "record.txt");
    assert_eq!(
        retained.files[0].after_content.as_deref(),
        Some(b"first".as_slice())
    );
    let mut new = update.clone();
    new.id = "new-after-failure".into();
    assert!(store.save_workspace_checkpoint(&new).await.is_err());
    assert!(
        store
            .load_workspace_checkpoint("creation-thread", Some(&new.id))
            .await
            .unwrap()
            .is_none()
    );
    store
        .connection()
        .execute("DROP TRIGGER refuse_owned_file", ())
        .await
        .unwrap();
    update.created = 600;
    update.files = vec![file("record.txt", b"committed")];
    store.save_workspace_checkpoint(&update).await.unwrap();
    new.created = 700;
    new.files = vec![file("record.txt", b"new committed")];
    store.save_workspace_checkpoint(&new).await.unwrap();
    let actual = store
        .load_workspace_checkpoint("creation-thread", Some("retained"))
        .await
        .unwrap()
        .unwrap();
    let fresh = store
        .load_workspace_checkpoint("creation-thread", Some(&new.id))
        .await
        .unwrap()
        .unwrap();
    record(
        "failed-transaction",
        json!({"actual_old":summary(&actual.summary),"actual_new":summary(&fresh.summary),"earlier_failure_rolled_back":true}),
    );
    assert_eq!(actual.summary.created, 100);
    assert_eq!(
        actual.files[0].after_content.as_deref(),
        Some(b"committed".as_slice())
    );
    assert_eq!(fresh.summary.created, 700);
}

#[tokio::test]
async fn updating_an_old_id_does_not_extend_the_existing_creation_retention_window() {
    let directory = tempfile::tempdir().unwrap();
    let store = LibsqlCheckpointer::open_at(directory.path().join("threads.db"))
        .await
        .unwrap();
    for index in 0..20 {
        store
            .save_workspace_checkpoint(&checkpoint(&format!("owned-{index:02}"), index, b"record"))
            .await
            .unwrap();
    }
    let mut other = checkpoint("other-thread", 99, b"other");
    other.thread_id = "other-thread".into();
    store.save_workspace_checkpoint(&other).await.unwrap();
    store
        .save_workspace_checkpoint(&checkpoint("owned-00", 1000, b"old updated"))
        .await
        .unwrap();
    store
        .save_workspace_checkpoint(&checkpoint("owned-20", 20, b"new record"))
        .await
        .unwrap();
    let list = store
        .list_workspace_checkpoints("creation-thread")
        .await
        .unwrap();
    record(
        "retention",
        json!({"order":list.iter().map(summary).collect::<Vec<_>>(),"oldest_updated_proposal":1000,"retained_count":list.len()}),
    );
    assert_eq!(list.len(), 20);
    assert_eq!(list[0].id, "owned-20");
    assert_eq!(list.last().unwrap().id, "owned-01");
    assert!(
        store
            .load_workspace_checkpoint("creation-thread", Some("owned-00"))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .list_workspace_checkpoints("other-thread")
            .await
            .unwrap()
            .len(),
        1
    );
    let mut rows = store
        .connection()
        .query(
            "SELECT COUNT(*) FROM workspace_checkpoint_files WHERE checkpoint_id = 'owned-00'",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn later_unreadable_turn_end_preserves_the_whole_record_across_a_real_second() {
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
    let later = cross_real_second(recorded[0].created).await;
    let denied = RestorePermissions::deny(&path);
    assert_eq!(
        std::fs::File::open(&path).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    manager.turn_end(&mut state).await.unwrap();
    let after = manager.list().await.unwrap();
    record(
        "unreadable-second-boundary",
        json!({"before":recorded.iter().map(summary).collect::<Vec<_>>(),"after":after.iter().map(summary).collect::<Vec<_>>(),"actual_later_second":later,"permission_denied_confirmed":true}),
    );
    assert_eq!(after, recorded);
    drop(denied);
    assert_eq!(
        manager.rewind(None, false).await.unwrap().restored,
        ["file.txt"]
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "before");
}

#[tokio::test]
async fn manager_updates_keep_first_creation_and_a_new_turn_gets_a_new_id() {
    let directory = tempfile::tempdir().unwrap();
    let manager = manager(&directory).await;
    let path = manager.root.join("file.txt");
    std::fs::write(&path, "before").unwrap();
    let mut state = state();
    manager.turn_start(&mut state).await.unwrap();
    let first = manager.begin_file_mutation("file.txt", "edit".into()).await;
    std::fs::write(&path, "first edit").unwrap();
    manager.finish_mutation(first).await;
    let initial = manager.list().await.unwrap()[0].clone();
    cross_real_second(initial.created).await;
    let next = manager
        .begin_file_mutation("file.txt", "edit again".into())
        .await;
    std::fs::write(&path, "second edit").unwrap();
    manager.finish_mutation(next).await;
    manager.turn_end(&mut state).await.unwrap();
    let updated = manager.list().await.unwrap()[0].clone();
    record(
        "manager-update",
        json!({"before":summary(&initial),"after":summary(&updated)}),
    );
    assert_eq!(updated.created, initial.created);
    assert_eq!(updated.id, initial.id);
    assert_eq!(updated.turn_seq, 1);
    let stored = manager
        .store
        .load_workspace_checkpoint("scoped-thread", Some(&initial.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.files[0].before_content.as_deref(),
        Some(b"before".as_slice())
    );
    assert_eq!(
        stored.files[0].after_hash.as_deref(),
        Some(format!("{:x}", Sha256::digest(b"second edit")).as_str())
    );
    assert_eq!(stored.files[0].after_content, None);
    state.messages.push(Msg::user("a different actual turn"));
    manager.turn_start(&mut state).await.unwrap();
    let new = manager
        .begin_file_mutation("file.txt", "new turn".into())
        .await;
    std::fs::write(&path, "new turn edit").unwrap();
    manager.finish_mutation(new).await;
    manager.turn_end(&mut state).await.unwrap();
    let list = manager.list().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_ne!(list[0].id, initial.id);
    assert_eq!(list[0].turn_seq, 2);
    assert!(list[0].created > initial.created);
}
