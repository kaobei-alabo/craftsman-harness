//! Explicitly selected, bounded task context. Original messages and effects stay authoritative.

mod state;
mod view;

use async_trait::async_trait;
use craftsman_harness::builtin::CompactionMiddleware;
use craftsman_harness::{
    AgentError, AgentMiddleware, AgentState, ModelRequest, ModelResponse, Next, ToolCall, ToolCtx,
    ToolNext, ToolOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub(crate) use view::{CompiledContext, CompiledCounts, RequestShape};

const CONFIG_ERROR: &str =
    "Invalid context settings. Use a boolean enabled value and max_bytes from 2048 to 32768.";
const STATE_ERROR: &str = "Context compilation state is unavailable; further sampling was stopped.";
/// Maximum original human input observed or hashed by this app adapter. The
/// framework retains the full input independently when this observation refuses.
pub(crate) const MAX_OBSERVED_INPUT_BYTES: usize = 1024 * 1024;
const MAX_ANCHOR_MESSAGES: usize = 1024;
const MAX_ANCHOR_BYTES: usize = 1024 * 1024;
const MAX_ANCHOR_NODES: usize = 8192;
const MAX_ANCHOR_DEPTH: usize = 16;
const MAX_ANCHOR_FIELDS: usize = 128;
const MAX_ANCHOR_KEY_BYTES: usize = 512;
const MAX_SCOPE_ENCODED_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextOptions {
    pub enabled: bool,
    pub max_bytes: usize,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            max_bytes: 12 * 1024,
        }
    }
}

impl ContextOptions {
    /// Called only after the app frontend explicitly selects context support.
    pub fn parse(value: Option<&Value>) -> Result<Self, AgentError> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let object = value.as_object().ok_or_else(config_error)?;
        if object.len() > 2
            || object
                .keys()
                .any(|key| !matches!(key.as_str(), "enabled" | "max_bytes"))
        {
            return Err(config_error());
        }
        let enabled = match object.get("enabled") {
            None => false,
            Some(Value::Bool(value)) => *value,
            _ => return Err(config_error()),
        };
        let max_bytes = match object.get("max_bytes") {
            None => Self::default().max_bytes,
            Some(value) => usize::try_from(value.as_u64().ok_or_else(config_error)?)
                .map_err(|_| config_error())?,
        };
        if !(2048..=32768).contains(&max_bytes) {
            return Err(config_error());
        }
        Ok(Self { enabled, max_bytes })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputStamp {
    pub revision: u64,
    pub digest: [u8; 32],
}

impl InputStamp {
    /// The caller supplies a real committed-input revision, never a model-derived guess.
    pub fn for_input(revision: u64, input: &str) -> Result<Self, AgentError> {
        if revision == 0 || input.len() > MAX_OBSERVED_INPUT_BYTES {
            return Err(context_error());
        }
        Ok(Self {
            revision,
            digest: Sha256::digest(input.as_bytes()).into(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompactionObservation {
    Unchanged,
    Changed,
    Unknown,
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptAnchor {
    messages: usize,
    digest: Option<[u8; 32]>,
}

impl TranscriptAnchor {
    pub fn capture(state: &AgentState) -> Self {
        let mut writer = AnchorWriter {
            bytes: 0,
            digest: Sha256::new(),
        };
        let digest = if anchor_input_fits(&state.messages)
            && serde_json::to_writer(&mut writer, &state.messages).is_ok()
        {
            Some(writer.digest.finalize().into())
        } else {
            None
        };
        Self {
            messages: state.messages.len(),
            digest,
        }
    }

    fn compare(&self, after: &Self) -> CompactionObservation {
        if self.messages != after.messages {
            return CompactionObservation::Changed;
        }
        match (&self.digest, &after.digest) {
            (Some(before), Some(after)) if before == after => CompactionObservation::Unchanged,
            (Some(_), Some(_)) => CompactionObservation::Changed,
            _ => CompactionObservation::Unknown,
        }
    }
}

struct AnchorWriter {
    bytes: usize,
    digest: Sha256,
}
impl Write for AnchorWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self
            .bytes
            .checked_add(bytes.len())
            .filter(|count| *count <= MAX_ANCHOR_BYTES)
            .ok_or_else(|| std::io::Error::other("context anchor limit"))?;
        self.digest.update(bytes);
        self.bytes = count;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct RuntimeState {
    passive_input_sequence: u64,
    last_request: Option<RequestShape>,
    failed: bool,
}

pub(crate) struct ContextController {
    options: ContextOptions,
    scope: String,
    runtime: Mutex<RuntimeState>,
}

impl ContextController {
    pub fn new(options: ContextOptions, workspace: &Path) -> Result<Self, AgentError> {
        if !(2048..=32768).contains(&options.max_bytes) || !workspace.is_absolute() {
            return Err(config_error());
        }
        // Windows UTF-16 hashing uses at most twice the encoded byte length;
        // this bounds scope hashing before conversion without filesystem IO.
        if workspace.as_os_str().as_encoded_bytes().len() > MAX_SCOPE_ENCODED_BYTES {
            return Err(context_error());
        }
        Ok(Self {
            options,
            scope: workspace_digest(workspace),
            runtime: Mutex::new(RuntimeState::default()),
        })
    }

    pub fn options(&self) -> ContextOptions {
        self.options
    }

    /// Parent calls this with the original human input at the actual append/save
    /// boundary. No generic User-message, summary, notification or prose scan.
    pub fn record_input(
        &self,
        state: &mut AgentState,
        input: &str,
    ) -> Result<InputStamp, AgentError> {
        if input.len() > MAX_OBSERVED_INPUT_BYTES {
            return Err(context_error());
        }
        if !self.options.enabled {
            let mut runtime = self.runtime.lock().map_err(|_| context_error())?;
            runtime.passive_input_sequence = runtime
                .passive_input_sequence
                .checked_add(1)
                .ok_or_else(context_error)?;
            return InputStamp::for_input(runtime.passive_input_sequence, input);
        }
        self.check_failure()?;
        let mut record = self.bound_record(state)?;
        let stamp = record.input(input)?;
        record.store(state)?;
        Ok(stamp)
    }

    pub fn input_stamp(&self, state: &AgentState) -> Result<Option<InputStamp>, AgentError> {
        if !self.options.enabled {
            return Ok(None);
        }
        self.check_failure()?;
        Ok(state::Stored::load(state, &self.scope)?.last_input)
    }

    pub fn validate_state(&self, state: &AgentState) -> Result<(), AgentError> {
        if self.options.enabled {
            self.check_failure()?;
            state::Stored::load(state, &self.scope)?.preflight_event()?;
        }
        Ok(())
    }

    pub fn observe_tool(
        &self,
        state: &mut AgentState,
        call: &ToolCall,
        outcome: &ToolOutcome,
    ) -> Result<(), AgentError> {
        if !self.options.enabled {
            return Ok(());
        }
        if matches!(
            outcome.meta.error_kind,
            Some(
                craftsman_harness::ToolErrorKind::NeedsApproval
                    | craftsman_harness::ToolErrorKind::DeniedCapability
            )
        ) {
            return Ok(());
        }
        self.check_failure()?;
        let mut record = self.bound_record(state)?;
        record.tool(call, outcome)?;
        record.store(state)
    }

    /// Actual runtime/resume notifications may have changed files outside
    /// observed tool selectors. This is an uncertainty event, not compaction.
    pub fn observe_runtime_activity(&self, state: &mut AgentState) -> Result<(), AgentError> {
        if !self.options.enabled {
            return Ok(());
        }
        self.check_failure()?;
        let mut record = self.bound_record(state)?;
        record.runtime_activity()?;
        record.store(state)
    }

    pub fn begin_compaction(&self, state: &AgentState) -> Result<TranscriptAnchor, AgentError> {
        self.validate_state(state)?;
        Ok(TranscriptAnchor::capture(state))
    }

    pub fn finish_compaction(
        &self,
        state: &mut AgentState,
        before: &TranscriptAnchor,
    ) -> Result<CompactionObservation, AgentError> {
        let observation = before.compare(&TranscriptAnchor::capture(state));
        self.observe_compaction(state, observation)?;
        Ok(observation)
    }

    /// The parent supplies Changed only after a real owned manual replacement
    /// returns; denied, cancelled or superseded passes do not establish it.
    pub fn observe_compaction(
        &self,
        state: &mut AgentState,
        observation: CompactionObservation,
    ) -> Result<(), AgentError> {
        if !self.options.enabled || observation == CompactionObservation::Unchanged {
            return Ok(());
        }
        self.check_failure()?;
        let mut record = self.bound_record(state)?;
        record.compaction(observation == CompactionObservation::Changed)?;
        record.store(state)?;
        self.reset_report()
    }

    pub fn compile(&self, state: &AgentState) -> Result<Option<CompiledContext>, AgentError> {
        if !self.options.enabled {
            return Ok(None);
        }
        self.check_failure()?;
        let mut record = state::Stored::load(state, &self.scope)?;
        record.changed_scope(&self.scope)?;
        Ok(Some(view::compile(&record, state, self.options)?))
    }

    pub fn append_context(
        &self,
        state: &mut AgentState,
    ) -> Result<Option<CompiledCounts>, AgentError> {
        if !self.options.enabled {
            return Ok(None);
        }
        self.check_failure()?;
        let record = self.bound_record(state)?;
        let compiled = self.compile(state)?.ok_or_else(context_error)?;
        record.store(state)?;
        state.system_extras.push(compiled.text);
        Ok(Some(compiled.counts))
    }

    pub fn capture_request(&self, request: &ModelRequest) -> Result<(), AgentError> {
        if !self.options.enabled {
            return Ok(());
        }
        self.check_failure()?;
        self.runtime
            .lock()
            .map_err(|_| context_error())?
            .last_request = Some(RequestShape::capture(request));
        Ok(())
    }

    pub fn render(
        &self,
        state: &AgentState,
        model_window: Option<u64>,
    ) -> Result<String, AgentError> {
        if !self.options.enabled {
            return render_disabled(state, model_window);
        }
        self.check_failure()?;
        let mut record = state::Stored::load(state, &self.scope)?;
        record.changed_scope(&self.scope)?;
        let runtime = self.runtime.lock().map_err(|_| context_error())?;
        view::render(
            self.options,
            state,
            Some(&record),
            runtime.last_request.as_ref(),
            model_window,
        )
    }

    /// Rebind after actual thread/model/workspace selection. No hook, sampling,
    /// checkpoint or filesystem action is performed here.
    pub fn rebind(&self, state: &mut AgentState) -> Result<(), AgentError> {
        if self.options.enabled {
            let mut record = self.bound_record(state)?;
            record.uncertain_files();
            record.store(state)?;
        }
        let mut runtime = self.runtime.lock().map_err(|_| context_error())?;
        runtime.last_request = None;
        runtime.failed = false;
        Ok(())
    }

    pub fn reset_report(&self) -> Result<(), AgentError> {
        self.runtime
            .lock()
            .map_err(|_| context_error())?
            .last_request = None;
        Ok(())
    }

    /// Used only for an explicit fresh conversation; unrelated state is retained.
    pub fn clear(&self, state: &mut AgentState) -> Result<(), AgentError> {
        state.custom.remove(state::KEY);
        *self.runtime.lock().map_err(|_| context_error())? = RuntimeState::default();
        Ok(())
    }

    #[cfg(test)]
    pub fn observer(self: &Arc<Self>) -> Arc<dyn AgentMiddleware> {
        Arc::new(ToolObserver {
            controller: self.clone(),
            trust: None,
        })
    }

    /// Production callers bind this predicate to actual permitted builtin
    /// identities in the final registry, not merely to a guest/tool name.
    pub fn observer_with_trust(
        self: &Arc<Self>,
        trust: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> Arc<dyn AgentMiddleware> {
        Arc::new(ToolObserver {
            controller: self.clone(),
            trust: Some(trust),
        })
    }

    pub fn request_observer(self: &Arc<Self>) -> Arc<dyn AgentMiddleware> {
        Arc::new(RequestObserver {
            controller: self.clone(),
        })
    }

    /// Used when a selected profile omits automatic compaction; compilation
    /// still has an explicit prepare hook and adds no summarization request.
    pub fn compiler(self: &Arc<Self>) -> Arc<dyn AgentMiddleware> {
        Arc::new(ContextCompiler {
            controller: self.clone(),
        })
    }

    #[cfg(test)]
    pub fn wrap_compaction(
        self: &Arc<Self>,
        original: Arc<CompactionMiddleware>,
    ) -> Arc<dyn AgentMiddleware> {
        self.wrap_compaction_with(original, None)
    }

    pub fn wrap_compaction_with(
        self: &Arc<Self>,
        original: Arc<CompactionMiddleware>,
        on_transition: Option<Arc<dyn Fn(CompactionObservation) + Send + Sync>>,
    ) -> Arc<dyn AgentMiddleware> {
        Arc::new(ContextCompaction {
            controller: self.clone(),
            original,
            on_transition,
        })
    }

    fn bound_record(&self, state: &AgentState) -> Result<state::Stored, AgentError> {
        let mut record = state::Stored::load(state, &self.scope)?;
        record.changed_scope(&self.scope)?;
        Ok(record)
    }

    fn check_failure(&self) -> Result<(), AgentError> {
        if self.runtime.lock().map_err(|_| context_error())?.failed {
            Err(context_error())
        } else {
            Ok(())
        }
    }

    fn latch_failure(&self) {
        if let Ok(mut runtime) = self.runtime.lock() {
            runtime.failed = true;
        }
    }
}

pub(crate) fn render_disabled(
    state: &AgentState,
    model_window: Option<u64>,
) -> Result<String, AgentError> {
    view::render(ContextOptions::default(), state, None, None, model_window)
}

struct ToolObserver {
    controller: Arc<ContextController>,
    trust: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
}
#[async_trait]
impl AgentMiddleware for ToolObserver {
    fn name(&self) -> &str {
        "context_observations"
    }

    async fn turn_start(&self, _: &mut AgentState) -> Result<(), AgentError> {
        self.controller.reset_report()
    }

    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        self.controller.validate_state(ctx.state)?;
        let trusted =
            call.name.len() <= 512 && self.trust.as_ref().is_none_or(|trust| trust(&call.name));
        let observed = if trusted {
            bounded_call(&call)
        } else {
            ToolCall {
                id: String::new(),
                name: "_context_untrusted_tool".into(),
                args: Value::Null,
            }
        };
        match next.run(call, ctx).await {
            Ok(outcome) => {
                // An observation failure must not erase an already completed
                // result. Latch it before the next sampling boundary instead.
                if self
                    .controller
                    .observe_tool(ctx.state, &observed, &outcome)
                    .is_err()
                {
                    self.controller.latch_failure();
                }
                Ok(outcome)
            }
            Err(error) => {
                let uncertain = ToolOutcome::err("Operation effects are unavailable.");
                if self
                    .controller
                    .observe_tool(ctx.state, &observed, &uncertain)
                    .is_err()
                {
                    self.controller.latch_failure();
                }
                Err(error)
            }
        }
    }
}

struct RequestObserver {
    controller: Arc<ContextController>,
}
#[async_trait]
impl AgentMiddleware for RequestObserver {
    fn name(&self) -> &str {
        "context_request_view"
    }
    async fn around_model(
        &self,
        request: ModelRequest,
        next: Next<'_>,
    ) -> Result<ModelResponse, AgentError> {
        self.controller.capture_request(&request)?;
        next.run(request).await
    }
}

struct ContextCompiler {
    controller: Arc<ContextController>,
}
#[async_trait]
impl AgentMiddleware for ContextCompiler {
    fn name(&self) -> &str {
        "context_compiler"
    }
    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        self.controller.append_context(state)?;
        Ok(())
    }
}

struct ContextCompaction {
    controller: Arc<ContextController>,
    original: Arc<CompactionMiddleware>,
    on_transition: Option<Arc<dyn Fn(CompactionObservation) + Send + Sync>>,
}
#[async_trait]
impl AgentMiddleware for ContextCompaction {
    fn name(&self) -> &str {
        "compaction"
    }
    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        if !self.controller.options.enabled && self.on_transition.is_none() {
            return self.original.prepare_model(state).await;
        }
        let before = if self.controller.options.enabled {
            self.controller.begin_compaction(state)?
        } else {
            TranscriptAnchor::capture(state)
        };
        self.original.prepare_model(state).await?;
        let observation = if self.controller.options.enabled {
            self.controller.finish_compaction(state, &before)?
        } else {
            before.compare(&TranscriptAnchor::capture(state))
        };
        if observation != CompactionObservation::Unchanged {
            if let Some(callback) = &self.on_transition {
                callback(observation);
            }
        }
        self.controller.append_context(state)?;
        Ok(())
    }
}

fn bounded_call(call: &ToolCall) -> ToolCall {
    let name = if call.name.len() <= 512 {
        call.name.clone()
    } else {
        "_context_unknown_tool".into()
    };
    let path = call
        .args
        .get("file_path")
        .and_then(Value::as_str)
        .filter(|path| path.len() <= state::MAX_SELECTOR_BYTES);
    ToolCall {
        id: String::new(),
        name,
        args: json!({
            "file_path": path,
            "offset": call.args.get("offset").and_then(Value::as_u64),
            "limit": call.args.get("limit").and_then(Value::as_u64),
        }),
    }
}

#[derive(Default)]
struct AnchorBudget {
    bytes: usize,
    nodes: usize,
}

impl AnchorBudget {
    fn bytes(&mut self, count: usize) -> bool {
        let Some(bytes) = self
            .bytes
            .checked_add(count)
            .filter(|bytes| *bytes <= MAX_ANCHOR_BYTES)
        else {
            return false;
        };
        self.bytes = bytes;
        true
    }

    fn node(&mut self) -> bool {
        let Some(nodes) = self.nodes.checked_add(1) else {
            return false;
        };
        self.nodes = nodes;
        self.nodes <= MAX_ANCHOR_NODES && self.bytes(8)
    }

    fn json(&mut self, value: &Value, depth: usize) -> bool {
        if depth > MAX_ANCHOR_DEPTH || !self.node() {
            return false;
        }
        match value {
            Value::String(text) => self.bytes(text.len()),
            Value::Array(values) => {
                values.len() <= MAX_ANCHOR_FIELDS
                    && values.iter().all(|value| self.json(value, depth + 1))
            }
            Value::Object(values) => {
                if values.len() > MAX_ANCHOR_FIELDS
                    || values.keys().any(|key| key.len() > MAX_ANCHOR_KEY_BYTES)
                {
                    return false;
                }
                values
                    .iter()
                    .all(|(key, value)| self.bytes(key.len()) && self.json(value, depth + 1))
            }
            Value::Number(_) => self.bytes(64),
            _ => true,
        }
    }
}

fn anchor_input_fits(messages: &[craftsman_harness::Msg]) -> bool {
    if messages.len() > MAX_ANCHOR_MESSAGES {
        return false;
    }
    let mut budget = AnchorBudget::default();
    for message in messages {
        if !budget.node() || !budget.bytes(128) {
            return false;
        }
        let content = match message {
            craftsman_harness::Msg::User { content }
            | craftsman_harness::Msg::System { content }
            | craftsman_harness::Msg::Tool { content, .. } => content,
            craftsman_harness::Msg::Assistant {
                content,
                tool_calls,
            } => {
                if tool_calls.len() > 64 {
                    return false;
                }
                for call in tool_calls {
                    if call.id.len() > MAX_ANCHOR_KEY_BYTES
                        || call.name.len() > MAX_ANCHOR_KEY_BYTES
                        || !budget.node()
                        || !budget.bytes(call.id.len())
                        || !budget.bytes(call.name.len())
                        || !budget.json(&call.args, 0)
                    {
                        return false;
                    }
                }
                content
            }
        };
        if !budget.bytes(content.len()) {
            return false;
        }
        if let craftsman_harness::Msg::Tool { tool_call_id, .. } = message {
            if tool_call_id.len() > MAX_ANCHOR_KEY_BYTES || !budget.bytes(tool_call_id.len()) {
                return false;
            }
        }
    }
    true
}

fn workspace_digest(workspace: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        state::digest(workspace.as_os_str().as_bytes())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut digest = Sha256::new();
        for unit in workspace.as_os_str().encode_wide() {
            digest.update(unit.to_le_bytes());
        }
        format!("{:x}", digest.finalize())
    }
    #[cfg(not(any(unix, windows)))]
    {
        state::digest(workspace.as_os_str().as_encoded_bytes())
    }
}

fn config_error() -> AgentError {
    AgentError::State(CONFIG_ERROR.into())
}
fn context_error() -> AgentError {
    AgentError::State(STATE_ERROR.into())
}

#[cfg(test)]
mod tests;
