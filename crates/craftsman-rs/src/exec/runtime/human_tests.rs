use super::*;
use crate::exec::construction::{Options, SessionSelection};
use crate::exec::input::{Invocation, Run, Sources};
use crate::exec::permissions::{OnUnapproved, Selectors};
use crate::exec::provider::request::Fault;
use crate::exec::terminal::{Error, Facts, Human, ReturnKind, Terminal};
use craftsman_harness::{FileData, Msg, ToolCall, ToolErrorKind};
use serde_json::json;
use std::path::PathBuf;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEED: &str = "human-runtime-secret";
const CAP: usize = 1024 * 1024;
const RETURNED: &str =
    "craftsman: private run return=returned; approval_required=false; final_answer=present\n";
struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    server: MockServer,
}
impl Fixture {
    async fn new(seed: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("selected-config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        let f = Self {
            _root: root,
            workspace,
            config,
            server: MockServer::start().await,
        };
        f.configure(&[seed]);
        f
    }
    fn configure(&self, seeds: &[&str]) {
        // Disabled MCP sources are captured by the actual builder, never sent
        // as HTTP auth or loaded. This also tests literal newline/control seeds
        // without invalid provider headers or ambient environment mutation.
        let headers: serde_json::Map<String, serde_json::Value> = seeds
            .iter()
            .enumerate()
            .map(|(n, seed)| (format!("X-Fixture-{n}"), json!(seed)))
            .collect();
        std::fs::write(
            self.config.join("config.json"),
            json!({"model":"synthetic-model","api_key":SEED,"base_url":self.server.uri(),
                "mcp_servers":[{"name":"disabled-fixture","url":"http://127.0.0.1:9","headers":headers}]})
            .to_string(),
        )
        .unwrap();
    }
    fn options(&self, allow: &[&str]) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session: SessionSelection::Ephemeral,
            selectors: Selectors {
                on_unapproved: OnUnapproved::Fail,
                allow: allow.iter().map(|n| format!("tool:{n}")).collect(),
                ..Default::default()
            },
        }
    }
    async fn response(&self, content: &str) {
        let data = json!({"choices":[{"delta":{"content":content},"finish_reason":"stop"}]});
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {data}\n\ndata: [DONE]\n\n").into_bytes(),
                "text/event-stream",
            ))
            .mount(&self.server)
            .await;
    }
    async fn approval_response(&self) {
        let data = json!({"choices":[{"delta":{"content":format!("actual partial {SEED}\u{001b}[31m"),"tool_calls":[{"index":0,"id":"stop","type":"function","function":{"name":"execute","arguments":json!({"command":"must-not-execute"}).to_string()}},{"index":1,"id":"later","type":"function","function":{"name":"write_file","arguments":json!({"file_path":"later.txt","content":SEED}).to_string()}}]},"finish_reason":"tool_calls"}]});
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!("data: {data}\n\ndata: [DONE]\n\n").into_bytes(),
                "text/event-stream",
            ))
            .mount(&self.server)
            .await;
    }
    async fn run(&self, allow: &[&str]) -> Run {
        Invocation::prepare(
            self.options(allow),
            Sources {
                prompt: Some(b"synthetic instruction"),
                prompt_file: None,
                files: &[],
            },
        )
        .await
        .unwrap()
        .run()
        .await
    }
    async fn returned(&self, content: &str) -> Run {
        self.response(content).await;
        let r = self.run(&[]).await;
        assert!(r.outcome().is_ok());
        r
    }
    fn no_store(&self) {
        assert!(!self.config.join("threads.db").exists());
        assert!(!self.config.join("sessions-rs").exists());
    }
}
fn state(run: &Run) -> Arc<std::sync::Mutex<AgentState>> {
    run.runtime_for_tests().session.agent.state.clone()
}
fn gate(run: &Run) -> Arc<InvocationGate> {
    run.runtime_for_tests().gate.clone()
}
fn total(view: &Human<'_>) -> usize {
    view.answer().unwrap().map_or(0, <[u8]>::len) + view.diagnostic().unwrap().len()
}
fn checked(t: &Terminal, max: usize) -> (Option<String>, String, Facts) {
    let h = t.human(max).unwrap();
    assert!(h.bind(&t.scope().unwrap()).is_ok());
    assert!(total(&h) <= max && total(&h) <= CAP);
    for bytes in [
        h.answer().unwrap().unwrap_or_default(),
        h.diagnostic().unwrap(),
    ] {
        if !bytes.is_empty() {
            assert_eq!(bytes.last(), Some(&b'\n'));
        }
        let text = std::str::from_utf8(bytes).unwrap();
        assert!(text.chars().all(|c| !c.is_control() || c == '\n'));
        assert!(!text.contains("::") && !text.contains("##"));
        assert!(!text.chars().any(|c| matches!(c,
            '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}' | '\u{feff}')));
        assert!(!text.contains(SEED));
    }
    (
        h.answer()
            .unwrap()
            .map(|b| std::str::from_utf8(b).unwrap().to_owned()),
        std::str::from_utf8(h.diagnostic().unwrap())
            .unwrap()
            .to_owned(),
        h.facts().unwrap(),
    )
}
fn answer(t: &Terminal) -> String {
    checked(t, CAP).0.unwrap()
}
fn error(t: &Terminal, max: usize, fault: Fault) {
    assert_eq!(t.human(max).err(), Some(Error::Codec(fault)));
}

#[tokio::test]
async fn ordinary_answer_is_final_text_only_with_separate_typed_private_diagnostic() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("actual final answer").await;
    let t = run.terminal();
    let (answer, diagnostic, facts) = checked(&t, CAP);
    assert_eq!(answer.as_deref(), Some("actual final answer\n"));
    assert_eq!(diagnostic, RETURNED);
    assert_eq!(facts.returned(), ReturnKind::Returned);
    assert!(!facts.approval_required());
    f.no_store();
}
#[tokio::test]
async fn unicode_scripts_emoji_joiners_and_normalization_remain_original_data() {
    let f = Fixture::new(SEED).await;
    let text = "中文 é e\u{0301} Å A\u{030a} 🦀👩\u{200d}💻 क्\u{200c}ष";
    let run = f.returned(text).await;
    assert_eq!(answer(&run.terminal()), format!("{text}\n"));
}
#[tokio::test]
async fn all_c0_controls_except_lf_are_visible_lowercase_scalar_escapes() {
    let f = Fixture::new(SEED).await;
    let text: String = (0..=31).map(|n| char::from_u32(n).unwrap()).collect();
    let expected: String = (0..=31)
        .map(|n| {
            if n == 10 {
                "\n".into()
            } else {
                format!("\\u{{{n:04x}}}")
            }
        })
        .collect();
    let run = f.returned(&text).await;
    assert_eq!(answer(&run.terminal()), format!("{expected}\n"));
}
#[tokio::test]
async fn del_and_all_c1_controls_are_visible_not_terminal_sequences() {
    let f = Fixture::new(SEED).await;
    let text: String = (127..=159).map(|n| char::from_u32(n).unwrap()).collect();
    let expected: String = (127..=159).map(|n| format!("\\u{{{n:04x}}}")).collect();
    let run = f.returned(&text).await;
    assert_eq!(answer(&run.terminal()), format!("{expected}\n"));
}
#[tokio::test]
async fn osc_csi_hyperlinks_bell_and_overwrite_cr_cannot_reach_the_terminal() {
    let f = Fixture::new(SEED).await;
    let run = f
        .returned("\u{001b}[31mred\u{001b}]8;;https://example.invalid\u{0007}link\rrewrite")
        .await;
    assert_eq!(
        answer(&run.terminal()),
        "\\u{001b}[31mred\\u{001b}]8;;https://example.invalid\\u{0007}link\\u{000d}rewrite\n"
    );
}
#[tokio::test]
async fn bidi_marks_embeddings_overrides_and_isolates_are_not_invisible_controls() {
    let f = Fixture::new(SEED).await;
    let chars = [
        0x061c, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066, 0x2067, 0x2068,
        0x2069,
    ];
    let text: String = chars.iter().map(|n| char::from_u32(*n).unwrap()).collect();
    let expected: String = chars.iter().map(|n| format!("\\u{{{n:04x}}}")).collect();
    let run = f.returned(&text).await;
    assert_eq!(answer(&run.terminal()), format!("{expected}\n"));
}
#[tokio::test]
async fn data_bom_and_unicode_line_paragraph_separators_are_visible_not_physical_lines() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("\u{feff}a\u{2028}b\u{2029}c").await;
    assert_eq!(answer(&run.terminal()), "\\u{feff}a\\u{2028}b\\u{2029}c\n");
}
#[tokio::test]
async fn literal_backslash_u_ansi_and_surrogate_text_is_not_decoded_or_repaired() {
    let f = Fixture::new(SEED).await;
    let run = f
        .returned(r"literal \u{001b} \u001b \ud800 \n \033[31m")
        .await;
    assert_eq!(
        answer(&run.terminal()),
        "literal \\\\u{001b} \\\\u001b \\\\ud800 \\\\n \\\\033[31m\n"
    );
}
#[tokio::test]
async fn lf_blank_lines_spaces_and_existing_final_lf_stay_exact() {
    let f = Fixture::new(SEED).await;
    let text = "  first  \n\n second \n";
    let run = f.returned(text).await;
    assert_eq!(answer(&run.terminal()), text);
}
#[tokio::test]
async fn crlf_keeps_lf_but_renders_cr_tab_and_vertical_tab_visibly() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("first\r\n\tsecond\u{000b}\r\n").await;
    assert_eq!(
        answer(&run.terminal()),
        "first\\u{000d}\n\\u{0009}second\\u{000b}\\u{000d}\n"
    );
}
#[tokio::test]
async fn github_runner_commands_are_defanged_on_every_line_and_after_leading_spaces() {
    let f = Fixture::new(SEED).await;
    let run = f
        .returned("::warning::data\n   ::stop-commands::token\n::token::")
        .await;
    assert_eq!(
        answer(&run.terminal()),
        ":\\u{003a}warning:\\u{003a}data\n   :\\u{003a}stop-commands:\\u{003a}token\n:\\u{003a}token:\\u{003a}\n"
    );
}
#[tokio::test]
async fn azure_logging_commands_are_defanged_even_embedded_in_a_line() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("prefix ##vso[task.setvariable variable=x]value\n##vso[task.complete result=Succeeded;]").await;
    assert_eq!(
        answer(&run.terminal()),
        "prefix #\\u{0023}vso[task.setvariable variable=x]value\n#\\u{0023}vso[task.complete result=Succeeded;]\n"
    );
}
#[tokio::test]
async fn hash_delimiters_also_cover_legacy_formatting_and_other_hash_runner_syntax() {
    let f = Fixture::new(SEED).await;
    let run = f
        .returned("##[warning]data\n##teamcity[message text='data']\n### heading")
        .await;
    assert_eq!(
        answer(&run.terminal()),
        "#\\u{0023}[warning]data\n#\\u{0023}teamcity[message text='data']\n#\\u{0023}\\u{0023} heading\n"
    );
}
#[tokio::test]
async fn overlapping_and_long_colon_hash_runs_never_leave_a_pair_at_a_replacement_join() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("::: #### :::::: #####").await;
    assert_eq!(
        answer(&run.terminal()),
        ":\\u{003a}\\u{003a} #\\u{0023}\\u{0023}\\u{0023} :\\u{003a}\\u{003a}\\u{003a}\\u{003a}\\u{003a} #\\u{0023}\\u{0023}\\u{0023}\\u{0023}\n"
    );
}
#[tokio::test]
async fn isolated_colons_hashes_urls_and_percent_escapes_are_not_parsed_as_commands() {
    let f = Fixture::new(SEED).await;
    let text = "# item a:b https://example.invalid/#x %0A%0D%3A%23";
    let run = f.returned(text).await;
    assert_eq!(answer(&run.terminal()), format!("{text}\n"));
}
#[tokio::test]
async fn command_delimiters_on_both_sides_of_escaped_controls_stay_data() {
    let f = Fixture::new(SEED).await;
    let run = f
        .returned("::warning::one\r::warning::two\u{009b}##vso[x]three")
        .await;
    assert_eq!(
        answer(&run.terminal()),
        ":\\u{003a}warning:\\u{003a}one\\u{000d}:\\u{003a}warning:\\u{003a}two\\u{009b}#\\u{0023}vso[x]three\n"
    );
}
#[tokio::test]
async fn source_credentials_are_redacted_before_control_and_runner_projection() {
    let f = Fixture::new(SEED).await;
    let run = f.returned(&format!("{SEED}\u{001b}[31m\n::{SEED}::")).await;
    assert_eq!(
        answer(&run.terminal()),
        "[REDACTED]\\u{001b}[31m\n:\\u{003a}[REDACTED]:\\u{003a}\n"
    );
}
#[tokio::test]
async fn configured_disabled_source_seeds_are_frozen_without_loading_mcp_or_reloading_config() {
    let f = Fixture::new("first-secret").await;
    f.configure(&["first-secret", "second-secret"]);
    let run = f.returned("first-secret second-secret later-key").await;
    f.configure(&["later-key"]);
    let t = run.terminal();
    assert_eq!(answer(&t), "[REDACTED] [REDACTED] later-key\n");
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}
#[tokio::test]
async fn overlapping_original_seeds_use_the_unchanged_full_coverage_redactor() {
    let f = Fixture::new("abc").await;
    f.configure(&["abc", "bcde"]);
    let run = f.returned("abcde::tail").await;
    assert_eq!(answer(&run.terminal()), "[REDACTED]:\\u{003a}tail\n");
}
#[tokio::test]
async fn redaction_marker_joins_are_checked_before_display_not_silently_rewritten_again() {
    let f = Fixture::new("hidden").await;
    f.configure(&["hidden", "prefix[REDACTED]suffix"]);
    let run = f.returned("prefixhiddensuffix").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn newly_generated_control_escape_credential_refuses_the_whole_display() {
    let f = Fixture::new(r"\u{001b}").await;
    let run = f.returned("safe then \u{001b}").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn newly_generated_bidi_escape_credential_refuses_without_alternative_spelling() {
    let f = Fixture::new(r"\u{202e}").await;
    let run = f.returned("safe then \u{202e}").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn doubled_original_backslash_can_create_a_credential_and_must_fail_fixed() {
    let f = Fixture::new(r"\\private").await;
    let run = f.returned(r"\private").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn colon_defanging_escape_collision_is_not_rescued_by_different_defanging() {
    let f = Fixture::new(r":\u{003a}").await;
    let run = f.returned("::warning::data").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn hash_defanging_escape_collision_is_not_rescued_by_a_line_prefix() {
    let f = Fixture::new(r"#\u{0023}").await;
    let run = f.returned("embedded ##vso[task.complete]").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn generated_literal_match_spanning_64k_verification_chunks_is_detected() {
    let f = Fixture::new(r"\u{001b}").await;
    let text = "a".repeat(65_536 - 4) + "\u{001b}";
    let run = f.returned(&text).await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn original_literal_secret_spanning_redactor_chunks_is_covered_completely() {
    let f = Fixture::new(SEED).await;
    let text = "a".repeat(65_536 - 5) + SEED + "tail";
    let run = f.returned(&text).await;
    assert_eq!(
        answer(&run.terminal()),
        "a".repeat(65_536 - 5) + "[REDACTED]tail\n"
    );
}
#[tokio::test]
async fn fixed_private_return_diagnostic_collision_preserves_facts_or_refuses() {
    let f = Fixture::new("return=returned").await;
    let run = f.returned("done").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn fixed_typed_approval_diagnostic_collision_never_redacts_a_false_fact() {
    let f = Fixture::new("approval_required=false").await;
    let run = f.returned("done").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn fixed_answer_presence_diagnostic_collision_has_no_silent_diagnostic_omission() {
    let f = Fixture::new("final_answer=present").await;
    let run = f.returned("done").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn newly_added_final_lf_literal_collision_has_no_unterminated_fallback() {
    let f = Fixture::new("done\n").await;
    let run = f.returned("done").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn final_answer_then_diagnostic_join_is_verified_with_the_same_stream() {
    let f = Fixture::new("done\ncraftsman: private run").await;
    let run = f.returned("done").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn diagnostic_then_final_answer_join_is_also_verified_without_claiming_sink_order() {
    let f = Fixture::new("final_answer=present\ndone").await;
    let run = f.returned("done").await;
    error(&run.terminal(), CAP, Fault::Collision);
}
#[tokio::test]
async fn exact_budget_includes_utf8_defanging_backslashes_both_channels_and_lf() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("中::\\\u{001b}").await;
    let t = run.terminal();
    let expected = "中:\\u{003a}\\\\\\u{001b}\n";
    let exact = expected.len() + RETURNED.len();
    assert_eq!(checked(&t, exact).0.as_deref(), Some(expected));
    assert_eq!(total(&t.human(exact).unwrap()), exact);
    error(&t, exact - 1, Fault::Limit);
}
#[tokio::test]
async fn full_answer_cannot_be_truncated_to_fit_a_tighter_display_budget() {
    let f = Fixture::new(SEED).await;
    let run = f.returned(&"answer".repeat(100)).await;
    let t = run.terminal();
    error(&t, RETURNED.len(), Fault::Limit);
    assert_eq!(answer(&t), "answer".repeat(100) + "\n");
}
#[tokio::test]
async fn genuine_empty_answer_is_present_and_lf_terminated_not_missing_failure_output() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("").await;
    let t = run.terminal();
    let (answer, diagnostic, facts) = checked(&t, RETURNED.len() + 1);
    assert_eq!(answer.as_deref(), Some("\n"));
    assert_eq!(diagnostic, RETURNED);
    assert_eq!(facts.returned(), ReturnKind::Returned);
    error(&t, RETURNED.len(), Fault::Limit);
}
#[tokio::test]
async fn actual_failed_return_has_absent_answer_and_only_a_fixed_safe_diagnostic() {
    let f = Fixture::new(SEED).await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_raw(
            format!("raw failure {SEED}\u{001b}[31m\n##vso[task.complete]"),
            "text/plain",
        ))
        .mount(&f.server)
        .await;
    let run = f.run(&[]).await;
    assert!(matches!(run.outcome(), Err(RuntimeError::Turn(_))));
    let t = run.terminal();
    let (answer, diagnostic, facts) = checked(&t, CAP);
    assert!(answer.is_none());
    assert_eq!(
        diagnostic,
        "craftsman: private run return=turn_failed; approval_required=false; final_answer=absent\n"
    );
    assert_eq!(facts.returned(), ReturnKind::TurnFailed);
    assert!(!diagnostic.contains("raw failure"));
    let exact = diagnostic.len();
    assert_eq!(total(&t.human(exact).unwrap()), exact);
    error(&t, exact - 1, Fault::Limit);
    f.no_store();
}
#[tokio::test]
async fn actual_approval_stop_keeps_partial_state_private_and_never_reopens_later_dispatch() {
    let f = Fixture::new(SEED).await;
    f.approval_response().await;
    let run = f.run(&["write_file"]).await;
    assert!(matches!(run.outcome(), Err(RuntimeError::ApprovalRequired)));
    let g = gate(&run);
    let t = run.terminal();
    let (answer, diagnostic, facts) = checked(&t, CAP);
    assert!(answer.is_none());
    assert_eq!(facts.returned(), ReturnKind::ApprovalRequired);
    assert!(facts.approval_required());
    assert_eq!(
        diagnostic,
        "craftsman: private run return=approval_required; approval_required=true; final_answer=absent\n"
    );
    assert!(g.execution_closed() && g.approval_required());
    assert!(!f.workspace.join("later.txt").exists());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
    f.no_store();
}
#[tokio::test]
async fn final_answer_phrases_are_not_task_correctness_public_outcome_or_approval_authority() {
    let f = Fixture::new(SEED).await;
    let run = f
        .returned("failed refused NeedsApproval success exit=0")
        .await;
    let t = run.terminal();
    let (answer, diagnostic, facts) = checked(&t, CAP);
    assert_eq!(
        answer.as_deref(),
        Some("failed refused NeedsApproval success exit=0\n")
    );
    assert_eq!(diagnostic, RETURNED);
    assert_eq!(facts.returned(), ReturnKind::Returned);
    assert!(!facts.approval_required());
}
#[tokio::test]
async fn only_actual_immutable_final_output_is_rendered_not_last_message_or_other_state() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("actual answer").await;
    let shared = state(&run);
    {
        let mut s = shared.lock().unwrap();
        s.messages = vec![Msg::Assistant {
            content: "different last raw state".into(),
            tool_calls: vec![ToolCall {
                id: "original".into(),
                name: "example".into(),
                args: json!({"path":SEED,"data":"##vso[task.complete]"}),
            }],
        }];
        s.files.insert("private".into(), FileData::text(SEED));
        s.custom.insert("origin".into(), json!({"data":SEED}));
        s.system_extras.push(SEED.into());
    }
    let original = shared.lock().unwrap().clone();
    let t = run.terminal();
    assert_eq!(answer(&t), "actual answer\n");
    let after = shared.lock().unwrap();
    assert_eq!(after.messages, original.messages);
    assert_eq!(after.files, original.files);
    assert_eq!(after.custom, original.custom);
    assert_eq!(after.system_extras, original.system_extras);
    f.no_store();
}
#[tokio::test]
async fn answer_only_display_does_not_admit_copy_or_claim_a_safe_retained_transcript() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("actual answer").await;
    state(&run).lock().unwrap().messages = vec![Msg::user(SEED); 257];
    let t = run.terminal();
    assert_eq!(answer(&t), "actual answer\n");
    assert_eq!(t.ascii(CAP).err(), Some(Error::Codec(Fault::Limit)));
    let h = t.human(CAP).unwrap();
    assert!(
        !std::str::from_utf8(h.diagnostic().unwrap())
            .unwrap()
            .contains("messages")
    );
}
#[tokio::test]
async fn immutable_rendered_channels_do_not_follow_later_state_or_config_changes() {
    let f = Fixture::new("first-secret").await;
    let run = f.returned("first-secret later-key").await;
    let shared = state(&run);
    let t = run.terminal();
    let h = t.human(CAP).unwrap();
    let bytes = h.answer().unwrap().unwrap().to_vec();
    let diagnostic = h.diagnostic().unwrap().to_vec();
    shared.lock().unwrap().messages = vec![Msg::assistant("new raw last")];
    f.configure(&["later-key"]);
    assert_eq!(h.answer().unwrap(), Some(bytes.as_slice()));
    assert_eq!(h.diagnostic().unwrap(), diagnostic);
    assert_eq!(answer(&t), "[REDACTED] later-key\n");
}
#[tokio::test]
async fn foreign_equal_terminal_owners_cannot_bind_each_others_human_channels() {
    let f = Fixture::new(SEED).await;
    f.response("same").await;
    let one = f.run(&[]).await.terminal();
    let two = f.run(&[]).await.terminal();
    let h = one.human(CAP).unwrap();
    assert!(h.bind(&one.scope().unwrap()).is_ok());
    assert_eq!(h.bind(&two.scope().unwrap()), Err(Error::Foreign));
}
#[tokio::test]
async fn late_actual_approval_supersedes_every_old_read_and_fresh_keeps_original_return() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("actual answer").await;
    let g = gate(&run);
    let t = run.terminal();
    let h = t.human(CAP).unwrap();
    let scope = t.scope().unwrap();
    g.observe_error_kind(ToolErrorKind::NeedsApproval);
    assert_eq!(h.answer().err(), Some(Error::Superseded));
    assert_eq!(h.diagnostic().err(), Some(Error::Superseded));
    assert_eq!(h.facts().err(), Some(Error::Superseded));
    assert_eq!(h.bind(&scope), Err(Error::Superseded));
    let (answer, diagnostic, facts) = checked(&t, CAP);
    assert_eq!(answer.as_deref(), Some("actual answer\n"));
    assert_eq!(facts.returned(), ReturnKind::Returned);
    assert!(facts.approval_required());
    assert_eq!(
        diagnostic,
        "craftsman: private run return=returned; approval_required=true; final_answer=present\n"
    );
    assert!(g.execution_closed());
}
#[tokio::test]
async fn publication_revocation_hides_every_old_read_binding_and_new_capture() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("answer").await;
    let t = run.terminal();
    let h = t.human(CAP).unwrap();
    let scope = t.scope().unwrap();
    t.close();
    assert_eq!(h.answer().err(), Some(Error::Closed));
    assert_eq!(h.diagnostic().err(), Some(Error::Closed));
    assert_eq!(h.facts().err(), Some(Error::Closed));
    assert_eq!(h.bind(&scope), Err(Error::Closed));
    assert_eq!(t.human(CAP).err(), Some(Error::Closed));
}
#[tokio::test]
async fn after_work_actual_epoch_change_overrides_success_collision_and_budget_errors() {
    for (seed, text, max) in [
        (SEED, "answer", CAP),
        (r"\u{001b}", "\u{001b}", CAP),
        (SEED, "answer", 0),
    ] {
        let f = Fixture::new(seed).await;
        let run = f.returned(text).await;
        let g = gate(&run);
        let t = run.terminal();
        assert_eq!(
            t.human_after_work_for_tests(max, || g
                .observe_error_kind(ToolErrorKind::NeedsApproval))
                .err(),
            Some(Error::Superseded)
        );
    }
}
#[tokio::test]
async fn after_work_actual_revocation_overrides_success_collision_and_budget_errors() {
    for (seed, text, max) in [
        (SEED, "answer", CAP),
        (r"\u{001b}", "\u{001b}", CAP),
        (SEED, "answer", 0),
    ] {
        let f = Fixture::new(seed).await;
        let run = f.returned(text).await;
        let t = run.terminal();
        assert_eq!(
            t.human_after_work_for_tests(max, || t.close()).err(),
            Some(Error::Closed)
        );
    }
}
#[tokio::test]
async fn caller_budgets_only_tighten_the_complete_human_cap() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("answer").await;
    let t = run.terminal();
    for max in [0, CAP + 1, usize::MAX] {
        error(&t, max, Fault::Limit);
    }
    assert_eq!(answer(&t), "answer\n");
}
#[tokio::test]
async fn full_1mib_rendered_cap_is_exact_and_one_extra_source_byte_cannot_be_cut() {
    let f = Fixture::new(SEED).await;
    let count = (CAP - RETURNED.len() + 6) / 8;
    let tail = CAP - RETURNED.len() - (8 * count - 6);
    let text = ":".repeat(count) + &"x".repeat(tail);
    let run = f.returned(&text).await;
    let t = run.terminal();
    let h = t.human(CAP).unwrap();
    assert_eq!(total(&h), CAP);
    assert_eq!(h.answer().unwrap().unwrap().len(), CAP - RETURNED.len());
    error(&t, CAP - 1, Fault::Limit);
    let f = Fixture::new(SEED).await;
    let run = f.returned(&(text + "x")).await;
    error(&run.terminal(), CAP, Fault::Limit);
}
#[tokio::test]
async fn oversized_original_and_original_plus_view_work_refuse_before_rendering() {
    for len in [CAP + 1, 400_000] {
        let f = Fixture::new(SEED).await;
        let run = f.returned(&"x".repeat(len)).await;
        error(&run.terminal(), CAP, Fault::Limit);
    }
}
#[tokio::test]
async fn shrinking_many_original_secrets_cannot_rescue_unadmitted_source_work() {
    let f = Fixture::new(SEED).await;
    let run = f.returned(&SEED.repeat(40_000)).await;
    error(&run.terminal(), CAP, Fault::Limit);
}
#[tokio::test]
async fn redacted_view_expansion_counts_toward_the_same_original_work_budget() {
    let f = Fixture::new("qz").await;
    let run = f.returned(&"qz-".repeat(80_000)).await;
    error(&run.terminal(), CAP, Fault::Limit);
}
#[tokio::test]
async fn all_bmp_scalars_are_finite_valid_utf8_and_only_the_documented_set_is_escaped() {
    let f = Fixture::new(SEED).await;
    let text: String = (0..=0xffff).filter_map(char::from_u32).collect();
    let run = f.returned(&text).await;
    let t = run.terminal();
    let display = answer(&t);
    let mut expected = String::new();
    for ch in text.chars() {
        if matches!(ch, '\u{0000}'..='\u{0009}' | '\u{000b}'..='\u{001f}'
            | '\u{007f}'..='\u{009f}' | '\u{061c}' | '\u{200e}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
        {
            expected.push_str(&format!("\\u{{{:04x}}}", ch as u32));
        } else if ch == '\\' {
            expected.push_str("\\\\");
        } else {
            expected.push(ch);
        }
    }
    expected.push('\n');
    assert_eq!(display, expected);
    assert!(display.contains('中') && display.contains('文'));
    assert!(display.contains('é') && display.contains('\u{200d}'));
    assert!(display.contains(r"\u{001b}") && display.contains(r"\u{feff}"));
    assert_eq!(display.chars().filter(|c| *c == '\n').count(), 2);
}
#[tokio::test]
async fn supplementary_noncharacters_and_replacement_data_are_not_lossily_rewritten() {
    let f = Fixture::new(SEED).await;
    let text = "🦀\u{10000}\u{10ffff}\u{fffd}\u{fffe}";
    let run = f.returned(text).await;
    assert_eq!(answer(&run.terminal()), format!("{text}\n"));
}
#[tokio::test]
async fn human_projection_never_reopens_compaction_or_adds_provider_requests() {
    let f = Fixture::new(SEED).await;
    let run = f.returned("answer").await;
    let g = gate(&run);
    let compaction = run.runtime_for_tests().session.compaction.clone();
    let shared = state(&run);
    let t = run.terminal();
    assert_eq!(answer(&t), "answer\n");
    let original = shared.lock().unwrap().messages.clone();
    assert!(compaction.compact_now(&original).await.is_err());
    assert!(g.execution_closed());
    assert_eq!(f.server.received_requests().await.unwrap().len(), 1);
}

// Independent syntax recognizers from the pinned primary parser references in
// terminal-human.md, not production sanitizer calls or a live runner claim.
// Properties/data decoding cannot rescue a missing raw opening delimiter.
fn github_name(info: &str) -> bool {
    let name = info.split(' ').next().unwrap_or_default();
    matches!(
        name,
        "warning"
            | "error"
            | "notice"
            | "set-output"
            | "add-mask"
            | "stop-commands"
            | "group"
            | "endgroup"
            | "token"
    )
}
fn github_v2(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("::") else {
        return false;
    };
    rest.split_once("::")
        .is_some_and(|(info, _)| github_name(info))
}
fn github_v1(line: &str) -> bool {
    let Some((_, rest)) = line.split_once("##[") else {
        return false;
    };
    rest.split_once(']')
        .is_some_and(|(info, _)| github_name(info))
}
fn azure_command(line: &str) -> bool {
    let Some((_, rest)) = line.split_once("##vso[") else {
        return false;
    };
    let Some((info, _)) = rest.split_once(']') else {
        return false;
    };
    let name = info.split(' ').next().unwrap_or_default();
    name.split('.').filter(|s| !s.is_empty()).count() == 2
}
#[tokio::test]
async fn independent_github_v2_recognizer_accepts_originals_and_rejects_every_rendered_line() {
    let lines = [
        "::warning::data",
        "  ::error file=x,line=2::data",
        "\t::notice::data",
        "::set-output name=x::%0A%3A",
        "::add-mask::value",
        "::stop-commands::token",
        "::token::",
    ];
    assert!(lines.iter().all(|line| github_v2(line)));
    let f = Fixture::new(SEED).await;
    let run = f.returned(&lines.join("\n")).await;
    let display = answer(&run.terminal());
    assert!(display.lines().all(|line| !github_v2(line)));
    assert_eq!(display.lines().count(), lines.len());
}
#[tokio::test]
async fn independent_github_legacy_recognizer_finds_embedded_originals_not_rendered_prefixes() {
    let lines = [
        "##[warning]data",
        "prefix ##[error file=x;line=2]data",
        " ##[notice]data",
    ];
    assert!(lines.iter().all(|line| github_v1(line)));
    let f = Fixture::new(SEED).await;
    let run = f.returned(&lines.join("\n")).await;
    let display = answer(&run.terminal());
    assert!(display.lines().all(|line| !github_v1(line)));
    assert_eq!(display.lines().count(), lines.len());
}
#[tokio::test]
async fn independent_azure_recognizer_finds_embedded_area_event_originals_not_rendered_commands() {
    let lines = [
        "##vso[task.setvariable variable=x;]value",
        "prefix ##vso[task.complete result=Succeeded;]data",
        "##vso[task.logissue type=error;]data",
        "##vso[..task..complete..]data",
    ];
    assert!(lines.iter().all(|line| azure_command(line)));
    let f = Fixture::new(SEED).await;
    let run = f.returned(&lines.join("\n")).await;
    let display = answer(&run.terminal());
    assert!(display.lines().all(|line| !azure_command(line)));
    assert_eq!(display.lines().count(), lines.len());
}

#[tokio::test]
async fn credential_containing_actual_control_bytes_is_redacted_before_scalar_escaping() {
    let seed = "start\u{001b}finish";
    let f = Fixture::new(seed).await;
    let run = f.returned(&format!("prefix {seed} suffix")).await;
    assert_eq!(answer(&run.terminal()), "prefix [REDACTED] suffix\n");
}
#[tokio::test]
async fn human_display_never_rewrites_the_independent_utf8_or_ascii_fragment_codecs() {
    let f = Fixture::new(SEED).await;
    let run = f
        .returned("中文 ::warning::data\r\n##vso[task.complete]\u{001b}[31m")
        .await;
    let t = run.terminal();
    let utf8 = t.selection(CAP).unwrap().bytes().unwrap().to_vec();
    let ascii = t.ascii(CAP).unwrap().bytes().unwrap().to_vec();
    assert!(answer(&t).contains(r":\u{003a}warning"));
    assert_eq!(t.selection(CAP).unwrap().bytes().unwrap(), utf8);
    assert_eq!(t.ascii(CAP).unwrap().bytes().unwrap(), ascii);
}
