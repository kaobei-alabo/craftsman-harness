//! Synthetic source approved by the dispatched privacy-detector task.
//! Pure detection only: no host calls, IO, environment, aliases, or vault keys.

use craftsman_gadget::{GadgetError, JsonSchema, gadget_call};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const MAX_NODES: usize = 64;
pub const MAX_NODE_BYTES: usize = 64 * 1024;
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_SPANS: usize = 128;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Node {
    id: String,
    text: String,
    input_binding: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Detect {
    version: u32,
    input_binding: String,
    nodes: Vec<Node>,
}

#[derive(Serialize)]
struct Span {
    node_id: String,
    start: usize,
    end: usize,
    category: &'static str,
    input_binding: String,
}

#[derive(Serialize)]
struct Detected {
    version: u32,
    input_binding: String,
    spans: Vec<Span>,
}

fn refuse() -> GadgetError {
    GadgetError::new("privacy detector input or result refused")
}

fn hash_shape(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn validate(args: &Detect) -> Result<(), GadgetError> {
    if args.version != 1 || !hash_shape(&args.input_binding) || args.nodes.len() > MAX_NODES {
        return Err(refuse());
    }
    let mut ids = BTreeSet::new();
    let mut total = 0usize;
    for node in &args.nodes {
        if node.id.is_empty()
            || node.id.len() > 64
            || !node
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:/-".contains(&b))
            || !ids.insert(&node.id)
            || !hash_shape(&node.input_binding)
            || node.text.len() > MAX_NODE_BYTES
        {
            return Err(refuse());
        }
        total = total.checked_add(node.text.len()).ok_or_else(refuse)?;
        if total > MAX_TEXT_BYTES {
            return Err(refuse());
        }
    }
    Ok(())
}

fn email_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"._%+-@".contains(&b)
}

fn email(candidate: &str) -> bool {
    if candidate.len() > 254 {
        return false;
    }
    let Some((local, domain)) = candidate.split_once('@') else {
        return false;
    };
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b))
        || domain.len() > 253
    {
        return false;
    }
    let mut labels = domain.split('.').peekable();
    let mut count = 0;
    while let Some(label) = labels.next() {
        if label.is_empty()
            || label.len() > 63
            || !label.as_bytes()[0].is_ascii_alphanumeric()
            || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return false;
        }
        count += 1;
        if labels.peek().is_none()
            && (label.len() < 2 || !label.bytes().all(|b| b.is_ascii_alphabetic()))
        {
            return false;
        }
    }
    count >= 2
}

fn word_or_address(c: char) -> bool {
    c.is_alphanumeric() || "_@+".contains(c)
}

fn boundaries(text: &str, start: usize, end: usize, phone: bool) -> bool {
    if text[..start]
        .chars()
        .next_back()
        .is_some_and(word_or_address)
        || text[end..].chars().next().is_some_and(word_or_address)
    {
        return false;
    }
    // Refuse a substring of a longer number/address, even at a punctuation cut.
    if phone {
        let before = &text[..start];
        let after = &text[end..];
        if before
            .strip_suffix(['-', '.'])
            .and_then(|s| s.chars().next_back())
            .is_some_and(|c| c.is_ascii_digit())
            || after
                .strip_prefix(['-', '.'])
                .and_then(|s| s.chars().next())
                .is_some_and(|c| c.is_ascii_digit())
        {
            return false;
        }
    }
    true
}

// Exact supported layouts, rather than guessing an arbitrary digit sequence's
// purpose. D means one ASCII digit. Host aliases preserve the actual layout.
const PHONE_LAYOUTS: &[&str] = &[
    "+1 (DDD) DDD-DDDD",
    "+1 DDD-DDD-DDDD",
    "+1 DDD DDD DDDD",
    "+44 20 DDDD DDDD",
    "+886 DDD DDD DDD",
    "+886-D-DDDD-DDDD",
    "(DDD) DDD-DDDD",
    "DDD-DDD-DDDD",
    "DDD.DDD.DDDD",
    "09DD-DDD-DDD",
    "09DD DDD DDD",
    "02-DDDD-DDDD",
];

fn phone_end(text: &str, start: usize) -> Option<usize> {
    let suffix = &text.as_bytes()[start..];
    let mut best = None;
    for layout in PHONE_LAYOUTS {
        if suffix.len() < layout.len()
            || !layout
                .bytes()
                .zip(suffix.iter().copied())
                .all(|(pattern, actual)| {
                    if pattern == b'D' {
                        actual.is_ascii_digit()
                    } else {
                        pattern == actual
                    }
                })
        {
            continue;
        }
        let end = start + layout.len();
        let matched = &text[start..end];
        let digits: Vec<u8> = matched.bytes().filter(u8::is_ascii_digit).collect();
        let nanp = layout.starts_with("+1 ")
            || matches!(*layout, "(DDD) DDD-DDDD" | "DDD-DDD-DDDD" | "DDD.DDD.DDDD");
        let offset = usize::from(layout.starts_with("+1 "));
        if nanp
            && (!(b'2'..=b'9').contains(&digits[offset])
                || !(b'2'..=b'9').contains(&digits[offset + 3]))
        {
            continue;
        }
        if *layout == "+886 DDD DDD DDD" && digits[3] != b'9' {
            continue;
        }
        if *layout == "+886-D-DDDD-DDDD" && !(b'2'..=b'8').contains(&digits[3]) {
            continue;
        }
        if boundaries(text, start, end, true) && best.is_none_or(|previous| previous < end) {
            best = Some(end);
        }
    }
    // A leading '+' with 8..15 contiguous digits is only a layout claim;
    // country allocation, ownership, extensions, and dialability are unknown.
    if suffix.first() == Some(&b'+') {
        let count = suffix[1..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if (8..=15).contains(&count)
            && suffix[1] != b'0'
            && boundaries(text, start, start + 1 + count, true)
        {
            let end = start + 1 + count;
            if best.is_none_or(|previous| previous < end) {
                best = Some(end);
            }
        }
    }
    best
}

fn push(
    spans: &mut Vec<Span>,
    node: &Node,
    start: usize,
    end: usize,
    category: &'static str,
) -> Result<(), GadgetError> {
    if spans.len() == MAX_SPANS {
        return Err(refuse());
    }
    spans.push(Span {
        node_id: node.id.clone(),
        start,
        end,
        category,
        input_binding: node.input_binding.clone(),
    });
    Ok(())
}

fn run(args: Detect) -> Result<Detected, GadgetError> {
    validate(&args)?;
    let mut spans = Vec::new();
    for node in &args.nodes {
        let bytes = node.text.as_bytes();
        let mut position = 0;
        while position < bytes.len() {
            if !node.text.is_char_boundary(position) {
                position += 1;
                continue;
            }
            if email_byte(bytes[position]) && (position == 0 || !email_byte(bytes[position - 1])) {
                let mut end = position + 1;
                while end < bytes.len() && email_byte(bytes[end]) {
                    end += 1;
                }
                // Sentence punctuation is not part of a mailbox's domain.
                let candidate_end = if bytes[end - 1] == b'.' { end - 1 } else { end };
                if email(&node.text[position..candidate_end])
                    && boundaries(&node.text, position, candidate_end, false)
                {
                    push(&mut spans, node, position, candidate_end, "email")?;
                    position = end;
                    continue;
                }
            }
            if let Some(end) = phone_end(&node.text, position) {
                push(&mut spans, node, position, end, "phone")?;
                position = end;
            } else {
                position += 1;
            }
        }
    }
    let result = Detected {
        version: 1,
        input_binding: args.input_binding,
        spans,
    };
    if serde_json::to_vec(&result).map_err(|_| refuse())?.len() > MAX_OUTPUT_BYTES {
        return Err(refuse());
    }
    Ok(result)
}

/// Detect supported email and phone spans in bounded host-selected UTF-8 nodes.
#[gadget_call]
fn detect(args: Detect) -> Result<Detected, GadgetError> {
    run(args)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Probe {
    mode: String,
    request: Detect,
}

/// Emit synthetic adversarial results for host validation; never a policy hook.
#[gadget_call]
fn fixture_adversarial(args: Probe) -> Result<Value, GadgetError> {
    let mut value = serde_json::to_value(run(args.request)?).map_err(|_| refuse())?;
    let first = value["spans"]
        .as_array()
        .and_then(|spans| spans.first())
        .cloned()
        .ok_or_else(refuse)?;
    match args.mode.as_str() {
        "bad_hash" => value["input_binding"] = json!("0".repeat(64)),
        "bad_node_hash" => value["spans"][0]["input_binding"] = json!("0".repeat(64)),
        "unknown_node" => value["spans"][0]["node_id"] = json!("foreign-node"),
        "negative" => value["spans"][0]["start"] = json!(-1),
        "scalar_split" => value["spans"][0]["start"] = json!(1),
        "overflow" => value["spans"][0]["end"] = json!(u64::MAX),
        "reversed" => value["spans"][0]["start"] = json!(first["end"].as_u64().unwrap() + 1),
        "overlap" => value["spans"].as_array_mut().unwrap().insert(1, first),
        "unknown_category" => value["spans"][0]["category"] = json!("secret"),
        "extra_field" => value["spans"][0]["original"] = json!("synthetic-only"),
        "count" => value["spans"] = json!(vec![first; MAX_SPANS + 1]),
        "oversized" => value["padding"] = json!("x".repeat(MAX_OUTPUT_BYTES)),
        "sdk_oversized" => value = json!("x".repeat(craftsman_gadget::MAX_JSON_BYTES)),
        "wrong_category" => value["spans"][0]["category"] = json!("phone"),
        "omit" => value["spans"] = json!([]),
        "spin" => loop {
            std::hint::spin_loop();
        },
        _ => return Err(refuse()),
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str) -> String {
        json!({"version":1,"input_binding":"a".repeat(64),"nodes":[
            {"id":"test","text":text,"input_binding":"b".repeat(64)}
        ]})
        .to_string()
    }

    #[test]
    fn registry_uses_original_json_abi_and_no_other_exports() {
        assert_eq!(
            craftsman_gadget::describe()
                .unwrap()
                .functions
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["detect", "fixture_adversarial"]
        );
        assert!(matches!(
            craftsman_gadget::call("unknown", "{}"),
            Err(craftsman_gadget::CallError::UnknownFunction(_))
        ));
    }

    #[test]
    fn utf8_repeated_values_and_multiple_categories_keep_byte_offsets() {
        let text = "中文🦀 alice@example.invalid; +886 912 345 678; alice@example.invalid.";
        let result: Value =
            serde_json::from_str(&craftsman_gadget::call("detect", &request(text)).unwrap())
                .unwrap();
        let spans = result["spans"].as_array().unwrap();
        assert_eq!(spans.len(), 3);
        let extracted: Vec<_> = spans
            .iter()
            .map(|s| {
                &text[s["start"].as_u64().unwrap() as usize..s["end"].as_u64().unwrap() as usize]
            })
            .collect();
        assert_eq!(
            extracted,
            [
                "alice@example.invalid",
                "+886 912 345 678",
                "alice@example.invalid"
            ]
        );
        assert_eq!(spans[0]["start"], 11);
        assert!(!result.to_string().contains("alice"));
    }

    #[test]
    fn supported_international_and_local_layouts_and_negative_boundaries() {
        let positives = [
            "+12025550142",
            "+1 (202) 555-0142",
            "+1 202-555-0142",
            "+1 202 555 0142",
            "+44 20 7946 0958",
            "+886 912 345 678",
            "+886-2-2345-6789",
            "(202) 555-0142",
            "202-555-0142",
            "202.555.0142",
            "0912-345-678",
            "0912 345 678",
            "02-2345-6789",
        ];
        for text in positives {
            let result: Value =
                serde_json::from_str(&craftsman_gadget::call("detect", &request(text)).unwrap())
                    .unwrap();
            assert_eq!(result["spans"].as_array().unwrap().len(), 1, "{text}");
            assert_eq!(result["spans"][0]["end"], text.len());
        }
        for text in [
            "2025550142",
            "202-555-01423",
            "x202-555-0142",
            "202-555-0142-999",
            "+012345678901",
            "+1234567890123456",
            "alice@@example.invalid",
            "a..b@example.invalid",
            "a@localhost",
            "a@example.1",
            "2026-11-20",
            "192.0.2.10",
            "202-555-0142@example.invalid",
        ] {
            let result: Value =
                serde_json::from_str(&craftsman_gadget::call("detect", &request(text)).unwrap())
                    .unwrap();
            let spans = result["spans"].as_array().unwrap();
            if text.ends_with("@example.invalid") && text.starts_with("202-") {
                assert_eq!(spans.len(), 1);
                assert_eq!(spans[0]["category"], "email");
            } else {
                assert!(spans.is_empty(), "{text}: {result}");
            }
        }
    }

    #[test]
    fn empty_nodes_empty_text_and_absent_values_are_complete_empty_results() {
        let result = craftsman_gadget::call("detect", &request("nothing sensitive 中文")).unwrap();
        assert!(
            serde_json::from_str::<Value>(&result).unwrap()["spans"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let empty = json!({"version":1,"input_binding":"a".repeat(64),"nodes":[]}).to_string();
        assert_eq!(
            serde_json::from_str::<Value>(&craftsman_gadget::call("detect", &empty).unwrap())
                .unwrap()["spans"],
            json!([])
        );
        assert!(craftsman_gadget::call("detect", &request("")).is_ok());
    }

    #[test]
    fn malformed_versions_types_duplicate_ids_and_hash_shapes_refuse() {
        let base: Value = serde_json::from_str(&request("alice@example.invalid")).unwrap();
        for kind in 0..7 {
            let mut input = base.clone();
            match kind {
                0 => input["version"] = json!(2),
                1 => input["nodes"][0]["text"] = json!(7),
                2 => input["nodes"][0]["input_binding"] = json!("f".repeat(63)),
                3 => input["input_binding"] = json!("F".repeat(64)),
                4 => {
                    let node = input["nodes"][0].clone();
                    input["nodes"].as_array_mut().unwrap().push(node);
                }
                5 => input["nodes"][0]["id"] = json!("bad\nnode"),
                _ => input["nodes"][0]["unknown"] = json!(true),
            }
            assert!(
                craftsman_gadget::call("detect", &input.to_string()).is_err(),
                "{kind}"
            );
        }
        assert!(craftsman_gadget::call("detect", "{malformed").is_err());
    }

    #[test]
    fn exact_node_text_total_span_and_sdk_output_limits_are_not_clipping() {
        let base: Value = serde_json::from_str(&request("")).unwrap();
        let mut input = base.clone();
        input["nodes"][0]["text"] = json!("x".repeat(MAX_NODE_BYTES));
        assert!(craftsman_gadget::call("detect", &input.to_string()).is_ok());
        input["nodes"][0]["text"] = json!("x".repeat(MAX_NODE_BYTES + 1));
        assert!(craftsman_gadget::call("detect", &input.to_string()).is_err());
        let mut nodes = Vec::new();
        for i in 0..MAX_NODES {
            nodes.push(json!({"id":format!("node-{i}"),"text":"","input_binding":"b".repeat(64)}));
        }
        input = json!({"version":1,"input_binding":"a".repeat(64),"nodes":nodes});
        assert!(craftsman_gadget::call("detect", &input.to_string()).is_ok());
        input["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"node-overflow","text":"","input_binding":"b".repeat(64)}));
        assert!(craftsman_gadget::call("detect", &input.to_string()).is_err());
        for i in 0..4 {
            input["nodes"][i]["text"] = json!("x".repeat(MAX_NODE_BYTES));
        }
        input["nodes"].as_array_mut().unwrap().pop();
        assert!(craftsman_gadget::call("detect", &input.to_string()).is_ok());
        input["nodes"][4]["text"] = json!("x");
        assert!(craftsman_gadget::call("detect", &input.to_string()).is_err());
        let exact = request(&"alice@example.invalid ".repeat(MAX_SPANS));
        assert_eq!(
            serde_json::from_str::<Value>(&craftsman_gadget::call("detect", &exact).unwrap())
                .unwrap()["spans"]
                .as_array()
                .unwrap()
                .len(),
            MAX_SPANS
        );
        assert!(
            craftsman_gadget::call(
                "detect",
                &request(&"alice@example.invalid ".repeat(MAX_SPANS + 1))
            )
            .is_err()
        );
        let probe = json!({"mode":"sdk_oversized","request":serde_json::from_str::<Value>(&request("alice@example.invalid")).unwrap()});
        assert!(
            matches!(craftsman_gadget::call("fixture_adversarial", &probe.to_string()), Err(craftsman_gadget::CallError::Failed(message)) if message.contains("JSON output exceeds 1 MiB"))
        );
    }
}
