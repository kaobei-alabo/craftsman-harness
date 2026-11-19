//! Pure per-built-Agent identity semantics; no IO, model call or authority grant.
use async_trait::async_trait;
use craftsman_harness::testing::ScriptedModel;
use craftsman_harness::{
    Agent, AgentError, AgentRegistry, AgentState, Backend, StateFs, Tool, ToolCtx, ToolDef,
    ToolOutcome,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct Probe;
#[async_trait]
impl Tool for Probe {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "identity-probe".into(),
            description: "Never dispatched by identity tests.".into(),
            parameters: json!({"type":"object","properties":{}}),
        }
    }
    async fn call(&self, _args: Value, _ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        panic!("identity comparison must not dispatch a tool");
    }
}

#[test]
fn built_agent_identity_is_stable_across_clones_moves_and_threads() {
    let model = Arc::new(ScriptedModel::new(vec![]));
    let state = Arc::new(Mutex::new(AgentState::default()));
    let backend: Arc<dyn Backend> = Arc::new(StateFs::new(state.clone()));
    let agent = Agent::builder(model.clone())
        .state(state)
        .backend(backend.clone())
        .build();
    let identity = agent.identity();
    let cloned = agent.clone();
    let moved = Box::new(agent);
    assert!(identity.matches(&cloned));
    assert!(identity.matches(&moved));
    assert!(identity.matches_backend(&moved, &backend));
    let witness = identity.clone();
    let retained = backend.clone();
    std::thread::spawn(move || {
        assert!(witness.matches(&cloned));
        assert!(witness.matches_backend(&cloned, &retained));
    })
    .join()
    .unwrap();
    assert!(model.requests.lock().unwrap().is_empty());
}

#[test]
fn fresh_builds_sharing_state_backend_registry_tool_and_names_get_distinct_identities() {
    let model = Arc::new(ScriptedModel::new(vec![]));
    let state = Arc::new(Mutex::new(AgentState::default()));
    let backend: Arc<dyn Backend> = Arc::new(StateFs::new(state.clone()));
    let registry = Arc::new(AgentRegistry::default());
    let tool: Arc<dyn Tool> = Arc::new(Probe);
    let make = || {
        Agent::builder(model.clone())
            .name("same-name")
            .thread_id("same-thread")
            .state(state.clone())
            .backend(backend.clone())
            .registry(registry.clone())
            .tool(tool.clone())
            .build()
    };
    let first = make();
    let second = make();
    assert!(Arc::ptr_eq(&first.state, &second.state));
    assert!(first.registered_tool_matches("identity-probe", &tool));
    assert!(second.registered_tool_matches("identity-probe", &tool));
    let identity = first.identity();
    assert!(!identity.matches(&second));
    assert!(!identity.matches_backend(&second, &backend));
    assert!(identity.matches(&first.clone()));
    assert!(model.requests.lock().unwrap().is_empty());
}

#[test]
fn identity_backend_check_uses_actual_immutable_agent_backend() {
    let model = Arc::new(ScriptedModel::new(vec![]));
    let state = Arc::new(Mutex::new(AgentState::default()));
    let actual: Arc<dyn Backend> = Arc::new(StateFs::new(state.clone()));
    let other: Arc<dyn Backend> = Arc::new(StateFs::new(state.clone()));
    let agent = Agent::builder(model.clone())
        .state(state)
        .backend(actual.clone())
        .build();
    let identity = agent.identity();
    assert!(identity.matches(&agent));
    assert!(identity.matches_backend(&agent, &actual));
    assert!(!identity.matches_backend(&agent, &other));
    assert!(model.requests.lock().unwrap().is_empty());
}
