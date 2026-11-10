//! Native model transports, selected explicitly by provider configuration.
//! The framework retains portable messages. Provider-owned response items are
//! checkpointed separately and returned only to the same configured connection.
use crate::llm::LlmClient;
use crate::provider_config::Protocol;
use async_trait::async_trait;
use craftsman_harness::{
    AgentError, AgentMiddleware, AgentState, Flow, ModelRequest, ModelResponse, Msg, ToolCall,
    Usage,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

const HISTORY_KEY: &str = "_craftsman_native_model_history";
const HISTORY_LIMIT: usize = 1_048_576;
const RESPONSE_LIMIT: usize = 16_777_216;

pub struct NativeClient {
    http: reqwest::Client,
    protocol: Protocol,
    base_url: String,
    api_key: String,
    model: String,
    options: BTreeMap<String, Value>,
    pub history: Arc<NativeHistory>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    index: usize,
    message: Msg,
    items: Vec<Value>,
}
pub struct NativeHistory {
    owner: String,
    protocol: Protocol,
    records: Mutex<Vec<Record>>,
}
impl NativeHistory {
    fn matching(&self, index: usize, message: &Msg) -> Option<Vec<Value>> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .find(|record| record.index == index && &record.message == message)
            .map(|record| record.items.clone())
    }
    fn retain(&self, messages: &[Msg]) {
        self.records
            .lock()
            .unwrap()
            .retain(|record| messages.get(record.index) == Some(&record.message));
    }
    fn record(
        &self,
        index: usize,
        response: &ModelResponse,
        items: Vec<Value>,
    ) -> anyhow::Result<()> {
        let mut records = self.records.lock().unwrap();
        records.retain(|record| record.index < index);
        let record = Record {
            index,
            message: response.message.clone(),
            items,
        };
        validate_record(self.protocol, &record)?;
        if records.len() >= 256 || serde_json::to_vec(&record)?.len() > HISTORY_LIMIT {
            anyhow::bail!("Native model history exceeds its checkpoint limit.");
        }
        records.push(record);
        Ok(())
    }
}

#[async_trait]
impl AgentMiddleware for NativeHistory {
    fn name(&self) -> &str {
        "native_model_history"
    }
    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        let mut records = Vec::new();
        if let Some(value) = state.custom.get(HISTORY_KEY) {
            let fail = || AgentError::State("Native model history could not be restored.".into());
            if serde_json::to_vec(value).map_err(|_| fail())?.len() > HISTORY_LIMIT
                || value["version"] != 1
                || !value["connections"].is_object()
            {
                return Err(fail());
            }
            if let Some(value) = value["connections"].get(&self.owner) {
                records = serde_json::from_value(value.clone()).map_err(|_| fail())?;
                if records.len() > 256 {
                    return Err(fail());
                }
                for record in &records {
                    validate_record(self.protocol, record).map_err(|_| fail())?;
                }
            }
        }
        *self.records.lock().unwrap() = records;
        self.retain(&state.messages);
        Ok(())
    }
    async fn review_model(&self, state: &mut AgentState) -> Result<Flow, AgentError> {
        if self.records.lock().unwrap().iter().any(|record| {
            record.index.checked_add(1) == Some(state.messages.len())
                && state.messages.get(record.index) != Some(&record.message)
        }) {
            return Err(AgentError::State(
                "Native model response was changed after validation; tool dispatch was refused."
                    .into(),
            ));
        }
        self.retain(&state.messages);
        let fail = || {
            AgentError::State(
                "Native model history could not be checkpointed. Further work was stopped.".into(),
            )
        };
        let mut value = state
            .custom
            .get(HISTORY_KEY)
            .cloned()
            .unwrap_or_else(|| json!({"version":1,"connections":{}}));
        if value["version"] != 1 || !value["connections"].is_object() {
            return Err(fail());
        }
        value["connections"][&self.owner] =
            serde_json::to_value(&*self.records.lock().unwrap()).map_err(|_| fail())?;
        if value["connections"].as_object().unwrap().len() > 16
            || serde_json::to_vec(&value).map_err(|_| fail())?.len() > HISTORY_LIMIT
        {
            return Err(fail());
        }
        state.custom.insert(HISTORY_KEY.into(), value);
        Ok(Flow::Proceed)
    }
}

impl NativeClient {
    pub fn new(
        protocol: Protocol,
        base_url: &str,
        api_key: &str,
        model: &str,
        owner: &str,
        options: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| anyhow::anyhow!("Native model HTTP client could not be initialized."))?;
        let identity = serde_json::to_vec(&(
            owner,
            protocol,
            base_url.trim_end_matches('/'),
            model,
            api_key,
        ))?;
        let owner = format!("{:x}", Sha256::digest(identity));
        Ok(Self {
            http,
            protocol,
            base_url: base_url.trim_end_matches('/').into(),
            api_key: api_key.into(),
            model: model.into(),
            options: options.clone(),
            history: Arc::new(NativeHistory {
                owner,
                protocol,
                records: Mutex::new(Vec::new()),
            }),
        })
    }

    pub async fn complete(
        &self,
        req: &ModelRequest,
        mut on_text: impl FnMut(&str),
    ) -> anyhow::Result<ModelResponse> {
        self.history.retain(&req.messages);
        let body = self.request(req)?;
        let encoded = serde_json::to_vec(&body)?;
        if encoded.len() > 4_194_304 {
            anyhow::bail!("Native model request exceeds 4 MiB.");
        }
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|_| anyhow::anyhow!("Invalid native model endpoint."))?;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid native model endpoint."))?;
            path.pop_if_empty();
            match self.protocol {
                Protocol::OpenaiResponses => {
                    path.push("responses");
                }
                Protocol::AnthropicMessages => {
                    path.push("messages");
                }
                Protocol::GeminiGenerateContent => {
                    path.push("models").push(&format!(
                        "{}:streamGenerateContent",
                        self.model.strip_prefix("models/").unwrap_or(&self.model)
                    ));
                }
                Protocol::OpenaiChatCompletions => {
                    anyhow::bail!("Chat Completions uses its existing transport.")
                }
            }
        }
        if self.protocol == Protocol::GeminiGenerateContent {
            url.query_pairs_mut().append_pair("alt", "sse");
        }
        let mut builder = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");
        builder = match self.protocol {
            Protocol::AnthropicMessages => builder
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2026-10-21"),
            Protocol::GeminiGenerateContent => builder.header("x-goog-api-key", &self.api_key),
            _ => builder.bearer_auth(&self.api_key),
        };
        let response = builder
            .body(encoded)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Native model connection failed."))?;
        if !response.status().is_success() {
            anyhow::bail!(
                "Native model endpoint returned HTTP {}.",
                response.status().as_u16()
            );
        }
        let mut parser = Parser::new(self.protocol);
        let mut stream = response.bytes_stream();
        let mut pending = Vec::new();
        let mut total = 0usize;
        'read: while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| anyhow::anyhow!("Native model stream was interrupted."))?;
            total = total
                .checked_add(chunk.len())
                .ok_or_else(|| anyhow::anyhow!("Native model response exceeds its limit."))?;
            if total > RESPONSE_LIMIT {
                anyhow::bail!("Native model response exceeds 16 MiB.");
            }
            pending.extend_from_slice(&chunk);
            if pending.len() > 4_194_304 {
                anyhow::bail!("Native model event exceeds 4 MiB.");
            }
            while let Some((position, separator)) = crate::llm::find_event_boundary(&pending) {
                let bytes: Vec<_> = pending.drain(..position + separator).collect();
                parser.event(&bytes[..position], &mut on_text)?;
                if parser.done {
                    pending.clear();
                    break 'read;
                }
            }
        }
        if !pending.is_empty() {
            parser.event(&pending, &mut on_text)?;
        }
        let (response, items) = parser.finish()?;
        self.history.record(req.messages.len(), &response, items)?;
        Ok(response)
    }

    fn request(&self, req: &ModelRequest) -> anyhow::Result<Value> {
        let mut messages = Vec::new();
        let mut system = req.system.clone();
        for (index, message) in req.messages.iter().enumerate() {
            match message {
                Msg::System { content } => { system.push_str("\n\n"); system.push_str(content); },
                Msg::User { content } => match self.protocol {
                    Protocol::OpenaiResponses => messages.push(json!({"role":"user","content":content})),
                    Protocol::AnthropicMessages => push_parts(&mut messages, "user", "content", vec![json!({"type":"text","text":content})]),
                    Protocol::GeminiGenerateContent => push_parts(&mut messages, "user", "parts", vec![json!({"text":content})]),
                    _ => unreachable!(),
                },
                Msg::Assistant { content, tool_calls } => {
                    if let Some(items) = self.history.matching(index, message) {
                        match self.protocol {
                            Protocol::OpenaiResponses => messages.extend(items),
                            Protocol::AnthropicMessages => push_parts(&mut messages, "assistant", "content", items),
                            Protocol::GeminiGenerateContent => push_parts(&mut messages, "model", "parts", items),
                            _ => unreachable!(),
                        }
                        continue;
                    }
                    let mut parts = Vec::new();
                    if !content.is_empty() { parts.push(match self.protocol {
                        Protocol::OpenaiResponses => json!({"role":"assistant","content":content}),
                        Protocol::AnthropicMessages => json!({"type":"text","text":content}),
                        _ => json!({"text":content}),
                    }); }
                    for call in tool_calls {
                        if !call.args.is_object() { anyhow::bail!("Native model tool history contains invalid arguments."); }
                        parts.push(match self.protocol {
                            Protocol::OpenaiResponses => json!({"type":"function_call","call_id":call.id,"name":call.name,"arguments":serde_json::to_string(&call.args)?}),
                            Protocol::AnthropicMessages => json!({"type":"tool_use","id":call.id,"name":call.name,"input":call.args}),
                            _ => json!({"functionCall":{"id":call.id,"name":call.name,"args":call.args}}),
                        });
                    }
                    match self.protocol {
                        Protocol::OpenaiResponses => messages.extend(parts),
                        Protocol::AnthropicMessages => push_parts(&mut messages, "assistant", "content", parts),
                        Protocol::GeminiGenerateContent => push_parts(&mut messages, "model", "parts", parts),
                        _ => unreachable!(),
                    }
                },
                Msg::Tool { tool_call_id, content, is_error } => match self.protocol {
                    Protocol::OpenaiResponses => messages.push(json!({"type":"function_call_output","call_id":tool_call_id,"output":content})),
                    Protocol::AnthropicMessages => push_parts(&mut messages, "user", "content", vec![json!({"type":"tool_result","tool_use_id":tool_call_id,"content":content,"is_error":is_error})]),
                    Protocol::GeminiGenerateContent => {
                        let (assistant_index, call_index, call) = matching_call(&req.messages[..index], tool_call_id).ok_or_else(|| anyhow::anyhow!("Native model tool result has no matching call."))?;
                        let mut result = json!({"id":call.id,"name":call.name,"response":if *is_error { json!({"error":content}) } else { json!({"result":content}) }});
                        if let Some(parts) = self.history.matching(assistant_index, &req.messages[assistant_index]) {
                            let original = parts.iter().filter_map(|part| part.get("functionCall")).nth(call_index);
                            if let Some(id) = original.and_then(|call| call.get("id")) { result["id"] = id.clone(); }
                            else { result.as_object_mut().unwrap().remove("id"); }
                        }
                        push_parts(&mut messages, "user", "parts", vec![json!({"functionResponse":result})]);
                    },
                    _ => unreachable!(),
                },
            }
        }
        let tools: Vec<Value> = req.tools.iter().map(|tool| match self.protocol {
            Protocol::OpenaiResponses => json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters,"strict":false}),
            Protocol::AnthropicMessages => json!({"name":tool.name,"description":tool.description,"input_schema":tool.parameters}),
            _ => json!({"name":tool.name,"description":tool.description,"parametersJsonSchema":tool.parameters}),
        }).collect();
        let mut body = match self.protocol {
            Protocol::OpenaiResponses => {
                json!({"model":self.model,"instructions":system,"input":messages,"tools":tools,"stream":true,"store":false,"include":["reasoning.encrypted_content"]})
            }
            Protocol::AnthropicMessages => {
                json!({"model":self.model,"system":system,"messages":messages,"tools":tools,"stream":true,"max_tokens":8192})
            }
            Protocol::GeminiGenerateContent => {
                json!({"systemInstruction":{"parts":[{"text":system}]},"contents":messages,"tools":[{"functionDeclarations":tools}],"generationConfig":{}})
            }
            _ => unreachable!(),
        };
        if req.tools.is_empty() {
            body.as_object_mut().unwrap().remove("tools");
        }
        for (key, value) in &self.options {
            if key == "craftsman_reasoning" {
                let effort = value["type"] == "effort";
                match self.protocol {
                    Protocol::OpenaiResponses if effort => {
                        body["reasoning"] = json!({"effort":value["value"]})
                    }
                    Protocol::AnthropicMessages if effort => {
                        body["thinking"] = json!({"type":"adaptive"});
                        body["output_config"] = json!({"effort":value["value"]});
                    }
                    Protocol::AnthropicMessages => {
                        body["thinking"] = json!({"type":"enabled","budget_tokens":value["value"]})
                    }
                    Protocol::GeminiGenerateContent if effort => {
                        let level = value["value"]
                            .as_str()
                            .filter(|level| matches!(*level, "minimal" | "low" | "medium" | "high"))
                            .ok_or_else(|| anyhow::anyhow!("Unsupported Gemini thinking level."))?;
                        body["generationConfig"]["thinkingConfig"] =
                            json!({"thinkingLevel":level.to_ascii_uppercase()});
                    }
                    Protocol::GeminiGenerateContent => {
                        body["generationConfig"]["thinkingConfig"] =
                            json!({"thinkingBudget":value["value"]})
                    }
                    _ => anyhow::bail!("This protocol does not support a thinking token budget."),
                }
                continue;
            }
            let supported = match self.protocol {
                Protocol::OpenaiResponses => matches!(
                    key.as_str(),
                    "max_output_tokens" | "temperature" | "top_p" | "text"
                ),
                Protocol::AnthropicMessages => matches!(
                    key.as_str(),
                    "max_tokens" | "temperature" | "top_p" | "top_k" | "stop_sequences"
                ),
                Protocol::GeminiGenerateContent => matches!(
                    key.as_str(),
                    "maxOutputTokens" | "temperature" | "topP" | "topK" | "stopSequences"
                ),
                _ => false,
            };
            if !supported {
                anyhow::bail!("Unsupported native model generation setting.");
            }
            if self.protocol == Protocol::GeminiGenerateContent {
                body["generationConfig"][key] = value.clone();
            } else {
                body[key] = value.clone();
            }
        }
        Ok(body)
    }
}

fn push_parts(messages: &mut Vec<Value>, role: &str, field: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut().filter(|value| value["role"] == role) {
        last[field].as_array_mut().unwrap().extend(parts);
    } else {
        let mut message = json!({"role":role});
        message[field] = Value::Array(parts);
        messages.push(message);
    }
}
fn matching_call<'a>(messages: &'a [Msg], id: &str) -> Option<(usize, usize, &'a ToolCall)> {
    for (index, message) in messages.iter().enumerate().rev() {
        if let Msg::Assistant { tool_calls, .. } = message {
            return tool_calls
                .iter()
                .enumerate()
                .find(|(_, call)| call.id == id)
                .map(|(call_index, call)| (index, call_index, call));
        }
    }
    None
}

/// A restored cache must describe the same portable assistant message. Opaque
/// reasoning/signature fields stay untouched, but cannot add or change calls.
fn validate_record(protocol: Protocol, record: &Record) -> anyhow::Result<()> {
    let Msg::Assistant {
        content,
        tool_calls,
    } = &record.message
    else {
        anyhow::bail!("Invalid native assistant history.");
    };
    if record.items.len() > 512 {
        anyhow::bail!("Invalid native assistant history.");
    }
    let mut text = String::new();
    let mut calls = Vec::new();
    for item in &record.items {
        match protocol {
            Protocol::OpenaiResponses => {
                if item["type"] == "message" {
                    for part in item["content"]
                        .as_array()
                        .ok_or_else(|| anyhow::anyhow!("Invalid native message history."))?
                    {
                        if part["type"] == "output_text" {
                            text.push_str(
                                part["text"].as_str().ok_or_else(|| {
                                    anyhow::anyhow!("Invalid native text history.")
                                })?,
                            );
                        } else if part["type"] == "refusal" {
                            text.push_str(part["refusal"].as_str().ok_or_else(|| {
                                anyhow::anyhow!("Invalid native refusal history.")
                            })?);
                        }
                    }
                } else if item["type"] == "function_call" {
                    if item
                        .get("status")
                        .is_some_and(|status| status != "completed")
                    {
                        anyhow::bail!("Unfinished native call history.");
                    }
                    calls.push((
                        item.get("call_id"),
                        item.get("name"),
                        serde_json::from_str::<Value>(
                            item["arguments"].as_str().unwrap_or_default(),
                        )?,
                    ));
                }
            }
            Protocol::AnthropicMessages => {
                if item["type"] == "text" {
                    text.push_str(
                        item["text"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("Invalid native text history."))?,
                    );
                } else if item["type"] == "tool_use" {
                    calls.push((item.get("id"), item.get("name"), item["input"].clone()));
                }
            }
            Protocol::GeminiGenerateContent => {
                if item["thought"] != true {
                    if let Some(value) = item["text"].as_str() {
                        text.push_str(value);
                    }
                }
                if let Some(call) = item.get("functionCall") {
                    calls.push((
                        call.get("id"),
                        call.get("name"),
                        call.get("args").cloned().unwrap_or_else(|| json!({})),
                    ));
                }
            }
            _ => anyhow::bail!("Invalid native history protocol."),
        }
    }
    if &text != content || calls.len() != tool_calls.len() {
        anyhow::bail!("Native history does not match its assistant message.");
    }
    for ((id, name, args), call) in calls.into_iter().zip(tool_calls) {
        if name.and_then(Value::as_str) != Some(&call.name)
            || args != call.args
            || id.is_some_and(|id| id.as_str() != Some(&call.id))
        {
            anyhow::bail!("Native call history does not match its assistant message.");
        }
    }
    Ok(())
}

struct Parser {
    protocol: Protocol,
    content: String,
    items: Vec<Value>,
    argument_fragments: BTreeMap<usize, String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    done: bool,
    stopped: bool,
    began: bool,
    open_blocks: BTreeSet<usize>,
}
impl Parser {
    fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            content: String::new(),
            items: Vec::new(),
            argument_fragments: BTreeMap::new(),
            input_tokens: None,
            output_tokens: None,
            done: false,
            stopped: false,
            began: false,
            open_blocks: BTreeSet::new(),
        }
    }
    fn event(&mut self, bytes: &[u8], on_text: &mut impl FnMut(&str)) -> anyhow::Result<()> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("Native model stream contains invalid UTF-8."))?;
        let data = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            return Ok(());
        }
        if self.done {
            anyhow::bail!("Native model sent data after completion.");
        }
        let value: Value = serde_json::from_str(&data)
            .map_err(|_| anyhow::anyhow!("Native model stream contains invalid JSON."))?;
        if value.get("error").is_some()
            || matches!(
                value["type"].as_str(),
                Some("error" | "response.failed" | "response.incomplete")
            )
        {
            anyhow::bail!("Native model reported a generation failure.");
        }
        match self.protocol {
            Protocol::OpenaiResponses => match value["type"].as_str() {
                Some("response.output_text.delta") => self.text(
                    value["delta"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Invalid native text event."))?,
                    on_text,
                )?,
                Some("response.completed") => {
                    let response = &value["response"];
                    if response["status"] != "completed" {
                        anyhow::bail!("Native response did not complete.");
                    }
                    self.items = response["output"]
                        .as_array()
                        .ok_or_else(|| anyhow::anyhow!("Native response has no output items."))?
                        .clone();
                    let final_text: String = self
                        .items
                        .iter()
                        .filter(|item| item["type"] == "message")
                        .filter_map(|item| item["content"].as_array())
                        .flatten()
                        .filter_map(|part| {
                            if part["type"] == "output_text" {
                                part["text"].as_str()
                            } else if part["type"] == "refusal" {
                                part["refusal"].as_str()
                            } else {
                                None
                            }
                        })
                        .collect();
                    if self.content.is_empty() {
                        self.text(&final_text, on_text)?;
                    } else if self.content != final_text {
                        anyhow::bail!("Native final text does not match its streamed text.");
                    }
                    self.input_tokens = response["usage"]["input_tokens"].as_u64();
                    self.output_tokens = response["usage"]["output_tokens"].as_u64();
                    self.done = true;
                }
                Some(_) => {}
                None => anyhow::bail!("Native response event has no type."),
            },
            Protocol::AnthropicMessages => match value["type"].as_str() {
                Some("message_start") => {
                    if self.began {
                        anyhow::bail!("Native message started more than once.");
                    }
                    self.began = true;
                    let usage = &value["message"]["usage"];
                    self.input_tokens = usage["input_tokens"]
                        .as_u64()
                        .and_then(|n| {
                            n.checked_add(
                                usage["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                            )
                        })
                        .and_then(|n| {
                            n.checked_add(usage["cache_read_input_tokens"].as_u64().unwrap_or(0))
                        });
                }
                Some("content_block_start") => {
                    if !self.began || self.stopped {
                        anyhow::bail!("Native content block is outside its message.");
                    }
                    let index = index(&value)?;
                    if index != self.items.len() || index >= 512 {
                        anyhow::bail!("Invalid native content block order.");
                    }
                    let block = value["content_block"].clone();
                    if !matches!(
                        block["type"].as_str(),
                        Some("text" | "thinking" | "redacted_thinking" | "tool_use")
                    ) {
                        anyhow::bail!("Unsupported native content block.");
                    }
                    if block["type"] == "text" {
                        self.text(block["text"].as_str().unwrap_or_default(), on_text)?;
                    }
                    self.items.push(block);
                    self.open_blocks.insert(index);
                }
                Some("content_block_delta") => {
                    let index = index(&value)?;
                    if !self.open_blocks.contains(&index) {
                        anyhow::bail!("Native content delta has no open block.");
                    }
                    let delta = &value["delta"];
                    if delta["type"] == "text_delta" {
                        self.text(
                            delta["text"]
                                .as_str()
                                .ok_or_else(|| anyhow::anyhow!("Invalid native text event."))?,
                            on_text,
                        )?;
                    }
                    let block = self
                        .items
                        .get_mut(index)
                        .ok_or_else(|| anyhow::anyhow!("Unknown native content block."))?;
                    match delta["type"].as_str() {
                        Some("input_json_delta") => {
                            self.argument_fragments.entry(index).or_default().push_str(
                                delta["partial_json"].as_str().ok_or_else(|| {
                                    anyhow::anyhow!("Invalid native tool arguments.")
                                })?,
                            )
                        }
                        Some(kind @ ("text_delta" | "thinking_delta" | "signature_delta")) => {
                            let field = match kind {
                                "text_delta" => "text",
                                "thinking_delta" => "thinking",
                                _ => "signature",
                            };
                            let mut text = block[field].as_str().unwrap_or_default().to_owned();
                            text.push_str(
                                delta[field].as_str().ok_or_else(|| {
                                    anyhow::anyhow!("Invalid native content delta.")
                                })?,
                            );
                            block[field] = Value::String(text);
                        }
                        _ => anyhow::bail!("Unsupported native content delta."),
                    }
                }
                Some("content_block_stop") => {
                    let index = index(&value)?;
                    if !self.open_blocks.remove(&index) {
                        anyhow::bail!("Native content block stopped without being open.");
                    }
                    if let Some(arguments) = self.argument_fragments.remove(&index) {
                        self.items[index]["input"] =
                            serde_json::from_str(&arguments).map_err(|_| {
                                anyhow::anyhow!("Native tool arguments are incomplete.")
                            })?;
                    }
                }
                Some("message_delta") => {
                    if !self.began {
                        anyhow::bail!("Native message delta has no message.");
                    }
                    self.output_tokens = value["usage"]["output_tokens"]
                        .as_u64()
                        .or(self.output_tokens);
                    if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                        if !matches!(
                            reason,
                            "end_turn" | "tool_use" | "stop_sequence" | "refusal"
                        ) {
                            anyhow::bail!("Native message stopped before a complete answer.");
                        }
                        self.stopped = true;
                    }
                }
                Some("message_stop") => {
                    if !self.began || !self.stopped || !self.open_blocks.is_empty() {
                        anyhow::bail!("Native message stopped before its content completed.");
                    }
                    self.done = true;
                }
                Some("ping") => {}
                _ => anyhow::bail!("Unsupported native message event."),
            },
            Protocol::GeminiGenerateContent => {
                let candidate = value["candidates"]
                    .as_array()
                    .and_then(|v| v.first())
                    .ok_or_else(|| anyhow::anyhow!("Native Gemini response has no candidate."))?;
                if let Some(parts) = candidate["content"]["parts"].as_array() {
                    for part in parts {
                        if part["thought"] != true {
                            if let Some(text) = part["text"].as_str() {
                                self.text(text, on_text)?;
                            }
                        }
                        self.items.push(part.clone());
                    }
                }
                if let Some(reason) = candidate["finishReason"].as_str() {
                    if reason != "STOP" {
                        anyhow::bail!("Native Gemini generation stopped before a complete answer.");
                    }
                    self.stopped = true;
                }
                if let Some(usage) = value.get("usageMetadata") {
                    self.input_tokens = usage["promptTokenCount"].as_u64();
                    self.output_tokens = usage["candidatesTokenCount"].as_u64().and_then(|n| {
                        n.checked_add(usage["thoughtsTokenCount"].as_u64().unwrap_or(0))
                    });
                }
            }
            _ => unreachable!(),
        }
        if self.items.len() > 512
            || self
                .argument_fragments
                .values()
                .any(|text| text.len() > HISTORY_LIMIT)
        {
            anyhow::bail!("Native model output exceeds its limit.");
        }
        Ok(())
    }
    fn text(&mut self, text: &str, on_text: &mut impl FnMut(&str)) -> anyhow::Result<()> {
        if self.content.len().saturating_add(text.len()) > HISTORY_LIMIT {
            anyhow::bail!("Native model text exceeds 1 MiB.");
        }
        self.content.push_str(text);
        on_text(text);
        Ok(())
    }
    fn finish(self) -> anyhow::Result<(ModelResponse, Vec<Value>)> {
        if !(self.done || self.protocol == Protocol::GeminiGenerateContent && self.stopped)
            || !self.argument_fragments.is_empty()
            || !self.open_blocks.is_empty()
        {
            anyhow::bail!("Native model stream ended before completion.");
        }
        let mut calls = Vec::new();
        for item in &self.items {
            let call = match self.protocol {
                Protocol::OpenaiResponses if item["type"] == "function_call" => Some((
                    item["call_id"].as_str().map(str::to_owned),
                    item["name"].as_str(),
                    serde_json::from_str(item["arguments"].as_str().unwrap_or_default()).ok(),
                )),
                Protocol::AnthropicMessages if item["type"] == "tool_use" => Some((
                    item["id"].as_str().map(str::to_owned),
                    item["name"].as_str(),
                    item.get("input").cloned(),
                )),
                Protocol::GeminiGenerateContent if item.get("functionCall").is_some() => {
                    let call = &item["functionCall"];
                    static NEXT: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(0);
                    let id = call["id"].as_str().map(str::to_owned).unwrap_or_else(|| {
                        format!(
                            "craftsman-gemini-{}-{}",
                            std::process::id(),
                            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        )
                    });
                    Some((Some(id), call["name"].as_str(), call.get("args").cloned()))
                }
                _ => None,
            };
            if let Some((id, name, args)) = call {
                let id = id
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("Native tool call has no identity."))?;
                let name = name
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("Native tool call has no name."))?;
                let args = args.filter(Value::is_object).ok_or_else(|| {
                    anyhow::anyhow!("Native tool call arguments must be an object.")
                })?;
                if calls.iter().any(|call: &ToolCall| call.id == id) {
                    anyhow::bail!("Native model returned duplicate tool call IDs.");
                }
                calls.push(ToolCall {
                    id,
                    name: name.into(),
                    args,
                });
            }
        }
        let usage = Usage {
            input_tokens: self
                .input_tokens
                .ok_or_else(|| anyhow::anyhow!("Native response omitted input usage."))?,
            output_tokens: self
                .output_tokens
                .ok_or_else(|| anyhow::anyhow!("Native response omitted output usage."))?,
        };
        if self.content.is_empty() && calls.is_empty() {
            anyhow::bail!("Native response contains no answer or tool calls.");
        }
        Ok((
            ModelResponse {
                message: Msg::Assistant {
                    content: self.content,
                    tool_calls: calls,
                },
                usage,
            },
            self.items,
        ))
    }
}
fn index(value: &Value) -> anyhow::Result<usize> {
    value["index"]
        .as_u64()
        .filter(|n| *n < 512)
        .map(|n| n as usize)
        .ok_or_else(|| anyhow::anyhow!("Invalid native content index."))
}

impl LlmClient {
    pub fn configured(
        base_url: &str,
        api_key: &str,
        model: &str,
        protocol: Protocol,
        owner: &str,
        options: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Self> {
        if protocol == Protocol::OpenaiChatCompletions {
            let mut options = options.clone();
            if let Some(value) = options.remove("craftsman_reasoning") {
                if value["type"] != "effort" {
                    anyhow::bail!("Chat Completions does not support a thinking token budget.");
                }
                options.insert("reasoning_effort".into(), value["value"].clone());
            }
            let mut client = Self::new_with_options(base_url, api_key, model, &options);
            client.configured_connection = true;
            return Ok(client);
        }
        let native = NativeClient::new(protocol, base_url, api_key, model, owner, options)?;
        let mut client = Self::new_with_options(base_url, api_key, model, &BTreeMap::new());
        client.native = Some(native);
        Ok(client)
    }
}

#[cfg(test)]
mod tests;
