//! Strict private HTTP facet: declaration AND invocation permission, bounded IO,
//! fixed errors and no ambient proxy/redirect/cookie/credential configuration.
use super::{
    ApprovedSource, CallBudget, ErrorKind, GadgetError, InvocationGate, Phase, ensure, error,
};
use super::{Request, Response};
use crate::exec::permissions::{InvocationPermissions, Mode, ToolDecision};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

const MAX_HTTP_BYTES: usize = 10 * 1024 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_REQUESTS: u64 = 64;

pub(super) fn client() -> Result<reqwest::Client, GadgetError> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| {
            error(
                ErrorKind::Io,
                "private guest HTTPS client construction failed",
            )
        })
}
#[cfg(test)]
type Reply = Arc<dyn Fn(&Request) -> Result<Response, GadgetError> + Send + Sync>;
pub(super) struct HttpFacet {
    client: reqwest::Client,
    hosts: BTreeSet<String>,
    permissions: Arc<InvocationPermissions>,
    gate: Arc<InvocationGate>,
    pub(super) network_calls: u64,
    #[cfg(test)]
    pub(super) reply: Option<Reply>,
}
impl HttpFacet {
    pub(super) fn from_source(
        source: &ApprovedSource,
        phase: Phase,
        gate: Arc<InvocationGate>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            client,
            hosts: if phase == Phase::Call {
                source.manifest().capabilities.http.clone()
            } else {
                BTreeSet::new()
            },
            permissions: source.permissions().clone(),
            gate,
            network_calls: 0,
            #[cfg(test)]
            reply: None,
        }
    }
    pub(super) async fn request(
        &mut self,
        budget: &Arc<CallBudget>,
        request: Request,
    ) -> Result<Response, GadgetError> {
        ensure(&self.gate, Some(budget))?;
        if request.url.len() > 16 * 1024
            || request.method.len() > 32
            || request.headers.len() > MAX_HEADERS
            || header_bytes(&request.headers) > MAX_HEADER_BYTES
            || request
                .body
                .as_ref()
                .is_some_and(|b| b.len() > craftsman_gadget::MAX_JSON_BYTES)
        {
            return Err(error(
                ErrorKind::OutputLimit,
                "private guest HTTPS request exceeds its bounds",
            ));
        }
        let url = url::Url::parse(&request.url)
            .map_err(|_| error(ErrorKind::InvalidArgs, "private guest HTTPS URL is invalid"))?;
        if url.scheme() != "https"
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || !url.host_str().is_some_and(|host| self.hosts.contains(host))
        {
            return Err(error(
                ErrorKind::DeniedCapability,
                "private guest HTTPS destination was not declared",
            ));
        }
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|_| {
            error(
                ErrorKind::InvalidArgs,
                "private guest HTTPS method is invalid",
            )
        })?;
        // Plan cannot admit network mutation. Safe methods remain explicit network
        // operations, not a promise of remote-server side-effect freedom.
        if self.permissions.mode() == Mode::Plan
            && method != reqwest::Method::GET
            && method != reqwest::Method::HEAD
        {
            return Err(error(
                ErrorKind::DeniedCapability,
                "private guest HTTPS mutation is denied in plan",
            ));
        }
        match self
            .permissions
            .tool_decision("web_fetch", &json!({"url":request.url}))
        {
            ToolDecision::Allow => {}
            ToolDecision::Deny => {
                return Err(error(
                    ErrorKind::DeniedCapability,
                    "private guest network tool is denied",
                ));
            }
            ToolDecision::Unapproved => {
                return Err(error(
                    ErrorKind::NeedsApproval,
                    "private guest network requires an invocation allow",
                ));
            }
        }
        let mut builder = self
            .client
            .request(method, url)
            .timeout(budget.remaining().min(Duration::from_secs(30)));
        for (name, value) in &request.headers {
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "host"
                    | "connection"
                    | "proxy-authorization"
                    | "transfer-encoding"
                    | "content-length"
            ) {
                return Err(error(
                    ErrorKind::DeniedCapability,
                    "private guest routing or proxy headers are denied",
                ));
            }
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                error(
                    ErrorKind::InvalidArgs,
                    "private guest header name is invalid",
                )
            })?;
            let value = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
                error(
                    ErrorKind::InvalidArgs,
                    "private guest header value is invalid",
                )
            })?;
            builder = builder.header(name, value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        if self.network_calls >= MAX_REQUESTS {
            return Err(error(
                ErrorKind::OutputLimit,
                "private guest HTTPS request count exceeded",
            ));
        }
        ensure(&self.gate, Some(budget))?;
        // Count initiated attempts, not successful replies/remote receipt.
        self.network_calls += 1;
        #[cfg(test)]
        if let Some(reply) = &self.reply {
            return validate_response(reply(&request)?);
        }
        let mut response = builder
            .send()
            .await
            .map_err(|e| http_error(e, "private guest HTTPS request failed"))?;
        if response
            .content_length()
            .is_some_and(|len| len > MAX_HTTP_BYTES as u64)
        {
            return Err(error(
                ErrorKind::OutputLimit,
                "private guest HTTPS response exceeds 10 MiB",
            ));
        }
        let status = response.status().as_u16();
        let mut headers = Vec::new();
        let mut size = 0usize;
        for (key, value) in response.headers() {
            let value = value.to_str().map_err(|_| {
                error(
                    ErrorKind::Failed,
                    "private guest response header is not text",
                )
            })?;
            size = size
                .saturating_add(key.as_str().len())
                .saturating_add(value.len());
            if headers.len() >= MAX_HEADERS || size > MAX_HEADER_BYTES {
                return Err(error(
                    ErrorKind::OutputLimit,
                    "private guest response headers exceed their bounds",
                ));
            }
            headers.push((key.to_string(), value.to_owned()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| http_error(e, "private guest HTTPS response read failed"))?
        {
            ensure(&self.gate, Some(budget))?;
            if chunk.len() > MAX_HTTP_BYTES.saturating_sub(body.len()) {
                return Err(error(
                    ErrorKind::OutputLimit,
                    "private guest HTTPS response exceeds 10 MiB",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        validate_response(Response {
            status,
            headers,
            body,
        })
    }
}
fn header_bytes(headers: &[(String, String)]) -> usize {
    headers.iter().fold(0usize, |sum, (k, v)| {
        sum.saturating_add(k.len()).saturating_add(v.len())
    })
}
fn validate_response(response: Response) -> Result<Response, GadgetError> {
    if response.body.len() > MAX_HTTP_BYTES
        || response.headers.len() > MAX_HEADERS
        || header_bytes(&response.headers) > MAX_HEADER_BYTES
    {
        return Err(error(
            ErrorKind::OutputLimit,
            "private guest HTTPS response exceeds its bounds",
        ));
    }
    Ok(response)
}
fn http_error(e: reqwest::Error, message: &'static str) -> GadgetError {
    error(
        if e.is_timeout() {
            ErrorKind::Timeout
        } else {
            ErrorKind::Failed
        },
        message,
    )
}
