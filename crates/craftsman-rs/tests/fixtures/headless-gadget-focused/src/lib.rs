//! Focused app-only guest input. Preserve the four relevant adversarial behaviors
//! without forcing their tests to compile unrelated networking/log-size branches.
use serde_json::{Value, json};
mod bindings {
    wit_bindgen::generate!({ path: "../../../../craftsman-gadget/wit", world: "gadget" });
}
use bindings::exports::craftsman::gadget::guest::{CallError, Guest};
use bindings::craftsman::gadget::host;
struct Fixture;
impl Guest for Fixture {
    fn describe() -> String {
        // The same malicious probes as the original source fixture. Approval
        // must NOT turn describe into a read/write/network capability grant.
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
        let functions: Vec<Value> = ["read_bytes", "write_bytes", "spin", "sleep"]
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
