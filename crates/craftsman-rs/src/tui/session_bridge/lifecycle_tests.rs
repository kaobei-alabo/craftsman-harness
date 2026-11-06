use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

#[tokio::test]
async fn retiring_a_pending_session_retains_its_initializer_until_actual_cleanup() {
    struct Owned(Arc<AtomicBool>);
    impl Drop for Owned {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let root = craftsman_harness::SessionCancellation::new();
    let cancellation = root.child().unwrap();
    let started = Arc::new(Notify::new());
    let cleaning = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let live = cancellation.clone();
    let begin = started.clone();
    let cleanup = cleaning.clone();
    let finish = release.clone();
    let owned = dropped.clone();
    let handle = tokio::spawn(async move {
        let _owned = Owned(owned);
        begin.notify_one();
        live.cancelled().await;
        cleanup.notify_one();
        finish.notified().await;
        None
    });
    let (tx, _events) = mpsc::unbounded_channel();
    let mut slot = AgentSlot {
        inner: AgentSlotInner::Pending(handle),
        bridge: UiBridge { tx },
        root_cancellation: root.clone(),
        cancellation,
        retired: tokio::task::JoinSet::new(),
    };
    tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    slot.retire_previous();
    tokio::time::timeout(std::time::Duration::from_secs(1), cleaning.notified())
        .await
        .unwrap();
    assert!(!dropped.load(Ordering::SeqCst));
    assert_eq!(slot.retired.len(), 1);
    assert!(!root.is_cancelled());
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), slot.retired.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert!(slot.retired.is_empty());
}
