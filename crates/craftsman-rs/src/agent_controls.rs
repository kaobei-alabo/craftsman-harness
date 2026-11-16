//! App-selected context and exact failure budgets over the Agent's real owners.

use crate::context_compiler::{
    CompactionObservation, ContextController, ContextOptions, InputStamp, MAX_OBSERVED_INPUT_BYTES,
};
use crate::failure_control::{
    Settings,
    governor::{
        BudgetDecision, FailureGovernor, PauseCandidate, PauseReceipt, TrustedClass,
        bounded_operation,
    },
};
use async_trait::async_trait;
use craftsman_harness::{
    Agent, AgentBoundaryPolicy, AgentError, AgentMiddleware, AgentState, HumanInputKind,
    ModelBoundaryAction, Msg, Tool, ToolCall, ToolCtx, ToolNext, ToolOutcome,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const UNAVAILABLE: &str =
    "Selected failure-control input or observation is unavailable; further sampling was stopped.";
const PAUSED: &str = "The selected repeated-operation failure budget paused further sampling. Completed work and actual results are retained; send a follow-up to continue.";
const INPUT_KEY: &str = "_craftsman_failure_input_v1";
// PROVENANCE(P5): original bounded failure-control guidance.
const ADVISORY: &str = "The same admitted operation returned the same error repeatedly. Inspect completed and possible partial effects, then use new evidence or a different permitted approach. Exact failure budgets cover only trusted observational or pre-execution failures; unknown effects remain advisory.";

pub(crate) struct AppControls {
    pub(crate) context: Arc<ContextController>,
    governor: Option<FailureGovernor>,
    input: Mutex<Option<InputStamp>>,
    candidate: Mutex<Option<PauseCandidate>>,
    observational: Mutex<BTreeSet<String>>,
    file_tools: Mutex<BTreeSet<String>>,
    registered: Mutex<Option<BTreeSet<String>>>,
    runtime_changed: AtomicBool,
    unavailable: AtomicBool,
}

impl AppControls {
    pub(crate) fn new(
        context: ContextOptions,
        failure: Option<&Settings>,
        workspace: &Path,
    ) -> Result<Arc<Self>, AgentError> {
        Ok(Arc::new(Self {
            context: Arc::new(ContextController::new(context, workspace)?),
            governor: failure.map(FailureGovernor::new),
            input: Mutex::new(None),
            candidate: Mutex::new(None),
            observational: Mutex::new(BTreeSet::new()),
            file_tools: Mutex::new(BTreeSet::new()),
            registered: Mutex::new(None),
            runtime_changed: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
        }))
    }

    pub(crate) fn bind_tools(
        &self,
        agent: &Agent,
        observational: &[Arc<dyn Tool>],
        file_tools: &[Arc<dyn Tool>],
    ) {
        let select = |tools: &[Arc<dyn Tool>]| {
            tools
                .iter()
                .filter_map(|tool| {
                    let name = tool.def().name;
                    agent.registered_tool_matches(&name, tool).then_some(name)
                })
                .collect()
        };
        *self.observational.lock().unwrap() = select(observational);
        *self.file_tools.lock().unwrap() = select(file_tools);
        let mut names = BTreeSet::new();
        let mut bytes = 0usize;
        let mut complete = true;
        for name in agent.registered_tool_names() {
            bytes = bytes.saturating_add(name.len());
            if name.len() > 512 || names.len() >= 1024 || bytes > 512 * 1024 {
                complete = false;
                break;
            }
            names.insert(name.to_owned());
        }
        *self.registered.lock().unwrap() = complete.then_some(names);
    }

    pub(crate) fn file_tool_is_trusted(&self, name: &str) -> bool {
        self.file_tools
            .lock()
            .map(|tools| tools.contains(name))
            .unwrap_or(false)
    }

    pub(crate) fn observer(self: &Arc<Self>) -> Arc<dyn AgentMiddleware> {
        Arc::new(ControlObserver {
            controls: self.clone(),
        })
    }

    pub(crate) fn prepare(self: &Arc<Self>) -> Arc<dyn AgentMiddleware> {
        Arc::new(ControlPrepare {
            controls: self.clone(),
        })
    }

    pub(crate) fn confirmed_pause(&self) -> Option<PauseReceipt> {
        if self.unavailable.load(Ordering::SeqCst) {
            return None;
        }
        self.governor
            .as_ref()
            .and_then(FailureGovernor::confirmed_pause)
    }

    pub(crate) fn reset(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.unavailable.store(true, Ordering::SeqCst);
        if let Some(governor) = &self.governor {
            governor.reset_for_context_change();
        }
        *self.input.lock().map_err(|_| unavailable())? = None;
        *self.candidate.lock().map_err(|_| unavailable())? = None;
        self.runtime_changed.store(false, Ordering::SeqCst);
        self.context.rebind(state)?;
        let governor_stamp = if self.governor.is_some() {
            stored_input(state)?
        } else {
            None
        };
        let stamp = if self.context.options().enabled {
            self.context.input_stamp(state)?
        } else if self.governor.is_some() {
            governor_stamp
        } else {
            None
        };
        if self.governor.is_some() && stamp != governor_stamp {
            return Err(unavailable());
        }
        *self.input.lock().map_err(|_| unavailable())? = stamp;
        self.unavailable.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub(crate) fn clear(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.context.clear(state)?;
        if self.governor.is_some() {
            state.custom.remove(INPUT_KEY);
        }
        if let Some(governor) = &self.governor {
            governor.reset_turn();
        }
        *self.input.lock().map_err(|_| unavailable())? = None;
        *self.candidate.lock().map_err(|_| unavailable())? = None;
        self.unavailable.store(false, Ordering::SeqCst);
        self.runtime_changed.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub(crate) fn compacted(&self, observation: CompactionObservation) {
        if observation == CompactionObservation::Changed {
            if let Some(governor) = &self.governor {
                governor.reset_for_context_change();
            }
            if let Ok(mut candidate) = self.candidate.lock() {
                *candidate = None;
            }
        }
    }

    fn restore_governor_input(&self) -> Result<(), AgentError> {
        if let Some(governor) = &self.governor {
            let stamp = self
                .input
                .lock()
                .map_err(|_| unavailable())?
                .ok_or_else(unavailable)?;
            if !governor.observe_input(stamp.revision, stamp.digest) {
                return Err(unavailable());
            }
        }
        Ok(())
    }
}

impl AgentBoundaryPolicy for AppControls {
    fn user_input(
        &self,
        state: &mut AgentState,
        message_index: usize,
        kind: HumanInputKind,
    ) -> Result<(), AgentError> {
        if kind == HumanInputKind::Initial {
            // A genuinely new turn cannot inherit a prior completed pause,
            // including when this input subsequently fails bounded validation.
            if let Some(governor) = &self.governor {
                governor.reset_turn();
            }
            *self.candidate.lock().map_err(|_| unavailable())? = None;
            *self.input.lock().map_err(|_| unavailable())? = None;
            self.context.reset_report()?;
        }
        let Some(Msg::User { content }) = state.messages.get(message_index) else {
            return Err(unavailable());
        };
        if content.len() > MAX_OBSERVED_INPUT_BYTES {
            return Err(unavailable());
        }
        let input = content.clone(); // Bounded original input, not a transcript scan.
        let previous = if self.governor.is_some() {
            stored_input(state)?
        } else {
            None
        };
        let stamp = if self.context.options().enabled {
            self.context.record_input(state, &input)?
        } else if self.governor.is_some() {
            let revision = previous
                .map_or(0, |stamp| stamp.revision)
                .checked_add(1)
                .ok_or_else(unavailable)?;
            InputStamp::for_input(revision, &input)?
        } else {
            self.context.record_input(state, &input)?
        };
        if self.governor.is_some() {
            state
                .custom
                .insert(INPUT_KEY.into(), json!({"version":1,"stamp":stamp}));
        }
        *self.input.lock().map_err(|_| unavailable())? = Some(stamp);
        *self.candidate.lock().map_err(|_| unavailable())? = None;
        self.restore_governor_input()
    }

    fn before_model(&self, state: &AgentState) -> Result<ModelBoundaryAction, AgentError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(unavailable());
        }
        self.context.validate_state(state)?;
        if self.governor.is_some() {
            let actual = *self.input.lock().map_err(|_| unavailable())?;
            let stored = stored_input(state)?;
            if actual.is_none()
                || stored != actual
                || self.context.options().enabled && self.context.input_stamp(state)? != actual
            {
                if let Ok(mut candidate) = self.candidate.lock() {
                    *candidate = None;
                }
                self.unavailable.store(true, Ordering::SeqCst);
                return Err(unavailable());
            }
        }
        self.restore_governor_input()?;
        let Some(governor) = &self.governor else {
            return Ok(ModelBoundaryAction::Continue);
        };
        match governor.decision() {
            BudgetDecision::Continue => {
                *self.candidate.lock().map_err(|_| unavailable())? = None;
                Ok(ModelBoundaryAction::Continue)
            }
            BudgetDecision::Pause(candidate) => {
                let failures = candidate.failures();
                *self.candidate.lock().map_err(|_| unavailable())? = Some(candidate);
                Ok(ModelBoundaryAction::Stop {
                    reason: format!(
                        "{PAUSED} At least {failures} identical failures were observed."
                    ),
                })
            }
            BudgetDecision::Unavailable => Err(unavailable()),
        }
    }

    fn runtime_input(&self) {
        self.runtime_changed.store(true, Ordering::SeqCst);
        if let Some(governor) = &self.governor {
            governor.observed_progress();
        }
        if let Ok(mut candidate) = self.candidate.lock() {
            *candidate = None;
        }
    }

    fn idle_compaction_committed(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.context
            .observe_compaction(state, CompactionObservation::Changed)?;
        self.compacted(CompactionObservation::Changed);
        Ok(())
    }

    fn stop_committed(&self, _: &AgentState) {
        if let Some(governor) = &self.governor {
            if let Ok(mut candidate) = self.candidate.lock() {
                if let Some(candidate) = candidate.take() {
                    governor.confirm_pause(&candidate);
                }
            }
        }
    }
}

struct ControlPrepare {
    controls: Arc<AppControls>,
}
#[async_trait]
impl AgentMiddleware for ControlPrepare {
    fn name(&self) -> &str {
        "local_control_prepare"
    }
    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        if self.controls.runtime_changed.swap(false, Ordering::SeqCst) {
            self.controls.context.observe_runtime_activity(state)?;
        }
        if self
            .controls
            .governor
            .as_ref()
            .is_some_and(FailureGovernor::advisory_repeated)
        {
            state.system_extras.push(ADVISORY.into());
        }
        Ok(())
    }
}

struct ControlObserver {
    controls: Arc<AppControls>,
}
#[async_trait]
impl AgentMiddleware for ControlObserver {
    fn name(&self) -> &str {
        "failure_observations"
    }
    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let observed = bounded_operation(&call);
        let result = next.run(call, ctx).await;
        if let Some(governor) = &self.controls.governor {
            match &result {
                Ok(outcome) if observed.is_some() => {
                    let observed = observed.as_ref().unwrap();
                    let class = if self
                        .controls
                        .observational
                        .lock()
                        .map(|tools| tools.contains(&observed.name))
                        .unwrap_or(false)
                    {
                        TrustedClass::Observational
                    } else if self
                        .controls
                        .registered
                        .lock()
                        .ok()
                        .and_then(|names| {
                            names.as_ref().map(|names| !names.contains(&observed.name))
                        })
                        .unwrap_or(false)
                    {
                        TrustedClass::PreExecutionFailure
                    } else {
                        TrustedClass::Unknown
                    };
                    governor.observe_actual(observed, outcome, class);
                }
                Ok(_) => governor.observed_progress(),
                Err(_) => governor.observed_progress(),
            }
        }
        result
    }
}

fn unavailable() -> AgentError {
    AgentError::State(UNAVAILABLE.into())
}

fn stored_input(state: &AgentState) -> Result<Option<InputStamp>, AgentError> {
    let Some(value) = state.custom.get(INPUT_KEY) else {
        return Ok(None);
    };
    let object = value.as_object().ok_or_else(unavailable)?;
    if object.len() != 2 || object.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(unavailable());
    }
    let stamp = object
        .get("stamp")
        .and_then(Value::as_object)
        .ok_or_else(unavailable)?;
    if stamp.len() != 2 || !stamp.contains_key("revision") || !stamp.contains_key("digest") {
        return Err(unavailable());
    }
    let revision = stamp["revision"]
        .as_u64()
        .filter(|revision| *revision > 0)
        .ok_or_else(unavailable)?;
    let bytes = stamp["digest"]
        .as_array()
        .filter(|bytes| bytes.len() == 32)
        .ok_or_else(unavailable)?;
    let mut digest = [0u8; 32];
    for (index, value) in bytes.iter().enumerate() {
        digest[index] = value
            .as_u64()
            .and_then(|byte| u8::try_from(byte).ok())
            .ok_or_else(unavailable)?;
    }
    Ok(Some(InputStamp { revision, digest }))
}
