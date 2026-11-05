//! Private supported control/source argv composition, not a public exec route.
//! Absolute scope defaults are caller input, NOT captured filesystem authority.
//! Unsupported output/usage/trace/gadget/wall flags refuse before any effects.
use super::{InputError, Sources};
use crate::exec::construction::{Options, SessionSelection, session_argument_id_valid};
use crate::exec::output::Destination;
use crate::exec::permissions::{
    self, InvocationPermissions, Mode, OnUnapproved, Selectors, UsageError,
};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
mod tests;

/// Explicit captured defaults. Never consult/mutate cwd/HOME/config/environment.
/// Old Bootstrap alone retains physical roots, credentials, policy and sessions.
pub(crate) struct Defaults {
    pub(crate) workspace: PathBuf,
    pub(crate) config_dir: PathBuf,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionError {
    Count,
    Resource,
    Utf8,
    Flag,
    Value,
    Duplicate,
    Scope,
    Session,
    Input(InputError),
    Stdin(super::StdinError),
    Permissions(UsageError),
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
    Preparation(super::arguments::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Error {}

// Only complete supported grammar creates this private pre-run input. No raw
// argv/paths/policy/run Debug/Serialize/Clone, or caller Runtime/Run constructor.
struct Selected {
    options: Options,
    source_arguments: Vec<OsString>,
}
impl Selected {
    fn select(defaults: Defaults, arguments: &[OsString]) -> Result<Self, SelectionError> {
        let (rules, rule_bytes, envs, env_bytes) = permissions::frontend_argument_bounds();
        let source_count = 2 * super::FILES + 4;
        let source_bytes = super::INSTRUCTION_BYTES + super::FILES * (super::PATH_BYTES + 8) + 64;
        let count_limit = source_count + 4 * rules + 2 * envs + 16;
        let byte_limit = source_bytes
            + 2 * rules * (rule_bytes + 8)
            + envs * (env_bytes + 16)
            + 2 * (super::PATH_BYTES + 16)
            + 128;
        if arguments.len() > count_limit {
            return Err(SelectionError::Count);
        }
        let mut bytes = 0usize;
        for argument in arguments {
            if argument.len() > super::INSTRUCTION_BYTES + 32 {
                return Err(SelectionError::Resource);
            }
            bytes = bytes
                .checked_add(argument.len())
                .filter(|n| *n <= byte_limit)
                .ok_or(SelectionError::Resource)?;
        }
        let mut workspace = None;
        let mut config_dir = None;
        let mut mode = None;
        let mut on_unapproved = None;
        let mut session = None;
        let mut trust = false;
        let mut selectors = Selectors::default();
        let mut source = None;
        let mut files = Vec::new();
        let mut source_indices = Vec::new();
        let mut positional = false;
        let mut index = 0usize;
        while index < arguments.len() {
            let start = index;
            let raw = text(&arguments[index])?;
            index += 1;
            if positional {
                add_source(&mut source, Source::Prompt(raw))?;
                source_indices.push(start);
                continue;
            }
            if raw == "--" {
                positional = true;
                source_indices.push(start);
                continue;
            }
            let (flag, joined) = raw
                .split_once('=')
                .map_or((raw, None), |(f, v)| (f, Some(v)));
            match flag {
                "--cwd" => once(&mut workspace, value(arguments, &mut index, joined)?)?,
                "--config-dir" => once(&mut config_dir, value(arguments, &mut index, joined)?)?,
                "--permission-mode" => {
                    let selected = Mode::parse(value(arguments, &mut index, joined)?)
                        .map_err(SelectionError::Permissions)?;
                    once(&mut mode, selected)?;
                }
                "--on-unapproved" => {
                    let selected = OnUnapproved::parse(value(arguments, &mut index, joined)?)
                        .map_err(SelectionError::Permissions)?;
                    once(&mut on_unapproved, selected)?;
                }
                "--session" => {
                    let id = value(arguments, &mut index, joined)?;
                    if !session_argument_id_valid(id) {
                        return Err(SelectionError::Session);
                    }
                    once(&mut session, SessionSelection::Resume(id.to_owned()))?;
                }
                "--ephemeral" if joined.is_none() => {
                    once(&mut session, SessionSelection::Ephemeral)?
                }
                "--continue" if joined.is_none() => once(&mut session, SessionSelection::Continue)?,
                "--trust-workspace" if joined.is_none() => {
                    if trust {
                        return Err(SelectionError::Duplicate);
                    }
                    trust = true;
                }
                "--allow" => add(
                    &mut selectors.allow,
                    value(arguments, &mut index, joined)?,
                    rules,
                    rule_bytes,
                )?,
                "--deny" => add(
                    &mut selectors.deny,
                    value(arguments, &mut index, joined)?,
                    rules,
                    rule_bytes,
                )?,
                "--env-pass" => add(
                    &mut selectors.env_pass,
                    value(arguments, &mut index, joined)?,
                    envs,
                    env_bytes,
                )?,
                "--prompt-file" => {
                    let path = value(arguments, &mut index, joined)?;
                    add_source(&mut source, Source::File(path))?;
                    source_indices.extend(start..index);
                }
                "--file" => {
                    let path = value(arguments, &mut index, joined)?;
                    if files.len() >= super::FILES {
                        return Err(SelectionError::Input(InputError::SelectionLimit));
                    }
                    super::admit_path(path).map_err(SelectionError::Input)?;
                    files.push(path);
                    source_indices.extend(start..index);
                }
                _ if raw.starts_with('-') && raw != "-" => return Err(SelectionError::Flag),
                _ => {
                    add_source(&mut source, Source::Prompt(raw))?;
                    source_indices.push(start);
                }
            }
        }
        let source = source.ok_or(SelectionError::Input(InputError::MissingSource))?;
        let (prompt, prompt_file) = match source {
            Source::Prompt(text) => (Some(text.as_bytes()), None),
            Source::File(path) => (None, Some(path)),
        };
        let sources = Sources {
            prompt,
            prompt_file,
            files: &files,
        };
        if prompt == Some(b"-".as_slice()) {
            super::StdinSelection::select(sources).map_err(SelectionError::Stdin)?;
        } else {
            super::Selected::select(sources).map_err(SelectionError::Input)?;
        }
        selectors.mode = mode.unwrap_or_default();
        selectors.on_unapproved = on_unapproved.unwrap_or_default();
        // Grammar/platform/conventional-secret preflight ONLY. Never retain this
        // provisional policy or use it for IO. Bootstrap revalidates against its
        // ORIGINAL configured credential-source names before reading input.
        InvocationPermissions::from_explicit(selectors.clone(), &[])
            .map_err(SelectionError::Permissions)?;
        let selected_workspace = workspace.map(Path::new).unwrap_or(&defaults.workspace);
        let selected_config = config_dir.map(Path::new).unwrap_or(&defaults.config_dir);
        scope(selected_workspace)?;
        scope(selected_config)?;
        if source_indices.len() > source_count {
            return Err(SelectionError::Count);
        }
        let options = Options {
            workspace: workspace.map(PathBuf::from).unwrap_or(defaults.workspace),
            config_dir: config_dir.map(PathBuf::from).unwrap_or(defaults.config_dir),
            trust_workspace: trust,
            session: session.unwrap_or_default(),
            selectors,
        };
        let source_arguments = source_indices
            .into_iter()
            .map(|i| arguments[i].clone())
            .collect();
        Ok(Self {
            options,
            source_arguments,
        })
    }
}
enum Source<'a> {
    Prompt(&'a str),
    File(&'a str),
}
fn text(argument: &OsString) -> Result<&str, SelectionError> {
    argument.to_str().ok_or(SelectionError::Utf8)
}
fn value<'a>(
    arguments: &'a [OsString],
    index: &mut usize,
    joined: Option<&'a str>,
) -> Result<&'a str, SelectionError> {
    if let Some(value) = joined {
        return Ok(value);
    }
    let value = text(arguments.get(*index).ok_or(SelectionError::Value)?)?;
    *index += 1;
    if value.starts_with('-') && value != "-" {
        return Err(SelectionError::Value);
    }
    Ok(value)
}
fn once<T>(field: &mut Option<T>, value: T) -> Result<(), SelectionError> {
    if field.is_some() {
        return Err(SelectionError::Duplicate);
    }
    *field = Some(value);
    Ok(())
}
fn add_source<'a>(
    source: &mut Option<Source<'a>>,
    value: Source<'a>,
) -> Result<(), SelectionError> {
    if source.is_some() {
        return Err(SelectionError::Input(InputError::ConflictingSources));
    }
    *source = Some(value);
    Ok(())
}
fn add(
    values: &mut Vec<String>,
    value: &str,
    count: usize,
    bytes: usize,
) -> Result<(), SelectionError> {
    if values.len() >= count || value.len() > bytes {
        return Err(SelectionError::Resource);
    }
    values.push(value.to_owned());
    Ok(())
}
fn scope(path: &Path) -> Result<(), SelectionError> {
    let raw = path.to_str().ok_or(SelectionError::Utf8)?;
    if !path.is_absolute()
        || raw.len() > super::PATH_BYTES
        || raw.chars().count() > super::PATH_SCALARS
        || raw.chars().any(char::is_control)
        || raw
            .split(['/', '\\'])
            .any(|part| part == "." || part == "..")
    {
        return Err(SelectionError::Scope);
    }
    let mut depth = 0usize;
    for part in path.components() {
        match part {
            Component::RootDir => {}
            #[cfg(windows)]
            Component::Prefix(prefix)
                if matches!(
                    prefix.kind(),
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                ) => {}
            Component::Normal(_) => {
                depth += 1;
                if depth > 128 {
                    return Err(SelectionError::Scope);
                }
            }
            _ => return Err(SelectionError::Scope),
        }
    }
    Ok(())
}

/// Consumes validated supported controls and invokes ONLY the old argv factory.
pub(crate) struct Invocation {
    invocation: super::ArgumentInvocation,
}
impl Invocation {
    pub(crate) async fn prepare(defaults: Defaults, arguments: &[OsString]) -> Result<Self, Error> {
        let selected = Selected::select(defaults, arguments).map_err(Error::Selection)?;
        let invocation =
            super::ArgumentInvocation::prepare(selected.options, &selected.source_arguments)
                .await
                .map_err(Error::Preparation)?;
        Ok(Self { invocation })
    }
    pub(crate) async fn run(self) -> super::Run {
        self.invocation.run().await
    }
}
/// The original Destination is typed pre-run input, never an output flag/alias.
pub(crate) struct FileInvocation {
    invocation: super::ArgumentFileInvocation,
}
impl FileInvocation {
    pub(crate) async fn prepare(
        defaults: Defaults,
        arguments: &[OsString],
        destination: Destination,
    ) -> Result<Self, Error> {
        let selected = Selected::select(defaults, arguments).map_err(Error::Selection)?;
        let invocation = super::ArgumentFileInvocation::prepare(
            selected.options,
            &selected.source_arguments,
            destination,
        )
        .await
        .map_err(Error::Preparation)?;
        Ok(Self { invocation })
    }
    pub(crate) async fn run(self) -> super::FileRun {
        self.invocation.run().await
    }
}

// Syntax-only reuse for the separate relative-scope/output argv adapter. This
// temporary selection is dropped; ONLY the old concrete factory captures IO.
pub(in crate::exec::input) fn preflight(
    defaults: Defaults,
    arguments: &[OsString],
) -> Result<(), SelectionError> {
    Selected::select(defaults, arguments).map(drop)
}

impl Invocation {
    pub(crate) async fn prepare_with_request_limit(
        defaults: Defaults,
        arguments: &[OsString],
        requests: crate::exec::meter::RequestLimit,
    ) -> Result<Self, Error> {
        let selected = Selected::select(defaults, arguments).map_err(Error::Selection)?;
        let invocation = super::ArgumentInvocation::prepare_with_request_limit(
            selected.options,
            &selected.source_arguments,
            requests,
        )
        .await
        .map_err(Error::Preparation)?;
        Ok(Self { invocation })
    }
}
impl FileInvocation {
    pub(crate) async fn prepare_with_request_limit(
        defaults: Defaults,
        arguments: &[OsString],
        destination: Destination,
        requests: crate::exec::meter::RequestLimit,
    ) -> Result<Self, Error> {
        let selected = Selected::select(defaults, arguments).map_err(Error::Selection)?;
        let invocation = super::ArgumentFileInvocation::prepare_with_request_limit(
            selected.options,
            &selected.source_arguments,
            destination,
            requests,
        )
        .await
        .map_err(Error::Preparation)?;
        Ok(Self { invocation })
    }
}
