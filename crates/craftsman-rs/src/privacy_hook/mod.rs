//! Private detector contract: the request the host sends to an approved
//! detector, and the checks it applies to whatever comes back.
//!
//! Nothing selects this module. No session, frontend, configuration key,
//! command, manifest or model starts a detector through it. A reply is
//! untrusted data: it is bound to the exact request, bounded, and every span
//! is checked against the original text before it is used. A guest's success
//! text, logs or diagnostics are never read as authority.
//!
//! The host cannot see everything an approved detector fails to report. For
//! the two supported categories it applies a narrow floor of its own (see
//! `floor`) and refuses a reply that leaves a floor match unreported. That is
//! a check on the reply, not a second detector and not a coverage claim.

mod floor;
#[cfg(feature = "gadgets")]
mod pinned;
#[cfg(test)]
pub(crate) mod tests;

#[cfg(feature = "gadgets")]
pub use pinned::{DetectorPin, PinnedDetector};

use crate::privacy_vault::Category;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_NODES: usize = 64;
pub const MAX_NODE_BYTES: usize = 64 * 1024;
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_SPANS: usize = 128;
pub const MAX_REPLY_BYTES: usize = 64 * 1024;

/// Why a reply was not used. Fixed, without reply bytes, offsets or values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReplyRefusal {
    #[error("The detector reply exceeds its size bound.")]
    Output,
    #[error("The detector reply is not the expected document.")]
    Shape,
    #[error("The detector reply is not bound to this request.")]
    Binding,
    #[error("The detector reply reports too many spans.")]
    Count,
    #[error("The detector reply names an unknown text node.")]
    Node,
    #[error("The detector reply reports an invalid span range.")]
    Offset,
    #[error("The detector reply reports spans out of order or overlapping.")]
    Order,
    #[error("The detector reply reports an unsupported category or value.")]
    Category,
    #[error("The detector reply leaves a supported value unreported.")]
    Coverage,
}

/// Why no usable detector result exists for one text. Fixed diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HookError {
    /// The text cannot be sent as one bounded request.
    #[error("The text exceeds the detector request bounds.")]
    Input,
    /// The selected source is missing, changed, shadowed, unapproved or
    /// declares a capability. The detector cannot be used at all.
    #[error("The selected detector source is unavailable or not approved.")]
    Source,
    /// The approved detector did not return: trap, timeout, memory or output
    /// limit, or its own failure.
    #[error("The detector did not return a result.")]
    Execution,
    #[error("The detector reply was refused.")]
    Reply(ReplyRefusal),
}

/// One reported value, in byte offsets of the whole original text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
    pub category: Category,
}

struct Node<'a> {
    id: String,
    text: &'a str,
    offset: usize,
    binding: String,
}

/// One bounded request over one original text. The text is split only after
/// a line feed, never inside a line, so no supported value is cut.
pub struct Request<'a> {
    nodes: Vec<Node<'a>>,
    binding: String,
}

fn hex(digest: Sha256) -> String {
    format!("{:x}", digest.finalize())
}

fn node_binding(id: &str, text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"craftsman/privacy-detector/node/v1\0");
    digest.update((id.len() as u64).to_le_bytes());
    digest.update(id.as_bytes());
    digest.update((text.len() as u64).to_le_bytes());
    digest.update(text.as_bytes());
    hex(digest)
}

fn batch_binding(nodes: &[Node<'_>]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"craftsman/privacy-detector/batch/v1\0");
    digest.update((nodes.len() as u64).to_le_bytes());
    for node in nodes {
        digest.update(node.binding.as_bytes());
    }
    hex(digest)
}

impl<'a> Request<'a> {
    /// Whole text or a refusal. At most 256 KiB, 64 nodes of at most 64 KiB,
    /// each ending after a line feed or at the end of the text.
    pub fn from_text(text: &'a str) -> Result<Self, HookError> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(HookError::Input);
        }
        let mut nodes = Vec::new();
        let mut start = 0;
        loop {
            let mut end = (start + MAX_NODE_BYTES).min(text.len());
            if end < text.len() {
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                // Cut after the last line feed in the window. A longer line
                // has no boundary that is safe for every supported value.
                end = start + text[start..end].rfind('\n').ok_or(HookError::Input)? + 1;
            }
            if nodes.len() >= MAX_NODES {
                return Err(HookError::Input);
            }
            let id = format!("t/{}", nodes.len());
            let slice = &text[start..end];
            nodes.push(Node {
                binding: node_binding(&id, slice),
                id,
                text: slice,
                offset: start,
            });
            start = end;
            if start >= text.len() {
                break;
            }
        }
        let binding = batch_binding(&nodes);
        Ok(Self { nodes, binding })
    }

    /// The document handed to the detector function.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "version": PROTOCOL_VERSION,
            "input_binding": self.binding,
            "nodes": self.nodes.iter().map(|node| serde_json::json!({
                "id": node.id,
                "text": node.text,
                "input_binding": node.binding,
            })).collect::<Vec<_>>(),
        })
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Checks a reply against this exact request and returns its spans in
    /// whole-text offsets, in order and without overlap.
    ///
    /// Refused: a reply over 64 KiB; anything but the one expected document
    /// with no extra field; another version or request binding; more than 128
    /// spans; an unknown node or a wrong node binding; an empty, reversed,
    /// out-of-range or scalar-splitting range; spans out of order or
    /// overlapping; a category other than `email` or `phone`; a span whose
    /// text the vault's own grammar does not admit for that category; and a
    /// reply that leaves a match of the host floor outside every span.
    pub fn validate_reply(&self, reply: &str) -> Result<Vec<TextSpan>, ReplyRefusal> {
        if reply.len() > MAX_REPLY_BYTES {
            return Err(ReplyRefusal::Output);
        }
        let reply: Reply = serde_json::from_str(reply).map_err(|_| ReplyRefusal::Shape)?;
        if reply.version != PROTOCOL_VERSION || reply.input_binding != self.binding {
            return Err(ReplyRefusal::Binding);
        }
        if reply.spans.len() > MAX_SPANS {
            return Err(ReplyRefusal::Count);
        }
        let mut spans = Vec::with_capacity(reply.spans.len());
        let mut reported: Vec<Vec<(usize, usize)>> = vec![Vec::new(); self.nodes.len()];
        let mut previous: Option<(usize, usize)> = None;
        for span in &reply.spans {
            let index = self
                .nodes
                .iter()
                .position(|node| node.id == span.node_id)
                .ok_or(ReplyRefusal::Node)?;
            let node = &self.nodes[index];
            if span.input_binding != node.binding {
                return Err(ReplyRefusal::Binding);
            }
            if span.start >= span.end
                || span.end > node.text.len()
                || !node.text.is_char_boundary(span.start)
                || !node.text.is_char_boundary(span.end)
            {
                return Err(ReplyRefusal::Offset);
            }
            if previous
                .is_some_and(|(prior, end)| index < prior || (index == prior && span.start < end))
            {
                return Err(ReplyRefusal::Order);
            }
            let category = match span.category.as_str() {
                "email" => Category::Email,
                "phone" => Category::Telephone,
                _ => return Err(ReplyRefusal::Category),
            };
            if !category.admits(&node.text[span.start..span.end]) {
                return Err(ReplyRefusal::Category);
            }
            previous = Some((index, span.end));
            reported[index].push((span.start, span.end));
            spans.push(TextSpan {
                start: node.offset + span.start,
                end: node.offset + span.end,
                category,
            });
        }
        // The host floor: every match of the fixed email and telephone rules
        // must lie inside a reported span of its node.
        for (node, reported) in self.nodes.iter().zip(&reported) {
            for (start, end, _) in floor::matches(node.text) {
                if !reported
                    .iter()
                    .any(|(from, to)| *from <= start && *to >= end)
                {
                    return Err(ReplyRefusal::Coverage);
                }
            }
        }
        Ok(spans)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u32,
    input_binding: String,
    spans: Vec<ReplySpan>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplySpan {
    node_id: String,
    start: usize,
    end: usize,
    category: String,
    input_binding: String,
}

/// Where a reply comes from. Implemented by the pinned gadget call and, in
/// tests only, by fixed host rules. Not a public extension point.
#[async_trait::async_trait]
pub(crate) trait ReplySource: Send + Sync {
    async fn reply(&self, request: &serde_json::Value) -> Result<String, HookError>;
}

/// The host's view of one selected detector: request out, checked spans in.
/// It holds no key, mapping, file or network handle.
pub struct Detector {
    source: Arc<dyn ReplySource>,
}

impl Detector {
    /// The production detector: one exact approved user-level gadget source.
    #[cfg(feature = "gadgets")]
    pub fn pinned(detector: PinnedDetector) -> Self {
        Self {
            source: Arc::new(detector),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_source(source: Arc<dyn ReplySource>) -> Self {
        Self { source }
    }

    /// Reported spans of `text`, each checked against `text`. Any failure
    /// means there is no result for this text; nothing partial is returned.
    pub async fn detect(&self, text: &str) -> Result<Vec<TextSpan>, HookError> {
        let request = Request::from_text(text)?;
        let reply = self.source.reply(&request.to_json()).await?;
        request.validate_reply(&reply).map_err(HookError::Reply)
    }
}

impl std::fmt::Debug for Detector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Detector([private])")
    }
}
