//! OpenAI-compatible chat-completions client with SSE streaming.
//!
//! Speaks the exact wire shape both craftsman routes use (the platform `llm`
//! app at `/apps/llm/api/v1` and any direct LiteLLM/OpenAI endpoint):
//! `POST {base}/chat/completions` with `stream: true`, accumulating
//! content deltas and tool-call argument fragments across chunks.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON string of arguments, accumulated across deltas.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<Value>>,
    },
    Tool {
        content: String,
        tool_call_id: String,
    },
}

impl Message {
    pub fn assistant_with_calls(content: Option<String>, calls: &[ToolCall]) -> Message {
        let tc = calls
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "type": "function",
                    "function": {"name": c.name, "arguments": c.arguments},
                })
            })
            .collect();
        Message::Assistant {
            content,
            tool_calls: Some(tc),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Default)]
pub struct TurnResult {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub finish_reason: Option<String>,
}

pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    pub model: String,
    request_defaults: serde_json::Map<String, Value>,
    request_source: Option<std::sync::Arc<crate::exec::provider::request::Source>>,
    // Bounded last private failure only, NOT a usage/admission/trace ledger.
    diagnostic:
        std::sync::Mutex<Option<std::sync::Arc<crate::exec::provider::diagnostics::Diagnostic>>>,
}

/// This client alone issues an initiated-send witness immediately before the
/// actual awaited send. It is NOT a receipt, billing or logical admission.
pub(crate) struct Sent {
    lease: crate::exec::provider::request::Lease,
    ordinal: usize,
}
impl Sent {
    pub(crate) fn metadata(&self) -> (&crate::exec::provider::request::Lease, usize) {
        (&self.lease, self.ordinal)
    }
    pub(crate) fn into_parts(self) -> (crate::exec::provider::request::Lease, usize) {
        (self.lease, self.ordinal)
    }
}

/// Only this client issues a receipt after send actually returns a response.
/// No caller response/client/gate/seed constructor or Debug/Serialize exists.
pub(crate) struct Received {
    response: reqwest::Response,
    lease: crate::exec::provider::request::Lease,
    ordinal: usize,
}
impl Received {
    pub(crate) fn metadata(&self) -> (&crate::exec::provider::request::Lease, usize, u16) {
        (&self.lease, self.ordinal, self.response.status().as_u16())
    }
    pub(crate) fn into_parts(
        self,
    ) -> (
        crate::exec::provider::request::Lease,
        usize,
        reqwest::Response,
    ) {
        (self.lease, self.ordinal, self.response)
    }
}

/// Client-only witness minted AFTER the actual supplied send future's first
/// poll. It is not proof of an HTTP wire attempt, receipt, usage or billing.
pub(crate) struct PolledSend {
    lease: crate::exec::provider::request::Lease,
    ordinal: usize,
}
impl PolledSend {
    pub(crate) fn into_parts(self) -> (crate::exec::provider::request::Lease, usize) {
        (self.lease, self.ordinal)
    }
}
/// Client-only witness minted AFTER the actual awaited send returned Err.
pub(crate) struct SendFailed {
    lease: crate::exec::provider::request::Lease,
    ordinal: usize,
}
impl SendFailed {
    pub(crate) fn into_parts(self) -> (crate::exec::provider::request::Lease, usize) {
        (self.lease, self.ordinal)
    }
}

impl LlmClient {
    pub fn new_with_options(
        base_url: &str,
        api_key: &str,
        model: &str,
        options: &std::collections::BTreeMap<String, Value>,
    ) -> Self {
        // connect_timeout bounds a dead host; read_timeout bounds a stalled
        // stream between chunks (a healthy stream ticks well under 60s even
        // while a slow model thinks). No overall timeout — long turns are
        // legitimate.
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(default_headers) = options.get("default_headers").and_then(Value::as_object) {
            for (name, value) in default_headers {
                let Ok(name) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) else {
                    continue;
                };
                let Some(value) = value.as_str() else {
                    continue;
                };
                let Ok(value) = reqwest::header::HeaderValue::from_str(value) else {
                    continue;
                };
                headers.insert(name, value);
            }
        }
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(120))
            .default_headers(headers)
            .build()
            .unwrap_or_default();
        let request_defaults = options
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "default_headers" | "use_responses_api"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            request_defaults,
            request_source: None,
            diagnostic: std::sync::Mutex::new(None),
        }
    }

    /// Private source construction accepts only the actual builder's opaque
    /// route/options/seeds/gate lease, never an arbitrary client or redactor.
    pub(crate) fn headless(
        scope: crate::agent::ProviderScope<'_>,
    ) -> Result<Self, crate::exec::provider::request::Fault> {
        let source =
            std::sync::Arc::new(crate::exec::provider::request::Source::from_scope(&scope)?);
        let route = scope.prepared().credentials().route();
        let mut client = Self::new_with_options(
            &route.base_url,
            &route.api_key,
            scope.model(),
            scope.options(),
        );
        client.request_source = Some(source);
        Ok(client)
    }
    pub(crate) async fn stream_model_turn(
        &self,
        original: &craftsman_harness::ModelRequest,
    ) -> anyhow::Result<TurnResult> {
        let source = self
            .request_source
            .as_ref()
            .ok_or(crate::exec::provider::request::Fault::Collision)?;
        *self.diagnostic.lock().unwrap() = None;
        let request = source.build(original)?;
        let result = self
            .stream_body(RequestBody::Checked(&request), |_| {}, |_| {})
            .await;
        if let Err(error) = &result {
            if let Some(failure) =
                error.downcast_ref::<crate::exec::provider::diagnostics::Failure>()
            {
                failure.0.bind(source, &request)?;
                // SAME latch overrides kind/text without losing original receipts.
                return result;
            }
        }
        source.ready(&request)?; // No stopped response/control can escape even without an outer wrapper.
        result
    }

    /// Preflight uses ONLY this actual scoped client's checked request source.
    pub(crate) fn prepare_reservation(
        &self,
        original: &craftsman_harness::ModelRequest,
    ) -> Result<crate::exec::provider::request::Checked, crate::exec::provider::request::Fault>
    {
        self.request_source
            .as_ref()
            .ok_or(crate::exec::provider::request::Fault::Collision)?
            .build(original)
    }
    /// Sealed combined-admission permit, not caller bytes or a detached reply
    /// number. Every existing send/retry still rechecks this SAME authority.
    pub(crate) async fn stream_admitted_model_turn(
        &self,
        admitted: &crate::exec::meter::tokens::Admitted,
    ) -> anyhow::Result<TurnResult> {
        let source = self
            .request_source
            .as_ref()
            .ok_or(crate::exec::provider::request::Fault::Collision)?;
        admitted.ready_for(source)?;
        let result = self
            .stream_body(RequestBody::Admitted(admitted), |_| {}, |_| {})
            .await;
        if let Err(error) = &result {
            if let Some(failure) =
                error.downcast_ref::<crate::exec::provider::diagnostics::Failure>()
            {
                failure.0.bind(source, admitted.checked())?;
                return result;
            }
        }
        admitted.ready_for(source)?;
        result
    }

    async fn send_observed<F>(
        &self,
        sending: F,
        scope: &crate::exec::meter::receipts::Scope<'_>,
        lease: crate::exec::provider::request::Lease,
        ordinal: usize,
    ) -> Result<Result<reqwest::Response, reqwest::Error>, crate::exec::meter::Fault>
    where
        F: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
    {
        let sent = Sent {
            lease: lease.clone(),
            ordinal,
        };
        let mut ticket = scope.prepare(&sent)?;
        let mut sending = std::pin::pin!(sending);
        let mut first = true;
        let result = std::future::poll_fn(|cx| {
            if first {
                if let Err(fault) = ticket.before_poll() {
                    return std::task::Poll::Ready(Err(fault));
                }
            }
            let result = sending.as_mut().poll(cx);
            if first {
                first = false;
                // The inner future was genuinely polled, even if it returned
                // Pending/no wire IO. Never mint this during preparation.
                if let Err(fault) = ticket.observed_poll(PolledSend {
                    lease: lease.clone(),
                    ordinal,
                }) {
                    return std::task::Poll::Ready(Err(fault));
                }
            }
            result.map(Ok)
        })
        .await?;
        match result {
            Ok(response) => {
                let received = Received {
                    response,
                    lease,
                    ordinal,
                };
                ticket.received(&received)?; // actual header survives late stop
                Ok(Ok(received.into_parts().2))
            }
            Err(error) => {
                ticket.failed(SendFailed { lease, ordinal })?;
                Ok(Err(error))
            }
        }
    }

    /// Test-only real client/lease preparation, deliberately NO send future
    /// poll/result witness. Drops the genuine prepared ticket without traffic.
    #[cfg(test)]
    pub(crate) fn prepare_only_for_observation_tests(
        &self,
        admitted: &crate::exec::meter::tokens::Admitted,
    ) -> Result<(), crate::exec::meter::Fault> {
        let source = self
            .request_source
            .as_ref()
            .ok_or(crate::exec::meter::Fault::Foreign)?;
        admitted
            .ready_for(source)
            .map_err(|_| crate::exec::meter::Fault::Foreign)?;
        let lease = source
            .lease(admitted.checked())
            .map_err(|_| crate::exec::meter::Fault::Foreign)?;
        let scope = admitted.observation_scope(lease.clone())?;
        let sent = Sent { lease, ordinal: 1 };
        let _prepared = scope.prepare(&sent)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn request_source_for_tests(
        &self,
    ) -> Option<std::sync::Arc<crate::exec::provider::request::Source>> {
        self.request_source.clone()
    }
    #[cfg(test)]
    pub(crate) async fn send_checked_for_tests(
        &self,
        request: &crate::exec::provider::request::Checked,
    ) -> anyhow::Result<TurnResult> {
        self.stream_body(RequestBody::Checked(request), |_| {}, |_| {})
            .await
    }

    #[cfg(test)]
    pub(crate) fn diagnostic_for_tests(
        &self,
    ) -> Option<std::sync::Arc<crate::exec::provider::diagnostics::Diagnostic>> {
        self.diagnostic.lock().unwrap().clone()
    }
    fn private_failure(
        &self,
        call: crate::exec::provider::diagnostics::Call,
        kind: crate::exec::provider::diagnostics::Kind,
        text: Option<crate::exec::provider::diagnostics::Detail>,
    ) -> anyhow::Error {
        let diagnostic = std::sync::Arc::new(call.failure(kind, text));
        *self.diagnostic.lock().unwrap() = Some(diagnostic.clone());
        crate::exec::provider::diagnostics::Failure(diagnostic).into()
    }

    /// Stream one completion. `on_text` fires per content delta (the REPL
    /// prints it live); tool-call deltas accumulate silently into the result.
    /// `on_status` reports a bounded pre-output transport retry.
    pub async fn stream_turn(
        &self,
        messages: &[Message],
        tools: &[Value],
        on_text: impl FnMut(&str),
        on_status: impl FnMut(&str),
    ) -> anyhow::Result<TurnResult> {
        if self.request_source.is_some() {
            return Err(crate::exec::provider::request::Fault::Collision.into());
        }
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if let Some(object) = body.as_object_mut() {
            for (key, value) in &self.request_defaults {
                object.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }

        self.stream_body(RequestBody::Legacy(&body), on_text, on_status)
            .await
    }

    async fn stream_body(
        &self,
        body: RequestBody<'_>,
        mut on_text: impl FnMut(&str),
        mut on_status: impl FnMut(&str),
    ) -> anyhow::Result<TurnResult> {
        // One retry is safe only before any content has been shown to the user.
        // Retrying after visible deltas would duplicate text and could splice two
        // different generations together. Tool-call-only partial attempts are
        // safe to discard because no tool has executed yet.
        const MAX_ATTEMPTS: usize = 2;
        let mut diagnostics = match body {
            RequestBody::Checked(request) => {
                *self.diagnostic.lock().unwrap() = None;
                let source = self
                    .request_source
                    .as_ref()
                    .ok_or(crate::exec::provider::request::Fault::Collision)?;
                Some(crate::exec::provider::diagnostics::Call::new(
                    source.lease(request)?,
                ))
            }
            RequestBody::Admitted(admitted) => {
                *self.diagnostic.lock().unwrap() = None;
                let source = self
                    .request_source
                    .as_ref()
                    .ok_or(crate::exec::provider::request::Fault::Collision)?;
                admitted.ready_for(source)?;
                Some(crate::exec::provider::diagnostics::Call::new(
                    source.lease(admitted.checked())?,
                ))
            }
            RequestBody::Legacy(_) => None,
        };
        let observations = match (body, diagnostics.as_ref()) {
            (RequestBody::Admitted(admitted), Some(call)) => {
                Some(admitted.observation_scope(call.lease())?)
            }
            _ => None, // Existing default/request-only frontends do not opt in.
        };
        for attempt in 0..MAX_ATTEMPTS {
            if let Err(fault) = body.ready(self) {
                if let Some(call) = diagnostics.take() {
                    return Err(self.private_failure(call, diagnostic_fault(fault), None));
                }
                return Err(fault.into());
            }
            let builder = self
                .http
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                // Ask proxies/providers not to compress the stream. This avoids
                // a class of upstream body-decoder failures and keeps SSE framing
                // transparent end-to-end.
                .header(reqwest::header::ACCEPT_ENCODING, "identity");
            let builder = match body {
                RequestBody::Legacy(value) => builder.json(value),
                RequestBody::Checked(checked) => builder
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(checked.bytes().to_vec()),
                RequestBody::Admitted(admitted) => builder
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(admitted.checked().bytes().to_vec()),
            };
            // SAME source/latch after the checked-byte copy, before EVERY send/retry.
            if let Err(fault) = body.ready(self) {
                if let Some(call) = diagnostics.take() {
                    return Err(self.private_failure(call, diagnostic_fault(fault), None));
                }
                return Err(fault.into());
            }
            if let Some(call) = &mut diagnostics {
                call.begin(Sent {
                    lease: call.lease(),
                    ordinal: attempt + 1,
                })?;
            }
            let sent = match observations.as_ref() {
                Some(scope) => {
                    self.send_observed(
                        builder.send(),
                        scope,
                        scope_lease(&diagnostics)?,
                        attempt + 1,
                    )
                    .await?
                }
                None => builder.send().await,
            };
            let resp = match sent {
                Ok(response) => response,
                Err(_) if diagnostics.is_some() => {
                    let call = diagnostics.as_mut().unwrap();
                    call.connection_failed();
                    if call.ready().is_err() {
                        return Err(self.private_failure(
                            diagnostics.take().unwrap(),
                            crate::exec::provider::diagnostics::Kind::Stopped,
                            None,
                        ));
                    }
                    if attempt + 1 < MAX_ATTEMPTS {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        continue;
                    }
                    return Err(self.private_failure(
                        diagnostics.take().unwrap(),
                        crate::exec::provider::diagnostics::Kind::Connection,
                        None,
                    ));
                }
                Err(error) if attempt + 1 < MAX_ATTEMPTS => {
                    on_status(&format!(
                        "LLM stream connection failed before output ({error}); retrying once…"
                    ));
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };

            let status = resp.status();
            let resp = if let Some(call) = &mut diagnostics {
                let received = Received {
                    response: resp,
                    lease: call.lease(),
                    ordinal: attempt + 1,
                };
                call.received(&received)?;
                received.into_parts().2
            } else {
                resp
            };
            if !status.is_success() {
                if let Some(call) = &mut diagnostics {
                    let receipt = Received {
                        response: resp,
                        lease: call.lease(),
                        ordinal: attempt + 1,
                    };
                    let text = match call.error_body(receipt).await {
                        Ok(text) => text,
                        Err(kind) => {
                            return Err(self.private_failure(
                                diagnostics.take().unwrap(),
                                kind,
                                None,
                            ));
                        }
                    };
                    let transient = status.is_server_error()
                        || status == reqwest::StatusCode::REQUEST_TIMEOUT
                        || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
                    if transient && attempt + 1 < MAX_ATTEMPTS {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        continue;
                    }
                    return Err(self.private_failure(
                        diagnostics.take().unwrap(),
                        crate::exec::provider::diagnostics::Kind::Http,
                        Some(text),
                    ));
                }
                let text = resp.text().await.unwrap_or_default();
                let error = anyhow::anyhow!("HTTP {status}: {}", truncate(&text, 400));
                let transient = status.is_server_error()
                    || status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
                if transient && attempt + 1 < MAX_ATTEMPTS {
                    on_status(&format!(
                        "LLM endpoint returned {status} before output; retrying once…"
                    ));
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    continue;
                }
                return Err(error);
            }

            let mut out = TurnResult::default();
            // index → (id, name, args) — deltas address calls by index.
            let mut calls: Vec<ToolCall> = Vec::new();
            let mut stream = resp.bytes_stream();
            // Byte accumulator: UTF-8 characters can split across network
            // chunks, so decode only complete SSE events, never raw chunks.
            let mut pending: Vec<u8> = Vec::new();
            let mut saw_done = false;
            let mut valid_events = 0usize;
            let mut received_bytes = 0usize;
            let mut visible_bytes = 0usize;
            let mut read_error: Option<reqwest::Error> = None;

            'read_stream: while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        read_error = Some(error);
                        break;
                    }
                };
                received_bytes = received_bytes.saturating_add(chunk.len());
                pending.extend_from_slice(&chunk);
                if pending.len() > 4_000_000 {
                    return Err(anyhow::anyhow!(
                        "SSE buffer exceeded 4MB without an event boundary"
                    ));
                }
                // Events end at a blank line — LF-LF or CRLF-CRLF framing.
                while let Some((pos, sep)) = find_event_boundary(&pending) {
                    let event_bytes: Vec<u8> = pending.drain(..pos + sep).collect();
                    ingest_sse_event(
                        &event_bytes[..pos],
                        &mut out,
                        &mut calls,
                        &mut saw_done,
                        &mut valid_events,
                        &mut visible_bytes,
                        &mut on_text,
                    );
                    if saw_done {
                        break 'read_stream;
                    }
                }
            }

            // A proxy that terminates immediately after its final data line may
            // omit the last blank-line delimiter. Parse that tail once rather
            // than silently dropping a complete final JSON event.
            if !pending.is_empty() {
                ingest_sse_event(
                    &pending,
                    &mut out,
                    &mut calls,
                    &mut saw_done,
                    &mut valid_events,
                    &mut visible_bytes,
                    &mut on_text,
                );
            }

            // A finish_reason is sufficient even if a proxy drops the optional
            // [DONE] sentinel or errors while reading trailing usage metadata.
            let complete = saw_done || out.finish_reason.is_some();
            if complete {
                out.tool_calls = finalize_tool_calls(calls);
                return Ok(out);
            }

            if let Some(call) = &mut diagnostics {
                call.stream_ended(received_bytes, read_error.is_some());
                if call.ready().is_err() {
                    return Err(self.private_failure(
                        diagnostics.take().unwrap(),
                        crate::exec::provider::diagnostics::Kind::Stopped,
                        None,
                    ));
                }
                if visible_bytes == 0 && attempt + 1 < MAX_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    continue;
                }
                return Err(self.private_failure(
                    diagnostics.take().unwrap(),
                    crate::exec::provider::diagnostics::Kind::Stream,
                    None,
                ));
            }
            let reason = match read_error {
                Some(error) => format!("{error:#}"),
                None if valid_events == 0 => "upstream closed without any SSE events".to_string(),
                None => "upstream closed before finish_reason or [DONE]".to_string(),
            };

            if visible_bytes == 0 && attempt + 1 < MAX_ATTEMPTS {
                on_status(&format!(
                    "LLM event stream ended before output ({reason}); retrying once…"
                ));
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            }

            anyhow::bail!(
                "LLM event stream interrupted before completion after {received_bytes} bytes: \
                 {reason}. The upstream proxy/provider closed a truncated response; retry the turn."
            );
        }

        unreachable!("stream attempts either return or continue")
    }
}

fn scope_lease(
    diagnostics: &Option<crate::exec::provider::diagnostics::Call>,
) -> Result<crate::exec::provider::request::Lease, crate::exec::provider::request::Fault> {
    diagnostics
        .as_ref()
        .map(|c| c.lease())
        .ok_or(crate::exec::provider::request::Fault::Collision)
}

fn diagnostic_fault(
    fault: crate::exec::provider::request::Fault,
) -> crate::exec::provider::diagnostics::Kind {
    use crate::exec::provider::{diagnostics::Kind, request::Fault};
    match fault {
        Fault::Stopped => Kind::Stopped,
        Fault::Limit => Kind::Limit,
        Fault::Collision => Kind::Redaction,
    }
}

#[derive(Clone, Copy)]
enum RequestBody<'a> {
    Legacy(&'a Value),
    Checked(&'a crate::exec::provider::request::Checked),
    Admitted(&'a crate::exec::meter::tokens::Admitted),
}
impl RequestBody<'_> {
    fn ready(self, client: &LlmClient) -> Result<(), crate::exec::provider::request::Fault> {
        match (self, client.request_source.as_ref()) {
            (Self::Legacy(_), None) => Ok(()),
            (Self::Checked(request), Some(source)) => source.ready(request),
            (Self::Admitted(admitted), Some(source)) => admitted.ready_for(source),
            _ => Err(crate::exec::provider::request::Fault::Collision),
        }
    }
}

fn ingest_sse_event(
    event_bytes: &[u8],
    out: &mut TurnResult,
    calls: &mut Vec<ToolCall>,
    saw_done: &mut bool,
    valid_events: &mut usize,
    visible_bytes: &mut usize,
    on_text: &mut impl FnMut(&str),
) {
    let event = String::from_utf8_lossy(event_bytes);
    let data = event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|raw| raw.strip_prefix(' ').unwrap_or(raw))
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return;
    }
    if data.trim() == "[DONE]" {
        *saw_done = true;
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(&data) else {
        return;
    };
    *valid_events += 1;
    let before = out.content.len();
    ingest_chunk(&value, out, calls, on_text);
    *visible_bytes = visible_bytes.saturating_add(out.content.len().saturating_sub(before));
}

fn finalize_tool_calls(calls: Vec<ToolCall>) -> Vec<ToolCall> {
    // Providers occasionally omit tool-call ids; synthesize stable ones so
    // assistant tool_calls and tool results stay linkable.
    calls
        .into_iter()
        .enumerate()
        .filter(|(_, call)| !call.name.is_empty())
        .map(|(index, mut call)| {
            if call.id.is_empty() {
                call.id = format!("call_{index}");
            }
            call
        })
        .collect()
}

/// Position and separator length of the first SSE event boundary
/// (`\n\n` or `\r\n\r\n`; a lone `\n\r\n` tail also matches via LF-LF
/// after the preceding line's \r is included in the event text).
fn find_event_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf.windows(2).position(|w| w == b"\n\n");
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(a), Some(b)) if b + 2 == a => Some((b, 4)), // same boundary — consume CRLF form
        (Some(a), Some(b)) if b < a => Some((b, 4)),
        (Some(a), _) => Some((a, 2)),
        (None, Some(b)) => Some((b, 4)),
        (None, None) => None,
    }
}

fn ingest_chunk(
    v: &Value,
    out: &mut TurnResult,
    calls: &mut Vec<ToolCall>,
    on_text: &mut impl FnMut(&str),
) {
    if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
        out.usage.input_tokens = u["prompt_tokens"].as_u64().unwrap_or(0);
        out.usage.output_tokens = u["completion_tokens"].as_u64().unwrap_or(0);
    }
    let Some(choice) = v["choices"].get(0) else {
        return;
    };
    if let Some(fr) = choice["finish_reason"].as_str() {
        out.finish_reason = Some(fr.to_string());
    }
    let delta = &choice["delta"];
    if let Some(text) = delta["content"].as_str() {
        if !text.is_empty() {
            out.content.push_str(text);
            on_text(text);
        }
    }
    if let Some(tcs) = delta["tool_calls"].as_array() {
        for tc in tcs {
            let idx = tc["index"].as_u64().unwrap_or(0) as usize;
            while calls.len() <= idx {
                calls.push(ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
            }
            let slot = &mut calls[idx];
            if let Some(id) = tc["id"].as_str() {
                if !id.is_empty() {
                    slot.id = id.to_string();
                }
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                slot.name.push_str(name);
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                slot.arguments.push_str(args);
            }
        }
    }
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(events: &[&str]) -> TurnResult {
        let mut out = TurnResult::default();
        let mut calls = Vec::new();
        let mut text = String::new();
        for e in events {
            let v: Value = serde_json::from_str(e).unwrap();
            ingest_chunk(&v, &mut out, &mut calls, &mut |t| text.push_str(t));
        }
        out.tool_calls = calls.into_iter().filter(|c| !c.name.is_empty()).collect();
        assert_eq!(text, out.content);
        out
    }

    #[test]
    fn accumulates_content_deltas() {
        let out = feed(&[
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(out.content, "Hello");
        assert_eq!(out.finish_reason.as_deref(), Some("stop"));
        assert!(out.tool_calls.is_empty());
    }

    #[test]
    fn accumulates_tool_call_argument_fragments() {
        let out = feed(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"execute","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"comm"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"and\": \"ls\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ]);
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].name, "execute");
        let args: Value = serde_json::from_str(&out.tool_calls[0].arguments).unwrap();
        assert_eq!(args["command"], "ls");
    }

    #[test]
    fn parallel_tool_calls_by_index() {
        let out = feed(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"execute","arguments":"{}"}}]}}]}"#,
        ]);
        assert_eq!(out.tool_calls.len(), 2);
        assert_eq!(out.tool_calls[0].name, "read_file");
        assert_eq!(out.tool_calls[1].name, "execute");
    }

    #[test]
    fn usage_captured_from_final_chunk() {
        let out = feed(&[
            r#"{"choices":[{"delta":{"content":"x"}}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3}}"#,
        ]);
        assert_eq!(out.usage.input_tokens, 10);
        assert_eq!(out.usage.output_tokens, 3);
    }

    #[test]
    fn event_boundary_handles_lf_and_crlf() {
        assert_eq!(find_event_boundary(b"data: x\n\nrest"), Some((7, 2)));
        assert_eq!(find_event_boundary(b"data: x\r\n\r\nrest"), Some((7, 4)));
        assert_eq!(find_event_boundary(b"data: x\r\n"), None);
        assert_eq!(find_event_boundary(b""), None);
        // CRLF boundary earlier than a later LF-LF one wins.
        let buf = b"a\r\n\r\nb\n\n";
        assert_eq!(find_event_boundary(buf), Some((1, 4)));
    }

    #[test]
    fn unterminated_final_event_is_ingested() {
        let mut out = TurnResult::default();
        let mut calls = Vec::new();
        let mut saw_done = false;
        let mut valid_events = 0;
        let mut visible_bytes = 0;
        let mut streamed = String::new();
        ingest_sse_event(
            br#"data: {"choices":[{"delta":{"content":"tail"},"finish_reason":"stop"}]}"#,
            &mut out,
            &mut calls,
            &mut saw_done,
            &mut valid_events,
            &mut visible_bytes,
            &mut |text| streamed.push_str(text),
        );
        assert_eq!(out.content, "tail");
        assert_eq!(streamed, "tail");
        assert_eq!(out.finish_reason.as_deref(), Some("stop"));
        assert_eq!(valid_events, 1);
        assert_eq!(visible_bytes, 4);
    }

    #[tokio::test]
    async fn delayed_sse_events_reach_callback_before_stream_finishes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 16_384];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            socket
                .write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"first\"}}]}\n\n")
                .await
                .unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            socket
                .write_all(
                    b"data: {\"choices\":[{\"delta\":{\"content\":\" second\"}}]}\n\n\
                      data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
                      data: [DONE]\n\n",
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
        });

        let client = LlmClient::new_with_options(
            &format!("http://{address}"),
            "key",
            "model",
            &std::collections::BTreeMap::new(),
        );
        let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let turn = tokio::spawn(async move {
            client
                .stream_turn(
                    &[Message::User {
                        content: "hello".into(),
                    }],
                    &[],
                    move |text| {
                        let _ = delta_tx.send(text.to_string());
                    },
                    |_| {},
                )
                .await
        });

        let first = tokio::time::timeout(std::time::Duration::from_secs(1), delta_rx.recv())
            .await
            .expect("first delta should stream before completion")
            .expect("delta channel open");
        assert_eq!(first, "first");
        assert!(
            !turn.is_finished(),
            "turn must still be waiting for the delayed event"
        );
        let second = delta_rx.recv().await.unwrap();
        assert_eq!(second, " second");
        let result = turn.await.unwrap().unwrap();
        assert_eq!(result.content, "first second");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn retries_truncated_stream_once_before_visible_output() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

        struct TruncatedThenComplete(Arc<AtomicUsize>);
        impl Respond for TruncatedThenComplete {
            fn respond(&self, _request: &Request) -> ResponseTemplate {
                let attempt = self.0.fetch_add(1, Ordering::SeqCst);
                let body = if attempt == 0 {
                    "data: {\"choices\":[]}\n\n".to_string()
                } else {
                    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
                     data: [DONE]\n\n"
                        .to_string()
                };
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body)
            }
        }

        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(TruncatedThenComplete(calls.clone()))
            .mount(&server)
            .await;
        let client = LlmClient::new_with_options(
            &server.uri(),
            "key",
            "model",
            &std::collections::BTreeMap::new(),
        );
        let messages = vec![Message::User {
            content: "hello".into(),
        }];
        let mut output = String::new();
        let mut statuses = Vec::new();
        let result = client
            .stream_turn(
                &messages,
                &[],
                |text| output.push_str(text),
                |status| statuses.push(status.to_string()),
            )
            .await
            .unwrap();
        assert_eq!(result.content, "ok");
        assert_eq!(output, "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(
            statuses
                .iter()
                .any(|status| status.contains("retrying once"))
        );
    }

    #[tokio::test]
    async fn partial_visible_stream_fails_explicitly_without_duplicate_retry() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

        struct AlwaysTruncated(Arc<AtomicUsize>);
        impl Respond for AlwaysTruncated {
            fn respond(&self, _request: &Request) -> ResponseTemplate {
                self.0.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
                    )
            }
        }

        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(AlwaysTruncated(calls.clone()))
            .mount(&server)
            .await;
        let client = LlmClient::new_with_options(
            &server.uri(),
            "key",
            "model",
            &std::collections::BTreeMap::new(),
        );
        let messages = vec![Message::User {
            content: "hello".into(),
        }];
        let mut output = String::new();
        let error = client
            .stream_turn(&messages, &[], |text| output.push_str(text), |_| {})
            .await
            .unwrap_err();
        assert_eq!(output, "partial");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(error.to_string().contains("interrupted before completion"));
    }

    #[test]
    fn data_field_without_space_is_accepted() {
        // WHATWG SSE: "data:x" (no space) is valid.
        let event = "data:{\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}";
        let mut out = TurnResult::default();
        let mut calls = Vec::new();
        for line in event.lines() {
            let raw = line.strip_prefix("data:").unwrap();
            let data = raw.strip_prefix(' ').unwrap_or(raw);
            let v: Value = serde_json::from_str(data).unwrap();
            ingest_chunk(&v, &mut out, &mut calls, &mut |_| {});
        }
        assert_eq!(out.content, "ok");
    }
}
