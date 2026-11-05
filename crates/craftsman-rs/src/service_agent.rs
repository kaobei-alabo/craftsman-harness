//! Restricted one-shot service-agent execution.
//!
//! This mode intentionally bypasses the normal workspace session builder: it
//! never constructs `RealFs`, never registers `execute`, and never opens local
//! thread, memory, skill, LSP, or checkpoint state.

use crate::agent::{StreamingLlmProvider, TokenSink};
use crate::config::Config;
use crate::execution_profile::ExecutionProfile;
use crate::llm::LlmClient;
use async_trait::async_trait;
use craftsman_harness::builtin::{
    PatchToolCallsMiddleware, ResultBudgetsMiddleware, SystemRemindersMiddleware,
    TodoListMiddleware,
};
use craftsman_harness::{
    Agent, AgentState, Backend, FileData, Msg, ScopedVirtualFs, Tool, ToolCtx, ToolDef,
    ToolOutcome, TurnResult, VirtualFsLimits,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

const PROTOCOL_VERSION: u32 = 1;
const DEFAULT_MAX_ROUNDS: usize = 8;
const MAX_MAX_ROUNDS: usize = 32;
const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_CONTEXT_BYTES: usize = 512 * 1024;
const MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024;
const HARD_MAX_VIRTUAL_FILES: usize = 128;
const HARD_MAX_VIRTUAL_FILE_BYTES: usize = 256 * 1024;
const HARD_MAX_VIRTUAL_TOTAL_BYTES: usize = 2 * 1024 * 1024;
const MAX_ALLOWED_TOOLS: usize = 16;
const SERVICE_AGENT_TOOLS: [&str; 8] = [
    "read_file",
    "write_file",
    "edit_file",
    "ls",
    "glob",
    "grep",
    "web_fetch",
    "web_search",
];

// PROVENANCE(P5): no match found in deepagents or langchain;
// see docs/oss-review.md 5.1 #20. Rewritten in P5.
const SERVICE_BOUNDARY: &str = "You are running in Craftsman's shared service-agent profile.\n\
You have no real host filesystem or shell access. File tools operate only on the runtime virtual filesystem.\n\
Approved skills are read-only under /skills. Temporary working files may be created only under /scratch.\n\
Selected web tools may access only public HTTPS destinations; private, loopback, link-local, and cluster-local targets are blocked.\n\
Do not claim to inspect host files, run commands, install software, or access paths outside the mounted virtual namespaces.";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceAgentRequest {
    version: u32,
    system_prompt: String,
    input: String,
    #[serde(default)]
    messages: Vec<Msg>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    files: Vec<VirtualFileInput>,
    #[serde(default = "default_writable_prefixes")]
    writable_prefixes: Vec<String>,
    #[serde(default = "default_max_rounds")]
    max_rounds: usize,
    #[serde(default)]
    limits: ServiceAgentLimits,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VirtualFileInput {
    path: String,
    content: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServiceAgentLimits {
    max_files: usize,
    max_file_bytes: usize,
    max_total_bytes: usize,
}

impl Default for ServiceAgentLimits {
    fn default() -> Self {
        let limits = VirtualFsLimits::default();
        Self {
            max_files: limits.max_files,
            max_file_bytes: limits.max_file_bytes,
            max_total_bytes: limits.max_total_bytes,
        }
    }
}

impl From<ServiceAgentLimits> for VirtualFsLimits {
    fn from(value: ServiceAgentLimits) -> Self {
        Self {
            max_files: value.max_files,
            max_file_bytes: value.max_file_bytes,
            max_total_bytes: value.max_total_bytes,
        }
    }
}

fn default_writable_prefixes() -> Vec<String> {
    vec!["/scratch".into()]
}

const fn default_max_rounds() -> usize {
    DEFAULT_MAX_ROUNDS
}

#[derive(Debug, Serialize)]
struct ServiceAgentResult {
    version: u32,
    profile: ExecutionProfile,
    status: &'static str,
    output: String,
    usage: ServiceAgentUsage,
    modified_files: Vec<VirtualFileOutput>,
}

#[derive(Debug, Serialize)]
struct ServiceAgentUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Serialize)]
struct VirtualFileOutput {
    path: String,
    content: String,
}

#[derive(Debug, Clone, Copy)]
enum VirtualFileKind {
    Read,
    Write,
    Edit,
    Ls,
    Glob,
    Grep,
}

struct VirtualFileTool {
    kind: VirtualFileKind,
    backend: Arc<dyn Backend>,
}

impl VirtualFileKind {
    const ALL: [Self; 6] = [
        Self::Read,
        Self::Write,
        Self::Edit,
        Self::Ls,
        Self::Glob,
        Self::Grep,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Read => "read_file",
            Self::Write => "write_file",
            Self::Edit => "edit_file",
            Self::Ls => "ls",
            Self::Glob => "glob",
            Self::Grep => "grep",
        }
    }
}

impl VirtualFileTool {
    fn selected(backend: Arc<dyn Backend>, allowed_tools: &[String]) -> Vec<Arc<dyn Tool>> {
        VirtualFileKind::ALL
            .into_iter()
            .filter(|kind| allowed_tools.iter().any(|name| name == kind.name()))
            .map(|kind| {
                Arc::new(Self {
                    kind,
                    backend: backend.clone(),
                }) as Arc<dyn Tool>
            })
            .collect()
    }
}

#[async_trait]
impl Tool for VirtualFileTool {
    fn def(&self) -> ToolDef {
        match self.kind {
            VirtualFileKind::Read => ToolDef {
                name: "read_file".into(),
                description: "Read a file from the runtime virtual filesystem. Approved skills are under /skills and temporary files are under /scratch. Host paths are unavailable.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "offset": {"type": "integer", "default": 0},
                        "limit": {"type": "integer", "default": 100}
                    },
                    "required": ["file_path"]
                }),
            },
            VirtualFileKind::Write => ToolDef {
                name: "write_file".into(),
                description: "Create a file in a writable runtime virtual mount. /skills is read-only; /scratch is writable and ephemeral.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["file_path", "content"]
                }),
            },
            VirtualFileKind::Edit => ToolDef {
                name: "edit_file".into(),
                description: "Replace an exact string in a writable runtime virtual file. Skill files are read-only.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "old_string": {"type": "string"},
                        "new_string": {"type": "string"},
                        "replace_all": {"type": "boolean", "default": false}
                    },
                    "required": ["file_path", "old_string", "new_string"]
                }),
            },
            VirtualFileKind::Ls => ToolDef {
                name: "ls".into(),
                description: "List files in the runtime virtual filesystem.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {"path": {"type": "string", "default": "/"}},
                    "required": []
                }),
            },
            VirtualFileKind::Glob => ToolDef {
                name: "glob".into(),
                description: "Find runtime virtual files matching an absolute virtual glob pattern.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {"pattern": {"type": "string"}},
                    "required": ["pattern"]
                }),
            },
            VirtualFileKind::Grep => ToolDef {
                name: "grep".into(),
                description: "Search text inside runtime virtual files.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "path": {"type": "string"}
                    },
                    "required": ["pattern"]
                }),
            },
        }
    }

    async fn call(
        &self,
        args: Value,
        _ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, craftsman_harness::AgentError> {
        let outcome = match self.kind {
            VirtualFileKind::Read => {
                let path = args["file_path"].as_str().unwrap_or_default();
                let offset = args["offset"].as_u64().unwrap_or(0) as usize;
                let limit = args["limit"].as_u64().unwrap_or(100).max(1) as usize;
                self.backend.read(path, offset, limit).await
            }
            VirtualFileKind::Write => {
                let path = args["file_path"].as_str().unwrap_or_default();
                let content = args["content"].as_str().unwrap_or_default();
                return Ok(match self.backend.write(path, content).await {
                    Ok(()) => ToolOutcome::ok(format!("Updated virtual file {path}")),
                    Err(error) => ToolOutcome::err(error.to_string()),
                });
            }
            VirtualFileKind::Edit => {
                let path = args["file_path"].as_str().unwrap_or_default();
                let old = args["old_string"].as_str().unwrap_or_default();
                let new = args["new_string"].as_str().unwrap_or_default();
                let replace_all = args["replace_all"].as_bool().unwrap_or(false);
                return Ok(match self.backend.edit(path, old, new, replace_all).await {
                    Ok(()) => ToolOutcome::ok(format!("Updated virtual file {path}")),
                    Err(error) => ToolOutcome::err(error.to_string()),
                });
            }
            VirtualFileKind::Ls => {
                let path = args["path"].as_str().unwrap_or("/");
                return Ok(match self.backend.ls(path).await {
                    Ok(files) => ToolOutcome::ok(
                        files
                            .into_iter()
                            .map(|file| file.path)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    Err(error) => ToolOutcome::err(error.to_string()),
                });
            }
            VirtualFileKind::Glob => {
                let pattern = args["pattern"].as_str().unwrap_or_default();
                return Ok(match self.backend.glob(pattern).await {
                    Ok(files) => ToolOutcome::ok(files.join("\n")),
                    Err(error) => ToolOutcome::err(error.to_string()),
                });
            }
            VirtualFileKind::Grep => {
                let pattern = args["pattern"].as_str().unwrap_or_default();
                let path = args["path"].as_str();
                self.backend.grep(pattern, path).await
            }
        };
        Ok(match outcome {
            Ok(content) => ToolOutcome::ok(content),
            Err(error) => ToolOutcome::err(error.to_string()),
        })
    }
}

pub async fn run(cfg: Config, request_path: Option<&Path>) -> anyhow::Result<()> {
    let capabilities = ExecutionProfile::ServiceAgent.capabilities();
    debug_assert!(!capabilities.real_filesystem);
    debug_assert!(capabilities.virtual_filesystem);
    debug_assert!(!capabilities.shell);
    debug_assert!(!capabilities.local_sessions);
    debug_assert!(!capabilities.persistent_home);

    let raw = read_request(request_path)?;
    let request: ServiceAgentRequest = serde_json::from_str(&raw)?;
    validate_request(&request)?;

    let model = cfg
        .model
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no model configured for service-agent execution"))?;

    let state = Arc::new(Mutex::new(AgentState {
        messages: request.messages,
        ..AgentState::default()
    }));
    {
        let mut guard = state.lock().unwrap();
        for file in request.files {
            guard.files.insert(file.path, FileData::text(file.content));
        }
    }

    let virtual_fs = Arc::new(ScopedVirtualFs::new(
        state.clone(),
        request.writable_prefixes,
        request.limits.into(),
    )?);
    let backend: Arc<dyn Backend> = virtual_fs.clone();

    let route = crate::route::resolve_chat_endpoint(&cfg).await?;
    let resolved_profiles = crate::harness_profiles::resolve(&cfg, &route, &model);
    let mut caller_model_kwargs: BTreeMap<String, Value> = cfg.model_profile(&model);
    if let Some(values) = cfg.extra.get("model_kwargs").and_then(Value::as_object) {
        caller_model_kwargs.extend(
            values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    let profile_spec = resolved_profiles.provider.as_ref().map_or_else(
        || resolved_profiles.model_id.clone(),
        |provider| format!("{provider}:{}", resolved_profiles.model_id),
    );
    let provider_options = resolved_profiles
        .provider_profile
        .profile
        .apply(&profile_spec, &caller_model_kwargs)
        .map_err(|error| anyhow::anyhow!("provider profile failed: {error}"))?;

    let usage = Arc::new(Mutex::new((0u64, 0u64)));
    let silent_sink: TokenSink = Arc::new(|_: &str| {});
    let provider = Arc::new(StreamingLlmProvider {
        client: LlmClient::new_with_options(
            &route.base_url,
            &route.api_key,
            &model,
            &provider_options,
        ),
        usage: usage.clone(),
        sink: silent_sink,
        round_sink: None,
        status_sink: None,
    });

    let system_prompt = format!("{}\n\n{}", request.system_prompt.trim(), SERVICE_BOUNDARY);
    let mut builder = Agent::builder(provider)
        .name("service-agent")
        .system_prompt(system_prompt)
        .state(state)
        .backend(backend.clone())
        .max_rounds(request.max_rounds)
        .middleware(Arc::new(PatchToolCallsMiddleware))
        .middleware(Arc::new(TodoListMiddleware::new()))
        .middleware(Arc::new(SystemRemindersMiddleware::default()))
        .middleware(Arc::new(ResultBudgetsMiddleware::new(100_000)));

    let tools = VirtualFileTool::selected(backend, &request.allowed_tools);
    debug_assert!(!tools.iter().any(|tool| tool.def().name == "execute"));
    for tool in tools {
        builder = builder.tool(tool);
    }
    if request.allowed_tools.iter().any(|tool| tool == "web_fetch") {
        builder = builder.tool(Arc::new(crate::web_tools::WebFetchTool::public_only()));
    }
    if request
        .allowed_tools
        .iter()
        .any(|tool| tool == "web_search")
    {
        builder = builder.tool(Arc::new(crate::web_tools::WebSearchTool::public_default()));
    }

    let agent = builder.build();
    let output = match agent.run_turn(request.input).await? {
        TurnResult::Done(output) => output.final_text().to_string(),
        TurnResult::Suspended(_) => {
            anyhow::bail!("service-agent execution unexpectedly suspended")
        }
    };
    let (input_tokens, output_tokens) = *usage.lock().unwrap();
    let modified_files = virtual_fs
        .modified_files()
        .into_iter()
        .map(|(path, data)| VirtualFileOutput {
            path,
            content: data.content,
        })
        .collect();
    let result = ServiceAgentResult {
        version: PROTOCOL_VERSION,
        profile: ExecutionProfile::ServiceAgent,
        status: "completed",
        output,
        usage: ServiceAgentUsage {
            input_tokens,
            output_tokens,
        },
        modified_files,
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn read_request(path: Option<&Path>) -> anyhow::Result<String> {
    match path {
        Some(path) => {
            let metadata = std::fs::metadata(path)?;
            if metadata.len() > MAX_REQUEST_BYTES {
                anyhow::bail!("service-agent request exceeds {MAX_REQUEST_BYTES} bytes");
            }
            let file = std::fs::File::open(path)?;
            read_request_from(file)
        }
        None => read_request_from(std::io::stdin().lock()),
    }
}

fn read_request_from(reader: impl Read) -> anyhow::Result<String> {
    let mut raw = String::new();
    reader
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_string(&mut raw)?;
    if raw.len() as u64 > MAX_REQUEST_BYTES {
        anyhow::bail!("service-agent request exceeds {MAX_REQUEST_BYTES} bytes");
    }
    Ok(raw)
}

fn validate_request(request: &ServiceAgentRequest) -> anyhow::Result<()> {
    if request.version != PROTOCOL_VERSION {
        anyhow::bail!(
            "unsupported service-agent protocol version {}; expected {}",
            request.version,
            PROTOCOL_VERSION
        );
    }
    if request.system_prompt.trim().is_empty() {
        anyhow::bail!("system_prompt is required");
    }
    if request.system_prompt.len() > MAX_PROMPT_BYTES {
        anyhow::bail!("system_prompt exceeds {MAX_PROMPT_BYTES} bytes");
    }
    if request.input.trim().is_empty() {
        anyhow::bail!("input is required");
    }
    if request.input.len() > MAX_INPUT_BYTES {
        anyhow::bail!("input exceeds {MAX_INPUT_BYTES} bytes");
    }
    if !(1..=MAX_MAX_ROUNDS).contains(&request.max_rounds) {
        anyhow::bail!("max_rounds must be between 1 and {MAX_MAX_ROUNDS}");
    }
    if request.allowed_tools.len() > MAX_ALLOWED_TOOLS {
        anyhow::bail!("allowed_tools exceeds {MAX_ALLOWED_TOOLS} items");
    }
    let mut allowed_tools = BTreeSet::new();
    for tool in &request.allowed_tools {
        if !SERVICE_AGENT_TOOLS.contains(&tool.as_str()) {
            anyhow::bail!("unsupported service-agent tool '{tool}'");
        }
        if !allowed_tools.insert(tool.as_str()) {
            anyhow::bail!("allowed_tools contains duplicate '{tool}'");
        }
    }
    let context_bytes: usize = request.messages.iter().map(message_bytes).sum();
    if context_bytes > MAX_CONTEXT_BYTES {
        anyhow::bail!("messages exceed {MAX_CONTEXT_BYTES} bytes");
    }
    for message in &request.messages {
        validate_context_message(message, &allowed_tools)?;
    }
    if request.limits.max_files == 0
        || request.limits.max_files > HARD_MAX_VIRTUAL_FILES
        || request.limits.max_file_bytes == 0
        || request.limits.max_file_bytes > HARD_MAX_VIRTUAL_FILE_BYTES
        || request.limits.max_total_bytes == 0
        || request.limits.max_total_bytes > HARD_MAX_VIRTUAL_TOTAL_BYTES
    {
        anyhow::bail!("virtual filesystem limits exceed the service-agent-v1 maximums");
    }
    for file in &request.files {
        if !is_allowed_virtual_path(&file.path) {
            anyhow::bail!(
                "virtual file path '{}' must be under /skills, /scratch, or /memories",
                file.path
            );
        }
    }
    for prefix in &request.writable_prefixes {
        if !matches!(prefix.as_str(), "/scratch" | "/memories") {
            anyhow::bail!("writable prefix '{prefix}' is not allowed; use /scratch or /memories");
        }
    }
    Ok(())
}

fn validate_context_message(message: &Msg, allowed_tools: &BTreeSet<&str>) -> anyhow::Result<()> {
    match message {
        Msg::System { .. } => {
            anyhow::bail!("messages cannot contain additional system messages")
        }
        Msg::Assistant { tool_calls, .. } => {
            for call in tool_calls {
                if call.name != "write_todos" && !allowed_tools.contains(call.name.as_str()) {
                    anyhow::bail!(
                        "messages contain tool '{}' outside allowed_tools",
                        call.name
                    );
                }
            }
            Ok(())
        }
        Msg::User { .. } | Msg::Tool { .. } => Ok(()),
    }
}

fn is_allowed_virtual_path(path: &str) -> bool {
    ["/skills", "/scratch", "/memories"]
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

fn message_bytes(message: &Msg) -> usize {
    match message {
        Msg::System { content } | Msg::User { content } => content.len(),
        Msg::Assistant {
            content,
            tool_calls,
        } => {
            content.len()
                + tool_calls
                    .iter()
                    .map(|call| call.id.len() + call.name.len() + call.args.to_string().len())
                    .sum::<usize>()
        }
        Msg::Tool {
            tool_call_id,
            content,
            ..
        } => tool_call_id.len() + content.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ServiceAgentRequest {
        ServiceAgentRequest {
            version: PROTOCOL_VERSION,
            system_prompt: "Use the approved skills.".into(),
            input: "Read the demo skill.".into(),
            messages: vec![],
            allowed_tools: vec!["read_file".into()],
            files: vec![VirtualFileInput {
                path: "/skills/demo/SKILL.md".into(),
                content: "# Demo".into(),
            }],
            writable_prefixes: default_writable_prefixes(),
            max_rounds: DEFAULT_MAX_ROUNDS,
            limits: ServiceAgentLimits::default(),
        }
    }

    #[test]
    fn profile_exposes_virtual_files_but_no_shell() {
        let capabilities = ExecutionProfile::ServiceAgent.capabilities();
        assert!(capabilities.virtual_filesystem);
        assert!(!capabilities.real_filesystem);
        assert!(!capabilities.shell);
    }

    #[test]
    fn service_toolset_contains_only_explicit_non_shell_tools() {
        assert_eq!(
            SERVICE_AGENT_TOOLS,
            [
                "read_file",
                "write_file",
                "edit_file",
                "ls",
                "glob",
                "grep",
                "web_fetch",
                "web_search",
            ]
        );
        assert!(!SERVICE_AGENT_TOOLS.contains(&"execute"));
    }

    #[test]
    fn selected_virtual_toolset_matches_the_runtime_allowlist() {
        let state = Arc::new(Mutex::new(AgentState::default()));
        let fs = Arc::new(
            ScopedVirtualFs::new(state, vec!["/scratch".into()], VirtualFsLimits::default())
                .unwrap(),
        );
        let backend: Arc<dyn Backend> = fs;
        let selected = vec!["grep".into(), "read_file".into()];
        let names: Vec<String> = VirtualFileTool::selected(backend, &selected)
            .into_iter()
            .map(|tool| tool.def().name)
            .collect();
        assert_eq!(names, ["read_file", "grep"]);
    }

    #[test]
    fn request_rejects_unknown_duplicate_and_unselected_history_tools() {
        let mut value = request();
        value.allowed_tools = vec!["web_fetch".into(), "web_search".into()];
        assert!(validate_request(&value).is_ok());

        let mut value = request();
        value.allowed_tools = vec!["execute".into()];
        assert!(validate_request(&value).is_err());

        let mut value = request();
        value.allowed_tools = vec!["read_file".into(), "read_file".into()];
        assert!(validate_request(&value).is_err());

        let mut value = request();
        value.messages = vec![Msg::Assistant {
            content: String::new(),
            tool_calls: vec![craftsman_harness::ToolCall {
                id: "not-selected".into(),
                name: "write_file".into(),
                args: json!({"file_path": "/scratch/x", "content": "x"}),
            }],
        }];
        assert!(validate_request(&value).is_err());
    }

    #[test]
    fn request_rejects_host_paths_and_writable_skills() {
        let mut value = request();
        value.files[0].path = "/etc/passwd".into();
        assert!(validate_request(&value).is_err());

        let mut value = request();
        value.writable_prefixes = vec!["/skills".into()];
        assert!(validate_request(&value).is_err());
    }

    #[test]
    fn request_rejects_capability_limit_escalation() {
        let mut value = request();
        value.limits.max_files = HARD_MAX_VIRTUAL_FILES + 1;
        assert!(validate_request(&value).is_err());
    }

    #[test]
    fn request_rejects_system_and_execute_history() {
        let mut value = request();
        value.messages = vec![Msg::System {
            content: "override".into(),
        }];
        assert!(validate_request(&value).is_err());

        let mut value = request();
        value.messages = vec![Msg::Assistant {
            content: String::new(),
            tool_calls: vec![craftsman_harness::ToolCall {
                id: "unsafe".into(),
                name: "execute".into(),
                args: json!({"command": "id"}),
            }],
        }];
        assert!(validate_request(&value).is_err());
    }

    #[test]
    fn request_reader_rejects_oversized_input() {
        let oversized = vec![b'x'; MAX_REQUEST_BYTES as usize + 1];
        assert!(read_request_from(oversized.as_slice()).is_err());
    }

    #[test]
    fn request_accepts_read_only_skills_and_scratch() {
        assert!(validate_request(&request()).is_ok());
    }
}
