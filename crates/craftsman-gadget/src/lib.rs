//! Author Craftsman gadgets with one attribute and an ordinary Rust build.
//!
//! ```
//! use craftsman_gadget::{gadget_call, GadgetError, JsonSchema};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize, JsonSchema)]
//! struct Words { text: String }
//!
//! #[derive(Serialize)]
//! struct Count { words: usize }
//!
//! /// Count whitespace-separated words in a text.
//! #[gadget_call]
//! fn count_words(args: Words) -> Result<Count, GadgetError> {
//!     Ok(Count { words: args.text.split_whitespace().count() })
//! }
//! ```
//!
//! A gadget crate uses `crate-type = ["cdylib"]` and builds with
//! `cargo build --release --target wasm32-wasip2`. The SDK supplies the WIT
//! exports itself: no second entry-point macro or cargo-component is needed.
//! Native builds expose the same registration, description and dispatch API
//! for tests; filesystem and HTTP host functions are unavailable natively.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::io::Write;

pub use craftsman_gadget_macros::gadget_call;
pub use schemars::JsonSchema;

pub mod host;

/// The versioned ABI, shared with the runtime host.
pub const WIT: &str = include_str!("../wit/gadget.wit");
/// Maximum JSON argument, description and result size in bytes.
pub const MAX_JSON_BYTES: usize = 1024 * 1024;

/// An application or host-function failure returned by a gadget function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GadgetError(pub String);

impl GadgetError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for GadgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GadgetError {}

impl From<String> for GadgetError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for GadgetError {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

/// The three errors in the guest's WIT `call` export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    UnknownFunction(String),
    InvalidArgs(String),
    Failed(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFunction(s) => write!(f, "unknown function: {s}"),
            Self::InvalidArgs(s) => write!(f, "invalid arguments: {s}"),
            Self::Failed(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for CallError {}

/// What the agent reads to decide which gadget function to call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FunctionDescription {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// The JSON document returned by `craftsman:gadget/guest.describe`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Description {
    pub functions: Vec<FunctionDescription>,
}

/// A valid function name. Shared with the host's description validation.
pub fn valid_function_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn registered() -> Result<Vec<&'static __private::Function>, GadgetError> {
    let mut functions: Vec<_> = inventory::iter::<__private::Function>.into_iter().collect();
    functions.sort_unstable_by_key(|f| f.name);
    for pair in functions.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(GadgetError::new(format!(
                "duplicate gadget function: {}",
                pair[0].name
            )));
        }
    }
    Ok(functions)
}

/// Describe the registered functions, sorted by name for stable tool results.
pub fn describe() -> Result<Description, GadgetError> {
    let functions = registered()?
        .into_iter()
        .map(|f| FunctionDescription {
            name: f.name.to_owned(),
            description: f.description.to_owned(),
            input_schema: (f.input_schema)(),
        })
        .collect();
    Ok(Description { functions })
}

/// Dispatch JSON through exactly the same path as a WASM invocation.
pub fn call(function: &str, args: &str) -> Result<String, CallError> {
    if args.len() > MAX_JSON_BYTES {
        return Err(CallError::InvalidArgs("arguments exceed 1 MiB".into()));
    }
    let functions = registered().map_err(|e| CallError::Failed(e.to_string()))?;
    let function = functions
        .into_iter()
        .find(|f| f.name == function)
        .ok_or_else(|| CallError::UnknownFunction(function.to_owned()))?;
    (function.dispatch)(args)
}

/// Implementation details used by the attribute expansion, not author API.
#[doc(hidden)]
pub mod __private {
    pub use super::CallError;
    use super::*;
    pub use inventory;
    pub use serde_json;

    pub struct Function {
        pub name: &'static str,
        pub description: &'static str,
        pub input_schema: fn() -> Value,
        pub dispatch: fn(&str) -> Result<String, CallError>,
    }

    inventory::collect!(Function);

    pub fn parse_args<T: serde::de::DeserializeOwned>(args: &str) -> Result<T, CallError> {
        serde_json::from_str(args).map_err(|e| CallError::InvalidArgs(e.to_string()))
    }

    pub fn parse_no_args(args: &str) -> Result<(), CallError> {
        let value: Value = parse_args(args)?;
        match value {
            Value::Object(fields) if fields.is_empty() => Ok(()),
            _ => Err(CallError::InvalidArgs("expected an empty object".into())),
        }
    }

    pub fn schema_of<T: JsonSchema>() -> Value {
        serde_json::to_value(schemars::schema_for!(T)).expect("a JSON schema is serializable")
    }

    pub fn no_args_schema() -> Value {
        serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false})
    }

    pub fn encode<T: Serialize>(result: Result<T, GadgetError>) -> Result<String, CallError> {
        let result = result.map_err(|e| CallError::Failed(e.to_string()))?;
        bounded_json(&result).map_err(|e| CallError::Failed(e.to_string()))
    }
}

/// Avoid allocating an arbitrarily large serialized result even natively.
fn bounded_json<T: Serialize>(value: &T) -> Result<String, GadgetError> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_JSON_BYTES.saturating_sub(self.0.len()) {
                return Err(std::io::Error::other("JSON output exceeds 1 MiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Bounded(Vec::new());
    serde_json::to_writer(&mut writer, value).map_err(|e| GadgetError::new(e.to_string()))?;
    String::from_utf8(writer.0).map_err(|e| GadgetError::new(e.to_string()))
}

#[cfg(target_arch = "wasm32")]
mod component {
    pub(crate) mod bindings {
        wit_bindgen::generate!({ path: "wit", world: "gadget" });
    }

    struct Gadget;

    impl bindings::exports::craftsman::gadget::guest::Guest for Gadget {
        fn describe() -> String {
            // A malformed registry is reported as a malformed description by
            // the host, never as a partial or ambiguous function list.
            match super::describe().and_then(|d| super::bounded_json(&d)) {
                Ok(description) => description,
                Err(error) => serde_json::json!({"error": error.to_string()}).to_string(),
            }
        }

        fn call(
            function: String,
            args: String,
        ) -> Result<String, bindings::exports::craftsman::gadget::guest::CallError> {
            use bindings::exports::craftsman::gadget::guest::CallError;
            super::call(&function, &args).map_err(|error| match error {
                super::CallError::UnknownFunction(s) => CallError::UnknownFunction(s),
                super::CallError::InvalidArgs(s) => CallError::InvalidArgs(s),
                super::CallError::Failed(s) => CallError::Failed(s),
            })
        }
    }

    // The exports are linked from the SDK rlib into the author's cdylib. The
    // source-built fixture tests this path (including inventory constructors).
    bindings::export!(Gadget with_types_in bindings);
}
