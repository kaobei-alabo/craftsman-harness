//! Ordinary source-aware human input and direct deadline/commit primitives.
//! No private MAIN callback/budget or retry-boundary scenario is injected.

use super::*;
use crate::privacy_entry::{HumanChoice, HumanDecisionProducer};

async fn parked_fixture(vault: &Vault, server: &MockServer) -> Gated {
    let g = gated(vault, server, "done").await;
    let text = format!("who handles {EMAIL}?");
    let node = g.c.session.admit_raw(&draft(&text)).await.unwrap();
    scripted(
        server,
        vec![
            asks(json!({"who":"them"})),
            answer("chat", &["fixture answer"]),
        ],
    )
    .await;
    g.gate.park.store(true, Ordering::SeqCst);
    assert!(matches!(
        g.agent
            .run_turn_with_view(text, g.c.session.source(&node).unwrap())
            .await
            .unwrap(),
        TurnResult::Suspended(_)
    ));
    g.gate.park.store(false, Ordering::SeqCst);
    g
}

#[tokio::test]
async fn equal_json_from_a_human_is_still_a_raw_human_edit() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let g = parked_fixture(&vault, &server).await;
    let parked = g.agent.actual_parked_call().unwrap();
    let raw = r#"{"who":"them"}"#;
    assert_eq!(
        serde_json::from_str::<Value>(raw).unwrap(),
        parked.call().args
    );
    let producer = HumanDecisionProducer::register(
        g.c.session.clone(),
        Arc::new(move |_, _| HumanChoice::Edit(raw.to_owned())),
    );
    let input = producer.receive(&parked).unwrap();
    g.agent
        .resume_with_view(
            input.action().unwrap().clone(),
            "Denied.",
            SourceRef::new(input),
        )
        .await
        .unwrap();
    let seen = g.tool.seen.lock().unwrap();
    assert_eq!(seen.args, [json!({"who":"them"})]);
    assert_eq!(seen.edited, [true]);
    assert!(seen.fields[0].is_empty());
    assert_eq!(producer.received().len(), 1);
}

#[tokio::test]
async fn raw_issued_alias_in_an_edit_refuses_effects_and_keeps_actual_input() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let g = parked_fixture(&vault, &server).await;
    let source = g.agent.recorded_sources().unwrap()[0].clone().unwrap();
    let node = g.c.session.admitted(&source).unwrap();
    let alias = issued(&node);
    let raw = json!({"who": alias}).to_string();
    let original = raw.clone();
    let producer = HumanDecisionProducer::register(
        g.c.session.clone(),
        Arc::new(move |_, _| HumanChoice::Edit(raw.clone())),
    );
    let input = producer
        .receive(&g.agent.actual_parked_call().unwrap())
        .unwrap();
    let action = input.action().unwrap().clone();
    assert!(
        g.agent
            .resume_with_view(action, "Denied.", SourceRef::new(input))
            .await
            .is_err()
    );
    assert!(g.tool.seen.lock().unwrap().args.is_empty());
    assert_eq!(recorded(&g.agent).len(), 2);
    assert_eq!(producer.received().len(), 1);
    assert_eq!(
        producer.received()[0].action(),
        Some(&HitlAction::ApproveEdited {
            args: serde_json::from_str(&original).unwrap()
        })
    );
    assert_eq!(g.c.session.check(), Err(SessionStop::Closed));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn absent_input_does_not_become_an_approval_or_default_answer() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let g = parked_fixture(&vault, &server).await;
    let parked = g.agent.actual_parked_call().unwrap();
    let producer =
        HumanDecisionProducer::register(g.c.session.clone(), Arc::new(|_, _| HumanChoice::Absent));
    let input = producer.receive(&parked).unwrap();
    assert!(input.was_absent());
    assert!(input.action().is_none());
    assert!(g.agent.actual_parked_call().unwrap().same_park(&parked));
    assert!(g.tool.seen.lock().unwrap().args.is_empty());
    assert_eq!(producer.received().len(), 1);
}

#[tokio::test]
async fn reading_always_does_not_apply_permission_until_its_real_decision_is_accepted() {
    let root = tempfile::tempdir().unwrap();
    let vault = vault(&root).await;
    let server = MockServer::start().await;
    let g = parked_fixture(&vault, &server).await;
    let parked = g.agent.actual_parked_call().unwrap();
    let effects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = effects.clone();
    let actual_call = parked.call().clone();
    let producer = HumanDecisionProducer::register_with_effect(
        g.c.session.clone(),
        Arc::new(|_, _| HumanChoice::ApproveAlways),
        Arc::new(move |choice, call| {
            assert!(matches!(choice, HumanChoice::ApproveAlways));
            assert_eq!(call, &actual_call);
            observed.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let input = producer.receive(&parked).unwrap();
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    // Direct actual-human decision primitive: no tool effect or main-loop
    // callback/budget failure is injected.
    g.c.boundary
        .accept_parked_decision(
            &parked,
            input.action().unwrap(),
            &SourceRef::new(input.clone()),
        )
        .await
        .unwrap();
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    input.apply_accepted_effect();
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert!(g.tool.seen.lock().unwrap().args.is_empty());
    assert_eq!(producer.received().len(), 1);
}
