//! Whole private request views before argument escaping and HTTP serialization.
//! Checked bytes are bound to one actual client source, never control/state data.
use crate::agent::ProviderScope;
use crate::exec::redaction::{RedactionError, Redactor};
use crate::exec::runtime::InvocationGate;
use craftsman_harness::{ModelRequest, Msg};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::Arc;
const BYTES: usize = 1024 * 1024;
const NODES: usize = 100_000;
const DEPTH: usize = 64;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    Limit,
    Collision,
    Stopped,
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Fault {}
impl From<RedactionError> for Fault {
    fn from(value: RedactionError) -> Self {
        match value {
            RedactionError::Record | RedactionError::Chunk | RedactionError::Position => {
                Self::Limit
            }
            _ => Self::Collision,
        }
    }
}

pub(crate) struct Source {
    gate: Arc<InvocationGate>,
    redactor: Redactor,
    model: String,
    defaults: BTreeMap<String, Value>,
    agents: Vec<String>,
    #[cfg(test)]
    observed: std::sync::atomic::AtomicU64,
}
pub(crate) struct Checked {
    source: Arc<Source>,
    identity: Arc<()>,
    bytes: Vec<u8>,
}

/// Opaque identity of one checked request and its actual client-source/latch.
/// Only Source can mint it; no caller gate, seeds, route or response constructor.
#[derive(Clone)]
pub(crate) struct Lease {
    source: Arc<Source>,
    identity: Arc<()>,
}
impl Lease {
    pub(crate) fn ready(&self) -> Result<(), Fault> {
        if self.stopped() {
            Err(Fault::Stopped)
        } else {
            Ok(())
        }
    }
    pub(crate) fn stopped(&self) -> bool {
        self.source.gate.approval_required()
    }
    pub(super) async fn wait_stopped(&self) {
        self.source.gate.wait_approval_required().await;
    }
    #[cfg(test)]
    pub(super) fn observed_for_tests(&self, bytes: u64) {
        // Actual received-body observation only; no raw callback or authority.
        self.source
            .observed
            .store(bytes, std::sync::atomic::Ordering::Release);
    }
    pub(super) fn redactor(&self) -> &Redactor {
        &self.source.redactor
    }
    pub(super) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source) && Arc::ptr_eq(&self.identity, &other.identity)
    }
}
impl Checked {
    /// Already checked PUBLICATION bytes only, no raw request/seed accessor.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
impl Source {
    /// Only the actual builder's opaque scope can couple policy/seeds/options.
    pub(crate) fn from_scope(scope: &ProviderScope<'_>) -> Result<Self, Fault> {
        let mut budget = Budget::new(scope.prepared().credentials().redactor());
        budget.charge(scope.model().len())?;
        let mut defaults = BTreeMap::new();
        for (key, value) in scope.options() {
            if matches!(key.as_str(), "default_headers" | "use_responses_api") {
                continue;
            }
            budget.charge(key.len())?;
            budget.admit(value, 0)?; // before the immutable source copy, never truncate
            defaults.insert(key.clone(), value.clone());
        }
        if scope.agents().len() > NODES {
            return Err(Fault::Limit);
        }
        for name in scope.agents() {
            budget.charge(name.len())?;
        }
        Ok(Self {
            gate: scope.gate(),
            redactor: scope.prepared().credentials().redactor().clone(),
            model: scope.model().into(),
            defaults,
            agents: scope.agents().to_vec(),
            #[cfg(test)]
            observed: std::sync::atomic::AtomicU64::new(0),
        })
    }
    pub(crate) fn ready(self: &Arc<Self>, request: &Checked) -> Result<(), Fault> {
        if !Arc::ptr_eq(self, &request.source) {
            return Err(Fault::Collision);
        }
        if self.gate.approval_required() {
            return Err(Fault::Stopped);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn observed_for_tests(&self) -> u64 {
        self.observed.load(std::sync::atomic::Ordering::Acquire)
    }
    pub(crate) fn lease(self: &Arc<Self>, request: &Checked) -> Result<Lease, Fault> {
        self.ready(request)?;
        #[cfg(test)]
        self.observed.store(0, std::sync::atomic::Ordering::Release);
        Ok(Lease {
            source: self.clone(),
            identity: request.identity.clone(),
        })
    }
    /// Bind a diagnostic without erasing already-observed facts on a late stop.
    /// Its kind/text accessors independently observe this same monotonic latch.
    pub(crate) fn bind_lease(
        self: &Arc<Self>,
        request: &Checked,
        lease: &Lease,
    ) -> Result<(), Fault> {
        if !Arc::ptr_eq(self, &request.source)
            || !Arc::ptr_eq(self, &lease.source)
            || !Arc::ptr_eq(&request.identity, &lease.identity)
        {
            return Err(Fault::Collision);
        }
        Ok(())
    }
    pub(crate) fn build(self: &Arc<Self>, original: &ModelRequest) -> Result<Checked, Fault> {
        if self.gate.approval_required() {
            return Err(Fault::Stopped);
        }
        let mut b = Budget::new(&self.redactor);
        // Defined agent ids occur in natural-language descriptions/context too.
        // Do not turn those identities into display aliases by redacting prose.
        for name in &self.agents {
            b.text(name, true)?;
        }
        let mut messages = Vec::new();
        b.node(0)?;
        messages.push(json!({"role":"system","content":b.text(&original.system, false)?}));
        if original.messages.len() > NODES || original.tools.len() > NODES {
            return Err(Fault::Limit);
        }
        for message in &original.messages {
            b.node(0)?;
            let view = match message {
                Msg::System { content } => {
                    json!({"role":"system","content":b.text(content,false)?})
                }
                Msg::User { content } => json!({"role":"user","content":b.text(content,false)?}),
                Msg::Tool {
                    content,
                    tool_call_id,
                    ..
                } => {
                    json!({"role":"tool","content":b.text(content,false)?,"tool_call_id":b.text(tool_call_id,true)?})
                }
                Msg::Assistant {
                    content,
                    tool_calls,
                } => {
                    let mut value = Map::new();
                    value.insert("role".into(), json!("assistant"));
                    if tool_calls.is_empty() {
                        value.insert("content".into(), json!(b.text(content, false)?));
                    } else {
                        if !content.is_empty() {
                            value.insert("content".into(), json!(b.text(content, false)?));
                        }
                        if tool_calls.len() > NODES {
                            return Err(Fault::Limit);
                        }
                        let mut calls = Vec::new();
                        for call in tool_calls {
                            b.node(0)?;
                            let args = b.value(&call.args, Role::Data, 0)?;
                            let encoded = encode(&self.redactor, &args)?;
                            b.charge(encoded.len())?;
                            let args = String::from_utf8(encoded).map_err(|_| Fault::Collision)?;
                            calls.push(json!({"id":b.text(&call.id,true)?,"type":"function","function":{"name":b.text(&call.name,true)?,"arguments":args}}));
                        }
                        value.insert("tool_calls".into(), Value::Array(calls));
                    }
                    Value::Object(value)
                }
            };
            messages.push(view);
        }
        let mut body = json!({"model":b.text(&self.model,true)?,"messages":messages,"stream":true,"stream_options":{"include_usage":true}});
        let object = body.as_object_mut().ok_or(Fault::Collision)?;
        for (key, value) in &self.defaults {
            // Exact effective legacy precedence, without making a raw temporary
            // copy of fields that the fixed request already overrides.
            if object.contains_key(key) || (key == "tools" && !original.tools.is_empty()) {
                continue;
            }
            if key == "tools" {
                return Err(Fault::Collision);
            } // Config is not an extra tool-source grant.
            let key = b.text(key, true)?;
            let role = match key.as_str() {
                "metadata" => Role::Data,
                "response_format" => Role::ResponseFormat,
                _ => Role::Authority,
            };
            object.insert(key, b.value(value, role, 0)?);
        }
        if !original.tools.is_empty() {
            let mut tools = Vec::new();
            for tool in &original.tools {
                b.node(0)?;
                tools.push(json!({"type":"function","function":{"name":b.text(&tool.name,true)?,"description":b.text(&tool.description,false)?,"parameters":b.value(&tool.parameters,Role::Schema,0)?}}));
            }
            object.insert("tools".into(), Value::Array(tools));
        }
        let bytes = encode(&self.redactor, &body)?;
        let checked = Checked {
            source: self.clone(),
            identity: Arc::new(()),
            bytes,
        };
        self.ready(&checked)?;
        Ok(checked)
    }
}
/// Pure private message codec. Actual state/seeds/gate authority is supplied
/// only by the separate opaque Runtime MessageScope; no control is returned.
/// Original whole-message/decoded-value admission precedes EVERY data copy.
pub(in crate::exec) fn retained_message_view(
    redactor: &Redactor,
    original: &[Msg],
) -> Result<Vec<u8>, Fault> {
    const MESSAGES: usize = 256;
    const CALLS: usize = 256;
    const ID_SCALARS: usize = 128;
    if original.len() > MESSAGES {
        return Err(Fault::Limit);
    }
    let mut b = Budget::new(redactor);
    for message in original {
        b.node(0)?;
        let content = match message {
            Msg::System { content }
            | Msg::User { content }
            | Msg::Assistant { content, .. }
            | Msg::Tool { content, .. } => content,
        };
        b.charge(content.len())?;
        match message {
            Msg::Tool { tool_call_id, .. } => {
                b.charge(tool_call_id.len())?;
                if tool_call_id.chars().count() > ID_SCALARS {
                    return Err(Fault::Limit);
                }
            }
            Msg::Assistant { tool_calls, .. } => {
                if tool_calls.len() > CALLS {
                    return Err(Fault::Limit);
                }
                for call in tool_calls {
                    b.node(0)?;
                    b.charge(call.id.len())?;
                    b.charge(call.name.len())?;
                    if call.id.chars().count() > ID_SCALARS
                        || call.name.chars().count() > ID_SCALARS
                    {
                        return Err(Fault::Limit);
                    }
                    b.admit(&call.args, 0)?;
                }
            }
            _ => {}
        }
    }
    let mut messages = Vec::new();
    for message in original {
        b.node(0)?;
        messages.push(match message {
            Msg::System { content } => Msg::System {
                content: b.text(content, false)?,
            },
            Msg::User { content } => Msg::User {
                content: b.text(content, false)?,
            },
            Msg::Tool {
                tool_call_id,
                content,
                is_error,
            } => Msg::Tool {
                tool_call_id: b.text(tool_call_id, true)?,
                content: b.text(content, false)?,
                is_error: *is_error,
            },
            Msg::Assistant {
                content,
                tool_calls,
            } => {
                let mut calls = Vec::new();
                for call in tool_calls {
                    b.node(0)?;
                    calls.push(craftsman_harness::ToolCall {
                        id: b.text(&call.id, true)?,
                        name: b.text(&call.name, true)?,
                        args: b.value(&call.args, Role::Data, 0)?,
                    });
                }
                Msg::Assistant {
                    content: b.text(content, false)?,
                    tool_calls: calls,
                }
            }
        });
    }
    #[derive(Serialize)]
    struct Frame<'a> {
        message_scope: &'static str,
        messages: &'a [Msg],
        messages_omitted: usize,
    }
    // All current retained records or fixed refusal; no clipped/repaired prefix.
    encode(
        redactor,
        &Frame {
            message_scope: "retained_state",
            messages: &messages,
            messages_omitted: 0,
        },
    )
}

/// Pure private finalization fragment. Only the actual consumed Run's opaque
/// TerminalScope may mint its bound Snapshot; this helper grants no authority.
/// Per-channel original/view bounds stay private; combined encoded frame <=1MiB.
pub(in crate::exec) fn terminal_fragment(
    redactor: &Redactor,
    original: &[Msg],
    final_text: Option<&str>,
    returned: &'static str,
    approval_required: bool,
) -> Result<Vec<u8>, Fault> {
    let mut b = Budget::new(redactor);
    if let Some(text) = final_text {
        b.charge(text.len())?;
    }
    // Output original byte admission precedes message copies; unchanged whole
    // message/decoded-argument admission precedes all of their own copies.
    let messages = retained_message_view(redactor, original)?;
    let output = final_text.map(|text| b.text(text, false)).transpose()?;
    #[derive(Serialize)]
    struct Prefix<'a> {
        run_return: &'static str,
        approval_required: bool,
        final_output: Option<&'a str>,
    }
    let prefix = encode(
        redactor,
        &Prefix {
            run_return: returned,
            approval_required,
            final_output: output.as_deref(),
        },
    )?;
    if prefix.last() != Some(&b'}') || messages.first() != Some(&b'{') {
        return Err(Fault::Collision);
    }
    let size = prefix
        .len()
        .checked_add(messages.len())
        .and_then(|n| n.checked_sub(1))
        .filter(|n| *n <= BYTES)
        .ok_or(Fault::Limit)?;
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(&prefix[..prefix.len() - 1]);
    bytes.push(b',');
    bytes.extend_from_slice(&messages[1..]);
    if bytes.len() != size {
        return Err(Fault::Limit);
    }
    // Cross-fragment joins and all fixed keys/scalars/nulls remain verified.
    verify(redactor, &bytes)?;
    Ok(bytes)
}

/// Independently checked terminal prefix and complete retained-message channel.
/// Selection can reduce their COMBINED framing only after both original sources
/// pass unchanged producer bounds. This pure helper cannot mint any authority.
pub(in crate::exec) fn terminal_channels(
    redactor: &Redactor,
    original: &[Msg],
    final_text: Option<&str>,
    returned: &'static str,
    approval_required: bool,
) -> Result<(Vec<u8>, Vec<u8>), Fault> {
    let mut b = Budget::new(redactor);
    if let Some(text) = final_text {
        b.charge(text.len())?;
    }
    let messages = retained_message_view(redactor, original)?;
    let output = final_text.map(|text| b.text(text, false)).transpose()?;
    #[derive(Serialize)]
    struct Prefix<'a> {
        run_return: &'static str,
        approval_required: bool,
        final_output: Option<&'a str>,
    }
    let prefix = encode(
        redactor,
        &Prefix {
            run_return: returned,
            approval_required,
            final_output: output.as_deref(),
        },
    )?;
    Ok((prefix, messages))
}

#[derive(Clone, Copy)]
enum Role {
    Data,
    Authority,
    Schema,
    SchemaMap,
    SchemaArray,
    ResponseFormat,
    ResponseSchema,
}
struct Budget<'a> {
    redactor: &'a Redactor,
    nodes: usize,
    bytes: usize,
}
impl<'a> Budget<'a> {
    fn new(redactor: &'a Redactor) -> Self {
        Self {
            redactor,
            nodes: NODES,
            bytes: BYTES,
        }
    }
    fn charge(&mut self, bytes: usize) -> Result<(), Fault> {
        self.bytes = self.bytes.checked_sub(bytes).ok_or(Fault::Limit)?;
        Ok(())
    }
    fn node(&mut self, depth: usize) -> Result<(), Fault> {
        if depth > DEPTH {
            return Err(Fault::Limit);
        }
        self.nodes = self.nodes.checked_sub(1).ok_or(Fault::Limit)?;
        self.charge(1)
    }
    fn admit(&mut self, value: &Value, depth: usize) -> Result<(), Fault> {
        self.node(depth)?;
        match value {
            Value::String(s) => self.charge(s.len())?,
            Value::Number(n) => self.charge(n.to_string().len())?,
            Value::Array(rows) => {
                if rows.len() > self.nodes {
                    return Err(Fault::Limit);
                }
                for row in rows {
                    self.admit(row, depth + 1)?;
                }
            }
            Value::Object(fields) => {
                if fields.len() > self.nodes {
                    return Err(Fault::Limit);
                }
                for (k, v) in fields {
                    self.charge(k.len())?;
                    self.admit(v, depth + 1)?;
                }
            }
            _ => self.charge(5)?,
        };
        Ok(())
    }
    fn text(&mut self, original: &str, authority: bool) -> Result<String, Fault> {
        self.charge(original.len())?;
        if authority {
            verify(self.redactor, original.as_bytes())?;
            self.charge(original.len())?;
            return Ok(original.into());
        }
        let safe = self.redactor.text(original)?;
        self.charge(safe.len())?;
        verify(self.redactor, safe.as_bytes())?;
        Ok(safe)
    }
    fn value(&mut self, original: &Value, role: Role, depth: usize) -> Result<Value, Fault> {
        self.node(depth)?;
        Ok(match original {
            Value::String(s) => Value::String(self.text(s, !matches!(role, Role::Data))?),
            Value::Number(n) => {
                self.charge(n.to_string().len())?;
                original.clone()
            }
            Value::Bool(_) | Value::Null => {
                self.charge(5)?;
                original.clone()
            }
            Value::Array(rows) => {
                if rows.len() > self.nodes {
                    return Err(Fault::Limit);
                }
                let next = if matches!(role, Role::Schema | Role::SchemaArray) {
                    Role::Schema
                } else if matches!(role, Role::Data) {
                    Role::Data
                } else {
                    Role::Authority
                };
                Value::Array(
                    rows.iter()
                        .map(|v| self.value(v, next, depth + 1))
                        .collect::<Result<_, _>>()?,
                )
            }
            Value::Object(fields) => {
                if fields.len() > self.nodes {
                    return Err(Fault::Limit);
                }
                let mut out = Map::new();
                for (key, value) in fields {
                    let key = self.text(key, true)?; // Never rename data/schema member names.
                    let next = match role {
                        Role::Data => Role::Data,
                        Role::SchemaMap => Role::Schema,
                        Role::Schema => match key.as_str() {
                            "description" | "title" | "$comment" if value.is_string() => Role::Data,
                            "properties" | "patternProperties" | "$defs" | "definitions"
                            | "dependentSchemas" => Role::SchemaMap,
                            "items"
                            | "additionalProperties"
                            | "contains"
                            | "not"
                            | "if"
                            | "then"
                            | "else"
                            | "propertyNames"
                            | "unevaluatedProperties"
                            | "unevaluatedItems"
                            | "additionalItems"
                            | "contentSchema" => Role::Schema,
                            "allOf" | "anyOf" | "oneOf" | "prefixItems" => Role::SchemaArray,
                            _ => Role::Authority,
                        },
                        Role::ResponseFormat if key == "json_schema" => Role::ResponseSchema,
                        Role::ResponseSchema if key == "description" && value.is_string() => {
                            Role::Data
                        }
                        Role::ResponseSchema if key == "schema" => Role::Schema,
                        _ => Role::Authority,
                    };
                    out.insert(key, self.value(value, next, depth + 1)?);
                }
                Value::Object(out)
            }
        })
    }
}
/// Exact finite selection framing uses the SAME unchanged encoded-literal check.
/// This pure helper cannot mint a runtime-bound View/Selection or publish bytes.
pub(in crate::exec) fn verify_message_frame(
    redactor: &Redactor,
    bytes: &[u8],
) -> Result<(), Fault> {
    verify(redactor, bytes)
}

fn verify(redactor: &Redactor, bytes: &[u8]) -> Result<(), Fault> {
    if bytes.len() > BYTES {
        return Err(Fault::Limit);
    }
    let mut check = redactor.byte_stream();
    for chunk in bytes.chunks(64 * 1024) {
        check.push(chunk)?;
        if check.matched() {
            return Err(Fault::Collision);
        }
    }
    check.abort();
    Ok(())
}
fn encode(redactor: &Redactor, safe: &impl Serialize) -> Result<Vec<u8>, Fault> {
    struct Buffer(Vec<u8>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > BYTES.saturating_sub(self.0.len()) {
                return Err(io::Error::other("private request limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut out = Buffer(Vec::new());
    serde_json::to_writer(&mut out, safe).map_err(|_| Fault::Limit)?;
    verify(redactor, &out.0)?;
    Ok(out.0)
}

// Private token preflight over already checked bytes; old codecs stay unchanged.
pub(in crate::exec) mod reservation;

// Fixed identity comparisons only; old codec/lease APIs and seeds stay exact.
impl Lease {
    pub(in crate::exec) fn same_request(&self, other: &Self) -> bool {
        self.same(other)
    }
}
impl Checked {
    pub(in crate::exec) fn matches_lease(&self, lease: &Lease) -> bool {
        Arc::ptr_eq(&self.source, &lease.source) && Arc::ptr_eq(&self.identity, &lease.identity)
    }
}
