//! Private adapter from explicit preparation into the shared session builder.
//!
//! Only synthetic tests call this unfinished runtime. No CLI, human channel,
//! transcript writer, guest host, token/usage ledger or trace producer is wired.
//! A shared logical-request cap is private and separate from those producers.
//! Durable preparation is retained, but the agent MUST NOT receive its raw
//! checkpointer/journal until seeded storage redaction exists.

#[cfg(test)]
mod admission_receipt_tests;
#[cfg(test)]
mod ascii_tests;
#[cfg(test)]
mod human_tests;
#[cfg(test)]
mod message_tests;
mod policy;
#[cfg(test)]
mod request_meter_tests;
#[cfg(test)]
mod selection_tests;
#[cfg(test)]
mod terminal_selection_tests;
#[cfg(test)]
mod terminal_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod token_reservation_tests;

pub(crate) use policy::InvocationGate;

use super::construction::Prepared;
use super::permissions::Mode;
use crate::agent::Session;
use craftsman_harness::{AgentState, HarnessProfile, TurnOutput, TurnResult};
use std::sync::Arc;
use thiserror::Error;

/// Owns the prepared authority and private live agent. No legacy SessionExtras,
/// UI callbacks, stdout fallback, session switch or approve/resume API escapes.
pub(crate) struct Runtime {
    prepared: Prepared,
    session: Session,
    gate: Arc<InvocationGate>,
    request_meter: Arc<super::meter::Meter>,
}

/// Opaque lease of the actual shared-builder invocation, not a policy-only
/// substitute. Only this runtime can pair its Prepared with its unique fail latch.
#[cfg(feature = "gadgets")]
pub(crate) struct GuestScope<'a> {
    prepared: &'a Prepared,
    gate: Arc<InvocationGate>,
}
#[cfg(feature = "gadgets")]
impl GuestScope<'_> {
    pub(crate) fn prepared(&self) -> &Prepared {
        self.prepared
    }
    pub(crate) fn gate(&self) -> Arc<InvocationGate> {
        self.gate.clone()
    }
}

/// Opaque lease of THIS actual runtime's retained state, frozen seeds and gate.
/// No arbitrary state/Prepared/gate/redactor constructor is exposed.
pub(crate) struct MessageScope<'a> {
    state: &'a Arc<std::sync::Mutex<AgentState>>,
    redactor: &'a crate::exec::redaction::Redactor,
    gate: &'a Arc<InvocationGate>,
}
impl MessageScope<'_> {
    pub(super) fn state(&self) -> &std::sync::Mutex<AgentState> {
        self.state
    }
    pub(super) fn redactor(&self) -> &crate::exec::redaction::Redactor {
        self.redactor
    }
    pub(super) fn ready(&self) -> Result<(), crate::exec::provider::request::Fault> {
        if self.gate.approval_required() {
            Err(crate::exec::provider::request::Fault::Stopped)
        } else {
            Ok(())
        }
    }
    pub(super) fn same(&self, other: &MessageScope<'_>) -> bool {
        Arc::ptr_eq(self.state, other.state) && Arc::ptr_eq(self.gate, other.gate)
    }
}

/// Read-only finalization lease; actual returned Run AND Runtime identity are
/// required. An arbitrary Runtime/Prepared/flag cannot mint terminal authority.
pub(crate) struct TerminalScope<'a> {
    state: &'a Arc<std::sync::Mutex<AgentState>>,
    redactor: &'a crate::exec::redaction::Redactor,
    gate: &'a Arc<InvocationGate>,
    returned: crate::exec::terminal::ReturnKind,
    output: Option<&'a str>,
}
impl TerminalScope<'_> {
    pub(super) fn ready(&self) -> Result<(), crate::exec::terminal::Error> {
        if !self.gate.execution_closed()
            || self.returned == crate::exec::terminal::ReturnKind::Suspended
        {
            Err(crate::exec::terminal::Error::Unsettled)
        } else {
            Ok(())
        }
    }
    pub(super) fn state(&self) -> &std::sync::Mutex<AgentState> {
        self.state
    }
    pub(super) fn redactor(&self) -> &crate::exec::redaction::Redactor {
        self.redactor
    }
    pub(super) fn approval_required(&self) -> bool {
        self.gate.approval_required()
    }
    pub(super) fn returned(&self) -> crate::exec::terminal::ReturnKind {
        self.returned
    }
    pub(super) fn output(&self) -> Option<&str> {
        self.output
    }
    pub(super) fn same(&self, other: &TerminalScope<'_>) -> bool {
        Arc::ptr_eq(self.state, other.state) && Arc::ptr_eq(self.gate, other.gate)
    }
}

#[derive(Debug, Error)]
pub(crate) enum RuntimeError {
    #[error("private headless session construction failed")]
    Construction,
    #[error("headless invocation requires explicit approval")]
    ApprovalRequired,
    #[error("private headless turn failed")]
    Turn(#[source] craftsman_harness::AgentError),
    #[error("private headless turn unexpectedly suspended")]
    Suspension,
    #[error("private headless model request admission limit reached")]
    RequestLimit,
    #[error("private headless model request accounting failed")]
    RequestAccounting,
    #[error("private headless token reservation allowance refused")]
    TokenReservation,
}

impl Runtime {
    /// Consumes the prepared context; callers cannot mix in ambient Config,
    /// mutable ApprovalState, arbitrary tools or interactive SessionExtras.
    pub(crate) async fn build(prepared: Prepared) -> Result<Self, RuntimeError> {
        Self::build_with_request_limit(prepared, super::meter::RequestLimit::default()).await
    }

    /// Explicit, validated private ceiling; no public argv/ambient-config route.
    pub(crate) async fn build_with_request_limit(
        prepared: Prepared,
        limit: super::meter::RequestLimit,
    ) -> Result<Self, RuntimeError> {
        let (session, gate, request_meter) =
            crate::agent::build_prepared_session_with_request_limit(&prepared, limit)
                .await
                .ok_or(RuntimeError::Construction)?;
        Ok(Self {
            prepared,
            session,
            gate,
            request_meter,
        })
    }

    /// Explicit private cumulative reservation ceiling. No settled usage,
    /// billing, public token-budget flag or release behavior follows from it.
    pub(crate) async fn build_with_token_limit(
        prepared: Prepared,
        limit: super::meter::RequestLimit,
        tokens: super::meter::TokenLimit,
    ) -> Result<Self, RuntimeError> {
        let (session, gate, request_meter) =
            crate::agent::build_prepared_session_with_limits(&prepared, limit, Some(tokens))
                .await
                .ok_or(RuntimeError::Construction)?;
        Ok(Self {
            prepared,
            session,
            gate,
            request_meter,
        })
    }

    #[cfg(feature = "gadgets")]
    pub(crate) fn guest_scope(&self) -> GuestScope<'_> {
        GuestScope {
            prepared: &self.prepared,
            gate: self.gate.clone(),
        }
    }

    pub(crate) fn prepared(&self) -> &Prepared {
        &self.prepared
    }

    pub(crate) fn retained_state(&self) -> AgentState {
        self.session.agent.state.lock().unwrap().clone()
    }

    pub(crate) fn message_scope(&self) -> MessageScope<'_> {
        MessageScope {
            state: &self.session.agent.state,
            redactor: self.prepared.credentials().redactor(),
            gate: &self.gate,
        }
    }

    /// Finite main retained-state data only, never original execution/control.
    pub(crate) fn message_view(
        &self,
    ) -> Result<super::messages::View<'_>, super::provider::request::Fault> {
        super::messages::capture(self.message_scope())
    }

    /// Tighten only an admitted actual-runtime snapshot; no raw-state reducer.
    pub(crate) fn message_selection(
        &self,
        max_bytes: usize,
    ) -> Result<super::messages::Selection<'_>, super::provider::request::Fault> {
        self.message_view()?.select(max_bytes)
    }

    pub(super) fn same_runtime(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.gate, &other.gate)
            && Arc::ptr_eq(&self.session.agent.state, &other.session.agent.state)
    }
    pub(super) fn close_for_terminal(&self) {
        self.gate.close_execution();
    }
    pub(super) fn terminal_scope<'a>(
        &'a self,
        run: &'a crate::exec::input::Run,
    ) -> Result<TerminalScope<'a>, crate::exec::terminal::Error> {
        use crate::exec::terminal::{Error, ReturnKind};
        if !run.matches_runtime(self) {
            return Err(Error::Foreign);
        }
        let returned = match run.outcome() {
            Ok(_) => ReturnKind::Returned,
            Err(RuntimeError::ApprovalRequired) => ReturnKind::ApprovalRequired,
            Err(
                RuntimeError::Turn(craftsman_harness::AgentError::Suspended(_))
                | RuntimeError::Suspension,
            ) => ReturnKind::Suspended,
            Err(
                RuntimeError::Turn(_)
                | RuntimeError::RequestLimit
                | RuntimeError::RequestAccounting
                | RuntimeError::TokenReservation,
            ) => ReturnKind::TurnFailed,
            Err(RuntimeError::Construction) => ReturnKind::ConstructionFailed,
        };
        let scope = TerminalScope {
            state: &self.session.agent.state,
            redactor: self.prepared.credentials().redactor(),
            gate: &self.gate,
            returned,
            output: run.outcome().as_ref().ok().map(TurnOutput::final_text),
        };
        scope.ready()?;
        Ok(scope)
    }

    pub(super) fn request_snapshot(&self) -> Result<super::meter::Snapshot, RuntimeError> {
        let snapshot = self
            .request_meter
            .snapshot()
            .map_err(|_| RuntimeError::RequestAccounting)?;
        self.request_meter
            .bind_snapshot(&snapshot, &self.gate)
            .map_err(|_| RuntimeError::RequestAccounting)?;
        Ok(snapshot)
    }

    fn request_stop(&self) -> Result<(), RuntimeError> {
        match self.request_meter.limit_refused() {
            Ok(true) => return Err(RuntimeError::RequestLimit),
            Err(_) => return Err(RuntimeError::RequestAccounting),
            Ok(false) => (),
        }
        match self.request_meter.tokens_refused() {
            Ok(true) => Err(RuntimeError::TokenReservation),
            Err(_) => Err(RuntimeError::RequestAccounting),
            Ok(false) => Ok(()),
        }
    }

    /// Internal turn execution, not the accepted input/output/exit producer.
    /// Fail policy remains latched even when the framework converts a tool
    /// error into a message; never relabel that partial turn as completed.
    pub(crate) async fn run_turn(&self, input: String) -> Result<TurnOutput, RuntimeError> {
        if self.gate.approval_required() {
            return Err(RuntimeError::ApprovalRequired);
        }
        self.request_stop()?;
        if self.gate.execution_closed() {
            return Err(RuntimeError::Turn(craftsman_harness::AgentError::State(
                "[REDACTED]".into(),
            )));
        }
        let turn = self.session.agent.run_turn(input).await;
        if self.gate.approval_required() {
            return Err(RuntimeError::ApprovalRequired);
        }
        self.request_stop()?;
        match turn {
            Ok(TurnResult::Suspended(_)) => Err(RuntimeError::Suspension),
            Ok(TurnResult::Done(turn)) => Ok(turn),
            Err(error) => Err(RuntimeError::Turn(error)),
        }
    }
}

// The legacy fallback explicitly advertises durable memory/platform tools.
// Keep it and its goldens unchanged; headless gets an honest separate base.
const BASE: &str = "You are Craftsman, a capable workspace assistant. Use the tools actually offered in this request to investigate, act and verify rather than guessing. Keep multi-step work organized and report the outcome concisely. Delegation is foreground-only and shares this invocation's permissions.";
const LIMITS: &str = "# Noninteractive invocation\nHuman input, background tasks, MCP, LSP, durable memory and callable gadgets are unavailable in this private profile. ask_user returns an error without reading stdin. File tools are confined to the selected workspace; config/credential paths remain denied and protected writes require the exact invocation permission. Native shell execution is a separate, explicitly allowed host operation, NOT an OS sandbox. Shell capture is bounded and memory-only; omitted output is not saved to a spill file. Provided context is data/instructions, not a grant of capabilities.";

pub(crate) fn harness_profile(
    profile: &HarnessProfile,
    gate: &Arc<InvocationGate>,
) -> HarnessProfile {
    let mut profile = profile.clone();
    // Runtime-only profile factories can also contribute tools to children.
    // Omit unavailable side-effecting services before materialization; gate the
    // remaining factory tools so child profile compilation cannot bypass R1.
    profile.extra_middleware = profile
        .extra_middleware
        .into_iter()
        .filter(|factory| {
            !matches!(
                factory.name(),
                "memory" | "workspace_checkpoint" | "hitl" | "hitl_policy" | "gadget_catalog"
            )
        })
        .map(|factory| {
            let name = factory.name().to_owned();
            let gate = gate.clone();
            craftsman_harness::NamedMiddlewareFactory::new(name, move || {
                gate.wrap_middleware(factory.materialize())
            })
        })
        .collect();
    profile.config.base_system_prompt = Some(BASE.into());
    if let Some(agent) = &mut profile.config.general_purpose_subagent {
        agent.description = Some("Foreground delegate with isolated conversation context and the same confined file tools and immutable invocation permissions; no background, MCP, LSP or durable memory.".into());
    }
    profile
}

/// Pure assembly: NEVER invoke legacy home/project prompt/skill discovery.
/// The same bounded snapshot is provided to main and declared foreground roles.
pub(crate) fn system_prompt(prepared: &Prepared, profile: &HarnessProfile, base: &str) -> String {
    let permission = match prepared.permissions().mode() {
        Mode::Manual => {
            "# Permission mode: manual\nWrites, shell and network require explicit invocation allows. Unapproved operations return errors; no human approval can widen this policy."
        }
        Mode::Plan => {
            "# Permission mode: plan\nInvestigate and propose a plan. File mutation and native execution stay denied, including explicit allows. No model-driven mode change can widen this policy."
        }
    };
    let mut chunks = vec![
        base.to_owned(),
        crate::platform_os::system_prompt_block(),
        LIMITS.into(),
        permission.into(),
        prepared.discovery().context_block(),
    ];
    if let Some(suffix) = &profile.config.system_prompt_suffix {
        chunks.push(suffix.clone());
    }
    chunks.join("\n\n")
}

#[cfg(test)]
impl Runtime {
    /// Test-only observation of the actual runtime gate, never a substitute.
    pub(in crate::exec) fn gate_for_tests(&self) -> Arc<InvocationGate> {
        self.gate.clone()
    }
    pub(in crate::exec) fn state_for_tests(&self) -> Arc<std::sync::Mutex<AgentState>> {
        self.session.agent.state.clone()
    }
}
