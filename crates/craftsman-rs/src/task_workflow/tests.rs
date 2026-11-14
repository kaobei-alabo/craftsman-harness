//! Actual agent-loop hints preserve outcomes, dispatch, permissions and turn state.
use super::*;
use craftsman_harness::testing::ScriptedModel;
use craftsman_harness::{Agent, ModelRequest, Msg, Tool, ToolDef};
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Results {
    values: Mutex<VecDeque<ToolOutcome>>,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Tool for Results {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "fixture".into(),
            description: "fixture outcomes".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }
    async fn call(
        &self,
        _: serde_json::Value,
        _: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.values.lock().unwrap().pop_front().unwrap())
    }
}

fn call(id: usize) -> Msg {
    Msg::Assistant {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: format!("call-{id}"),
            name: "fixture".into(),
            args: serde_json::json!({"attempt":id}),
        }],
    }
}

fn agent(
    values: Vec<ToolOutcome>,
    followups: usize,
) -> (Agent, Arc<ScriptedModel>, Arc<AtomicUsize>) {
    let mut script: Vec<_> = (0..values.len()).map(call).collect();
    script.extend((0..followups).map(|_| Msg::assistant("done")));
    let model = Arc::new(ScriptedModel::new(script));
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder(model.clone())
        .middleware(Arc::new(TaskWorkflowMiddleware::default()))
        .tool(Arc::new(Results {
            values: Mutex::new(values.into()),
            calls: calls.clone(),
        }))
        .build();
    (agent, model, calls)
}

fn has_hint(request: &ModelRequest) -> bool {
    request.system.contains(REPEATED_ERROR_GUIDANCE)
}

#[tokio::test]
async fn repeated_actual_errors_add_fixed_guidance_without_retrying_or_blocking_calls() {
    let detail = "synthetic-private-detail";
    let (agent, model, calls) = agent(
        vec![
            ToolOutcome::err(detail),
            ToolOutcome::err(detail),
            ToolOutcome::err(detail),
        ],
        1,
    );
    agent.run_turn("complete the fixture").await.unwrap();
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(!has_hint(&requests[0]) && !has_hint(&requests[1]));
    assert!(has_hint(&requests[2]) && has_hint(&requests[3]));
    for request in requests.iter() {
        assert_eq!(request.system.matches(WORKFLOW_GUIDANCE).count(), 1);
        assert!(!request.system.contains(detail));
    }
    let results: Vec<_> = agent
        .state
        .lock()
        .unwrap()
        .messages
        .iter()
        .filter_map(|message| {
            if let Msg::Tool {
                content, is_error, ..
            } = message
            {
                Some((content.clone(), *is_error))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(results, vec![(detail.into(), true); 3]);
}

#[tokio::test]
async fn changed_errors_and_successful_operations_preserve_normal_iteration() {
    let (agent, model, calls) = agent(
        vec![
            ToolOutcome::err("first"),
            ToolOutcome::err("different"),
            ToolOutcome::ok("evidence"),
            ToolOutcome::err("different"),
            ToolOutcome::ok("evidence"),
            ToolOutcome::ok("evidence"),
        ],
        1,
    );
    agent.run_turn("complete").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    assert!(
        model
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !has_hint(request))
    );
}

#[tokio::test]
async fn permission_and_cancellation_outcomes_never_create_recovery_pressure() {
    for kind in [
        ToolErrorKind::NeedsApproval,
        ToolErrorKind::DeniedCapability,
        ToolErrorKind::Cancelled,
    ] {
        let mut meta = craftsman_harness::ToolMeta::new();
        meta.error_kind = Some(kind);
        let value = ToolOutcome::err("decision must remain in force").with_meta(meta);
        let (agent, model, calls) = agent(vec![value.clone(), value], 1);
        agent.run_turn("complete").await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(
            model
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| !has_hint(request))
        );
    }
}

#[tokio::test]
async fn successful_nonzero_exits_are_not_reclassified_from_harness_only_metadata() {
    let mut meta = craftsman_harness::ToolMeta::new();
    meta.exit_code = Some(1);
    let value = ToolOutcome::ok("expected absence").with_meta(meta);
    let (agent, model, _) = agent(vec![value.clone(), value], 1);
    agent.run_turn("check absence").await.unwrap();
    assert!(
        model
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !has_hint(request))
    );
    assert!(
        agent
            .state
            .lock()
            .unwrap()
            .messages
            .iter()
            .all(|message| !matches!(message, Msg::Tool { is_error: true, .. }))
    );
}

#[tokio::test]
async fn oversized_error_results_remain_original_without_advisory_hash_work() {
    let error = "x".repeat(MAX_ERROR_BYTES + 1);
    let (agent, model, _) = agent(
        vec![
            ToolOutcome::err(error.clone()),
            ToolOutcome::err(error.clone()),
        ],
        1,
    );
    agent.run_turn("complete").await.unwrap();
    assert!(
        model
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !has_hint(request))
    );
    let state = agent.state.lock().unwrap();
    assert_eq!(
        state
            .messages
            .iter()
            .filter(|m| matches!(m, Msg::Tool { content, .. } if content == &error))
            .count(),
        2
    );
}

#[tokio::test]
async fn workflow_refreshes_each_turn_without_serializing_progress() {
    let (agent, model, _) = agent(vec![ToolOutcome::err("same"), ToolOutcome::err("same")], 2);
    agent.run_turn("first task").await.unwrap();
    agent.run_turn("next task").await.unwrap();
    let requests = model.requests.lock().unwrap();
    assert!(has_hint(&requests[2]));
    assert!(!has_hint(&requests[3]));
    assert!(agent.state.lock().unwrap().custom.is_empty());
}

#[tokio::test]
async fn model_failures_remain_errors_without_extra_sampling_or_dispatch() {
    let (agent, model, calls) = agent(vec![ToolOutcome::err("same")], 0);
    assert!(matches!(
        agent.run_turn("complete").await,
        Err(AgentError::Model(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 2);
}
