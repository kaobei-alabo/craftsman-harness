use craftsman_gadget::{GadgetError, gadget_call};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    text: String,
}

/// Count whitespace-separated words without any filesystem or network capability.
#[gadget_call]
fn count_words(args: Input) -> Result<usize, GadgetError> {
    Ok(args.text.split_whitespace().count())
}
