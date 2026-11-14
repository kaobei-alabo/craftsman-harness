//! Explicit source-scoped verification using existing approvals and ShellRunner.
use crate::{
    config::Config,
    tools::ShellRunner,
    workspace_checkpoint::{self, SourceObservation},
};
use async_trait::async_trait;
use craftsman_harness::{
    AgentError, AgentMiddleware, AgentState, CompletionAction, CompletionAfterCheck,
    CompletionCheck, Tool, ToolCall, ToolCtx, ToolDef, ToolErrorKind, ToolMeta, ToolNext,
    ToolOutcome,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

const POLICY_FILE: &str = ".craftsman/verification.json";
const MAX_EVIDENCE_BYTES: usize = 1024 * 1024;

pub(crate) struct VerificationPlan {
    commands: Vec<String>,
    paths: Vec<String>,
    protected: Vec<String>,
    repair_rounds: u8,
    project_policy: Option<String>,
}

fn declared_paths(value: Option<&Value>, required: bool) -> Result<Vec<String>, &'static str> {
    let values = match value {
        Some(Value::Array(values)) => values,
        None if !required => return Ok(Vec::new()),
        _ => return Err("Verification paths must be a bounded array of relative paths."),
    };
    if values.len() > 64 || required && values.is_empty() {
        return Err("Declare between 1 and 64 verification paths.");
    }
    let mut result = BTreeSet::new();
    for value in values {
        let path = value
            .as_str()
            .ok_or("Verification paths must be strings.")?;
        if path.is_empty() || path.len() > 4096 || path.contains(['\0', '\n', '\r']) {
            return Err("Verification path is invalid.");
        }
        if path == "." {
            result.insert(String::new());
            continue;
        }
        #[cfg(unix)]
        if path.contains('\\') {
            return Err("Verification paths must use native relative components.");
        }
        let normalized = path.replace('\\', "/");
        if normalized
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err("Verification paths must use native relative components.");
        }
        if Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err("Verification paths must stay within the working folder.");
        }
        result.insert(normalized);
    }
    Ok(result.into_iter().collect())
}

impl VerificationPlan {
    pub(crate) fn load(cfg: &Config, root: &Path) -> Result<Option<Self>, &'static str> {
        let (value, project_policy) = if let Some(value) = cfg.extra.get("verification") {
            (value.clone(), None)
        } else {
            let Some(bytes) = workspace_checkpoint::read_verification_policy(root, POLICY_FILE)?
            else {
                return Ok(None);
            };
            let hash = format!("{:x}", Sha256::digest(&bytes));
            (
                serde_json::from_slice(&bytes)
                    .map_err(|_| "Project verification policy must be valid JSON.")?,
                Some(hash),
            )
        };
        let object = value
            .as_object()
            .ok_or("Verification settings must be an object.")?;
        if object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "commands" | "paths" | "protected_paths" | "max_repair_rounds"
            )
        }) {
            return Err("Verification settings contain unsupported fields.");
        }
        let commands = match object.get("commands") {
            None => return Ok(None),
            Some(Value::Array(values)) if values.is_empty() => return Ok(None),
            Some(Value::Array(values)) if values.len() <= 16 => {
                let mut commands = Vec::new();
                let mut bytes = 0usize;
                for value in values {
                    let command = value
                        .as_str()
                        .ok_or("Verification commands must be strings.")?;
                    bytes = bytes.saturating_add(command.len());
                    if command.trim().is_empty()
                        || command.len() > 4096
                        || bytes > 32 * 1024
                        || command.contains('\0')
                    {
                        return Err("Verification command list exceeds its bounds.");
                    }
                    commands.push(command.to_owned());
                }
                commands
            }
            _ => return Err("Declare at most 16 verification commands."),
        };
        let paths = declared_paths(object.get("paths"), true)?;
        let protected = declared_paths(object.get("protected_paths"), false)?;
        if protected.iter().any(|path| {
            !paths.iter().any(|scope| {
                scope.is_empty() || path == scope || path.starts_with(&format!("{scope}/"))
            })
        }) {
            return Err("Protected paths must belong to the declared verification scopes.");
        }
        let repairs = match object.get("max_repair_rounds") {
            None => 3,
            Some(value) => value
                .as_u64()
                .filter(|n| *n <= 3)
                .ok_or("Verification permits at most three repair rounds.")?
                as u8,
        };
        Ok(Some(Self {
            commands,
            paths,
            protected,
            repair_rounds: repairs,
            project_policy,
        }))
    }
}

#[derive(Default)]
struct VerificationState {
    before: Option<Arc<SourceObservation>>,
    passed: Option<String>,
    protected_approval: Option<String>,
    completed: u64,
    scheduled: u64,
    failures: u8,
    last_passed: bool,
    restored: bool,
    issued: u64,
}

pub(crate) struct VerificationReview {
    pub(crate) summary: String,
    pub(crate) details: String,
    pub(crate) protected_changed: bool,
    protected_digest: String,
}

pub(crate) struct VerificationController {
    root: PathBuf,
    plan: VerificationPlan,
    shell: Arc<ShellRunner>,
    excluded: BTreeSet<String>,
    state: Mutex<VerificationState>,
    activity: Option<crate::agent::ActivitySink>,
    approved_project: std::sync::atomic::AtomicBool,
}

impl VerificationController {
    pub(crate) fn reset(&self) {
        *self.state.lock().unwrap() = VerificationState::default();
    }
    pub(crate) fn mark_restored(&self) {
        self.reset();
        self.state.lock().unwrap().restored = true;
    }
    pub(crate) fn new(root: PathBuf, plan: VerificationPlan, shell: Arc<ShellRunner>) -> Self {
        let mut excluded = BTreeSet::from([String::from(".craftsman")]);
        let configuration = crate::config::config_dir();
        if let Ok(relative) = configuration.strip_prefix(&root) {
            if !relative.as_os_str().is_empty() {
                excluded.insert(relative.to_string_lossy().replace('\\', "/"));
            }
        }
        Self {
            root,
            plan,
            shell,
            excluded,
            state: Mutex::new(VerificationState::default()),
            activity: None,
            approved_project: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub(crate) fn with_activity(mut self, activity: Option<crate::agent::ActivitySink>) -> Self {
        self.activity = activity;
        self
    }

    pub(crate) fn status(&self) -> String {
        let state = self.state.lock().unwrap();
        format!(
            "{} declared checks / {} scopes; last result: {}",
            self.plan.commands.len(),
            self.plan.paths.len(),
            if state.restored {
                "restored baseline requires fresh checks"
            } else if state.completed == 0 {
                "not run"
            } else if state.last_passed {
                "passed for its captured source hash"
            } else {
                "did not pass"
            }
        )
    }

    async fn observe(&self) -> Result<SourceObservation, AgentError> {
        let root = self.root.clone();
        let paths = self.plan.paths.clone();
        let excluded = self.excluded.clone();
        let image = tokio::task::spawn_blocking(move || {
            workspace_checkpoint::observe_source(&root, &paths, &excluded)
        })
        .await
        .map_err(|_| state_error("Verification observation could not finish."))?
        .map_err(state_error)?;
        if !image.complete() {
            return Err(state_error(
                "Verification coverage is incomplete; narrow the declared scopes or resolve unavailable paths.",
            ));
        }
        Ok(image)
    }

    async fn ensure_before(&self) -> Result<(), AgentError> {
        if self.state.lock().unwrap().before.is_some() {
            return Ok(());
        }
        let image = self.observe().await?;
        self.state
            .lock()
            .unwrap()
            .before
            .get_or_insert(Arc::new(image));
        Ok(())
    }

    async fn revalidate_policy(&self) -> Result<(), AgentError> {
        if let Some(expected) = &self.plan.project_policy {
            let root = self.root.clone();
            let bytes = tokio::task::spawn_blocking(move || {
                workspace_checkpoint::read_verification_policy(&root, POLICY_FILE)
            })
            .await
            .map_err(|_| state_error("Verification policy validation could not finish."))?
            .map_err(state_error)?;
            if bytes
                .as_ref()
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                .as_ref()
                != Some(expected)
            {
                return Err(state_error(
                    "Project verification policy changed; rebuild the session before running its checks.",
                ));
            }
        }
        Ok(())
    }

    pub(crate) async fn approval_context(&self) -> Result<VerificationReview, AgentError> {
        self.revalidate_policy().await?;
        let image = self.observe().await?;
        let (before, restored) = {
            let state = self.state.lock().unwrap();
            (state.before.clone(), state.restored)
        };
        let (protected_digest, changes) = match before.filter(|_| !restored) {
            Some(before) => before
                .protected_review(&image, &self.plan.protected)
                .map_err(state_error)?,
            None if restored => image
                .protected_contents(&self.plan.protected)
                .map_err(state_error)?,
            None => (String::new(), String::new()),
        };
        let changed = !changes.is_empty();
        let summary = if changed {
            "Review protected test changes and run declared verification"
        } else {
            "Run declared verification checks"
        }
        .to_owned();
        let paths: Vec<_> = self
            .plan
            .paths
            .iter()
            .map(|path| if path.is_empty() { "." } else { path.as_str() })
            .collect();
        let details = format!(
            "Working folder: {}\nDeclared scopes: {}\nChecks:\n{}\n{}",
            self.root.display(),
            paths.join(", "),
            self.plan
                .commands
                .iter()
                .enumerate()
                .map(|(i, command)| format!("{}. {command}", i + 1))
                .collect::<Vec<_>>()
                .join("\n"),
            changes
        );
        if details.len() > MAX_EVIDENCE_BYTES {
            return Err(state_error(
                "Verification review exceeds its display limit.",
            ));
        }
        Ok(VerificationReview {
            summary,
            details,
            protected_changed: changed,
            protected_digest,
        })
    }

    pub(crate) fn needs_protected_approval(&self, review: &VerificationReview) -> bool {
        review.protected_changed
            && self.state.lock().unwrap().protected_approval.as_ref()
                != Some(&review.protected_digest)
    }

    pub(crate) fn needs_project_approval(&self) -> bool {
        self.plan.project_policy.is_some()
            && !self
                .approved_project
                .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn approve_protected(&self, review: &VerificationReview) {
        if self.plan.project_policy.is_some() {
            self.approved_project
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if review.protected_changed {
            self.state.lock().unwrap().protected_approval = Some(review.protected_digest.clone());
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolOutcome, AgentError> {
        if args.as_object().is_none_or(|args| !args.is_empty()) {
            return Ok(ToolOutcome::err(
                "Verification arguments cannot override the declared policy.",
            ));
        }
        self.revalidate_policy().await?;
        let review = self.approval_context().await?;
        if self.needs_project_approval() {
            let mut meta = ToolMeta::new();
            meta.error_kind = Some(ToolErrorKind::NeedsApproval);
            return Ok(ToolOutcome::err(
                "Project verification commands require approval of the declared policy.",
            )
            .with_meta(meta));
        }
        if self.needs_protected_approval(&review) {
            let mut meta = ToolMeta::new();
            meta.error_kind = Some(ToolErrorKind::NeedsApproval);
            return Ok(ToolOutcome::err(
                "Protected test changes require approval for their current contents.",
            )
            .with_meta(meta));
        }
        let before = self.observe().await?;
        let issued = {
            let mut state = self.state.lock().unwrap();
            state.issued = state
                .issued
                .checked_add(1)
                .ok_or_else(|| state_error("Verification receipt counter is unavailable."))?;
            state.issued
        };
        let mut evidence = String::new();
        let mut passed = true;
        let mut exit = Some(0);
        let mut cancelled = false;
        for (index, command) in self.plan.commands.iter().enumerate() {
            if craftsman_harness::TurnCancellation::current()
                .is_some_and(|turn| turn.is_cancelled())
            {
                cancelled = true;
                passed = false;
                break;
            }
            if let Some(activity) = &self.activity {
                activity(&format!(
                    "Verification check {} of {} started: {}",
                    index + 1,
                    self.plan.commands.len(),
                    command
                ));
            }
            let outcome = self.shell.run_outcome(command, None).await;
            let status = outcome.meta.exit_code;
            if outcome.is_error || status != Some(0) {
                passed = false;
                exit = status;
            }
            let section = format!(
                "\nCheck: {command}\nExit: {}\n{}\n",
                status.map_or_else(|| "unknown".into(), |code| code.to_string()),
                outcome.content
            );
            if evidence.len().saturating_add(section.len()) > MAX_EVIDENCE_BYTES {
                passed = false;
                evidence.push_str(
                    "\nVerification evidence exceeds its limit; no passing result is accepted.\n",
                );
                break;
            }
            evidence.push_str(&section);
            if outcome.meta.error_kind == Some(ToolErrorKind::Cancelled) {
                cancelled = true;
                passed = false;
                break;
            }
        }
        let after = self.observe().await;
        match &after {
            Ok(after) if before.digest() == after.digest() => {}
            Ok(_) => {
                passed = false;
                evidence.push_str("\nDeclared sources changed while checks ran; their result is not accepted for the new contents.\n");
            }
            Err(_) => {
                passed = false;
                evidence.push_str("\nDeclared sources could not be observed after checks; their result is not accepted.\n");
            }
        }
        let mut state = self.state.lock().unwrap();
        state.completed = issued;
        state.last_passed = passed;
        if passed {
            state.passed = Some(after.as_ref().unwrap().digest().into());
            state.restored = false;
        } else {
            state.failures = state.failures.saturating_add(1);
            state.passed = None;
        }
        drop(state);
        let text = format!(
            "Verification {}.{}",
            if passed { "passed" } else { "did not pass" },
            evidence
        );
        let mut meta = ToolMeta::new();
        meta.exit_code = exit;
        if cancelled {
            meta.error_kind = Some(ToolErrorKind::Cancelled);
        }
        Ok(if passed {
            ToolOutcome::ok(text)
        } else {
            ToolOutcome::err(text)
        }
        .with_meta(meta))
    }
}

fn state_error(text: &'static str) -> AgentError {
    AgentError::State(text.into())
}

#[async_trait]
impl AgentMiddleware for VerificationController {
    fn name(&self) -> &str {
        "verification"
    }
    async fn turn_start(&self, _: &mut AgentState) -> Result<(), AgentError> {
        let mut state = self.state.lock().unwrap();
        let restored = state.restored;
        *state = VerificationState {
            restored,
            ..VerificationState::default()
        };
        Ok(())
    }
    async fn prepare_model(&self, state: &mut AgentState) -> Result<(), AgentError> {
        state.system_extras.push("Declared verification is enabled. Use the verify tool for actual check results. Existing protected paths require approval of their current changes. Completion is checked against the declared source scopes.".into());
        Ok(())
    }
    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        let readonly = matches!(
            call.name.as_str(),
            "verify"
                | "read_file"
                | "ls"
                | "glob"
                | "grep"
                | "write_todos"
                | "read_todos"
                | "ask_user"
                | "render_ui"
        ) || call.name.starts_with("lsp_");
        if !readonly {
            self.ensure_before().await?;
        }
        next.run(call, ctx).await
    }
}

#[async_trait]
impl CompletionCheck for VerificationController {
    async fn before_finish(&self, _: &AgentState) -> Result<CompletionAction, AgentError> {
        let (before, passed, restored) = {
            let state = self.state.lock().unwrap();
            (state.before.clone(), state.passed.clone(), state.restored)
        };
        if before.is_none() && !restored {
            return Ok(CompletionAction::Ready);
        }
        let current = self.observe().await?;
        if !restored
            && before
                .as_ref()
                .is_some_and(|before| current.digest() == before.digest())
            || passed.as_deref() == Some(current.digest())
        {
            return Ok(CompletionAction::Ready);
        }
        let mut state = self.state.lock().unwrap();
        state.scheduled = state.completed;
        Ok(CompletionAction::Check {
            tool: "verify".into(),
            args: json!({}),
        })
    }
    async fn after_check(&self, _: &AgentState) -> Result<CompletionAfterCheck, AgentError> {
        let state = self.state.lock().unwrap();
        if state.completed == state.scheduled {
            return Ok(CompletionAfterCheck::Stop { reason: "Required verification did not run; review its permissions or policy before continuing.".into() });
        }
        if !state.last_passed && state.failures > self.plan.repair_rounds {
            return Ok(CompletionAfterCheck::Stop { reason: "Verification still failed after the allowed repair rounds. Completed work and actual check results are retained.".into() });
        }
        Ok(CompletionAfterCheck::Continue)
    }
}

pub(crate) struct VerifyTool(pub(crate) Arc<VerificationController>);
#[async_trait]
impl Tool for VerifyTool {
    fn def(&self) -> ToolDef {
        ToolDef { name: "verify".into(), description: "Run the declared verification commands against their scoped sources. Returns actual check results; protected changes require approval.".into(), parameters: json!({"type":"object","properties":{},"additionalProperties":false}) }
    }
    async fn call(&self, args: Value, _: &mut ToolCtx<'_>) -> Result<ToolOutcome, AgentError> {
        self.0.execute(args).await
    }
}

#[cfg(test)]
#[path = "verification/tests.rs"]
mod tests;
