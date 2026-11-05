//! Local, capability-scoped WebAssembly gadgets (harness plan G1).
//!
//! Components have no ambient filesystem, network or environment access.
//! Approval precedes *all* execution, including the `describe` export.

pub mod authoring;
mod catalogue;
pub mod cli;
#[allow(dead_code)]
// Preparation-only prototype accessors remain; private host consumes approved sources.
mod confined;
mod grants;
#[allow(dead_code)] // Private composed host; callable registration/publication remain open.
pub(crate) mod headless;
mod host;
mod manifest;
pub(crate) mod migration;
mod runtime;
#[allow(dead_code)] // Private source/grant accessors beyond composed-host use remain unwired.
mod sources;
pub(crate) mod surfaces;
mod tools;

pub use catalogue::GadgetCatalogMiddleware;
pub use runtime::{GadgetRuntime, RuntimeOptions};

use craftsman_harness::ToolErrorKind;
use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorKind {
    NotFound,
    NeedsApproval,
    InvalidArgs,
    DeniedCapability,
    Trap,
    Timeout,
    OutOfMemory,
    InvalidGadget,
    Io,
    OutputLimit,
    Failed,
}

#[derive(Debug, Clone)]
pub struct GadgetError {
    pub kind: ErrorKind,
    pub message: String,
    pub network_calls: u64,
    pub logs_truncated: bool,
}

impl GadgetError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > 16 * 1024 {
            let mut end = 16 * 1024;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            message.push_str("… [truncated]");
        }
        Self {
            kind,
            message,
            network_calls: 0,
            logs_truncated: false,
        }
    }

    fn tool_kind(&self) -> ToolErrorKind {
        match self.kind {
            ErrorKind::NotFound => ToolErrorKind::NotFound,
            ErrorKind::NeedsApproval => ToolErrorKind::NeedsApproval,
            ErrorKind::InvalidArgs => ToolErrorKind::InvalidArgs,
            ErrorKind::DeniedCapability => ToolErrorKind::DeniedCapability,
            ErrorKind::Timeout => ToolErrorKind::Timeout,
            ErrorKind::OutOfMemory => ToolErrorKind::OutOfMemory,
            ErrorKind::OutputLimit => ToolErrorKind::OutputLimit,
            ErrorKind::Trap => ToolErrorKind::Trap,
            ErrorKind::InvalidGadget | ErrorKind::Io | ErrorKind::Failed => ToolErrorKind::Failed,
        }
    }
}

impl fmt::Display for GadgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for GadgetError {}

#[cfg(test)]
mod tests;
