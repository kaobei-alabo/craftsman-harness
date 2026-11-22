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
        .envs(compiler_environment::build_environment())
        .env_remove(compiler_environment::TRANSPORT_X86)
        .env_remove(compiler_environment::TRANSPORT_NATIVE)
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
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    compiler_environment::isolated_supervisor_environment(
        command.as_std_mut(),
        home.path(),
        &cargo_home,
        &std::env::var_os("PATH").expect("actual toolchain PATH"),
        compiler_environment::system_root(),
        compiler_environment::host_discovery(),
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

// Additive: the production pinned call path and admission owner, run against
// this module's source-built detector. Nothing above or below is changed.
mod pinned_hook;
// Additive: one privacy conversation per model wire through the shared
// session builder, with this module's source-built detector approved and
// pinned. Nothing above or below is changed.
mod restore_wires;
mod view_wires;

mod offline_lock_tests {
    use super::root;
    use serde::Deserialize;
    use std::collections::{BTreeMap, BTreeSet};

    type Identity = (String, String, Option<String>);

    #[derive(Clone, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Lock {
        version: u32,
        package: Vec<Package>,
    }

    #[derive(Clone, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Package {
        name: String,
        version: String,
        source: Option<String>,
        checksum: Option<String>,
        #[serde(default)]
        dependencies: Vec<String>,
    }

    fn identity(package: &Package) -> Identity {
        (
            package.name.clone(),
            package.version.clone(),
            package.source.clone(),
        )
    }

    fn packages(lock: &Lock) -> Result<BTreeMap<Identity, &Package>, &'static str> {
        if lock.version != 4 {
            return Err("unexpected lock version");
        }
        let mut packages = BTreeMap::new();
        for package in &lock.package {
            if packages.insert(identity(package), package).is_some() {
                return Err("duplicate locked package");
            }
        }
        Ok(packages)
    }

    fn dependency(
        graph: &BTreeMap<Identity, &Package>,
        spelling: &str,
    ) -> Result<Identity, &'static str> {
        let fields: Vec<_> = spelling.split_whitespace().collect();
        let (name, version, source) = match fields.as_slice() {
            [name] => (*name, None, None),
            [name, version] => (*name, Some(*version), None),
            [name, version, source] if source.starts_with('(') && source.ends_with(')') => {
                (*name, Some(*version), Some(&source[1..source.len() - 1]))
            }
            _ => return Err("invalid locked dependency"),
        };
        let mut found = graph.keys().filter(|key| {
            key.0 == name
                && version.is_none_or(|version| key.1 == version)
                && source.is_none_or(|source| key.2.as_deref() == Some(source))
        });
        let selected = found.next().ok_or("missing locked dependency")?;
        if found.next().is_some() {
            return Err("ambiguous locked dependency");
        }
        Ok(selected.clone())
    }

    fn edges(
        graph: &BTreeMap<Identity, &Package>,
        package: &Package,
    ) -> Result<BTreeSet<Identity>, &'static str> {
        let selected: BTreeSet<_> = package
            .dependencies
            .iter()
            .map(|spelling| dependency(graph, spelling))
            .collect::<Result<_, _>>()?;
        if selected.len() != package.dependencies.len() {
            return Err("duplicate locked dependency");
        }
        Ok(selected)
    }

    // Compare resolved identities, rather than spelling: the root's broader
    // feature union qualifies versions that are unambiguous in this fixture.
    // Lockfiles retain target-only edges even when this test runs natively.
    fn workspace_closure(
        fixture: &Lock,
        workspace: &Lock,
    ) -> Result<BTreeSet<Identity>, &'static str> {
        let fixture = packages(fixture)?;
        let workspace = packages(workspace)?;
        let entry = (
            "privacy-detector-fixture".to_owned(),
            "0.1.0".to_owned(),
            None,
        );
        let package = fixture.get(&entry).ok_or("missing fixture entry")?;
        if package.checksum.is_some()
            || edges(&fixture, package)?
                .iter()
                .map(|key| key.0.as_str())
                .collect::<BTreeSet<_>>()
                != BTreeSet::from(["craftsman-gadget", "schemars", "serde", "serde_json"])
        {
            return Err("changed standalone fixture dependencies");
        }
        for (key, package) in &fixture {
            if key == &entry {
                continue;
            }
            let original = workspace.get(key).ok_or("package absent from root lock")?;
            if package.checksum != original.checksum {
                return Err("package checksum differs from root lock");
            }
            let selected = edges(&fixture, package)?;
            let original = edges(&workspace, original)?;
            if !selected.is_subset(&original) {
                return Err("dependency edge absent from root lock");
            }
            if package.source.is_none() && selected != original {
                return Err("changed source SDK or macro dependencies");
            }
            if package.source.is_some() {
                let mut required = original.clone();
                // Only these two root package feature unions add optional
                // serde edges absent from the fixture's exact resolution.
                // Match complete identities; no other missing edge is allowed.
                if matches!(
                    (key.0.as_str(), key.1.as_str(), key.2.as_deref()),
                    (
                        "hashbrown",
                        "0.17.1",
                        Some("registry+https://github.com/rust-lang/crates.io-index")
                    ) | (
                        "semver",
                        "1.0.28",
                        Some("registry+https://github.com/rust-lang/crates.io-index")
                    )
                ) {
                    for name in ["serde", "serde_core"] {
                        let optional = (
                            name.to_owned(),
                            "1.0.228".to_owned(),
                            Some(
                                "registry+https://github.com/rust-lang/crates.io-index".to_owned(),
                            ),
                        );
                        if !required.remove(&optional) {
                            return Err("changed root-only serialization feature union");
                        }
                    }
                }
                if selected != required {
                    return Err("changed required registry dependency set");
                }
            }
        }
        let mut pending = vec![entry];
        let mut reached = BTreeSet::new();
        while let Some(key) = pending.pop() {
            if reached.insert(key.clone()) {
                pending.extend(edges(&fixture, fixture[&key])?);
            }
        }
        if reached.len() != fixture.len() {
            return Err("unreachable fixture lock package");
        }
        Ok(reached)
    }

    fn graphs() -> (Lock, Lock) {
        let root = root();
        (
            toml::from_str(
                &std::fs::read_to_string(
                    root.join("crates/craftsman-rs/tests/fixtures/privacy-detector/Cargo.lock"),
                )
                .unwrap(),
            )
            .unwrap(),
            toml::from_str(&std::fs::read_to_string(root.join("Cargo.lock")).unwrap()).unwrap(),
        )
    }

    #[test]
    fn detector_entire_lock_graph_reuses_workspace_including_wasm_only_dependencies() {
        let (fixture, workspace) = graphs();
        let reached = workspace_closure(&fixture, &workspace).unwrap();
        assert_eq!(reached.len(), 45);
        assert_eq!(reached.iter().filter(|key| key.2.is_some()).count(), 42);
        for name in [
            "rustversion",
            "wit-bindgen",
            "wit-bindgen-core",
            "wit-bindgen-rust",
            "wit-bindgen-rust-macro",
            "wit-component",
            "wit-parser",
        ] {
            assert!(reached.iter().any(|key| key.0 == name));
        }
    }

    #[test]
    fn detector_lock_rejects_uncached_versions_checksums_and_foreign_sources() {
        let (fixture, workspace) = graphs();
        for (name, version) in [("ref-cast", "1.0.27"), ("syn", "3.0.9")] {
            let mut changed = fixture.clone();
            let package = changed
                .package
                .iter_mut()
                .find(|package| package.name == name && package.version.starts_with(&version[..1]))
                .unwrap();
            package.version = version.to_owned();
            assert!(workspace_closure(&changed, &workspace).is_err());
        }
        let mut changed = fixture.clone();
        let package = changed
            .package
            .iter_mut()
            .find(|package| package.name == "serde")
            .unwrap();
        package.checksum = Some("0".repeat(64));
        assert!(workspace_closure(&changed, &workspace).is_err());
        let package = changed
            .package
            .iter_mut()
            .find(|package| package.name == "serde")
            .unwrap();
        package.checksum = fixture
            .package
            .iter()
            .find(|package| package.name == "serde")
            .unwrap()
            .checksum
            .clone();
        package.source = Some("registry+https://example.invalid/index".to_owned());
        assert!(workspace_closure(&changed, &workspace).is_err());
    }

    #[test]
    fn detector_lock_rejects_missing_wasm_packages_and_changed_sdk_or_macro_edges() {
        let (fixture, workspace) = graphs();
        let mut changed = fixture.clone();
        changed
            .package
            .retain(|package| package.name != "wit-bindgen-rust-macro");
        assert!(workspace_closure(&changed, &workspace).is_err());
        let mut changed = fixture.clone();
        changed
            .package
            .iter_mut()
            .find(|package| package.name == "craftsman-gadget")
            .unwrap()
            .dependencies
            .retain(|edge| edge != "wit-bindgen");
        assert!(workspace_closure(&changed, &workspace).is_err());
        let mut changed = fixture.clone();
        changed
            .package
            .iter_mut()
            .find(|package| package.name == "craftsman-gadget-macros")
            .unwrap()
            .dependencies
            .push("serde".to_owned());
        assert!(workspace_closure(&changed, &workspace).is_err());
    }

    #[test]
    fn detector_lock_rejects_duplicate_unreachable_and_ambiguous_packages() {
        let (fixture, workspace) = graphs();
        let mut changed = fixture.clone();
        changed.package.push(changed.package[0].clone());
        assert!(workspace_closure(&changed, &workspace).is_err());
        let mut changed = fixture.clone();
        changed.package.push(
            workspace
                .package
                .iter()
                .find(|package| package.name == "atomic-waker")
                .unwrap()
                .clone(),
        );
        assert!(workspace_closure(&changed, &workspace).is_err());
        let mut changed = fixture.clone();
        changed
            .package
            .iter_mut()
            .find(|package| package.name == "craftsman-gadget-macros")
            .unwrap()
            .dependencies
            .iter_mut()
            .find(|edge| edge.starts_with("syn "))
            .map(|edge| *edge = "syn".to_owned())
            .unwrap();
        assert!(workspace_closure(&changed, &workspace).is_err());
    }

    #[test]
    fn detector_lock_rejects_required_registry_edge_omissions_with_other_paths_remaining() {
        let (fixture, workspace) = graphs();
        let mut accepted = Vec::new();
        for (name, omitted) in [
            ("serde_derive", "proc-macro2"),
            ("quote", "proc-macro2"),
            ("wit-bindgen-rust-macro", "quote"),
        ] {
            let mut changed = fixture.clone();
            let package = changed
                .package
                .iter_mut()
                .find(|package| package.name == name)
                .unwrap();
            let before = package.dependencies.len();
            package
                .dependencies
                .retain(|edge| edge.split_whitespace().next() != Some(omitted));
            assert_eq!(package.dependencies.len() + 1, before);
            if workspace_closure(&changed, &workspace).is_ok() {
                accepted.push(format!("{name} -> {omitted}"));
            }
        }
        assert!(
            accepted.is_empty(),
            "required registry edges were accepted as omitted: {accepted:?}"
        );
    }

    #[test]
    fn detector_lock_rejects_unselected_root_only_registry_feature_edges() {
        let (fixture, workspace) = graphs();
        let mut accepted = Vec::new();
        for name in ["hashbrown", "semver"] {
            for added in ["serde", "serde_core"] {
                let mut changed = fixture.clone();
                let package = changed
                    .package
                    .iter_mut()
                    .find(|package| package.name == name)
                    .unwrap();
                assert!(
                    !package
                        .dependencies
                        .iter()
                        .any(|edge| edge.split_whitespace().next() == Some(added))
                );
                package.dependencies.push(added.to_owned());
                if workspace_closure(&changed, &workspace).is_ok() {
                    accepted.push(format!("{name} -> {added}"));
                }
            }
        }
        assert!(
            accepted.is_empty(),
            "unselected registry feature edges were accepted: {accepted:?}"
        );
    }
}

mod compiler_environment {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::process::{Command, Stdio};

    pub(super) const TRANSPORT_X86: &str = "CRAFTSMAN_DETECTOR_COMPILER_PROGRAM_FILES_X86";
    pub(super) const TRANSPORT_NATIVE: &str = "CRAFTSMAN_DETECTOR_COMPILER_PROGRAM_FILES";
    const PROBE: &str = "gadgets::privacy_detector_tests::compiler_environment::compiler_environment_native_fixture";
    const PROBE_MARKER: &str = "craftsman-privacy-detector-compiler=";

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) struct DiscoveryRoot {
        name: &'static str,
        value: OsString,
    }

    impl DiscoveryRoot {
        fn new(name: &'static str, value: OsString) -> Result<Self, &'static str> {
            let bytes = value.as_encoded_bytes();
            if bytes.is_empty()
                || bytes.len() > 16 * 1024
                || bytes.contains(&0)
                || !Path::new(&value).is_absolute()
            {
                return Err("invalid compiler discovery root");
            }
            #[cfg(windows)]
            {
                use std::path::{Component, Prefix};
                let mut components = Path::new(&value).components();
                if matches!(components.next(), Some(Component::Prefix(prefix))
                    if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
                    && !matches!(components.next(), Some(Component::RootDir))
                {
                    return Err("invalid compiler discovery root");
                }
            }
            Ok(Self { name, value })
        }

        fn transport_name(&self) -> &'static str {
            match self.name {
                "ProgramFiles(x86)" => TRANSPORT_X86,
                "ProgramFiles" => TRANSPORT_NATIVE,
                _ => unreachable!("owned discovery root name"),
            }
        }

        fn environment(&self) -> Vec<(&'static str, OsString)> {
            vec![(self.name, self.value.clone())]
        }
    }

    fn discovery(
        mut lookup: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<DiscoveryRoot, &'static str> {
        for name in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(value) = lookup(name) {
                return DiscoveryRoot::new(name, value);
            }
        }
        Err("compiler discovery root unavailable")
    }

    fn transferred(
        mut lookup: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<Option<DiscoveryRoot>, &'static str> {
        match (lookup(TRANSPORT_X86), lookup(TRANSPORT_NATIVE)) {
            (Some(value), None) => DiscoveryRoot::new("ProgramFiles(x86)", value).map(Some),
            (None, Some(value)) => DiscoveryRoot::new("ProgramFiles", value).map(Some),
            (None, None) => Ok(None),
            (Some(_), Some(_)) => Err("ambiguous compiler discovery roots"),
        }
    }

    pub(super) fn system_root() -> Option<OsString> {
        #[cfg(windows)]
        {
            Some(std::env::var_os("SystemRoot").expect("native SystemRoot for trusted compiler"))
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub(super) fn host_discovery() -> Option<DiscoveryRoot> {
        #[cfg(windows)]
        {
            Some(
                discovery(|name| std::env::var_os(name))
                    .expect("compiler discovery root unavailable"),
            )
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    // A private transport keeps installation roots out of the helper's normal
    // environment. Only its Cargo child gets the standard discovery name;
    // rustc owns MSVC/SDK discovery and the unchanged guest inherits no env.
    pub(super) fn build_environment() -> Vec<(&'static str, OsString)> {
        #[cfg(windows)]
        {
            let selected = transferred(|name| std::env::var_os(name))
                .expect("invalid compiler discovery transfer")
                .or_else(host_discovery)
                .expect("compiler discovery root unavailable");
            msvc_setup::prepare(&selected)
                .expect("installed MSVC compiler preparation failed")
                .environment()
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    }

    pub(super) fn isolated_supervisor_environment(
        command: &mut Command,
        home: &Path,
        cargo_home: &OsStr,
        path: &OsStr,
        system_root: Option<OsString>,
        discovery: Option<DiscoveryRoot>,
    ) {
        msvc_setup::transport_setup_state_comparison(command);
        command
            .env_clear()
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("TMPDIR", home)
            .env("TEMP", home)
            .env("TMP", home)
            .env("CARGO_HOME", cargo_home)
            .env("PATH", path)
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
            );
        if let Some(value) = system_root {
            command.env("SystemRoot", value);
        }
        if let Some(selected) = discovery {
            command.env(selected.transport_name(), selected.value);
        }
    }

    fn probe_mode(arguments: impl IntoIterator<Item = OsString>) -> Option<&'static str> {
        let values: Vec<_> = arguments
            .into_iter()
            // The registered ASCII prefix is self-synchronizing in the native
            // encoding. Count it before UTF-8 conversion so a malformed
            // registered companion cannot disappear beside a valid marker.
            .filter(|argument| {
                argument
                    .as_encoded_bytes()
                    .starts_with(PROBE_MARKER.as_bytes())
            })
            .map(|argument| {
                argument
                    .to_str()
                    .and_then(|argument| argument.strip_prefix(PROBE_MARKER))
                    .map(str::to_owned)
            })
            .collect();
        match values.as_slice() {
            [Some(value)] if value == "parent" => Some("parent"),
            [Some(value)] if value == "snapshot" => Some("snapshot"),
            _ => None,
        }
    }

    #[test]
    #[ignore = "compiler environment probe; exactly one registered argv marker required"]
    fn compiler_environment_native_fixture() {
        let Some(mode) = probe_mode(std::env::args_os()) else {
            println!("COMPILER-ENVIRONMENT:NOT-SELECTED");
            return;
        };
        if mode == "snapshot" {
            let mut snapshot = BTreeMap::new();
            for name in [
                "HOME",
                "USERPROFILE",
                "TMPDIR",
                "TEMP",
                "TMP",
                "CARGO_HOME",
                "PATH",
                "RUSTC",
                "RUSTDOC",
                "SystemRoot",
                "ProgramFiles(x86)",
                "ProgramFiles",
                TRANSPORT_X86,
                TRANSPORT_NATIVE,
                "CRAFTSMAN_TEST_PARENT_CREDENTIAL",
            ] {
                snapshot.insert(
                    name,
                    std::env::var_os(name).map(|value| value.into_string().unwrap()),
                );
            }
            println!(
                "COMPILER-ENVIRONMENT:JSON {}",
                serde_json::to_string(&snapshot).unwrap()
            );
            return;
        }
        let selected = discovery(|name| std::env::var_os(name)).unwrap();
        let home = std::env::var_os("CRAFTSMAN_TEST_COMPILER_HOME").unwrap();
        let cargo_home = std::env::var_os("CRAFTSMAN_TEST_COMPILER_CARGO_HOME").unwrap();
        let path = std::env::var_os("PATH").unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--ignored", "--exact", PROBE, "--nocapture"])
            .arg(format!("{PROBE_MARKER}snapshot"));
        isolated_supervisor_environment(
            &mut child,
            Path::new(&home),
            &cargo_home,
            &path,
            system_root(),
            Some(selected.clone()),
        );
        let environment: BTreeMap<_, _> = child
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
            .collect();
        assert_eq!(environment.get(OsStr::new("ProgramFiles(x86)")), None);
        assert_eq!(environment.get(OsStr::new("ProgramFiles")), None);
        assert_eq!(
            environment.get(OsStr::new(selected.transport_name())),
            Some(&Some(selected.value.clone()))
        );
        let translated = transferred(|name| environment.get(OsStr::new(name)).cloned().flatten())
            .unwrap()
            .unwrap();
        child
            .envs(translated.environment())
            .env_remove(TRANSPORT_X86)
            .env_remove(TRANSPORT_NATIVE)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = child.output().unwrap();
        assert!(output.status.success());
        assert!(output.stdout.len() <= 64 * 1024 && output.stderr.len() <= 64 * 1024);
        std::io::stdout().write_all(&output.stdout).unwrap();
        std::io::stderr().write_all(&output.stderr).unwrap();
    }

    async fn output(mut command: tokio::process::Command) -> std::process::Output {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let mut out = child.stdout.take().unwrap().take(65537);
        let mut err = child.stderr.take().unwrap().take(65537);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let (out_read, err_read, status) =
            match tokio::time::timeout(Duration::from_secs(20), async {
                tokio::join!(
                    out.read_to_end(&mut stdout),
                    err.read_to_end(&mut stderr),
                    child.wait()
                )
            })
            .await
            {
                Ok(completed) => completed,
                Err(_) => {
                    child.start_kill().unwrap();
                    tokio::time::timeout(Duration::from_secs(5), child.wait())
                        .await
                        .unwrap()
                        .unwrap();
                    panic!("compiler probe exceeded its owned bound");
                }
            };
        out_read.unwrap();
        err_read.unwrap();
        assert!(stdout.len() <= 64 * 1024 && stderr.len() <= 64 * 1024);
        std::process::Output {
            status: status.unwrap(),
            stdout,
            stderr,
        }
    }

    #[tokio::test]
    async fn compiler_discovery_is_private_until_actual_isolated_child_translation() {
        for name in ["ProgramFiles(x86)", "ProgramFiles"] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join("compiler installation 中文 with spaces");
            let owned_home = home.path().join("isolated-home");
            let cargo_home = home.path().join("isolated-cargo");
            let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--ignored", "--exact", PROBE, "--nocapture"])
                .arg(format!("{PROBE_MARKER}parent"))
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env(name, &root)
                .env("HOME", home.path().join("unselected-parent-home"))
                .env(
                    "CRAFTSMAN_TEST_PARENT_CREDENTIAL",
                    "synthetic excluded value",
                )
                .env("CRAFTSMAN_TEST_COMPILER_HOME", &owned_home)
                .env("CRAFTSMAN_TEST_COMPILER_CARGO_HOME", &cargo_home);
            if name == "ProgramFiles(x86)" {
                command.env("ProgramFiles", home.path().join("unselected installation"));
            }
            if let Some(value) = system_root() {
                command.env("SystemRoot", value);
            }
            let observed = output(command).await;
            assert!(
                observed.status.success(),
                "{}",
                String::from_utf8_lossy(&observed.stderr)
            );
            let text = std::str::from_utf8(&observed.stdout).unwrap();
            let snapshot: BTreeMap<String, Option<String>> = serde_json::from_str(
                text.lines()
                    .find_map(|line| line.strip_prefix("COMPILER-ENVIRONMENT:JSON "))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(snapshot[name].as_deref(), root.to_str());
            let unselected = if name == "ProgramFiles(x86)" {
                "ProgramFiles"
            } else {
                "ProgramFiles(x86)"
            };
            assert_eq!(snapshot[unselected], None);
            assert_eq!(snapshot[TRANSPORT_X86], None);
            assert_eq!(snapshot[TRANSPORT_NATIVE], None);
            assert_eq!(snapshot["CRAFTSMAN_TEST_PARENT_CREDENTIAL"], None);
            for variable in ["HOME", "USERPROFILE", "TMPDIR", "TEMP", "TMP"] {
                assert_eq!(snapshot[variable].as_deref(), owned_home.to_str());
            }
            assert_eq!(snapshot["CARGO_HOME"].as_deref(), cargo_home.to_str());
            for (variable, executable) in [("RUSTC", "rustc"), ("RUSTDOC", "rustdoc")] {
                let expected = Path::new(env!("CARGO")).with_file_name(if cfg!(windows) {
                    format!("{executable}.exe")
                } else {
                    executable.to_owned()
                });
                assert_eq!(snapshot[variable].as_deref(), expected.to_str());
            }
        }
    }

    #[test]
    fn compiler_discovery_selection_preserves_preference_and_refuses_whole_invalid_roots() {
        let home = tempfile::tempdir().unwrap();
        let mut looked_up = Vec::new();
        let selected = discovery(|name| {
            looked_up.push(name.to_owned());
            Some(home.path().as_os_str().to_owned())
        })
        .unwrap();
        assert_eq!(looked_up, ["ProgramFiles(x86)"]);
        assert_eq!(selected.name, "ProgramFiles(x86)");
        assert!(discovery(|_| None).is_err());
        for invalid in [
            OsString::new(),
            OsString::from("relative/installation"),
            OsString::from("bad\0root"),
            OsString::from("x".repeat(16 * 1024 + 1)),
        ] {
            let mut looked_up = Vec::new();
            assert!(
                discovery(|name| {
                    looked_up.push(name.to_owned());
                    Some(invalid.clone())
                })
                .is_err()
            );
            assert_eq!(looked_up, ["ProgramFiles(x86)"]);
        }
        assert!(transferred(|_| Some(home.path().as_os_str().to_owned())).is_err());
        assert_eq!(transferred(|_| None).unwrap(), None);
    }

    #[tokio::test]
    async fn unmarked_unknown_and_duplicate_compiler_probes_do_not_inspect_poisoned_roots() {
        for markers in [vec![], vec!["unknown"], vec!["parent", "parent"]] {
            let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--ignored", "--exact", PROBE, "--nocapture"]);
            for marker in markers {
                command.arg(format!("{PROBE_MARKER}{marker}"));
            }
            command
                .env_clear()
                .env("ProgramFiles(x86)", "relative poisoned root");
            if let Some(value) = system_root() {
                command.env("SystemRoot", value);
            }
            let observed = output(command).await;
            assert!(observed.status.success());
            let text = std::str::from_utf8(&observed.stdout).unwrap();
            assert!(text.contains("COMPILER-ENVIRONMENT:NOT-SELECTED"));
            assert!(!text.contains("COMPILER-ENVIRONMENT:JSON"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn actual_pinned_native_compiler_distinguishes_a_link_utility_from_a_linker() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("compiler_probe.rs");
        std::fs::write(
            &source,
            b"fn main() { assert_eq!(std::env::args_os().count(), 2); }\n",
        )
        .unwrap();
        let rustc = Path::new(env!("CARGO")).with_file_name("rustc");
        let path = std::env::var_os("PATH").unwrap();
        let utility = if cfg!(target_os = "macos") {
            "/bin/link"
        } else {
            "/usr/bin/link"
        };
        let failed_image = home.path().join("failed-native-image");
        let mut wrong = tokio::process::Command::new(&rustc);
        wrong
            .arg(&source)
            .args(["--edition", "2024", "--crate-name", "compiler_probe", "-C"])
            .arg(format!("linker={utility}"))
            .arg("-o")
            .arg(&failed_image);
        isolated_supervisor_environment(
            wrong.as_std_mut(),
            home.path(),
            home.path().join("isolated-cargo").as_os_str(),
            &path,
            None,
            None,
        );
        let failed = output(wrong).await;
        assert!(!failed.status.success());
        assert!(!failed_image.exists());
        let diagnostic = std::str::from_utf8(&failed.stderr).unwrap();
        assert!(diagnostic.contains("linking with"));
        assert!(diagnostic.contains(utility));
        println!("ACTUAL-NATIVE-LINK-UTILITY-DIAGNOSTIC {diagnostic}");

        let image = home.path().join("actual-linked-native-image");
        let mut correct = tokio::process::Command::new(&rustc);
        correct
            .arg(&source)
            .args([
                "--edition",
                "2024",
                "--crate-name",
                "compiler_probe",
                "-C",
                "linker=/usr/bin/cc",
            ])
            .arg("-o")
            .arg(&image);
        isolated_supervisor_environment(
            correct.as_std_mut(),
            home.path(),
            home.path().join("isolated-cargo").as_os_str(),
            &path,
            None,
            None,
        );
        let compiled = output(correct).await;
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        assert!(image.is_file());
        let mut execute = tokio::process::Command::new(&image);
        execute.arg("actual-compiler-probe").env_clear();
        let returned = output(execute).await;
        assert!(returned.status.success());
        assert!(returned.stdout.is_empty() && returned.stderr.is_empty());
    }

    #[test]
    fn compiler_native_marker_counts_malformed_registered_companions_before_decoding() {
        #[cfg(unix)]
        let malformed = {
            use std::os::unix::ffi::OsStringExt;
            let mut bytes = format!("{PROBE_MARKER}unknown").into_bytes();
            bytes.push(0xff);
            OsString::from_vec(bytes)
        };
        #[cfg(windows)]
        let malformed = {
            use std::os::windows::ffi::OsStringExt;
            let mut units: Vec<_> = format!("{PROBE_MARKER}unknown").encode_utf16().collect();
            units.push(0xd800);
            OsString::from_wide(&units)
        };
        assert!(malformed.to_str().is_none());
        for arguments in [
            vec![
                OsString::from(format!("{PROBE_MARKER}parent")),
                malformed.clone(),
            ],
            vec![
                malformed.clone(),
                OsString::from(format!("{PROBE_MARKER}snapshot")),
            ],
            vec![malformed],
        ] {
            assert_eq!(probe_mode(arguments), None);
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_compiler_discovery_drive_roots_require_a_physical_separator_before_transfer() {
        for incomplete in [r"C:", r"\\?\C:", r"\Program Files"] {
            let value = OsString::from(incomplete);
            assert!(DiscoveryRoot::new("ProgramFiles(x86)", value.clone()).is_err());
            assert!(
                transferred(|name| { (name == TRANSPORT_X86).then(|| value.clone()) }).is_err()
            );
            let mut consulted = Vec::new();
            assert!(
                discovery(|name| {
                    consulted.push(name.to_owned());
                    Some(value.clone())
                })
                .is_err()
            );
            assert_eq!(consulted, ["ProgramFiles(x86)"]);
        }
        for complete in [
            r"C:\",
            r"\\?\C:\",
            r"C:\Program Files (x86)",
            r"\\?\C:\Program Files (x86)",
            r"\\server\share",
            r"\\?\UNC\server\share",
        ] {
            let value = OsString::from(complete);
            let selected = DiscoveryRoot::new("ProgramFiles(x86)", value.clone()).unwrap();
            assert_eq!(selected.value.as_encoded_bytes(), value.as_encoded_bytes());
            let translated = transferred(|name| (name == TRANSPORT_X86).then(|| value.clone()))
                .unwrap()
                .unwrap();
            assert_eq!(translated, selected);
        }
    }

    mod msvc_setup {
        use super::*;
        use std::io::Read;
        use std::time::{Duration, Instant};

        type SetupResult<T> = Result<T, Failure>;
        const FAILURE: &str = "installed MSVC compiler preparation failed";
        const CAPTURE_BYTES: usize = 64 * 1024;
        const MAX_PATH_UNITS: usize = 16 * 1024;

        mod facts {
            use super::*;
            use serde::Serialize;

            const EXCERPT_BYTES: usize = 2048;

            #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
            #[serde(rename_all = "snake_case")]
            pub(super) enum Stage {
                Host,
                SystemRoot,
                RustcIdentity,
                VswhereFile,
                VswhereDiscovery,
                Installation,
                VsdevcmdFile,
                CmdFile,
                VsdevcmdSetup,
                Utf16Fields,
                NativePath,
                File,
                Directory,
                VcDirectory,
                SdkDirectory,
                VcContainment,
                LinkerFile,
                PeIdentity,
                LibList,
                LibDirectory,
                RequiredLibrary,
                LibraryRoots,
                CompilerEnvironment,
                LinkerBanner,
                Control,
            }

            #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
            #[serde(rename_all = "snake_case")]
            pub(super) enum Kind {
                Host,
                Environment,
                Spawn,
                Wait,
                Timeout,
                CaptureReader,
                CaptureRead,
                CaptureDrain,
                CaptureJoin,
                CaptureLimit,
                Exit,
                Empty,
                Bounds,
                Utf8,
                OddUtf16,
                Nul,
                Shape,
                UnknownField,
                MissingField,
                DuplicateField,
                FieldValue,
                HostMismatch,
                NativePath,
                Metadata,
                NotFile,
                Symlink,
                EmptyFile,
                NotDirectory,
                Containment,
                Read,
                Pe,
                Library,
                LibraryRoots,
                PathList,
                Banner,
            }

            #[derive(Serialize)]
            pub(super) struct StreamFacts {
                pub(super) captured_bytes: Option<usize>,
                pub(super) reached_capture_limit: bool,
                pub(super) actual_eof: bool,
                pub(super) original_size: Option<usize>,
                pub(super) original_sha256: Option<String>,
                pub(super) captured_prefix_sha256: Option<String>,
                pub(super) excerpt_hex: Option<String>,
                pub(super) excerpt_cut: bool,
                pub(super) contents_withheld: bool,
                excerpt_encoding: &'static str,
            }

            pub(super) fn hex(bytes: &[u8]) -> String {
                const DIGITS: &[u8; 16] = b"0123456789abcdef";
                let mut text = String::with_capacity(bytes.len() * 2);
                for &byte in bytes {
                    text.push(DIGITS[(byte >> 4) as usize] as char);
                    text.push(DIGITS[(byte & 15) as usize] as char);
                }
                text
            }

            pub(super) fn stream(
                bytes: Option<&[u8]>,
                eof: bool,
                capture_limit: bool,
                excerpt: bool,
                excerpt_bound: usize,
            ) -> StreamFacts {
                let complete = eof && !capture_limit;
                StreamFacts {
                    captured_bytes: bytes.map(<[u8]>::len),
                    reached_capture_limit: capture_limit,
                    actual_eof: eof,
                    original_size: complete.then(|| bytes.map(<[u8]>::len)).flatten(),
                    original_sha256: complete.then(|| bytes.map(hash)).flatten(),
                    captured_prefix_sha256: bytes.map(hash),
                    excerpt_hex: bytes
                        .filter(|_| excerpt)
                        .map(|bytes| hex(&bytes[..bytes.len().min(excerpt_bound)])),
                    excerpt_cut: bytes.is_some_and(|bytes| excerpt && bytes.len() > excerpt_bound),
                    contents_withheld: bytes.is_some() && !excerpt,
                    excerpt_encoding: "hex-original-bytes",
                }
            }

            #[derive(Serialize)]
            pub(super) struct CleanupFacts {
                pub(super) kill_returned_ok: bool,
                pub(super) kill_raw_os_error: Option<i32>,
                pub(super) try_wait_returned_status: bool,
                pub(super) try_wait_raw_os_error: Option<i32>,
                pub(super) exit_code_after_cleanup: Option<i32>,
                pub(super) bound_reached_without_status: bool,
            }

            #[derive(Serialize)]
            pub(super) struct ProcessFacts {
                pub(super) spawn_returned_child: bool,
                pub(super) exit_code: Option<i32>,
                pub(super) exit_success: Option<bool>,
                pub(super) stdout: Option<StreamFacts>,
                pub(super) stderr: Option<StreamFacts>,
                pub(super) cleanup: Option<CleanupFacts>,
            }

            #[derive(Serialize)]
            pub(super) struct LibraryObservation {
                pub(super) directory_ordinal: usize,
                pub(super) kind: Kind,
                pub(super) raw_os_error: Option<i32>,
            }

            #[derive(Serialize)]
            pub(in super::super) struct Failure {
                fixed: &'static str,
                pub(super) stage: Stage,
                pub(super) kind: Kind,
                pub(super) expected: Option<&'static str>,
                pub(super) raw_os_error: Option<i32>,
                pub(super) process: Option<ProcessFacts>,
                pub(super) observed_read_bytes: Option<usize>,
                pub(super) observed_read_prefix_sha256: Option<String>,
                pub(super) observed_pe_machine: Option<u16>,
                pub(super) library_observations: Vec<LibraryObservation>,
                pub(super) discovery_comparisons: Option<super::DiscoveryComparisons>,
                #[serde(skip_serializing_if = "Option::is_none")]
                pub(super) command_comparisons:
                    Option<super::command_comparison::CommandComparisons>,
            }

            impl Failure {
                pub(super) fn new(stage: Stage, kind: Kind) -> Self {
                    Self {
                        fixed: FAILURE,
                        stage,
                        kind,
                        expected: None,
                        raw_os_error: None,
                        process: None,
                        observed_read_bytes: None,
                        observed_read_prefix_sha256: None,
                        observed_pe_machine: None,
                        library_observations: Vec::new(),
                        discovery_comparisons: None,
                        command_comparisons: None,
                    }
                }

                pub(super) fn io(stage: Stage, kind: Kind, error: &std::io::Error) -> Self {
                    let mut failure = Self::new(stage, kind);
                    failure.raw_os_error = error.raw_os_error();
                    failure
                }

                pub(super) fn at(mut self, stage: Stage) -> Self {
                    self.stage = stage;
                    self
                }

                pub(super) fn expected(mut self, expected: &'static str) -> Self {
                    // Call sites supply fixed required environment/field/library identifiers.
                    self.expected = Some(expected);
                    self
                }

                pub(super) fn output(
                    mut self,
                    output: &std::process::Output,
                    stdout_excerpt: bool,
                ) -> Self {
                    self.process = Some(ProcessFacts {
                        spawn_returned_child: true,
                        exit_code: output.status.code(),
                        exit_success: Some(output.status.success()),
                        stdout: Some(stream(
                            Some(&output.stdout),
                            true,
                            false,
                            stdout_excerpt,
                            EXCERPT_BYTES,
                        )),
                        stderr: Some(stream(
                            Some(&output.stderr),
                            true,
                            false,
                            true,
                            EXCERPT_BYTES,
                        )),
                        cleanup: None,
                    });
                    self
                }
            }

            impl std::fmt::Debug for Failure {
                fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    // This is only the owned synthetic compiler test lane. Never print
                    // io::Error strings, panic payloads, environment maps or decoded keys.
                    match serde_json::to_string(self) {
                        Ok(json) => write!(formatter, "MSVC-STAGE-FACTS:{json}"),
                        Err(_) => formatter.write_str("MSVC-STAGE-FACTS:record-encoding-failed"),
                    }
                }
            }

            pub(super) fn output_failure(stage: Stage, output: &std::process::Output) -> Failure {
                Failure::new(stage, Kind::Exit).output(output, true)
            }

            pub(super) fn require_exit(
                stage: Stage,
                output: &std::process::Output,
            ) -> SetupResult<()> {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(output_failure(stage, output))
                }
            }

            pub(super) fn helper_failure_record(stdout: &[u8], stderr: &[u8]) -> Value {
                // The parent has already checked both complete buffers against64KiB.
                // Hex is lossless and cannot inject native/control terminal syntax.
                json!({
                    "stdout": stream(Some(stdout), true, false, true, CAPTURE_BYTES),
                    "stderr": stream(Some(stderr), true, false, true, CAPTURE_BYTES),
                    "source":"already-bounded-new-installed-helper"
                })
            }

            pub(super) fn helper_failure(stdout: &[u8], stderr: &[u8]) {
                let record = helper_failure_record(stdout, stderr);
                eprintln!("MSVC-HELPER-FAILURE:{record}");
            }
        }
        use facts::{Failure, Kind, Stage};

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum Host {
            X64,
            X86,
            Arm64,
        }
        impl Host {
            fn architecture(self) -> &'static str {
                match self {
                    Self::X64 => "x64",
                    Self::X86 => "x86",
                    Self::Arm64 => "arm64",
                }
            }
            fn target(self) -> &'static str {
                match self {
                    Self::X64 => "x86_64-pc-windows-msvc",
                    Self::X86 => "i686-pc-windows-msvc",
                    Self::Arm64 => "aarch64-pc-windows-msvc",
                }
            }
            fn linker_key(self) -> &'static str {
                match self {
                    Self::X64 => "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER",
                    Self::X86 => "CARGO_TARGET_I686_PC_WINDOWS_MSVC_LINKER",
                    Self::Arm64 => "CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER",
                }
            }
            fn component(self) -> &'static str {
                match self {
                    Self::X64 | Self::X86 => "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                    Self::Arm64 => "Microsoft.VisualStudio.Component.VC.Tools.ARM64",
                }
            }
            fn pe_machine(self) -> u16 {
                match self {
                    Self::X64 => 0x8664,
                    Self::X86 => 0x14c,
                    Self::Arm64 => 0xaa64,
                }
            }
            fn command(self) -> &'static str {
                // No discovered pathname or caller shell text is interpolated.
                // The API sets current_dir to the checked Common7/Tools directory.
                match self {
                    Self::X64 => {
                        "\"call VsDevCmd.bat -no_logo -arch=x64 -host_arch=x64 1>&2 & (if errorlevel 1 (exit /b 91)) & set VCToolsInstallDir & set WindowsSdkDir & set VSCMD_ARG_HOST_ARCH & set VSCMD_ARG_TGT_ARCH & set LIB & exit /b 0\""
                    }
                    Self::X86 => {
                        "\"call VsDevCmd.bat -no_logo -arch=x86 -host_arch=x86 1>&2 & (if errorlevel 1 (exit /b 91)) & set VCToolsInstallDir & set WindowsSdkDir & set VSCMD_ARG_HOST_ARCH & set VSCMD_ARG_TGT_ARCH & set LIB & exit /b 0\""
                    }
                    Self::Arm64 => {
                        "\"call VsDevCmd.bat -no_logo -arch=arm64 -host_arch=arm64 1>&2 & (if errorlevel 1 (exit /b 91)) & set VCToolsInstallDir & set WindowsSdkDir & set VSCMD_ARG_HOST_ARCH & set VSCMD_ARG_TGT_ARCH & set LIB & exit /b 0\""
                    }
                }
            }
        }

        struct SetupFields {
            values: BTreeMap<String, Vec<u16>>,
        }
        fn parse_utf16(bytes: &[u8], host: Host) -> SetupResult<SetupFields> {
            if bytes.is_empty() {
                return Err(Failure::new(Stage::Utf16Fields, Kind::Empty));
            }
            if bytes.len() > CAPTURE_BYTES {
                return Err(Failure::new(Stage::Utf16Fields, Kind::Bounds));
            }
            if bytes.len() % 2 != 0 {
                return Err(Failure::new(Stage::Utf16Fields, Kind::OddUtf16));
            }
            let mut units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            if units.first() == Some(&0xfeff) {
                units.remove(0);
            }
            if units.contains(&0) {
                return Err(Failure::new(Stage::Utf16Fields, Kind::Nul));
            }
            let mut values = BTreeMap::new();
            for raw in units.split(|u| *u == 10) {
                let line = raw.strip_suffix(&[13]).unwrap_or(raw);
                if line.is_empty() {
                    continue;
                }
                let split = line
                    .iter()
                    .position(|u| *u == 61)
                    .ok_or_else(|| Failure::new(Stage::Utf16Fields, Kind::Shape))?;
                let name: String = line[..split]
                    .iter()
                    .map(|u| u8::try_from(*u).map(char::from))
                    .collect::<Result<_, _>>()
                    .map_err(|_| Failure::new(Stage::Utf16Fields, Kind::UnknownField))?;
                let name = name.to_ascii_uppercase();
                if !matches!(
                    name.as_str(),
                    "VCTOOLSINSTALLDIR"
                        | "WINDOWSSDKDIR"
                        | "VSCMD_ARG_HOST_ARCH"
                        | "VSCMD_ARG_TGT_ARCH"
                        | "LIB"
                        | "LIBPATH"
                ) {
                    return Err(Failure::new(Stage::Utf16Fields, Kind::UnknownField));
                }
                let value = line[split + 1..].to_vec();
                if value.is_empty() {
                    return Err(Failure::new(Stage::Utf16Fields, Kind::FieldValue));
                }
                if value.len() > MAX_PATH_UNITS {
                    return Err(Failure::new(Stage::Utf16Fields, Kind::Bounds));
                }
                if values.insert(name, value).is_some() {
                    return Err(Failure::new(Stage::Utf16Fields, Kind::DuplicateField));
                }
            }
            for name in [
                "VCTOOLSINSTALLDIR",
                "WINDOWSSDKDIR",
                "VSCMD_ARG_HOST_ARCH",
                "VSCMD_ARG_TGT_ARCH",
                "LIB",
            ] {
                if !values.contains_key(name) {
                    return Err(Failure::new(Stage::Utf16Fields, Kind::MissingField).expected(name));
                }
            }
            let arch: Vec<u16> = host.architecture().encode_utf16().collect();
            if values["VSCMD_ARG_HOST_ARCH"] != arch {
                return Err(Failure::new(Stage::Utf16Fields, Kind::HostMismatch)
                    .expected("VSCMD_ARG_HOST_ARCH"));
            }
            if values["VSCMD_ARG_TGT_ARCH"] != arch {
                return Err(Failure::new(Stage::Utf16Fields, Kind::HostMismatch)
                    .expected("VSCMD_ARG_TGT_ARCH"));
            }
            Ok(SetupFields { values })
        }

        fn split_lib(units: &[u16]) -> SetupResult<Vec<Vec<u16>>> {
            let mut paths = Vec::new();
            let mut path = Vec::new();
            let mut quoted = false;
            for &unit in units {
                match unit {
                    0 | 10 | 13 => {
                        return Err(Failure::new(Stage::LibList, Kind::Nul).expected("LIB"));
                    }
                    34 => quoted = !quoted,
                    59 if !quoted => {
                        if path.is_empty() {
                            return Err(Failure::new(Stage::LibList, Kind::Shape).expected("LIB"));
                        }
                        paths.push(std::mem::take(&mut path));
                    }
                    _ => path.push(unit),
                }
            }
            if quoted {
                return Err(Failure::new(Stage::LibList, Kind::Shape).expected("LIB"));
            }
            // A final list separator names no installed directory. Never pass
            // an empty element to LIB, where it could admit current_dir search.
            // Leading/interior empty elements still fail in the loop above.
            if !path.is_empty() {
                paths.push(path);
            }
            if paths.is_empty()
                || paths.len() > 32
                || paths.iter().any(|p| p.len() > MAX_PATH_UNITS)
            {
                return Err(Failure::new(Stage::LibList, Kind::Bounds).expected("LIB"));
            }
            Ok(paths)
        }
        fn pe_matches(bytes: &[u8], host: Host) -> bool {
            if bytes.len() < 64 || &bytes[..2] != b"MZ" {
                return false;
            }
            let offset = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
            offset.checked_add(6).is_some_and(|end| end <= bytes.len())
                && &bytes[offset..offset + 4] == b"PE\0\0"
                && u16::from_le_bytes(bytes[offset + 4..offset + 6].try_into().unwrap())
                    == host.pe_machine()
        }

        struct OwnedCommand {
            child: std::process::Child,
            reaped: bool,
            cleanup_attempted: bool,
        }
        impl OwnedCommand {
            fn cleanup(&mut self) -> Option<facts::CleanupFacts> {
                if self.reaped || self.cleanup_attempted {
                    return None;
                }
                self.cleanup_attempted = true;
                let kill = self.child.kill();
                let mut result = facts::CleanupFacts {
                    kill_returned_ok: kill.is_ok(),
                    kill_raw_os_error: kill.err().and_then(|error| error.raw_os_error()),
                    try_wait_returned_status: false,
                    try_wait_raw_os_error: None,
                    exit_code_after_cleanup: None,
                    bound_reached_without_status: false,
                };
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs(5) {
                    match self.child.try_wait() {
                        Ok(Some(status)) => {
                            self.reaped = true;
                            result.try_wait_returned_status = true;
                            result.exit_code_after_cleanup = status.code();
                            return Some(result);
                        }
                        Err(error) => {
                            result.try_wait_raw_os_error = error.raw_os_error();
                        }
                        Ok(None) => {}
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                result.bound_reached_without_status = true;
                Some(result)
            }
        }
        impl Drop for OwnedCommand {
            fn drop(&mut self) {
                let _ = self.cleanup();
            }
        }

        #[derive(Default)]
        struct CaptureProgress {
            bytes: Vec<u8>,
            observed_read: bool,
            actual_eof: bool,
            limit: bool,
        }
        type Progress = Arc<std::sync::Mutex<CaptureProgress>>;

        fn capture(mut reader: impl Read, progress: Progress) -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            let mut scratch = [0u8; 4096];
            loop {
                let remaining = CAPTURE_BYTES + 1 - bytes.len();
                if remaining == 0 {
                    if let Ok(mut state) = progress.lock() {
                        state.limit = true;
                    }
                    return Ok(bytes);
                }
                let count = match reader.read(&mut scratch[..remaining.min(4096)]) {
                    Ok(count) => count,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                };
                bytes.extend_from_slice(&scratch[..count]);
                if let Ok(mut state) = progress.lock() {
                    state.bytes.clone_from(&bytes);
                    state.observed_read = true;
                    state.actual_eof = count == 0;
                    state.limit = bytes.len() == CAPTURE_BYTES + 1;
                }
                if count == 0 {
                    return Ok(bytes);
                }
            }
        }

        fn snapshot(progress: &Progress) -> Option<facts::StreamFacts> {
            progress.try_lock().ok().map(|state| {
                facts::stream(
                    state.observed_read.then_some(state.bytes.as_slice()),
                    state.actual_eof,
                    state.limit,
                    true,
                    2048,
                )
            })
        }

        fn command_failure(
            stage: Stage,
            kind: Kind,
            error: Option<&std::io::Error>,
            owned: &mut OwnedCommand,
            status: Option<std::process::ExitStatus>,
            stdout: &Progress,
            stderr: &Progress,
        ) -> Failure {
            let cleanup = owned.cleanup();
            let mut failure = error.map_or_else(
                || Failure::new(stage, kind),
                |error| Failure::io(stage, kind, error),
            );
            failure.process = Some(facts::ProcessFacts {
                spawn_returned_child: true,
                exit_code: status.and_then(|status| status.code()),
                exit_success: status.map(|status| status.success()),
                stdout: snapshot(stdout),
                stderr: snapshot(stderr),
                cleanup,
            });
            failure
        }

        fn bounded(mut command: Command, seconds: u64) -> SetupResult<std::process::Output> {
            bounded_at(&mut command, seconds, Stage::Control)
        }

        fn bounded_at(
            command: &mut Command,
            seconds: u64,
            stage: Stage,
        ) -> SetupResult<std::process::Output> {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let child = command.spawn().map_err(|error| {
                let mut failure = Failure::io(stage, Kind::Spawn, &error);
                failure.process = Some(facts::ProcessFacts {
                    spawn_returned_child: false,
                    exit_code: None,
                    exit_success: None,
                    stdout: None,
                    stderr: None,
                    cleanup: None,
                });
                failure
            })?;
            let mut owned = OwnedCommand {
                child,
                reaped: false,
                cleanup_attempted: false,
            };
            let out_progress = Progress::default();
            let err_progress = Progress::default();
            let stdout = owned.child.stdout.take().ok_or_else(|| {
                command_failure(
                    stage,
                    Kind::CaptureReader,
                    None,
                    &mut owned,
                    None,
                    &out_progress,
                    &err_progress,
                )
            })?;
            let stderr = owned.child.stderr.take().ok_or_else(|| {
                command_failure(
                    stage,
                    Kind::CaptureReader,
                    None,
                    &mut owned,
                    None,
                    &out_progress,
                    &err_progress,
                )
            })?;
            let (a_send, a_receive) = std::sync::mpsc::sync_channel(1);
            let (b_send, b_receive) = std::sync::mpsc::sync_channel(1);
            let progress = out_progress.clone();
            let a = std::thread::Builder::new()
                .spawn(move || {
                    let _ = a_send.send(capture(stdout, progress));
                })
                .map_err(|error| {
                    command_failure(
                        stage,
                        Kind::CaptureReader,
                        Some(&error),
                        &mut owned,
                        None,
                        &out_progress,
                        &err_progress,
                    )
                })?;
            let progress = err_progress.clone();
            let b = std::thread::Builder::new()
                .spawn(move || {
                    let _ = b_send.send(capture(stderr, progress));
                })
                .map_err(|error| {
                    command_failure(
                        stage,
                        Kind::CaptureReader,
                        Some(&error),
                        &mut owned,
                        None,
                        &out_progress,
                        &err_progress,
                    )
                })?;
            let started = Instant::now();
            let status = loop {
                if let Some(status) = owned.child.try_wait().map_err(|error| {
                    command_failure(
                        stage,
                        Kind::Wait,
                        Some(&error),
                        &mut owned,
                        None,
                        &out_progress,
                        &err_progress,
                    )
                })? {
                    owned.reaped = true;
                    break status;
                }
                if started.elapsed() >= Duration::from_secs(seconds) {
                    return Err(command_failure(
                        stage,
                        Kind::Timeout,
                        None,
                        &mut owned,
                        None,
                        &out_progress,
                        &err_progress,
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            let drain = Instant::now();
            let stdout = a_receive
                .recv_timeout(Duration::from_secs(5))
                .map_err(|error| {
                    command_failure(
                        stage,
                        if error == std::sync::mpsc::RecvTimeoutError::Timeout {
                            Kind::CaptureDrain
                        } else {
                            Kind::CaptureReader
                        },
                        None,
                        &mut owned,
                        Some(status),
                        &out_progress,
                        &err_progress,
                    )
                })?
                .map_err(|error| {
                    command_failure(
                        stage,
                        Kind::CaptureRead,
                        Some(&error),
                        &mut owned,
                        Some(status),
                        &out_progress,
                        &err_progress,
                    )
                })?;
            let remaining = Duration::from_secs(5).saturating_sub(drain.elapsed());
            let stderr = b_receive
                .recv_timeout(remaining)
                .map_err(|error| {
                    command_failure(
                        stage,
                        if error == std::sync::mpsc::RecvTimeoutError::Timeout {
                            Kind::CaptureDrain
                        } else {
                            Kind::CaptureReader
                        },
                        None,
                        &mut owned,
                        Some(status),
                        &out_progress,
                        &err_progress,
                    )
                })?
                .map_err(|error| {
                    command_failure(
                        stage,
                        Kind::CaptureRead,
                        Some(&error),
                        &mut owned,
                        Some(status),
                        &out_progress,
                        &err_progress,
                    )
                })?;
            a.join().map_err(|_| {
                command_failure(
                    stage,
                    Kind::CaptureJoin,
                    None,
                    &mut owned,
                    Some(status),
                    &out_progress,
                    &err_progress,
                )
            })?;
            b.join().map_err(|_| {
                command_failure(
                    stage,
                    Kind::CaptureJoin,
                    None,
                    &mut owned,
                    Some(status),
                    &out_progress,
                    &err_progress,
                )
            })?;
            if stdout.len() > CAPTURE_BYTES || stderr.len() > CAPTURE_BYTES {
                return Err(command_failure(
                    stage,
                    Kind::CaptureLimit,
                    None,
                    &mut owned,
                    Some(status),
                    &out_progress,
                    &err_progress,
                ));
            }
            Ok(std::process::Output {
                status,
                stdout,
                stderr,
            })
        }

        #[cfg(windows)]
        fn host() -> SetupResult<Host> {
            match std::env::consts::ARCH {
                "x86_64" => Ok(Host::X64),
                "x86" => Ok(Host::X86),
                "aarch64" => Ok(Host::Arm64),
                _ => Err(Failure::new(Stage::Host, Kind::Host)),
            }
        }
        #[cfg(windows)]
        fn native(units: &[u16]) -> OsString {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(units)
        }
        fn checked_path(value: OsString) -> SetupResult<PathBuf> {
            DiscoveryRoot::new("ProgramFiles", value.clone())
                .map_err(|_| Failure::new(Stage::NativePath, Kind::NativePath))?;
            if value
                .as_encoded_bytes()
                .iter()
                .any(|b| matches!(b, b'\r' | b'\n'))
            {
                return Err(Failure::new(Stage::NativePath, Kind::NativePath));
            }
            Ok(PathBuf::from(value))
        }
        fn physical_file(path: &Path) -> SetupResult<()> {
            let metadata = std::fs::symlink_metadata(path)
                .map_err(|error| Failure::io(Stage::File, Kind::Metadata, &error))?;
            if metadata.file_type().is_symlink() {
                return Err(Failure::new(Stage::File, Kind::Symlink));
            }
            if !metadata.is_file() {
                return Err(Failure::new(Stage::File, Kind::NotFile));
            }
            if metadata.len() == 0 {
                return Err(Failure::new(Stage::File, Kind::EmptyFile));
            }
            Ok(())
        }
        fn physical_directory(path: &Path) -> SetupResult<PathBuf> {
            let metadata = std::fs::symlink_metadata(path)
                .map_err(|error| Failure::io(Stage::Directory, Kind::Metadata, &error))?;
            if metadata.file_type().is_symlink() {
                return Err(Failure::new(Stage::Directory, Kind::Symlink));
            }
            if !metadata.is_dir() {
                return Err(Failure::new(Stage::Directory, Kind::NotDirectory));
            }
            Ok(path.to_path_buf())
        }
        fn base_values(
            command: &mut Command,
            selected: &DiscoveryRoot,
            system: &Path,
            mut lookup: impl FnMut(&str) -> Option<OsString>,
        ) -> SetupResult<()> {
            command.env_clear();
            for name in ["HOME", "USERPROFILE", "TMPDIR", "TEMP", "TMP"] {
                let value = lookup(name).ok_or_else(|| {
                    Failure::new(Stage::CompilerEnvironment, Kind::Environment).expected(name)
                })?;
                command.env(name, value);
            }
            let toolchain = Path::new(env!("CARGO"))
                .parent()
                .ok_or_else(|| Failure::new(Stage::CompilerEnvironment, Kind::NativePath))?;
            let system_bin = system.join("System32");
            command
                .env("SystemRoot", system)
                .env("ComSpec", system_bin.join("cmd.exe"))
                .env(selected.name, &selected.value)
                .env(
                    "PATH",
                    std::env::join_paths([toolchain, system_bin.as_path()])
                        .map_err(|_| Failure::new(Stage::CompilerEnvironment, Kind::PathList))?,
                );
            Ok(())
        }
        #[cfg(windows)]
        fn base(command: &mut Command, selected: &DiscoveryRoot, system: &Path) -> SetupResult<()> {
            base_values(command, selected, system, |name| std::env::var_os(name))
        }

        fn required_library(name: &'static str, dirs: &[PathBuf]) -> SetupResult<()> {
            assert!(matches!(
                name,
                "kernel32.lib"
                    | "ntdll.lib"
                    | "userenv.lib"
                    | "ws2_32.lib"
                    | "dbghelp.lib"
                    | "msvcrt.lib"
            ));
            if dirs.len() > 32 {
                return Err(Failure::new(Stage::RequiredLibrary, Kind::Bounds).expected(name));
            }
            let mut observations = Vec::new();
            for (directory_ordinal, dir) in dirs.iter().enumerate() {
                match physical_file(&dir.join(name)) {
                    Ok(()) => return Ok(()),
                    Err(error) => observations.push(facts::LibraryObservation {
                        directory_ordinal,
                        kind: error.kind,
                        raw_os_error: error.raw_os_error,
                    }),
                }
            }
            let mut error = Failure::new(Stage::RequiredLibrary, Kind::Library).expected(name);
            error.library_observations = observations;
            Err(error)
        }

        fn rustc_identity(output: &std::process::Output, host: Host) -> SetupResult<()> {
            facts::require_exit(Stage::RustcIdentity, output)?;
            let text = std::str::from_utf8(&output.stdout).map_err(|_| {
                Failure::new(Stage::RustcIdentity, Kind::Utf8).output(output, false)
            })?;
            if !text
                .lines()
                .any(|line| line == format!("host: {}", host.target()))
            {
                return Err(
                    Failure::new(Stage::RustcIdentity, Kind::HostMismatch).output(output, true)
                );
            }
            Ok(())
        }

        fn installation_line(output: &std::process::Output) -> SetupResult<&str> {
            facts::require_exit(Stage::VswhereDiscovery, output)?;
            let text = std::str::from_utf8(&output.stdout).map_err(|_| {
                Failure::new(Stage::VswhereDiscovery, Kind::Utf8).output(output, false)
            })?;
            let lines: Vec<_> = text.lines().filter(|line| !line.is_empty()).collect();
            match lines.as_slice() {
                [installation] => Ok(installation),
                _ => Err(Failure::new(Stage::VswhereDiscovery, Kind::Shape).output(output, false)),
            }
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
        #[serde(rename_all = "snake_case")]
        enum DiscoveryQuery {
            SameFilterPrerelease,
            StableInstances,
            PrereleaseInstances,
            StableComponentInstances,
            PrereleaseComponentInstances,
            ProgramDataSameFilter,
        }

        fn original_discovery_arguments(host: Host) -> Vec<&'static str> {
            vec![
                "-latest",
                "-products",
                "*",
                "-requires",
                host.component(),
                "-property",
                "installationPath",
                "-utf8",
            ]
        }

        impl DiscoveryQuery {
            fn arguments(self, host: Host) -> Vec<&'static str> {
                if self == Self::ProgramDataSameFilter {
                    return original_discovery_arguments(host);
                }
                if self == Self::SameFilterPrerelease {
                    let mut args = original_discovery_arguments(host);
                    args.push("-prerelease");
                    return args;
                }
                let mut args = vec!["-all", "-products", "*"];
                if matches!(
                    self,
                    Self::PrereleaseInstances | Self::PrereleaseComponentInstances
                ) {
                    args.push("-prerelease");
                }
                if matches!(
                    self,
                    Self::StableComponentInstances | Self::PrereleaseComponentInstances
                ) {
                    args.extend(["-requires", host.component()]);
                }
                args.extend(["-format", "json", "-utf8"]);
                args
            }
        }

        #[derive(serde::Serialize)]
        struct InstanceSummary {
            instance_id_sha256: Option<String>,
            installation_path_bytes: Option<usize>,
            installation_path_sha256: Option<String>,
            installation_version: Option<String>,
            is_prerelease: Option<bool>,
            is_complete: Option<bool>,
            is_launchable: Option<bool>,
            is_reboot_required: Option<bool>,
            state: Option<u64>,
        }

        #[derive(serde::Serialize)]
        struct DiscoverySummary {
            output_kind: &'static str,
            records: usize,
            instances: Vec<InstanceSummary>,
        }

        #[derive(serde::Serialize)]
        struct QueryComparison {
            query: DiscoveryQuery,
            arguments: Vec<&'static str>,
            required_component_filter: Option<&'static str>,
            output: Option<facts::ProcessFacts>,
            summary: Option<DiscoverySummary>,
            failure: Option<Box<Failure>>,
        }

        #[derive(serde::Serialize)]
        struct DiscoveryComparisons {
            program_encoded_bytes: usize,
            program_encoded_sha256: String,
            program_encoded_hex: String,
            program_encoding: &'static str,
            program_excerpt_cut: bool,
            comparison_only_not_source_or_compiler_selection: bool,
            same_cleared_preparation_environment: bool,
            queries: Vec<QueryComparison>,
            program_data_same_query: Option<SetupStateComparison>,
        }

        #[derive(serde::Serialize)]
        struct SetupStateComparison {
            source: &'static str,
            only_added_environment_field: &'static str,
            original_program_arguments_and_base_unchanged: bool,
            native_encoding: &'static str,
            native_bytes: Option<usize>,
            native_sha256: Option<String>,
            physical_directory_accepted: Option<bool>,
            query: QueryComparison,
        }

        const STATE_MARKER: &str = "craftsman-msvc-program-data=";
        // Keep the additional native argv contribution well below Windows'
        // whole command-line limit without changing old discovery path bounds.
        const MAX_SETUP_STATE_HEX_BYTES: usize = 8 * 1024;

        struct SetupStateRoot {
            value: OsString,
        }
        impl SetupStateRoot {
            fn new(value: OsString) -> SetupResult<Self> {
                checked_path(value.clone()).map_err(|error| {
                    error.at(Stage::CompilerEnvironment).expected("ProgramData")
                })?;
                if value.as_encoded_bytes().iter().any(u8::is_ascii_control) {
                    return Err(Failure::new(Stage::CompilerEnvironment, Kind::NativePath)
                        .expected("ProgramData"));
                }
                let state = Self { value };
                if state.encoded().len() * 2 > MAX_SETUP_STATE_HEX_BYTES {
                    return Err(Failure::new(Stage::CompilerEnvironment, Kind::Bounds)
                        .expected("ProgramData"));
                }
                Ok(state)
            }
            fn encoded(&self) -> Vec<u8> {
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStrExt;
                    self.value
                        .encode_wide()
                        .flat_map(|unit| unit.to_le_bytes())
                        .collect()
                }
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    self.value.as_bytes().to_vec()
                }
            }
            fn argument(&self) -> OsString {
                OsString::from(format!("{STATE_MARKER}{}", facts::hex(&self.encoded())))
            }
        }

        fn transferred_setup_state(
            arguments: impl IntoIterator<Item = OsString>,
        ) -> SetupResult<SetupStateRoot> {
            let values: Vec<_> = arguments
                .into_iter()
                .filter(|value| {
                    value
                        .as_encoded_bytes()
                        .starts_with(STATE_MARKER.as_bytes())
                })
                .collect();
            let [value] = values.as_slice() else {
                let kind = if values.is_empty() {
                    Kind::Environment
                } else {
                    Kind::DuplicateField
                };
                return Err(Failure::new(Stage::CompilerEnvironment, kind).expected("ProgramData"));
            };
            let text = value
                .to_str()
                .and_then(|text| text.strip_prefix(STATE_MARKER))
                .ok_or_else(|| {
                    Failure::new(Stage::CompilerEnvironment, Kind::NativePath)
                        .expected("ProgramData")
                })?;
            if text.is_empty() || text.len() > MAX_SETUP_STATE_HEX_BYTES || text.len() % 2 != 0 {
                return Err(
                    Failure::new(Stage::CompilerEnvironment, Kind::Bounds).expected("ProgramData")
                );
            }
            if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(Failure::new(Stage::CompilerEnvironment, Kind::NativePath)
                    .expected("ProgramData"));
            }
            let mut bytes = Vec::new();
            for pair in text.as_bytes().chunks_exact(2) {
                let text = std::str::from_utf8(pair).map_err(|_| {
                    Failure::new(Stage::CompilerEnvironment, Kind::NativePath)
                        .expected("ProgramData")
                })?;
                bytes.push(u8::from_str_radix(text, 16).map_err(|_| {
                    Failure::new(Stage::CompilerEnvironment, Kind::NativePath)
                        .expected("ProgramData")
                })?);
            }
            #[cfg(windows)]
            let value = {
                use std::os::windows::ffi::OsStringExt;
                if bytes.len() % 2 != 0 {
                    return Err(Failure::new(Stage::CompilerEnvironment, Kind::NativePath)
                        .expected("ProgramData"));
                }
                let units: Vec<_> = bytes
                    .chunks_exact(2)
                    .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
                    .collect();
                OsString::from_wide(&units)
            };
            #[cfg(unix)]
            let value = {
                use std::os::unix::ffi::OsStringExt;
                OsString::from_vec(bytes)
            };
            SetupStateRoot::new(value)
        }

        fn needs_setup_state_comparison(command: &Command) -> bool {
            let arguments: Vec<_> = command.get_args().map(OsStr::to_owned).collect();
            if arguments.iter().any(|value| {
                value
                    .as_encoded_bytes()
                    .starts_with(STATE_MARKER.as_bytes())
            }) {
                return false;
            }
            let source_values = registered_marker_values(&arguments, MARKER);
            let installed_values = registered_marker_values(&arguments, NATIVE_MARKER);
            let source_count = arguments.iter().filter(|value| *value == HELPER).count();
            let installed_count = arguments
                .iter()
                .filter(|value| *value == NATIVE_HELPER)
                .count();
            let source_helper = source_count == 1
                && installed_count == 0
                && installed_values.is_empty()
                && matches!(source_values.as_slice(), [Some("suite" | "collect")]);
            let installed_helper = installed_count == 1
                && source_count == 0
                && source_values.is_empty()
                && matches!(installed_values.as_slice(), [Some("guest")]);
            source_helper || installed_helper
        }

        fn registered_marker_values<'a>(
            arguments: &'a [OsString],
            prefix: &str,
        ) -> Vec<Option<&'a str>> {
            arguments
                .iter()
                // Retain undecodable registered companions as None records.
                .filter(|value| value.as_encoded_bytes().starts_with(prefix.as_bytes()))
                .map(|value| value.to_str().and_then(|value| value.strip_prefix(prefix)))
                .collect()
        }

        fn capture_setup_state_comparison(
            command: &mut Command,
            mut lookup: impl FnMut(&str) -> Option<OsString>,
        ) -> SetupResult<()> {
            if command.get_args().any(|value| {
                value
                    .as_encoded_bytes()
                    .starts_with(STATE_MARKER.as_bytes())
            }) {
                return Err(
                    Failure::new(Stage::CompilerEnvironment, Kind::DuplicateField)
                        .expected("ProgramData"),
                );
            }
            if !needs_setup_state_comparison(command) {
                return Ok(());
            }
            let state = lookup("ProgramData")
                .ok_or_else(|| {
                    Failure::new(Stage::CompilerEnvironment, Kind::Environment)
                        .expected("ProgramData")
                })
                .and_then(SetupStateRoot::new)?;
            command.arg(state.argument());
            Ok(())
        }

        pub(super) fn transport_setup_state_comparison(command: &mut Command) {
            #[cfg(windows)]
            {
                capture_setup_state_comparison(command, |name| std::env::var_os(name))
                    .expect("invalid machine-wide setup-state comparison root");
            }
            #[cfg(not(windows))]
            {
                let _ = command;
            }
        }

        fn setup_state_environment(command: &mut Command, state: &SetupStateRoot) {
            command.env("ProgramData", &state.value);
        }

        fn strict_discovery_state(
            arguments: impl IntoIterator<Item = OsString>,
        ) -> SetupResult<SetupStateRoot> {
            let state = transferred_setup_state(arguments)?;
            physical_directory(Path::new(&state.value))
                .map_err(|error| error.at(Stage::CompilerEnvironment).expected("ProgramData"))?;
            Ok(state)
        }

        fn comparison_json(bytes: &[u8]) -> SetupResult<Value> {
            use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
            use std::cell::Cell;
            if bytes.len() > CAPTURE_BYTES {
                return Err(Failure::new(Stage::VswhereDiscovery, Kind::Bounds));
            }
            struct Seed<'a> {
                nodes: &'a Cell<usize>,
                depth: usize,
            }
            impl<'de> DeserializeSeed<'de> for Seed<'_> {
                type Value = Value;
                fn deserialize<D: serde::Deserializer<'de>>(
                    self,
                    deserializer: D,
                ) -> Result<Value, D::Error> {
                    if self.depth > 32 || self.nodes.get() >= 4096 {
                        return Err(serde::de::Error::custom("bounded installed instance JSON"));
                    }
                    self.nodes.set(self.nodes.get() + 1);
                    deserializer.deserialize_any(self)
                }
            }
            impl<'de> Visitor<'de> for Seed<'_> {
                type Value = Value;
                fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                    formatter.write_str("bounded duplicate-free installed instance JSON")
                }
                fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
                    Ok(Value::Bool(value))
                }
                fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
                    Ok(Value::from(value))
                }
                fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
                    Ok(Value::from(value))
                }
                fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
                    serde_json::Number::from_f64(value)
                        .map(Value::Number)
                        .ok_or_else(|| E::custom("nonfinite instance number"))
                }
                fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
                    Ok(Value::String(value.to_owned()))
                }
                fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
                    Ok(Value::String(value))
                }
                fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
                    Ok(Value::Null)
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
                    Ok(Value::Null)
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
                    let mut values = Vec::new();
                    while let Some(value) = sequence.next_element_seed(Seed {
                        nodes: self.nodes,
                        depth: self.depth + 1,
                    })? {
                        values.push(value);
                    }
                    Ok(Value::Array(values))
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
                    let mut values = serde_json::Map::new();
                    while let Some(key) = map.next_key::<String>()? {
                        if self.nodes.get() >= 4096 || values.contains_key(&key) {
                            return Err(serde::de::Error::custom(
                                "duplicate or overbound instance member",
                            ));
                        }
                        self.nodes.set(self.nodes.get() + 1);
                        let value = map.next_value_seed(Seed {
                            nodes: self.nodes,
                            depth: self.depth + 1,
                        })?;
                        values.insert(key, value);
                    }
                    Ok(Value::Object(values))
                }
            }
            let nodes = Cell::new(0);
            let mut decoder = serde_json::Deserializer::from_slice(bytes);
            let value = Seed {
                nodes: &nodes,
                depth: 0,
            }
            .deserialize(&mut decoder)
            .map_err(|_| Failure::new(Stage::VswhereDiscovery, Kind::Shape))?;
            decoder
                .end()
                .map_err(|_| Failure::new(Stage::VswhereDiscovery, Kind::Shape))?;
            Ok(value)
        }

        fn summary_string<'a>(
            object: &'a serde_json::Map<String, Value>,
            name: &str,
            bound: usize,
        ) -> SetupResult<Option<&'a str>> {
            match object.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(value)) if value.len() <= bound => Ok(Some(value)),
                _ => Err(Failure::new(Stage::VswhereDiscovery, Kind::Shape)),
            }
        }

        fn summary_bool(
            object: &serde_json::Map<String, Value>,
            name: &str,
        ) -> SetupResult<Option<bool>> {
            match object.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Bool(value)) => Ok(Some(*value)),
                _ => Err(Failure::new(Stage::VswhereDiscovery, Kind::Shape)),
            }
        }

        fn discovery_summary(query: DiscoveryQuery, bytes: &[u8]) -> SetupResult<DiscoverySummary> {
            if matches!(
                query,
                DiscoveryQuery::SameFilterPrerelease | DiscoveryQuery::ProgramDataSameFilter
            ) {
                if bytes.len() > CAPTURE_BYTES {
                    return Err(Failure::new(Stage::VswhereDiscovery, Kind::Bounds));
                }
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| Failure::new(Stage::VswhereDiscovery, Kind::Utf8))?;
                let lines: Vec<_> = text.lines().filter(|line| !line.is_empty()).collect();
                if lines.len() > 1 || lines.iter().any(|line| line.len() > MAX_PATH_UNITS) {
                    return Err(Failure::new(Stage::VswhereDiscovery, Kind::Shape));
                }
                let instances = lines
                    .iter()
                    .map(|line| InstanceSummary {
                        instance_id_sha256: None,
                        installation_path_bytes: Some(line.len()),
                        installation_path_sha256: Some(hash(line.as_bytes())),
                        installation_version: None,
                        is_prerelease: None,
                        is_complete: None,
                        is_launchable: None,
                        is_reboot_required: None,
                        state: None,
                    })
                    .collect();
                return Ok(DiscoverySummary {
                    output_kind: "installation-path-records",
                    records: lines.len(),
                    instances,
                });
            }
            let value = comparison_json(bytes)?;
            let array = value
                .as_array()
                .ok_or_else(|| Failure::new(Stage::VswhereDiscovery, Kind::Shape))?;
            if array.len() > 8 {
                return Err(Failure::new(Stage::VswhereDiscovery, Kind::Bounds));
            }
            let mut instances = Vec::new();
            for value in array {
                let object = value
                    .as_object()
                    .ok_or_else(|| Failure::new(Stage::VswhereDiscovery, Kind::Shape))?;
                let id = summary_string(object, "instanceId", 128)?;
                let path = summary_string(object, "installationPath", MAX_PATH_UNITS)?;
                let version = summary_string(object, "installationVersion", 64)?;
                if version.is_some_and(|version| {
                    !version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                }) {
                    return Err(Failure::new(Stage::VswhereDiscovery, Kind::Shape));
                }
                let state = match object.get("state") {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(
                        value
                            .as_u64()
                            .ok_or_else(|| Failure::new(Stage::VswhereDiscovery, Kind::Shape))?,
                    ),
                };
                instances.push(InstanceSummary {
                    instance_id_sha256: id.map(|id| hash(id.as_bytes())),
                    installation_path_bytes: path.map(str::len),
                    installation_path_sha256: path.map(|path| hash(path.as_bytes())),
                    installation_version: version.map(str::to_owned),
                    is_prerelease: summary_bool(object, "isPrerelease")?,
                    is_complete: summary_bool(object, "isComplete")?,
                    is_launchable: summary_bool(object, "isLaunchable")?,
                    is_reboot_required: summary_bool(object, "isRebootRequired")?,
                    state,
                });
            }
            Ok(DiscoverySummary {
                output_kind: "installed-instance-records",
                records: instances.len(),
                instances,
            })
        }

        fn comparison_result(
            query: DiscoveryQuery,
            host: Host,
            output: SetupResult<std::process::Output>,
        ) -> QueryComparison {
            let arguments = query.arguments(host);
            let required_component_filter = matches!(
                query,
                DiscoveryQuery::SameFilterPrerelease
                    | DiscoveryQuery::StableComponentInstances
                    | DiscoveryQuery::PrereleaseComponentInstances
                    | DiscoveryQuery::ProgramDataSameFilter
            )
            .then(|| host.component());
            match output {
                Err(mut failure) => {
                    // Unfiltered installed-tool metadata may contain private display/
                    // property strings. Keep prefix sizes/hashes, never dump those bytes.
                    if let Some(stdout) = failure
                        .process
                        .as_mut()
                        .and_then(|process| process.stdout.as_mut())
                    {
                        stdout.excerpt_hex = None;
                        stdout.contents_withheld = stdout.captured_bytes.is_some();
                        stdout.excerpt_cut = false;
                    }
                    QueryComparison {
                        query,
                        arguments,
                        required_component_filter,
                        output: None,
                        summary: None,
                        failure: Some(Box::new(failure)),
                    }
                }
                Ok(output) => {
                    let facts = Failure::new(Stage::VswhereDiscovery, Kind::Shape)
                        .output(&output, false)
                        .process;
                    let summary = if output.status.success() {
                        discovery_summary(query, &output.stdout)
                    } else {
                        // Actual exit/capture data is in `output`; do not duplicate an
                        // unfiltered stdout excerpt into the failed summary record.
                        Err(Failure::new(Stage::VswhereDiscovery, Kind::Exit))
                    };
                    match summary {
                        Ok(summary) => QueryComparison {
                            query,
                            arguments,
                            required_component_filter,
                            output: facts,
                            summary: Some(summary),
                            failure: None,
                        },
                        Err(failure) => QueryComparison {
                            query,
                            arguments,
                            required_component_filter,
                            output: facts,
                            summary: None,
                            failure: Some(Box::new(failure)),
                        },
                    }
                }
            }
        }

        #[cfg(windows)]
        fn compare_discovery(
            vswhere: &Path,
            selected: &DiscoveryRoot,
            system: &Path,
            host: Host,
        ) -> DiscoveryComparisons {
            let program = vswhere.as_os_str().as_encoded_bytes();
            let mut queries = Vec::new();
            for query in [
                DiscoveryQuery::SameFilterPrerelease,
                DiscoveryQuery::StableInstances,
                DiscoveryQuery::PrereleaseInstances,
                DiscoveryQuery::StableComponentInstances,
                DiscoveryQuery::PrereleaseComponentInstances,
            ] {
                let mut command = Command::new(vswhere);
                let output = base(&mut command, selected, system).and_then(|_| {
                    command.args(query.arguments(host));
                    bounded_at(&mut command, 15, Stage::VswhereDiscovery)
                });
                queries.push(comparison_result(query, host, output));
            }
            let program_data_query = DiscoveryQuery::ProgramDataSameFilter;
            let mut native_bytes = None;
            let mut native_sha256 = None;
            let mut physical_directory_accepted = None;
            let program_data_output =
                transferred_setup_state(std::env::args_os()).and_then(|state| {
                    let encoded = state.encoded();
                    native_bytes = Some(encoded.len());
                    native_sha256 = Some(hash(&encoded));
                    if let Err(error) = physical_directory(Path::new(&state.value)) {
                        physical_directory_accepted = Some(false);
                        return Err(error.at(Stage::CompilerEnvironment).expected("ProgramData"));
                    }
                    physical_directory_accepted = Some(true);
                    let mut command = Command::new(vswhere);
                    base(&mut command, selected, system)?;
                    setup_state_environment(&mut command, &state);
                    command.args(program_data_query.arguments(host));
                    bounded_at(&mut command, 15, Stage::VswhereDiscovery)
                });
            DiscoveryComparisons {
                program_encoded_bytes: program.len(),
                program_encoded_sha256: hash(program),
                program_encoded_hex: facts::hex(&program[..program.len().min(2048)]),
                program_encoding: "same-native-OsStr-encoded-bytes-as-original-query",
                program_excerpt_cut: program.len() > 2048,
                comparison_only_not_source_or_compiler_selection: true,
                // The selected strict query now carries ProgramData. These
                // five historical controls retain the original minimal base.
                same_cleared_preparation_environment: false,
                queries,
                program_data_same_query: Some(SetupStateComparison {
                    source: "registered-native-supervisor-ProgramData",
                    only_added_environment_field: "ProgramData",
                    original_program_arguments_and_base_unchanged: true,
                    native_encoding: "UTF-16LE-native-Windows-units",
                    native_bytes,
                    native_sha256,
                    physical_directory_accepted,
                    query: comparison_result(program_data_query, host, program_data_output),
                }),
            }
        }

        fn linker_banner(output: &std::process::Output) -> SetupResult<()> {
            facts::require_exit(Stage::LinkerBanner, output)?;
            if !output
                .stdout
                .windows(b"Microsoft".len())
                .any(|s| s == b"Microsoft")
                || !output
                    .stdout
                    .windows(b"Linker".len())
                    .any(|s| s == b"Linker")
            {
                return Err(Failure::new(Stage::LinkerBanner, Kind::Banner).output(output, true));
            }
            Ok(())
        }

        fn pe_identity(header: &[u8], host: Host) -> SetupResult<()> {
            if pe_matches(header, host) {
                return Ok(());
            }
            let mut error = Failure::new(Stage::PeIdentity, Kind::Pe);
            error.observed_read_bytes = Some(header.len());
            error.observed_read_prefix_sha256 = Some(hash(header));
            if header.len() >= 64 && &header[..2] == b"MZ" {
                let offset = u32::from_le_bytes(header[60..64].try_into().unwrap()) as usize;
                if offset.checked_add(6).is_some_and(|end| end <= header.len())
                    && &header[offset..offset + 4] == b"PE\0\0"
                {
                    error.observed_pe_machine = Some(u16::from_le_bytes(
                        header[offset + 4..offset + 6].try_into().unwrap(),
                    ));
                }
            }
            Err(error)
        }

        fn read_pe_header(path: &Path) -> SetupResult<Vec<u8>> {
            let file = std::fs::File::open(path)
                .map_err(|error| Failure::io(Stage::PeIdentity, Kind::Read, &error))?;
            let mut header = Vec::new();
            if let Err(error) = file.take(CAPTURE_BYTES as u64).read_to_end(&mut header) {
                let mut failure = Failure::io(Stage::PeIdentity, Kind::Read, &error);
                if !header.is_empty() {
                    failure.observed_read_bytes = Some(header.len());
                    failure.observed_read_prefix_sha256 = Some(hash(&header));
                }
                return Err(failure);
            }
            Ok(header)
        }

        pub(super) struct Prepared {
            host: Host,
            linker: PathBuf,
            lib: OsString,
            path: OsString,
        }
        impl Prepared {
            pub(super) fn environment(self) -> Vec<(&'static str, OsString)> {
                vec![
                    (self.host.linker_key(), self.linker.into_os_string()),
                    ("LIB", self.lib),
                    ("PATH", self.path),
                ]
            }
        }
        #[cfg(windows)]
        pub(super) fn prepare(selected: &DiscoveryRoot) -> SetupResult<Prepared> {
            use std::os::windows::process::CommandExt;
            let state = strict_discovery_state(std::env::args_os())?;
            let host = host()?;
            let system = checked_path(std::env::var_os("SystemRoot").ok_or_else(|| {
                Failure::new(Stage::SystemRoot, Kind::Environment).expected("SystemRoot")
            })?)
            .map_err(|error| error.at(Stage::SystemRoot))?;
            let rustc = Path::new(env!("CARGO")).with_file_name("rustc.exe");
            let mut identify = Command::new(&rustc);
            base(&mut identify, selected, &system)?;
            identify.arg("-vV");
            let output = bounded_at(&mut identify, 5, Stage::RustcIdentity)?;
            rustc_identity(&output, host)?;

            let vswhere = Path::new(&selected.value)
                .join("Microsoft Visual Studio")
                .join("Installer")
                .join("vswhere.exe");
            physical_file(&vswhere).map_err(|error| error.at(Stage::VswhereFile))?;
            let mut discover = Command::new(&vswhere);
            base(&mut discover, selected, &system)?;
            setup_state_environment(&mut discover, &state);
            discover.args(original_discovery_arguments(host));
            let output = bounded_at(&mut discover, 15, Stage::VswhereDiscovery)?;
            let installation = match installation_line(&output) {
                Ok(installation) => installation,
                Err(mut failure) => {
                    failure.discovery_comparisons =
                        Some(compare_discovery(&vswhere, selected, &system, host));
                    return Err(failure);
                }
            };
            let installation = physical_directory(
                &checked_path(OsString::from(installation))
                    .map_err(|error| error.at(Stage::Installation).output(&output, false))?,
            )
            .map_err(|error| error.at(Stage::Installation).output(&output, false))?;
            let directory = installation.join("Common7").join("Tools");
            physical_file(&directory.join("VsDevCmd.bat"))
                .map_err(|error| error.at(Stage::VsdevcmdFile))?;
            let cmd = system.join("System32").join("cmd.exe");
            physical_file(&cmd).map_err(|error| error.at(Stage::CmdFile))?;
            let mut setup = Command::new(&cmd);
            base(&mut setup, selected, &system)?;
            setup
                .current_dir(&directory)
                .args(["/d", "/u", "/v:off", "/s", "/c"])
                .raw_arg(host.command());
            let output = bounded_at(&mut setup, 20, Stage::VsdevcmdSetup)?;
            facts::require_exit(Stage::VsdevcmdSetup, &output)?;
            let fields = parse_utf16(&output.stdout, host).map_err(|error| {
                command_comparison::attach(
                    error.output(&output, false),
                    &setup,
                    selected,
                    &system,
                    host,
                )
            })?;
            let vc = physical_directory(
                &checked_path(native(&fields.values["VCTOOLSINSTALLDIR"])).map_err(|error| {
                    error
                        .at(Stage::VcDirectory)
                        .expected("VCTOOLSINSTALLDIR")
                        .output(&output, false)
                })?,
            )
            .map_err(|error| {
                error
                    .at(Stage::VcDirectory)
                    .expected("VCTOOLSINSTALLDIR")
                    .output(&output, false)
            })?;
            let sdk = physical_directory(
                &checked_path(native(&fields.values["WINDOWSSDKDIR"])).map_err(|error| {
                    error
                        .at(Stage::SdkDirectory)
                        .expected("WINDOWSSDKDIR")
                        .output(&output, false)
                })?,
            )
            .map_err(|error| {
                error
                    .at(Stage::SdkDirectory)
                    .expected("WINDOWSSDKDIR")
                    .output(&output, false)
            })?;
            let canonical_vc = std::fs::canonicalize(&vc)
                .map_err(|error| Failure::io(Stage::VcContainment, Kind::NativePath, &error))?;
            let canonical_installation = std::fs::canonicalize(&installation)
                .map_err(|error| Failure::io(Stage::VcContainment, Kind::NativePath, &error))?;
            if !canonical_vc
                .starts_with(canonical_installation.join("VC").join("Tools").join("MSVC"))
            {
                return Err(Failure::new(Stage::VcContainment, Kind::Containment));
            }
            let linker = vc
                .join("bin")
                .join(format!("Host{}", host.architecture()))
                .join(host.architecture())
                .join("link.exe");
            physical_file(&linker).map_err(|error| error.at(Stage::LinkerFile))?;
            let header = read_pe_header(&linker)?;
            pe_identity(&header, host)?;
            let raw_lib =
                split_lib(&fields.values["LIB"]).map_err(|error| error.output(&output, false))?;
            let mut dirs = Vec::new();
            for units in &raw_lib {
                dirs.push(
                    physical_directory(&checked_path(native(units)).map_err(|error| {
                        error
                            .at(Stage::LibDirectory)
                            .expected("LIB")
                            .output(&output, false)
                    })?)
                    .map_err(|error| {
                        error
                            .at(Stage::LibDirectory)
                            .expected("LIB")
                            .output(&output, false)
                    })?,
                );
            }
            // Paths came only from official setup. All inputs remain complete;
            // required Rust/MSVC system libraries must actually exist there.
            for name in [
                "kernel32.lib",
                "ntdll.lib",
                "userenv.lib",
                "ws2_32.lib",
                "dbghelp.lib",
                "msvcrt.lib",
            ] {
                required_library(name, &dirs)?;
            }
            if !dirs.iter().any(|dir| dir.starts_with(&vc))
                || !dirs.iter().any(|dir| dir.starts_with(&sdk))
            {
                return Err(Failure::new(Stage::LibraryRoots, Kind::LibraryRoots));
            }
            let lib = std::env::join_paths(&dirs).map_err(|_| {
                Failure::new(Stage::CompilerEnvironment, Kind::PathList).expected("LIB")
            })?;
            let path = std::env::join_paths([
                linker
                    .parent()
                    .ok_or_else(|| Failure::new(Stage::CompilerEnvironment, Kind::NativePath))?,
                Path::new(env!("CARGO"))
                    .parent()
                    .ok_or_else(|| Failure::new(Stage::CompilerEnvironment, Kind::NativePath))?,
                system.join("System32").as_path(),
            ])
            .map_err(|_| {
                Failure::new(Stage::CompilerEnvironment, Kind::PathList).expected("PATH")
            })?;
            let prepared = Prepared {
                host,
                linker,
                lib,
                path,
            };
            let mut banner = Command::new(&prepared.linker);
            base(&mut banner, selected, &system)?;
            banner.envs(prepared_values(&prepared));
            linker_data_fixture::run(&mut banner, host)?;
            Ok(prepared)
        }
        #[cfg(windows)]
        fn prepared_values(prepared: &Prepared) -> Vec<(&'static str, OsString)> {
            vec![
                (
                    prepared.host.linker_key(),
                    prepared.linker.as_os_str().to_owned(),
                ),
                ("LIB", prepared.lib.clone()),
                ("PATH", prepared.path.clone()),
            ]
        }

        mod command_comparison {
            include!("privacy_msvc_command_comparison.rs");
        }

        mod linker_data_fixture {
            include!("privacy_linker_data_fixture.rs");
        }

        #[test]
        fn native_setup_utf16_preserves_unicode_equals_quotes_and_opaque_units() {
            let vc = r"C:\VS 中文 & % ! (tools)\VC\Tools\MSVC\selected\";
            let lines = [
                format!("VCToolsInstallDir={vc}"),
                r"WindowsSdkDir=C:\Windows Kits\10\".into(),
                "VSCMD_ARG_HOST_ARCH=x64".into(),
                "VSCMD_ARG_TGT_ARCH=x64".into(),
                r#"LIB="C:\VS 中文\lib;x64";C:\Windows Kits\10\um\x64"#.into(),
                r"LIBPATH=C:\not-forwarded".into(),
            ];
            let mut value: Vec<u16> = lines.join("\r\n").encode_utf16().collect();
            let at = value.iter().position(|u| *u == 59).unwrap();
            value.insert(at, 0xd800);
            let bytes: Vec<u8> = value.iter().flat_map(|u| u.to_le_bytes()).collect();
            let parsed = parse_utf16(&bytes, Host::X64).unwrap();
            assert!(parsed.values["LIB"].contains(&0xd800));
            let paths = split_lib(&parsed.values["LIB"]).unwrap();
            assert_eq!(paths.len(), 2);
            assert!(paths[0].contains(&59));
            assert_eq!(
                parsed.values["VCTOOLSINSTALLDIR"],
                vc.encode_utf16().collect::<Vec<_>>()
            );
            assert!(parsed.values.contains_key("LIBPATH"));
        }
        #[test]
        fn setup_rejects_incomplete_ambiguous_overbound_and_wrong_host_outputs() {
            let encode = |s: &str| {
                s.encode_utf16()
                    .flat_map(|u| u.to_le_bytes())
                    .collect::<Vec<_>>()
            };
            let valid = [
                r"VCToolsInstallDir=C:\VC\",
                r"WindowsSdkDir=C:\SDK\",
                "VSCMD_ARG_HOST_ARCH=x64",
                "VSCMD_ARG_TGT_ARCH=x64",
                r"LIB=C:\VC\lib;C:\SDK\lib",
            ]
            .join("\r\n");
            assert!(parse_utf16(&encode(&valid), Host::X64).is_ok());
            for bad in [
                valid.replace("x64", "x86"),
                valid.replace("LIB=", "missing="),
                format!("{valid}\r\nLIB=duplicate"),
                format!("{valid}\r\nUSER_SECRET=not-permitted"),
                valid.replace("C:\\VC", "bad\0value"),
            ] {
                assert!(parse_utf16(&encode(&bad), Host::X64).is_err());
            }
            let mut odd = encode(&valid);
            odd.pop();
            assert!(parse_utf16(&odd, Host::X64).is_err());
            assert!(parse_utf16(&vec![65; CAPTURE_BYTES + 2], Host::X64).is_err());
            for bad in [
                "",
                ";",
                r"C:\one;;",
                r";C:\two",
                r#""C:\unterminated"#,
                "C:\\first\r\n",
            ] {
                assert!(split_lib(&bad.encode_utf16().collect::<Vec<_>>()).is_err());
            }
            let terminated =
                split_lib(&r"C:\one;C:\two;".encode_utf16().collect::<Vec<_>>()).unwrap();
            assert_eq!(terminated.len(), 2);
            assert!(terminated.iter().all(|path| !path.is_empty()));
        }
        #[test]
        fn msvc_prepared_settings_are_host_only_and_pe_host_identity_is_checked() {
            for host in [Host::X64, Host::X86, Host::Arm64] {
                let mut pe = vec![0; 512];
                pe[..2].copy_from_slice(b"MZ");
                pe[60..64].copy_from_slice(&128u32.to_le_bytes());
                pe[128..132].copy_from_slice(b"PE\0\0");
                pe[132..134].copy_from_slice(&host.pe_machine().to_le_bytes());
                assert!(pe_matches(&pe, host));
                let wrong = if host == Host::X64 {
                    Host::X86
                } else {
                    Host::X64
                };
                assert!(!pe_matches(&pe, wrong));
                pe[128] = 0;
                assert!(!pe_matches(&pe, host));
                let prepared = Prepared {
                    host,
                    linker: PathBuf::from("synthetic-owned-msvc-link.exe"),
                    lib: OsString::from("synthetic-native-libraries"),
                    path: OsString::from("synthetic-owned-tool-bins"),
                };
                let environment: BTreeMap<_, _> = prepared.environment().into_iter().collect();
                assert_eq!(environment.len(), 3);
                assert!(environment.contains_key(host.linker_key()));
                assert!(!environment.keys().any(|key| key.contains("WASM32")
                    || key.contains("RUSTFLAGS")
                    || key.contains("HOME")
                    || key.contains("CREDENTIAL")));
                assert!(host.target().ends_with("-pc-windows-msvc"));
                assert!(host.command().contains("call VsDevCmd.bat"));
                assert!(
                    !host.command().contains("%CRAFTSMAN") && !host.command().contains("echo %")
                );
            }
        }
        #[cfg(unix)]
        #[tokio::test]
        async fn explicit_host_cargo_linker_controls_real_build_script_without_changing_wasm_linker()
         {
            let home = tempfile::tempdir().unwrap();
            let project = home.path().join("project");
            std::fs::create_dir_all(project.join("src")).unwrap();
            std::fs::write(project.join("Cargo.toml"),"[package]\nname=\"msvc-host-contract\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n[lib]\ncrate-type=[\"cdylib\"]\n").unwrap();
            std::fs::write(
                project.join("Cargo.lock"),
                "version=4\n[[package]]\nname=\"msvc-host-contract\"\nversion=\"0.1.0\"\n",
            )
            .unwrap();
            std::fs::write(
                project.join("build.rs"),
                "fn main(){println!(\"cargo:rerun-if-changed=build.rs\");}\n",
            )
            .unwrap();
            std::fs::write(project.join("src/lib.rs"),"#![no_std]\n#[panic_handler] fn panic(_: &core::panic::PanicInfo)->!{loop{}}\n#[unsafe(no_mangle)] pub extern \"C\" fn value()->u32{19}\n").unwrap();
            let cargo = Path::new(env!("CARGO"));
            let tools = cargo.parent().unwrap();
            let host_output = bounded(
                {
                    let mut command = Command::new(tools.join("rustc"));
                    command.env_clear().env("PATH", "/usr/bin:/bin").arg("-vV");
                    command
                },
                5,
            )
            .unwrap();
            let text = std::str::from_utf8(&host_output.stdout).unwrap();
            let target = text
                .lines()
                .find_map(|line| line.strip_prefix("host: "))
                .unwrap();
            let key = format!(
                "CARGO_TARGET_{}_LINKER",
                target.to_ascii_uppercase().replace('-', "_")
            );
            let utility = if cfg!(target_os = "macos") {
                "/bin/link"
            } else {
                "/usr/bin/link"
            };
            for (label, linker, success) in
                [("wrong", utility, false), ("native", "/usr/bin/cc", true)]
            {
                let build = home.path().join(label);
                let mut command = Command::new(cargo);
                command
                    .env_clear()
                    .env("HOME", home.path())
                    .env("USERPROFILE", home.path())
                    .env("TMPDIR", home.path())
                    .env("CARGO_HOME", home.path().join("empty-cargo-home"))
                    .env(
                        "PATH",
                        std::env::join_paths([tools, Path::new("/usr/bin"), Path::new("/bin")])
                            .unwrap(),
                    )
                    .env("RUSTC", tools.join("rustc"))
                    .env("RUSTDOC", tools.join("rustdoc"))
                    .env(&key, linker);
                #[cfg(target_os = "macos")]
                command.env("DYLD_LIBRARY_PATH", tools.parent().unwrap().join("lib"));
                command
                    .args([
                        "build",
                        "--offline",
                        "--locked",
                        "--release",
                        "--target",
                        "wasm32-wasip2",
                        "--manifest-path",
                    ])
                    .arg(project.join("Cargo.toml"))
                    .arg("--target-dir")
                    .arg(&build);
                let result = bounded(command, 20).unwrap();
                assert_eq!(
                    result.status.success(),
                    success,
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                if success {
                    assert!(
                        build
                            .join("wasm32-wasip2/release/msvc_host_contract.wasm")
                            .is_file()
                    );
                    assert!(
                        !std::str::from_utf8(&result.stderr)
                            .unwrap()
                            .lines()
                            .any(|line| line.starts_with("warning:"))
                    );
                } else {
                    let error = std::str::from_utf8(&result.stderr).unwrap();
                    assert!(error.contains(utility) && error.contains("build script"));
                    println!("ACTUAL-HOST-LINKER-FAILURE {error}");
                }
            }
        }
        #[cfg(unix)]
        #[test]
        fn bounded_capture_observes_complete_native_streams_and_refuses_overflow() {
            let make = |script: &str| {
                let mut command = Command::new("/bin/sh");
                command
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .args(["-c", script]);
                command
            };
            let output = bounded(
                make("printf 'actual stdout'; printf 'actual stderr' >&2"),
                5,
            )
            .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"actual stdout");
            assert_eq!(output.stderr, b"actual stderr");
            assert!(bounded(make("head -c 65537 /dev/zero"), 5).is_err());
            assert_eq!(bounded(make("exit 7"), 5).unwrap().status.code(), Some(7));
            let started = Instant::now();
            assert!(bounded(make("printf 'entered'; exec sleep 30"), 1).is_err());
            assert!(started.elapsed() < Duration::from_secs(7));
        }
        const NATIVE_HELPER: &str = "gadgets::privacy_detector_tests::compiler_environment::msvc_setup::installed_msvc_guest_fixture";
        const NATIVE_MARKER: &str = "craftsman-detector-msvc=";
        fn native_selected(arguments: impl IntoIterator<Item = OsString>) -> bool {
            let values: Vec<_> = arguments
                .into_iter()
                .filter(|arg| arg.as_encoded_bytes().starts_with(NATIVE_MARKER.as_bytes()))
                .collect();
            matches!(values.as_slice(),[value] if value.to_str().and_then(|value|value.strip_prefix(NATIVE_MARKER))==Some("guest"))
        }
        #[test]
        fn new_msvc_native_marker_refuses_registered_malformed_companions_before_decoding() {
            assert!(!native_selected([]));
            assert!(!native_selected([OsString::from(format!(
                "{NATIVE_MARKER}unknown"
            ))]));
            assert!(!native_selected([
                OsString::from(format!("{NATIVE_MARKER}guest")),
                OsString::from(format!("{NATIVE_MARKER}guest"))
            ]));
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                let mut malformed = NATIVE_MARKER.as_bytes().to_vec();
                malformed.push(255);
                assert!(!native_selected([
                    OsString::from(format!("{NATIVE_MARKER}guest")),
                    OsString::from_vec(malformed)
                ]));
            }
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStringExt;
                let mut malformed: Vec<u16> = NATIVE_MARKER.encode_utf16().collect();
                malformed.push(0xd800);
                assert!(!native_selected([
                    OsString::from(format!("{NATIVE_MARKER}guest")),
                    OsString::from_wide(&malformed)
                ]));
            }
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[ignore = "installed MSVC source/guest case; exactly one native marker required"]
        async fn installed_msvc_guest_fixture() {
            if !native_selected(std::env::args_os()) {
                println!("INSTALLED-MSVC:NOT-SELECTED");
                return;
            }
            #[cfg(not(windows))]
            panic!("installed MSVC source case requires its actual Windows host");
            #[cfg(windows)]
            {
                let selected = transferred(|name| std::env::var_os(name))
                    .unwrap()
                    .or_else(host_discovery)
                    .unwrap();
                let prepared =
                    prepare(&selected).expect("installed MSVC compiler preparation failed");
                assert!(std::env::var_os(prepared.host.linker_key()).is_none());
                assert!(std::env::var_os("LIB").is_none());
                let repo = root();
                let before = sources(&repo);
                let home = tempfile::tempdir().unwrap();
                let build = home.path().join("cold-wasm-build");
                assert!(!build.exists());
                let mut command = Command::new(env!("CARGO"));
                command
                    .args([
                        "build",
                        "--offline",
                        "--locked",
                        "--release",
                        "--target",
                        "wasm32-wasip2",
                        "--manifest-path",
                    ])
                    .arg(
                        repo.join("crates/craftsman-rs/tests/fixtures/privacy-detector/Cargo.toml"),
                    )
                    .arg("--target-dir")
                    .arg(&build)
                    .envs(prepared.environment())
                    .env_remove(TRANSPORT_X86)
                    .env_remove(TRANSPORT_NATIVE);
                let output =
                    bounded(command, 180).expect("installed MSVC source build did not complete");
                assert!(
                    output.status.success(),
                    "installed MSVC trusted offline source build failed"
                );
                assert_eq!(before, sources(&repo));
                let image = std::fs::read(
                    build.join("wasm32-wasip2/release/privacy_detector_fixture.wasm"),
                )
                .unwrap();
                let workspace = home.path().join("workspace");
                let config = home.path().join("config");
                let bundle = config.join("gadgets/privacy-detector");
                std::fs::create_dir_all(&workspace).unwrap();
                std::fs::create_dir_all(&bundle).unwrap();
                std::fs::copy(
                    repo.join("crates/craftsman-rs/tests/fixtures/privacy-detector/gadget.toml"),
                    bundle.join("gadget.toml"),
                )
                .unwrap();
                std::fs::write(bundle.join("gadget.wasm"), &image).unwrap();
                let options = RuntimeOptions {
                    disk_cache: false,
                    ..Default::default()
                };
                assert_eq!(options.timeout, Duration::from_secs(30));
                assert_eq!(options.memory_bytes, 256 * 1024 * 1024);
                let denied = GadgetRuntime::new(&workspace, &config, options.clone()).unwrap();
                let list = denied.list().await.unwrap();
                assert_eq!(list[0].status, "needs-approval");
                assert!(list[0].functions.is_empty());
                assert_eq!(denied.compiled_count(), 0);
                drop(denied);
                // The dispatched task approves this exact source-built synthetic image.
                let runtime = GadgetRuntime::new(
                    &workspace,
                    &config,
                    RuntimeOptions {
                        allowed_gadgets: BTreeSet::from([ID.into()]),
                        ..options
                    },
                )
                .unwrap();
                let input = nodes(&[(
                    "request/0/text",
                    "中文 alice@example.invalid +886 912 345 678",
                )]);
                let request = admitted_request(&input).unwrap();
                let result = runtime.call(ID, "detect", request).await.unwrap();
                assert_eq!(
                    validate_returned(&input, &result.json).unwrap().spans.len(),
                    2
                );
                assert_eq!(result.network_calls, 0);
                assert!(result.logs.is_empty());
                assert!(!result.logs_truncated);
                assert_eq!(before, sources(&repo));
                println!("INSTALLED-MSVC:SOURCE-GUEST-COMPLETE");
            }
        }
        #[tokio::test]
        async fn unmarked_new_setup_helper_returns_before_environment_paths_or_stdin() {
            for markers in [vec![], vec!["unknown"], vec!["guest", "guest"]] {
                let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
                command
                    .args(["--ignored", "--exact", NATIVE_HELPER, "--nocapture"])
                    .env_clear()
                    .env(TRANSPORT_X86, "relative poisoned root");
                for marker in markers {
                    command.arg(format!("{NATIVE_MARKER}{marker}"));
                }
                if let Some(root) = system_root() {
                    command.env("SystemRoot", root);
                }
                let observed = super::output(command).await;
                assert!(observed.status.success());
                assert!(
                    std::str::from_utf8(&observed.stdout)
                        .unwrap()
                        .contains("INSTALLED-MSVC:NOT-SELECTED")
                );
            }
        }
        #[cfg(windows)]
        #[tokio::test]
        async fn installed_msvc_host_linker_and_libraries_source_build_and_execute_the_guest() {
            let home = tempfile::tempdir().unwrap();
            let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--ignored", "--exact", NATIVE_HELPER, "--nocapture"])
                .arg(format!("{NATIVE_MARKER}guest"));
            isolated_supervisor_environment(
                command.as_std_mut(),
                home.path(),
                &std::env::var_os("CARGO_HOME")
                    .or_else(|| {
                        std::env::var_os("HOME")
                            .map(|home| PathBuf::from(home).join(".cargo").into_os_string())
                    })
                    .unwrap(),
                &std::env::var_os("PATH").unwrap(),
                system_root(),
                host_discovery(),
            );
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = command.spawn().unwrap();
            let mut out = child.stdout.take().unwrap().take(65537);
            let mut err = child.stderr.take().unwrap().take(65537);
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let (a, b, status) = match tokio::time::timeout(Duration::from_secs(240), async {
                tokio::join!(
                    out.read_to_end(&mut stdout),
                    err.read_to_end(&mut stderr),
                    child.wait()
                )
            })
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    child.start_kill().unwrap();
                    tokio::time::timeout(Duration::from_secs(5), child.wait())
                        .await
                        .unwrap()
                        .unwrap();
                    panic!("installed MSVC native case exceeded its owned bound");
                }
            };
            a.unwrap();
            b.unwrap();
            assert!(stdout.len() <= CAPTURE_BYTES && stderr.len() <= CAPTURE_BYTES);
            if !status.as_ref().is_ok_and(|status| status.success()) {
                facts::helper_failure(&stdout, &stderr);
            }
            assert!(
                status.unwrap().success(),
                "installed MSVC source/guest helper failed"
            );
            assert!(
                std::str::from_utf8(&stdout)
                    .unwrap()
                    .contains("INSTALLED-MSVC:SOURCE-GUEST-COMPLETE")
            );
        }

        const FACT_HELPER: &str = "gadgets::privacy_detector_tests::compiler_environment::msvc_setup::diagnostic_facts_native_fixture";
        const FACT_MARKER: &str = "craftsman-msvc-stage-facts=";
        const SYNTHETIC_CREDENTIAL: &str = "CRAFTSMAN_TEST_MSVC_STAGE_CREDENTIAL";

        fn fact_mode(arguments: impl IntoIterator<Item = OsString>) -> Option<String> {
            let values: Vec<_> = arguments
                .into_iter()
                .filter(|value| value.as_encoded_bytes().starts_with(FACT_MARKER.as_bytes()))
                .collect();
            match values.as_slice() {
                [value] => value
                    .to_str()
                    .and_then(|value| value.strip_prefix(FACT_MARKER))
                    .filter(|value| {
                        matches!(
                            *value,
                            "streams"
                                | "fields"
                                | "overflow"
                                | "sleep"
                                | "env"
                                | "credential-parent"
                        )
                    })
                    .map(str::to_owned),
                _ => None,
            }
        }

        fn fact_command(mode: &str) -> Command {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--ignored", "--exact", FACT_HELPER, "--nocapture"])
                .arg(format!("{FACT_MARKER}{mode}"))
                .env_clear();
            if let Some(root) = system_root() {
                command.env("SystemRoot", root);
            }
            command
        }

        #[test]
        #[ignore = "owned compiler diagnostic control; exactly one registered native marker required"]
        fn diagnostic_facts_native_fixture() {
            let Some(mode) = fact_mode(std::env::args_os()) else {
                println!("MSVC-FACTS:NOT-SELECTED");
                return;
            };
            match mode.as_str() {
                "streams" => {
                    std::io::stdout()
                        .write_all(b"actual stdout\0\xff\x1b[31m\n")
                        .unwrap();
                    std::io::stderr()
                        .write_all(b"actual stderr\0\xfe\n")
                        .unwrap();
                    std::io::stdout().flush().unwrap();
                    std::io::stderr().flush().unwrap();
                    std::process::exit(7);
                }
                "fields" => {
                    let text = "UNREGISTERED=synthetic-private-field-value\r\n";
                    let bytes: Vec<_> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
                    std::io::stdout().write_all(&bytes).unwrap();
                    std::io::stderr()
                        .write_all(b"actual field probe stderr")
                        .unwrap();
                    std::io::stdout().flush().unwrap();
                    std::io::stderr().flush().unwrap();
                    std::process::exit(0);
                }
                "overflow" => {
                    std::io::stdout()
                        .write_all(&vec![b'x'; CAPTURE_BYTES + 1])
                        .unwrap();
                    std::io::stdout().flush().unwrap();
                    std::process::exit(0);
                }
                "sleep" => {
                    std::io::stdout()
                        .write_all(b"actual diagnostic sleeper entered")
                        .unwrap();
                    std::io::stdout().flush().unwrap();
                    std::thread::sleep(Duration::from_secs(30));
                    std::process::exit(0);
                }
                "env" => {
                    assert!(
                        std::env::var_os(SYNTHETIC_CREDENTIAL).is_none(),
                        "compiler child retained excluded credential"
                    );
                    std::io::stdout()
                        .write_all(b"actual isolated credential absent")
                        .unwrap();
                    std::io::stderr()
                        .write_all(b"actual isolated failure")
                        .unwrap();
                    std::io::stdout().flush().unwrap();
                    std::io::stderr().flush().unwrap();
                    std::process::exit(17);
                }
                "credential-parent" => {
                    assert!(std::env::var_os(SYNTHETIC_CREDENTIAL).is_some());
                    let home = std::env::var_os("HOME").unwrap();
                    let selected = DiscoveryRoot::new("ProgramFiles(x86)", home.clone()).unwrap();
                    let system = system_root()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from(&home));
                    let mut child = fact_command("env");
                    base_values(&mut child, &selected, &system, |name| {
                        std::env::var_os(name)
                    })
                    .unwrap();
                    let output = bounded_at(&mut child, 5, Stage::RustcIdentity).unwrap();
                    assert_eq!(output.status.code(), Some(17));
                    let failure = facts::output_failure(Stage::RustcIdentity, &output);
                    println!("{failure:?}");
                }
                _ => unreachable!("registered diagnostic fixture mode"),
            }
        }

        #[test]
        fn diagnostic_fields_classify_whole_input_without_raw_unknown_values() {
            let encode = |text: &str| {
                text.encode_utf16()
                    .flat_map(|u| u.to_le_bytes())
                    .collect::<Vec<_>>()
            };
            let valid = [
                r"VCToolsInstallDir=C:\VC\",
                r"WindowsSdkDir=C:\SDK\",
                "VSCMD_ARG_HOST_ARCH=x64",
                "VSCMD_ARG_TGT_ARCH=x64",
                r"LIB=C:\VC\lib;C:\SDK\lib",
            ]
            .join("\r\n");
            for (bytes, expected) in [
                (Vec::new(), Kind::Empty),
                (vec![1], Kind::OddUtf16),
                (vec![0; CAPTURE_BYTES + 2], Kind::Bounds),
                (encode("nul\0"), Kind::Nul),
                (encode("malformed"), Kind::Shape),
                (
                    encode("UNKNOWN=synthetic-private-field-value"),
                    Kind::UnknownField,
                ),
                (
                    encode(&format!("{valid}\r\nLIB=duplicate")),
                    Kind::DuplicateField,
                ),
                (
                    encode(&valid.replace(r"LIB=C:\VC\lib;C:\SDK\lib", "LIB=")),
                    Kind::FieldValue,
                ),
            ] {
                let error = parse_utf16(&bytes, Host::X64).err().unwrap();
                assert_eq!(error.stage, Stage::Utf16Fields);
                assert_eq!(error.kind, expected);
                assert!(error.process.is_none());
                let text = format!("{error:?}");
                assert!(!text.contains("synthetic-private-field-value"));
                assert!(!text.contains(&facts::hex(b"synthetic-private-field-value")));
            }
            let missing = valid
                .lines()
                .filter(|line| !line.starts_with("LIB="))
                .collect::<Vec<_>>()
                .join("\r\n");
            let error = parse_utf16(&encode(&missing), Host::X64).err().unwrap();
            assert_eq!(error.kind, Kind::MissingField);
            assert_eq!(error.expected, Some("LIB"));
            let error = parse_utf16(
                &encode(&valid.replace("HOST_ARCH=x64", "HOST_ARCH=x86")),
                Host::X64,
            )
            .err()
            .unwrap();
            assert_eq!(error.kind, Kind::HostMismatch);
            assert_eq!(error.expected, Some("VSCMD_ARG_HOST_ARCH"));
            for host in [Host::X64, Host::X86, Host::Arm64] {
                assert!(host.command().contains("1>&2"));
                assert!(!host.command().contains("2>nul"));
                assert!(!host.command().contains("echo %"));
            }
        }

        #[test]
        fn actual_diagnostic_process_exit_and_spawn_errors_are_observed() {
            let mut command = fact_command("streams");
            let output = bounded_at(&mut command, 5, Stage::RustcIdentity).unwrap();
            assert_eq!(output.status.code(), Some(7));
            let error = rustc_identity(&output, Host::X64).err().unwrap();
            assert_eq!(error.stage, Stage::RustcIdentity);
            assert_eq!(error.kind, Kind::Exit);
            let process = error.process.as_ref().unwrap();
            assert_eq!(process.exit_code, output.status.code());
            assert_eq!(process.exit_success, Some(false));
            assert_eq!(
                process.stdout.as_ref().unwrap().captured_bytes,
                Some(output.stdout.len())
            );
            assert_eq!(
                process.stderr.as_ref().unwrap().original_sha256,
                Some(hash(&output.stderr))
            );
            println!("ACTUAL-DIAGNOSTIC-NONZERO {error:?}");
            let home = tempfile::tempdir().unwrap();
            let mut absent = Command::new(home.path().join("owned-absent-tool"));
            let error = bounded_at(&mut absent, 5, Stage::VswhereDiscovery)
                .err()
                .unwrap();
            assert_eq!(error.stage, Stage::VswhereDiscovery);
            assert_eq!(error.kind, Kind::Spawn);
            assert!(error.raw_os_error.is_some());
            let process = error.process.as_ref().unwrap();
            assert!(!process.spawn_returned_child);
            assert!(process.exit_code.is_none() && process.exit_success.is_none());
            assert!(process.stdout.is_none() && process.stderr.is_none());
            println!("ACTUAL-DIAGNOSTIC-SPAWN {error:?}");
        }

        #[test]
        fn actual_diagnostic_overflow_and_timeout_do_not_fabricate_full_size_or_exit() {
            let mut command = fact_command("overflow");
            let error = bounded_at(&mut command, 5, Stage::VswhereDiscovery)
                .err()
                .unwrap();
            assert_eq!(error.kind, Kind::CaptureLimit);
            let stream = error.process.as_ref().unwrap().stdout.as_ref().unwrap();
            assert_eq!(stream.captured_bytes, Some(CAPTURE_BYTES + 1));
            assert!(stream.reached_capture_limit && stream.excerpt_cut);
            assert!(
                !stream.actual_eof
                    && stream.original_size.is_none()
                    && stream.original_sha256.is_none()
            );
            let excerpt = stream
                .excerpt_hex
                .as_ref()
                .unwrap()
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect::<Vec<_>>();
            // Libtest itself emits its original preamble before the helper body.
            // Keep it in the whole capture and hash; never trim/normalize it.
            let prefix_len = excerpt
                .iter()
                .rposition(|byte| *byte != b'x')
                .map_or(0, |at| at + 1);
            assert!(prefix_len < excerpt.len());
            assert!(excerpt[prefix_len..].iter().all(|byte| *byte == b'x'));
            let mut expected = excerpt[..prefix_len].to_vec();
            expected.resize(CAPTURE_BYTES + 1, b'x');
            assert_eq!(stream.captured_prefix_sha256, Some(hash(&expected)));
            println!("ACTUAL-DIAGNOSTIC-OVERFLOW {error:?}");
            let mut command = fact_command("sleep");
            let started = Instant::now();
            let error = bounded_at(&mut command, 3, Stage::VsdevcmdSetup)
                .err()
                .unwrap();
            assert_eq!(error.kind, Kind::Timeout);
            assert_eq!(error.stage, Stage::VsdevcmdSetup);
            let process = error.process.as_ref().unwrap();
            assert!(process.spawn_returned_child);
            assert!(process.exit_code.is_none() && process.exit_success.is_none());
            assert!(process.cleanup.as_ref().unwrap().try_wait_returned_status);
            assert!(started.elapsed() < Duration::from_secs(9));
            println!("ACTUAL-DIAGNOSTIC-TIMEOUT {error:?}");
        }

        #[test]
        fn actual_unknown_field_capture_withholds_contents_and_keeps_complete_hash() {
            let mut command = fact_command("fields");
            let output = bounded_at(&mut command, 5, Stage::VsdevcmdSetup).unwrap();
            assert!(output.status.success());
            let error = parse_utf16(&output.stdout, Host::X64)
                .err()
                .unwrap()
                .output(&output, false);
            assert_eq!(error.kind, Kind::UnknownField);
            let stdout = error.process.as_ref().unwrap().stdout.as_ref().unwrap();
            assert!(stdout.contents_withheld && stdout.excerpt_hex.is_none());
            assert_eq!(stdout.original_size, Some(output.stdout.len()));
            assert_eq!(stdout.original_sha256, Some(hash(&output.stdout)));
            let text = format!("{error:?}");
            assert!(!text.contains("synthetic-private-field-value"));
            let private: Vec<_> = "synthetic-private-field-value"
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .collect();
            assert!(!text.contains(&facts::hex(&private)));
            println!("ACTUAL-DIAGNOSTIC-WITHHELD {error:?}");
        }

        #[test]
        fn actual_setup_failure_capture_excludes_parent_synthetic_credentials() {
            let home = tempfile::tempdir().unwrap();
            let mut command = fact_command("credential-parent");
            let credential = "synthetic-credential-must-not-reach-compiler";
            command.env(SYNTHETIC_CREDENTIAL, credential);
            for name in ["HOME", "USERPROFILE", "TMPDIR", "TEMP", "TMP"] {
                command.env(name, home.path());
            }
            let output = bounded_at(&mut command, 10, Stage::Control).unwrap();
            assert!(output.status.success());
            let stdout = std::str::from_utf8(&output.stdout).unwrap();
            assert!(stdout.contains("MSVC-STAGE-FACTS:"));
            assert!(
                stdout.contains("actual")
                    || stdout.contains(&facts::hex(b"actual isolated credential absent"))
            );
            for bytes in [&output.stdout, &output.stderr] {
                assert!(
                    !bytes
                        .windows(credential.len())
                        .any(|part| part == credential.as_bytes())
                );
                assert!(
                    !bytes
                        .windows(facts::hex(credential.as_bytes()).len())
                        .any(|part| part == facts::hex(credential.as_bytes()).as_bytes())
                );
            }
        }

        #[test]
        fn native_paths_libraries_and_pe_failures_use_fixed_identifiers_and_actual_reads() {
            let home = tempfile::tempdir().unwrap();
            let error = physical_file(&home.path().join("owned-absent-vswhere"))
                .err()
                .unwrap()
                .at(Stage::VswhereFile);
            assert_eq!(error.stage, Stage::VswhereFile);
            assert_eq!(error.kind, Kind::Metadata);
            assert!(error.raw_os_error.is_some());
            assert_eq!(
                checked_path(OsString::from("relative-tool"))
                    .err()
                    .unwrap()
                    .kind,
                Kind::NativePath
            );
            assert_eq!(physical_directory(home.path()).unwrap(), home.path());
            let error = required_library("kernel32.lib", &[home.path().to_path_buf()])
                .err()
                .unwrap();
            assert_eq!(error.stage, Stage::RequiredLibrary);
            assert_eq!(error.expected, Some("kernel32.lib"));
            assert_eq!(error.library_observations.len(), 1);
            assert!(error.library_observations[0].raw_os_error.is_some());
            assert!(error.process.is_none());
            std::fs::write(home.path().join("kernel32.lib"), b"").unwrap();
            let error = required_library("kernel32.lib", &[home.path().to_path_buf()])
                .err()
                .unwrap();
            assert_eq!(error.library_observations[0].kind, Kind::EmptyFile);
            assert!(error.library_observations[0].raw_os_error.is_none());
            std::fs::write(home.path().join("kernel32.lib"), b"owned synthetic library").unwrap();
            required_library("kernel32.lib", &[home.path().to_path_buf()]).unwrap();
            let header = [0u8; 65];
            let error = pe_identity(&header, Host::X64).err().unwrap();
            assert_eq!(error.kind, Kind::Pe);
            assert_eq!(error.observed_read_bytes, Some(header.len()));
            assert_eq!(error.observed_read_prefix_sha256, Some(hash(&header)));
            assert!(error.observed_pe_machine.is_none());
        }

        #[test]
        fn helper_failure_frame_preserves_all_actual_bounded_binary_streams() {
            let mut command = fact_command("streams");
            let output = bounded_at(&mut command, 5, Stage::Control).unwrap();
            let record = facts::helper_failure_record(&output.stdout, &output.stderr);
            assert_eq!(record["stdout"]["original_size"], output.stdout.len());
            assert_eq!(record["stderr"]["original_sha256"], hash(&output.stderr));
            assert_eq!(record["stdout"]["excerpt_hex"], facts::hex(&output.stdout));
            assert_eq!(record["stderr"]["excerpt_hex"], facts::hex(&output.stderr));
            assert_eq!(record["stdout"]["excerpt_cut"], false);
            let encoded = record.to_string();
            assert!(encoded.is_ascii());
            assert!(!encoded.as_bytes().contains(&27));
            facts::helper_failure(&output.stdout, &output.stderr);
        }

        #[test]
        fn unmarked_unknown_duplicate_and_malformed_fact_helpers_return_before_environment() {
            for markers in [
                vec![],
                vec![OsString::from(format!("{FACT_MARKER}unknown"))],
                vec![
                    OsString::from(format!("{FACT_MARKER}credential-parent")),
                    OsString::from(format!("{FACT_MARKER}credential-parent")),
                ],
            ] {
                let mut command = Command::new(std::env::current_exe().unwrap());
                command
                    .args(["--ignored", "--exact", FACT_HELPER, "--nocapture"])
                    .args(markers)
                    .env_clear()
                    .env("HOME", "relative-poisoned-unselected-root");
                if let Some(root) = system_root() {
                    command.env("SystemRoot", root);
                }
                let output = bounded(command, 5).unwrap();
                assert!(output.status.success());
                assert!(
                    std::str::from_utf8(&output.stdout)
                        .unwrap()
                        .contains("MSVC-FACTS:NOT-SELECTED")
                );
            }
            let good = OsString::from(format!("{FACT_MARKER}credential-parent"));
            #[cfg(unix)]
            let malformed = {
                use std::os::unix::ffi::OsStringExt;
                let mut bytes = FACT_MARKER.as_bytes().to_vec();
                bytes.push(255);
                OsString::from_vec(bytes)
            };
            #[cfg(windows)]
            let malformed = {
                use std::os::windows::ffi::OsStringExt;
                let mut units: Vec<_> = FACT_MARKER.encode_utf16().collect();
                units.push(0xd800);
                OsString::from_wide(&units)
            };
            assert!(fact_mode([good.clone(), malformed.clone()]).is_none());
            assert!(fact_mode([malformed.clone(), good]).is_none());
            assert!(fact_mode([malformed]).is_none());
        }

        #[test]
        fn diagnostic_capture_retries_interrupted_reads_without_claiming_missing_bytes() {
            struct OnceInterrupted {
                first: bool,
                bytes: std::io::Cursor<Vec<u8>>,
            }
            impl Read for OnceInterrupted {
                fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                    if self.first {
                        self.first = false;
                        return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
                    }
                    std::io::Read::read(&mut self.bytes, output)
                }
            }
            let progress = Progress::default();
            let bytes = capture(
                OnceInterrupted {
                    first: true,
                    bytes: std::io::Cursor::new(b"actual synthetic reader bytes".to_vec()),
                },
                progress.clone(),
            )
            .unwrap();
            assert_eq!(bytes, b"actual synthetic reader bytes");
            let snapshot = snapshot(&progress).unwrap();
            assert_eq!(snapshot.original_sha256, Some(hash(&bytes)));
            assert!(snapshot.actual_eof && !snapshot.reached_capture_limit);
        }

        #[test]
        fn diagnostic_read_failures_preserve_only_actual_prefix_and_optional_os_error() {
            struct PrefixThenError {
                done: bool,
            }
            impl Read for PrefixThenError {
                fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                    if self.done {
                        return Err(std::io::Error::other("synthetic read failure"));
                    }
                    self.done = true;
                    output[..3].copy_from_slice(b"abc");
                    Ok(3)
                }
            }
            let progress = Progress::default();
            let error = capture(PrefixThenError { done: false }, progress.clone())
                .err()
                .unwrap();
            assert!(error.raw_os_error().is_none());
            let prefix = snapshot(&progress).unwrap();
            assert_eq!(prefix.captured_bytes, Some(3));
            assert_eq!(prefix.captured_prefix_sha256, Some(hash(b"abc")));
            assert!(prefix.original_size.is_none() && prefix.original_sha256.is_none());
            assert!(!prefix.actual_eof);
            let home = tempfile::tempdir().unwrap();
            let error = read_pe_header(&home.path().join("owned-absent-header"))
                .err()
                .unwrap();
            assert_eq!(error.stage, Stage::PeIdentity);
            assert_eq!(error.kind, Kind::Read);
            assert!(error.raw_os_error.is_some());
            assert!(error.observed_read_bytes.is_none());
            let header = home.path().join("owned-header");
            std::fs::write(&header, b"actual bounded header").unwrap();
            assert_eq!(read_pe_header(&header).unwrap(), b"actual bounded header");
        }

        #[test]
        fn all_fixed_diagnostic_identifiers_serialize_without_admitting_success() {
            let stages = [
                Stage::Host,
                Stage::SystemRoot,
                Stage::RustcIdentity,
                Stage::VswhereFile,
                Stage::VswhereDiscovery,
                Stage::Installation,
                Stage::VsdevcmdFile,
                Stage::CmdFile,
                Stage::VsdevcmdSetup,
                Stage::Utf16Fields,
                Stage::NativePath,
                Stage::File,
                Stage::Directory,
                Stage::VcDirectory,
                Stage::SdkDirectory,
                Stage::VcContainment,
                Stage::LinkerFile,
                Stage::PeIdentity,
                Stage::LibList,
                Stage::LibDirectory,
                Stage::RequiredLibrary,
                Stage::LibraryRoots,
                Stage::CompilerEnvironment,
                Stage::LinkerBanner,
                Stage::Control,
            ];
            let kinds = [
                Kind::Host,
                Kind::Environment,
                Kind::Spawn,
                Kind::Wait,
                Kind::Timeout,
                Kind::CaptureReader,
                Kind::CaptureRead,
                Kind::CaptureDrain,
                Kind::CaptureJoin,
                Kind::CaptureLimit,
                Kind::Exit,
                Kind::Empty,
                Kind::Bounds,
                Kind::Utf8,
                Kind::OddUtf16,
                Kind::Nul,
                Kind::Shape,
                Kind::UnknownField,
                Kind::MissingField,
                Kind::DuplicateField,
                Kind::FieldValue,
                Kind::HostMismatch,
                Kind::NativePath,
                Kind::Metadata,
                Kind::NotFile,
                Kind::Symlink,
                Kind::EmptyFile,
                Kind::NotDirectory,
                Kind::Containment,
                Kind::Read,
                Kind::Pe,
                Kind::Library,
                Kind::LibraryRoots,
                Kind::PathList,
                Kind::Banner,
            ];
            for stage in stages {
                for kind in kinds {
                    let error = Failure::new(stage, kind);
                    let encoded = serde_json::to_value(&error).unwrap();
                    assert_eq!(encoded["fixed"], FAILURE);
                    assert!(
                        encoded["process"].is_null() && encoded["observed_read_bytes"].is_null()
                    );
                }
            }
            let progress = Progress::default();
            let snapshot = snapshot(&progress).unwrap();
            assert!(snapshot.captured_bytes.is_none() && snapshot.original_size.is_none());
            assert!(
                snapshot.original_sha256.is_none() && snapshot.captured_prefix_sha256.is_none()
            );
            let mut command = fact_command("streams");
            let output = bounded_at(&mut command, 5, Stage::Control).unwrap();
            assert_eq!(
                installation_line(&output).err().unwrap().stage,
                Stage::VswhereDiscovery
            );
            assert_eq!(
                linker_banner(&output).err().unwrap().stage,
                Stage::LinkerBanner
            );
        }

        #[test]
        fn discovery_comparison_arguments_preserve_original_selector_and_do_not_select_prerelease()
        {
            for host in [Host::X64, Host::X86, Host::Arm64] {
                let original = original_discovery_arguments(host);
                assert_eq!(
                    original,
                    [
                        "-latest",
                        "-products",
                        "*",
                        "-requires",
                        host.component(),
                        "-property",
                        "installationPath",
                        "-utf8"
                    ]
                );
                let mut plus_prerelease = original.clone();
                plus_prerelease.push("-prerelease");
                assert_eq!(
                    DiscoveryQuery::SameFilterPrerelease.arguments(host),
                    plus_prerelease
                );
                for query in [
                    DiscoveryQuery::StableInstances,
                    DiscoveryQuery::PrereleaseInstances,
                    DiscoveryQuery::StableComponentInstances,
                    DiscoveryQuery::PrereleaseComponentInstances,
                ] {
                    let args = query.arguments(host);
                    assert_eq!(
                        args.contains(&"-prerelease"),
                        matches!(
                            query,
                            DiscoveryQuery::PrereleaseInstances
                                | DiscoveryQuery::PrereleaseComponentInstances
                        )
                    );
                    assert_eq!(
                        args.contains(&"-requires"),
                        matches!(
                            query,
                            DiscoveryQuery::StableComponentInstances
                                | DiscoveryQuery::PrereleaseComponentInstances
                        )
                    );
                    assert!(
                        args.contains(&"-all") && args.contains(&"json") && args.contains(&"-utf8")
                    );
                    assert!(
                        !args.contains(&"-property")
                            && !args
                                .iter()
                                .any(|arg| arg.contains("HOME") || arg.contains("LIB"))
                    );
                }
            }
            let summary = discovery_summary(DiscoveryQuery::SameFilterPrerelease, b"").unwrap();
            assert_eq!(summary.records, 0);
            assert!(summary.instances.is_empty());
        }

        #[test]
        fn instance_comparison_summaries_keep_flags_and_hash_private_strings_without_publication() {
            let private = "synthetic-private-display-credential";
            let bytes = json!([{
                "instanceId":"owned-instance", "installationPath": "C:\\VS 中文 & % !\\", "installationVersion":"18.12.3",
                "isPrerelease":true,"isComplete":true,"isLaunchable":false,"isRebootRequired":false,"state":5,
                "displayName":private,"description":private,"properties":{"custom":private},
                "catalog":{"productName":private}
            }]).to_string().into_bytes();
            let summary = discovery_summary(DiscoveryQuery::PrereleaseInstances, &bytes).unwrap();
            assert_eq!(summary.records, 1);
            let instance = &summary.instances[0];
            assert_eq!(instance.instance_id_sha256, Some(hash(b"owned-instance")));
            assert_eq!(
                instance.installation_path_sha256,
                Some(hash("C:\\VS 中文 & % !\\".as_bytes()))
            );
            assert_eq!(instance.is_prerelease, Some(true));
            assert_eq!(instance.is_launchable, Some(false));
            assert_eq!(instance.state, Some(5));
            let encoded = serde_json::to_string(&summary).unwrap();
            assert!(
                !encoded.contains(private)
                    && !encoded.contains("displayName")
                    && !encoded.contains("custom")
            );
            let missing = discovery_summary(DiscoveryQuery::StableInstances, b"[{}]").unwrap();
            assert!(
                missing.instances[0].is_complete.is_none()
                    && missing.instances[0].installation_path_sha256.is_none()
            );
        }

        #[test]
        fn instance_comparison_rejects_whole_ambiguous_malformed_and_overbound_json() {
            for bytes in [
                b"{malformed".as_slice(),
                b"{}",
                b"[false]",
                br#"[{"isPrerelease":true,"isPrerelease":false}]"#,
                br#"[{"isPrerelease":true,"\u0069sPrerelease":false}]"#,
                br#"[{"isComplete":"true"}]"#,
                br#"[{"installationVersion":"synthetic-secret"}]"#,
                br#"[{"state":-1}]"#,
                b"[] trailing",
            ] {
                assert!(discovery_summary(DiscoveryQuery::StableInstances, bytes).is_err());
            }
            assert!(
                discovery_summary(
                    DiscoveryQuery::StableInstances,
                    &vec![b' '; CAPTURE_BYTES + 1]
                )
                .is_err()
            );
            assert!(
                discovery_summary(
                    DiscoveryQuery::StableInstances,
                    &json!(vec![json!({}); 9]).to_string().into_bytes()
                )
                .is_err()
            );
            let deep = format!("{}0{}", "[".repeat(35), "]".repeat(35));
            assert!(comparison_json(deep.as_bytes()).is_err());
            let work = json!(vec![0; 4097]).to_string();
            assert!(comparison_json(work.as_bytes()).is_err());
            assert!(
                discovery_summary(
                    DiscoveryQuery::SameFilterPrerelease,
                    b"C:\\one\r\nC:\\two\r\n"
                )
                .is_err()
            );
            assert!(discovery_summary(DiscoveryQuery::SameFilterPrerelease, &[255]).is_err());
        }

        #[test]
        fn actual_comparison_failures_preserve_exit_and_cut_hashes_without_metadata_contents() {
            let mut command = fact_command("streams");
            let output = bounded_at(&mut command, 5, Stage::VswhereDiscovery).unwrap();
            let code = output.status.code();
            let stderr_hash = hash(&output.stderr);
            let comparison =
                comparison_result(DiscoveryQuery::PrereleaseInstances, Host::X64, Ok(output));
            assert!(comparison.summary.is_none());
            assert_eq!(comparison.failure.as_ref().unwrap().kind, Kind::Exit);
            let facts = comparison.output.as_ref().unwrap();
            assert_eq!(facts.exit_code, code);
            assert_eq!(
                facts.stderr.as_ref().unwrap().original_sha256,
                Some(stderr_hash)
            );
            assert!(facts.stdout.as_ref().unwrap().contents_withheld);
            let mut command = fact_command("overflow");
            let failed = bounded_at(&mut command, 5, Stage::VswhereDiscovery);
            let comparison =
                comparison_result(DiscoveryQuery::StableComponentInstances, Host::X64, failed);
            assert!(comparison.output.is_none() && comparison.summary.is_none());
            let stdout = comparison
                .failure
                .as_ref()
                .unwrap()
                .process
                .as_ref()
                .unwrap()
                .stdout
                .as_ref()
                .unwrap();
            assert_eq!(stdout.captured_bytes, Some(CAPTURE_BYTES + 1));
            assert!(
                stdout.original_sha256.is_none()
                    && stdout.contents_withheld
                    && stdout.excerpt_hex.is_none()
            );
        }

        #[test]
        fn actual_comparison_environment_remains_the_same_cleared_allowlist() {
            let home = tempfile::tempdir().unwrap();
            let selected =
                DiscoveryRoot::new("ProgramFiles(x86)", home.path().as_os_str().to_owned())
                    .unwrap();
            let system = system_root()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.path().to_path_buf());
            let secret = "synthetic-comparison-credential";
            let mut command = fact_command("env");
            command.env(SYNTHETIC_CREDENTIAL, secret);
            base_values(&mut command, &selected, &system, |_| {
                Some(home.path().as_os_str().to_owned())
            })
            .unwrap();
            let output = bounded_at(&mut command, 5, Stage::VswhereDiscovery).unwrap();
            assert_eq!(output.status.code(), Some(17));
            let comparison =
                comparison_result(DiscoveryQuery::SameFilterPrerelease, Host::X64, Ok(output));
            assert!(comparison.summary.is_none());
            let json = serde_json::to_string(&comparison).unwrap();
            assert!(!json.contains(secret) && !json.contains(&facts::hex(secret.as_bytes())));
            let fields: BTreeMap<_, _> = command
                .get_envs()
                .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
                .collect();
            assert!(!fields.contains_key(OsStr::new(SYNTHETIC_CREDENTIAL)));
            assert_eq!(
                fields.get(OsStr::new("HOME")),
                Some(&Some(home.path().as_os_str().to_owned()))
            );
        }

        #[test]
        fn setup_state_transport_keeps_complete_owned_root_and_refuses_ambiguous_or_invalid_data() {
            let home = tempfile::tempdir().unwrap();
            let root = SetupStateRoot::new(home.path().as_os_str().to_owned()).unwrap();
            let argument = root.argument();
            let recovered = transferred_setup_state([argument.clone()]).unwrap();
            assert_eq!(
                recovered.value.as_encoded_bytes(),
                root.value.as_encoded_bytes()
            );
            assert!(transferred_setup_state([]).is_err());
            assert_eq!(
                transferred_setup_state([argument.clone(), argument])
                    .err()
                    .unwrap()
                    .kind,
                Kind::DuplicateField
            );
            for data in [
                "",
                "0",
                "gg",
                "+0",
                "-0",
                "00",
                &"f".repeat(MAX_SETUP_STATE_HEX_BYTES + 1),
            ] {
                assert!(
                    transferred_setup_state([OsString::from(format!("{STATE_MARKER}{data}"))])
                        .is_err()
                );
            }
            for invalid in [
                OsString::from("relative"),
                OsString::from("bad\0root"),
                OsString::from("/bad\nroot"),
                home.path().join("bad\troot").into_os_string(),
                home.path().join("bad\x1broot").into_os_string(),
                home.path().join("bad\x7froot").into_os_string(),
            ] {
                assert!(SetupStateRoot::new(invalid).is_err());
            }
            #[cfg(windows)]
            {
                for invalid in [r"C:", r"\\?\C:", r"\root-relative"] {
                    assert!(SetupStateRoot::new(OsString::from(invalid)).is_err());
                }
                for complete in [
                    r"C:\",
                    r"\\?\C:\",
                    r"C:\ProgramData",
                    r"\\?\C:\ProgramData",
                    r"\\server\share",
                    r"\\?\UNC\server\share",
                ] {
                    let value = OsString::from(complete);
                    let state = SetupStateRoot::new(value.clone()).unwrap();
                    let returned = transferred_setup_state([state.argument()]).unwrap();
                    assert_eq!(returned.value.as_encoded_bytes(), value.as_encoded_bytes());
                }
            }
            let value = home
                .path()
                .join(malformed_setup_state_argument("owned-opaque-"))
                .into_os_string();
            assert!(value.to_str().is_none());
            let state = SetupStateRoot::new(value.clone()).unwrap();
            let returned = transferred_setup_state([state.argument()]).unwrap();
            assert_eq!(returned.value.as_encoded_bytes(), value.as_encoded_bytes());
            let malformed = malformed_setup_state_argument(STATE_MARKER);
            for arguments in [
                vec![root.argument(), malformed.clone()],
                vec![malformed.clone(), root.argument()],
                vec![malformed],
            ] {
                assert!(transferred_setup_state(arguments).is_err());
            }
            #[cfg(unix)]
            let full = OsString::from(format!(
                "/{}",
                "x".repeat(MAX_SETUP_STATE_HEX_BYTES / 2 - 1)
            ));
            #[cfg(windows)]
            let full = OsString::from(format!(
                "C:\\{}",
                "x".repeat(MAX_SETUP_STATE_HEX_BYTES / 4 - 3)
            ));
            let state = SetupStateRoot::new(full.clone()).unwrap();
            assert_eq!(
                state.argument().len(),
                STATE_MARKER.len() + MAX_SETUP_STATE_HEX_BYTES
            );
            assert_eq!(
                transferred_setup_state([state.argument()]).unwrap().value,
                full
            );
            let mut too_long = full;
            too_long.push("x");
            assert_eq!(
                SetupStateRoot::new(too_long).err().unwrap().kind,
                Kind::Bounds
            );
        }

        fn malformed_setup_state_argument(prefix: &str) -> OsString {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                let mut bytes = prefix.as_bytes().to_vec();
                bytes.push(0xff);
                OsString::from_vec(bytes)
            }
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStringExt;
                let mut units: Vec<_> = prefix.encode_utf16().collect();
                units.push(0xd800);
                OsString::from_wide(&units)
            }
        }

        #[test]
        fn only_marked_source_build_supervisors_request_machine_state_transport() {
            let mut unrelated = fact_command("streams");
            assert!(!needs_setup_state_comparison(&unrelated));
            transport_setup_state_comparison(&mut unrelated);
            assert!(!unrelated.get_args().any(|value| {
                value
                    .as_encoded_bytes()
                    .starts_with(STATE_MARKER.as_bytes())
            }));
            let mut source = Command::new("owned-unused-supervisor-program");
            source
                .args(["--ignored", "--exact", HELPER, "--nocapture"])
                .arg(format!("{MARKER}suite"));
            assert!(needs_setup_state_comparison(&source));
            source.arg(format!("{MARKER}suite"));
            assert!(!needs_setup_state_comparison(&source));
            let mut installed = Command::new("owned-unused-supervisor-program");
            installed
                .args(["--ignored", "--exact", NATIVE_HELPER, "--nocapture"])
                .arg(format!("{NATIVE_MARKER}guest"));
            assert!(needs_setup_state_comparison(&installed));
            for (helper, prefix, mode) in [
                (HELPER, MARKER, "suite"),
                (HELPER, MARKER, "collect"),
                (NATIVE_HELPER, NATIVE_MARKER, "guest"),
            ] {
                for markers in [
                    vec![],
                    vec![OsString::from(format!("{prefix}unknown"))],
                    vec![
                        OsString::from(format!("{prefix}{mode}")),
                        OsString::from(format!("{prefix}{mode}")),
                    ],
                    vec![
                        OsString::from(format!("{prefix}{mode}")),
                        malformed_setup_state_argument(prefix),
                    ],
                    vec![
                        malformed_setup_state_argument(prefix),
                        OsString::from(format!("{prefix}{mode}")),
                    ],
                    vec![malformed_setup_state_argument(prefix)],
                ] {
                    let mut command = Command::new("owned-unused-supervisor-program");
                    command
                        .args(["--ignored", "--exact", helper, "--nocapture"])
                        .args(markers);
                    assert!(!needs_setup_state_comparison(&command));
                    let mut requested = Vec::new();
                    capture_setup_state_comparison(&mut command, |name| {
                        requested.push(name.to_owned());
                        panic!("unselected supervisor inspected a setup field")
                    })
                    .unwrap();
                    assert!(requested.is_empty());
                }
            }
            let mut crossed = Command::new("owned-unused-supervisor-program");
            crossed
                .args([HELPER, NATIVE_HELPER])
                .arg(format!("{MARKER}suite"));
            assert!(!needs_setup_state_comparison(&crossed));
            let mut companion = Command::new("owned-unused-supervisor-program");
            companion
                .arg(HELPER)
                .arg(format!("{MARKER}suite"))
                .arg(format!("{NATIVE_MARKER}unknown"));
            assert!(!needs_setup_state_comparison(&companion));
            let mut duplicate_helper = Command::new("owned-unused-supervisor-program");
            duplicate_helper
                .args([HELPER, HELPER])
                .arg(format!("{MARKER}suite"));
            assert!(!needs_setup_state_comparison(&duplicate_helper));
            let mut collision = Command::new("owned-unused-supervisor-program");
            collision
                .arg(HELPER)
                .arg(format!("{MARKER}suite"))
                .arg(format!("{STATE_MARKER}2f"));
            assert!(!needs_setup_state_comparison(&collision));
            assert_eq!(
                capture_setup_state_comparison(&mut collision, |_| panic!(
                    "collided transport inspected environment"
                ))
                .err()
                .unwrap()
                .kind,
                Kind::DuplicateField
            );
        }

        #[test]
        fn selected_setup_state_capture_is_exact_and_refuses_missing_or_invalid_owned_inputs() {
            let home = tempfile::tempdir().unwrap();
            let mut command = Command::new("owned-unused-supervisor-program");
            command.arg(HELPER).arg(format!("{MARKER}suite"));
            let before: Vec<_> = command.get_args().map(OsStr::to_owned).collect();
            let mut requested = Vec::new();
            capture_setup_state_comparison(&mut command, |name| {
                requested.push(name.to_owned());
                Some(home.path().as_os_str().to_owned())
            })
            .unwrap();
            assert_eq!(requested, ["ProgramData"]);
            assert!(command.get_envs().next().is_none());
            let after: Vec<_> = command.get_args().map(OsStr::to_owned).collect();
            assert_eq!(&after[..before.len()], &before);
            assert_eq!(after.len(), before.len() + 1);
            assert_eq!(
                transferred_setup_state(after).unwrap().value,
                home.path().as_os_str()
            );
            assert!(
                capture_setup_state_comparison(&mut command, |_| panic!(
                    "duplicate capture inspected environment"
                ))
                .is_err()
            );
            for (value, expected) in [
                (None, Kind::Environment),
                (Some(OsString::new()), Kind::NativePath),
                (Some(OsString::from("relative")), Kind::NativePath),
                (
                    Some(home.path().join("bad\0root").into_os_string()),
                    Kind::NativePath,
                ),
                (
                    Some(home.path().join("bad\nroot").into_os_string()),
                    Kind::NativePath,
                ),
                (
                    Some(
                        home.path()
                            .join("x".repeat(MAX_SETUP_STATE_HEX_BYTES))
                            .into_os_string(),
                    ),
                    Kind::Bounds,
                ),
            ] {
                let mut command = Command::new("owned-unused-supervisor-program");
                command.arg(HELPER).arg(format!("{MARKER}suite"));
                let error = capture_setup_state_comparison(&mut command, |name| {
                    assert_eq!(name, "ProgramData");
                    value.clone()
                })
                .err()
                .unwrap();
                assert_eq!(error.stage, Stage::CompilerEnvironment);
                assert_eq!(error.kind, expected);
                assert_eq!(error.expected, Some("ProgramData"));
                assert!(error.process.is_none());
                assert_eq!(command.get_args().count(), 2);
                let text = format!("{error:?}");
                assert!(!text.contains("bad") && !text.contains(&facts::hex(b"bad")));
            }
        }

        #[test]
        fn machine_state_is_one_comparison_environment_field_and_never_a_selector_change() {
            let home = tempfile::tempdir().unwrap();
            let state = SetupStateRoot::new(home.path().as_os_str().to_owned()).unwrap();
            let selected =
                DiscoveryRoot::new("ProgramFiles(x86)", home.path().as_os_str().to_owned())
                    .unwrap();
            let system = system_root()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.path().to_path_buf());
            let mut command = fact_command("env");
            command.env(SYNTHETIC_CREDENTIAL, "synthetic-parent-credential");
            base_values(&mut command, &selected, &system, |_| {
                Some(home.path().as_os_str().to_owned())
            })
            .unwrap();
            let original: BTreeMap<_, _> = command
                .get_envs()
                .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
                .collect();
            assert!(!original.contains_key(OsStr::new("ProgramData")));
            setup_state_environment(&mut command, &state);
            let mut changed: BTreeMap<_, _> = command
                .get_envs()
                .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
                .collect();
            assert_eq!(
                changed.remove(OsStr::new("ProgramData")),
                Some(Some(state.value.clone()))
            );
            assert_eq!(changed, original);
            assert!(!changed.contains_key(OsStr::new(SYNTHETIC_CREDENTIAL)));
            assert_eq!(
                DiscoveryQuery::ProgramDataSameFilter.arguments(Host::X64),
                original_discovery_arguments(Host::X64)
            );
            let output = bounded_at(&mut command, 5, Stage::VswhereDiscovery).unwrap();
            assert_eq!(output.status.code(), Some(17));
            assert!(
                !output
                    .stdout
                    .windows(b"synthetic-parent-credential".len())
                    .any(|bytes| bytes == b"synthetic-parent-credential")
            );
        }

        #[test]
        fn machine_state_comparison_packet_fits_the_unchanged_whole_supervisor_capture() {
            let successful = bounded_at(&mut fact_command("fields"), 5, Stage::Control)
                .unwrap()
                .status;
            let refused = bounded_at(&mut fact_command("streams"), 5, Stage::Control)
                .unwrap()
                .status;
            assert!(successful.success() && !refused.success());
            let raw = vec![b'x'; CAPTURE_BYTES];
            let objects: Vec<_> = (0..8)
                .map(|ordinal| {
                    json!({
                        "instanceId": "i".repeat(128),
                        // Six five-digit path byte counts fit complete64KiB
                        // JSON; seven cannot. Keep all eight admitted records.
                        "installationPath": format!("C:\\{}", "v".repeat(if ordinal < 6 { 10000 } else { 300 })),
                        "installationVersion": "9".repeat(64),
                        "isPrerelease": false, "isComplete": false,
                        "isLaunchable": false, "isRebootRequired": false,
                        "state": u64::MAX,
                    })
                })
                .collect();
            let json_bytes = serde_json::to_vec(&objects).unwrap();
            assert!(json_bytes.len() < CAPTURE_BYTES);
            let path_bytes = format!("C:\\{}", "v".repeat(MAX_PATH_UNITS - 3)).into_bytes();
            for host in [Host::X64, Host::X86, Host::Arm64] {
                for query_failed in [false, true] {
                    let mut queries = Vec::new();
                    for query in [
                        DiscoveryQuery::SameFilterPrerelease,
                        DiscoveryQuery::StableInstances,
                        DiscoveryQuery::PrereleaseInstances,
                        DiscoveryQuery::StableComponentInstances,
                        DiscoveryQuery::PrereleaseComponentInstances,
                    ] {
                        let stdout = if query == DiscoveryQuery::SameFilterPrerelease {
                            path_bytes.clone()
                        } else {
                            json_bytes.clone()
                        };
                        queries.push(comparison_result(
                            query,
                            host,
                            Ok(std::process::Output {
                                status: successful,
                                stdout,
                                stderr: raw.clone(),
                            }),
                        ));
                    }
                    let added_output = if query_failed {
                        let mut error = Failure::new(Stage::VswhereDiscovery, Kind::CaptureDrain);
                        error.raw_os_error = Some(i32::MIN);
                        error.process = Some(facts::ProcessFacts {
                            spawn_returned_child: true,
                            exit_code: Some(i32::MIN),
                            exit_success: Some(false),
                            stdout: Some(facts::stream(Some(&raw), true, false, true, 2048)),
                            stderr: Some(facts::stream(Some(&raw), true, false, true, 2048)),
                            cleanup: Some(facts::CleanupFacts {
                                kill_returned_ok: false,
                                kill_raw_os_error: Some(i32::MIN),
                                try_wait_returned_status: false,
                                try_wait_raw_os_error: Some(i32::MIN),
                                exit_code_after_cleanup: Some(i32::MIN),
                                bound_reached_without_status: false,
                            }),
                        });
                        Err(error)
                    } else {
                        Ok(std::process::Output {
                            status: successful,
                            stdout: path_bytes.clone(),
                            stderr: raw.clone(),
                        })
                    };
                    let added_query = comparison_result(
                        DiscoveryQuery::ProgramDataSameFilter,
                        host,
                        added_output,
                    );
                    if query_failed {
                        assert!(added_query.failure.is_some() && added_query.summary.is_none());
                    } else {
                        assert_eq!(added_query.summary.as_ref().unwrap().records, 1);
                    }
                    // Only installation_line's refusal receives comparisons. Its
                    // original output/exit facts can be maximal, but PE/library/read
                    // contexts and recursive comparisons are unreachable here.
                    let mut original = Failure::new(Stage::VswhereDiscovery, Kind::Exit).output(
                        &std::process::Output {
                            status: refused,
                            stdout: raw.clone(),
                            stderr: raw.clone(),
                        },
                        true,
                    );
                    // Synthetic numeric extreme bounds the Windows exit field;
                    // this constructed record is not a native observation.
                    original.process.as_mut().unwrap().exit_code = Some(i32::MIN);
                    original.discovery_comparisons = Some(DiscoveryComparisons {
                        program_encoded_bytes: MAX_PATH_UNITS,
                        program_encoded_sha256: hash(&raw),
                        program_encoded_hex: facts::hex(&raw[..2048]),
                        program_encoding: "same-native-OsStr-encoded-bytes-as-original-query",
                        program_excerpt_cut: true,
                        comparison_only_not_source_or_compiler_selection: true,
                        same_cleared_preparation_environment: true,
                        queries,
                        program_data_same_query: Some(SetupStateComparison {
                            source: "registered-native-supervisor-ProgramData",
                            only_added_environment_field: "ProgramData",
                            original_program_arguments_and_base_unchanged: true,
                            native_encoding: "UTF-16LE-native-Windows-units",
                            native_bytes: Some(MAX_SETUP_STATE_HEX_BYTES / 2),
                            native_sha256: Some(hash(&raw)),
                            physical_directory_accepted: Some(false),
                            query: added_query,
                        }),
                    });
                    let encoded = format!("{original:?}");
                    assert!(encoded.is_ascii());
                    assert!(
                        encoded.len() + 4096 < CAPTURE_BYTES,
                        "combined failure and original framing exceed capture: {}",
                        encoded.len()
                    );
                    println!(
                        "SYNTHETIC-MACHINE-STATE-FAILURE-BOUND host={host:?} query_failed={query_failed} bytes={} sha256={} framing=4096 cap={CAPTURE_BYTES}",
                        encoded.len(),
                        hash(encoded.as_bytes())
                    );
                }
            }
        }

        #[test]
        fn strict_discovery_adds_only_the_validated_private_machine_state_field() {
            let home = tempfile::tempdir().unwrap();
            let state = SetupStateRoot::new(home.path().as_os_str().to_owned()).unwrap();
            let selected =
                DiscoveryRoot::new("ProgramFiles(x86)", home.path().as_os_str().to_owned())
                    .unwrap();
            let system = system_root()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.path().to_path_buf());
            let mut command = fact_command("env");
            command.env(SYNTHETIC_CREDENTIAL, "synthetic-strict-query-credential");
            base_values(&mut command, &selected, &system, |_| {
                Some(home.path().as_os_str().to_owned())
            })
            .unwrap();
            let original_arguments: Vec<_> = command.get_args().map(OsStr::to_owned).collect();
            let original_environment: BTreeMap<_, _> = command
                .get_envs()
                .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
                .collect();
            assert!(!original_environment.contains_key(OsStr::new("ProgramData")));
            let checked = strict_discovery_state([state.argument()]).unwrap();
            setup_state_environment(&mut command, &checked);
            let mut changed: BTreeMap<_, _> = command
                .get_envs()
                .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
                .collect();
            assert_eq!(
                changed.remove(OsStr::new("ProgramData")),
                Some(Some(state.value.clone()))
            );
            assert_eq!(changed, original_environment);
            assert_eq!(
                command.get_args().map(OsStr::to_owned).collect::<Vec<_>>(),
                original_arguments
            );
            assert!(!changed.contains_key(OsStr::new(SYNTHETIC_CREDENTIAL)));
            assert!(!command.get_args().any(|value| {
                value
                    .as_encoded_bytes()
                    .starts_with(STATE_MARKER.as_bytes())
            }));
            let output = bounded_at(&mut command, 5, Stage::VswhereDiscovery).unwrap();
            assert_eq!(output.status.code(), Some(17));
            assert!(
                !output
                    .stdout
                    .windows(b"synthetic-strict-query-credential".len())
                    .any(|bytes| bytes == b"synthetic-strict-query-credential")
            );
            for host in [Host::X64, Host::X86, Host::Arm64] {
                assert_eq!(
                    DiscoveryQuery::ProgramDataSameFilter.arguments(host),
                    original_discovery_arguments(host)
                );
            }
        }

        #[test]
        fn strict_discovery_refuses_missing_ambiguous_invalid_or_nonphysical_transfers_before_start()
         {
            let home = tempfile::tempdir().unwrap();
            let state = SetupStateRoot::new(home.path().as_os_str().to_owned()).unwrap();
            let missing =
                SetupStateRoot::new(home.path().join("owned-missing-state").into_os_string())
                    .unwrap();
            let mut malformed = malformed_setup_state_argument(STATE_MARKER);
            assert!(malformed.to_str().is_none());
            for arguments in [
                vec![],
                vec![state.argument(), state.argument()],
                vec![malformed.clone(), state.argument()],
                vec![state.argument(), malformed.clone()],
                vec![OsString::from(format!("{STATE_MARKER}00"))],
                vec![missing.argument()],
            ] {
                let mut command = Command::new(home.path().join("owned-absent-query-program"));
                command.env_clear().arg("unchanged-query-argument");
                let before: Vec<_> = command.get_args().map(OsStr::to_owned).collect();
                let error = strict_discovery_state(arguments).err().unwrap();
                assert_eq!(error.stage, Stage::CompilerEnvironment);
                assert_eq!(error.expected, Some("ProgramData"));
                assert!(error.process.is_none());
                assert!(command.get_envs().next().is_none());
                assert_eq!(
                    command.get_args().map(OsStr::to_owned).collect::<Vec<_>>(),
                    before
                );
            }
            malformed.push("ignored-native-tail");
            assert!(transferred_setup_state([malformed]).is_err());
            let file = home.path().join("owned-state-is-a-file");
            std::fs::write(&file, b"owned non-directory state control").unwrap();
            let state = SetupStateRoot::new(file.into_os_string()).unwrap();
            let command = Command::new("owned-unused-query-program");
            let error = strict_discovery_state([state.argument()]).err().unwrap();
            assert_eq!(error.kind, Kind::NotDirectory);
            assert!(error.process.is_none() && command.get_envs().next().is_none());
        }
    }
}
