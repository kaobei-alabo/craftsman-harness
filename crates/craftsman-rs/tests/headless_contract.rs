//! Accepted-contract schema tests only. No `exec` entry point, model call, credential,
//! filesystem permission enforcement or frozen-protocol change is introduced.

use serde_json::Value;
use std::collections::BTreeSet;

const COMMON: &str = include_str!("../../../docs/schemas/exec-contract-v1.schema.json");
const RESULT: &str = include_str!("../../../docs/schemas/exec-result-v1.schema.json");
const EVENT: &str = include_str!("../../../docs/schemas/exec-events-v1.schema.json");
const VECTORS: &str = include_str!("../../../docs/schemas/exec-v1.vectors.json");
const COMMON_ID: &str = "urn:craftsman-harness:exec-contract:1";

fn compile(wrapper: &str) -> jsonschema::Validator {
    let common: Value = serde_json::from_str(COMMON).unwrap();
    let wrapper: Value = serde_json::from_str(wrapper).unwrap();
    jsonschema::options()
        .with_resource(
            COMMON_ID,
            jsonschema::Resource::from_contents(common).unwrap(),
        )
        .build(&wrapper)
        .expect("local accepted schema compiles without a network retriever")
}
fn get<'a>(mut value: &'a mut Value, path: &[Value]) -> &'a mut Value {
    for part in path {
        value = match part {
            Value::String(name) => value.get_mut(name).unwrap(),
            Value::Number(index) => value.get_mut(index.as_u64().unwrap() as usize).unwrap(),
            _ => panic!("invalid conformance path"),
        };
    }
    value
}
fn materialize(case: &Value, fixtures: &Value) -> Value {
    let mut value = fixtures[case["seed"].as_str().unwrap()].clone();
    for patch in case["patches"].as_array().into_iter().flatten() {
        let path = patch["path"].as_array().unwrap();
        match patch["op"].as_str().unwrap() {
            "append" => get(&mut value, path)
                .as_array_mut()
                .unwrap()
                .push(patch["value"].clone()),
            "set" => {
                let (last, parent) = path.split_last().unwrap();
                let target = get(&mut value, parent);
                match last {
                    Value::String(key) => {
                        target
                            .as_object_mut()
                            .unwrap()
                            .insert(key.clone(), patch["value"].clone());
                    }
                    Value::Number(index) => {
                        target[index.as_u64().unwrap() as usize] = patch["value"].clone()
                    }
                    _ => panic!("invalid conformance path"),
                }
            }
            "remove" => {
                let (last, parent) = path.split_last().unwrap();
                get(&mut value, parent)
                    .as_object_mut()
                    .unwrap()
                    .remove(last.as_str().unwrap())
                    .unwrap();
            }
            _ => panic!("unknown conformance mutation"),
        }
    }
    value
}

#[test]
fn accepted_result_and_event_vectors_match_the_native_schema_engine() {
    let result = compile(RESULT);
    let event = compile(EVENT);
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();
    let mut names = BTreeSet::new();
    let mut positive = 0;
    let mut negative = 0;
    for case in vectors["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        assert!(names.insert(name), "duplicate conformance name {name}");
        let value = materialize(case, &vectors["fixtures"]);
        let valid = if case["target"] == "result" {
            result.is_valid(&value)
        } else {
            value.as_array().unwrap().iter().all(|v| event.is_valid(v))
        };
        assert_eq!(
            valid,
            case["schema_valid"].as_bool().unwrap(),
            "{name}: native JSON Schema and reference vectors disagree"
        );
        if valid {
            positive += 1;
        } else {
            negative += 1;
        }
    }
    assert!(
        names.len() >= 70 && positive > 15 && negative > 10,
        "both acceptance and rejection vectors must be exercised"
    );
}

#[test]
fn standard_stop_reasons_are_not_extended_with_application_failures() {
    let result = compile(RESULT);
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();
    for stop in [
        "provider_failure",
        "approval_required",
        "verification_failed",
        "wall_timeout",
    ] {
        let mut value = vectors["fixtures"]["completed"].clone();
        value["stop_reason"] = Value::String(stop.into());
        assert!(
            !result.is_valid(&value),
            "Craftsman detail {stop} must not become an ACP stop reason"
        );
    }
}

#[test]
fn accepted_wrappers_have_distinct_stable_ids_and_one_local_common_resource() {
    let result: Value = serde_json::from_str(RESULT).unwrap();
    let event: Value = serde_json::from_str(EVENT).unwrap();
    let common: Value = serde_json::from_str(COMMON).unwrap();
    assert_eq!(common["$id"], COMMON_ID);
    assert_eq!(result["$id"], "urn:craftsman-harness:exec-result:1");
    assert_eq!(event["$id"], "urn:craftsman-harness:exec-event:1");
    assert_eq!(result["$ref"], format!("{COMMON_ID}#/$defs/result"));
    assert_eq!(event["$ref"], format!("{COMMON_ID}#/$defs/event"));
    assert_ne!(result["$id"], event["$id"]);
}

// Proposed #97 clarification: independent test-only semantic/ledger checks.
#[path = "headless_contract/admission.rs"]
mod admission;
