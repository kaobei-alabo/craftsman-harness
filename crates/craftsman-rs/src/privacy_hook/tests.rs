//! Codec and detector-view tests. The reply sources here are fixed host
//! rules for tests only; they are not detectors and select nothing.

use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(crate) const EMAIL: &str = "alice@example.invalid";
pub(crate) const PHONE: &str = "+886 912 345 678";

/// Reports every occurrence of its literal rules, in order, bound to the
/// request it was given. Counts calls and can be told to fail.
pub(crate) struct LiteralSource {
    rules: Mutex<Vec<(String, &'static str)>>,
    failure: Mutex<Option<HookError>>,
    during: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    first_only: AtomicBool,
    pub(crate) calls: AtomicUsize,
}

impl LiteralSource {
    pub(crate) fn new(rules: &[(&str, &'static str)]) -> Arc<Self> {
        let source = Arc::new(Self {
            rules: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
            during: Mutex::new(None),
            first_only: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        source.set_rules(rules);
        source
    }

    pub(crate) fn set_rules(&self, rules: &[(&str, &'static str)]) {
        *self.rules.lock().unwrap() = rules
            .iter()
            .map(|(literal, category)| ((*literal).to_owned(), *category))
            .collect();
    }

    pub(crate) fn fail(&self, failure: Option<HookError>) {
        *self.failure.lock().unwrap() = failure;
    }

    pub(crate) fn during(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.during.lock().unwrap() = Some(Box::new(hook));
    }

    /// Report only the first occurrence of each rule in a node.
    pub(crate) fn first_only(&self, first_only: bool) {
        self.first_only.store(first_only, Ordering::SeqCst);
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ReplySource for LiteralSource {
    async fn reply(&self, request: &Value) -> Result<String, HookError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = self.during.lock().unwrap().as_ref() {
            hook();
        }
        if let Some(failure) = *self.failure.lock().unwrap() {
            return Err(failure);
        }
        let rules = self.rules.lock().unwrap().clone();
        let mut spans = Vec::new();
        for node in request["nodes"].as_array().unwrap() {
            let text = node["text"].as_str().unwrap();
            let mut found = Vec::new();
            for (literal, category) in &rules {
                for (start, matched) in text.match_indices(literal.as_str()) {
                    found.push((start, start + matched.len(), *category));
                    if self.first_only.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
            found.sort();
            let mut end = 0;
            for (start, stop, category) in found {
                if start < end {
                    continue;
                }
                end = stop;
                spans.push(json!({
                    "node_id": node["id"], "start": start, "end": stop,
                    "category": category, "input_binding": node["input_binding"],
                }));
            }
        }
        Ok(
            json!({"version": 1, "input_binding": request["input_binding"], "spans": spans})
                .to_string(),
        )
    }
}

/// Returns one fixed text whatever the request.
struct FixedReply(String);

#[async_trait::async_trait]
impl ReplySource for FixedReply {
    async fn reply(&self, _: &Value) -> Result<String, HookError> {
        Ok(self.0.clone())
    }
}

fn span(request: &Value, node: usize, start: usize, end: usize, category: &str) -> Value {
    let node = &request["nodes"][node];
    json!({
        "node_id": node["id"], "start": start, "end": end,
        "category": category, "input_binding": node["input_binding"],
    })
}

fn reply(request: &Value, spans: Vec<Value>) -> Value {
    json!({"version": 1, "input_binding": request["input_binding"], "spans": spans})
}

fn is_hash(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text.len() == 64
            && text
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

#[test]
fn a_request_binds_every_node_and_splits_only_after_line_feeds() {
    let one = Request::from_text("first 中文🦀 line").unwrap();
    let json = one.to_json();
    assert_eq!(one.node_count(), 1);
    assert_eq!(json["version"], 1);
    assert_eq!(json["nodes"][0]["id"], "t/0");
    assert_eq!(json["nodes"][0]["text"], "first 中文🦀 line");
    assert!(is_hash(&json["input_binding"]) && is_hash(&json["nodes"][0]["input_binding"]));
    // One changed byte changes the node and the request binding.
    let other = Request::from_text("first 中文🦀 lime").unwrap().to_json();
    assert_ne!(json["input_binding"], other["input_binding"]);
    assert_ne!(
        json["nodes"][0]["input_binding"],
        other["nodes"][0]["input_binding"]
    );
    // The same text gives the same bindings.
    assert_eq!(
        json,
        Request::from_text("first 中文🦀 line").unwrap().to_json()
    );

    // 150 KiB in 30 KiB lines: every node ends after a line feed and holds
    // whole lines; the nodes concatenate to the text.
    let line = format!("{}\n", "é".repeat(15 * 1024 - 1) + "x");
    assert_eq!(line.len(), 30 * 1024);
    let text = line.repeat(5);
    let split = Request::from_text(&text).unwrap();
    let json = split.to_json();
    let nodes = json["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 3);
    let mut joined = String::new();
    for (index, node) in nodes.iter().enumerate() {
        let part = node["text"].as_str().unwrap();
        assert!(part.len() <= MAX_NODE_BYTES && part.ends_with('\n'));
        assert_eq!(node["id"], format!("t/{index}"));
        joined.push_str(part);
    }
    assert_eq!(joined, text);

    // Whole-input bounds refuse; nothing is truncated.
    assert_eq!(
        Request::from_text(&"x".repeat(MAX_TEXT_BYTES + 1)).err(),
        Some(HookError::Input)
    );
    assert_eq!(
        Request::from_text(&"x".repeat(MAX_NODE_BYTES + 1)).err(),
        Some(HookError::Input)
    );
    assert!(Request::from_text(&"x".repeat(MAX_NODE_BYTES)).is_ok());
    assert_eq!(Request::from_text("").unwrap().node_count(), 1);
}

#[test]
fn a_valid_reply_returns_checked_spans_in_whole_text_offsets() {
    let first = format!("{}\n", "a".repeat(40 * 1024));
    let second = format!("mail {EMAIL} and {EMAIL}\n");
    let third = format!("{}\ncall {PHONE}.", "b".repeat(30 * 1024));
    let text = format!("{first}{second}{third}");
    let request = Request::from_text(&text).unwrap();
    let json = request.to_json();
    assert_eq!(request.node_count(), 2);
    let node0 = json["nodes"][0]["text"].as_str().unwrap();
    let node1 = json["nodes"][1]["text"].as_str().unwrap();
    let email = node0.find(EMAIL).unwrap();
    let again = node0.rfind(EMAIL).unwrap();
    let phone = node1.find(PHONE).unwrap();
    let spans = request
        .validate_reply(
            &reply(
                &json,
                vec![
                    span(&json, 0, email, email + EMAIL.len(), "email"),
                    span(&json, 0, again, again + EMAIL.len(), "email"),
                    span(&json, 1, phone, phone + PHONE.len(), "phone"),
                ],
            )
            .to_string(),
        )
        .unwrap();
    assert_eq!(spans.len(), 3);
    assert_eq!(&text[spans[0].start..spans[0].end], EMAIL);
    assert_eq!(&text[spans[1].start..spans[1].end], EMAIL);
    assert_eq!(&text[spans[2].start..spans[2].end], PHONE);
    assert_eq!(spans[0].category, Category::Email);
    assert_eq!(spans[2].category, Category::Telephone);
    assert!(spans[0].end <= spans[1].start && spans[1].end <= spans[2].start);
    // An empty report is a valid reply; the host cannot see an omission.
    assert!(
        request
            .validate_reply(&reply(&json, vec![]).to_string())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn every_unbound_or_malformed_reply_is_refused_whole() {
    let text = format!("中文🦀 {EMAIL}; {PHONE}\nsecond {EMAIL}");
    let request = Request::from_text(&text).unwrap();
    let json = request.to_json();
    let email = text.find(EMAIL).unwrap();
    let phone = text.find(PHONE).unwrap();
    let good = || {
        reply(
            &json,
            vec![
                span(&json, 0, email, email + EMAIL.len(), "email"),
                span(&json, 0, phone, phone + PHONE.len(), "phone"),
            ],
        )
    };
    assert_eq!(
        request.validate_reply(&good().to_string()).unwrap().len(),
        2
    );
    let refused = |value: Value| request.validate_reply(&value.to_string()).unwrap_err();

    assert_eq!(
        request.validate_reply("not json").unwrap_err(),
        ReplyRefusal::Shape
    );
    assert_eq!(
        request.validate_reply("[]").unwrap_err(),
        ReplyRefusal::Shape
    );
    let mut extra = good();
    extra["approved"] = json!(true);
    assert_eq!(refused(extra), ReplyRefusal::Shape);
    let mut extra = good();
    extra["spans"][0]["original"] = json!(EMAIL);
    assert_eq!(refused(extra), ReplyRefusal::Shape);
    let mut negative = good();
    negative["spans"][0]["start"] = json!(-1);
    assert_eq!(refused(negative), ReplyRefusal::Shape);
    let mut missing = good();
    missing["spans"][0]
        .as_object_mut()
        .unwrap()
        .remove("category");
    assert_eq!(refused(missing), ReplyRefusal::Shape);

    let mut version = good();
    version["version"] = json!(2);
    assert_eq!(refused(version), ReplyRefusal::Binding);
    let mut batch = good();
    batch["input_binding"] = json!("0".repeat(64));
    assert_eq!(refused(batch), ReplyRefusal::Binding);
    let mut node = good();
    node["spans"][0]["input_binding"] = json!("0".repeat(64));
    assert_eq!(refused(node), ReplyRefusal::Binding);
    // A reply to another request, even for equal-looking text, is unbound.
    let other = Request::from_text(&format!("{text} ")).unwrap().to_json();
    assert_eq!(
        refused(reply(
            &other,
            vec![span(&other, 0, email, email + EMAIL.len(), "email")]
        )),
        ReplyRefusal::Binding
    );

    let mut unknown = good();
    unknown["spans"][0]["node_id"] = json!("t/9");
    assert_eq!(refused(unknown), ReplyRefusal::Node);

    for (start, end) in [
        (email, email),
        (email + 1, email),
        (0, text.len() + 1),
        (1, email + EMAIL.len()), // Splits the first UTF-8 scalar.
    ] {
        let mut range = good();
        range["spans"][0]["start"] = json!(start);
        range["spans"][0]["end"] = json!(end);
        assert_eq!(refused(range), ReplyRefusal::Offset, "{start}..{end}");
    }
    let mut overflow = good();
    overflow["spans"][0]["end"] = json!(u64::MAX);
    assert!(matches!(
        refused(overflow),
        ReplyRefusal::Offset | ReplyRefusal::Shape
    ));

    let mut reversed = good();
    reversed["spans"].as_array_mut().unwrap().swap(0, 1);
    assert_eq!(refused(reversed), ReplyRefusal::Order);
    let mut overlap = good();
    let first = overlap["spans"][0].clone();
    overlap["spans"].as_array_mut().unwrap().insert(1, first);
    assert_eq!(refused(overlap), ReplyRefusal::Order);

    let mut secret = good();
    secret["spans"][0]["category"] = json!("secret");
    assert_eq!(refused(secret), ReplyRefusal::Category);
    // A supported category whose text the vault grammar does not admit.
    let mut wrong = good();
    wrong["spans"][0]["category"] = json!("phone");
    assert_eq!(refused(wrong), ReplyRefusal::Category);
    let mut partial = good();
    partial["spans"][0]["end"] = json!(email + EMAIL.len() - 8);
    assert_eq!(refused(partial), ReplyRefusal::Category);

    let one = span(&json, 0, email, email + EMAIL.len(), "email");
    assert_eq!(
        refused(reply(&json, vec![one.clone(); MAX_SPANS + 1])),
        ReplyRefusal::Count
    );
    let mut padded = good();
    padded["padding"] = json!("x".repeat(MAX_REPLY_BYTES));
    assert_eq!(refused(padded), ReplyRefusal::Output);
}

#[tokio::test]
async fn the_detector_view_returns_checked_spans_or_one_typed_failure() {
    let text = format!("{EMAIL} then {PHONE}");
    let source = LiteralSource::new(&[(EMAIL, "email"), (PHONE, "phone")]);
    let detector = Detector::from_test_source(source.clone());
    let spans = detector.detect(&text).await.unwrap();
    assert_eq!(spans.len(), 2);
    assert_eq!(&text[spans[0].start..spans[0].end], EMAIL);
    assert_eq!(&text[spans[1].start..spans[1].end], PHONE);
    assert_eq!(source.calls(), 1);

    // An inadmissible text never reaches the source.
    assert_eq!(
        detector
            .detect(&"x".repeat(MAX_TEXT_BYTES + 1))
            .await
            .unwrap_err(),
        HookError::Input
    );
    assert_eq!(source.calls(), 1);

    for failure in [HookError::Execution, HookError::Source] {
        source.fail(Some(failure));
        assert_eq!(detector.detect(&text).await.unwrap_err(), failure);
    }

    // Success text and extra claims from a source are not a result.
    for body in [
        "ok".to_owned(),
        json!({"status": "approved", "spans": []}).to_string(),
        json!({"version": 1, "input_binding": "0".repeat(64), "spans": []}).to_string(),
    ] {
        let detector = Detector::from_test_source(Arc::new(FixedReply(body)));
        assert!(matches!(
            detector.detect(&text).await.unwrap_err(),
            HookError::Reply(ReplyRefusal::Shape | ReplyRefusal::Binding)
        ));
    }
    assert_eq!(format!("{detector:?}"), "Detector([private])");
}
