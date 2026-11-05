use craftsman_gadget::{GadgetError, gadget_call, host};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
}

/// Count lines and bytes in one workspace file through the scoped host API.
#[gadget_call]
fn file_stats(args: Input) -> Result<Value, GadgetError> {
    let bytes = host::read_file(&args.path)?;
    let text = std::str::from_utf8(&bytes).map_err(|e| GadgetError::new(e.to_string()))?;
    Ok(json!({"bytes": bytes.len(), "lines": text.lines().count()}))
}
