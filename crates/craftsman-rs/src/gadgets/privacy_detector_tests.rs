//! Test-only detector collector. Nothing here is a production hook or factory.
//! The ignored native entry point returns before build/config/path inspection
//! unless exactly one registered argv marker selected this synthetic fixture.

use super::manifest::Capabilities;
use super::{ErrorKind, GadgetRuntime, RuntimeOptions};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

const ID: &str = "privacy-detector";
const HELPER: &str = "gadgets::privacy_detector_tests::privacy_detector_native_fixture";
const MARKER: &str = "craftsman-privacy-detector=";
const MAX_NODES: usize = 64;
const MAX_NODE_BYTES: usize = 64 * 1024;
const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_SPANS: usize = 128;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

struct OriginalNode {
    id: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u32,
    input_binding: String,
    spans: Vec<ReturnedSpan>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReturnedSpan {
    node_id: String,
    start: usize,
    end: usize,
    category: String,
    input_binding: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    Input,
    Output,
    Shape,
    Binding,
    Count,
    Node,
    Offset,
    Overlap,
    Category,
    Coverage,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn node_binding(node: &OriginalNode) -> String {
    let mut digest = Sha256::new();
    digest.update(b"craftsman/privacy-detector/node/v1\0");
    digest.update((node.id.len() as u64).to_le_bytes());
    digest.update(node.id.as_bytes());
    digest.update((node.text.len() as u64).to_le_bytes());
    digest.update(node.text.as_bytes());
    format!("{:x}", digest.finalize())
}

fn batch_binding(nodes: &[OriginalNode]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"craftsman/privacy-detector/batch/v1\0");
    digest.update((nodes.len() as u64).to_le_bytes());
    for node in nodes {
        digest.update(node_binding(node).as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn admitted_request(nodes: &[OriginalNode]) -> Result<Value, Refusal> {
    if nodes.len() > MAX_NODES {
        return Err(Refusal::Input);
    }
    let mut total = 0usize;
    let mut ids = BTreeSet::new();
    for node in nodes {
        if node.id.is_empty()
            || node.id.len() > 64
            || node
                .id
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && !"_.:/-".contains(c))
            || !ids.insert(&node.id)
            || node.text.len() > MAX_NODE_BYTES
        {
            return Err(Refusal::Input);
        }
        total = total.checked_add(node.text.len()).ok_or(Refusal::Input)?;
        if total > MAX_TEXT_BYTES {
            return Err(Refusal::Input);
        }
    }
    Ok(json!({
        "version":1,
        "input_binding":batch_binding(nodes),
        "nodes":nodes.iter().map(|node| json!({
            "id":node.id,"text":node.text,"input_binding":node_binding(node)
        })).collect::<Vec<_>>()
    }))
}

// Host-owned rules are deliberately independent of guest code and hashes.
// They describe the fixture's narrow declared formats, not arbitrary PII.
fn mailbox(value: &str) -> bool {
    let parts: Vec<_> = value.split('@').collect();
    if parts.len() != 2 || value.len() > 254 {
        return false;
    }
    let local = parts[0];
    let domain = parts[1];
    if !(1..=64).contains(&local.len())
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || local
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && !"._%+-".contains(c))
        || domain.len() > 253
    {
        return false;
    }
    let labels: Vec<_> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    for label in &labels {
        if !(1..=63).contains(&label.len())
            || !label.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !label.ends_with(|c: char| c.is_ascii_alphanumeric())
            || label
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && c != '-')
        {
            return false;
        }
    }
    let last = labels.last().unwrap();
    last.len() >= 2 && last.chars().all(|c| c.is_ascii_alphabetic())
}

fn telephone(value: &str) -> bool {
    // Contiguous international layout is not proof of a valid assigned number.
    if let Some(digits) = value.strip_prefix('+') {
        if (8..=15).contains(&digits.len())
            && !digits.starts_with('0')
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            return true;
        }
    }
    let layout: String = value
        .chars()
        .map(|c| if c.is_ascii_digit() { 'D' } else { c })
        .collect();
    let digits: Vec<_> = value.bytes().filter(u8::is_ascii_digit).collect();
    let nanp_offset = match layout.as_str() {
        "+D (DDD) DDD-DDDD" | "+D DDD-DDD-DDDD" | "+D DDD DDD DDDD" if value.starts_with("+1 ") => {
            Some(1)
        }
        "(DDD) DDD-DDDD" | "DDD-DDD-DDDD" | "DDD.DDD.DDDD" => Some(0),
        _ => None,
    };
    if let Some(offset) = nanp_offset {
        return (b'2'..=b'9').contains(&digits[offset])
            && (b'2'..=b'9').contains(&digits[offset + 3]);
    }
    match layout.as_str() {
        "+DD DD DDDD DDDD" => value.starts_with("+44 20 "),
        "+DDD DDD DDD DDD" => value.starts_with("+886 9"),
        "+DDD-D-DDDD-DDDD" => value.starts_with("+886-") && (b'2'..=b'8').contains(&digits[3]),
        "DDDD-DDD-DDD" | "DDDD DDD DDD" => value.starts_with("09"),
        "DD-DDDD-DDDD" => value.starts_with("02-"),
        _ => false,
    }
}

fn boundary(text: &str, start: usize, end: usize, is_phone: bool) -> bool {
    let excluded = |c: char| c.is_alphanumeric() || "_@+".contains(c);
    if text[..start].chars().last().is_some_and(excluded)
        || text[end..].chars().next().is_some_and(excluded)
    {
        return false;
    }
    if is_phone {
        let left: Vec<_> = text[..start].chars().rev().take(2).collect();
        let right: Vec<_> = text[end..].chars().take(2).collect();
        if (left.len() == 2 && "-.".contains(left[0]) && left[1].is_ascii_digit())
            || (right.len() == 2 && "-.".contains(right[0]) && right[1].is_ascii_digit())
        {
            return false;
        }
    }
    true
}

fn fixed_host_spans(nodes: &[OriginalNode]) -> Vec<(usize, usize, usize, &'static str)> {
    let mut spans = Vec::new();
    for (node_index, node) in nodes.iter().enumerate() {
        let mut accepted_end = 0;
        for (start, _) in node.text.char_indices() {
            if start < accepted_end {
                continue;
            }
            let address_char = |b: u8| b.is_ascii_alphanumeric() || b"._%+-@".contains(&b);
            if (start == 0 || !address_char(node.text.as_bytes()[start - 1]))
                && address_char(node.text.as_bytes()[start])
            {
                let length = node.text.as_bytes()[start..]
                    .iter()
                    .take_while(|b| address_char(**b))
                    .count();
                let candidate = &node.text[start..start + length];
                let candidate = candidate.strip_suffix('.').unwrap_or(candidate);
                let end = start + candidate.len();
                if mailbox(candidate) && boundary(&node.text, start, end, false) {
                    spans.push((node_index, start, end, "email"));
                    accepted_end = end;
                    continue;
                }
            }
            // Examine only possible complete layout lengths; no quadratic scan.
            let mut best = None;
            for length in 9..=17 {
                let Some(end) = start.checked_add(length) else {
                    continue;
                };
                if end <= node.text.len()
                    && node.text.is_char_boundary(end)
                    && telephone(&node.text[start..end])
                    && boundary(&node.text, start, end, true)
                {
                    best = Some(end);
                }
            }
            if let Some(end) = best {
                spans.push((node_index, start, end, "phone"));
                accepted_end = end;
            }
        }
    }
    spans
}

fn validate_returned(nodes: &[OriginalNode], json: &str) -> Result<Reply, Refusal> {
    admitted_request(nodes)?;
    if json.len() > MAX_OUTPUT_BYTES {
        return Err(Refusal::Output);
    }
    let reply: Reply = serde_json::from_str(json).map_err(|_| Refusal::Shape)?;
    if reply.version != 1 || reply.input_binding != batch_binding(nodes) {
        return Err(Refusal::Binding);
    }
    if reply.spans.len() > MAX_SPANS {
        return Err(Refusal::Count);
    }
    let mut previous = None;
    let mut accepted = Vec::new();
    for span in &reply.spans {
        let index = nodes
            .iter()
            .position(|node| node.id == span.node_id)
            .ok_or(Refusal::Node)?;
        let node = &nodes[index];
        if span.input_binding != node_binding(node) {
            return Err(Refusal::Binding);
        }
        if span.start >= span.end
            || span.end > node.text.len()
            || !node.text.is_char_boundary(span.start)
            || !node.text.is_char_boundary(span.end)
        {
            return Err(Refusal::Offset);
        }
        if let Some((prior_index, prior_end)) = previous {
            if index < prior_index || (index == prior_index && span.start < prior_end) {
                return Err(Refusal::Overlap);
            }
        }
        let value = &node.text[span.start..span.end];
        let category = match span.category.as_str() {
            "email" if mailbox(value) && boundary(&node.text, span.start, span.end, false) => {
                "email"
            }
            "phone" if telephone(value) && boundary(&node.text, span.start, span.end, true) => {
                "phone"
            }
            _ => return Err(Refusal::Category),
        };
        accepted.push((index, span.start, span.end, category));
        previous = Some((index, span.end));
    }
    // An approved gadget is still not the sole authority. This independent
    // fixture rule scan refuses missing matches rather than trusting empty spans.
    if accepted != fixed_host_spans(nodes) {
        return Err(Refusal::Coverage);
    }
    Ok(reply)
}

fn selected(arguments: impl IntoIterator<Item = OsString>) -> Option<&'static str> {
    let values: Vec<_> = arguments
        .into_iter()
        .filter_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix(MARKER))
                .map(str::to_owned)
        })
        .collect();
    match values.as_slice() {
        [value] if value == "suite" => Some("suite"),
        [value] if value == "collect" => Some("collect"),
        _ => None,
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn sources(root: &Path) -> BTreeMap<String, String> {
    let mut hashes = BTreeMap::new();
    for path in [
        "Cargo.lock",
        "Cargo.toml",
        "rust-toolchain.toml",
        "crates/craftsman-gadget/Cargo.toml",
        "crates/craftsman-gadget/src/lib.rs",
        "crates/craftsman-gadget/src/host.rs",
        "crates/craftsman-gadget/wit/gadget.wit",
        "crates/craftsman-gadget-macros/Cargo.toml",
        "crates/craftsman-gadget-macros/src/lib.rs",
        "crates/craftsman-rs/tests/fixtures/privacy-detector/Cargo.toml",
        "crates/craftsman-rs/tests/fixtures/privacy-detector/Cargo.lock",
        "crates/craftsman-rs/tests/fixtures/privacy-detector/gadget.toml",
        "crates/craftsman-rs/tests/fixtures/privacy-detector/src/lib.rs",
        "crates/craftsman-rs/src/gadgets/privacy_detector_tests.rs",
    ] {
        hashes.insert(
            path.to_owned(),
            hash(&std::fs::read(root.join(path)).unwrap()),
        );
    }
    hashes
}

fn create_record(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn build_fixture(root: &Path) -> (Vec<u8>, std::process::Output) {
    // Trusted source build is separate from runtime consent. Offline and locked;
    // no project hook, SDK materialization, guest execution, or installer here.
    let output = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "--offline",
            "--locked",
            "--release",
            "--target",
            "wasm32-wasip2",
            "--manifest-path",
        ])
        .arg(root.join("crates/craftsman-rs/tests/fixtures/privacy-detector/Cargo.toml"))
        .arg("--target-dir")
        .arg(root.join("target/privacy-detector-wasm"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "trusted detector fixture build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes =
        std::fs::read(root.join(
            "target/privacy-detector-wasm/wasm32-wasip2/release/privacy_detector_fixture.wasm",
        ))
        .unwrap();
    (bytes, output)
}

struct Collector {
    runtime: Arc<GadgetRuntime>,
    bundle: PathBuf,
    image_hash: String,
    manifest_hash: String,
    records: Vec<Value>,
}

impl Collector {
    fn still_bound(&self) -> bool {
        hash(&std::fs::read(self.bundle.join("gadget.wasm")).unwrap()) == self.image_hash
            && hash(&std::fs::read(self.bundle.join("gadget.toml")).unwrap()) == self.manifest_hash
    }

    async fn actual_call(&mut self, function: &str, arguments: Value) -> Result<String, ErrorKind> {
        assert!(
            self.still_bound(),
            "test-owned source/image binding changed before call"
        );
        let started = Instant::now();
        let result = self.runtime.call(ID, function, arguments.clone()).await;
        assert!(
            self.still_bound(),
            "test-owned source/image binding changed after call"
        );
        match result {
            Ok(result) => {
                assert_eq!(result.network_calls, 0);
                assert!(result.logs.is_empty());
                assert!(!result.logs_truncated);
                self.records.push(json!({
                    "function":function,"arguments":arguments,"result_json":result.json,
                    "component_sha256":self.image_hash,"manifest_sha256":self.manifest_hash,
                    "duration_ns":started.elapsed().as_nanos().to_string(),
                    "network_calls":result.network_calls,"logs":result.logs,
                    "logs_truncated":result.logs_truncated
                }));
                Ok(result.json)
            }
            Err(error) => {
                assert_eq!(error.network_calls, 0);
                assert!(!error.logs_truncated);
                self.records.push(json!({
                    "function":function,"arguments":arguments,"error_kind":error.kind,
                    "component_sha256":self.image_hash,"manifest_sha256":self.manifest_hash,
                    "duration_ns":started.elapsed().as_nanos().to_string(),
                    "network_calls":error.network_calls,"logs_truncated":error.logs_truncated
                }));
                Err(error.kind)
            }
        }
    }
}

fn nodes(items: &[(&str, &str)]) -> Vec<OriginalNode> {
    items
        .iter()
        .map(|(id, text)| OriginalNode {
            id: (*id).to_owned(),
            text: (*text).to_owned(),
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "synthetic native collector; exactly one argv marker required before build/config access"]
async fn privacy_detector_native_fixture() {
    let Some(mode) = selected(std::env::args_os()) else {
        println!("PRIVACY-DETECTOR-FIXTURE:NOT-SELECTED");
        return;
    };
    let root = root();
    let original_sources = sources(&root);
    let (image, build) = build_fixture(&root);
    assert_eq!(original_sources, sources(&root));
    let home = tempfile::tempdir().unwrap();
    let workspace = home.path().join("workspace");
    let config = home.path().join("owned-config");
    let bundle = config.join("gadgets/privacy-detector");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&bundle).unwrap();
    let sentinel = b"synthetic sentinel; guest has no read or write authority";
    std::fs::write(workspace.join("sentinel.txt"), sentinel).unwrap();
    let manifest =
        std::fs::read(root.join("crates/craftsman-rs/tests/fixtures/privacy-detector/gadget.toml"))
            .unwrap();
    std::fs::write(bundle.join("gadget.toml"), &manifest).unwrap();
    std::fs::write(bundle.join("gadget.wasm"), &image).unwrap();
    let options = RuntimeOptions {
        disk_cache: false,
        ..RuntimeOptions::default()
    };
    assert_eq!(options.timeout, Duration::from_secs(30));
    assert_eq!(options.memory_bytes, 256 * 1024 * 1024);
    assert!(!options.interactive);
    assert!(!options.trust_workspace);
    let unapproved = GadgetRuntime::new(&workspace, &config, options.clone()).unwrap();
    let list = unapproved.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].status, "needs-approval");
    assert_eq!(list[0].capabilities, Capabilities::default());
    assert!(list[0].functions.is_empty());
    assert_eq!(unapproved.compiled_count(), 0);
    let input = nodes(&[(
        "request/0/text",
        "中文🦀 alice@example.invalid; +886 912 345 678; alice@example.invalid.",
    )]);
    let request = admitted_request(&input).unwrap();
    assert_eq!(
        unapproved
            .call(ID, "detect", request.clone())
            .await
            .unwrap_err()
            .kind,
        ErrorKind::NeedsApproval
    );
    assert_eq!(unapproved.compiled_count(), 0);
    drop(unapproved);
    // This task explicitly approves only this synthetic source-built image.
    // Ephemeral allowed-id is a legacy runtime test input, not required policy.
    let approved_options = RuntimeOptions {
        allowed_gadgets: BTreeSet::from([ID.to_owned()]),
        ..options
    };
    let runtime = Arc::new(GadgetRuntime::new(&workspace, &config, approved_options).unwrap());
    let describe_started = Instant::now();
    let described = runtime.list().await.unwrap();
    let describe_ns = describe_started.elapsed().as_nanos().to_string();
    assert_eq!(described[0].status, "ready");
    assert_eq!(described[0].capabilities, Capabilities::default());
    assert_eq!(
        described[0]
            .functions
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        ["detect", "fixture_adversarial"]
    );
    assert_eq!(
        described[0].component_hash.as_deref(),
        Some(hash(&image).as_str())
    );
    assert_eq!(
        described[0].manifest_hash.as_deref(),
        Some(hash(&manifest).as_str())
    );
    let mut collector = Collector {
        runtime,
        bundle,
        image_hash: hash(&image),
        manifest_hash: hash(&manifest),
        records: Vec::new(),
    };
    let result = collector
        .actual_call("detect", request.clone())
        .await
        .unwrap();
    let reply = validate_returned(&input, &result).unwrap();
    assert_eq!(reply.spans.len(), 3);
    assert_eq!(reply.spans[0].start, 11);
    assert_eq!(reply.spans[0].category, "email");
    assert_eq!(reply.spans[1].category, "phone");
    assert_eq!(reply.spans[2].category, "email");
    assert!(!result.contains("alice"));
    let multi = nodes(&[
        ("file/0/content", "alice@example.invalid and (202) 555-0142"),
        ("file/1/old_string", "中文 alice@example.invalid"),
        ("file/1/new_string", "+44 20 7946 0958 / 0912-345-678"),
        ("empty", ""),
    ]);
    let multi_result = collector
        .actual_call("detect", admitted_request(&multi).unwrap())
        .await
        .unwrap();
    assert_eq!(
        validate_returned(&multi, &multi_result)
            .unwrap()
            .spans
            .len(),
        5
    );
    for empty in [
        nodes(&[]),
        nodes(&[("empty", "")]),
        nodes(&[("plain", "No selected data. 中文")]),
    ] {
        let result = collector
            .actual_call("detect", admitted_request(&empty).unwrap())
            .await
            .unwrap();
        assert!(validate_returned(&empty, &result).unwrap().spans.is_empty());
    }
    let layouts = "+12025550142; +1 (202) 555-0142; +1 202-555-0142; +1 202 555 0142; +44 20 7946 0958; +886 912 345 678; +886-2-2345-6789; (202) 555-0142; 202-555-0142; 202.555.0142; 0912-345-678; 0912 345 678; 02-2345-6789";
    let all = nodes(&[("layouts", layouts)]);
    let result = collector
        .actual_call("detect", admitted_request(&all).unwrap())
        .await
        .unwrap();
    assert_eq!(validate_returned(&all, &result).unwrap().spans.len(), 13);
    let negative = nodes(&[(
        "negative",
        "2025550142; x202-555-0142; 202-555-01423; 202-555-0142-999; +012345678901; +1234567890123456; alice@@example.invalid; a..b@example.invalid; a@localhost; a@example.1; 2026-11-20; 192.0.2.10",
    )]);
    let result = collector
        .actual_call("detect", admitted_request(&negative).unwrap())
        .await
        .unwrap();
    assert!(
        validate_returned(&negative, &result)
            .unwrap()
            .spans
            .is_empty()
    );
    let repeat = nodes(&[("repeated", &"alice@example.invalid ".repeat(MAX_SPANS))]);
    let result = collector
        .actual_call("detect", admitted_request(&repeat).unwrap())
        .await
        .unwrap();
    assert_eq!(
        validate_returned(&repeat, &result).unwrap().spans.len(),
        MAX_SPANS
    );
    let over = nodes(&[("repeated", &"alice@example.invalid ".repeat(MAX_SPANS + 1))]);
    assert_eq!(
        collector
            .actual_call("detect", admitted_request(&over).unwrap())
            .await
            .unwrap_err(),
        ErrorKind::Failed
    );
    let full = (0..4)
        .map(|index| OriginalNode {
            id: format!("full/{index}"),
            text: "x".repeat(MAX_NODE_BYTES),
        })
        .collect::<Vec<_>>();
    let result = collector
        .actual_call("detect", admitted_request(&full).unwrap())
        .await
        .unwrap();
    assert!(validate_returned(&full, &result).unwrap().spans.is_empty());
    let exact_nodes = (0..MAX_NODES)
        .map(|index| OriginalNode {
            id: format!("many/{index}"),
            text: "alice@example.invalid".into(),
        })
        .collect::<Vec<_>>();
    let result = collector
        .actual_call("detect", admitted_request(&exact_nodes).unwrap())
        .await
        .unwrap();
    assert_eq!(
        validate_returned(&exact_nodes, &result)
            .unwrap()
            .spans
            .len(),
        MAX_NODES
    );
    let mut too_many = admitted_request(&exact_nodes).unwrap();
    let extra = json!({"id":"many/overflow","text":"","input_binding":"a".repeat(64)});
    too_many["nodes"].as_array_mut().unwrap().push(extra);
    assert_eq!(
        collector.actual_call("detect", too_many).await.unwrap_err(),
        ErrorKind::Failed
    );
    let mut oversized_node = request.clone();
    oversized_node["nodes"][0]["text"] = json!("x".repeat(MAX_NODE_BYTES + 1));
    assert_eq!(
        collector
            .actual_call("detect", oversized_node)
            .await
            .unwrap_err(),
        ErrorKind::Failed
    );
    let mut oversized_total = admitted_request(&full).unwrap();
    oversized_total["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"full/overflow","text":"x","input_binding":"b".repeat(64)}));
    assert_eq!(
        collector
            .actual_call("detect", oversized_total)
            .await
            .unwrap_err(),
        ErrorKind::Failed
    );
    for (probe, expected) in [
        ("bad_hash", Refusal::Binding),
        ("bad_node_hash", Refusal::Binding),
        ("unknown_node", Refusal::Node),
        ("negative", Refusal::Shape),
        ("scalar_split", Refusal::Offset),
        ("overflow", Refusal::Offset),
        ("reversed", Refusal::Offset),
        ("overlap", Refusal::Overlap),
        ("unknown_category", Refusal::Category),
        ("extra_field", Refusal::Shape),
        ("count", Refusal::Count),
        ("oversized", Refusal::Output),
        ("wrong_category", Refusal::Category),
        ("omit", Refusal::Coverage),
    ] {
        let arguments = json!({"mode":probe,"request":request});
        let result = collector
            .actual_call("fixture_adversarial", arguments)
            .await
            .unwrap();
        assert_eq!(
            validate_returned(&input, &result).err(),
            Some(expected),
            "{probe}"
        );
    }
    assert_eq!(
        collector
            .actual_call(
                "fixture_adversarial",
                json!({"mode":"sdk_oversized","request":request})
            )
            .await
            .unwrap_err(),
        ErrorKind::OutputLimit
    );
    assert_eq!(
        collector
            .actual_call("detect", json!({"version":1,"input_binding":7,"nodes":[]}))
            .await
            .unwrap_err(),
        ErrorKind::InvalidArgs
    );
    assert_eq!(
        collector
            .actual_call("unknown", json!({}))
            .await
            .unwrap_err(),
        ErrorKind::NotFound
    );
    let spin_started = Instant::now();
    let timed = tokio::time::timeout(
        Duration::from_secs(45),
        collector.actual_call(
            "fixture_adversarial",
            json!({"mode":"spin","request":request}),
        ),
    )
    .await
    .expect("actual runtime must return within new outer fixture bound");
    assert_eq!(timed.unwrap_err(), ErrorKind::Timeout);
    assert!(spin_started.elapsed() >= Duration::from_secs(30));
    // A later ordinary call is real; timeout did not fabricate or reuse output.
    let result = collector
        .actual_call("detect", request.clone())
        .await
        .unwrap();
    assert_eq!(validate_returned(&input, &result).unwrap().spans.len(), 3);
    assert_eq!(
        std::fs::read(workspace.join("sentinel.txt")).unwrap(),
        sentinel
    );
    assert!(!config.join("gadgets/.grants.json").exists());
    assert!(!config.join("gadgets/.grants.lock").exists());
    assert_eq!(original_sources, sources(&root));
    // Mutated activation bindings refuse before another guest call.
    let call_count = collector.records.len();
    std::fs::write(
        collector.bundle.join("gadget.wasm"),
        b"changed synthetic image",
    )
    .unwrap();
    assert!(!collector.still_bound());
    assert_eq!(collector.records.len(), call_count);
    let details = json!({
        "schema":"craftsman-harness/privacy-detector-fixture@1",
        "scope":"synthetic_detector_only",
        "base_commit":"29349c5ea0284e194fd7bfa744290095593f1f13",
        "base_tree":"93748936c055b2b22e4164d8b0ef9175ba6f2b97",
        "os":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "rust_toolchain":"1.99.1","wasmtime":"48.0.8","abi":"craftsman:gadget@0.1.0",
        "component_sha256":collector.image_hash,"component_bytes":image.len(),
        "manifest_sha256":collector.manifest_hash,"source_sha256":original_sources,
        "describe_via_approved_list_ns":describe_ns,"described":described,
        "native_test_binary_sha256":hash(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),
        "runtime_timeout_seconds":30,"runtime_memory_bytes":256 * 1024 * 1024,
        "capabilities":{"read":[],"write":[],"http":[]},"disk_cache":false,
        "gadget_source_approved_by":"dispatched task synthetic-source approval",
        "runtime_activation":"ephemeral test-only allow-id plus independently checked image/manifest",
        "actual_calls":collector.records.len(),"adversarial_host_refusals":14,
        "production_hooks_wired":false,"model_requests":0,"vault_keys_used":false,
        "unknown_data_confidentiality_claim":false,"all_platform_gates_cleared":false
    });
    if mode == "collect" {
        let report = root.join("target/verification/privacy-hook-detector");
        std::fs::create_dir_all(report.join("calls")).unwrap();
        std::fs::create_dir_all(report.join("image")).unwrap();
        create_record(&report.join("image/privacy-detector.wasm"), &image);
        create_record(&report.join("image/gadget.toml"), &manifest);
        create_record(
            &report.join("calls/actual-calls.json"),
            &serde_json::to_vec_pretty(&collector.records).unwrap(),
        );
        create_record(
            &report.join("calls/describe-and-bindings.json"),
            &serde_json::to_vec_pretty(&details).unwrap(),
        );
        create_record(
            &report.join("logs/fixture-wasm-build.stdout"),
            &build.stdout,
        );
        create_record(
            &report.join("logs/fixture-wasm-build.stderr"),
            &build.stderr,
        );
    }
    println!(
        "PRIVACY-DETECTOR-FIXTURE:COMPLETE calls={} adversarial=14",
        collector.records.len()
    );
}

#[test]
fn absent_unknown_duplicate_markers_return_before_fixture_selection() {
    assert_eq!(selected([]), None);
    assert_eq!(selected([OsString::from(format!("{MARKER}unknown"))]), None);
    assert_eq!(
        selected([
            OsString::from(format!("{MARKER}suite")),
            OsString::from(format!("{MARKER}suite"))
        ]),
        None
    );
    assert_eq!(
        selected([OsString::from(format!("{MARKER}collect"))]),
        Some("collect")
    );
}

#[test]
fn host_bindings_cover_original_utf8_id_order_content_and_not_guest_claims() {
    let input = nodes(&[
        ("a", "中文 alice@example.invalid"),
        ("b", "alice@example.invalid"),
    ]);
    let request = admitted_request(&input).unwrap();
    assert_ne!(
        request["nodes"][0]["input_binding"],
        request["nodes"][1]["input_binding"]
    );
    let changed = nodes(&[
        ("a", "中文 alice@example.invalid!"),
        ("b", "alice@example.invalid"),
    ]);
    assert_ne!(batch_binding(&input), batch_binding(&changed));
    let reordered = nodes(&[
        ("b", "alice@example.invalid"),
        ("a", "中文 alice@example.invalid"),
    ]);
    assert_ne!(batch_binding(&input), batch_binding(&reordered));
    assert!(validate_returned(&input, "{malformed").is_err());
    let false_claim = json!({"version":1,"input_binding":"0".repeat(64),"spans":[]});
    assert_eq!(
        validate_returned(&input, &false_claim.to_string()).err(),
        Some(Refusal::Binding)
    );
    let missed = json!({"version":1,"input_binding":batch_binding(&input),"spans":[]});
    assert_eq!(
        validate_returned(&input, &missed.to_string()).err(),
        Some(Refusal::Coverage)
    );
}

#[test]
fn host_whole_input_limits_and_ambiguous_data_are_not_empty_success() {
    assert!(admitted_request(&nodes(&[("a", &"x".repeat(MAX_NODE_BYTES))])).is_ok());
    assert!(admitted_request(&nodes(&[("a", &"x".repeat(MAX_NODE_BYTES + 1))])).is_err());
    assert!(admitted_request(&nodes(&[("a", ""), ("a", "")])).is_err());
    assert!(!mailbox("alice@@example.invalid"));
    assert!(!telephone("2025550142"));
    assert!(!telephone("192.0.2.10"));
    assert!(!telephone("2026-11-20"));
    assert!(!boundary("x202-555-0142", 1, 13, true));
}

#[tokio::test]
async fn source_built_privacy_detector_passes_marked_actual_native_runtime() {
    let home = tempfile::tempdir().unwrap();
    let cargo_home = std::env::var_os("CARGO_HOME")
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo").into_os_string())
        })
        .expect("explicit readonly Cargo source cache for trusted fixture build");
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", HELPER, "--nocapture"])
        .arg(format!("{MARKER}suite"))
        .env_clear()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("TMPDIR", home.path())
        .env("TEMP", home.path())
        .env("TMP", home.path())
        .env("CARGO_HOME", cargo_home)
        .env(
            "PATH",
            std::env::var_os("PATH").expect("actual toolchain PATH"),
        )
        .env(
            "RUSTC",
            Path::new(env!("CARGO")).with_file_name(if cfg!(windows) {
                "rustc.exe"
            } else {
                "rustc"
            }),
        )
        .env(
            "RUSTDOC",
            Path::new(env!("CARGO")).with_file_name(if cfg!(windows) {
                "rustdoc.exe"
            } else {
                "rustdoc"
            }),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.env(
        "SystemRoot",
        std::env::var_os("SystemRoot").expect("native SystemRoot for trusted compiler"),
    );
    let mut child = command.spawn().unwrap();
    let mut stdout_reader = child.stdout.take().unwrap().take(65537);
    let mut stderr_reader = child.stderr.take().unwrap().take(65537);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let completed = tokio::time::timeout(Duration::from_secs(240), async {
        tokio::join!(
            stdout_reader.read_to_end(&mut stdout),
            stderr_reader.read_to_end(&mut stderr),
            child.wait()
        )
    })
    .await;
    let (out_read, err_read, status) = match completed {
        Ok(completed) => completed,
        Err(_) => {
            child.start_kill().unwrap();
            tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .expect("new fixture cleanup did not return")
                .unwrap();
            panic!("new detector native fixture exceeded full setup/actual-call outer bound");
        }
    };
    out_read.unwrap();
    err_read.unwrap();
    let status = status.unwrap();
    assert!(stdout.len() <= 64 * 1024 && stderr.len() <= 64 * 1024);
    assert!(
        status.success(),
        "detector native fixture failed: {} {}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    let stdout = std::str::from_utf8(&stdout).unwrap();
    assert!(stdout.contains("PRIVACY-DETECTOR-FIXTURE:COMPLETE calls="));
    assert!(stdout.contains("adversarial=14"));
    assert!(!stdout.contains("NOT-SELECTED"));
}
