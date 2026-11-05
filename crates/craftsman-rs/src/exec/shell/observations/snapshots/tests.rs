use super::super::Recorder;
use super::*;

fn fixture() -> (Recorder, Call, u64) {
    let recorder = Recorder::default();
    let call = recorder.begin();
    call.launch_returned(Some(123));
    let sequence = call.sequence.unwrap();
    (recorder, call, sequence)
}
#[test]
fn active_claim_and_before_stop_share_one_non_refunded_snapshot_ticket() {
    let (recorder, call, sequence) = fixture();
    let mut state = lock(&recorder.0);
    let first = claim(
        state.entries.back_mut().unwrap(),
        Phase::ActiveView,
        0,
        Instant::now(),
    )
    .unwrap();
    finish(&mut state, &first, "\nfixed-native-facts".into(), 1);
    assert!(
        note(
            &mut state,
            Some(sequence),
            Some(123),
            Some(ToolErrorKind::Timeout),
            5,
            call.started
        )
        .is_none()
    );
    let entry = state.entries.back().unwrap();
    assert!(entry.before_stop.is_some());
    assert!(matches!(
        entry.native_snapshot.as_ref().unwrap().phase,
        Phase::ActiveView
    ));
    assert_eq!(
        entry.native_snapshot.as_ref().unwrap().report.as_deref(),
        Some("\nfixed-native-facts")
    );
}
#[test]
fn before_stop_is_one_attempt_and_preserves_the_original_stop_kind() {
    let (recorder, call, sequence) = fixture();
    let mut state = lock(&recorder.0);
    assert!(
        note(
            &mut state,
            Some(sequence),
            Some(123),
            Some(ToolErrorKind::Timeout),
            1,
            call.started
        )
        .is_some()
    );
    assert!(
        note(
            &mut state,
            Some(sequence),
            Some(123),
            Some(ToolErrorKind::NeedsApproval),
            2,
            call.started
        )
        .is_none()
    );
    assert_eq!(
        state
            .entries
            .back()
            .unwrap()
            .before_stop
            .as_ref()
            .unwrap()
            .kind
            .label(),
        "timeout"
    );
    assert!(matches!(
        state
            .entries
            .back()
            .unwrap()
            .native_snapshot
            .as_ref()
            .unwrap()
            .phase,
        Phase::BeforeStop
    ));
}
#[test]
fn missing_stop_or_current_tree_pid_never_authorizes_a_saved_pid_query() {
    for (pid, stop, expected_note) in [
        (Some(123), None, false),
        (None, Some(ToolErrorKind::Timeout), true),
        (Some(456), Some(ToolErrorKind::Failed), true),
    ] {
        let (recorder, call, sequence) = fixture();
        let mut state = lock(&recorder.0);
        assert!(note(&mut state, Some(sequence), pid, stop, 1, call.started).is_none());
        let entry = state.entries.back().unwrap();
        assert_eq!(entry.before_stop.is_some(), expected_note);
        assert!(entry.native_snapshot.is_none());
        if let Some(note) = &entry.before_stop {
            assert!(!note.tree_pid_matches_recorded);
        }
    }
}
#[test]
fn released_and_evicted_entries_cannot_rebind_snapshot_work_to_new_calls() {
    let (recorder, call, sequence) = fixture();
    {
        let mut state = lock(&recorder.0);
        state.entries.back_mut().unwrap().scope_end_us = Some(1);
        assert!(
            claim(
                state.entries.back_mut().unwrap(),
                Phase::ActiveView,
                0,
                Instant::now()
            )
            .is_none()
        );
        assert!(
            note(
                &mut state,
                Some(sequence),
                Some(123),
                Some(ToolErrorKind::Timeout),
                2,
                call.started
            )
            .is_none()
        );
    }
    for _ in 0..super::super::RETAINED_CALLS {
        drop(recorder.begin());
    }
    let mut state = lock(&recorder.0);
    assert!(
        note(
            &mut state,
            Some(sequence),
            Some(123),
            Some(ToolErrorKind::Timeout),
            3,
            call.started
        )
        .is_none()
    );
    assert!(
        state
            .entries
            .iter()
            .all(|entry| entry.native_snapshot.is_none() && entry.before_stop.is_none())
    );
}
#[test]
fn saved_report_admission_never_clips_or_echoes_overbound_or_control_text() {
    assert_eq!(
        admitted_report(1, "a".repeat(MAX_SAVED_REPORT_BYTES)).len(),
        MAX_SAVED_REPORT_BYTES
    );
    for value in [
        "private-prefix".repeat(MAX_SAVED_REPORT_BYTES),
        "private\u{1b}value".into(),
        "private\u{80}value".into(),
    ] {
        assert_eq!(
            admitted_report(1, value),
            "\ncall=1 native_snapshot_report=refused"
        );
    }
}
#[test]
fn before_stop_pipe_facts_are_original_observations_not_later_drain_eof() {
    let (recorder, call, sequence) = fixture();
    let mut state = lock(&recorder.0);
    let entry = state.entries.back_mut().unwrap();
    entry.pipes[0].bytes = 23;
    entry.pipes[0].first_poll_us = Some(7);
    let ticket = note(
        &mut state,
        Some(sequence),
        Some(123),
        Some(ToolErrorKind::NeedsApproval),
        8,
        call.started,
    )
    .unwrap();
    state.entries.back_mut().unwrap().pipes[0].bytes = 24;
    state.entries.back_mut().unwrap().pipes[0].eof = true;
    let before = state.entries.back().unwrap().before_stop.as_ref().unwrap();
    assert_eq!(before.pipes[0].bytes, 23);
    assert!(!before.pipes[0].eof);
    assert_eq!(before.pipes[0].first_poll_us, Some(7));
    finish(&mut state, &ticket, "\nfixed-native-facts".into(), 9);
    let mut report = String::new();
    append_teardown(&mut report, &state);
    assert!(report.contains("before_stop_stdout_bytes=23 before_stop_stdout_eof=false"));
    assert!(report.contains("native_snapshot_phase=before-stop native_snapshot_query_count=1"));
}
#[test]
fn a_pending_claim_and_late_eviction_never_fabricate_native_return_or_completion() {
    let (recorder, call, sequence) = fixture();
    let ticket = {
        let mut state = lock(&recorder.0);
        note(
            &mut state,
            Some(sequence),
            Some(123),
            Some(ToolErrorKind::Timeout),
            1,
            call.started,
        )
        .unwrap()
    };
    {
        let state = lock(&recorder.0);
        assert!(
            state
                .entries
                .back()
                .unwrap()
                .native_snapshot
                .as_ref()
                .unwrap()
                .return_us
                .is_none()
        );
        let mut report = String::new();
        append_teardown(&mut report, &state);
        assert!(report.contains("native_snapshot=pending"));
    }
    for _ in 0..super::super::RETAINED_CALLS {
        drop(recorder.begin());
    }
    let mut state = lock(&recorder.0);
    finish(&mut state, &ticket, "\nfixed-native-facts".into(), 2);
    assert!(
        state
            .entries
            .iter()
            .all(|entry| entry.native_snapshot.is_none())
    );
}
#[test]
fn ordinary_active_reports_hide_released_cached_native_facts_but_teardown_retains_them() {
    let (recorder, call, sequence) = fixture();
    let mut state = lock(&recorder.0);
    let ticket = note(
        &mut state,
        Some(sequence),
        Some(123),
        Some(ToolErrorKind::Timeout),
        1,
        call.started,
    )
    .unwrap();
    finish(&mut state, &ticket, "\nfixed-native-facts".into(), 2);
    state.entries.back_mut().unwrap().scope_end_us = Some(3);
    let mut active = String::new();
    append_active(&mut active, &state);
    assert!(active.is_empty());
    let mut teardown = String::new();
    append_teardown(&mut teardown, &state);
    assert!(teardown.contains("fixed-native-facts"));
    assert!(teardown.contains("before_stop_decision=timeout"));
}
