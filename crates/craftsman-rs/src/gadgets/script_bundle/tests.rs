//! Owned synthetic source/artifact DATA only. No package or script execution.

use super::*;
use base64::Engine as _;
use serde_json::{Value, json};

struct Fixture {
    runtime: RuntimeProfile,
    entry: (Vec<u8>, Vec<u8>),
    modules: Vec<(Vec<u8>, Vec<u8>)>,
    assets: Vec<(Vec<u8>, Vec<u8>)>,
    artifacts: Vec<(Vec<u8>, Vec<u8>)>,
    declaration: Value,
}
fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn runtime_record(runtime: RuntimeProfile) -> Value {
    json!({
        "profile":runtime.name(), "engine":runtime.engine(),
        "image_sha256":runtime.image_sha256(), "image_bytes":runtime.image_len(),
        "loader_sha256":runtime.loader_sha256(),
        "transform": runtime.transform_sha256().map(|digest| json!({
            "name":"sucrase", "version":"3.35.1", "sha256":digest,
        })),
    })
}
fn local_source(id: &str) -> Value {
    json!({"id":id,"origin":"owned synthetic source data","license":"fixture notice\r\n",
        "distribution":{"kind":"local"}})
}
fn module_edge(from: &str, to: &str) -> Value {
    json!({"from":from,"target":{"kind":"module","name":to}})
}
fn asset_edge(from: &str, to: &str) -> Value {
    json!({"from":from,"target":{"kind":"asset","name":to}})
}
fn runtime_edge(from: &str, to: &str) -> Value {
    json!({"from":from,"target":{"kind":"runtime_module","name":to}})
}
fn supplied_files(source: &[(Vec<u8>, Vec<u8>)]) -> Vec<SuppliedFile<'_>> {
    source
        .iter()
        .map(|(name, bytes)| SuppliedFile::new(name, bytes))
        .collect()
}
impl Fixture {
    fn new(runtime: RuntimeProfile) -> Self {
        let name = match runtime {
            RuntimeProfile::JavaScript => "entry.js",
            RuntimeProfile::TypeScript => "entry.ts",
            RuntimeProfile::Cpython => "main.py",
        };
        let mut fixture = Self {
            runtime,
            entry: (name.as_bytes().to_vec(), Vec::new()),
            modules: Vec::new(),
            assets: Vec::new(),
            artifacts: Vec::new(),
            declaration: json!({"version":1,"runtime":runtime_record(runtime),"files":[],
                "sources":[local_source("script")],"direct_packages":[],"dependencies":[]}),
        };
        fixture.sync_files();
        fixture
    }
    fn entry_name(&self) -> &str {
        std::str::from_utf8(&self.entry.0).unwrap()
    }
    fn sync_files(&mut self) {
        self.declaration["files"] = Value::Array(
            std::iter::once(("entry", &self.entry))
                .chain(self.modules.iter().map(|f| ("module", f)))
                .chain(self.assets.iter().map(|f| ("asset", f)))
                .chain(self.artifacts.iter().map(|f| ("artifact", f)))
                .map(|(kind, (name, bytes))| {
                    json!({"kind":kind,
                    "name":String::from_utf8_lossy(name),"sha256":hex(bytes),"source":"script"})
                })
                .collect(),
        );
    }
    fn add_module(&mut self, name: &str, bytes: &[u8]) {
        self.modules
            .push((name.as_bytes().to_vec(), bytes.to_vec()));
        self.sync_files();
        let from = self.entry_name().to_owned();
        self.declaration["dependencies"]
            .as_array_mut()
            .unwrap()
            .push(module_edge(&from, name));
    }
    fn add_asset(&mut self, name: &str, bytes: &[u8]) {
        self.assets.push((name.as_bytes().to_vec(), bytes.to_vec()));
        self.sync_files();
        let from = self.entry_name().to_owned();
        self.declaration["dependencies"]
            .as_array_mut()
            .unwrap()
            .push(asset_edge(&from, name));
    }
    fn metadata(&self) -> Vec<u8> {
        serde_json::to_vec(&self.declaration).unwrap()
    }
    fn admit(&self) -> Result<CheckedBundle> {
        self.admit_with(&self.metadata(), None)
    }
    fn admit_with(&self, metadata: &[u8], image: Option<&[u8]>) -> Result<CheckedBundle> {
        let modules = supplied_files(&self.modules);
        let assets = supplied_files(&self.assets);
        let artifacts = supplied_files(&self.artifacts);
        CandidateBundle::new(
            SuppliedFile::new(&self.entry.0, &self.entry.1),
            &modules,
            &assets,
            &artifacts,
            metadata,
            image,
        )
        .admit()
    }
    fn data_encoding(&self) -> Vec<u8> {
        let modules: Vec<_> = self
            .modules
            .iter()
            .map(|(n, b)| SuppliedFile::new(n, b))
            .collect();
        let assets: Vec<_> = self
            .assets
            .iter()
            .map(|(n, b)| SuppliedFile::new(n, b))
            .collect();
        let artifacts: Vec<_> = self
            .artifacts
            .iter()
            .map(|(n, b)| SuppliedFile::new(n, b))
            .collect();
        let metadata = self.metadata();
        let candidate = CandidateBundle::new(
            SuppliedFile::new(&self.entry.0, &self.entry.1),
            &modules,
            &assets,
            &artifacts,
            &metadata,
            None,
        );
        let mut encoded = Vec::new();
        encoding::encode_data(&candidate, self.runtime, &mut encoded).unwrap();
        encoded
    }
    fn guest_encoding(&self) -> Value {
        let modules: Vec<_> = self
            .modules
            .iter()
            .map(|(n, b)| SuppliedFile::new(n, b))
            .collect();
        let metadata = self.metadata();
        let candidate = CandidateBundle::new(
            SuppliedFile::new(&self.entry.0, &self.entry.1),
            &modules,
            &[],
            &[],
            &metadata,
            None,
        );
        let mut encoded = Vec::new();
        encoding::encode_guest_code(&candidate, self.runtime, &mut encoded).unwrap();
        serde_json::from_slice(&encoded).unwrap()
    }
}
fn refused(fixture: &Fixture, expected: AdmissionError) {
    match fixture.admit() {
        Ok(_) => panic!("unexpected admitted synthetic data"),
        Err(actual) => assert_eq!(actual, expected),
    }
}
fn package_fixture(runtime: RuntimeProfile, count: usize) -> Fixture {
    let mut f = Fixture::new(runtime);
    let mut sources = vec![local_source("script")];
    let mut pins = Vec::new();
    for index in 0..count {
        let module = if runtime == RuntimeProfile::Cpython {
            format!("bundle_pkg{index}")
        } else {
            format!("pkg{index}.mjs")
        };
        let name = format!("pkg{index}");
        let id = format!("pkg_{index}");
        let artifact = format!("pkg{index}-1.0.0.fixture");
        f.add_module(&module, b"# owned metadata stand-in, never executed");
        f.artifacts.push((artifact.as_bytes().to_vec(), Vec::new()));
        sources.push(json!({"id":id,"origin":"owned synthetic package metadata",
            "license":"synthetic notice","distribution":{"kind":"package", "name":name,
            "version":"1.0.0","artifact":artifact,"requires":[]}}));
        pins.push(json!({"name":name,"version":"1.0.0"}));
    }
    f.sync_files();
    for (index, file) in f.declaration["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        if index != 0 {
            let owner = if index <= count {
                index - 1
            } else {
                index - count - 1
            };
            file["source"] = json!(format!("pkg_{owner}"));
        }
    }
    f.declaration["sources"] = json!(sources);
    f.declaration["direct_packages"] = json!(pins);
    f
}
fn defu_fixture() -> Fixture {
    let mut f = package_fixture(RuntimeProfile::JavaScript, 1);
    f.modules[0].0 = b"defu.mjs".to_vec();
    f.artifacts[0] = (
        b"defu-6.1.7.fixture".to_vec(),
        b"owned synthetic defu artifact DATA".to_vec(),
    );
    f.sync_files();
    f.declaration["files"][1]["source"] = json!("pkg_0");
    f.declaration["files"][2]["source"] = json!("pkg_0");
    f.declaration["sources"][1]["distribution"]["name"] = json!("defu");
    f.declaration["sources"][1]["distribution"]["version"] = json!("6.1.7");
    f.declaration["sources"][1]["distribution"]["artifact"] = json!("defu-6.1.7.fixture");
    f.declaration["direct_packages"] = json!([{"name":"defu","version":"6.1.7"}]);
    f.declaration["dependencies"] = json!([module_edge("entry.js", "defu.mjs")]);
    f
}
fn six_struct_fixture() -> Fixture {
    let mut f = package_fixture(RuntimeProfile::Cpython, 1);
    f.modules[0].0 = b"six".to_vec();
    f.modules.push((
        b"struct".to_vec(),
        b"# owned synthetic struct wrapper metadata".to_vec(),
    ));
    f.artifacts[0] = (
        b"six-1.17.0.fixture".to_vec(),
        b"owned synthetic six artifact DATA".to_vec(),
    );
    f.sync_files();
    f.declaration["files"][1]["source"] = json!("pkg_0");
    f.declaration["files"][2]["source"] = json!("stdlib_struct");
    f.declaration["files"][3]["source"] = json!("pkg_0");
    let distribution = &mut f.declaration["sources"][1]["distribution"];
    distribution["name"] = json!("six");
    distribution["version"] = json!("1.17.0");
    distribution["artifact"] = json!("six-1.17.0.fixture");
    f.declaration["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"stdlib_struct",
        "origin":"owned synthetic cpython wrapper metadata", "license":"fixture notice",
        "distribution":{"kind":"stdlib","name":"struct","version":"3.14.0"}}));
    f.declaration["direct_packages"] = json!([{"name":"six","version":"1.17.0"}]);
    f.declaration["dependencies"] = json!([
        module_edge("main.py", "six"),
        module_edge("six", "struct"),
        runtime_edge("struct", "_struct")
    ]);
    f
}

#[test]
fn all_three_fixed_profiles_admit_empty_owned_data_without_authority() {
    for runtime in [
        RuntimeProfile::JavaScript,
        RuntimeProfile::TypeScript,
        RuntimeProfile::Cpython,
    ] {
        let f = Fixture::new(runtime);
        let checked = f.admit().unwrap();
        assert_eq!(checked.runtime(), runtime);
        assert_eq!(checked.entry().name(), f.entry_name());
        assert!(checked.entry().bytes().is_empty());
        assert_eq!(
            checked.entry().sha256(),
            &parse_digest("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .unwrap()
        );
        assert!(
            checked.modules().is_empty()
                && checked.assets().is_empty()
                && checked.artifacts().is_empty()
        );
        assert!(checked.image_bytes().is_none());
        assert_eq!(checked.metadata_bytes(), f.metadata());
        assert_eq!(checked.encoded_bytes(), f.data_encoding().len());
        assert_eq!(
            checked.guest_code_bytes(),
            serde_json::to_vec(&f.guest_encoding()).unwrap().len()
        );
    }
}
#[test]
fn deterministic_binding_preserves_exact_source_bom_crlf_unicode_and_binary() {
    for runtime in [
        RuntimeProfile::JavaScript,
        RuntimeProfile::TypeScript,
        RuntimeProfile::Cpython,
    ] {
        let mut f = Fixture::new(runtime);
        f.entry.1 = "\u{feff}// owned source\r\n🛠\r\n".as_bytes().to_vec();
        f.add_asset("data.bin", &[0, 255, 13, 10, 0]);
        f.sync_files();
        let first = f.admit().unwrap();
        let second = f.admit().unwrap();
        assert_eq!(first.content_digest(), second.content_digest());
        assert_eq!(first.entry().bytes(), f.entry.1);
        assert_eq!(first.assets()[0].bytes(), [0, 255, 13, 10, 0]);
        assert_eq!(first.assets()[0].sha256(), &sha256(&[0, 255, 13, 10, 0]));
        let encoding: Value = serde_json::from_slice(&f.data_encoding()).unwrap();
        assert_eq!(
            encoding["entry"]["source"].as_str().unwrap().as_bytes(),
            f.entry.1
        );
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoding["assets"][0]["base64"].as_str().unwrap())
                .unwrap(),
            [0, 255, 13, 10, 0]
        );
    }
}
#[test]
fn original_unicode_module_spelling_is_retained_without_alias_repairs() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module("cafe\u{301}/資料.mjs", b"// owned data\r\n");
    let checked = f.admit().unwrap();
    assert_eq!(checked.modules()[0].name(), "cafe\u{301}/資料.mjs");
    assert_eq!(checked.modules()[0].bytes(), b"// owned data\r\n");
}
#[test]
fn immutable_snapshot_survives_changes_to_every_original_input() {
    let mut f = defu_fixture();
    f.add_asset("data.bin", b"old");
    // add_asset refreshes declarations, so restore exact package ownership.
    f.declaration["files"][1]["source"] = json!("pkg_0");
    f.declaration["files"][3]["source"] = json!("pkg_0");
    let original_metadata = f.metadata();
    let checked = f.admit().unwrap();
    let digest = *checked.content_digest();
    f.entry.0.fill(b'x');
    f.entry.1.push(1);
    f.modules[0].1.push(2);
    f.modules[0].0.fill(b'y');
    f.assets[0].1.fill(3);
    f.artifacts[0].1.fill(4);
    f.declaration["sources"][0]["origin"] = json!("changed");
    assert_eq!(checked.entry().name(), "entry.js");
    assert!(checked.entry().bytes().is_empty());
    assert_eq!(checked.modules()[0].name(), "defu.mjs");
    assert_eq!(checked.assets()[0].bytes(), b"old");
    assert_eq!(
        checked.artifacts()[0].bytes(),
        b"owned synthetic defu artifact DATA"
    );
    assert_eq!(checked.metadata_bytes(), original_metadata);
    assert_eq!(checked.content_digest(), &digest);
}
#[test]
fn zero_byte_modules_assets_and_artifacts_remain_present_records() {
    let mut f = package_fixture(RuntimeProfile::JavaScript, 1);
    f.modules[0].1.clear();
    f.sync_files();
    f.add_asset("empty.dat", b"");
    f.declaration["files"][1]["source"] = json!("pkg_0");
    f.declaration["files"][3]["source"] = json!("pkg_0");
    let checked = f.admit().unwrap();
    assert_eq!(checked.modules().len(), 1);
    assert!(checked.modules()[0].bytes().is_empty());
    assert_eq!(checked.assets().len(), 1);
    assert!(checked.assets()[0].bytes().is_empty());
    assert_eq!(checked.artifacts().len(), 1);
    assert!(checked.artifacts()[0].bytes().is_empty());
    let before = *checked.content_digest();
    f.assets.clear();
    f.sync_files();
    f.declaration["files"][1]["source"] = json!("pkg_0");
    f.declaration["files"][2]["source"] = json!("pkg_0");
    f.declaration["dependencies"].as_array_mut().unwrap().pop();
    assert_ne!(f.admit().unwrap().content_digest(), &before);
}

#[test]
fn entry_and_runtime_choices_bind_even_when_every_source_byte_is_identical() {
    let js = Fixture::new(RuntimeProfile::JavaScript);
    let mut ts = Fixture::new(RuntimeProfile::TypeScript);
    ts.entry.0 = js.entry.0.clone();
    ts.sync_files();
    let py = Fixture::new(RuntimeProfile::Cpython);
    assert_ne!(
        js.admit().unwrap().content_digest(),
        ts.admit().unwrap().content_digest()
    );
    assert_ne!(
        js.admit().unwrap().content_digest(),
        py.admit().unwrap().content_digest()
    );
    let mut renamed = Fixture::new(RuntimeProfile::JavaScript);
    renamed.entry.0 = b"other.js".to_vec();
    renamed.sync_files();
    assert_ne!(
        js.admit().unwrap().content_digest(),
        renamed.admit().unwrap().content_digest()
    );
}
#[test]
fn actual_source_asset_and_artifact_changes_refuse_old_declared_digests() {
    for category in 0..3 {
        let mut f = defu_fixture();
        f.add_asset("data.bin", b"old");
        f.declaration["files"][1]["source"] = json!("pkg_0");
        f.declaration["files"][3]["source"] = json!("pkg_0");
        let old = *f.admit().unwrap().content_digest();
        match category {
            0 => f.modules[0].1.push(1),
            1 => f.assets[0].1.push(1),
            _ => f.artifacts[0].1.push(1),
        }
        refused(&f, AdmissionError::Digest);
        f.sync_files();
        f.declaration["files"][1]["source"] = json!("pkg_0");
        f.declaration["files"][3]["source"] = json!("pkg_0");
        assert_ne!(f.admit().unwrap().content_digest(), &old);
    }
}
#[test]
fn declarations_bind_provenance_not_a_claimed_success_or_digest() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    let before = *f.admit().unwrap().content_digest();
    f.declaration["sources"][0]["origin"] = json!("caller approved label, DATA only");
    assert_ne!(f.admit().unwrap().content_digest(), &before);
    for field in [
        "approved",
        "granted",
        "ready",
        "success",
        "hash",
        "bundle_sha256",
        "consent",
        "root",
        "snapshot",
        "task",
    ] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        f.declaration[field] = json!(true);
        refused(&f, AdmissionError::Metadata);
    }
    for section in ["runtime", "files", "sources"] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        if section == "runtime" {
            f.declaration[section]["approved"] = json!(true);
        } else {
            f.declaration[section][0]["approved"] = json!(true);
        }
        refused(&f, AdmissionError::Metadata);
    }
}
#[test]
fn dependency_target_and_original_order_are_part_of_binding() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module("a.js", b"");
    f.add_module("b.js", b"");
    let before = *f.admit().unwrap().content_digest();
    f.modules.swap(0, 1);
    f.sync_files();
    assert_ne!(f.admit().unwrap().content_digest(), &before);
    let reordered = *f.admit().unwrap().content_digest();
    f.declaration["dependencies"] =
        json!([module_edge("entry.js", "a.js"), module_edge("a.js", "b.js")]);
    assert_ne!(f.admit().unwrap().content_digest(), &reordered);
    let graph_changed = *f.admit().unwrap().content_digest();
    let mut bytes = f.metadata();
    bytes.extend_from_slice(b" \r\n");
    assert_ne!(
        f.admit_with(&bytes, None).unwrap().content_digest(),
        &graph_changed
    );
}

#[test]
fn code_module_limit_includes_entry_and_never_accepts_a_partial_closure() {
    for runtime in [
        RuntimeProfile::JavaScript,
        RuntimeProfile::TypeScript,
        RuntimeProfile::Cpython,
    ] {
        let mut f = Fixture::new(runtime);
        for index in 0..CODE_FILES - 1 {
            let name = if runtime == RuntimeProfile::Cpython {
                format!("bundle_m{index}")
            } else {
                format!("m{index}.js")
            };
            f.add_module(&name, b"");
        }
        assert_eq!(f.admit().unwrap().modules().len(), CODE_FILES - 1);
        f.add_module(
            if runtime == RuntimeProfile::Cpython {
                "bundle_over"
            } else {
                "over.js"
            },
            b"",
        );
        refused(&f, AdmissionError::Bound);
    }
}
#[test]
fn source_limit_matches_distinct_actual_python_and_js_accounting() {
    let mut py = Fixture::new(RuntimeProfile::Cpython);
    py.entry.1 = vec![b' '; CODE_BYTES];
    py.sync_files();
    assert_eq!(py.admit().unwrap().entry().bytes().len(), CODE_BYTES);
    py.entry.1.push(b' ');
    py.sync_files();
    refused(&py, AdmissionError::Bound);
    let mut js = Fixture::new(RuntimeProfile::JavaScript);
    js.entry.1 = vec![b' '; CODE_BYTES - js.entry.0.len()];
    js.sync_files();
    assert_eq!(
        js.admit().unwrap().entry().bytes().len() + js.entry.0.len(),
        CODE_BYTES
    );
    js.entry.1.push(b' ');
    js.sync_files();
    refused(&js, AdmissionError::Bound);
}
#[test]
fn utf8_aggregate_code_is_measured_in_bytes_not_scalars() {
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    f.entry.1 = "🛠".repeat(CODE_BYTES / 4).into_bytes();
    f.sync_files();
    assert_eq!(f.admit().unwrap().entry().bytes().len(), CODE_BYTES);
    f.entry.1.extend_from_slice("🛠".as_bytes());
    f.sync_files();
    refused(&f, AdmissionError::Bound);
}
#[test]
fn asset_count_per_file_and_aggregate_limits_keep_every_zero_record() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    for index in 0..ASSETS {
        f.add_asset(&format!("d{index}.bin"), b"");
    }
    assert_eq!(f.admit().unwrap().assets().len(), ASSETS);
    f.add_asset("over.bin", b"");
    refused(&f, AdmissionError::Bound);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    for index in 0..4 {
        f.add_asset(&format!("d{index}.bin"), &vec![255; ASSET_FILE_BYTES]);
    }
    assert_eq!(
        f.admit()
            .unwrap()
            .assets()
            .iter()
            .map(|a| a.bytes().len())
            .sum::<usize>(),
        ASSET_BYTES
    );
    f.add_asset("over.bin", b"x");
    refused(&f, AdmissionError::Bound);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_asset("one.bin", &vec![0; ASSET_FILE_BYTES + 1]);
    refused(&f, AdmissionError::Bound);
}
#[test]
fn artifact_and_package_limits_do_not_fetch_or_unpack() {
    let f = package_fixture(RuntimeProfile::JavaScript, PACKAGES);
    assert_eq!(f.admit().unwrap().artifacts().len(), ARTIFACTS);
    refused(
        &package_fixture(RuntimeProfile::JavaScript, PACKAGES + 1),
        AdmissionError::Bound,
    );
    let mut f = defu_fixture();
    f.artifacts[0].1 = vec![255; ARTIFACT_BYTES];
    f.sync_files();
    f.declaration["files"][1]["source"] = json!("pkg_0");
    f.declaration["files"][2]["source"] = json!("pkg_0");
    assert_eq!(
        f.admit().unwrap().artifacts()[0].bytes().len(),
        ARTIFACT_BYTES
    );
    f.artifacts[0].1.push(255);
    f.sync_files();
    refused(&f, AdmissionError::Bound);
}
#[test]
fn exact_complete_encoding_limit_refuses_one_extra_byte_and_expansion() {
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    // A literal C0 byte in owned source DATA expands to six JSON bytes. Hash
    // text has a fixed 64-byte length, so this independent sizing formula holds.
    let overhead = f.data_encoding().len();
    let remaining = ENCODED_BYTES - overhead;
    f.entry.1 = vec![1; remaining / 6];
    f.entry.1.extend(std::iter::repeat_n(b' ', remaining % 6));
    f.sync_files();
    assert_eq!(f.data_encoding().len(), ENCODED_BYTES);
    assert_eq!(f.admit().unwrap().encoded_bytes(), ENCODED_BYTES);
    f.entry.1.push(b' ');
    f.sync_files();
    refused(&f, AdmissionError::Bound);
    f.entry.1 = vec![1; CODE_BYTES];
    f.sync_files();
    refused(&f, AdmissionError::Bound);
}
#[test]
fn measured_encoding_matches_independent_json_and_canonical_base64() {
    let mut f = Fixture::new(RuntimeProfile::TypeScript);
    f.entry.1 = "// \\\"\r\n🛠\t\u{1}".as_bytes().to_vec();
    f.add_module("m.ts", b"// owned source");
    for size in 0..10 {
        f.add_asset(
            &format!("bytes{size}.bin"),
            &(0..size).map(|i| i as u8).collect::<Vec<_>>(),
        );
    }
    f.sync_files();
    let sources: Vec<_> = f.modules.iter().map(|(n,b)|json!({"name":std::str::from_utf8(n).unwrap(),"source":std::str::from_utf8(b).unwrap()})).collect();
    let assets: Vec<_> = f.assets.iter().map(|(n,b)|json!({"name":std::str::from_utf8(n).unwrap(),"base64":base64::engine::general_purpose::STANDARD.encode(b)})).collect();
    let independent = json!({"runtime":f.runtime.name(),"metadata":std::str::from_utf8(&f.metadata()).unwrap(),
        "entry":{"name":f.entry_name(),"source":std::str::from_utf8(&f.entry.1).unwrap()},
        "modules":sources,"assets":assets,"artifacts":[],"image_provided":false});
    let encoded = f.data_encoding();
    assert_eq!(
        serde_json::from_slice::<Value>(&encoded).unwrap(),
        independent
    );
    assert_eq!(
        f.admit().unwrap().encoded_bytes(),
        serde_json::to_vec(&independent).unwrap().len()
    );
}
#[test]
fn data_asset_and_metadata_encoding_are_included_in_the_whole_bound() {
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    f.entry.1 = vec![1; 150_000];
    f.add_asset("data.bin", &vec![255; ASSET_FILE_BYTES]);
    f.sync_files();
    f.admit().unwrap();
    f.add_asset("second.bin", &vec![255; ASSET_FILE_BYTES]);
    refused(&f, AdmissionError::Bound);
}
#[test]
fn metadata_raw_limit_accepts_exact_whitespace_but_not_one_extra_byte() {
    let f = Fixture::new(RuntimeProfile::JavaScript);
    let mut bytes = f.metadata();
    bytes.resize(METADATA_BYTES, b' ');
    assert_eq!(
        f.admit_with(&bytes, None).unwrap().metadata_bytes().len(),
        METADATA_BYTES
    );
    bytes.push(b' ');
    assert!(matches!(
        f.admit_with(&bytes, None),
        Err(AdmissionError::Bound)
    ));
}
#[test]
fn metadata_decoded_utf8_limit_handles_real_surrogate_expansion() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.declaration["sources"][0]["license"] = json!("🛠".repeat(256));
    let bytes = String::from_utf8(f.metadata())
        .unwrap()
        .replace('🛠', "\\ud83d\\udee0")
        .into_bytes();
    f.admit_with(&bytes, None).unwrap();
    f.declaration["sources"][0]["license"] = json!("🛠".repeat(257));
    let bytes = String::from_utf8(f.metadata())
        .unwrap()
        .replace('🛠', "\\ud83d\\udee0")
        .into_bytes();
    assert!(matches!(
        f.admit_with(&bytes, None),
        Err(AdmissionError::Bound)
    ));
}

#[test]
fn module_path_component_depth_and_name_byte_limits_are_exact() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    let name = format!("{}.js", "a".repeat(COMPONENT_BYTES - 3));
    f.add_module(&name, b"");
    f.admit().unwrap();
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module(&format!("{}.js", "a".repeat(COMPONENT_BYTES - 2)), b"");
    refused(&f, AdmissionError::Name);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    let name = format!(
        "{}/{}/{}/{}/a.js",
        "a".repeat(255),
        "b".repeat(255),
        "c".repeat(255),
        "d".repeat(251)
    );
    assert_eq!(name.len(), PATH_BYTES);
    f.add_module(&name, b"");
    f.admit().unwrap();
    f.modules[0].0.push(b'x');
    f.sync_files();
    refused(&f, AdmissionError::Bound);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module(&format!("{}m.js", "d/".repeat(PATH_DEPTH - 1)), b"");
    f.admit().unwrap();
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module(&format!("{}m.js", "d/".repeat(PATH_DEPTH)), b"");
    refused(&f, AdmissionError::Name);
}
#[test]
fn aggregate_original_name_bytes_are_bounded_before_copy_or_hash() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    for index in 0..63 {
        let size = if index == 62 { 256 } else { 260 };
        let prefix = format!("dd/{index:02}/");
        f.add_module(
            &format!("{}{}.js", prefix, "a".repeat(size - prefix.len() - 3)),
            b"",
        );
    }
    assert_eq!(
        f.entry.0.len() + f.modules.iter().map(|(n, _)| n.len()).sum::<usize>(),
        NAME_BYTES
    );
    let checked = f.admit().unwrap();
    assert_eq!(checked.modules().len(), 63);
    let name = &mut f.modules.last_mut().unwrap().0;
    name.insert(name.len() - 3, b'a');
    f.sync_files();
    refused(&f, AdmissionError::Bound);
}
#[test]
fn python_identifier_length_and_original_flat_profile_are_distinct() {
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    f.add_module(&format!("bundle_{}", "a".repeat(IDENTIFIER_BYTES - 7)), b"");
    f.admit().unwrap();
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    f.add_module(&format!("bundle_{}", "a".repeat(IDENTIFIER_BYTES - 6)), b"");
    refused(&f, AdmissionError::Name);
    for name in [
        "bundle_x.py",
        "bundle_x/y",
        "bundle_x.y",
        "bundle_x-y",
        "bundle_資料",
        "./bundle_x",
        "/skill/bundle_x",
        "bundle_",
        "ordinary_helper",
    ] {
        let mut f = Fixture::new(RuntimeProfile::Cpython);
        f.add_module(name, b"");
        refused(&f, AdmissionError::Name);
    }
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    f.entry.0 = b"other.py".to_vec();
    f.sync_files();
    refused(&f, AdmissionError::Name);
}
#[test]
fn python_preloaded_and_native_modules_cannot_be_overwritten() {
    for name in [
        "sys",
        "os",
        "io",
        "json",
        "pathlib",
        "importlib",
        "traceback",
        "guest_stdio",
        "runner_v4",
        "componentize_py_types",
        "wit_world",
        "_struct",
        "builtins",
        "encodings",
        "types",
        "collections",
        "socket",
        "subprocess",
        "ctypes",
        "_ctypes",
        "posix",
        "nt",
        "marshal",
        "time",
    ] {
        let mut f = Fixture::new(RuntimeProfile::Cpython);
        f.add_module(name, b"");
        refused(&f, AdmissionError::Name);
    }
}

#[test]
fn asset_names_respect_the_distinct_ascii_cpython_virtual_profile() {
    let mut js = Fixture::new(RuntimeProfile::JavaScript);
    js.add_asset("資料.bin", &[0, 255]);
    assert_eq!(js.admit().unwrap().assets()[0].name(), "資料.bin");
    let mut py = Fixture::new(RuntimeProfile::Cpython);
    py.add_asset("資料.bin", &[0, 255]);
    refused(&py, AdmissionError::Name);
    let mut py = Fixture::new(RuntimeProfile::Cpython);
    py.add_asset("data/input.bin", &[0, 255]);
    assert_eq!(py.admit().unwrap().assets()[0].bytes(), [0, 255]);
}
#[test]
fn javascript_and_typescript_support_no_fallback_module_kinds() {
    for name in [
        "m.ts", "m.jsx", "m.tsx", "m.cjs", "m.py", "m.node", "m.wasm", "m.so",
    ] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        f.add_module(name, b"");
        refused(&f, AdmissionError::Profile);
    }
    let mut ts = Fixture::new(RuntimeProfile::TypeScript);
    ts.add_module("m.ts", b"");
    ts.admit().unwrap();
    ts.add_module("m.d.ts", b"");
    refused(&ts, AdmissionError::Profile);
}
#[test]
fn ambiguous_traversal_absolute_device_and_reserved_paths_refuse_whole() {
    for name in [
        "../m.js",
        "a/../m.js",
        "./m.js",
        "a/./m.js",
        "a//m.js",
        "/skill/m.js",
        "/work/m.js",
        "C:/m.js",
        "C:m.js",
        "\\\\server\\m.js",
        "a\\m.js",
        "CON.js",
        "CON .js",
        "COM1 .js",
        "a/NUL.js",
        "LPT9.js",
        "COM¹.js",
        "m.js ",
        " m.js",
        "m.js.",
        "a?.js",
        "a*.js",
        "a\".js",
        "node_modules/m.js",
        ".craftsman/m.js",
        ".craftsman-cache/m.js",
        "__pycache__/m.js",
        "a\u{feff}.js",
        "a\u{202e}.js",
        "a\u{7f}.js",
        "a\u{85}.js",
        "a／b.js",
    ] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        f.add_module(name, b"");
        refused(&f, AdmissionError::Name);
    }
    for name in ["node:fs", "node:fs/promises", "node:path"] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        f.add_module(name, b"");
        refused(&f, AdmissionError::Name);
    }
}
#[test]
fn duplicate_canonical_case_and_parent_collisions_never_merge() {
    for pair in [
        ("a.js", "a.js"),
        ("A.js", "a.js"),
        ("café.js", "cafe\u{301}.js"),
        ("Ａ.js", "A.js"),
        ("a.js", "a.js/b.js"),
    ] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        f.add_module(pair.0, b"");
        f.add_module(pair.1, b"");
        refused(&f, AdmissionError::Collision);
    }
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module("entry.js", b"different");
    refused(&f, AdmissionError::Collision);
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    f.add_module("bundle_A", b"");
    f.add_module("bundle_a", b"");
    refused(&f, AdmissionError::Collision);
}
#[test]
fn code_asset_collisions_and_hidden_executable_assets_refuse() {
    for name in [
        "entry.js", "m.js", "m.mjs", "m.ts", "m.py", "m.pyc", "m.node", "m.wasm", "m.SO",
    ] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        f.add_asset(name, b"");
        refused(&f, AdmissionError::Name);
    }
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_asset("a.bin", b"");
    f.add_asset("A.bin", b"other");
    refused(&f, AdmissionError::Collision);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_asset("data.bin", b"");
    f.add_module("data.bin/m.js", b"");
    refused(&f, AdmissionError::Collision);
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    f.add_module("six", b"");
    f.add_asset("six.py", b"");
    refused(&f, AdmissionError::Name);
}

#[test]
fn exact_pinned_defu_and_six_struct_are_metadata_fixtures_only() {
    let defu = defu_fixture();
    defu.admit().unwrap();
    let python = six_struct_fixture();
    python.admit().unwrap();
    assert_eq!(
        defu.declaration["sources"][1]["distribution"]["requires"],
        json!([])
    );
    assert_eq!(
        python.declaration["direct_packages"][0]["version"],
        "1.17.0"
    );
    assert!(
        python
            .modules
            .iter()
            .all(|(_, bytes)| bytes.starts_with(b"# owned"))
    );
}
#[test]
fn package_ranges_extras_markers_and_native_hook_fields_refuse() {
    for pin in [
        ">=6.1.7",
        "^6.1.7",
        "~6.1.7",
        "6.*",
        "latest",
        "6.1.7-beta.1",
        "06.1.7",
        "6.1",
        "6.1.7;python_version>3",
        "6.1.7+local",
    ] {
        let mut f = defu_fixture();
        f.declaration["sources"][1]["distribution"]["version"] = json!(pin);
        refused(&f, AdmissionError::Metadata);
    }
    for name in [
        "defu[extra]",
        "@scope/defu",
        "defu;marker",
        "Defu",
        "git+https://fixture.invalid/defu",
    ] {
        let mut f = defu_fixture();
        f.declaration["sources"][1]["distribution"]["name"] = json!(name);
        refused(&f, AdmissionError::Name);
    }
    for field in [
        "extras", "markers", "scripts", "hooks", "install", "native", "resolver", "ready",
    ] {
        let mut f = defu_fixture();
        f.declaration["sources"][1]["distribution"][field] = json!([]);
        refused(&f, AdmissionError::Metadata);
    }
}
#[test]
fn package_pin_artifact_owner_and_supplied_byte_gaps_refuse() {
    let mut f = defu_fixture();
    f.declaration["direct_packages"][0]["version"] = json!("6.1.8");
    refused(&f, AdmissionError::Closure);
    let mut f = defu_fixture();
    f.artifacts.clear();
    f.sync_files();
    refused(&f, AdmissionError::Closure);
    let mut f = defu_fixture();
    f.declaration["files"][2]["source"] = json!("script");
    refused(&f, AdmissionError::Closure);
    let mut f = defu_fixture();
    f.declaration["direct_packages"] = json!([]);
    refused(&f, AdmissionError::Closure);
    let mut f = defu_fixture();
    f.declaration["files"][1]["source"] = json!("missing");
    refused(&f, AdmissionError::Closure);
}
#[test]
fn transitive_pins_require_present_exact_packages_and_cross_source_edges() {
    let mut f = package_fixture(RuntimeProfile::JavaScript, 2);
    f.declaration["direct_packages"] = json!([{"name":"pkg0","version":"1.0.0"}]);
    f.declaration["sources"][1]["distribution"]["requires"] =
        json!([{"name":"pkg1","version":"1.0.0"}]);
    f.declaration["dependencies"] = json!([
        module_edge("entry.js", "pkg0.mjs"),
        module_edge("pkg0.mjs", "pkg1.mjs")
    ]);
    f.admit().unwrap();
    f.declaration["sources"][1]["distribution"]["requires"][0]["version"] = json!("1.0.1");
    refused(&f, AdmissionError::Closure);
    f.declaration["sources"][1]["distribution"]["requires"][0]["version"] = json!("1.0.0");
    f.declaration["dependencies"] = json!([
        module_edge("entry.js", "pkg0.mjs"),
        module_edge("entry.js", "pkg1.mjs")
    ]);
    refused(&f, AdmissionError::Closure);
}
#[test]
fn undeclared_package_edges_and_same_name_multi_versions_refuse() {
    let mut f = package_fixture(RuntimeProfile::JavaScript, 2);
    f.declaration["dependencies"] = json!([
        module_edge("entry.js", "pkg0.mjs"),
        module_edge("pkg0.mjs", "pkg1.mjs")
    ]);
    refused(&f, AdmissionError::Closure);
    let mut f = package_fixture(RuntimeProfile::JavaScript, 2);
    f.declaration["sources"][2]["distribution"]["name"] = json!("pkg0");
    f.declaration["sources"][2]["distribution"]["version"] = json!("2.0.0");
    refused(&f, AdmissionError::Collision);
    let mut f = package_fixture(RuntimeProfile::JavaScript, 2);
    f.declaration["sources"][1]["distribution"]["name"] = json!("p__kg");
    f.declaration["sources"][2]["distribution"]["name"] = json!("p-kg");
    refused(&f, AdmissionError::Collision);
}
#[test]
fn stdlib_wrapper_requires_exact_runtime_and_embedded_representation() {
    let mut f = six_struct_fixture();
    f.declaration["dependencies"].as_array_mut().unwrap().pop();
    refused(&f, AdmissionError::Profile);
    let mut f = six_struct_fixture();
    f.declaration["sources"][2]["distribution"]["version"] = json!("3.13.0");
    refused(&f, AdmissionError::Profile);
    let mut f = six_struct_fixture();
    f.declaration["dependencies"][2] = runtime_edge("struct", "ctypes");
    refused(&f, AdmissionError::Profile);
    let mut f = six_struct_fixture();
    f.modules[1].1.push(b' ');
    refused(&f, AdmissionError::Digest);
}
#[test]
fn runtime_references_follow_actual_js_and_python_rules() {
    for runtime in [RuntimeProfile::JavaScript, RuntimeProfile::TypeScript] {
        let mut f = Fixture::new(runtime);
        f.declaration["dependencies"] = json!([
            runtime_edge(f.entry_name(), "node:fs"),
            runtime_edge(f.entry_name(), "node:fs/promises"),
            runtime_edge(f.entry_name(), "node:path")
        ]);
        f.admit().unwrap();
        let entry = f.entry_name().to_owned();
        f.declaration["dependencies"]
            .as_array_mut()
            .unwrap()
            .push(runtime_edge(&entry, "node:http"));
        refused(&f, AdmissionError::Profile);
    }
    let mut py = Fixture::new(RuntimeProfile::Cpython);
    py.declaration["dependencies"] = json!([runtime_edge("main.py", "json")]);
    py.admit().unwrap();
    py.declaration["dependencies"] = json!([runtime_edge("main.py", "node:fs")]);
    refused(&py, AdmissionError::Profile);
}

#[test]
fn every_supplied_file_and_metadata_record_must_match_a_complete_closure() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module("m.js", b"");
    f.declaration["dependencies"] = json!([]);
    refused(&f, AdmissionError::Closure);
    f.declaration["dependencies"] = json!([module_edge("entry.js", "missing.js")]);
    refused(&f, AdmissionError::Closure);
    f.declaration["dependencies"] = json!([module_edge("missing.js", "m.js")]);
    refused(&f, AdmissionError::Closure);
    f.declaration["dependencies"] = json!([asset_edge("entry.js", "m.js")]);
    refused(&f, AdmissionError::Closure);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_asset("d.bin", b"");
    f.declaration["dependencies"] = json!([module_edge("entry.js", "d.bin")]);
    refused(&f, AdmissionError::Closure);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    let duplicate = f.declaration["files"][0].clone();
    f.declaration["files"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    refused(&f, AdmissionError::Closure);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.declaration["files"][0]["name"] = json!("missing.js");
    refused(&f, AdmissionError::Closure);
}
#[test]
fn dependency_cycles_duplicates_and_unsupported_targets_refuse() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module("m.js", b"");
    f.declaration["dependencies"]
        .as_array_mut()
        .unwrap()
        .push(module_edge("m.js", "entry.js"));
    refused(&f, AdmissionError::Closure);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module("m.js", b"");
    f.declaration["dependencies"]
        .as_array_mut()
        .unwrap()
        .push(module_edge("entry.js", "m.js"));
    refused(&f, AdmissionError::Collision);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.declaration["dependencies"] =
        json!([{"from":"entry.js","target":{"kind":"native","name":"node"}}]);
    refused(&f, AdmissionError::Metadata);
}
#[test]
fn dependency_count_exact_bound_is_checked_before_partial_admission() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    for index in 0..63 {
        f.add_module(&format!("m{index}.js"), b"");
    }
    let mut edges = f.declaration["dependencies"].as_array().unwrap().clone();
    'outer: for from in 0..4 {
        for to in 4..63 {
            if edges.len() == DEPENDENCIES {
                break 'outer;
            }
            edges.push(module_edge(&format!("m{from}.js"), &format!("m{to}.js")));
        }
    }
    assert_eq!(edges.len(), DEPENDENCIES);
    f.declaration["dependencies"] = json!(edges);
    f.admit().unwrap();
    f.declaration["dependencies"]
        .as_array_mut()
        .unwrap()
        .push(runtime_edge("entry.js", "node:path"));
    refused(&f, AdmissionError::Bound);
}
fn chain(nodes: usize) -> Fixture {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    for index in 1..nodes {
        f.add_module(&format!("m{index}.js"), b"");
    }
    let edges: Vec<_> = (1..nodes)
        .map(|index| {
            module_edge(
                if index == 1 {
                    "entry.js".to_owned()
                } else {
                    format!("m{}.js", index - 1)
                }
                .as_str(),
                &format!("m{index}.js"),
            )
        })
        .collect();
    f.declaration["dependencies"] = json!(edges);
    f
}
#[test]
fn exact_dependency_depth_and_later_long_diamond_path_are_enforced() {
    chain(DEPENDENCY_DEPTH).admit().unwrap();
    refused(&chain(DEPENDENCY_DEPTH + 1), AdmissionError::Closure);
    let mut f = chain(DEPENDENCY_DEPTH + 1);
    f.declaration["dependencies"]
        .as_array_mut()
        .unwrap()
        .insert(0, module_edge("entry.js", "m8.js"));
    refused(&f, AdmissionError::Closure);
}
#[test]
fn bounded_diamond_graph_is_admitted_without_path_explosion() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    for level in 0..7 {
        for column in 0..6 {
            f.add_module(&format!("l{level}_{column}.js"), b"");
        }
    }
    let mut edges = Vec::new();
    for column in 0..6 {
        edges.push(module_edge("entry.js", &format!("l0_{column}.js")));
    }
    for level in 0..6 {
        for from in 0..6 {
            for to in 0..6 {
                edges.push(module_edge(
                    &format!("l{level}_{from}.js"),
                    &format!("l{}_{to}.js", level + 1),
                ));
            }
        }
    }
    f.declaration["dependencies"] = json!(edges);
    f.admit().unwrap();
}
#[test]
fn package_cycles_duplicate_direct_pins_and_duplicate_requires_refuse() {
    let mut f = package_fixture(RuntimeProfile::JavaScript, 2);
    f.declaration["sources"][1]["distribution"]["requires"] =
        json!([{"name":"pkg1","version":"1.0.0"}]);
    f.declaration["sources"][2]["distribution"]["requires"] =
        json!([{"name":"pkg0","version":"1.0.0"}]);
    refused(&f, AdmissionError::Closure);
    let mut f = defu_fixture();
    let pin = f.declaration["direct_packages"][0].clone();
    f.declaration["direct_packages"]
        .as_array_mut()
        .unwrap()
        .push(pin);
    refused(&f, AdmissionError::Collision);
    let mut f = package_fixture(RuntimeProfile::JavaScript, 2);
    f.declaration["sources"][1]["distribution"]["requires"] =
        json!([{"name":"pkg1","version":"1.0.0"},{"name":"pkg1","version":"1.0.0"}]);
    refused(&f, AdmissionError::Collision);
}
#[test]
fn provenance_count_unused_duplicate_and_alias_records_refuse() {
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    for index in 1..SOURCES {
        f.add_module(&format!("m{index}.js"), b"");
    }
    f.declaration["sources"] = json!(
        (0..SOURCES)
            .map(|index| local_source(&format!("source_{index}")))
            .collect::<Vec<_>>()
    );
    for (index, file) in f.declaration["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        file["source"] = json!(format!("source_{index}"));
    }
    f.admit().unwrap();
    f.declaration["sources"]
        .as_array_mut()
        .unwrap()
        .push(local_source("extra"));
    refused(&f, AdmissionError::Bound);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.declaration["sources"]
        .as_array_mut()
        .unwrap()
        .push(local_source("unused"));
    refused(&f, AdmissionError::Closure);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.add_module("m.js", b"");
    f.declaration["sources"]
        .as_array_mut()
        .unwrap()
        .push(local_source("SCRIPT"));
    f.declaration["files"][1]["source"] = json!("SCRIPT");
    refused(&f, AdmissionError::Closure);
}

#[test]
fn wrong_loader_transform_engine_image_or_profile_never_falls_back() {
    for (field, value) in [
        ("profile", json!("node")),
        ("engine", json!("bun")),
        ("image_sha256", json!("0".repeat(64))),
        ("image_bytes", json!(1)),
        ("loader_sha256", json!("0".repeat(64))),
        ("transform", json!(null)),
    ] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        f.declaration["runtime"][field] = value;
        refused(&f, AdmissionError::Profile);
    }
    for (field, value) in [
        ("version", json!("3.35.2")),
        ("name", json!("typescript")),
        ("sha256", json!("0".repeat(64))),
    ] {
        let mut f = Fixture::new(RuntimeProfile::TypeScript);
        f.declaration["runtime"]["transform"][field] = value;
        refused(&f, AdmissionError::Profile);
    }
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    f.declaration["runtime"]["transform"] =
        runtime_record(RuntimeProfile::TypeScript)["transform"].clone();
    refused(&f, AdmissionError::Profile);
}
#[test]
fn supplied_image_data_requires_exact_known_bytes_and_never_mints_consent() {
    let f = Fixture::new(RuntimeProfile::JavaScript);
    for image in [
        Vec::new(),
        vec![0; 1],
        vec![0; RuntimeProfile::JavaScript.image_len()],
        vec![0; IMAGE_BYTES],
    ] {
        assert!(matches!(
            f.admit_with(&f.metadata(), Some(&image)),
            Err(AdmissionError::Image)
        ));
    }
    let image = vec![0; IMAGE_BYTES + 1];
    assert!(matches!(
        f.admit_with(&f.metadata(), Some(&image)),
        Err(AdmissionError::Bound)
    ));
}
#[test]
fn actual_distinct_guest_envelopes_are_sized_without_protocol_fields() {
    let mut js = Fixture::new(RuntimeProfile::TypeScript);
    js.add_module("m.ts", b"// owned DATA");
    let encoded = js.guest_encoding();
    assert_eq!(
        encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["entry", "input", "modules", "work"]
    );
    assert_eq!(encoded["modules"]["entry.ts"], "");
    assert!(encoded.get("version").is_none());
    let mut py = Fixture::new(RuntimeProfile::Cpython);
    py.add_module("bundle_m", b"# owned DATA");
    let encoded = py.guest_encoding();
    assert_eq!(
        encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["code", "cwd", "modules", "version"]
    );
    assert_eq!(encoded["cwd"], "/work");
    assert_eq!(encoded["version"], 1);
    assert!(encoded.get("entry").is_none() && encoded.get("work").is_none());
}
#[test]
fn malformed_utf8_nul_code_and_metadata_fail_without_lossy_repair() {
    for bytes in [
        vec![255],
        vec![0xc0, 0x80],
        vec![0xed, 0xa0, 0x80],
        b"a\0b".to_vec(),
    ] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        f.entry.1 = bytes;
        f.sync_files();
        refused(&f, AdmissionError::Utf8);
    }
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.entry.0 = vec![255];
    f.sync_files();
    refused(&f, AdmissionError::Utf8);
    let f = Fixture::new(RuntimeProfile::JavaScript);
    assert!(matches!(
        f.admit_with(&[255], None),
        Err(AdmissionError::Utf8)
    ));
    for bytes in [
        b"".as_slice(),
        b"null",
        b"true",
        b"{} {}",
        b"{\"version\":NaN}",
        b"{\"version\":-1}",
        b"{\"version\":1.0}",
        b"{\"version\":1e0}",
        b"{\"version\":01}",
        b"{\"version\":18446744073709551616}",
        b"{\"version\":1,}",
    ] {
        assert!(f.admit_with(bytes, None).is_err());
    }
}
#[test]
fn duplicate_escaped_json_keys_and_surrogate_errors_are_rejected() {
    let f = Fixture::new(RuntimeProfile::JavaScript);
    let bytes = String::from_utf8(f.metadata())
        .unwrap()
        .replacen("{", "{\"version\":1,\"\\u0076ersion\":1,", 1)
        .into_bytes();
    assert!(matches!(
        f.admit_with(&bytes, None),
        Err(AdmissionError::Metadata)
    ));
    for suffix in ["\\ud800", "\\udc00", "\\ud800\\u0041", "\\x41"] {
        let mut metadata = f.declaration.clone();
        metadata["sources"][0]["license"] = json!("sentinel");
        let bytes = serde_json::to_string(&metadata)
            .unwrap()
            .replace("sentinel", suffix)
            .into_bytes();
        assert!(matches!(
            f.admit_with(&bytes, None),
            Err(AdmissionError::Metadata)
        ));
    }
}
#[test]
fn excessive_json_depth_nodes_fields_and_decoded_key_sizes_refuse() {
    let f = Fixture::new(RuntimeProfile::JavaScript);
    let deep = format!("{}0{}", "[".repeat(13), "]".repeat(13));
    assert!(matches!(
        f.admit_with(deep.as_bytes(), None),
        Err(AdmissionError::Bound)
    ));
    let fields = format!(
        "{{{}}}",
        (0..33)
            .map(|i| format!("\"f{i}\":0"))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(matches!(
        f.admit_with(fields.as_bytes(), None),
        Err(AdmissionError::Bound)
    ));
    let nodes = format!(
        "[{}]",
        std::iter::repeat_n(
            format!(
                "[{}]",
                std::iter::repeat_n("0", 255).collect::<Vec<_>>().join(",")
            ),
            17
        )
        .collect::<Vec<_>>()
        .join(",")
    );
    assert!(matches!(
        f.admit_with(nodes.as_bytes(), None),
        Err(AdmissionError::Bound)
    ));
    let key = format!("{{\"{}\":0}}", "a".repeat(65));
    assert!(matches!(
        f.admit_with(key.as_bytes(), None),
        Err(AdmissionError::Bound)
    ));
}
#[test]
fn hashes_are_recomputed_and_declared_hash_grammar_is_exact() {
    assert_eq!(
        hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    for digest in [
        "f".repeat(63),
        "f".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        let mut f = Fixture::new(RuntimeProfile::JavaScript);
        f.declaration["files"][0]["sha256"] = json!(digest);
        refused(&f, AdmissionError::Metadata);
    }
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.declaration["files"][0]["sha256"] = json!("0".repeat(64));
    refused(&f, AdmissionError::Digest);
}
#[test]
fn refusal_diagnostics_contain_no_original_path_source_body_or_grant_label() {
    let marker = "/Users/synthetic-owner/private-root/model-body-approval";
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.entry.0 = marker.as_bytes().to_vec();
    f.sync_files();
    let error = match f.admit() {
        Err(error) => error,
        Ok(_) => panic!("invalid data accepted"),
    };
    assert_eq!(error.to_string(), "Script bundle data admission refused.");
    assert!(!error.to_string().contains(marker));
    for error in [
        AdmissionError::Bound,
        AdmissionError::Utf8,
        AdmissionError::Metadata,
        AdmissionError::Profile,
        AdmissionError::Name,
        AdmissionError::Collision,
        AdmissionError::Closure,
        AdmissionError::Digest,
        AdmissionError::Image,
    ] {
        assert_eq!(error.to_string(), "Script bundle data admission refused.");
    }
}
#[test]
fn sensitive_data_types_have_no_debug_serialize_or_clone_traits() {
    trait Absent<A> {
        fn check() {}
    }
    struct DebugMarker;
    struct SerializeMarker;
    struct CloneMarker;
    impl<T: ?Sized> Absent<()> for T {}
    impl<T: ?Sized + std::fmt::Debug> Absent<DebugMarker> for T {}
    impl<T: ?Sized + serde::Serialize> Absent<SerializeMarker> for T {}
    impl<T: ?Sized + Clone> Absent<CloneMarker> for T {}
    let _ = <CandidateBundle<'static> as Absent<_>>::check;
    let _ = <SuppliedFile<'static> as Absent<_>>::check;
    let _ = <CheckedBundle as Absent<_>>::check;
    let _ = <CheckedFile as Absent<_>>::check;
}

#[test]
fn every_original_name_mutation_refuses_stale_records_and_changes_binding() {
    for category in 0..4 {
        let mut f = defu_fixture();
        f.add_asset("data.bin", b"owned data");
        f.declaration["files"][1]["source"] = json!("pkg_0");
        f.declaration["files"][3]["source"] = json!("pkg_0");
        let before = *f.admit().unwrap().content_digest();
        match category {
            0 => f.entry.0 = b"other.js".to_vec(),
            1 => f.modules[0].0 = b"other.mjs".to_vec(),
            2 => f.assets[0].0 = b"other.bin".to_vec(),
            _ => f.artifacts[0].0 = b"other.fixture".to_vec(),
        }
        refused(&f, AdmissionError::Closure);
        f.sync_files();
        f.declaration["files"][1]["source"] = json!("pkg_0");
        f.declaration["files"][3]["source"] = json!("pkg_0");
        match category {
            0 => {
                f.declaration["dependencies"][0]["from"] = json!("other.js");
                f.declaration["dependencies"][1]["from"] = json!("other.js");
            }
            1 => f.declaration["dependencies"][0]["target"]["name"] = json!("other.mjs"),
            2 => f.declaration["dependencies"][1]["target"]["name"] = json!("other.bin"),
            _ => f.declaration["sources"][1]["distribution"]["artifact"] = json!("other.fixture"),
        }
        assert_ne!(f.admit().unwrap().content_digest(), &before);
    }
}
#[test]
fn exact_pin_and_license_changes_bind_even_when_provided_bytes_are_unchanged() {
    let mut f = defu_fixture();
    let before = *f.admit().unwrap().content_digest();
    f.declaration["sources"][1]["distribution"]["version"] = json!("6.1.8");
    refused(&f, AdmissionError::Closure);
    f.declaration["direct_packages"][0]["version"] = json!("6.1.8");
    let new_pin = *f.admit().unwrap().content_digest();
    assert_ne!(new_pin, before);
    f.declaration["sources"][1]["license"] = json!("changed original notice\r\n");
    assert_ne!(f.admit().unwrap().content_digest(), &new_pin);
}
#[test]
fn nested_pin_target_and_distribution_authority_flags_are_never_data_shortcuts() {
    for location in 0..3 {
        let mut f = defu_fixture();
        match location {
            0 => f.declaration["direct_packages"][0]["granted"] = json!(true),
            1 => f.declaration["dependencies"][0]["target"]["approved"] = json!(true),
            _ => f.declaration["sources"][1]["distribution"]["ready"] = json!(true),
        }
        refused(&f, AdmissionError::Metadata);
    }
}
#[test]
fn preflight_bounds_precede_invalid_metadata_and_actual_byte_digests() {
    let mut f = Fixture::new(RuntimeProfile::Cpython);
    f.entry.1 = vec![255; CODE_BYTES + 1];
    assert!(matches!(
        f.admit_with(b"invalid metadata", None),
        Err(AdmissionError::Bound)
    ));
    f.entry.1.clear();
    assert!(matches!(
        f.admit_with(&vec![255; METADATA_BYTES + 1], None),
        Err(AdmissionError::Bound)
    ));
    f.declaration["files"][0]["sha256"] = json!("0".repeat(64));
    f.entry.1 = vec![b' '; CODE_BYTES + 1];
    refused(&f, AdmissionError::Bound);
}
#[test]
fn schema_array_and_identifier_caps_are_checked_before_metadata_copy() {
    let f = Fixture::new(RuntimeProfile::JavaScript);
    for field in ["direct_packages", "requires"] {
        let value = json!({field:(0..33).map(|index|json!({"name":format!("pkg{index}"),"version":"1.0.0"})).collect::<Vec<_>>()});
        assert!(matches!(
            f.admit_with(&serde_json::to_vec(&value).unwrap(), None),
            Err(AdmissionError::Bound)
        ));
    }
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.declaration["sources"][0]["id"] = json!("a".repeat(IDENTIFIER_BYTES + 1));
    refused(&f, AdmissionError::Bound);
    let mut f = Fixture::new(RuntimeProfile::JavaScript);
    f.declaration["sources"][0]["origin"] = json!("a".repeat(513));
    refused(&f, AdmissionError::Bound);
}
#[test]
fn code_and_package_edges_share_one_complete_work_ceiling() {
    let mut f = package_fixture(RuntimeProfile::JavaScript, 2);
    for index in 0..61 {
        f.add_module(&format!("m{index}.js"), b"");
    }
    f.sync_files();
    f.declaration["files"][1]["source"] = json!("pkg_0");
    f.declaration["files"][2]["source"] = json!("pkg_1");
    f.declaration["files"][64]["source"] = json!("pkg_0");
    f.declaration["files"][65]["source"] = json!("pkg_1");
    f.declaration["direct_packages"] = json!([{"name":"pkg0","version":"1.0.0"}]);
    f.declaration["sources"][1]["distribution"]["requires"] =
        json!([{"name":"pkg1","version":"1.0.0"}]);
    let mut edges = vec![
        module_edge("entry.js", "pkg0.mjs"),
        module_edge("pkg0.mjs", "pkg1.mjs"),
    ];
    for index in 0..61 {
        edges.push(module_edge("entry.js", &format!("m{index}.js")));
    }
    'outer: for from in 0..4 {
        for to in 4..61 {
            if edges.len() == DEPENDENCIES - 1 {
                break 'outer;
            }
            edges.push(module_edge(&format!("m{from}.js"), &format!("m{to}.js")));
        }
    }
    assert_eq!(edges.len(), DEPENDENCIES - 1);
    f.declaration["dependencies"] = json!(edges);
    f.admit().unwrap();
    f.declaration["dependencies"]
        .as_array_mut()
        .unwrap()
        .push(runtime_edge("entry.js", "node:path"));
    refused(&f, AdmissionError::Bound);
}
fn package_chain(nodes: usize) -> Fixture {
    let mut f = package_fixture(RuntimeProfile::JavaScript, nodes);
    f.declaration["files"][0]["source"] = json!("pkg_0");
    f.declaration["sources"].as_array_mut().unwrap().remove(0);
    f.declaration["direct_packages"] = json!([{"name":"pkg0","version":"1.0.0"}]);
    let mut edges = vec![module_edge("entry.js", "pkg0.mjs")];
    for index in 1..nodes {
        f.declaration["sources"][index - 1]["distribution"]["requires"] =
            json!([{"name":format!("pkg{index}"),"version":"1.0.0"}]);
        let from = if index == 1 {
            "entry.js".to_owned()
        } else {
            format!("pkg{}.mjs", index - 1)
        };
        edges.push(module_edge(&from, &format!("pkg{index}.mjs")));
    }
    f.declaration["dependencies"] = json!(edges);
    f
}
#[test]
fn provided_package_closure_depth_has_exact_supported_boundary() {
    package_chain(DEPENDENCY_DEPTH).admit().unwrap();
    refused(
        &package_chain(DEPENDENCY_DEPTH + 1),
        AdmissionError::Closure,
    );
}

#[test]
#[ignore = "explicit owned fixed-image DATA fixture; no compilation or execution"]
fn owned_known_image_data_admission() {
    // Ordinary ignored/manual-all runs return before any path/image inspection.
    // The exact selector is a fixture marker, never production authority.
    if !std::env::args()
        .any(|arg| arg == "gadgets::script_bundle::tests::owned_known_image_data_admission")
    {
        return;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    for (runtime, file) in [
        (RuntimeProfile::JavaScript, "javascript.wasm"),
        (RuntimeProfile::Cpython, "python.wasm"),
    ] {
        let path = root
            .join("target/verification/script-bundle-admission/fixtures")
            .join(file);
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            runtime.image_len() as u64
        );
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(hex(&bytes), runtime.image_sha256());
        let f = Fixture::new(runtime);
        let without = f.admit().unwrap();
        let checked = f.admit_with(&f.metadata(), Some(&bytes)).unwrap();
        assert_eq!(checked.image_bytes().unwrap(), bytes);
        assert_ne!(checked.content_digest(), without.content_digest());
        assert_eq!(hex(&std::fs::read(path).unwrap()), runtime.image_sha256());
    }
}
