//! Private bounded relative-scope/output-file argv, not public exec or formats.
//! Resolution is explicit input spelling, NEVER filesystem/source authority.
//! Old Bootstrap/ConfinedFs and paired publisher alone retain actual authority.
use super::frontend::{self, Defaults};
use crate::exec::output::{self, Destination, Format};
use crate::exec::permissions;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
mod tests;

/// Private preview choice is typed BEFORE preparation, not public format argv.
/// No caller output bytes/status/target/gate/seed/writer or post-run switch.
pub(crate) struct FileOptions {
    pub(crate) format: Format,
    pub(crate) max_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionError {
    Count,
    Resource,
    Utf8,
    Value,
    Duplicate,
    Scope,
    MissingDestination,
    UnexpectedDestination,
    Controls(frontend::SelectionError),
    Output(output::Error),
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
    Preparation(frontend::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Error {}

// Native preflight is derived from the SAME old source/control/policy bounds.
// Only one output-file path/two entries are additional, not bigger old inputs.
fn bounds() -> (usize, usize, usize) {
    let (rules, rule_bytes, envs, env_bytes) = permissions::frontend_argument_bounds();
    let source_count = 2 * super::FILES + 4;
    let source_bytes = super::INSTRUCTION_BYTES + super::FILES * (super::PATH_BYTES + 8) + 64;
    let control_count = source_count + 4 * rules + 2 * envs + 16;
    let control_bytes = source_bytes
        + 2 * rules * (rule_bytes + 8)
        + envs * (env_bytes + 16)
        + 2 * (super::PATH_BYTES + 16)
        + 128;
    (
        control_count + 2,
        super::INSTRUCTION_BYTES + 32,
        control_bytes + OUTPUT_PATH_BYTES + 16,
    )
}
const OUTPUT_PATH_BYTES: usize = super::PATH_BYTES + 256;
const OUTPUT_PATH_SCALARS: usize = super::PATH_SCALARS + 256;

struct Selected {
    defaults: Defaults,
    arguments: Vec<OsString>,
    destination: Option<Destination>,
}
struct Scope<'a> {
    flag: &'static str,
    value: &'a str,
    joined: bool,
}
enum Argument<'a> {
    Original(&'a OsString),
    Scope(Scope<'a>),
}
impl Selected {
    fn select(
        defaults: Defaults,
        arguments: &[OsString],
        file_options: Option<FileOptions>,
    ) -> Result<Self, SelectionError> {
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
        // All native bounds (including explicit default scopes) BEFORE UTF8,
        // borrowed retention, path concatenation or copies. No ambient lookup.
        if defaults.workspace.as_os_str().len() > super::PATH_BYTES
            || defaults.config_dir.as_os_str().len() > super::PATH_BYTES
        {
            return Err(SelectionError::Resource);
        }
        let texts = arguments
            .iter()
            .map(|s| s.to_str().ok_or(SelectionError::Utf8))
            .collect::<Result<Vec<_>, _>>()?;
        absolute(&defaults.workspace)?;
        absolute(&defaults.config_dir)?;
        let mut selected = Vec::new();
        let mut workspace = None;
        let mut config = None;
        let mut output_file = None;
        let mut positional = false;
        let mut index = 0usize;
        while index < arguments.len() {
            let start = index;
            let raw = texts[index];
            index += 1;
            if positional {
                selected.push(Argument::Original(&arguments[start]));
                continue;
            }
            if raw == "--" {
                positional = true;
                selected.push(Argument::Original(&arguments[start]));
                continue;
            }
            let (flag, joined) = raw
                .split_once('=')
                .map_or((raw, None), |(f, v)| (f, Some(v)));
            match flag {
                "--cwd" | "--config-dir" => {
                    let value = value(&texts, &mut index, joined)?;
                    let field = if flag == "--cwd" {
                        &mut workspace
                    } else {
                        &mut config
                    };
                    once(field, value)?;
                    selected.push(Argument::Scope(Scope {
                        flag: if flag == "--cwd" {
                            "--cwd"
                        } else {
                            "--config-dir"
                        },
                        value,
                        joined: joined.is_some(),
                    }));
                }
                "--output-file" => once(&mut output_file, value(&texts, &mut index, joined)?)?,
                // Preserve each older split value as data, never scan it as an
                // independent scope/output control. Old whole grammar validates.
                "--permission-mode" | "--on-unapproved" | "--session" | "--allow" | "--deny"
                | "--env-pass" | "--prompt-file" | "--file"
                    if joined.is_none() =>
                {
                    if index >= arguments.len() {
                        return Err(SelectionError::Value);
                    }
                    index += 1;
                    selected.extend(arguments[start..index].iter().map(Argument::Original));
                }
                _ => selected.push(Argument::Original(&arguments[start])),
            }
        }
        if file_options.is_none() && output_file.is_some() {
            return Err(SelectionError::UnexpectedDestination);
        }
        if file_options.is_some() && output_file.is_none() {
            return Err(SelectionError::MissingDestination);
        }
        // Every relative scope uses the ORIGINAL explicit launch workspace,
        // independently of option order. Relative task/output paths use the
        // SELECTED workspace, never the caller's later cwd.
        let selected_workspace = workspace
            .map(|s| resolve(&defaults.workspace, s))
            .transpose()?
            .unwrap_or_else(|| defaults.workspace.clone());
        let mut projected = Vec::new();
        for item in selected {
            match item {
                Argument::Original(s) => projected.push(s.clone()),
                Argument::Scope(scope) => {
                    let path = resolve(&defaults.workspace, scope.value)?;
                    if scope.joined {
                        let path = path.to_str().ok_or(SelectionError::Utf8)?;
                        projected.push(OsString::from(format!("{}={path}", scope.flag)));
                    } else {
                        projected.push(OsString::from(scope.flag));
                        projected.push(path.into_os_string());
                    }
                }
            }
        }
        // SAME complete absolute source/control/session/permission grammar,
        // syntax-only and DROPPED. No cached provisional execution authority.
        frontend::preflight(
            Defaults {
                workspace: defaults.workspace.clone(),
                config_dir: defaults.config_dir.clone(),
            },
            &projected,
        )
        .map_err(SelectionError::Controls)?;
        let destination = match (file_options, output_file) {
            (Some(options), Some(path)) => {
                let destination = destination(&selected_workspace, path, options)?;
                // Reuse old complete pure destination grammar. Temporary old
                // selection is discarded; old factory later captures SAME roots.
                output::Selected::select(Destination {
                    directory: destination.directory.clone(),
                    file_name: destination.file_name.clone(),
                    format: destination.format,
                    max_bytes: destination.max_bytes,
                })
                .map_err(SelectionError::Output)?;
                Some(destination)
            }
            (None, None) => None,
            _ => return Err(SelectionError::MissingDestination),
        };
        Ok(Self {
            defaults,
            arguments: projected,
            destination,
        })
    }
}
fn value<'a>(
    texts: &[&'a str],
    index: &mut usize,
    joined: Option<&'a str>,
) -> Result<&'a str, SelectionError> {
    if let Some(value) = joined {
        return Ok(value);
    }
    let value = *texts.get(*index).ok_or(SelectionError::Value)?;
    *index += 1;
    if value.starts_with('-') && value != "-" {
        return Err(SelectionError::Value);
    }
    Ok(value)
}
fn once<'a>(field: &mut Option<&'a str>, value: &'a str) -> Result<(), SelectionError> {
    if field.is_some() {
        return Err(SelectionError::Duplicate);
    }
    *field = Some(value);
    Ok(())
}
fn raw_path(path: &Path) -> Result<&str, SelectionError> {
    let raw = path.to_str().ok_or(SelectionError::Utf8)?;
    if raw.is_empty()
        || raw.len() > super::PATH_BYTES
        || raw.chars().count() > super::PATH_SCALARS
        || raw.chars().any(char::is_control)
        || raw.split(['/', '\\']).any(|p| p == "." || p == "..")
    {
        return Err(SelectionError::Scope);
    }
    Ok(raw)
}
fn absolute(path: &Path) -> Result<(), SelectionError> {
    let raw = raw_path(path)?;
    if !path.is_absolute() {
        return Err(SelectionError::Scope);
    }
    if raw.starts_with("//") {
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
            Component::Normal(name) => {
                // A portable strict subset, not alias/ADS/foreign-drive repair.
                let name = name.to_str().ok_or(SelectionError::Utf8)?;
                if name.contains(['/', ':', '\\']) {
                    return Err(SelectionError::Scope);
                }
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
fn resolve(base: &Path, raw: &str) -> Result<PathBuf, SelectionError> {
    let path = Path::new(raw);
    raw_path(path)?;
    if path.is_absolute() {
        absolute(path)?;
        return Ok(path.to_path_buf());
    }
    for part in path.components() {
        match part {
            Component::Normal(name)
                if !name
                    .to_str()
                    .ok_or(SelectionError::Utf8)?
                    .contains(['/', ':', '\\']) => {}
            _ => return Err(SelectionError::Scope),
        }
    }
    let base_raw = base.to_str().ok_or(SelectionError::Utf8)?;
    let separator = if base_raw.ends_with(std::path::MAIN_SEPARATOR)
        || (cfg!(windows) && base_raw.ends_with('/'))
    {
        0
    } else {
        1
    };
    if base_raw
        .len()
        .checked_add(separator)
        .and_then(|n| n.checked_add(raw.len()))
        .is_none_or(|n| n > super::PATH_BYTES)
    {
        return Err(SelectionError::Scope);
    }
    // Raw native append, NOT PathBuf::join/push/canonicalize/chdir: Windows
    // verbatim bases must not normalize away a component before admission.
    let mut spelling = base.as_os_str().to_os_string();
    if separator != 0 {
        spelling.push(std::path::MAIN_SEPARATOR_STR);
    }
    spelling.push(path.as_os_str());
    let path = PathBuf::from(spelling);
    absolute(&path)?;
    Ok(path)
}
fn destination(
    workspace: &Path,
    raw: &str,
    options: FileOptions,
) -> Result<Destination, SelectionError> {
    if raw.is_empty()
        || raw.len() > OUTPUT_PATH_BYTES
        || raw.chars().count() > OUTPUT_PATH_SCALARS
        || raw.chars().any(char::is_control)
        || raw.ends_with(['/', '\\'])
        || raw.split(['/', '\\']).any(|p| p == "." || p == "..")
    {
        return Err(SelectionError::Output(output::Error::Path));
    }
    let path = Path::new(raw);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(SelectionError::Output(output::Error::Path))?;
    let directory_raw = &raw[..raw.len() - name.len()];
    let directory = if directory_raw.is_empty() {
        workspace.to_path_buf()
    } else {
        resolve(workspace, directory_raw)?
    };
    Ok(Destination {
        directory,
        file_name: name.to_owned(),
        format: options.format,
        max_bytes: options.max_bytes,
    })
}

pub(crate) struct Invocation {
    invocation: super::ControlInvocation,
}
impl Invocation {
    pub(crate) async fn prepare(defaults: Defaults, arguments: &[OsString]) -> Result<Self, Error> {
        let selected = Selected::select(defaults, arguments, None).map_err(Error::Selection)?;
        let invocation = super::ControlInvocation::prepare(selected.defaults, &selected.arguments)
            .await
            .map_err(Error::Preparation)?;
        Ok(Self { invocation })
    }
    pub(crate) async fn run(self) -> super::Run {
        self.invocation.run().await
    }
}
pub(crate) struct FileInvocation {
    invocation: super::ControlFileInvocation,
}
impl FileInvocation {
    pub(crate) async fn prepare(
        defaults: Defaults,
        arguments: &[OsString],
        options: FileOptions,
    ) -> Result<Self, Error> {
        let selected =
            Selected::select(defaults, arguments, Some(options)).map_err(Error::Selection)?;
        let destination = selected
            .destination
            .ok_or(Error::Selection(SelectionError::MissingDestination))?;
        let invocation = super::ControlFileInvocation::prepare(
            selected.defaults,
            &selected.arguments,
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
