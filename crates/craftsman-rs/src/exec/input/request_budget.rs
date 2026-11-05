//! Explicit private request-budget argv selection through the existing owners.
//! `--max-rounds` caps shared logical admissions, not only main-agent rounds.
use super::frontend::Defaults;
use super::scopes::{self, FileOptions};
use crate::exec::meter::RequestLimit;
use std::ffi::OsString;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionError {
    Count,
    Resource,
    Utf8,
    Value,
    Duplicate,
}
impl std::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for SelectionError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Selection(SelectionError),
    Preparation(scopes::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Error {}

// Add only the split control's two entries and its bounded native overhead.
// The old complete grammar still admits every projected source/control value.
fn bounds() -> (usize, usize, usize) {
    let (count, token, total) = scopes::request_argument_bounds();
    (count + 2, token, total + 32)
}

// These are original caller inputs, not authority or redacted public records.
// No raw argv, path, request source, Runtime, gate, meter or target constructor.
struct Selected {
    arguments: Vec<OsString>,
    limit: RequestLimit,
}
impl Selected {
    fn select(defaults: &Defaults, arguments: &[OsString]) -> Result<Self, SelectionError> {
        let (count, token, total) = bounds();
        if arguments.len() > count {
            return Err(SelectionError::Count);
        }
        let mut bytes = 0usize;
        for argument in arguments {
            if argument.len() > token {
                return Err(SelectionError::Resource);
            }
            bytes = bytes
                .checked_add(argument.len())
                .filter(|n| *n <= total)
                .ok_or(SelectionError::Resource)?;
        }
        if defaults.workspace.as_os_str().len() > super::PATH_BYTES
            || defaults.config_dir.as_os_str().len() > super::PATH_BYTES
        {
            return Err(SelectionError::Resource);
        }
        // WHOLE original native admission precedes UTF-8, retention and copies.
        let texts = arguments
            .iter()
            .map(|s| s.to_str().ok_or(SelectionError::Utf8))
            .collect::<Result<Vec<_>, _>>()?;
        let mut limit = None;
        let mut retained = Vec::new();
        let mut positional = false;
        let mut index = 0usize;
        while index < arguments.len() {
            let start = index;
            let raw = texts[index];
            index += 1;
            if positional {
                retained.push(&arguments[start]);
                continue;
            }
            if raw == "--" {
                positional = true;
                retained.push(&arguments[start]);
                continue;
            }
            let (flag, joined) = raw
                .split_once('=')
                .map_or((raw, None), |(f, v)| (f, Some(v)));
            match flag {
                "--max-rounds" => {
                    if limit.is_some() {
                        return Err(SelectionError::Duplicate);
                    }
                    let value = if let Some(value) = joined {
                        value
                    } else {
                        let value = *texts.get(index).ok_or(SelectionError::Value)?;
                        index += 1;
                        value
                    };
                    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(SelectionError::Value);
                    }
                    let maximum = value.parse::<u64>().map_err(|_| SelectionError::Value)?;
                    limit = Some(RequestLimit::new(maximum).map_err(|_| SelectionError::Value)?);
                }
                // A split flag's value is DATA, including an invalid option-like
                // spelling. Preserve it so extraction cannot repair old syntax.
                "--cwd" | "--config-dir" | "--output-file" | "--permission-mode"
                | "--on-unapproved" | "--session" | "--allow" | "--deny" | "--env-pass"
                | "--prompt-file" | "--file"
                    if joined.is_none() =>
                {
                    if index >= arguments.len() {
                        return Err(SelectionError::Value);
                    }
                    index += 1;
                    retained.extend(arguments[start..index].iter());
                }
                _ => retained.push(&arguments[start]),
            }
        }
        Ok(Self {
            arguments: retained.into_iter().cloned().collect(),
            limit: limit.unwrap_or_default(),
        })
    }
}

/// Select the cap before complete old syntax/input-first preparation.
pub(crate) struct Invocation {
    invocation: scopes::Invocation,
}
impl Invocation {
    pub(crate) async fn prepare(defaults: Defaults, arguments: &[OsString]) -> Result<Self, Error> {
        let selected = Selected::select(&defaults, arguments).map_err(Error::Selection)?;
        let invocation = scopes::Invocation::prepare_with_request_limit(
            defaults,
            &selected.arguments,
            selected.limit,
        )
        .await
        .map_err(Error::Preparation)?;
        Ok(Self { invocation })
    }
    pub(crate) async fn run(self) -> super::Run {
        self.invocation.run().await
    }
}

/// Keep the SAME pre-run Destination/FileRun/terminal pairing and one attempt.
pub(crate) struct FileInvocation {
    invocation: scopes::FileInvocation,
}
impl FileInvocation {
    pub(crate) async fn prepare(
        defaults: Defaults,
        arguments: &[OsString],
        options: FileOptions,
    ) -> Result<Self, Error> {
        let selected = Selected::select(&defaults, arguments).map_err(Error::Selection)?;
        let invocation = scopes::FileInvocation::prepare_with_request_limit(
            defaults,
            &selected.arguments,
            options,
            selected.limit,
        )
        .await
        .map_err(Error::Preparation)?;
        Ok(Self { invocation })
    }
    pub(crate) async fn run(self) -> super::FileRun {
        self.invocation.run().await
    }
}
