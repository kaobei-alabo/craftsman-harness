//! App-only adversarial guest fixture; no SDK/WIT/old fixture changes.
use serde_json::{Value, json};
use std::io::Write;
mod bindings {
    wit_bindgen::generate!({ path: "../../../../craftsman-gadget/wit", world: "gadget" });
}
use bindings::exports::craftsman::gadget::guest::{CallError, Guest};
use bindings::craftsman::gadget::host;
struct Fixture;
impl Guest for Fixture {
    fn describe() -> String {
        // Even an approved source must see all three denied during describe.
        let read = host::read_file("safe.txt").is_ok();
        let write = host::write_file("describe-write.txt", b"must-not-exist").is_ok();
        let http = host::http_request(&host::Request {
            method: "GET".into(),
            url: "https://example.com/".into(),
            headers: vec![],
            body: None,
        })
        .is_ok();
        let probes = format!("read={read};write={write};http={http}");
        let functions: Vec<Value> = [
            "read_bytes",
            "write_bytes",
            "catch_write_then_read",
            "catch_write_then_spin",
            "catch_write_then_sleep",
            "ambient",
            "literal_error",
            "output",
            "spin",
            "sleep",
            "allocate",
            "huge",
            "echo",
            "write_and_echo",
            "logs",
            "invalid_stdout",
            "huge_log",
            "post_fail_stdio",
            "logs_then_error",
        ]
        .into_iter()
        .map(|name| json!({"name":name,"description":probes,"input_schema":{"type":"object"}}))
        .collect();
        json!({"functions":functions}).to_string()
    }
    fn call(function: String, args: String) -> Result<String, CallError> {
        let args: Value = serde_json::from_str(&args)
            .map_err(|_| CallError::InvalidArgs("fixture arguments".into()))?;
        let value = match function.as_str() {
            "read_bytes" => json!(
                host::read_file(args["path"].as_str().unwrap_or("safe.txt"))
                    .map_err(CallError::Failed)?
            ),
            "write_bytes" => {
                let bytes: Vec<u8> = serde_json::from_value(args["bytes"].clone())
                    .map_err(|_| CallError::InvalidArgs("bytes".into()))?;
                host::write_file(args["path"].as_str().unwrap_or("created.bin"), &bytes)
                    .map_err(CallError::Failed)?;
                json!({"written":true})
            }
            "catch_write_then_read" => {
                let caught = host::write_file("unapproved.txt", b"no").is_err();
                let later = host::read_file("safe.txt").is_ok();
                json!({"caught":caught,"later_read":later})
            }
            "catch_write_then_spin" => {
                let _ = host::write_file("unapproved.txt", b"no");
                host::log(host::Level::Info, "must-not-be-logged-after-fail");
                loop {
                    std::hint::spin_loop();
                }
            }
            "catch_write_then_sleep" => {
                let _ = host::write_file("unapproved.txt", b"no");
                std::thread::sleep(std::time::Duration::from_secs(3600));
                json!({"must_not_finish":true})
            }
            "ambient" => json!({"env":std::env::var("GADGET_TEST_SECRET").ok(),
                "file":std::fs::read("Cargo.toml").is_ok(), "socket":std::net::TcpStream::connect("127.0.0.1:9").is_ok()}),
            "literal_error" => {
                return Err(CallError::Failed(
                    "NeedsApproval SECRET-GUEST-CONTENT".into(),
                ));
            }
            "output" => {
                for _ in 0..1000 {
                    println!("{}", "x".repeat(100));
                    eprintln!("{}", "y".repeat(100));
                    host::log(host::Level::Info, &"z".repeat(100));
                }
                json!({"done":true})
            }
            "spin" => {
                host::log(
                    host::Level::Info,
                    "spin-entered .................................................................",
                );
                loop {
                    std::hint::spin_loop();
                }
            }
            "sleep" => {
                host::log(
                    host::Level::Info,
                    "sleep-entered .................................................................",
                );
                std::thread::sleep(std::time::Duration::from_secs(3600));
                json!({})
            }
            "allocate" => {
                let bytes = std::hint::black_box(vec![1u8; 1024 * 1024 * 1024]);
                json!(bytes.iter().map(|b| *b as u64).sum::<u64>())
            }
            "echo" => args["value"].clone(),
            "write_and_echo" => {
                let path = args["path"].as_str().unwrap_or("created.bin");
                let payload = args["payload"].as_str().unwrap_or("payload");
                host::write_file(path, payload.as_bytes()).map_err(CallError::Failed)?;
                json!({"path":path,"payload":payload})
            }
            "logs" => {
                for part in args["parts"].as_array().unwrap() {
                    let text = part["text"].as_str().unwrap();
                    match part["channel"].as_str().unwrap() {
                        "stdout" => {
                            std::io::stdout().write_all(text.as_bytes()).unwrap();
                            std::io::stdout().flush().unwrap();
                        }
                        "stderr" => {
                            std::io::stderr().write_all(text.as_bytes()).unwrap();
                            std::io::stderr().flush().unwrap();
                        }
                        "log" => host::log(host::Level::Info, text),
                        _ => panic!("unknown fixture channel"),
                    }
                }
                json!({"done":true})
            }
            "invalid_stdout" => {
                std::io::stdout().write_all(&[0xff]).unwrap();
                std::io::stdout().flush().unwrap();
                json!({"must_not_publish":true})
            }
            "huge_log" => {
                host::log(host::Level::Info, &"x".repeat(64 * 1024 + 1));
                json!({"must_not_publish":true})
            }
            "post_fail_stdio" => {
                std::io::stdout().write_all(b"before-fail-entry .................................................................\n").unwrap();
                std::io::stdout().flush().unwrap();
                let caught = host::write_file("unapproved.txt", b"no").is_err();
                let stdout = std::io::stdout()
                    .write_all(b"must-not-be-logged-after-fail-stdio")
                    .is_ok();
                let stderr = std::io::stderr()
                    .write_all(b"must-not-be-logged-after-fail-stderr")
                    .is_ok();
                host::log(host::Level::Info, "must-not-be-logged-after-fail-import");
                json!({"caught":caught,"stdout":stdout,"stderr":stderr})
            }
            "logs_then_error" => {
                std::io::stdout()
                    .write_all(args["text"].as_str().unwrap().as_bytes())
                    .unwrap();
                std::io::stdout().flush().unwrap();
                return Err(CallError::Failed(
                    "NeedsApproval SECRET-GUEST-CONTENT".into(),
                ));
            }
            "huge" => return Ok(json!("x".repeat(1024 * 1024 + 1)).to_string()),
            _ => {
                return Err(CallError::UnknownFunction(
                    "fixture missing function".into(),
                ));
            }
        };
        Ok(value.to_string())
    }
}
bindings::export!(Fixture with_types_in bindings);
