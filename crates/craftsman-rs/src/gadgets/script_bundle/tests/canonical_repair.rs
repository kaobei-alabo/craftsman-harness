//! Version-1 canonical denial repair controls: owned supplied DATA only.
//! Preserve the reviewer's exact 17 inputs/outcomes, then require consistent
//! denial without executing source, images, packages, loaders or a VM.

use super::*;

fn reviewed_fixture() -> Fixture {
    let mut fixture = Fixture::new(RuntimeProfile::JavaScript);
    fixture.declaration["sources"][0]["origin"] = json!("owned synthetic DATA");
    fixture.declaration["sources"][0]["license"] = json!("fixture");
    fixture
}

#[test]
fn frozen_reviewer_17_data_controls() {
    let bytes = [0u8, 255];
    let mut rows = Vec::new();
    let mut missing_denials = Vec::new();
    // Exact original strings and canonical keys from data-controls.stdout.jsonl.
    for (name, key, accepted) in [
        ("safe.bin", "safe.bin", true),
        ("hidden.js", "hidden.js", false),
        ("hidden.ｊｓ", "hidden.js", false),
        ("hidden．js", "hidden.js", false),
        ("hidden.ｔｓ", "hidden.ts", false),
        ("hidden.ｎｏｄｅ", "hidden.node", false),
        ("hidden.jſ", "hidden.js", false),
        ("m＊.bin", "m*.bin", false),
        ("m？.bin", "m?.bin", false),
        ("CON.bin", "con.bin", false),
        ("ＣＯＮ.bin", "con.bin", false),
    ] {
        let mut fixture = reviewed_fixture();
        fixture.add_asset(name, &bytes);
        let outcome = fixture.admit();
        if outcome.is_ok() != accepted {
            missing_denials.push(name);
        }
        rows.push(json!({"case":"asset-name-classification","original":name,
            "normalized_key":key,"accepted":outcome.is_ok(),
            "error":outcome.as_ref().err().map(ToString::to_string),
            "scope":"pure supplied synthetic DATA; no loader/executable/package/VM call"}));
    }

    let mut fixture = reviewed_fixture();
    fixture.declaration["approved"] = json!(true);
    let outcome = fixture.admit();
    rows.push(
        json!({"case":"authority-field-refusal","accepted":outcome.is_ok(),
        "error":outcome.as_ref().err().map(ToString::to_string)}),
    );
    assert!(outcome.is_err());

    let fixture = reviewed_fixture();
    let raw = fixture.metadata();
    let mut changed = reviewed_fixture();
    changed.entry.1 = b"owned change".to_vec();
    let outcome = changed.admit_with(&raw, None);
    rows.push(json!({"case":"actual-source-digest-refusal","accepted":outcome.is_ok()}));
    assert!(outcome.is_err());

    let mut duplicate = raw.clone();
    duplicate.splice(1..1, b"\"version\":1,".iter().copied());
    let outcome = fixture.admit_with(&duplicate, None);
    rows.push(json!({"case":"duplicate-metadata-key-refusal","accepted":outcome.is_ok()}));
    assert!(outcome.is_err());

    let mut spaced = raw.clone();
    spaced.extend_from_slice(b" \r\n");
    let first = fixture.admit_with(&raw, None).unwrap();
    let second = fixture.admit_with(&spaced, None).unwrap();
    let different = first.content_digest() != second.content_digest();
    rows.push(
        json!({"case":"original-metadata-byte-identity","different":different,
        "image_omitted_only_fixed_ref":first.image_bytes().is_none()}),
    );
    assert!(different && first.image_bytes().is_none());

    let outcome = fixture.admit_with(&raw, Some(&[0u8; 32]));
    rows.push(json!({"case":"supplied-image-substitution-refusal","accepted":outcome.is_ok()}));
    assert!(outcome.is_err());

    let text = "\u{feff}\r\n\t\\\"🛠\u{1}".as_bytes();
    let asset = [0u8, 255, 13, 10, 0];
    let mut fixture = reviewed_fixture();
    fixture.entry.1 = text.to_vec();
    fixture.add_asset("raw.bin", &asset);
    let raw = fixture.metadata();
    let checked = fixture.admit().unwrap();
    let independent = json!({"runtime":RuntimeProfile::JavaScript.name(),
        "metadata":std::str::from_utf8(&raw).unwrap(),
        "entry":{"name":"entry.js","source":std::str::from_utf8(text).unwrap()},
        "modules":[],"assets":[{"name":"raw.bin","base64":"AP8NCgA="}],
        "artifacts":[],"image_provided":false});
    let guest = json!({"entry":"entry.js","modules":{"entry.js":std::str::from_utf8(text).unwrap()},
        "work":{},"input":null});
    assert_eq!(
        checked.encoded_bytes(),
        serde_json::to_vec(&independent).unwrap().len()
    );
    assert_eq!(
        checked.guest_code_bytes(),
        serde_json::to_vec(&guest).unwrap().len()
    );
    assert_eq!(checked.encoded_bytes(), 1203);
    assert_eq!(checked.guest_code_bytes(), 92);
    rows.push(json!({"case":"independent-complete-encoding","bytes":checked.encoded_bytes(),
        "guest_bytes":checked.guest_code_bytes(),"original_code_retained":checked.entry().bytes()==text,
        "binary_retained":checked.assets()[0].bytes()==asset}));
    assert_eq!(checked.entry().bytes(), text);
    assert_eq!(checked.assets()[0].bytes(), asset);

    assert_eq!(rows.len(), 17);
    // These lines contain only the exact owned synthetic review controls, never
    // a serialized CheckedBundle, current task/root/consent or model body.
    for row in rows {
        println!("BUNDLE_CANONICAL_REPAIR_CONTROL {row}");
    }
    assert!(
        missing_denials.is_empty(),
        "canonical DATA denials missing: {missing_denials:?}"
    );
}

fn fullwidth(value: &str) -> String {
    value
        .chars()
        .map(|c| char::from_u32(c as u32 + 0xfee0).unwrap())
        .collect()
}

#[test]
fn ascii_fullwidth_dot_case_and_suffix_matrix_refuses_executable_assets() {
    for runtime in [RuntimeProfile::JavaScript, RuntimeProfile::TypeScript] {
        for suffix in [
            "js", "mjs", "cjs", "ts", "jsx", "tsx", "py", "pyc", "pyo", "so", "pyd", "dll", "node",
            "wasm",
        ] {
            for name in [
                format!("hidden.{suffix}"),
                format!("hidden.{}", suffix.to_ascii_uppercase()),
                format!("hidden.{}", fullwidth(suffix)),
                format!("hidden．{suffix}"),
                format!("hidden．{}", fullwidth(&suffix.to_ascii_uppercase())),
                format!("assets/hidden﹒{}", fullwidth(suffix)),
            ] {
                let mut fixture = Fixture::new(runtime);
                fixture.add_asset(&name, &[0, 255]);
                refused(&fixture, AdmissionError::Name);
            }
        }
    }
}

#[test]
fn long_s_and_mixed_compatibility_executable_suffixes_refuse_whole() {
    for name in [
        "hidden.jſ",
        "hidden.ｊſ",
        "hidden.mjſ",
        "hidden.cjſ",
        "hidden.tſ",
        "hidden.jſx",
        "hidden.tſx",
        "hidden.ſo",
        "hidden.waſm",
        "hidden．jſ",
        "hidden﹒ｊS",
        "hidden.Ｊs",
        "hidden．jＳ",
        "hidden．．ｊｓ",
        "assets/hidden．Ｔs",
        "assets/hidden.ｎOｄE",
    ] {
        let mut fixture = reviewed_fixture();
        fixture.add_asset(name, &[0, 255]);
        refused(&fixture, AdmissionError::Name);
    }
    for name in [
        "jſ.bin",
        "records．ｊｓ.bin",
        "file．jsx.dat",
        "data．wasm.bin",
    ] {
        let mut fixture = reviewed_fixture();
        fixture.add_asset(name, &[0, 255]);
        assert_eq!(fixture.admit().unwrap().assets()[0].name(), name);
    }
}

#[test]
fn canonical_forbidden_punctuation_refuses_assets_entries_modules_and_artifacts() {
    for (ascii, compatibility) in [
        ('<', '＜'),
        ('>', '＞'),
        ('"', '＂'),
        ('|', '｜'),
        ('?', '？'),
        ('*', '＊'),
        (':', '：'),
        ('\\', '＼'),
    ] {
        for c in [ascii, compatibility] {
            let mut asset = reviewed_fixture();
            asset.add_asset(&format!("m{c}.bin"), &[0, 255]);
            refused(&asset, AdmissionError::Name);

            let mut entry = reviewed_fixture();
            entry.entry.0 = format!("m{c}.js").into_bytes();
            entry.sync_files();
            refused(&entry, AdmissionError::Name);

            let mut module = reviewed_fixture();
            module.add_module(&format!("m{c}.js"), b"");
            refused(&module, AdmissionError::Name);

            let mut artifact = defu_fixture();
            let name = format!("m{c}.fixture");
            artifact.artifacts[0].0 = name.as_bytes().to_vec();
            artifact.sync_files();
            artifact.declaration["files"][1]["source"] = json!("pkg_0");
            artifact.declaration["files"][2]["source"] = json!("pkg_0");
            artifact.declaration["sources"][1]["distribution"]["artifact"] = json!(name);
            refused(&artifact, AdmissionError::Name);
        }
    }
}

#[test]
fn canonical_traversal_devices_separators_and_controls_remain_refused() {
    for name in [
        "../data.bin",
        "．．/data.bin",
        "．/data.bin",
        "folder/．．/data.bin",
        "folder/．/data.bin",
        "／data.bin",
        "folder／data.bin",
        "＼＼server＼data.bin",
        "C：data.bin",
        "ＣＯＮ.bin",
        "ＣＯＮ．ｂｉｎ",
        "ＡＵＸ.bin",
        "ＣＯＭ¹.bin",
        "ＬＰＴ９.bin",
        "ＣＯＮ　．bin",
        "data．",
        "　data.bin",
        "data.bin　",
        "data\0.bin",
        "data\u{1}.bin",
        "data\u{7f}.bin",
        "data\u{85}.bin",
        "data\u{feff}.bin",
        "data\u{202e}.bin",
        "data\u{2066}.bin",
        "．ｃｒａｆｔｓｍａｎ/data.bin",
        "ｎｏｄｅ＿ｍｏｄｕｌｅｓ/data.bin",
    ] {
        let mut fixture = reviewed_fixture();
        fixture.add_asset(name, &[0, 255]);
        refused(&fixture, AdmissionError::Name);
    }
}

#[test]
fn valid_original_compatibility_names_bytes_and_bindings_are_preserved() {
    let text = "\u{feff}// owned DATA\r\n🛠\r\n\u{1}".as_bytes();
    let binary = [0u8, 255, 13, 10, 0];
    for (original, canonical) in [
        ("Ｃａｆé．ｂｉｎ", "café.bin"),
        ("cafe\u{301}．DAT", "café.dat"),
        ("jſ-data．bin", "js-data.bin"),
        ("資料．bin", "資料.bin"),
        ("Data+literal.BIN", "data+literal.bin"),
    ] {
        let mut fixture = reviewed_fixture();
        fixture.entry.1 = text.to_vec();
        fixture.add_asset(original, &binary);
        let metadata = fixture.metadata();
        let checked = fixture.admit().unwrap();
        assert_eq!(checked.assets()[0].name().as_bytes(), original.as_bytes());
        assert_eq!(checked.assets()[0].bytes(), binary);
        assert_eq!(checked.assets()[0].sha256(), &sha256(&binary));
        assert_eq!(checked.entry().bytes(), text);
        assert_eq!(checked.metadata_bytes(), metadata);

        let independent = json!({"runtime":RuntimeProfile::JavaScript.name(),
            "metadata":std::str::from_utf8(&metadata).unwrap(),
            "entry":{"name":"entry.js","source":std::str::from_utf8(text).unwrap()},
            "modules":[],"assets":[{"name":original,"base64":"AP8NCgA="}],
            "artifacts":[],"image_provided":false});
        let encoded = fixture.data_encoding();
        assert_eq!(
            serde_json::from_slice::<Value>(&encoded).unwrap(),
            independent
        );
        assert_eq!(
            checked.encoded_bytes(),
            serde_json::to_vec(&independent).unwrap().len()
        );

        let mut other = reviewed_fixture();
        other.entry.1 = text.to_vec();
        other.add_asset(canonical, &binary);
        assert_ne!(
            checked.content_digest(),
            other.admit().unwrap().content_digest()
        );
    }
}

#[test]
fn code_suffixes_keep_original_profile_rules_without_alias_repair() {
    for (runtime, name) in [
        (RuntimeProfile::JavaScript, "module.ｊｓ"),
        (RuntimeProfile::JavaScript, "module．js"),
        (RuntimeProfile::JavaScript, "module.jſ"),
        (RuntimeProfile::TypeScript, "module.ｔｓ"),
        (RuntimeProfile::TypeScript, "module．ts"),
    ] {
        let mut fixture = Fixture::new(runtime);
        fixture.add_module(name, b"");
        refused(&fixture, AdmissionError::Profile);
    }
    let mut fixture = reviewed_fixture();
    fixture.add_module("Ｍｏｄｕｌｅ.js", b"// owned DATA\r\n");
    let checked = fixture.admit().unwrap();
    assert_eq!(checked.modules()[0].name(), "Ｍｏｄｕｌｅ.js");
    assert_eq!(checked.modules()[0].bytes(), b"// owned DATA\r\n");
}

#[test]
fn canonically_colliding_valid_assets_are_refused_without_merge() {
    let mut fixture = reviewed_fixture();
    fixture.add_asset("data.bin", &[0, 255]);
    fixture.add_asset("ｄａｔａ．ｂｉｎ", &[255, 0]);
    refused(&fixture, AdmissionError::Collision);
}
