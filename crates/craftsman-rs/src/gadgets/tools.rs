use super::GadgetRuntime;
use async_trait::async_trait;
use craftsman_harness::{AgentError, Tool, ToolCtx, ToolDef, ToolMeta, ToolOutcome};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;

struct ListTool(Arc<GadgetRuntime>);
struct CallTool(Arc<GadgetRuntime>);

pub fn all(runtime: Arc<GadgetRuntime>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ListTool(runtime.clone())),
        Arc::new(CallTool(runtime)),
    ]
}

#[async_trait]
impl Tool for ListTool {
    fn def(&self) -> ToolDef {
        ToolDef { name: "gadget_list".into(),
            description: "List local gadgets, their status, capabilities and callable function schemas. Newly built gadgets are discovered immediately. Unapproved components are not executed and have no function schemas yet.".into(),
            parameters: json!({"type":"object", "properties":{}, "additionalProperties":false}) }
    }
    async fn call(&self, args: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let started = Instant::now();
        let result = if args.as_object().is_some_and(|args| args.is_empty()) {
            self.0.list().await
        } else {
            Err(super::GadgetError::new(
                super::ErrorKind::InvalidArgs,
                "gadget_list takes an empty object",
            ))
        };
        let mut meta = ToolMeta::new();
        meta.duration = Some(started.elapsed());
        let outcome = match result {
            Ok(info) => {
                let json = super::cli::terminal_safe(&serde_json::to_string(&info).unwrap());
                if json.len() > craftsman_gadget::MAX_JSON_BYTES {
                    meta.error_kind = Some(craftsman_harness::ToolErrorKind::OutputLimit);
                    ToolOutcome::err("gadget list exceeds 1 MiB")
                } else {
                    meta.bytes = Some(json.len() as u64);
                    ToolOutcome::ok(json)
                }
            }
            Err(error) => {
                meta.error_kind = Some(error.tool_kind());
                meta.network_calls = error.network_calls;
                meta.truncated = error.logs_truncated;
                ToolOutcome::err(super::cli::terminal_safe(&error.to_string()))
            }
        };
        Ok(outcome.with_meta(meta))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallArgs {
    gadget: String,
    function: String,
    args: Value,
}

#[async_trait]
impl Tool for CallTool {
    fn def(&self) -> ToolDef {
        ToolDef { name: "gadget_call".into(),
            description: "Call one local gadget function with JSON arguments. The harness automatically announces gadget installation, rebuilds and function schemas; use gadget_list only for manual inspection or omitted details. Newly built or rebuilt gadgets are loaded on the next call. First-load capability approval and per-write approval are enforced by the host.".into(),
            parameters: json!({"type":"object", "properties":{
                "gadget":{"type":"string"}, "function":{"type":"string"}, "args":{}
            }, "required":["gadget","function","args"], "additionalProperties":false}) }
    }
    async fn call(&self, args: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let started = Instant::now();
        let parsed: Result<CallArgs, _> = serde_json::from_value(args);
        let result = match parsed {
            Ok(args) => self.0.call(&args.gadget, &args.function, args.args).await,
            Err(error) => Err(super::GadgetError::new(
                super::ErrorKind::InvalidArgs,
                error.to_string(),
            )),
        };
        let mut meta = ToolMeta::new();
        meta.duration = Some(started.elapsed());
        let outcome = match result {
            Ok(result) => {
                meta.duration = Some(result.duration);
                meta.bytes = Some(result.json.len() as u64);
                meta.truncated = result.logs_truncated;
                meta.network_calls = result.network_calls;
                // Captured logs are not model content and never print directly
                // to the terminal. They remain available to the runtime/CLI.
                ToolOutcome::ok(result.json)
            }
            Err(error) => {
                meta.error_kind = Some(error.tool_kind());
                meta.network_calls = error.network_calls;
                meta.truncated = error.logs_truncated;
                ToolOutcome::err(super::cli::terminal_safe(&error.to_string()))
            }
        };
        Ok(outcome.with_meta(meta))
    }
}
