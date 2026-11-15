use super::*;
use base64::Engine;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn reports(bytes: &[u8]) -> Vec<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).unwrap();
    text.split("\x1b]7501;")
        .skip(1)
        .map(|text| {
            let (body, tail) = text.split_once("\x1b\\").unwrap();
            assert!(tail.is_empty());
            assert!(body.len() + 9 <= 4096);
            let pairs: BTreeMap<_, _> = body
                .split(':')
                .map(|pair| {
                    let (key, value) = pair.split_once('=').unwrap();
                    assert!(matches!(key, "state" | "app" | "id" | "kind" | "msg"));
                    assert!(
                        value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_.,+/=-".contains(&byte))
                    );
                    (key.to_owned(), value.to_owned())
                })
                .collect();
            assert_eq!(pairs["app"], "craftsman");
            assert!(matches!(
                pairs["state"].as_str(),
                "idle" | "working" | "blocked" | "done" | "error" | "clear"
            ));
            if let Some(message) = pairs.get("msg") {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(message)
                    .unwrap();
                let message = std::str::from_utf8(&decoded).unwrap();
                assert!(!message.chars().any(char::is_control));
                assert!(decoded.len() <= 2048);
            }
            if let Some(id) = pairs.get("id") {
                assert!(id.len() <= 128);
                assert!(id.starts_with("craftsman/tasks"));
                assert!(id.split('/').all(|part| {
                    !part.is_empty()
                        && part.len() <= 32
                        && part
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
                }));
            }
            pairs
        })
        .collect()
}

#[test]
fn terminal_status_setting_defaults_on_and_invalid_values_refuse_without_echo() {
    let mut cfg = Config::default();
    assert_eq!(enabled(&cfg), Ok(true));
    for (value, expected) in [
        (json!({}), true),
        (json!({"osc7501": true}), true),
        (json!({"osc7501": false}), false),
    ] {
        cfg.extra.insert("terminal".into(), value);
        assert_eq!(enabled(&cfg), Ok(expected));
    }
    for value in [
        Value::Null,
        json!(0),
        json!("secret-status-setting"),
        json!([]),
    ] {
        cfg.extra
            .insert("terminal".into(), json!({"osc7501": value}));
        assert_eq!(
            enabled(&cfg),
            Err("Invalid terminal.osc7501: use true or false.")
        );
    }
    cfg.extra
        .insert("terminal".into(), json!("secret-terminal-setting"));
    assert_eq!(
        enabled(&cfg),
        Err("Invalid terminal settings: terminal must be an object.")
    );
}

#[test]
fn status_messages_are_valid_single_line_reports_and_repeat_app_identity() {
    let mut reporter = Reporter::default();
    let mut bytes = Vec::new();
    for status in [
        Status::Idle,
        Status::Working,
        Status::Permission,
        Status::Question,
        Status::Stopping,
        Status::Paused,
        Status::Done,
        Status::Error,
    ] {
        reporter.update(&mut bytes, status, None, false).unwrap();
    }
    let parsed = reports(&bytes);
    assert_eq!(
        parsed
            .iter()
            .map(|report| report["state"].as_str())
            .collect::<Vec<_>>(),
        [
            "idle", "working", "blocked", "blocked", "working", "blocked", "done", "error"
        ]
    );
    assert_eq!(parsed[2]["kind"], "permission");
    assert_eq!(parsed[3]["kind"], "question");
    assert!(!parsed[4].contains_key("kind"));
    assert!(!parsed.iter().any(|report| report.contains_key("progress")));
}

#[test]
fn actual_registry_states_report_child_work_without_leaking_names_or_progress() {
    let registry = AgentRegistry::default();
    let name = "secret-task\x1b]7501;state=error:app=forged\x1b\\";
    registry.insert(name, AgentStatus::Running);
    let mut reporter = Reporter::default();
    let mut bytes = Vec::new();
    reporter
        .update(&mut bytes, Status::Done, Some(&registry), false)
        .unwrap();
    let first = reports(&bytes);
    assert_eq!(first[0]["id"], "craftsman/tasks/1");
    assert_eq!(first[0]["state"], "working");
    assert_eq!(first[1]["state"], "done");
    assert!(!String::from_utf8_lossy(&bytes).contains("secret-task"));
    assert!(!String::from_utf8_lossy(&bytes).contains("forged"));
    bytes.clear();
    registry.set_progress(name, "cancelling\x1b]52;c;secret\x07");
    for _ in 0..50 {
        reporter
            .update(&mut bytes, Status::Done, Some(&registry), false)
            .unwrap();
    }
    assert!(
        bytes.is_empty(),
        "unchanged state must not emit a heartbeat"
    );
    registry.set_status(name, AgentStatus::Suspended);
    reporter
        .update(&mut bytes, Status::Done, Some(&registry), false)
        .unwrap();
    let blocked = reports(&bytes);
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["state"], "blocked");
    assert_eq!(blocked[0]["kind"], "permission");
    bytes.clear();
    registry.set_status(name, AgentStatus::Failed);
    reporter
        .update(&mut bytes, Status::Done, Some(&registry), false)
        .unwrap();
    assert_eq!(reports(&bytes)[0]["state"], "error");
    bytes.clear();
    registry.set_status(name, AgentStatus::Done);
    reporter
        .update(&mut bytes, Status::Done, Some(&registry), false)
        .unwrap();
    assert_eq!(reports(&bytes)[0]["state"], "done");
}

#[test]
fn status_record_capacity_keeps_omitted_live_work_in_an_explicit_aggregate() {
    let registry = AgentRegistry::default();
    for id in 0..(MAX_CHILDREN + 5) {
        registry.insert(format!("worker-{id:03}"), AgentStatus::Running);
    }
    let large_name = "n".repeat(MAX_NAME_BYTES + 1);
    registry.insert(&large_name, AgentStatus::Suspended);
    let mut reporter = Reporter::default();
    let mut bytes = Vec::new();
    reporter
        .update(&mut bytes, Status::Idle, Some(&registry), false)
        .unwrap();
    let parsed = reports(&bytes);
    assert_eq!(reporter.children.len(), MAX_CHILDREN);
    assert_eq!(parsed.len(), MAX_CHILDREN + 2);
    let aggregate = parsed
        .iter()
        .find(|report| report.get("id").map(String::as_str) == Some("craftsman/tasks/overflow"))
        .unwrap();
    assert_eq!(aggregate["state"], "blocked");
    assert_eq!(aggregate["kind"], "permission");
    assert!(!String::from_utf8_lossy(&bytes).contains(&large_name));
    for (name, _) in registry.list() {
        registry.set_status(&name, AgentStatus::Done);
    }
    bytes.clear();
    reporter
        .update(&mut bytes, Status::Done, Some(&registry), false)
        .unwrap();
    assert!(
        reports(&bytes)
            .iter()
            .any(|report| report["state"] == "clear"
                && report.get("id").map(String::as_str) == Some("craftsman/tasks/overflow"))
    );
    registry.insert("fresh-worker", AgentStatus::Running);
    bytes.clear();
    reporter
        .update(&mut bytes, Status::Done, Some(&registry), false)
        .unwrap();
    let parsed = reports(&bytes);
    assert_eq!(reporter.children.len(), MAX_CHILDREN);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0]["state"], "clear");
    assert_eq!(parsed[1]["id"], "craftsman/tasks/33");
    assert_eq!(parsed[1]["state"], "working");
}

#[test]
fn reconfiguration_retains_unobserved_children_until_the_new_session_is_ready() {
    let registry = AgentRegistry::default();
    registry.insert("old", AgentStatus::Running);
    let mut reporter = Reporter::default();
    let mut bytes = Vec::new();
    reporter
        .update(&mut bytes, Status::Idle, Some(&registry), false)
        .unwrap();
    bytes.clear();
    reporter
        .update(&mut bytes, Status::Working, None, true)
        .unwrap();
    let parsed = reports(&bytes);
    assert_eq!(parsed.len(), 1);
    assert!(!parsed[0].contains_key("id"));
    bytes.clear();
    reporter
        .update(&mut bytes, Status::Idle, None, false)
        .unwrap();
    assert_eq!(reports(&bytes)[0]["state"], "clear");
    let next = AgentRegistry::default();
    next.insert("old", AgentStatus::Running);
    bytes.clear();
    reporter
        .update(&mut bytes, Status::Idle, Some(&next), false)
        .unwrap();
    assert_eq!(reports(&bytes)[0]["id"], "craftsman/tasks/2");
}

#[test]
fn exhausted_local_child_ids_do_not_wrap_or_hide_live_work() {
    let registry = AgentRegistry::default();
    registry.insert("live", AgentStatus::Running);
    let mut reporter = Reporter {
        next_id: u64::MAX,
        ..Reporter::default()
    };
    let mut bytes = Vec::new();
    reporter
        .update(&mut bytes, Status::Done, Some(&registry), false)
        .unwrap();
    assert!(reporter.children.is_empty());
    assert_eq!(reports(&bytes)[0]["id"], "craftsman/tasks/overflow");
    assert_eq!(reports(&bytes)[0]["state"], "working");
}

#[test]
fn final_report_clears_only_craftsman_children_and_refuses_later_reports() {
    let registry = AgentRegistry::default();
    registry.insert("live", AgentStatus::Running);
    let mut reporter = Reporter::default();
    let mut bytes = Vec::new();
    reporter
        .update(&mut bytes, Status::Stopping, Some(&registry), false)
        .unwrap();
    bytes.clear();
    reporter.finish(&mut bytes, Status::Idle).unwrap();
    let parsed = reports(&bytes);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0]["state"], "clear");
    assert_eq!(parsed[0]["id"], "craftsman/tasks");
    assert_eq!(parsed[1]["state"], "idle");
    let length = bytes.len();
    reporter
        .update(&mut bytes, Status::Working, Some(&registry), false)
        .unwrap();
    reporter.finish(&mut bytes, Status::Error).unwrap();
    assert_eq!(bytes.len(), length);
}

#[test]
fn partial_write_and_flush_failures_do_not_retry_or_commit_status() {
    struct Failing {
        bytes: Vec<u8>,
        calls: usize,
        fail_flush: bool,
    }
    impl Write for Failing {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.fail_flush {
                self.bytes.extend_from_slice(bytes);
                return Ok(bytes.len());
            }
            if self.calls == 1 {
                self.bytes.extend_from_slice(&bytes[..5]);
                Ok(5)
            } else {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture"))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            self.calls += 1;
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture"))
        }
    }
    for fail_flush in [false, true] {
        let mut output = Failing {
            bytes: Vec::new(),
            calls: 0,
            fail_flush,
        };
        let mut reporter = Reporter::default();
        assert_eq!(
            reporter
                .update(&mut output, Status::Working, None, false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(reporter.root, None);
        let calls = output.calls;
        let length = output.bytes.len();
        reporter
            .update(&mut output, Status::Done, None, false)
            .unwrap();
        reporter.finish(&mut output, Status::Error).unwrap();
        assert_eq!(output.calls, calls);
        assert_eq!(output.bytes.len(), length);
    }
}

mod pipeline;

#[cfg(unix)]
mod native;
