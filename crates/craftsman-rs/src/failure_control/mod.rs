//! Explicit app-selected repeated-operation budgets and protected review notes.
//!
//! This module does not classify tools from model-supplied names, stop an agent,
//! dispatch work, grant permissions or read files. The session owner supplies
//! trusted invocation/input observations and performs any actual boundary stop.

pub(crate) mod governor;
pub(crate) mod integrity;

use serde_json::Value;

const INVALID_SETTINGS: &str = "failure_control requires repeat_limit as an integer from 3 to 20 and optional boolean test_review_signals.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    repeat_limit: u8,
    test_review_signals: bool,
}

impl Settings {
    /// Call only on the selected user settings before constructing a session.
    /// Absence is off; malformed input is never echoed in the fixed refusal.
    pub(crate) fn parse(value: Option<&Value>) -> Result<Option<Self>, &'static str> {
        let Some(value) = value else {
            return Ok(None);
        };
        let object = value.as_object().ok_or(INVALID_SETTINGS)?;
        if object.len() > 2
            || object
                .keys()
                .any(|key| !matches!(key.as_str(), "repeat_limit" | "test_review_signals"))
        {
            return Err(INVALID_SETTINGS);
        }
        let repeat_limit = object
            .get("repeat_limit")
            .and_then(Value::as_u64)
            .filter(|limit| (3..=20).contains(limit))
            .ok_or(INVALID_SETTINGS)? as u8;
        let test_review_signals = match object.get("test_review_signals") {
            None => false,
            Some(value) => value.as_bool().ok_or(INVALID_SETTINGS)?,
        };
        Ok(Some(Self {
            repeat_limit,
            test_review_signals,
        }))
    }

    pub(crate) fn repeat_limit(&self) -> u8 {
        self.repeat_limit
    }

    pub(crate) fn test_review_signals(&self) -> bool {
        self.test_review_signals
    }
}

#[cfg(test)]
mod tests;
