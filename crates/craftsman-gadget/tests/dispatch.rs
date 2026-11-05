use craftsman_gadget::{CallError, GadgetError, JsonSchema, call, describe, gadget_call};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Words {
    text: String,
}

#[derive(Serialize)]
struct Count {
    words: usize,
}

/// Counts whitespace-separated words.
/// Empty text has zero words.
#[gadget_call]
fn count_words(args: Words) -> Result<Count, GadgetError> {
    Ok(Count {
        words: args.text.split_whitespace().count(),
    })
}

#[gadget_call(name = "ping", description = "A zero-argument function.")]
fn rust_ping() -> Result<Value, GadgetError> {
    Ok(json!({"pong": true}))
}

/// Return an application error.
#[gadget_call]
fn failing() -> Result<(), GadgetError> {
    Err(GadgetError::new("expected failure"))
}

/// Try to return an oversized result.
#[gadget_call]
fn enormous() -> Result<String, GadgetError> {
    Ok("x".repeat(craftsman_gadget::MAX_JSON_BYTES))
}

#[test]
fn registration_description_schema_and_overrides() {
    let description = describe().unwrap();
    let names: Vec<_> = description
        .functions
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, ["count_words", "enormous", "failing", "ping"]);
    let words = &description.functions[0];
    assert_eq!(
        words.description,
        "Counts whitespace-separated words.\nEmpty text has zero words."
    );
    assert_eq!(words.input_schema["properties"]["text"]["type"], "string");
    assert_eq!(words.input_schema["required"], json!(["text"]));
    let ping = description.functions.last().unwrap();
    assert_eq!(ping.input_schema["additionalProperties"], false);
}

#[test]
fn dispatches_json_and_distinguishes_errors() {
    let result = call("count_words", r#"{"text":"one two three"}"#).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap(),
        json!({"words": 3})
    );
    assert_eq!(call("ping", "{}").unwrap(), r#"{"pong":true}"#);
    for args in ["null", "[]", "", "{", r#"{"text":12}"#, r#"{"other":"x"}"#] {
        assert!(matches!(
            call("count_words", args),
            Err(CallError::InvalidArgs(_))
        ));
    }
    assert!(matches!(
        call("ping", "null"),
        Err(CallError::InvalidArgs(_))
    ));
    assert!(matches!(
        call("ping", r#"{"unexpected":1}"#),
        Err(CallError::InvalidArgs(_))
    ));
    assert_eq!(
        call("missing", "{}"),
        Err(CallError::UnknownFunction("missing".into()))
    );
    assert_eq!(
        call("failing", "{}"),
        Err(CallError::Failed("expected failure".into()))
    );
}

#[test]
fn oversized_input_and_result_do_not_poison_dispatch() {
    assert!(matches!(
        call("ping", &" ".repeat(craftsman_gadget::MAX_JSON_BYTES + 1)),
        Err(CallError::InvalidArgs(_))
    ));
    assert!(
        matches!(call("enormous", "{}"), Err(CallError::Failed(message)) if message.contains("1 MiB"))
    );
    assert_eq!(call("ping", "{}").unwrap(), r#"{"pong":true}"#);
}

#[test]
fn native_host_functions_do_not_reach_the_machine() {
    assert!(craftsman_gadget::host::read_file("Cargo.toml").is_err());
    assert!(craftsman_gadget::host::list_dir(".").is_err());
    assert!(craftsman_gadget::host::write_file("must-not-exist", b"data").is_err());
    assert!(
        craftsman_gadget::host::http_request(craftsman_gadget::host::Request {
            method: "GET".into(),
            url: "https://example.com".into(),
            headers: vec![],
            body: None,
        })
        .is_err()
    );
}
