use super::{InputStamp, MAX_OBSERVED_INPUT_BYTES, context_error};
use craftsman_harness::{AgentError, AgentState, ToolCall, ToolErrorKind, ToolOutcome};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) const KEY: &str = "_craftsman_context_v1";
pub(super) const MAX_STATE_BYTES: usize = 32 * 1024;
pub(super) const MAX_INPUT_BYTES: usize = 4 * 1024;
pub(super) const MAX_UPDATES: usize = 4;
pub(super) const MAX_UPDATE_BYTES: usize = 8 * 1024;
pub(super) const MAX_FILES: usize = 32;
pub(super) const MAX_SELECTOR_BYTES: usize = 512;
pub(super) const MAX_RESULT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum InputOmission {
    TooLarge,
    StorageBudget,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct InputRecord {
    pub event: u64,
    pub digest: String,
    pub original_bytes: u64,
    pub text: Option<String>,
    pub omission: Option<InputOmission>,
}

impl InputRecord {
    fn new(event: u64, input: &str) -> Result<Self, AgentError> {
        if input.len() > MAX_OBSERVED_INPUT_BYTES {
            return Err(context_error());
        }
        let original_bytes = u64::try_from(input.len()).map_err(|_| context_error())?;
        let fits = input.len() <= MAX_INPUT_BYTES;
        Ok(Self {
            event,
            digest: digest(input.as_bytes()),
            original_bytes,
            text: fits.then(|| input.to_owned()),
            omission: (!fits).then_some(InputOmission::TooLarge),
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Freshness {
    Historical,
    Stale,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Operation {
    Read,
    Write,
    Edit,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadObservation {
    pub event: u64,
    pub result_digest: String,
    pub result_bytes: u64,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FileObservation {
    pub selector: String,
    pub event: u64,
    pub operation: Operation,
    pub completed: bool,
    pub freshness: Freshness,
    pub read: Option<ReadObservation>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Omissions {
    pub input_events: u64,
    pub input_texts: u64,
    pub file_selectors: u64,
    pub large_results: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Stored {
    pub version: u8,
    pub scope: String,
    pub sequence: u64,
    pub initial_input: Option<InputRecord>,
    pub updates: Vec<InputRecord>,
    pub last_input: Option<InputStamp>,
    pub files: Vec<FileObservation>,
    pub omissions: Omissions,
    pub compactions: u64,
    pub uncertain_compactions: u64,
}

impl Stored {
    pub fn fresh(scope: &str) -> Self {
        Self {
            version: 1,
            scope: scope.to_owned(),
            sequence: 0,
            initial_input: None,
            updates: Vec::new(),
            last_input: None,
            files: Vec::new(),
            omissions: Omissions::default(),
            compactions: 0,
            uncertain_compactions: 0,
        }
    }

    pub fn load(state: &AgentState, scope: &str) -> Result<Self, AgentError> {
        let Some(value) = state.custom.get(KEY) else {
            return Ok(Self::fresh(scope));
        };
        let mut nodes = 0;
        validate_tree(value, 0, &mut nodes)?;
        if serde_json::to_vec(value)
            .map_err(|_| context_error())?
            .len()
            > MAX_STATE_BYTES
        {
            return Err(context_error());
        }
        let stored: Self = serde_json::from_value(value.clone()).map_err(|_| context_error())?;
        stored.validate()?;
        Ok(stored)
    }

    fn validate(&self) -> Result<(), AgentError> {
        if self.version != 1
            || !valid_digest(&self.scope)
            || self.updates.len() > MAX_UPDATES
            || self.files.len() > MAX_FILES
            || self
                .updates
                .iter()
                .filter_map(|input| input.text.as_ref())
                .map(String::len)
                .sum::<usize>()
                > MAX_UPDATE_BYTES
        {
            return Err(context_error());
        }
        let mut last_event = 0;
        for input in self.initial_input.iter().chain(self.updates.iter()) {
            if input.event == 0
                || input.event > self.sequence
                || input.event <= last_event
                || input.original_bytes > MAX_OBSERVED_INPUT_BYTES as u64
                || !valid_digest(&input.digest)
            {
                return Err(context_error());
            }
            match (&input.text, &input.omission) {
                (Some(text), None)
                    if text.len() <= MAX_INPUT_BYTES
                        && u64::try_from(text.len()).ok() == Some(input.original_bytes)
                        && digest(text.as_bytes()) == input.digest => {}
                (None, Some(InputOmission::TooLarge))
                    if input.original_bytes > MAX_INPUT_BYTES as u64 => {}
                (None, Some(InputOmission::StorageBudget)) => {}
                _ => return Err(context_error()),
            }
            last_event = input.event;
        }
        if let Some(input) = &self.last_input {
            if input.revision == 0 || input.revision > self.sequence {
                return Err(context_error());
            }
            if let Some(last) = self.updates.last().or(self.initial_input.as_ref()) {
                if last.event > input.revision {
                    return Err(context_error());
                }
                if last.event == input.revision && decode_digest(&last.digest)? != input.digest {
                    return Err(context_error());
                }
            }
        } else if self.initial_input.is_some() || !self.updates.is_empty() {
            return Err(context_error());
        }
        let mut selectors = std::collections::BTreeSet::new();
        for file in &self.files {
            if file.selector.is_empty()
                || file.selector.len() > MAX_SELECTOR_BYTES
                || file.event == 0
                || file.event > self.sequence
                || !selectors.insert(&file.selector)
            {
                return Err(context_error());
            }
            if let Some(read) = &file.read {
                if read.event == 0
                    || read.event > file.event
                    || read.limit == 0
                    || read.result_bytes > MAX_RESULT_BYTES as u64
                    || !valid_digest(&read.result_digest)
                {
                    return Err(context_error());
                }
            }
            if file.freshness == Freshness::Historical && (file.read.is_none() || !file.completed) {
                return Err(context_error());
            }
        }
        Ok(())
    }

    pub fn preflight_event(&self) -> Result<(), AgentError> {
        self.sequence.checked_add(1).ok_or_else(context_error)?;
        for count in [
            self.omissions.input_events,
            self.omissions.input_texts,
            self.omissions.file_selectors,
            self.omissions.large_results,
            self.compactions,
            self.uncertain_compactions,
        ] {
            count.checked_add(64).ok_or_else(context_error)?;
        }
        Ok(())
    }

    fn next_event(&mut self) -> Result<u64, AgentError> {
        self.sequence = self.sequence.checked_add(1).ok_or_else(context_error)?;
        Ok(self.sequence)
    }

    pub fn input(&mut self, input: &str) -> Result<InputStamp, AgentError> {
        if input.len() > MAX_OBSERVED_INPUT_BYTES {
            return Err(context_error());
        }
        self.preflight_event()?;
        let event = self.next_event()?;
        let pin = InputRecord::new(event, input)?;
        if pin.text.is_none() {
            increment(&mut self.omissions.input_texts)?;
        }
        let stamp = InputStamp {
            revision: event,
            digest: decode_digest(&pin.digest)?,
        };
        self.last_input = Some(stamp);
        if self.initial_input.is_none() {
            self.initial_input = Some(pin);
        } else {
            self.updates.push(pin);
            while self.updates.len() > MAX_UPDATES
                || self
                    .updates
                    .iter()
                    .filter_map(|input| input.text.as_ref())
                    .map(String::len)
                    .sum::<usize>()
                    > MAX_UPDATE_BYTES
            {
                self.updates.remove(0);
                increment(&mut self.omissions.input_events)?;
            }
        }
        Ok(stamp)
    }

    pub fn changed_scope(&mut self, scope: &str) -> Result<bool, AgentError> {
        if self.scope == scope {
            return Ok(false);
        }
        self.preflight_event()?;
        self.next_event()?;
        self.scope = scope.to_owned();
        self.uncertain_files();
        Ok(true)
    }

    pub fn uncertain_files(&mut self) {
        for file in &mut self.files {
            if file.freshness != Freshness::Stale {
                file.freshness = Freshness::Uncertain;
            }
        }
    }

    pub fn compaction(&mut self, known_change: bool) -> Result<(), AgentError> {
        self.preflight_event()?;
        self.next_event()?;
        if known_change {
            increment(&mut self.compactions)?;
        } else {
            increment(&mut self.uncertain_compactions)?;
        }
        self.uncertain_files();
        Ok(())
    }

    pub fn runtime_activity(&mut self) -> Result<(), AgentError> {
        self.preflight_event()?;
        self.next_event()?;
        self.uncertain_files();
        Ok(())
    }

    pub fn tool(&mut self, call: &ToolCall, outcome: &ToolOutcome) -> Result<(), AgentError> {
        if matches!(
            outcome.meta.error_kind,
            Some(ToolErrorKind::NeedsApproval | ToolErrorKind::DeniedCapability)
        ) {
            return Ok(());
        }
        let operation = match call.name.as_str() {
            "read_file" => Operation::Read,
            "write_file" => Operation::Write,
            "edit_file" => Operation::Edit,
            name if known_observational_tool(name) => return Ok(()),
            _ => {
                self.preflight_event()?;
                self.next_event()?;
                self.uncertain_files();
                return Ok(());
            }
        };
        if operation == Operation::Read && outcome.is_error {
            return Ok(());
        }
        self.preflight_event()?;
        let event = self.next_event()?;
        if operation != Operation::Read {
            self.uncertain_files();
        }
        let Some(selector) = call
            .args
            .get("file_path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty() && path.len() <= MAX_SELECTOR_BYTES)
        else {
            increment(&mut self.omissions.file_selectors)?;
            return Ok(());
        };
        let existing = self.files.iter().position(|file| file.selector == selector);
        let mut file = existing
            .map(|index| self.files.remove(index))
            .unwrap_or_else(|| FileObservation {
                selector: selector.to_owned(),
                event,
                operation,
                completed: false,
                freshness: Freshness::Uncertain,
                read: None,
            });
        file.event = event;
        file.operation = operation;
        file.completed =
            !outcome.is_error && !matches!(outcome.meta.error_kind, Some(ToolErrorKind::Cancelled));
        if operation == Operation::Read {
            if outcome.content.len() <= MAX_RESULT_BYTES {
                file.read = Some(ReadObservation {
                    event,
                    result_digest: digest(outcome.content.as_bytes()),
                    result_bytes: u64::try_from(outcome.content.len())
                        .map_err(|_| context_error())?,
                    offset: call.args.get("offset").and_then(Value::as_u64).unwrap_or(0),
                    limit: call
                        .args
                        .get("limit")
                        .and_then(Value::as_u64)
                        .unwrap_or(100)
                        .max(1),
                });
                file.freshness = if file.completed {
                    Freshness::Historical
                } else {
                    Freshness::Uncertain
                };
            } else {
                file.read = None;
                file.freshness = Freshness::Uncertain;
                increment(&mut self.omissions.large_results)?;
            }
        } else if file.completed && file.read.is_some() {
            file.freshness = Freshness::Stale;
        }
        self.files.push(file);
        while self.files.len() > MAX_FILES {
            self.files.remove(0);
            increment(&mut self.omissions.file_selectors)?;
        }
        Ok(())
    }

    pub fn store(mut self, state: &mut AgentState) -> Result<(), AgentError> {
        self.validate()?;
        loop {
            let bytes = serde_json::to_vec(&self).map_err(|_| context_error())?;
            if bytes.len() <= MAX_STATE_BYTES {
                let value = serde_json::from_slice(&bytes).map_err(|_| context_error())?;
                state.custom.insert(KEY.into(), value);
                return Ok(());
            }
            if !self.files.is_empty() {
                self.files.remove(0);
                increment(&mut self.omissions.file_selectors)?;
            } else if !self.updates.is_empty() {
                self.updates.remove(0);
                increment(&mut self.omissions.input_events)?;
            } else if self
                .initial_input
                .as_ref()
                .is_some_and(|input| input.text.is_some())
            {
                let input = self.initial_input.as_mut().unwrap();
                input.text = None;
                input.omission = Some(InputOmission::StorageBudget);
                increment(&mut self.omissions.input_texts)?;
            } else {
                return Err(context_error());
            }
        }
    }
}

fn known_observational_tool(name: &str) -> bool {
    matches!(
        name,
        "ls" | "glob"
            | "grep"
            | "web_fetch"
            | "web_search"
            | "write_todos"
            | "ask_user"
            | "get_lsp_diagnostics"
            | "get_lsp_definitions"
            | "get_lsp_references"
            | "get_lsp_document_symbols"
            | "gadget_list"
    )
}

fn validate_tree(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), AgentError> {
    *nodes = nodes.checked_add(1).ok_or_else(context_error)?;
    if depth > 8 || *nodes > 2048 {
        return Err(context_error());
    }
    match value {
        Value::String(text) if text.len() > MAX_INPUT_BYTES => Err(context_error()),
        Value::Array(values) => {
            if values.len() > MAX_FILES {
                return Err(context_error());
            }
            for child in values {
                validate_tree(child, depth + 1, nodes)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            if values.len() > 32 || values.keys().any(|key| key.len() > 64) {
                return Err(context_error());
            }
            for child in values.values() {
                validate_tree(child, depth + 1, nodes)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_digest(value: &str) -> Result<[u8; 32], AgentError> {
    if !valid_digest(value) {
        return Err(context_error());
    }
    let mut bytes = [0; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let digit = |byte| match byte {
            b'0'..=b'9' => byte - b'0',
            _ => byte - b'a' + 10,
        };
        bytes[index] = digit(chunk[0]) * 16 + digit(chunk[1]);
    }
    Ok(bytes)
}

fn increment(value: &mut u64) -> Result<(), AgentError> {
    *value = value.checked_add(1).ok_or_else(context_error)?;
    Ok(())
}
