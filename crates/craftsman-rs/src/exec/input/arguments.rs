//! Private instruction argv grammar and routing, not a public exec frontend.
//!
//! Only the source/attachment subset is parsed. Scopes/permissions remain
//! explicit Options and guarded output is chosen before preparation as the
//! existing Destination. Unknown/full frontend flags refuse, never run ignored.
use super::{InputError, Sources};
use crate::exec::construction::{
    InputPreparationError, Options, OutputPreparationError, StdinOutputPreparationError,
    StdinPreparationError,
};
use crate::exec::output::Destination;
use std::ffi::OsString;

#[cfg(test)]
mod tests;

// Every legal split source/attachment argv fits these derived bounds. Check
// native encoded lengths before UTF-8 inspection, borrowed retention or copies.
const ARGUMENTS: usize = 2 * super::FILES + 4;
const TOKEN_BYTES: usize = super::INSTRUCTION_BYTES + 16;
const ARGUMENT_BYTES: usize =
    super::INSTRUCTION_BYTES + super::FILES * (super::PATH_BYTES + 8) + 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionError {
    Count,
    Resource,
    Utf8,
    Flag,
    Value,
    Input(InputError),
    Stdin(super::StdinError),
}
impl std::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for SelectionError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Arguments(SelectionError),
    Input(InputPreparationError),
    Stdin(StdinPreparationError),
    Output(OutputPreparationError),
    StdinOutput(StdinOutputPreparationError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Error {}

// Original borrowed values are task input, not checked views or source grants.
// No Debug/Serialize/Clone of argv, instructions, paths or invocations.
enum Source<'a> {
    Prompt(&'a str),
    File(&'a str),
}
struct Selected<'a> {
    source: Source<'a>,
    files: Vec<&'a str>,
}
impl<'a> Selected<'a> {
    fn select(arguments: &'a [OsString]) -> Result<Self, SelectionError> {
        if arguments.len() > ARGUMENTS {
            return Err(SelectionError::Count);
        }
        let mut bytes = 0usize;
        for argument in arguments {
            if argument.len() > TOKEN_BYTES {
                return Err(SelectionError::Resource);
            }
            bytes = bytes
                .checked_add(argument.len())
                .filter(|total| *total <= ARGUMENT_BYTES)
                .ok_or(SelectionError::Resource)?;
        }
        let mut source = None;
        let mut files = Vec::new();
        let mut positional = false;
        let mut index = 0usize;
        while index < arguments.len() {
            let argument = text(&arguments[index])?;
            index += 1;
            if positional {
                set_source(&mut source, Source::Prompt(argument))?;
            } else if argument == "--" {
                positional = true;
            } else if argument == "--prompt-file" {
                let value = split_value(arguments, &mut index)?;
                set_source(&mut source, Source::File(value))?;
            } else if let Some(value) = argument.strip_prefix("--prompt-file=") {
                set_source(&mut source, Source::File(value))?;
            } else if argument == "--file" {
                let value = split_value(arguments, &mut index)?;
                add_file(&mut files, value)?;
            } else if let Some(value) = argument.strip_prefix("--file=") {
                add_file(&mut files, value)?;
            } else if argument.starts_with('-') && argument != "-" {
                return Err(SelectionError::Flag);
            } else {
                set_source(&mut source, Source::Prompt(argument))?;
            }
        }
        let selected = Self {
            source: source.ok_or(SelectionError::Input(InputError::MissingSource))?,
            files,
        };
        // Reuse the unchanged WHOLE input/count/path grammar, never a new
        // source policy or an IO probe. These temporary selections are dropped;
        // only the existing concrete factory later captures its owned input.
        if selected.is_stdin() {
            super::StdinSelection::select(selected.sources()).map_err(SelectionError::Stdin)?;
        } else {
            super::Selected::select(selected.sources()).map_err(SelectionError::Input)?;
        }
        Ok(selected)
    }
    fn is_stdin(&self) -> bool {
        matches!(self.source, Source::Prompt("-"))
    }
    fn sources(&self) -> Sources<'_> {
        let (prompt, prompt_file) = match self.source {
            Source::Prompt(text) => (Some(text.as_bytes()), None),
            Source::File(path) => (None, Some(path)),
        };
        Sources {
            prompt,
            prompt_file,
            files: &self.files,
        }
    }
}
fn text(argument: &OsString) -> Result<&str, SelectionError> {
    argument.to_str().ok_or(SelectionError::Utf8)
}
fn split_value<'a>(
    arguments: &'a [OsString],
    index: &mut usize,
) -> Result<&'a str, SelectionError> {
    let value = text(arguments.get(*index).ok_or(SelectionError::Value)?)?;
    *index += 1;
    // Option-looking filenames require the unambiguous joined spelling. A
    // single '-' is a FILE value here, not a positional/native stdin marker.
    if value.starts_with('-') && value != "-" {
        return Err(SelectionError::Value);
    }
    Ok(value)
}
fn set_source<'a>(
    source: &mut Option<Source<'a>>,
    value: Source<'a>,
) -> Result<(), SelectionError> {
    if source.is_some() {
        return Err(SelectionError::Input(InputError::ConflictingSources));
    }
    *source = Some(value);
    Ok(())
}
fn add_file<'a>(files: &mut Vec<&'a str>, value: &'a str) -> Result<(), SelectionError> {
    if files.len() >= super::FILES {
        return Err(SelectionError::Input(InputError::SelectionLimit));
    }
    super::admit_path(value).map_err(SelectionError::Input)?;
    files.push(value);
    Ok(())
}

/// Only explicit argv + the original Options create this opaque memory route.
/// It returns the EXISTING actual Run, never a new raw state/status/IO wrapper.
pub(crate) struct Invocation {
    route: MemoryRoute,
}
enum MemoryRoute {
    Input(super::Invocation),
    Stdin(super::StdinInvocation),
}
impl Invocation {
    pub(crate) async fn prepare(options: Options, arguments: &[OsString]) -> Result<Self, Error> {
        let selected = Selected::select(arguments).map_err(Error::Arguments)?;
        let route = if selected.is_stdin() {
            MemoryRoute::Stdin(
                super::StdinInvocation::prepare(options, selected.sources())
                    .await
                    .map_err(Error::Stdin)?,
            )
        } else {
            MemoryRoute::Input(
                super::Invocation::prepare(options, selected.sources())
                    .await
                    .map_err(Error::Input)?,
            )
        };
        Ok(Self { route })
    }
    pub(crate) async fn run(self) -> super::Run {
        match self.route {
            MemoryRoute::Input(invocation) => invocation.run().await,
            MemoryRoute::Stdin(invocation) => invocation.run().await,
        }
    }
    pub(crate) async fn prepare_with_request_limit(
        options: Options,
        arguments: &[OsString],
        requests: crate::exec::meter::RequestLimit,
    ) -> Result<Self, Error> {
        let selected = Selected::select(arguments).map_err(Error::Arguments)?;
        let route = if selected.is_stdin() {
            MemoryRoute::Stdin(
                super::StdinInvocation::prepare_with_request_limit(
                    options,
                    selected.sources(),
                    requests,
                )
                .await
                .map_err(Error::Stdin)?,
            )
        } else {
            MemoryRoute::Input(
                super::Invocation::prepare_with_request_limit(
                    options,
                    selected.sources(),
                    requests,
                )
                .await
                .map_err(Error::Input)?,
            )
        };
        Ok(Self { route })
    }
}

/// Guarded Destination is selected BEFORE preparation/run, never parsed from
/// output flags or switched after execution. Only the old paired FileRun exits.
pub(crate) struct FileInvocation {
    route: FileRoute,
}
enum FileRoute {
    Input(super::FileInvocation),
    Stdin(super::StdinFileInvocation),
}
impl FileInvocation {
    pub(crate) async fn prepare(
        options: Options,
        arguments: &[OsString],
        destination: Destination,
    ) -> Result<Self, Error> {
        let selected = Selected::select(arguments).map_err(Error::Arguments)?;
        let route = if selected.is_stdin() {
            FileRoute::Stdin(
                super::StdinFileInvocation::prepare(options, selected.sources(), destination)
                    .await
                    .map_err(Error::StdinOutput)?,
            )
        } else {
            FileRoute::Input(
                super::FileInvocation::prepare(options, selected.sources(), destination)
                    .await
                    .map_err(Error::Output)?,
            )
        };
        Ok(Self { route })
    }
    pub(crate) async fn run(self) -> super::FileRun {
        match self.route {
            FileRoute::Input(invocation) => invocation.run().await,
            FileRoute::Stdin(invocation) => invocation.run().await,
        }
    }
    pub(crate) async fn prepare_with_request_limit(
        options: Options,
        arguments: &[OsString],
        destination: Destination,
        requests: crate::exec::meter::RequestLimit,
    ) -> Result<Self, Error> {
        let selected = Selected::select(arguments).map_err(Error::Arguments)?;
        let route = if selected.is_stdin() {
            FileRoute::Stdin(
                super::StdinFileInvocation::prepare_with_request_limit(
                    options,
                    selected.sources(),
                    destination,
                    requests,
                )
                .await
                .map_err(Error::StdinOutput)?,
            )
        } else {
            FileRoute::Input(
                super::FileInvocation::prepare_with_request_limit(
                    options,
                    selected.sources(),
                    destination,
                    requests,
                )
                .await
                .map_err(Error::Output)?,
            )
        };
        Ok(Self { route })
    }
}
