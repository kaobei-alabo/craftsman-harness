//! Bounded observations of exact returned operation failures, without control IO.
use super::Settings;
use craftsman_harness::{ToolCall, ToolErrorKind, ToolOutcome};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

const MAX_NAME_BYTES: usize = 512;
const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_ARGUMENT_DEPTH: usize = 32;
const MAX_ARGUMENT_NODES: usize = 1024;

/// Classification supplied by the trusted actual invocation owner. This is not
/// a field from tool arguments or an inference from a model-supplied tool name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustedClass {
    Observational,
    PreExecutionFailure,
    Unknown,
}

impl TrustedClass {
    fn eligible(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

struct Identity;

/// A proposed local stop. Only the actual session owner may confirm it after
/// closing admission, retaining input/results and completing end hooks/save.
#[derive(Clone)]
pub(crate) struct PauseCandidate {
    issuer: Arc<Identity>,
    epoch: u64,
    signature: [u8; 32],
    failures: u8,
}

impl PauseCandidate {
    pub(crate) fn failures(&self) -> u8 {
        self.failures
    }
}

/// Observation of a successfully executed local pause, not an execution grant.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct PauseReceipt {
    failures: u8,
}

impl PauseReceipt {
    pub(crate) fn failures(&self) -> u8 {
        self.failures
    }
}

pub(crate) enum BudgetDecision {
    Continue,
    Pause(PauseCandidate),
    /// The selected input/observation identity is unavailable. The actual
    /// owner reports fixed text; this must not become a silent budget downgrade.
    Unavailable,
}

#[derive(Default)]
struct State {
    input: Option<(u64, [u8; 32])>,
    input_unavailable: bool,
    signature: Option<[u8; 32]>,
    consecutive: u8,
    strict_consecutive: u8,
    epoch: u64,
    unavailable: bool,
    confirmed: Option<PauseReceipt>,
}

impl State {
    fn advance(&mut self) -> bool {
        let Some(next) = self.epoch.checked_add(1) else {
            self.unavailable = true;
            self.signature = None;
            self.consecutive = 0;
            self.strict_consecutive = 0;
            self.confirmed = None;
            return false;
        };
        self.epoch = next;
        true
    }

    fn clear_pressure(&mut self) {
        self.advance();
        self.signature = None;
        self.consecutive = 0;
        self.strict_consecutive = 0;
        self.confirmed = None;
    }
}

pub(crate) struct FailureGovernor {
    limit: u8,
    identity: Arc<Identity>,
    state: Mutex<State>,
}

impl FailureGovernor {
    pub(crate) fn new(settings: &Settings) -> Self {
        Self {
            limit: settings.repeat_limit(),
            identity: Arc::new(Identity),
            state: Mutex::new(State::default()),
        }
    }

    /// The same fields as the context owner's actual human-input stamp. No
    /// model message or temporary request view is accepted as input authority.
    /// Identical text with a new accepted revision is still new direction.
    pub(crate) fn observe_input(&self, revision: u64, digest: [u8; 32]) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.unavailable || state.input_unavailable {
            return false;
        }
        if revision == 0 {
            state.clear_pressure();
            state.input_unavailable = true;
            return false;
        }
        if let Some((old_revision, old_digest)) = state.input {
            if revision < old_revision {
                return false;
            }
            if revision == old_revision {
                if digest == old_digest {
                    return true;
                }
                state.clear_pressure();
                state.input_unavailable = true;
                return false;
            }
        }
        state.clear_pressure();
        state.input = Some((revision, digest));
        !state.unavailable
    }

    /// Start an actual new turn. Epochs never wrap or recycle within this owner.
    pub(crate) fn reset_turn(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.clear_pressure();
            state.input = None;
            state.input_unavailable = false;
        }
    }

    /// Explicit compaction, restore or conversation/model-transfer boundary.
    /// The caller supplies the resulting actual input stamp before more work.
    pub(crate) fn reset_for_context_change(&self) {
        self.reset_turn();
    }

    /// Observe an actual admitted result without changing it or its metadata.
    /// Call this only after the actual tool returned, inside the approval gate.
    /// Unknown/effectful errors remain advisory; only the owner-supplied trusted
    /// class can contribute to a strict consecutive-operation budget.
    pub(crate) fn observe_actual(
        &self,
        call: &ToolCall,
        outcome: &ToolOutcome,
        class: TrustedClass,
    ) {
        let reset = !outcome.is_error
            || matches!(
                outcome.meta.error_kind,
                Some(
                    ToolErrorKind::NeedsApproval
                        | ToolErrorKind::DeniedCapability
                        | ToolErrorKind::Cancelled
                )
            );
        let signature = if reset {
            None
        } else {
            failure_signature(call, outcome)
        };
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.unavailable {
            return;
        }
        let Some(signature) = signature else {
            state.clear_pressure();
            return;
        };
        if !state.advance() {
            return;
        }
        state.confirmed = None;
        if state.signature == Some(signature) {
            state.consecutive = state.consecutive.saturating_add(1).min(self.limit);
            state.strict_consecutive = if class.eligible() {
                state.strict_consecutive.saturating_add(1).min(self.limit)
            } else {
                0
            };
        } else {
            state.signature = Some(signature);
            state.consecutive = 1;
            state.strict_consecutive = u8::from(class.eligible());
        }
    }

    /// Known effects/new evidence reset pressure without asserting coverage of
    /// an ambient workspace or unobserved external operations.
    pub(crate) fn observed_progress(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.clear_pressure();
        }
    }

    pub(crate) fn advisory_repeated(&self) -> bool {
        self.state
            .lock()
            .map(|state| !state.unavailable && state.consecutive >= 2)
            .unwrap_or(false)
    }

    /// Inspect only after the current committed tool group and actual input
    /// are retained. This does not stop sampling, dispatch or native work.
    pub(crate) fn decision(&self) -> BudgetDecision {
        let Ok(state) = self.state.lock() else {
            return BudgetDecision::Unavailable;
        };
        if state.unavailable || state.input_unavailable || state.input.is_none() {
            return BudgetDecision::Unavailable;
        }
        if state.confirmed.is_some() || state.strict_consecutive < self.limit {
            return BudgetDecision::Continue;
        }
        match state.signature {
            Some(signature) => BudgetDecision::Pause(PauseCandidate {
                issuer: self.identity.clone(),
                epoch: state.epoch,
                signature,
                failures: state.strict_consecutive,
            }),
            None => BudgetDecision::Continue,
        }
    }

    /// Called by the actual owner only after normal end hooks/final save
    /// succeed. Stale, foreign or duplicate candidates cannot create a receipt.
    pub(crate) fn confirm_pause(&self, candidate: &PauseCandidate) -> Option<PauseReceipt> {
        if !Arc::ptr_eq(&self.identity, &candidate.issuer) {
            return None;
        }
        let mut state = self.state.lock().ok()?;
        if state.unavailable
            || state.input_unavailable
            || state.input.is_none()
            || state.confirmed.is_some()
            || state.epoch != candidate.epoch
            || state.signature != Some(candidate.signature)
            || state.strict_consecutive != candidate.failures
            || state.strict_consecutive < self.limit
        {
            return None;
        }
        let receipt = PauseReceipt {
            failures: candidate.failures,
        };
        state.confirmed = Some(receipt);
        Some(receipt)
    }

    pub(crate) fn confirmed_pause(&self) -> Option<PauseReceipt> {
        self.state.lock().ok()?.confirmed
    }

    #[cfg(test)]
    pub(super) fn exhaust_epoch(&self) {
        self.state.lock().unwrap().epoch = u64::MAX;
    }
}

fn failure_signature(call: &ToolCall, outcome: &ToolOutcome) -> Option<[u8; 32]> {
    if call.name.len() > MAX_NAME_BYTES || outcome.content.len() > MAX_ERROR_BYTES {
        return None;
    }
    let arguments = argument_digest(&call.args)?;
    let mut digest = Sha256::new();
    digest.update(b"craftsman-exact-returned-failure-v1");
    digest.update((call.name.len() as u64).to_le_bytes());
    digest.update(call.name.as_bytes());
    digest.update(arguments);
    digest.update((outcome.content.len() as u64).to_le_bytes());
    digest.update(outcome.content.as_bytes());
    Some(digest.finalize().into())
}

/// Retain only a validated, bounded original operation for post-return
/// observation. Caller-supplied call ids are neither retained nor authority.
pub(crate) fn bounded_operation(call: &ToolCall) -> Option<ToolCall> {
    if call.name.len() > MAX_NAME_BYTES || argument_digest(&call.args).is_none() {
        return None;
    }
    Some(ToolCall {
        id: String::new(),
        name: call.name.clone(),
        args: call.args.clone(),
    })
}

struct DigestWriter {
    digest: Sha256,
    bytes: usize,
}

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_ARGUMENT_BYTES.saturating_sub(self.bytes) {
            return Err(io::Error::other("Argument observation exceeds its limit."));
        }
        self.digest.update(bytes);
        self.bytes += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn argument_digest(value: &Value) -> Option<[u8; 32]> {
    let mut writer = DigestWriter {
        digest: Sha256::new(),
        bytes: 0,
    };
    let mut nodes = 0;
    canonical(value, &mut writer, 0, &mut nodes).ok()?;
    Some(writer.digest.finalize().into())
}

fn canonical(
    value: &Value,
    writer: &mut DigestWriter,
    depth: usize,
    nodes: &mut usize,
) -> io::Result<()> {
    *nodes += 1;
    if depth > MAX_ARGUMENT_DEPTH || *nodes > MAX_ARGUMENT_NODES {
        return Err(io::Error::other("Argument observation exceeds its limit."));
    }
    match value {
        Value::String(text) => {
            if text.len() > MAX_ARGUMENT_BYTES {
                return Err(io::Error::other("Argument observation exceeds its limit."));
            }
            serde_json::to_writer(writer, text).map_err(io::Error::other)
        }
        Value::Array(values) => {
            if values.len() > MAX_ARGUMENT_NODES.saturating_sub(*nodes) {
                return Err(io::Error::other("Argument observation exceeds its limit."));
            }
            writer.write_all(b"[")?;
            for (index, item) in values.iter().enumerate() {
                if index != 0 {
                    writer.write_all(b",")?;
                }
                canonical(item, writer, depth + 1, nodes)?;
            }
            writer.write_all(b"]")
        }
        Value::Object(values) => {
            if values.len() > MAX_ARGUMENT_NODES.saturating_sub(*nodes) {
                return Err(io::Error::other("Argument observation exceeds its limit."));
            }
            let mut key_bytes = 0usize;
            for key in values.keys() {
                if key.len() > MAX_ARGUMENT_BYTES.saturating_sub(key_bytes) {
                    return Err(io::Error::other("Argument observation exceeds its limit."));
                }
                key_bytes += key.len();
            }
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort_unstable();
            writer.write_all(b"{")?;
            for (index, key) in keys.into_iter().enumerate() {
                if key.len() > MAX_ARGUMENT_BYTES {
                    return Err(io::Error::other("Argument observation exceeds its limit."));
                }
                if index != 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, key).map_err(io::Error::other)?;
                writer.write_all(b":")?;
                canonical(values.get(key.as_str()).unwrap(), writer, depth + 1, nodes)?;
            }
            writer.write_all(b"}")
        }
        value => serde_json::to_writer(writer, value).map_err(io::Error::other),
    }
}
