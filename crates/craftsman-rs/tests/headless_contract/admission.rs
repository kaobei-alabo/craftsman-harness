//! Proposed #97 admission vectors, not a runtime meter or complete event checker.
//! Independent native arithmetic/ledger checks; no reference process or requests.
use super::{EVENT, RESULT, VECTORS, compile, materialize};
use serde_json::{Value, json};

const ROLES: [&str; 3] = ["main", "subagent", "compaction"];
const ROW: [&str; 6] = [
    "requests",
    "input_tokens",
    "output_tokens",
    "reported_calls",
    "estimated_calls",
    "unknown_attempts",
];
fn n(value: &Value) -> Option<u128> {
    value.as_u64().map(u128::from)
}
fn integer_counters(v: &Value) -> bool {
    let mut values = vec![
        &v["exit_code"],
        &v["rounds"],
        &v["wall_ms"],
        &v["messages_omitted"],
        &v["permissions"]["denied_calls"],
        &v["limits"]["max_requests"],
        &v["usage"]["input_tokens"],
        &v["usage"]["output_tokens"],
    ];
    for key in ["token_budget", "timeout_ms"] {
        if !v["limits"][key].is_null() {
            values.push(&v["limits"][key]);
        }
    }
    for role in ROLES {
        for key in ROW {
            values.push(&v["usage"]["roles"][role][key]);
        }
    }
    let limit = &v["_meta"]["craftsman"]["limit"];
    values.extend([&limit["maximum"], &limit["observed"]]);
    if let Some(admission) = limit.get("admission") {
        values.extend([
            &admission["estimated_input_tokens"],
            &admission["requested_output_tokens"],
            &admission["remaining_tokens"],
        ]);
    }
    values.iter().all(|value| n(value).is_some())
}
fn result_valid(v: &Value, schema: &jsonschema::Validator) -> bool {
    if !schema.is_valid(v)
        || v["status"] != "limit"
        || v["_meta"]["craftsman"]["limit"]["kind"] != "tokens"
        || !integer_counters(v)
    {
        return false;
    }
    let usage = &v["usage"];
    let mut totals = [0_u128; 6];
    for role in ROLES {
        let row = &usage["roles"][role];
        let values = ROW.map(|key| n(&row[key]).unwrap());
        if values[0] != values[3] + values[4]
            || (values[0] == 0 && (values[1] != 0 || values[2] != 0 || values[5] != 0))
        {
            return false;
        }
        for (total, value) in totals.iter_mut().zip(values) {
            *total += value;
        }
    }
    let source = if totals[3] > 0 && totals[4] > 0 {
        "mixed"
    } else if totals[3] > 0 {
        "reported"
    } else if totals[4] > 0 {
        "estimated"
    } else {
        "none"
    };
    if usage["source"] != source
        || usage["billing_complete"] != (totals[5] == 0)
        || n(&usage["input_tokens"]) != Some(totals[1])
        || n(&usage["output_tokens"]) != Some(totals[2])
        || v["rounds"] != usage["roles"]["main"]["requests"]
        || totals[0] > n(&v["limits"]["max_requests"]).unwrap()
    {
        return false;
    }
    let limit = &v["_meta"]["craftsman"]["limit"];
    let maximum = n(&limit["maximum"]).unwrap();
    let observed = n(&limit["observed"]).unwrap();
    if n(&v["limits"]["token_budget"]) != Some(maximum) || observed != totals[1] + totals[2] {
        return false;
    }
    let Some(admission) = limit.get("admission") else {
        return observed >= maximum;
    };
    if observed >= maximum {
        return false;
    }
    let input = n(&admission["estimated_input_tokens"]).unwrap();
    let output = n(&admission["requested_output_tokens"]).unwrap();
    let remaining = n(&admission["remaining_tokens"]).unwrap();
    remaining == maximum - observed && input + output > remaining
}
fn stream_valid(
    value: &Value,
    event: &jsonschema::Validator,
    result: &jsonschema::Validator,
    prefix: bool,
) -> bool {
    let Some(events) = value.as_array() else {
        return false;
    };
    let Some(first) = events.first() else {
        return false;
    };
    if first["kind"] != "run_started"
        || first["agent_path"] != json!(["main"])
        || events.iter().filter(|e| e["kind"] == "run_started").count() != 1
    {
        return false;
    }
    let finals = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e["kind"] == "result")
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if finals.len() > 1
        || finals.first().is_some_and(|i| *i != events.len() - 1)
        || (finals.is_empty() && !prefix)
    {
        return false;
    }
    let mut ledger = [[0_u128; 6]; 3];
    for (index, e) in events.iter().enumerate() {
        if !event.is_valid(e)
            || n(&e["seq"]) != Some(index as u128)
            || e["run_id"] != first["run_id"]
            || e["thread_id"] != first["thread_id"]
        {
            return false;
        }
        let body = &e["payload"];
        if e["kind"] == "model_usage" {
            let Some(role) = body["role"]
                .as_str()
                .and_then(|r| ROLES.iter().position(|known| *known == r))
            else {
                return false;
            };
            let path = e["agent_path"].as_array().unwrap();
            let valid_path = match role {
                0 => path == &[json!("main")],
                1 => path.len() >= 2 && path.last().unwrap() != "compaction",
                2 => path == &[json!("main"), json!("compaction")],
                _ => false,
            };
            if !valid_path {
                return false;
            }
            let source = match body["source"].as_str() {
                Some("reported") => 3,
                Some("estimated") => 4,
                _ => return false,
            };
            let row = &mut ledger[role];
            row[0] += 1;
            row[source] += 1;
            for (slot, key) in [
                (1, "input_tokens"),
                (2, "output_tokens"),
                (5, "unknown_attempts"),
            ] {
                let Some(number) = n(&body[key]) else {
                    return false;
                };
                row[slot] += number;
            }
        } else if e["kind"] == "result" {
            if !result_valid(body, result)
                || body["run_id"] != first["run_id"]
                || body["thread_id"] != first["thread_id"]
                || body["permissions"]["denied_calls"] != 0
            {
                return false;
            }
            for (role, counts) in ROLES.into_iter().zip(ledger) {
                for (key, count) in ROW.into_iter().zip(counts) {
                    if n(&body["usage"]["roles"][role][key]) != Some(count) {
                        return false;
                    }
                }
            }
            for (key, names) in [
                (
                    "workspace",
                    vec!["cwd_sha256", "trust_workspace", "confinement"],
                ),
                (
                    "features",
                    vec![
                        "gadgets",
                        "project_instructions",
                        "skills",
                        "subagents",
                        "mcp",
                        "lsp",
                        "durable_memory",
                        "background_tasks",
                    ],
                ),
                (
                    "permissions",
                    vec![
                        "mode",
                        "on_unapproved",
                        "allow",
                        "deny",
                        "env_pass",
                        "ignored_ambient_settings",
                    ],
                ),
                ("limits", vec!["max_requests", "token_budget", "timeout_ms"]),
                ("session", vec!["mode"]),
            ] {
                if names
                    .iter()
                    .any(|name| body[key][name] != first["payload"][key][name])
                {
                    return false;
                }
            }
        }
    }
    true
}
fn fixtures() -> Value {
    serde_json::from_str(VECTORS).unwrap()
}

#[test]
fn proposal_vectors_match_native_structure_and_independent_counter_semantics() {
    let result = compile(RESULT);
    let event = compile(EVENT);
    let vectors = fixtures();
    let mut cases = 0;
    let mut valid_cases = 0;
    let mut invalid_cases = 0;
    for case in vectors["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if !name.starts_with("token-admission-") {
            continue;
        }
        let value = materialize(case, &vectors["fixtures"]);
        let (structural, valid) = if case["target"] == "result" {
            (result.is_valid(&value), result_valid(&value, &result))
        } else {
            (
                value.as_array().unwrap().iter().all(|e| event.is_valid(e)),
                stream_valid(
                    &value,
                    &event,
                    &result,
                    case["allow_prefix"].as_bool().unwrap_or(false),
                ),
            )
        };
        assert_eq!(
            structural,
            case["schema_valid"].as_bool().unwrap(),
            "{name}: native structure"
        );
        assert_eq!(
            valid,
            case["valid"].as_bool().unwrap(),
            "{name}: independent native semantics"
        );
        cases += 1;
        if valid {
            valid_cases += 1;
        } else {
            invalid_cases += 1;
        }
    }
    assert_eq!((cases, valid_cases, invalid_cases), (61, 16, 45));
}
#[test]
fn rejected_reservation_does_not_inflate_observed_usage_or_request_counts() {
    let all = fixtures();
    let value = &all["fixtures"]["token_admission_main"];
    let before = value.clone();
    assert!(result_valid(value, &compile(RESULT)));
    assert_eq!(value["_meta"]["craftsman"]["limit"]["observed"], 30);
    assert_eq!(value["usage"]["input_tokens"], 30);
    assert_eq!(value["usage"]["output_tokens"], 0);
    assert_eq!(value["rounds"], 1);
    assert_eq!(value["usage"]["roles"]["main"]["requests"], 1);
    assert_eq!(value["output"], "retained partial output");
    assert_eq!(value["messages_omitted"], 0);
    assert_eq!(*value, before);
}
#[test]
fn initial_admission_refusal_has_zero_calls_empty_output_and_no_usage_event() {
    let all = fixtures();
    let value = &all["fixtures"]["token_admission_initial"];
    assert!(result_valid(value, &compile(RESULT)));
    assert_eq!(value["usage"]["source"], "none");
    assert_eq!(value["rounds"], 0);
    assert_eq!(value["output"], "");
    assert!(value["messages"].as_array().unwrap().is_empty());
    for role in ROLES {
        assert_eq!(value["usage"]["roles"][role]["requests"], 0);
    }
    let stream = &all["fixtures"]["token_admission_initial_stream"];
    assert_eq!(
        stream
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["run_started", "result"]
    );
    assert!(stream_valid(
        stream,
        &compile(EVENT),
        &compile(RESULT),
        false
    ));
}
#[test]
fn admission_after_each_role_preserves_real_role_sums_and_partial_output() {
    let all = fixtures();
    let schema = compile(RESULT);
    for (seed, observed, requests) in [
        ("subagent", 40, 2),
        ("compaction", 28, 2),
        ("all_roles", 44, 3),
    ] {
        let value = &all["fixtures"][format!("token_admission_{seed}")];
        assert!(result_valid(value, &schema));
        assert_eq!(value["_meta"]["craftsman"]["limit"]["observed"], observed);
        assert_eq!(
            ROLES
                .iter()
                .map(|r| value["usage"]["roles"][r]["requests"].as_u64().unwrap())
                .sum::<u64>(),
            requests
        );
        assert_eq!(value["output"], "retained partial output");
    }
    let unknown = &all["fixtures"]["token_admission_unknown_billing"];
    assert!(result_valid(unknown, &schema));
    assert_eq!(unknown["usage"]["source"], "estimated");
    assert_eq!(unknown["usage"]["billing_complete"], false);
}
#[test]
fn reservation_addition_cannot_wrap_u64_into_a_false_fits_decision() {
    let all = fixtures();
    let mut value = all["fixtures"]["token_admission_wide_sum"].clone();
    let schema = compile(RESULT);
    assert!(result_valid(&value, &schema));
    let admission = &value["_meta"]["craftsman"]["limit"]["admission"];
    assert_eq!(
        n(&admission["estimated_input_tokens"]).unwrap()
            + n(&admission["requested_output_tokens"]).unwrap(),
        u128::from(u64::MAX) + 1
    );
    value["_meta"]["craftsman"]["limit"]["admission"]["estimated_input_tokens"] =
        json!(u64::MAX - 1);
    assert!(
        !result_valid(&value, &schema),
        "an exact fit is not a refusal"
    );
}
#[test]
fn exhaustion_and_admission_context_are_nonoverlapping_modes() {
    let all = fixtures();
    let schema = compile(RESULT);
    assert!(result_valid(&all["fixtures"]["token_limit"], &schema));
    assert!(result_valid(
        &all["fixtures"]["reported_token_overrun"],
        &schema
    ));
    let mut no_context = all["fixtures"]["token_admission_main"].clone();
    no_context["_meta"]["craftsman"]["limit"]
        .as_object_mut()
        .unwrap()
        .remove("admission");
    assert!(!result_valid(&no_context, &schema));
    let mut exhausted = all["fixtures"]["token_limit"].clone();
    exhausted["_meta"]["craftsman"]["limit"]["admission"] =
        json!({"estimated_input_tokens":1,"requested_output_tokens":1,"remaining_tokens":0});
    assert!(!result_valid(&exhausted, &schema));
}
#[test]
fn mathematical_schema_integers_do_not_make_float_counter_literals_valid() {
    let all = fixtures();
    let mut value = all["fixtures"]["token_admission_main"].clone();
    let schema = compile(RESULT);
    value["_meta"]["craftsman"]["limit"]["admission"]["estimated_input_tokens"] = json!(80.0);
    assert!(schema.is_valid(&value));
    assert!(!result_valid(&value, &schema));
}
#[test]
fn extra_or_missing_usage_events_cannot_fake_a_refused_request_in_the_ledger() {
    let all = fixtures();
    let event = compile(EVENT);
    let result = compile(RESULT);
    for seed in [
        "token_admission_extra_usage_stream",
        "token_admission_missing_usage_stream",
    ] {
        let value = &all["fixtures"][seed];
        assert!(value.as_array().unwrap().iter().all(|e| event.is_valid(e)));
        assert!(!stream_valid(value, &event, &result, false));
    }
}
