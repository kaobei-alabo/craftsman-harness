//! `web_fetch` + `web_search` — the agent's network-egress tools.
//!
//! Both reach the public internet, so both go through the HITL gate (they're in
//! [`crate::tools::NETWORK_TOOLS`], gated under the `auto_approve_network`
//! switch — rule #6). This module is the *tool* layer; the gating decision
//! lives in [`crate::hitl`].
//!
//! - [`WebFetchTool`]: a redirect-safe (max 5) `GET` with a 15s timeout and a
//!   200KB body cap. HTML is run through a small hand-rolled tag/script
//!   stripper (NO new heavy deps); other text is returned as-is (lossy UTF-8).
//!   `http://` is upgraded to `https://` except for loopback hosts (so local
//!   dev servers and tests still work).
//! - [`WebSearchTool`]: a thin wrapper over a [`SearchProvider`]. The only
//!   provider today is [`BraveSearch`] (API key from config
//!   `extra["search_api_key"]`, provider name `extra["search_provider"]`,
//!   default `"brave"`). With no key configured the tool returns an honest,
//!   non-silent "requires setup" message rather than pretending to have no
//!   results.
//!
//! The shared service-agent path uses a stricter public-HTTPS fetch policy and
//! a keyless DuckDuckGo provider fallback; interactive sessions retain their
//! existing approval-gated behavior.

use async_trait::async_trait;
use craftsman_harness::{AgentError, Tool, ToolCtx, ToolDef, ToolOutcome};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// Max response body we keep from a fetch (bytes). Anything larger is truncated
/// at a char boundary with an explicit marker.
const MAX_FETCH_BYTES: usize = 200_000;
/// Per-fetch timeout — a slow/hung host must not stall the turn.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// Redirect cap (reqwest's `limited` policy).
const MAX_REDIRECTS: usize = 5;
/// Default Brave web-search endpoint. Overridable in tests.
const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const DUCKDUCKGO_ENDPOINT: &str = "https://api.duckduckgo.com/";
const MAX_SEARCH_BYTES: usize = 512 * 1024;

// ---------------------------------------------------------------------------
// web_fetch
// ---------------------------------------------------------------------------

/// Upgrade an `http://` URL to `https://`, EXCEPT for loopback hosts
/// (`localhost`, `127.0.0.1`, `[::1]`) — forcing TLS on a local dev server (or
/// a test mock) would just break it. Non-`http://` URLs pass through unchanged.
fn upgrade_scheme(url: &str) -> String {
    let Some(rest) = url.strip_prefix("http://") else {
        return url.to_string();
    };
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
    if matches!(host, "localhost" | "127.0.0.1" | "::1") || host.starts_with("[::1]") {
        return url.to_string();
    }
    format!("https://{rest}")
}

/// Is `host` a metadata/link-local/private address that a redirect must never
/// reach? An approved PUBLIC URL can 3xx-bounce anywhere, so redirect targets
/// are re-checked hop by hop: cloud metadata (169.254.x, notably
/// 169.254.169.254) and RFC-1918 ranges are blocked. Loopback is allowed —
/// the user can approve a fetch of their own local dev server, and the
/// initial-URL path keeps working (tests use 127.0.0.1 mocks).
fn is_blocked_redirect_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_link_local() || v4.is_private(),
            std::net::IpAddr::V6(v6) => {
                // fe80::/10 link-local; fc00::/7 unique-local.
                (v6.segments()[0] & 0xffc0) == 0xfe80 || (v6.segments()[0] & 0xfe00) == 0xfc00
            }
        };
    }
    false
}

/// Truncate `s` to at most `MAX_FETCH_BYTES` at a char boundary, appending a
/// marker when it had to cut.
fn cap_body(mut s: String) -> String {
    if s.len() <= MAX_FETCH_BYTES {
        return s;
    }
    let mut cut = MAX_FETCH_BYTES;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str("\n… [content truncated at 200KB]");
    s
}

/// Strip HTML to rough plain text: drop `<script>`/`<style>` bodies, remove all
/// remaining tags, decode a handful of common entities, and collapse runs of
/// blank lines. Deliberately crude — no HTML parser dep (rule: no new heavy
/// deps) — but enough to hand a model readable page text.
fn html_to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        if html.as_bytes()[i] != b'<' {
            let ch = html[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        // A tag starts here. First check for <script>/<style> to drop wholesale.
        let mut skipped = false;
        for (open, close) in [("<script", "</script>"), ("<style", "</style>")] {
            if lower[i..].starts_with(open) {
                i = match lower[i..].find(close) {
                    Some(end) => i + end + close.len(),
                    None => html.len(),
                };
                skipped = true;
                break;
            }
        }
        if skipped {
            continue;
        }
        // A normal tag: drop it, but map block-level closers/breaks to newlines
        // so adjacent text blocks don't run together.
        match html[i..].find('>') {
            Some(end) => {
                let tag = &lower[i..i + end];
                if tag.starts_with("</p")
                    || tag.starts_with("<br")
                    || tag.starts_with("</div")
                    || tag.starts_with("</li")
                    || tag.starts_with("</h")
                    || tag.starts_with("</tr")
                {
                    out.push('\n');
                }
                i += end + 1;
            }
            None => i = html.len(),
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    collapse_blank_lines(&decoded)
}

/// Collapse runs of 3+ newlines to 2 and trim trailing spaces per line.
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            blank += 1;
            if blank <= 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

/// Whether `ip` is safe for the shared service-agent's public-web boundary.
/// Private, loopback, link-local, documentation, multicast, benchmark, and
/// otherwise reserved ranges are rejected before a socket is opened.
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, d] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || (a == 255 && b == 255 && c == 255 && d == 255))
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_ipv4(v4);
    }
    let segments = ip.segments();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

fn blocked_public_hostname(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".svc")
        || host.ends_with(".cluster.local")
}

fn parse_public_https_url(raw: &str) -> Result<reqwest::Url, String> {
    if raw.len() > 2_048 {
        return Err("Error: URL exceeds 2048 bytes".into());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| "Error: URL is invalid".to_string())?;
    if url.scheme() != "https" {
        return Err("Error: service-agent web_fetch requires an https URL".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Error: URL credentials are not allowed".into());
    }
    if url.port_or_known_default() != Some(443) {
        return Err("Error: service-agent web_fetch allows only HTTPS port 443".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "Error: URL host is required".to_string())?;
    if blocked_public_hostname(host)
        || host
            .parse::<IpAddr>()
            .ok()
            .is_some_and(|ip| !is_public_ip(ip))
    {
        return Err("Error: private or reserved web destinations are not allowed".into());
    }
    Ok(url)
}

async fn public_client_for(url: &reqwest::Url) -> Result<reqwest::Client, String> {
    let host = url
        .host_str()
        .ok_or_else(|| "Error: URL host is required".to_string())?;
    let port = url.port_or_known_default().unwrap_or(443);
    let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| "Error: public destination DNS lookup failed".to_string())?
        .collect();
    addrs.sort_unstable();
    addrs.dedup();
    if addrs.is_empty() || addrs.iter().any(|addr| !is_public_ip(addr.ip())) {
        return Err("Error: destination resolves to a private or reserved address".into());
    }
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(FETCH_TIMEOUT)
        .user_agent("craftsman-service-agent/1")
        .no_proxy()
        .resolve_to_addrs(host, &addrs)
        .build()
        .map_err(|_| "Error: could not initialize the public web client".to_string())
}

async fn response_text(resp: reqwest::Response, url: &str) -> String {
    let status = resp.status();
    if !status.is_success() {
        return format!("Error: HTTP {status} fetching {url}");
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => {
                let remaining = MAX_FETCH_BYTES.saturating_sub(buf.len());
                if chunk.len() > remaining {
                    truncated = true;
                }
                buf.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if buf.len() >= MAX_FETCH_BYTES {
                    truncated = true;
                    break;
                }
            }
            Err(error) => return format!("Error reading body from {url}: {error}"),
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let mut body = if ctype.contains("html") {
        html_to_text(&text)
    } else {
        text
    };
    if truncated {
        body.push_str(
            "
… [content truncated at 200KB]",
        );
    }
    cap_body(body)
}

/// The `web_fetch` tool: a bounded, redirect-safe HTTP GET.
pub struct WebFetchTool {
    http: Option<reqwest::Client>,
    public_only: bool,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    pub fn new() -> Self {
        // Interactive use retains loopback access for explicit local testing.
        let policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error("too many redirects");
            }
            let host = attempt.url().host_str().unwrap_or("");
            if is_blocked_redirect_host(host) {
                return attempt.error("redirect to a private/link-local address blocked");
            }
            attempt.follow()
        });
        let http = reqwest::Client::builder()
            .redirect(policy)
            .timeout(FETCH_TIMEOUT)
            .user_agent("craftsman/0.1 (+https://github.com/trend-mgai)")
            .build()
            .unwrap_or_default();
        Self {
            http: Some(http),
            public_only: false,
        }
    }

    /// Shared service-agent mode: HTTPS-only, port 443 only, no local/private
    /// hostnames or IPs, DNS resolved and pinned before every redirect hop.
    pub fn public_only() -> Self {
        Self {
            http: None,
            public_only: true,
        }
    }

    async fn fetch_public(&self, raw_url: &str) -> String {
        let mut url = match parse_public_https_url(raw_url) {
            Ok(url) => url,
            Err(error) => return error,
        };
        for hop in 0..=MAX_REDIRECTS {
            let client = match public_client_for(&url).await {
                Ok(client) => client,
                Err(error) => return error,
            };
            let response = match client.get(url.clone()).send().await {
                Ok(response) => response,
                Err(error) => return format!("Error fetching {url}: {error}"),
            };
            if response.status().is_redirection() {
                if hop == MAX_REDIRECTS {
                    return "Error: too many redirects".into();
                }
                let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                else {
                    return "Error: redirect response is missing Location".into();
                };
                url = match url
                    .join(location)
                    .ok()
                    .and_then(|next| parse_public_https_url(next.as_str()).ok())
                {
                    Some(next) => next,
                    None => {
                        return "Error: redirect target is not an allowed public HTTPS URL".into();
                    }
                };
                continue;
            }
            return response_text(response, url.as_str()).await;
        }
        "Error: too many redirects".into()
    }

    /// Fetch `url` and return page text (HTML stripped, body capped). Errors
    /// come back as the tool RESULT string (the model reads and adjusts) — this
    /// never fails the turn.
    async fn fetch(&self, url: &str) -> String {
        if self.public_only {
            return self.fetch_public(url).await;
        }
        let url = upgrade_scheme(url);
        if let Ok(parsed) = reqwest::Url::parse(&url) {
            let host = parsed.host_str().unwrap_or("");
            let bare = host.trim_start_matches('[').trim_end_matches(']');
            if let Ok(ip) = bare.parse::<IpAddr>() {
                let link_local = match ip {
                    IpAddr::V4(v4) => v4.is_link_local(),
                    IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
                };
                if link_local {
                    return format!(
                        "Error: refusing to fetch link-local address {host} (metadata endpoint)"
                    );
                }
            }
        }
        let Some(http) = &self.http else {
            return "Error: web client is unavailable".into();
        };
        let response = match http.get(&url).send().await {
            Ok(response) => response,
            Err(error) => return format!("Error fetching {url}: {error}"),
        };
        response_text(response, &url).await
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "web_fetch".into(),
            description: "Fetch a URL over HTTP(S) and return its text content. \
                 HTML is reduced to readable text; other text types are returned as-is. \
                 Follows up to 5 redirects, 15s timeout, 200KB cap."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The URL to fetch."}
                },
                "required": ["url"]
            }),
        }
    }

    async fn call(&self, args: Value, _ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let url = args["url"].as_str().unwrap_or_default();
        if url.is_empty() {
            return Ok(ToolOutcome::err("Error: 'url' is required"));
        }
        Ok(ToolOutcome::ok(self.fetch(url).await))
    }
}

// ---------------------------------------------------------------------------
// web_search
// ---------------------------------------------------------------------------

/// A pluggable web-search backend. Config selects the concrete provider by name
/// (`extra["search_provider"]`, default `"brave"`).
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Run `query`, returning already-formatted result text (never fails the
    /// turn — errors come back as the result string).
    async fn search(&self, query: &str) -> String;
}

/// Format a Brave `web/search` JSON response into ranked plain-text results.
/// Kept pure so it can be unit-tested without the network.
fn format_brave_results(v: &Value) -> String {
    let results = v["web"]["results"].as_array();
    let Some(results) = results.filter(|r| !r.is_empty()) else {
        return "No results.".to_string();
    };
    let mut out = String::new();
    for (i, r) in results.iter().enumerate().take(10) {
        let title = r["title"].as_str().unwrap_or("(untitled)");
        let url = r["url"].as_str().unwrap_or("");
        let desc = r["description"].as_str().unwrap_or("");
        out.push_str(&format!("{}. {title}\n   {url}\n", i + 1));
        if !desc.is_empty() {
            out.push_str(&format!("   {}\n", crate::util::one_line(desc, 300)));
        }
    }
    out.trim_end().to_string()
}

/// Brave Search provider. Reads use the `X-Subscription-Token` header.
pub struct BraveSearch {
    api_key: String,
    http: reqwest::Client,
    endpoint: String,
}

impl BraveSearch {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            http: reqwest::Client::builder()
                .timeout(FETCH_TIMEOUT)
                .build()
                .unwrap_or_default(),
            endpoint: BRAVE_ENDPOINT.to_string(),
        }
    }

    /// Point the provider at a different endpoint (tests → mock server).
    #[cfg(test)]
    fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

#[async_trait]
impl SearchProvider for BraveSearch {
    async fn search(&self, query: &str) -> String {
        let resp = self
            .http
            .get(&self.endpoint)
            .query(&[("q", query)])
            .header("X-Subscription-Token", &self.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => return format!("Error contacting search provider: {e}"),
        };
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return format!(
                "Error: search provider returned HTTP {status}: {}",
                crate::util::one_line(&body, 200)
            );
        }
        match resp.json::<Value>().await {
            Ok(v) => format_brave_results(&v),
            Err(e) => format!("Error parsing search response: {e}"),
        }
    }
}

fn collect_duckduckgo_topics(items: &[Value], out: &mut Vec<(String, String)>) {
    for item in items {
        if out.len() >= 10 {
            return;
        }
        if let (Some(text), Some(url)) = (item["Text"].as_str(), item["FirstURL"].as_str()) {
            if !text.trim().is_empty() && !url.trim().is_empty() {
                out.push((text.trim().to_string(), url.trim().to_string()));
            }
        }
        if let Some(topics) = item["Topics"].as_array() {
            collect_duckduckgo_topics(topics, out);
        }
    }
}

fn format_duckduckgo_results(value: &Value) -> String {
    let mut results = Vec::new();
    let abstract_text = value["AbstractText"].as_str().unwrap_or_default().trim();
    let abstract_url = value["AbstractURL"].as_str().unwrap_or_default().trim();
    if !abstract_text.is_empty() && !abstract_url.is_empty() {
        results.push((abstract_text.to_string(), abstract_url.to_string()));
    }
    if let Some(items) = value["Results"].as_array() {
        collect_duckduckgo_topics(items, &mut results);
    }
    if let Some(items) = value["RelatedTopics"].as_array() {
        collect_duckduckgo_topics(items, &mut results);
    }
    results.truncate(10);
    if results.is_empty() {
        return "No results.".into();
    }
    results
        .into_iter()
        .enumerate()
        .map(|(index, (text, url))| {
            format!(
                "{}. {}\n   {}",
                index + 1,
                crate::util::one_line(&text, 300),
                url
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keyless public search provider used by the isolated service-agent worker.
pub struct DuckDuckGoSearch {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for DuckDuckGoSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl DuckDuckGoSearch {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
                .timeout(FETCH_TIMEOUT)
                .user_agent("craftsman-service-agent/1")
                .build()
                .unwrap_or_default(),
            endpoint: DUCKDUCKGO_ENDPOINT.to_string(),
        }
    }

    #[cfg(test)]
    fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

#[async_trait]
impl SearchProvider for DuckDuckGoSearch {
    async fn search(&self, query: &str) -> String {
        let response = match self
            .http
            .get(&self.endpoint)
            .query(&[
                ("q", query),
                ("format", "json"),
                ("no_html", "1"),
                ("skip_disambig", "1"),
            ])
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => return format!("Error contacting search provider: {error}"),
        };
        let status = response.status();
        if !status.is_success() {
            return format!("Error: search provider returned HTTP {status}");
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_SEARCH_BYTES as u64)
        {
            return "Error: search provider response is too large".into();
        }
        let bytes = match response.bytes().await {
            Ok(bytes) if bytes.len() <= MAX_SEARCH_BYTES => bytes,
            Ok(_) => return "Error: search provider response is too large".into(),
            Err(error) => return format!("Error reading search response: {error}"),
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => format_duckduckgo_results(&value),
            Err(error) => format!("Error parsing search response: {error}"),
        }
    }
}

/// The `web_search` tool. Holds an optional provider — `None` when no API key is
/// configured, in which case the tool returns an honest setup message.
pub struct WebSearchTool {
    provider: Option<Arc<dyn SearchProvider>>,
}

/// Shown when `web_search` is invoked without a configured provider — present
/// and honest (rule #6 spirit: never silently absent).
const SETUP_MESSAGE: &str =
    "web_search requires setup: set search_api_key in ~/.craftsman/config.json";

impl WebSearchTool {
    /// Build from config: constructs the named provider if a key is present,
    /// otherwise a tool that reports the setup message.
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        // Only Brave today; `search_provider` is read for forward-compat but an
        // unknown name still takes the Brave path so a typo degrades to a
        // visible HTTP error, not a silent no-op.
        let _ = cfg.search_provider();
        let provider: Option<Arc<dyn SearchProvider>> = cfg
            .search_api_key()
            .map(|key| Arc::new(BraveSearch::new(key)) as Arc<dyn SearchProvider>);
        Self { provider }
    }

    /// Build the shared service-agent provider. It uses the configured Brave
    /// provider when a key exists and otherwise falls back to DuckDuckGo's
    /// keyless public instant-answer endpoint.
    pub fn public_default() -> Self {
        Self {
            provider: Some(Arc::new(DuckDuckGoSearch::new())),
        }
    }

    /// Construct directly from a provider (tests).
    #[cfg(test)]
    fn with_provider(provider: Option<Arc<dyn SearchProvider>>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "web_search".into(),
            description: "Search the web and return ranked result titles, URLs, and snippets."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "The search query."}
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Value, _ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let query = args["query"].as_str().unwrap_or_default().trim();
        if query.is_empty() {
            return Ok(ToolOutcome::err("Error: 'query' is required"));
        }
        if query.len() > 2_048 {
            return Ok(ToolOutcome::err("Error: query exceeds 2048 bytes"));
        }
        match &self.provider {
            Some(p) => Ok(ToolOutcome::ok(p.search(query).await)),
            None => Ok(ToolOutcome::ok(SETUP_MESSAGE)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // A ToolCtx is a pain to build; the tool `call` methods only touch args, so
    // exercise the async cores (`fetch`, `search`) and the pure helpers directly.

    #[test]
    fn upgrade_scheme_forces_https_except_loopback() {
        assert_eq!(
            upgrade_scheme("http://example.com/x"),
            "https://example.com/x"
        );
        assert_eq!(upgrade_scheme("https://example.com"), "https://example.com");
        // Loopback stays http so local servers / mocks work.
        assert_eq!(
            upgrade_scheme("http://127.0.0.1:8080/x"),
            "http://127.0.0.1:8080/x"
        );
        assert_eq!(upgrade_scheme("http://localhost/x"), "http://localhost/x");
        // Non-http scheme untouched.
        assert_eq!(upgrade_scheme("ftp://x"), "ftp://x");
    }

    #[test]
    fn blocked_redirect_hosts_cover_metadata_and_private_ranges() {
        // Cloud metadata + link-local: always blocked on redirect hops.
        assert!(is_blocked_redirect_host("169.254.169.254"));
        assert!(is_blocked_redirect_host("169.254.0.1"));
        // RFC-1918 private ranges: blocked on redirect hops.
        assert!(is_blocked_redirect_host("10.0.0.1"));
        assert!(is_blocked_redirect_host("172.16.5.5"));
        assert!(is_blocked_redirect_host("192.168.1.1"));
        // IPv6 link-local / unique-local.
        assert!(is_blocked_redirect_host("fe80::1"));
        assert!(is_blocked_redirect_host("[fe80::1]"));
        assert!(is_blocked_redirect_host("fd00::1"));
        // Public addresses and hostnames pass.
        assert!(!is_blocked_redirect_host("93.184.216.34"));
        assert!(!is_blocked_redirect_host("example.com"));
        // Loopback intentionally allowed (local dev servers, test mocks).
        assert!(!is_blocked_redirect_host("127.0.0.1"));
        assert!(!is_blocked_redirect_host("::1"));
    }

    #[test]
    fn public_fetch_policy_rejects_non_public_destinations() {
        assert!(parse_public_https_url("https://example.com/docs").is_ok());
        assert!(parse_public_https_url("http://example.com/docs").is_err());
        assert!(parse_public_https_url("https://127.0.0.1/").is_err());
        assert!(parse_public_https_url("https://10.0.0.1/").is_err());
        assert!(parse_public_https_url("https://service.craftsman.svc/").is_err());
        assert!(parse_public_https_url("https://example.com:8443/").is_err());
    }

    #[tokio::test]
    async fn public_fetch_rejects_loopback_before_network_access() {
        let tool = WebFetchTool::public_only();
        let out = tool.fetch("https://127.0.0.1/").await;
        assert!(out.contains("private or reserved"), "got: {out}");
    }

    #[tokio::test]
    async fn fetch_refuses_link_local_initial_target() {
        let tool = WebFetchTool::new();
        let out = tool
            .fetch("https://169.254.169.254/latest/meta-data/")
            .await;
        assert!(
            out.contains("refusing to fetch link-local"),
            "metadata endpoint must be refused, got: {out}"
        );
    }

    #[tokio::test]
    async fn fetch_blocks_redirect_to_private_address() {
        // A public-looking (loopback mock) URL 302-bounces to a private
        // address; the redirect policy must refuse to follow it.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bounce"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "http://192.168.1.1/admin"),
            )
            .mount(&server)
            .await;
        let tool = WebFetchTool::new();
        let out = tool.fetch(&format!("{}/bounce", server.uri())).await;
        // reqwest surfaces a refused redirect as "error following redirect"
        // (our policy error is wrapped); the essential property is that the
        // fetch FAILS rather than following into the private address.
        assert!(
            out.contains("Error") && out.contains("redirect"),
            "redirect into RFC-1918 must be refused, got: {out}"
        );
    }

    #[test]
    fn html_to_text_strips_tags_scripts_and_styles() {
        let html = "<html><head><style>body{color:red}</style>\
            <script>alert('x')</script></head>\
            <body><h1>Title</h1><p>Hello &amp; welcome</p>\
            <p>Second&nbsp;line</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Title"), "got: {text}");
        assert!(text.contains("Hello & welcome"), "got: {text}");
        assert!(text.contains("Second line"), "got: {text}");
        assert!(!text.contains("alert"), "script body leaked: {text}");
        assert!(!text.contains("color:red"), "style body leaked: {text}");
        assert!(!text.contains('<'), "tag leaked: {text}");
    }

    #[test]
    fn cap_body_truncates_at_char_boundary() {
        let big = "é".repeat(MAX_FETCH_BYTES); // 2 bytes each → over the cap
        let out = cap_body(big);
        assert!(out.len() <= MAX_FETCH_BYTES + 40);
        assert!(out.contains("truncated"));
        // Small bodies pass through untouched.
        assert_eq!(cap_body("short".into()), "short");
    }

    #[tokio::test]
    async fn web_fetch_returns_extracted_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string("<h1>Docs</h1><p>content here</p>"),
            )
            .mount(&server)
            .await;
        let tool = WebFetchTool::new();
        // Loopback URL is NOT https-upgraded, so the mock is reachable.
        let out = tool.fetch(&format!("{}/page", server.uri())).await;
        assert!(out.contains("Docs"), "got: {out}");
        assert!(out.contains("content here"), "got: {out}");
    }

    #[tokio::test]
    async fn web_fetch_reports_http_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let tool = WebFetchTool::new();
        let out = tool.fetch(&format!("{}/missing", server.uri())).await;
        assert!(out.contains("404"), "got: {out}");
    }

    #[test]
    fn brave_results_format_ranked() {
        let v = json!({
            "web": {"results": [
                {"title": "First", "url": "https://a", "description": "desc a"},
                {"title": "Second", "url": "https://b", "description": ""}
            ]}
        });
        let out = format_brave_results(&v);
        assert!(out.contains("1. First"));
        assert!(out.contains("https://a"));
        assert!(out.contains("desc a"));
        assert!(out.contains("2. Second"));
        // Empty results → explicit message.
        assert_eq!(
            format_brave_results(&json!({"web": {"results": []}})),
            "No results."
        );
        assert_eq!(format_brave_results(&json!({})), "No results.");
    }

    #[test]
    fn duckduckgo_results_include_abstract_and_nested_topics() {
        let value = json!({
            "AbstractText": "Craftsman overview",
            "AbstractURL": "https://example.com/overview",
            "RelatedTopics": [
                {"Text": "First result", "FirstURL": "https://example.com/one"},
                {"Topics": [
                    {"Text": "Nested result", "FirstURL": "https://example.com/two"}
                ]}
            ]
        });
        let out = format_duckduckgo_results(&value);
        assert!(out.contains("1. Craftsman overview"), "got: {out}");
        assert!(out.contains("2. First result"), "got: {out}");
        assert!(out.contains("3. Nested result"), "got: {out}");
    }

    #[tokio::test]
    async fn duckduckgo_search_uses_the_keyless_json_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("q", "craftsman runtime"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "RelatedTopics": [
                    {"Text": "Runtime result", "FirstURL": "https://example.com/runtime"}
                ]
            })))
            .mount(&server)
            .await;
        let provider = DuckDuckGoSearch::new().with_endpoint(server.uri());
        let out = provider.search("craftsman runtime").await;
        assert!(out.contains("Runtime result"), "got: {out}");
        assert!(out.contains("https://example.com/runtime"), "got: {out}");
    }

    #[tokio::test]
    async fn brave_search_hits_provider_with_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/res/v1/web/search"))
            .and(query_param("q", "rust async"))
            .and(header("X-Subscription-Token", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "web": {"results": [{"title": "Async Rust", "url": "https://rust", "description": "book"}]}
            })))
            .mount(&server)
            .await;
        let brave = BraveSearch::new("test-key")
            .with_endpoint(format!("{}/res/v1/web/search", server.uri()));
        let out = brave.search("rust async").await;
        assert!(out.contains("Async Rust"), "got: {out}");
        assert!(out.contains("https://rust"), "got: {out}");
    }

    // Building a `ToolCtx` needs owned state/registry/notifications and a
    // borrow, which is noisy; the `call` methods only read `args`, so drive the
    // provider path directly and cover `call`'s ok/err framing via `call_query`.
    async fn call_query(tool: &WebSearchTool, query: &str) -> ToolOutcome {
        use craftsman_harness::{
            AgentRegistry, AgentState, Backend, StateFs, ToolCtx, notification_channel,
        };
        use std::sync::Mutex;
        let mut state = AgentState::default();
        let backend: Arc<dyn Backend> =
            Arc::new(StateFs::new(Arc::new(Mutex::new(AgentState::default()))));
        let registry = Arc::new(AgentRegistry::default());
        let (notifications, _rx) = notification_channel();
        let mut ctx = ToolCtx {
            state: &mut state,
            backend,
            registry,
            notifications,
        };
        tool.call(json!({ "query": query }), &mut ctx)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn web_search_without_key_returns_setup_message() {
        // No key configured → no provider → the tool `call` yields the honest
        // setup message (not silent, not empty).
        let cfg = crate::config::Config::default();
        let tool = WebSearchTool::from_config(&cfg);
        assert!(tool.provider.is_none());
        let out = call_query(&tool, "anything").await;
        assert_eq!(out.content, SETUP_MESSAGE);
    }

    #[tokio::test]
    async fn service_agent_search_always_has_a_provider() {
        let tool = WebSearchTool::public_default();
        assert!(tool.provider.is_some());
    }

    #[tokio::test]
    async fn web_search_with_provider_returns_results() {
        struct Stub;
        #[async_trait]
        impl SearchProvider for Stub {
            async fn search(&self, query: &str) -> String {
                format!("results for {query}")
            }
        }
        let tool = WebSearchTool::with_provider(Some(Arc::new(Stub)));
        let out = call_query(&tool, "q").await;
        assert_eq!(out.content, "results for q");
    }

    #[test]
    fn from_config_builds_provider_when_key_present() {
        let mut cfg = crate::config::Config::default();
        cfg.extra.insert("search_api_key".into(), json!("bsk-123"));
        let tool = WebSearchTool::from_config(&cfg);
        assert!(tool.provider.is_some());
    }
}
