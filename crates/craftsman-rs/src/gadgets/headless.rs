//! Private source-approved guest host. NOT a model tool/catalog or exec route.
//!
//! Only owned ApprovedSource bytes enter compilation. Its original filesystem
//! authority/policy and the same invocation fail latch cover every import.
//! JSON/log output selects the frozen redactor; original provenance remains
//! private, with no ToolOutcome/schema producer or callable registration.
//! Legacy GadgetRuntime/HostState/defaults are not selected or altered here.

#[cfg(test)]
#[path = "../../tests/common/headless_gadget_fixture.rs"]
pub(crate) mod fixture;
mod http;
mod output;
#[cfg(test)]
mod tests;

use super::confined::{ConfinedGuestFs, Phase};
use super::host::{Limits, bindings};
use super::runtime::{CallBudget, Ticker};
pub(crate) use super::sources::SourceKind;
use super::sources::{ApprovalRecord, ApprovedSource, SourceAuthority};
use super::{ErrorKind, GadgetError};
use crate::exec::redaction::Redactor;
use crate::exec::runtime::{GuestScope, InvocationGate};
use bindings::craftsman::gadget::host::{DirEntry, Host, Level, Request, Response};
use craftsman_gadget::{Description, MAX_JSON_BYTES};
use output::RedactedCapture;
use serde_json::Value;
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

#[derive(Clone, Copy)]
pub(crate) struct GuestLimits {
    pub(crate) timeout: Duration,
    pub(crate) memory_bytes: usize,
}
impl Default for GuestLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            memory_bytes: 256 * 1024 * 1024,
        }
    }
}
impl GuestLimits {
    fn validate(&self) -> Result<(), GadgetError> {
        if self.timeout.is_zero()
            || self.timeout > Duration::from_secs(300)
            || !(4 * 1024 * 1024..=256 * 1024 * 1024).contains(&self.memory_bytes)
        {
            return Err(error(
                ErrorKind::InvalidArgs,
                "invalid private guest resource limits",
            ));
        }
        Ok(())
    }
}

/// JSON/log views are redacted; original approval and trusted execution facts
/// stay private. No Serialize/ToolOutcome/public schema producer is supplied.
pub(crate) struct PrivateCallResult {
    pub(crate) json: String,
    pub(crate) logs: String,
    pub(crate) logs_truncated: bool,
    pub(crate) duration: Duration,
    pub(crate) network_calls: u64,
    pub(crate) approval: ApprovalRecord,
}

pub(crate) struct GuestHost {
    engine: Engine,
    _ticker: Ticker,
    linker: Linker<HeadlessState>,
    sources: SourceAuthority,
    gate: Arc<InvocationGate>,
    limits: GuestLimits,
    client: reqwest::Client,
    redactor: Redactor,
    #[cfg(test)]
    compiled: std::sync::atomic::AtomicUsize,
}

impl GuestHost {
    /// Mandatory opaque runtime lease: not legacy RuntimeOptions, paths, arbitrary
    /// backends/declarations/callbacks, or even a new gate with the same policy.
    pub(crate) async fn from_scope(
        scope: GuestScope<'_>,
        explicit: Vec<String>,
        limits: GuestLimits,
    ) -> Result<Self, GadgetError> {
        limits.validate()?;
        let prepared = scope.prepared();
        let gate = scope.gate();
        let redactor = prepared.credentials().redactor().clone();
        if !gate.matches_permissions(prepared.permissions()) {
            return Err(error(
                ErrorKind::InvalidArgs,
                "guest host needs the same prepared invocation gate",
            ));
        }
        ensure(&gate, None)?;
        let sources = SourceAuthority::from_prepared(prepared, explicit)
            .await
            .map_err(GadgetError::from)?;
        let mut settings = wasmtime::Config::new();
        settings
            .wasm_component_model(true)
            .epoch_interruption(true)
            .wasm_gc(false);
        // No persistent cache configuration, native code deserialization or SDK writes.
        let engine = Engine::new(&settings)
            .map_err(|_| error(ErrorKind::Io, "private guest engine construction failed"))?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|_| error(ErrorKind::Io, "private guest WASI linking failed"))?;
        bindings::craftsman::gadget::host::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
            &mut linker,
            |state| state,
        )
        .map_err(|_| error(ErrorKind::Io, "private guest import linking failed"))?;
        let ticker = Ticker::new(engine.clone()).map_err(|_| {
            error(
                ErrorKind::Io,
                "private guest epoch timer construction failed",
            )
        })?;
        let client = http::client()?;
        ensure(&gate, None)?;
        Ok(Self {
            engine,
            _ticker: ticker,
            linker,
            sources,
            gate,
            limits,
            client,
            redactor,
            #[cfg(test)]
            compiled: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Per-operation budget covers source reads, compilation, instantiation and
    /// describe/call, not provider token admission or an OS process deadline.
    async fn load(
        &self,
        kind: SourceKind,
        id: &str,
        budget: &Arc<CallBudget>,
    ) -> Result<(ApprovedSource, Component), GadgetError> {
        let result = self
            .guarded(budget, async {
                let source = self
                    .sources
                    .authorize(kind, id)
                    .await
                    .map_err(GadgetError::from)?;
                if !self.gate.matches_permissions(source.permissions()) {
                    return Err(error(
                        ErrorKind::InvalidArgs,
                        "approved source belongs to another invocation",
                    ));
                }
                ensure(&self.gate, Some(budget))?;
                let bytes = source.component_bytes().to_vec();
                let engine = self.engine.clone();
                let component = tokio::task::spawn_blocking(move || Component::new(&engine, bytes))
                    .await
                    .map_err(|_| error(ErrorKind::Io, "private guest compilation worker failed"))?
                    .map_err(|_| {
                        error(
                            ErrorKind::InvalidGadget,
                            "approved bytes are not a supported component",
                        )
                    })?;
                #[cfg(test)]
                self.compiled
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok((source, component))
            })
            .await;
        self.observe(result)
    }

    fn store(
        &self,
        source: &ApprovedSource,
        phase: Phase,
        budget: Arc<CallBudget>,
    ) -> Result<Store<HeadlessState>, GadgetError> {
        let state = HeadlessState::from_source(
            source,
            self.gate.clone(),
            phase,
            budget,
            self.limits.memory_bytes,
            self.client.clone(),
            &self.redactor,
        )?;
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(|context| {
            if ensure(&context.data().gate, Some(&context.data().budget)).is_err() {
                Err(wasmtime::Error::msg("private guest execution stopped"))
            } else {
                Ok(UpdateDeadline::Yield(1))
            }
        });
        Ok(store)
    }

    async fn describe_source(
        &self,
        source: &ApprovedSource,
        component: &Component,
        budget: &Arc<CallBudget>,
    ) -> Result<Description, GadgetError> {
        // Capabilities remain EMPTY even after approval; never use call's store.
        let mut store = self.store(source, Phase::Describe, budget.clone())?;
        let result = self
            .guarded(budget, async {
                let gadget =
                    bindings::Gadget::instantiate_async(&mut store, component, &self.linker)
                        .await
                        .map_err(|_| {
                            error(
                                ErrorKind::InvalidGadget,
                                "approved component does not implement the gadget ABI",
                            )
                        })?;
                gadget
                    .craftsman_gadget_guest()
                    .call_describe(&mut store)
                    .await
                    .map_err(|_| error(ErrorKind::Trap, "private guest describe trapped"))
            })
            .await
            .map_err(|e| classify(e, store.data()))?;
        let description = parse_description(&result).map_err(|e| classify(e, store.data()))?;
        // Describe logs use the same mandatory adapter, but metadata stays
        // original/private for source-bound schema/dispatch validation.
        store
            .data()
            .capture
            .finish()
            .map_err(|e| classify(e, store.data()))?;
        ensure(&self.gate, Some(budget)).map_err(|e| classify(e, store.data()))?;
        Ok(description)
    }

    pub(crate) async fn describe(
        &self,
        kind: SourceKind,
        id: &str,
    ) -> Result<Description, GadgetError> {
        let budget = Arc::new(CallBudget::new(self.limits.timeout));
        let (source, component) = self.load(kind, id, &budget).await?;
        let result = self.describe_source(&source, &component, &budget).await;
        self.observe(result)
    }

    pub(crate) async fn call(
        &self,
        kind: SourceKind,
        id: &str,
        function: &str,
        args: Value,
    ) -> Result<PrivateCallResult, GadgetError> {
        let started = Instant::now();
        let budget = Arc::new(CallBudget::new(self.limits.timeout));
        ensure(&self.gate, Some(&budget))?;
        if !craftsman_gadget::valid_function_name(function) {
            return Err(error(
                ErrorKind::InvalidArgs,
                "invalid private guest function name",
            ));
        }
        let input = serde_json::to_string(&args)
            .map_err(|_| error(ErrorKind::InvalidArgs, "invalid private guest input"))?;
        if input.len() > MAX_JSON_BYTES {
            return Err(error(
                ErrorKind::OutputLimit,
                "private guest arguments exceed 1 MiB",
            ));
        }
        let (source, component) = self.load(kind, id, &budget).await?;
        let description = self.describe_source(&source, &component, &budget).await?;
        let definition = description
            .functions
            .iter()
            .find(|f| f.name == function)
            .ok_or_else(|| error(ErrorKind::NotFound, "private guest function does not exist"))?;
        let validator = validator(&definition.input_schema)?;
        if !validator.is_valid(&args) {
            return Err(error(
                ErrorKind::InvalidArgs,
                "private guest arguments do not match the schema",
            ));
        }
        let mut store = self.store(&source, Phase::Call, budget.clone())?;
        let returned = self
            .guarded(&budget, async {
                let gadget =
                    bindings::Gadget::instantiate_async(&mut store, &component, &self.linker)
                        .await
                        .map_err(|_| {
                            error(
                                ErrorKind::InvalidGadget,
                                "approved component does not implement the gadget ABI",
                            )
                        })?;
                gadget
                    .craftsman_gadget_guest()
                    .call_call(&mut store, function, &input)
                    .await
                    .map_err(|_| error(ErrorKind::Trap, "private guest call trapped"))
            })
            .await
            .map_err(|e| classify(e, store.data()))?;
        use bindings::exports::craftsman::gadget::guest::CallError;
        let json = returned.map_err(|e| {
            classify(
                match e {
                    CallError::UnknownFunction(_) => {
                        error(ErrorKind::NotFound, "private guest function does not exist")
                    }
                    CallError::InvalidArgs(_) => error(
                        ErrorKind::InvalidArgs,
                        "private guest rejected its arguments",
                    ),
                    CallError::Failed(_) => error(
                        store.data().last_failure.unwrap_or(ErrorKind::Failed),
                        "private guest function failed",
                    ),
                },
                store.data(),
            )
        })?;
        if json.len() > MAX_JSON_BYTES {
            return Err(classify(
                error(ErrorKind::OutputLimit, "private guest result exceeds 1 MiB"),
                store.data(),
            ));
        }
        // Arguments/schema/host authorization consumed ORIGINAL values above.
        // Only the completed guest's publication view is redacted here.
        let json = output::json(&self.redactor, &json).map_err(|e| classify(e, store.data()))?;
        ensure(&self.gate, Some(&budget)).map_err(|e| classify(e, store.data()))?;
        let (logs, logs_truncated) = store
            .data()
            .capture
            .finish()
            .map_err(|e| classify(e, store.data()))?;
        ensure(&self.gate, Some(&budget)).map_err(|e| classify(e, store.data()))?;
        Ok(PrivateCallResult {
            json,
            logs,
            logs_truncated,
            duration: started.elapsed(),
            network_calls: store.data().http.network_calls,
            approval: source.approval().clone(),
        })
    }

    async fn guarded<T>(
        &self,
        budget: &Arc<CallBudget>,
        future: impl Future<Output = Result<T, GadgetError>>,
    ) -> Result<T, GadgetError> {
        bounded(&self.gate, budget, future).await
    }

    fn observe<T>(&self, result: Result<T, GadgetError>) -> Result<T, GadgetError> {
        if let Err(e) = &result {
            self.gate.observe_error_kind(e.tool_kind());
        }
        result
    }
}

struct HeadlessState {
    wasi: WasiCtx,
    table: wasmtime::component::ResourceTable,
    limits: Limits,
    capture: RedactedCapture,
    filesystem: ConfinedGuestFs,
    http: http::HttpFacet,
    gate: Arc<InvocationGate>,
    budget: Arc<CallBudget>,
    last_failure: Option<ErrorKind>,
}
impl WasiView for HeadlessState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}
impl HeadlessState {
    fn from_source(
        source: &ApprovedSource,
        gate: Arc<InvocationGate>,
        phase: Phase,
        budget: Arc<CallBudget>,
        memory_bytes: usize,
        client: reqwest::Client,
        redactor: &Redactor,
    ) -> Result<Self, GadgetError> {
        if !gate.matches_permissions(source.permissions()) {
            return Err(error(
                ErrorKind::InvalidArgs,
                "approved source belongs to another invocation",
            ));
        }
        ensure(&gate, Some(&budget))?;
        let capture = RedactedCapture::new(redactor, gate.clone(), budget.clone());
        let mut wasi = WasiCtxBuilder::new();
        wasi.stdout(capture.clone())
            .stderr(capture.clone())
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false);
        // No inherited env/args/stdio, preopens, sockets, human input or pause.
        Ok(Self {
            wasi: wasi.build(),
            table: wasmtime::component::ResourceTable::new(),
            limits: Limits::new(memory_bytes),
            capture,
            filesystem: ConfinedGuestFs::from_source(source, phase),
            http: http::HttpFacet::from_source(source, phase, gate.clone(), client),
            gate,
            budget,
            last_failure: None,
        })
    }
    #[cfg(test)]
    fn set_budget_for_tests(&mut self, budget: Arc<CallBudget>) {
        self.capture.set_budget_for_tests(budget.clone());
        self.budget = budget;
    }
    fn result<T>(&mut self, result: Result<T, GadgetError>) -> Result<T, String> {
        if let Err(e) = &result {
            self.gate.observe_error_kind(e.tool_kind());
        }
        let result = ensure(&self.gate, Some(&self.budget)).and(result);
        result.map_err(|e| {
            self.last_failure = Some(e.kind);
            self.gate.observe_error_kind(e.tool_kind());
            e.to_string()
        })
    }
}
impl Host for HeadlessState {
    async fn log(&mut self, level: Level, message: String) {
        if ensure(&self.gate, Some(&self.budget)).is_ok() {
            // Bound the WHOLE import record before allocation/appending a prefix.
            if message.len() > output::MAX_LOG_BYTES - 16 {
                self.capture.reject_oversized_record();
                return;
            }
            let record = format!("[{level:?}] {message}\n");
            let _ = self.capture.append(record.as_bytes());
        }
    }
    async fn read_file(&mut self, path: String) -> Result<Vec<u8>, String> {
        let result = bounded(&self.gate, &self.budget, self.filesystem.read_file(&path)).await;
        self.result(result)
    }
    async fn list_dir(&mut self, path: String) -> Result<Vec<DirEntry>, String> {
        let result = bounded(&self.gate, &self.budget, async {
            self.filesystem.list_dir(&path).await.map(|items| {
                items
                    .into_iter()
                    .map(|e| DirEntry {
                        name: e.name,
                        is_dir: e.is_dir,
                        size: e.size,
                    })
                    .collect()
            })
        })
        .await;
        self.result(result)
    }
    async fn write_file(&mut self, path: String, bytes: Vec<u8>) -> Result<(), String> {
        let result = bounded(
            &self.gate,
            &self.budget,
            self.filesystem.write_file(&path, &bytes),
        )
        .await;
        self.result(result)
    }
    async fn http_request(&mut self, request: Request) -> Result<Response, String> {
        let result = bounded(
            &self.gate,
            &self.budget,
            self.http.request(&self.budget, request),
        )
        .await;
        self.result(result)
    }
}

async fn bounded<T>(
    gate: &InvocationGate,
    budget: &CallBudget,
    future: impl Future<Output = Result<T, GadgetError>>,
) -> Result<T, GadgetError> {
    ensure(gate, Some(budget))?;
    // A caught import followed by a WASI sleep/CPU loop cannot postpone fail
    // until timeout. Dropping active work is NOT rollback of blocking IO.
    let result = tokio::select! { biased;
        _ = gate.wait_approval_required() => Err(error(ErrorKind::NeedsApproval, "private guest invocation requires approval")),
        result = budget.run(future) => result,
    };
    ensure(gate, Some(budget))?;
    result
}

fn ensure(gate: &InvocationGate, budget: Option<&CallBudget>) -> Result<(), GadgetError> {
    if gate.approval_required() {
        return Err(error(
            ErrorKind::NeedsApproval,
            "private guest invocation requires approval",
        ));
    }
    if budget.is_some_and(CallBudget::expired) {
        return Err(error(
            ErrorKind::Timeout,
            "private guest operation timed out",
        ));
    }
    Ok(())
}
fn error(kind: ErrorKind, message: &'static str) -> GadgetError {
    GadgetError::new(kind, message)
}
fn classify(mut e: GadgetError, state: &HeadlessState) -> GadgetError {
    if state.gate.approval_required() {
        e = error(
            ErrorKind::NeedsApproval,
            "private guest invocation requires approval",
        );
    } else if state.limits.memory_denied {
        e = error(
            ErrorKind::OutOfMemory,
            "private guest memory limit exceeded",
        );
    } else if state.budget.expired() {
        e = error(ErrorKind::Timeout, "private guest operation timed out");
    }
    e.network_calls = state.http.network_calls;
    state.capture.abort();
    e.logs_truncated = state.capture.truncated();
    e
}

struct NoRetrieval;
impl jsonschema::Retrieve for NoRetrieval {
    fn retrieve(
        &self,
        _: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(std::io::Error::other(
            "private guest schemas must be self-contained",
        )))
    }
}
fn validator(schema: &Value) -> Result<jsonschema::Validator, GadgetError> {
    jsonschema::options()
        .with_retriever(NoRetrieval)
        .with_pattern_options(jsonschema::PatternOptions::fancy_regex().backtrack_limit(10_000))
        .build(schema)
        .map_err(|_| {
            error(
                ErrorKind::InvalidGadget,
                "private guest schema is invalid or needs external resources",
            )
        })
}
fn parse_description(text: &str) -> Result<Description, GadgetError> {
    if text.len() > MAX_JSON_BYTES {
        return Err(error(
            ErrorKind::OutputLimit,
            "private guest description exceeds 1 MiB",
        ));
    }
    let mut description: Description = serde_json::from_str(text).map_err(|_| {
        error(
            ErrorKind::InvalidGadget,
            "private guest description is invalid",
        )
    })?;
    if description.functions.len() > 128 {
        return Err(error(
            ErrorKind::OutputLimit,
            "private guest has too many functions",
        ));
    }
    let mut names = BTreeSet::new();
    for f in &description.functions {
        if !craftsman_gadget::valid_function_name(&f.name)
            || !names.insert(&f.name)
            || f.description.trim().is_empty()
            || f.description.len() > 16 * 1024
            || !f.input_schema.is_object()
        {
            return Err(error(
                ErrorKind::InvalidGadget,
                "private guest function metadata is invalid",
            ));
        }
        validator(&f.input_schema)?;
    }
    description
        .functions
        .sort_unstable_by(|a, b| a.name.cmp(&b.name));
    Ok(description)
}

// Test-only source-input optimization. The production host/budgets stay exact.
#[cfg(test)]
#[path = "../../tests/common/headless_focused_gadget_fixture.rs"]
mod focused_fixture;
