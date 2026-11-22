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
        Arc::new(move |_| HumanChoice::Edit(raw.to_owned())),
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
        Arc::new(move |_| HumanChoice::Edit(raw.clone())),
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
        HumanDecisionProducer::register(g.c.session.clone(), Arc::new(|_| HumanChoice::Absent));
    let input = producer.receive(&parked).unwrap();
    assert!(input.was_absent());
    assert!(input.action().is_none());
    assert!(g.agent.actual_parked_call().unwrap().same_park(&parked));
    assert!(g.tool.seen.lock().unwrap().args.is_empty());
    assert_eq!(producer.received().len(), 1);
}
