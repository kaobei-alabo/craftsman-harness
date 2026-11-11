//! Transport-agnostic A2UI (Agent-to-UI) support used by ACP.
//!
//! A2UI (https://a2ui.org, spec v0.9) is a declarative, transport-agnostic
//! protocol: the agent streams flat JSON messages — `createSurface` →
//! `updateComponents` (flat component list, tree via id refs, root id
//! `"root"`) → `updateDataModel` (JSON-pointer data binding) →
//! `deleteSurface` — and the client renders them from a catalog of
//! pre-approved components.
//!
//! * [`RenderUiTool`] — the framework tool the LLM calls with a list of A2UI
//!   messages. Payloads are validated against the vendored v0.9 schema
//!   (`data/a2ui_v0_9_schema.json`); valid ones are pushed onto the current
//!   turn's frame channel as `{"type":"a2ui","messages":[…]}`, invalid ones
//!   bounce back to the LLM as a tool error (self-repair).
//! * [`FrameSlot`] — the per-turn frame sink. the ACP frontend installs a channel
//!   sender for the duration of each turn; the tool (running inside the
//!   agent's tool loop) pushes frames through it mid-turn for progressive
//!   rendering. The frontend owns the per-session slot and turn ordering.
//! * [`SYSTEM_PROMPT_SUFFIX`] — the prompt block teaching the agent when and
//!   how to emit A2UI via the tool (the frontend renders with the same
//!   vendored basic catalog).
//!
//! The schema bundle is vendored from github.com/a2ui-project/a2ui
//! `specification/v0_9` (Apache-2.0); see the `_source` key inside the JSON.

use async_trait::async_trait;
use craftsman_harness::{AgentError, Tool, ToolCtx, ToolDef, ToolOutcome};
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock, Mutex};

/// Vendored spec bundle: `server_to_client` (message schema), `common_types`,
/// and the basic catalog, each carrying its canonical `$id`.
const SCHEMA_JSON: &str = include_str!("data/a2ui_v0_9_schema.json");

/// Sender for one turn's outbound frames (already-shaped protocol objects,
/// e.g. `{"type":"a2ui",…}`). Unbounded: a turn produces at most a handful of
/// UI frames and the WS drain is faster than the model.
pub type FrameTx = tokio::sync::mpsc::UnboundedSender<Value>;

/// The per-session frame sink the tool writes through. `None` outside a turn
/// (or outside serve mode entirely) — the tool then reports "no UI surface"
/// to the model instead of failing the turn.
pub type FrameSlot = Arc<Mutex<Option<FrameTx>>>;

/// Compiled v0.9 message validator. Built once; the bundle documents are
/// registered under their canonical `$id`s so every cross-file `$ref`
/// resolves offline. `server_to_client.json` references the catalog as
/// `catalog.json#/…` relative to its own `$id` base while the basic
/// catalog's real `$id` lives under `catalogs/basic/` — the spec leaves
/// catalog binding to the client and we ship the basic catalog, so it is
/// registered under BOTH URIs.
static VALIDATOR: LazyLock<Result<jsonschema::Validator, String>> = LazyLock::new(|| {
    let bundle: Value = serde_json::from_str(SCHEMA_JSON).map_err(|e| e.to_string())?;
    let s2c = bundle["server_to_client"].clone();
    let resource =
        |v: &Value| jsonschema::Resource::from_contents(v.clone()).map_err(|e| e.to_string());
    jsonschema::options()
        .with_resource(
            "https://a2ui.org/specification/v0_9/common_types.json",
            resource(&bundle["common_types"])?,
        )
        .with_resource(
            "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
            resource(&bundle["catalog_basic"])?,
        )
        .with_resource(
            "https://a2ui.org/specification/v0_9/catalog.json",
            resource(&bundle["catalog_basic"])?,
        )
        .build(&s2c)
        .map_err(|e| e.to_string())
});

/// Validate a list of A2UI v0.9 messages. Returns error strings (`[]` = ok).
pub fn validate_messages(messages: &Value) -> Vec<String> {
    let Some(list) = messages.as_array().filter(|l| !l.is_empty()) else {
        return vec!["messages must be a non-empty list of A2UI message objects".into()];
    };
    let validator = match &*VALIDATOR {
        Ok(v) => v,
        Err(e) => return vec![format!("a2ui schema failed to load: {e}")],
    };
    let mut errors = Vec::new();
    for (i, msg) in list.iter().enumerate() {
        if !msg.is_object() {
            errors.push(format!("message[{i}]: not an object"));
            continue;
        }
        if let Some(err) = validator.iter_errors(msg).next() {
            let path = err.instance_path.to_string();
            let at = if path.is_empty() {
                "(root)".to_string()
            } else {
                path
            };
            errors.push(format!("message[{i}] at {at}: {err}"));
        }
    }
    errors
}

/// The `render_ui` tool: validate and forward A2UI frames to the client.
pub struct RenderUiTool {
    slot: FrameSlot,
}

impl RenderUiTool {
    pub fn new(slot: FrameSlot) -> Self {
        Self { slot }
    }
}

#[async_trait]
impl Tool for RenderUiTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "render_ui".into(),
            description: "Render or update dynamic UI on the user's canvas. \
                 `messages` is a list of A2UI v0.9 messages (createSurface, \
                 updateComponents, updateDataModel, deleteSurface), applied in \
                 order. Use this instead of describing forms/choices/structured \
                 data in prose. Call it again with the same surfaceId to update \
                 an existing surface."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "messages": {
                        "type": "array",
                        "items": {"type": "object"},
                        "description": "A2UI v0.9 messages, applied in order."
                    }
                },
                "required": ["messages"]
            }),
        }
    }

    async fn call(&self, args: Value, _ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        let messages = &args["messages"];
        let errs = validate_messages(messages);
        if !errs.is_empty() {
            // Tool error (not turn failure): the model reads it and repairs.
            return Ok(ToolOutcome::err(format!(
                "invalid a2ui: {}",
                errs[..errs.len().min(5)].join("; ")
            )));
        }
        let frame = json!({"type": "a2ui", "messages": messages});
        let sent = {
            let slot = self.slot.lock().unwrap();
            match &*slot {
                Some(tx) => tx.send(frame).is_ok(),
                None => false,
            }
        };
        if !sent {
            return Ok(ToolOutcome::ok(
                "no UI surface attached to this session".to_string(),
            ));
        }
        let n = messages.as_array().map(|l| l.len()).unwrap_or(0);
        Ok(ToolOutcome::ok(format!("rendered {n} a2ui message(s)")))
    }
}

/// Prompt block teaching the agent the A2UI contract. Kept deliberately
/// compact: the standard basic catalog + the four message types + our
/// `ui_action` turn convention. The client renders with the same vendored
/// basic catalog, so names/props below are authoritative.
// PROVENANCE(P5): no match found in deepagents, langchain or a2ui;
// see docs/oss-review.md 5.1 #18. Rewritten in P5.
pub const SYSTEM_PROMPT_SUFFIX: &str = r#"

# Dynamic UI (A2UI)

You can render interactive UI on the user's canvas with the `render_ui`
tool. Prefer it over prose whenever the user should SEE or DO something
structured: forms to fill, options to choose, confirmations, tabular or
card-like data, progress summaries. Keep chat text short when you render
UI ("I've put a form on the canvas — fill it in and hit Submit.").

`render_ui` takes `messages`: a list of A2UI v0.9 messages applied in
order. Every message needs `"version": "v0.9"` plus exactly one of:

1. `createSurface` — `{"surfaceId": "<unique>", "catalogId":
   "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"}`.
   Send once per surface, before anything else targets it.
2. `updateComponents` — `{"surfaceId": ..., "components": [...]}`. A FLAT
   list; the tree comes from id references. Exactly one component must
   have `"id": "root"`. Containers reference children by id via
   `children: [ids...]` (Row/Column/List) or `child: id` (Card).
3. `updateDataModel` — `{"surfaceId": ..., "path": "/some/key", "value":
   ...}`. Omit `path` (or use "/") to replace the whole model.
4. `deleteSurface` — `{"surfaceId": ...}` when the surface is obsolete.

Components (basic catalog) and their key props:
- Text {text, variant?: h1|h2|h3|caption|body} — text supports markdown.
- Image {url}, Icon {name}, Video {url}, AudioPlayer {url}
- Row/Column {children, align?, justify?}, List {children}, Card {child},
  Tabs {tabs}, Divider {}, Modal {trigger, content}
- Button {child, action, variant?: default|primary|borderless} — `child`
  is the ID of a separate Text component (never inline text). action:
  {"event": {"name": "<event>", "context": {...}}}; context values may
  bind with {"path": "/x"}.
- TextField {label, value?, variant?}, CheckBox {label, value},
  ChoicePicker {label, options, value?, variant?}, Slider {value, max,
  min?}, DateTimeInput {value, enableDate?, enableTime?}
Input components two-way bind their `value` to the data model via
{"path": "/form/field"} — seed defaults with updateDataModel first.

Rules:
- ALWAYS include required props (Text.text, Image.url, Button.child +
  Button.action, TextField/CheckBox labels, CheckBox/Slider/DateTimeInput
  value) even when values are data-bound.
- Bind form fields to data-model paths and reference those paths in the
  submit Button's action context so submitted values reach you.
- Reuse a surfaceId to update in place; don't create duplicate surfaces.

When the user interacts (e.g. presses a Button), the next user message
arrives as `[ui_action] {json}` containing the action name, surface id
and resolved context values. Treat it as the user's input: act on it and
reply normally (and update or delete the surface if appropriate).

Example — a small feedback form:
render_ui(messages=[
 {"version":"v0.9","createSurface":{"surfaceId":"feedback","catalogId":
  "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"}},
 {"version":"v0.9","updateComponents":{"surfaceId":"feedback","components":[
  {"id":"root","component":"Column","children":["title","name","score","send"]},
  {"id":"title","component":"Text","text":"Quick feedback","variant":"h2"},
  {"id":"name","component":"TextField","label":"Your name","value":{"path":"/form/name"}},
  {"id":"score","component":"ChoicePicker","label":"Rating","options":[
    {"label":"Good","value":"good"},{"label":"Bad","value":"bad"}],
    "value":{"path":"/form/score"}},
  {"id":"send","component":"Button","child":"send_label","variant":"primary",
   "action":{"event":{"name":"submit_feedback","context":{
    "name":{"path":"/form/name"},"score":{"path":"/form/score"}}}}},
  {"id":"send_label","component":"Text","text":"Submit"}]}},
 {"version":"v0.9","updateDataModel":{"surfaceId":"feedback","path":"/form",
  "value":{"name":"","score":[]}}}])
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn create_surface() -> Value {
        json!({
            "version": "v0.9",
            "createSurface": {
                "surfaceId": "s1",
                "catalogId": "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"
            }
        })
    }

    #[test]
    fn valid_create_surface_passes() {
        let errs = validate_messages(&json!([create_surface()]));
        assert!(errs.is_empty(), "expected valid, got: {errs:?}");
    }

    #[test]
    fn valid_components_update_passes() {
        let msg = json!({
            "version": "v0.9",
            "updateComponents": {
                "surfaceId": "s1",
                "components": [
                    {"id": "root", "component": "Column", "children": ["t"]},
                    {"id": "t", "component": "Text", "text": "hello"}
                ]
            }
        });
        let errs = validate_messages(&json!([create_surface(), msg]));
        assert!(errs.is_empty(), "expected valid, got: {errs:?}");
    }

    #[test]
    fn missing_surface_id_fails() {
        let errs = validate_messages(&json!([{"version": "v0.9", "createSurface": {}}]));
        assert_eq!(errs.len(), 1, "one invalid message: {errs:?}");
        assert!(errs[0].contains("message[0]"));
    }

    #[test]
    fn empty_or_non_list_fails() {
        assert_eq!(validate_messages(&json!([])).len(), 1);
        assert_eq!(validate_messages(&json!("nope")).len(), 1);
        assert_eq!(
            validate_messages(&json!([42])),
            vec!["message[0]: not an object"]
        );
    }

    #[tokio::test]
    async fn tool_pushes_valid_frames_and_reports_without_sink() {
        let slot: FrameSlot = Arc::new(Mutex::new(None));
        let tool = RenderUiTool::new(slot.clone());
        // Framework plumbing for a bare ToolCtx is heavyweight; validate the
        // sink paths directly instead.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        *slot.lock().unwrap() = Some(tx);
        let msgs = json!([create_surface()]);
        let errs = validate_messages(&msgs);
        assert!(errs.is_empty());
        slot.lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(json!({"type": "a2ui", "messages": msgs}))
            .unwrap();
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame["type"], "a2ui");
        // And the tool's schema definition names the required arg.
        assert_eq!(tool.def().name, "render_ui");
        assert!(tool.def().parameters["required"][0] == json!("messages"));
    }
}
