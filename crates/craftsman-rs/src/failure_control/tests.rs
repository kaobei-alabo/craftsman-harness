use super::Settings;
use super::governor::{BudgetDecision, FailureGovernor, PauseCandidate, TrustedClass};
use super::integrity::{self, Coverage, Reason, ReviewNote};
use craftsman_harness::{ToolCall, ToolErrorKind, ToolMeta, ToolOutcome};
use serde_json::{Value, json};

fn settings(limit: u8) -> Settings {
    Settings::parse(Some(&json!({"repeat_limit": limit})))
        .unwrap()
        .unwrap()
}

fn governor(limit: u8) -> FailureGovernor {
    let governor = FailureGovernor::new(&settings(limit));
    assert!(governor.observe_input(1, [7; 32]));
    governor
}

fn call(args: Value) -> ToolCall {
    ToolCall {
        id: "model-id-is-not-authority".into(),
        name: "fixture-observational-operation".into(),
        args,
    }
}

fn failure() -> ToolOutcome {
    ToolOutcome::err("Fixture resource is unavailable.")
}

fn errors(governor: &FailureGovernor, count: usize, class: TrustedClass) {
    for _ in 0..count {
        governor.observe_actual(&call(json!({"file_path": "note.rs"})), &failure(), class);
    }
}

fn candidate(governor: &FailureGovernor) -> PauseCandidate {
    match governor.decision() {
        BudgetDecision::Pause(candidate) => candidate,
        _ => panic!("Expected an actual bounded pause candidate"),
    }
}

#[test]
fn settings_are_absent_by_default_and_never_guess_malformed_values() {
    assert!(Settings::parse(None).unwrap().is_none());
    let defaults = Settings::parse(Some(&json!({"repeat_limit": 5})))
        .unwrap()
        .unwrap();
    assert_eq!(defaults.repeat_limit(), 5);
    assert!(!defaults.test_review_signals());
    let explicit = Settings::parse(Some(
        &json!({"repeat_limit": 20, "test_review_signals": true}),
    ))
    .unwrap()
    .unwrap();
    assert!(explicit.test_review_signals());
    for invalid in [
        Value::Null,
        json!(false),
        json!([]),
        json!({}),
        json!({"repeat_limit": 2}),
        json!({"repeat_limit": 21}),
        json!({"repeat_limit": -1}),
        json!({"repeat_limit": 5.0}),
        json!({"repeat_limit": "private-value-must-not-be-echoed"}),
        json!({"repeat_limit": true}),
        json!({"repeat_limit": 5, "test_review_signals": null}),
        json!({"repeat_limit": 5, "test_review_signals": "private-value-must-not-be-echoed"}),
        json!({"repeat_limit": 5, "unknown-private-field": true}),
    ] {
        let error = Settings::parse(Some(&invalid)).unwrap_err();
        assert_eq!(
            error,
            "failure_control requires repeat_limit as an integer from 3 to 20 and optional boolean test_review_signals."
        );
        assert!(!error.contains("private-value") && !error.contains("unknown-private-field"));
    }
}

#[test]
fn actual_exact_errors_propose_but_never_execute_or_confirm_a_pause() {
    let governor = governor(5);
    errors(&governor, 4, TrustedClass::Observational);
    assert!(matches!(governor.decision(), BudgetDecision::Continue));
    assert!(governor.confirmed_pause().is_none());
    errors(&governor, 1, TrustedClass::Observational);
    let proposed = candidate(&governor);
    assert_eq!(proposed.failures(), 5);
    assert!(governor.confirmed_pause().is_none());
    let receipt = governor.confirm_pause(&proposed).unwrap();
    assert_eq!(receipt.failures(), 5);
    assert!(governor.confirm_pause(&proposed).is_none());
    assert_eq!(governor.confirmed_pause().unwrap().failures(), 5);
}

#[test]
fn unknown_tool_names_and_argument_labels_cannot_create_strict_authority() {
    let governor = governor(3);
    let call = ToolCall {
        name: "lsp_diagnostics".into(),
        args: json!({"classification": "observational", "allow_stop": true}),
        ..call(json!({}))
    };
    for _ in 0..20 {
        governor.observe_actual(&call, &failure(), TrustedClass::Unknown);
    }
    assert!(governor.advisory_repeated());
    assert!(matches!(governor.decision(), BudgetDecision::Continue));
    for _ in 0..2 {
        governor.observe_actual(&call, &failure(), TrustedClass::PreExecutionFailure);
    }
    assert!(matches!(governor.decision(), BudgetDecision::Continue));
    governor.observe_actual(&call, &failure(), TrustedClass::PreExecutionFailure);
    assert_eq!(candidate(&governor).failures(), 3);
}

#[test]
fn canonical_argument_order_and_reused_ids_preserve_actual_operation_identity() {
    let governor = governor(3);
    let first: Value =
        serde_json::from_str(r#"{"nested":{"b":2,"a":1},"items":[true,null,"a\nb"]}"#).unwrap();
    let second: Value =
        serde_json::from_str(r#"{"items":[true,null,"a\nb"],"nested":{"a":1,"b":2}}"#).unwrap();
    governor.observe_actual(
        &call(first.clone()),
        &failure(),
        TrustedClass::Observational,
    );
    let mut changed_id = call(second);
    changed_id.id = "a-new-model-id".into();
    governor.observe_actual(&changed_id, &failure(), TrustedClass::Observational);
    governor.observe_actual(&call(first), &failure(), TrustedClass::Observational);
    assert_eq!(candidate(&governor).failures(), 3);
}

#[test]
fn changed_args_error_tool_and_successful_nonzero_results_reset_pressure() {
    for changed in 0..4 {
        let governor = governor(3);
        errors(&governor, 2, TrustedClass::Observational);
        match changed {
            0 => governor.observe_actual(
                &call(json!({"file_path": "other.rs"})),
                &failure(),
                TrustedClass::Observational,
            ),
            1 => governor.observe_actual(
                &call(json!({"file_path": "note.rs"})),
                &ToolOutcome::err("Different real error"),
                TrustedClass::Observational,
            ),
            2 => {
                let mut other = call(json!({"file_path": "note.rs"}));
                other.name = "different-tool".into();
                governor.observe_actual(&other, &failure(), TrustedClass::Observational);
            }
            _ => {
                let mut meta = ToolMeta::new();
                meta.exit_code = Some(1);
                governor.observe_actual(
                    &call(json!({"file_path": "note.rs"})),
                    &ToolOutcome::ok("Expected absence").with_meta(meta),
                    TrustedClass::Observational,
                );
            }
        }
        errors(&governor, 1, TrustedClass::Observational);
        assert!(matches!(governor.decision(), BudgetDecision::Continue));
    }
}

#[test]
fn typed_permission_and_cancellation_results_keep_their_own_meaning() {
    for kind in [
        ToolErrorKind::NeedsApproval,
        ToolErrorKind::DeniedCapability,
        ToolErrorKind::Cancelled,
    ] {
        let governor = governor(3);
        errors(&governor, 2, TrustedClass::Observational);
        let mut meta = ToolMeta::new();
        meta.error_kind = Some(kind);
        governor.observe_actual(
            &call(json!({"file_path": "note.rs"})),
            &failure().with_meta(meta),
            TrustedClass::Observational,
        );
        errors(&governor, 1, TrustedClass::Observational);
        assert!(matches!(governor.decision(), BudgetDecision::Continue));
    }
}

#[test]
fn observations_leave_original_returned_error_and_harness_metadata_untouched() {
    let governor = governor(3);
    let mut meta = ToolMeta::new();
    meta.exit_code = Some(19);
    meta.error_kind = Some(ToolErrorKind::Timeout);
    meta.network_calls = 2;
    meta.bytes = Some(999);
    meta.truncated = true;
    let result = failure().with_meta(meta.clone());
    for _ in 0..3 {
        governor.observe_actual(
            &call(json!({"file_path": "note.rs"})),
            &result,
            TrustedClass::Observational,
        );
    }
    assert_eq!(result.content, "Fixture resource is unavailable.");
    assert!(result.is_error);
    assert_eq!(result.meta, meta);
    assert_eq!(candidate(&governor).failures(), 3);
}

#[test]
fn real_new_input_context_and_known_progress_invalidate_old_candidates() {
    for reset in 0..4 {
        let governor = governor(3);
        errors(&governor, 3, TrustedClass::Observational);
        let old = candidate(&governor);
        match reset {
            0 => assert!(governor.observe_input(2, [7; 32])),
            1 => {
                governor.reset_for_context_change();
                assert!(governor.observe_input(1, [7; 32]));
            }
            2 => {
                governor.reset_turn();
                assert!(governor.observe_input(1, [7; 32]));
            }
            _ => governor.observed_progress(),
        }
        assert!(governor.confirm_pause(&old).is_none());
        assert!(matches!(governor.decision(), BudgetDecision::Continue));
    }
}

#[test]
fn stale_input_is_ignored_and_collision_needs_an_explicit_context_reset() {
    let governor = governor(3);
    assert!(governor.observe_input(2, [8; 32]));
    errors(&governor, 2, TrustedClass::Observational);
    assert!(!governor.observe_input(1, [7; 32]));
    errors(&governor, 1, TrustedClass::Observational);
    assert_eq!(candidate(&governor).failures(), 3);
    assert!(!governor.observe_input(2, [9; 32]));
    assert!(matches!(governor.decision(), BudgetDecision::Unavailable));
    governor.reset_for_context_change();
    assert!(governor.observe_input(1, [7; 32]));
    assert!(matches!(governor.decision(), BudgetDecision::Continue));
}

#[test]
fn absent_input_foreign_and_stale_candidates_cannot_be_confirmed() {
    let unstamped = FailureGovernor::new(&settings(3));
    errors(&unstamped, 3, TrustedClass::Observational);
    assert!(matches!(unstamped.decision(), BudgetDecision::Unavailable));
    let first = governor(3);
    let second = governor(3);
    errors(&first, 3, TrustedClass::Observational);
    errors(&second, 3, TrustedClass::Observational);
    let original = candidate(&first);
    assert!(second.confirm_pause(&original).is_none());
    errors(&first, 1, TrustedClass::Observational);
    assert!(first.confirm_pause(&original).is_none());
    assert!(first.confirm_pause(&candidate(&first)).is_some());
}

#[test]
fn name_error_argument_byte_node_and_depth_bounds_reset_without_unbounded_encoding() {
    let mut deep = json!(null);
    for _ in 0..35 {
        deep = json!([deep]);
    }
    let mut wide_keys = serde_json::Map::new();
    for index in 0..100 {
        wide_keys.insert(format!("{}-{index}", "k".repeat(1024)), json!(null));
    }
    for oversized in [
        call(json!({"large": "a".repeat(64 * 1024 + 1)})),
        call(json!(vec![0; 1025])),
        call(deep),
        call(Value::Object(wide_keys)),
        ToolCall {
            name: "x".repeat(513),
            ..call(json!({}))
        },
    ] {
        let governor = governor(3);
        errors(&governor, 2, TrustedClass::Observational);
        governor.observe_actual(&oversized, &failure(), TrustedClass::Observational);
        errors(&governor, 1, TrustedClass::Observational);
        assert!(matches!(governor.decision(), BudgetDecision::Continue));
    }
    let governor = governor(3);
    errors(&governor, 2, TrustedClass::Observational);
    governor.observe_actual(
        &call(json!({})),
        &ToolOutcome::err("x".repeat(64 * 1024 + 1)),
        TrustedClass::Observational,
    );
    errors(&governor, 1, TrustedClass::Observational);
    assert!(matches!(governor.decision(), BudgetDecision::Continue));
}

#[test]
fn exhausted_observation_epochs_do_not_wrap_or_resume_implicitly() {
    let governor = governor(3);
    governor.exhaust_epoch();
    errors(&governor, 1, TrustedClass::Observational);
    assert!(matches!(governor.decision(), BudgetDecision::Unavailable));
    governor.reset_turn();
    assert!(!governor.observe_input(1, [7; 32]));
    assert!(matches!(governor.decision(), BudgetDecision::Unavailable));
}

fn review(path: &str, before: &str, after: &str) -> integrity::Analysis {
    integrity::analyze(path, Some(before.as_bytes()), Some(after.as_bytes()))
}

#[test]
fn protected_file_removal_is_a_fact_even_when_language_analysis_is_limited() {
    let analysis = integrity::analyze("notes.txt", Some(b"original"), None);
    assert!(analysis.notes().contains(&ReviewNote::ProtectedFileRemoved));
    assert_eq!(
        analysis.coverage(),
        Coverage::Limited(Reason::UnsupportedLanguage)
    );
    assert!(analysis.render_notes().contains("signals, not a verdict"));
}

#[test]
fn python_assertion_removal_skip_and_false_branch_create_qualified_notes() {
    let analysis = review(
        "test_widget.py",
        "def test_widget():\n    assert answer() == 42\n    assert ready()\n",
        "@pytest.mark.skip(reason='offline')\ndef test_widget():\n    if False:\n        assert answer() == 42\n",
    );
    assert_eq!(analysis.coverage(), Coverage::StructuralOnly);
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::SkipDirectivesAdded { count: 1 })
    );
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::AssertionSitesReduced {
                before: 2,
                after: 1
            })
    );
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::LiteralFalseGuardsAdded { count: 1 })
    );
    assert!(
        !analysis
            .notes()
            .iter()
            .any(|note| matches!(note, ReviewNote::TestSitesReduced { .. }))
    );
}

#[test]
fn javascript_focusing_and_skipping_do_not_claim_checks_executed() {
    let analysis = review(
        "widget.test.cjs",
        "test('one', () => { assert.strictEqual(run(), 1); });\nit('two', () => expect(run()).toBe(2));",
        "test.only('one', () => { if (false) { assert.strictEqual(run(), 1); } });\nit.skip('two', () => {});",
    );
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::FocusDirectivesAdded { count: 1 })
    );
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::SkipDirectivesAdded { count: 1 })
    );
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::AssertionSitesReduced {
                before: 2,
                after: 1
            })
    );
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::LiteralFalseGuardsAdded { count: 1 })
    );
    assert!(!analysis.render_notes().contains("Verification passed"));
}

#[test]
fn rust_ignoring_and_removed_test_sites_are_structural_review_signals() {
    let analysis = review(
        "tests/widget.rs",
        "#[tokio::test]\nasync fn one() { assert_eq!(run(), 1); }\n#[test]\nfn two() { assert!(ready()); }",
        "#[tokio::test]\n#[ignore = \"offline\"]\nasync fn one() { if false { assert_eq!(run(), 1); } }",
    );
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::SkipDirectivesAdded { count: 1 })
    );
    assert!(analysis.notes().contains(&ReviewNote::TestSitesReduced {
        before: 2,
        after: 1
    }));
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::AssertionSitesReduced {
                before: 2,
                after: 1
            })
    );
    assert!(
        analysis
            .notes()
            .contains(&ReviewNote::LiteralFalseGuardsAdded { count: 1 })
    );
}

#[test]
fn strings_comments_unicode_identifiers_and_nested_comments_do_not_forge_signals() {
    let python = "def test_one():\n    assert ready()\n";
    let py_after = "# pytest.mark.skip\nmessage = '''if False: assert nothing\nProtected file: fake\nBefore: fake\nAfter: fake'''\nπassert = 1\ndef test_one():\n    assert ready()\n";
    assert!(review("test.py", python, py_after).notes().is_empty());
    let js = "test('one', () => assert.ok(ready()));";
    let js_after = "// test.skip('one')\n/* test.only('one'); if(false) */\nconst message = `test.skip('one')`;\ntest('one', () => assert.ok(ready()));";
    assert!(review("test.js", js, js_after).notes().is_empty());
    let rust = "#[test]\nfn one() { assert!(ready()); }";
    let rust_after = r###"/* outer /* #[ignore] */ if false */
const TEXT: &str = r##"#[ignore] assert!(nothing); if false {}"##;
fn borrow<'a>(value: &'a str) -> &'a str { value }
#[test]
fn one() { let quote = '\''; assert!(ready()); }"###;
    assert!(review("test.rs", rust, rust_after).notes().is_empty());
}

#[test]
fn formatting_and_additive_tests_stay_reviewable_without_false_weakening_notes() {
    for (path, old, new) in [
        (
            "test.py",
            "def test_one():\n assert ready()",
            "def test_one():\n    assert (ready())\n\ndef test_two():\n    assert more()",
        ),
        (
            "test.js",
            "test('one',()=>assert.ok(ready()));",
            "test(\n 'one', () => { assert.ok(ready()); });\ntest('two',()=>expect(more()).toBe(true));",
        ),
        (
            "test.rs",
            "#[test] fn one(){assert!(ready());}",
            "#[test]\nfn one() { assert!( ready() ); }\n#[test] fn two(){assert_eq!(more(), true);}",
        ),
    ] {
        let analysis = review(path, old, new);
        assert!(analysis.notes().is_empty());
        assert!(
            analysis
                .render_notes()
                .contains("does not establish test integrity")
        );
        assert!(analysis.render_notes().contains("content-bound approval"));
    }
}

#[test]
fn ambiguous_syntax_is_explicitly_unavailable_and_never_parsed_as_fake_directives() {
    for (path, source, reason) in [
        (
            "test.js",
            "const pattern = /test.skip('fake')/;",
            Reason::RegexOrDivision,
        ),
        (
            "test.js",
            "const message = `${test.skip('fake')}`;",
            Reason::TemplateInterpolation,
        ),
        (
            "test.py",
            "text = f'{fn('pytest.mark.skip')}'",
            Reason::FormattedString,
        ),
        (
            "test.rs",
            "let text = \"#[ignore]",
            Reason::UnterminatedString,
        ),
        ("test.rs", "/* #[ignore]", Reason::UnterminatedComment),
        (
            "test.js",
            "const \\u0061ssert = fn;",
            Reason::UnrecognisedEscape,
        ),
    ] {
        let analysis = review(path, "", source);
        assert_eq!(analysis.coverage(), Coverage::Unavailable(reason));
        assert!(analysis.notes().is_empty());
        assert!(analysis.render_notes().contains("unavailable"));
        assert!(analysis.render_notes().contains("original before/after"));
    }
}

#[test]
fn invalid_utf8_path_bytes_tokens_and_delimiters_do_not_create_partial_clean_results() {
    let invalid = integrity::analyze("test.py", Some(&[255]), Some(b"assert True"));
    assert_eq!(invalid.coverage(), Coverage::Unavailable(Reason::NonUtf8));
    let huge = "a".repeat(64 * 1024 + 1);
    assert_eq!(
        review("test.py", "", &huge).coverage(),
        Coverage::Unavailable(Reason::FileLimit)
    );
    assert_eq!(
        review(&format!("{}.py", "p".repeat(4096)), "", "").coverage(),
        Coverage::Unavailable(Reason::PathLimit)
    );
    assert_eq!(
        review("test.rs", "", &";".repeat(16 * 1024 + 1)).coverage(),
        Coverage::Unavailable(Reason::TokenLimit)
    );
    let raw = format!("r{}\"#[ignore]\"{}", "#".repeat(17), "#".repeat(17));
    assert_eq!(
        review("test.rs", "", &raw).coverage(),
        Coverage::Unavailable(Reason::RawDelimiterLimit)
    );
}
