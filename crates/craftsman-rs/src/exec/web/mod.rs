//! Mandatory private web producers selected by the actual shared-builder lease.
//! Original URL/query/auth/permission facts stay separate from redacted views.
//! Not public exec, OS containment, provider/state/storage/trace publication.

mod capture;
mod html;
mod search;
use crate::agent::WebScope;
use crate::exec::redaction::Redactor;
use crate::exec::redaction::publication::{Output, View};
use crate::exec::runtime::InvocationGate;
use async_trait::async_trait;
use capture::{Capture, Fault, JsonInput};
use craftsman_harness::{AgentError, Tool, ToolCtx, ToolDef, ToolErrorKind, ToolMeta, ToolOutcome};
use reqwest::{Client, Url};
use serde_json::Value;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
const FETCH_BYTES: usize = 200_000;
const SEARCH_BYTES: usize = 512 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
const REDIRECTS: usize = 5;
const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

#[derive(Default)]
struct Facts {
    network_calls: u64,
    bytes: Option<u64>,
    status: Option<u16>,
}
struct Failure {
    kind: ToolErrorKind,
    message: &'static str,
}
impl Failure {
    fn new(kind: ToolErrorKind, message: &'static str) -> Self {
        Self { kind, message }
    }
    fn http(error: reqwest::Error) -> Self {
        Self::new(
            if error.is_timeout() {
                ToolErrorKind::Timeout
            } else {
                ToolErrorKind::Failed
            },
            "private web request failed",
        )
    }
}
impl From<Fault> for Failure {
    fn from(value: Fault) -> Self {
        Self::new(
            if value == Fault::Limit {
                ToolErrorKind::OutputLimit
            } else {
                ToolErrorKind::Failed
            },
            "private web response cannot be safely published",
        )
    }
}
type Result<T> = std::result::Result<T, Failure>;

pub(crate) struct Web {
    http: Option<Client>,
    gate: Arc<InvocationGate>,
    redactor: Redactor,
    search_key: Option<String>,
    search_provider: String,
    search_endpoint: Option<String>,
}
impl Web {
    /// The ONLY constructor consumes the actual builder's opaque scope. It
    /// never accepts caller paths/gates/seeds/clients or reopens config/env.
    pub(crate) fn from_scope(scope: WebScope<'_>) -> Self {
        let config = scope.prepared().config();
        let endpoint = Some(ENDPOINT.to_owned());
        #[cfg(test)]
        let endpoint = match config
            .extra
            .get("_test_web_search_endpoint")
            .and_then(Value::as_str)
        {
            None => endpoint,
            Some(raw) => test_endpoint(raw).map(|_| raw.to_owned()),
        };
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(TIMEOUT)
            .timeout(TIMEOUT)
            .user_agent("craftsman/0.1 (+https://github.com/trend-mgai)")
            .build()
            .ok();
        Self {
            http,
            gate: scope.gate(),
            redactor: scope.prepared().credentials().redactor().clone(),
            // Current search clients pass this literal. Preserve that SAME
            // captured string, never expand it again or apply default headers.
            search_key: config.search_api_key().map(str::to_owned),
            search_provider: config.search_provider().to_owned(),
            search_endpoint: endpoint,
        }
    }
    pub(crate) fn wrap_host_tool(self: &Arc<Self>, original: Arc<dyn Tool>) -> Arc<dyn Tool> {
        let def = original.def();
        if matches!(def.name.as_str(), "web_fetch" | "web_search") {
            Arc::new(ScopedTool {
                def,
                web: self.clone(),
            })
        } else {
            original
        }
    }
    pub(crate) async fn call(&self, name: &str, args: Value) -> ToolOutcome {
        let started = Instant::now();
        if let Some(outcome) = self.gate.denied(name, &args) {
            return outcome;
        } // captured fixed-denial publisher
        let mut facts = Facts::default();
        let result = {
            let operation = self.operation(name, &args, &mut facts);
            tokio::pin!(operation);
            tokio::select! { biased;
                _=self.gate.wait_approval_required()=>Err(Failure::new(ToolErrorKind::NeedsApproval,"private web invocation requires approval")),
                _=tokio::time::sleep(TIMEOUT)=>Err(Failure::new(ToolErrorKind::Timeout,"private web operation timed out")),
                result=&mut operation=>result,
            }
            // Drop the OWNED request/body future before reading its final facts;
            // dropping only Pin<&mut _> would leave the borrowing future alive.
        };
        self.publication(result, facts, started)
    }
    fn publication(&self, result: Result<View>, facts: Facts, started: Instant) -> ToolOutcome {
        let mut meta = ToolMeta::new();
        meta.duration = Some(started.elapsed());
        meta.bytes = facts.bytes;
        meta.network_calls = facts.network_calls;
        if self.gate.approval_required() {
            meta.error_kind = Some(ToolErrorKind::NeedsApproval);
            meta.truncated = facts.bytes.is_some_and(|n| n > 0);
            return ToolOutcome::err("[REDACTED]").with_meta(meta);
        }
        let outcome = match result {
            Ok(view) => {
                meta.truncated = view.truncated;
                ToolOutcome::ok(view.text).with_meta(meta)
            }
            Err(error) => {
                self.gate.observe_error_kind(error.kind);
                meta.error_kind = Some(error.kind);
                meta.truncated =
                    facts.bytes.is_some_and(|n| n > 0) || error.kind == ToolErrorKind::OutputLimit;
                if self.gate.approval_required() {
                    return self
                        .recheck_publication(ToolOutcome::err("[REDACTED]").with_meta(meta));
                }
                let message = match facts.status {
                    Some(status) => format!("{} (HTTP {status})", error.message),
                    None => error.message.to_owned(),
                };
                let mut out = Output::new(&self.redactor);
                match out
                    .frame(&message)
                    .and_then(|complete| out.finish(complete))
                {
                    Ok(view) => {
                        meta.truncated |= view.truncated;
                        ToolOutcome::err(view.text).with_meta(meta)
                    }
                    Err(_) => {
                        meta.truncated = true;
                        ToolOutcome::err("[REDACTED]").with_meta(meta)
                    }
                }
            }
        };
        self.recheck_publication(outcome)
    }
    fn recheck_publication(&self, mut outcome: ToolOutcome) -> ToolOutcome {
        // The SAME latch can seal while success/error framing is assembled on
        // another thread. Recheck at the final boundary, preserving real attempt,
        // original-byte and duration facts rather than returning a stale kind.
        if self.gate.approval_required() {
            outcome.content = "[REDACTED]".into();
            outcome.is_error = true;
            outcome.meta.error_kind = Some(ToolErrorKind::NeedsApproval);
            outcome.meta.truncated |= outcome.meta.bytes.is_some_and(|n| n > 0);
        }
        outcome
    }
    #[cfg(test)]
    pub(crate) fn recheck_publication_for_tests(&self, outcome: ToolOutcome) -> ToolOutcome {
        self.recheck_publication(outcome)
    }
    async fn operation(&self, name: &str, args: &Value, facts: &mut Facts) -> Result<View> {
        match name {
            "web_fetch" => {
                self.fetch(args["url"].as_str().unwrap_or_default(), facts)
                    .await
            }
            "web_search" => {
                self.search(args["query"].as_str().unwrap_or_default(), facts)
                    .await
            }
            _ => Err(Failure::new(
                ToolErrorKind::DeniedCapability,
                "private web capability is unavailable",
            )),
        }
    }
    fn ready(&self, name: &str, args: &Value) -> Result<()> {
        if let Some(outcome) = self.gate.denied(name, args) {
            return Err(Failure::new(
                outcome
                    .meta
                    .error_kind
                    .unwrap_or(ToolErrorKind::DeniedCapability),
                "private web operation was not admitted",
            ));
        }
        Ok(())
    }
    async fn send(
        &self,
        builder: reqwest::RequestBuilder,
        facts: &mut Facts,
    ) -> Result<reqwest::Response> {
        let request = builder.build().map_err(|_| {
            Failure::new(ToolErrorKind::InvalidArgs, "private web request is invalid")
        })?;
        if self.gate.approval_required() {
            return Err(Failure::new(
                ToolErrorKind::NeedsApproval,
                "private web operation was not admitted",
            ));
        }
        let client = self.http.as_ref().ok_or_else(|| {
            Failure::new(ToolErrorKind::Failed, "private web client is unavailable")
        })?;
        // Initiated attempts, not claimed remote receipt/success or token usage.
        facts.network_calls += 1;
        let response = client.execute(request).await.map_err(Failure::http)?;
        facts.status = Some(response.status().as_u16());
        Ok(response)
    }
    async fn fetch(&self, original: &str, facts: &mut Facts) -> Result<View> {
        let mut url = parse_url(original, false)?;
        let http = self.http.as_ref().ok_or_else(|| {
            Failure::new(ToolErrorKind::Failed, "private web client is unavailable")
        })?;
        for hop in 0..=REDIRECTS {
            self.ready("web_fetch", &serde_json::json!({"url":url.as_str()}))?;
            let mut response = self.send(http.get(url.clone()), facts).await?;
            if response.status().is_redirection() {
                if hop == REDIRECTS {
                    return Err(Failure::new(
                        ToolErrorKind::Failed,
                        "private web redirect limit exceeded",
                    ));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .ok_or_else(|| {
                        Failure::new(
                            ToolErrorKind::Failed,
                            "private web redirect is missing a valid Location",
                        )
                    })?;
                let next = url.join(location).map_err(|_| {
                    Failure::new(
                        ToolErrorKind::InvalidArgs,
                        "private web redirect URL is invalid",
                    )
                })?;
                url = parse_url(next.as_str(), true)?;
                continue;
            }
            if !response.status().is_success() {
                return Err(Failure::new(
                    ToolErrorKind::Failed,
                    "private web request returned an unsuccessful status",
                ));
            }
            let html = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.to_ascii_lowercase().contains("html"));
            let mut capture = Capture::new(&self.redactor, html, FETCH_BYTES);
            facts.bytes = Some(0);
            while let Some(chunk) = response.chunk().await.map_err(Failure::http)? {
                let result = capture.push(&chunk);
                facts.bytes = Some(capture.observed());
                if !result? {
                    return capture.finish(false).map_err(Into::into);
                }
                if self.gate.approval_required() {
                    return Err(Failure::new(
                        ToolErrorKind::NeedsApproval,
                        "private web invocation requires approval",
                    ));
                }
            }
            return capture.finish(true).map_err(Into::into);
        }
        Err(Failure::new(
            ToolErrorKind::Failed,
            "private web redirect limit exceeded",
        ))
    }
    async fn search(&self, original: &str, facts: &mut Facts) -> Result<View> {
        let query = original.trim();
        if query.is_empty() || query.len() > 2048 {
            return Err(Failure::new(
                ToolErrorKind::InvalidArgs,
                "private web query is empty or exceeds 2048 bytes",
            ));
        }
        if self.search_provider != "brave" {
            return Err(Failure::new(
                ToolErrorKind::DeniedCapability,
                "private web search provider is unsupported",
            ));
        }
        let key = self.search_key.as_deref().ok_or_else(|| {
            Failure::new(
                ToolErrorKind::InvalidArgs,
                "private web search requires captured provider configuration",
            )
        })?;
        let header = reqwest::header::HeaderValue::from_str(key).map_err(|_| {
            Failure::new(
                ToolErrorKind::InvalidArgs,
                "private web search credential cannot form a valid header",
            )
        })?;
        let endpoint = self.search_endpoint.as_deref().ok_or_else(|| {
            Failure::new(
                ToolErrorKind::InvalidArgs,
                "private test search destination is invalid",
            )
        })?;
        let http = self.http.as_ref().ok_or_else(|| {
            Failure::new(ToolErrorKind::Failed, "private web client is unavailable")
        })?;
        self.ready("web_search", &serde_json::json!({"query":original}))?;
        let mut response = self
            .send(
                http.get(endpoint)
                    .query(&[("q", query)])
                    .header("X-Subscription-Token", header)
                    .header(reqwest::header::ACCEPT, "application/json"),
                facts,
            )
            .await?;
        // Never forward a captured credential to a redirect or echo an untrusted
        // provider error body. Status remains an original private typed fact.
        if !response.status().is_success() {
            return Err(Failure::new(
                ToolErrorKind::Failed,
                "private web search returned an unsuccessful status",
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > SEARCH_BYTES as u64)
        {
            return Err(Failure::new(
                ToolErrorKind::OutputLimit,
                "private web search response exceeds 512 KiB",
            ));
        }
        let mut input = JsonInput::new(&self.redactor, SEARCH_BYTES);
        facts.bytes = Some(0);
        while let Some(chunk) = response.chunk().await.map_err(Failure::http)? {
            let result = input.push(&chunk);
            facts.bytes = Some(input.observed());
            result?;
            if self.gate.approval_required() {
                input.abort();
                return Err(Failure::new(
                    ToolErrorKind::NeedsApproval,
                    "private web invocation requires approval",
                ));
            }
        }
        let original = input.finish()?;
        search::render(&self.redactor, &original).map_err(Into::into)
    }
}
fn parse_url(raw: &str, redirect: bool) -> Result<Url> {
    if raw.is_empty() || raw.len() > 2048 {
        return Err(Failure::new(
            ToolErrorKind::InvalidArgs,
            "private web URL is empty or exceeds 2048 bytes",
        ));
    }
    let url = Url::parse(raw)
        .map_err(|_| Failure::new(ToolErrorKind::InvalidArgs, "private web URL is invalid"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Failure::new(
            ToolErrorKind::DeniedCapability,
            "private web URL scheme or credentials are denied",
        ));
    }
    let host = url
        .host_str()
        .unwrap()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        let blocked = match ip {
            IpAddr::V4(ip) => ip.is_link_local() || (redirect && ip.is_private()),
            IpAddr::V6(ip) => {
                if let Some(mapped) = ip.to_ipv4_mapped() {
                    mapped.is_link_local() || (redirect && mapped.is_private())
                } else {
                    (ip.segments()[0] & 0xffc0) == 0xfe80
                        || (redirect && (ip.segments()[0] & 0xfe00) == 0xfc00)
                }
            }
        };
        if blocked {
            return Err(Failure::new(
                ToolErrorKind::DeniedCapability,
                "private web link-local or redirect destination is denied",
            ));
        }
    }
    Ok(url)
}
#[cfg(test)]
fn test_endpoint(raw: &str) -> Option<Url> {
    let url = parse_url(raw, false).ok()?;
    let host = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']');
    if host == "localhost"
        || host
            .parse::<IpAddr>()
            .ok()
            .is_some_and(|ip| ip.is_loopback())
    {
        Some(url)
    } else {
        None
    }
}
struct ScopedTool {
    def: ToolDef,
    web: Arc<Web>,
}
#[async_trait]
impl Tool for ScopedTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }
    async fn call(
        &self,
        args: Value,
        _: &mut ToolCtx<'_>,
    ) -> std::result::Result<ToolOutcome, AgentError> {
        Ok(self.web.call(&self.def.name, args).await)
    }
}
