//! Private one-file publication of actual-terminal checked records, not exec/R7.
//! Destinations are captured before session creation; no stdio/JSONL/store writer.
mod path;
#[cfg(test)]
mod tests;
mod write;

use crate::exec::terminal::Terminal;
pub(super) use path::{Selected, Target};
use std::io::ErrorKind;
use std::path::PathBuf;
pub(super) const CAP: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// Private terminal fragment, NOT a schema-backed exec-result object.
    AsciiFragment,
    /// Complete answer then fixed private-fact diagnostic, NOT public text output.
    HumanPreview,
}
/// Explicit invocation options, NOT a target/data/permission/source authority.
/// Original names are never redacted or substituted into filesystem operations.
pub(crate) struct Destination {
    pub(crate) directory: PathBuf,
    pub(crate) file_name: String,
    pub(crate) format: Format,
    pub(crate) max_bytes: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    Path,
    Scope,
    Exists,
    Changed,
    Limit,
    Random,
    Io(ErrorKind),
    Terminal(crate::exec::terminal::Error),
    AlreadyAttempted,
    Worker,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == ErrorKind::AlreadyExists {
            Self::Exists
        } else {
            Self::Io(e.kind())
        }
    }
}
impl From<crate::exec::terminal::Error> for Error {
    fn from(e: crate::exec::terminal::Error) -> Self {
        Self::Terminal(e)
    }
}
pub(crate) type Failure = crate::exec::input::PublicationFailure;
pub(crate) type FileTerminal = crate::exec::input::FileTerminal;

pub(in crate::exec) fn publish_target(
    target: &Target,
    terminal: &Terminal,
    record: &crate::exec::terminal::Record<'_>,
    linked: &mut bool,
) -> Result<(), Error> {
    write::publish(
        target,
        terminal,
        record,
        linked,
        &mut write::Signals::default(),
    )
}
#[cfg(test)]
pub(in crate::exec) use write::Signals;
#[cfg(test)]
pub(in crate::exec) fn publish_target_for_tests(
    target: &Target,
    terminal: &Terminal,
    record: &crate::exec::terminal::Record<'_>,
    linked: &mut bool,
    signals: &mut Signals<'_>,
) -> Result<(), Error> {
    write::publish(target, terminal, record, linked, signals)
}

#[cfg(test)]
mod stdin_tests;
