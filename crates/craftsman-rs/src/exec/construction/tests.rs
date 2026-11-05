use super::*;
use craftsman_harness::{Backend, Checkpointer, Msg};
use serde_json::json;

struct Fixture {
    root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        let config = root.path().join("config");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&config).unwrap();
        std::fs::write(
            config.join("config.json"),
            json!({"model":"mock-model","api_key":"synthetic-key"}).to_string(),
        )
        .unwrap();
        Self {
            root,
            workspace,
            config,
        }
    }
    fn options(&self, session: SessionSelection) -> Options {
        Options {
            workspace: self.workspace.clone(),
            config_dir: self.config.clone(),
            trust_workspace: false,
            session,
            selectors: Selectors::default(),
        }
    }
}

#[tokio::test]
async fn ephemeral_preparation_uses_only_explicit_config_and_creates_no_storage() {
    let f = Fixture::new();
    let p = Prepared::prepare(f.options(SessionSelection::Ephemeral))
        .await
        .unwrap();
    assert!(p.persistence().thread_id().is_none());
    assert!(p.persistence().store().is_none());
    assert!(p.persistence().journal_dir().is_none());
    assert!(!f.config.join("threads.db").exists());
    assert!(!f.config.join("sessions-rs").exists());
    assert_eq!(p.workspace(), f.workspace.canonicalize().unwrap());
    assert_eq!(p.config_dir(), f.config.canonicalize().unwrap());
    assert_eq!(p.config().model.as_deref(), Some("mock-model"));
    assert!(p.initial_state().messages.is_empty());
    assert!(p.filesystem().write("not-approved", "no").await.is_err());
    assert!(f.root.path().join("config/config.json").is_file());
}

#[tokio::test]
async fn fresh_preparation_creates_an_os_random_scoped_thread_in_the_explicit_store() {
    let f = Fixture::new();
    let p = Prepared::prepare(f.options(SessionSelection::Fresh))
        .await
        .unwrap();
    let id = p.persistence().thread_id().unwrap();
    assert!(id.starts_with("thread-") && id.len() == 39);
    let store = p.persistence().store().unwrap();
    let rows = store.list_threads().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(
        rows[0].project_contexts,
        [f.workspace.canonicalize().unwrap().to_string_lossy()]
    );
    assert!(f.config.join("threads.db").is_file());
    assert!(!f.config.join("sessions-rs").exists());
    assert_eq!(
        p.persistence().journal_dir(),
        Some(p.config_dir().join("sessions-rs").as_path())
    );
    assert!(matches!(
        p.initial_state(),
        craftsman_harness::AgentState { .. }
    ));
}

fn write(base: &Path, relative: &str, text: &str) {
    let path = base.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn plugin(name: &str, prompt: &str) -> String {
    json!({"version":1,"name":name,"prompt":prompt}).to_string()
}
fn error(result: Result<Prepared>, expected: PreparationError) {
    assert_eq!(result.err().unwrap(), expected);
}

#[tokio::test]
async fn project_instructions_skills_plugins_need_trust_and_config_discovery_is_explicit() {
    let f = Fixture::new();
    write(
        &f.config,
        "agents/craftsman/AGENTS.md",
        "USER-AGENT-CONTEXT",
    );
    write(
        &f.config,
        "skills/user/SKILL.md",
        "---\nname: user-skill\ndescription: user\n---\nUSER-SKILL-GUIDANCE",
    );
    write(
        &f.config,
        "agents/craftsman/skills/agent/SKILL.md",
        "---\nname: agent-skill\n---\nAGENT-SKILL-GUIDANCE",
    );
    write(
        &f.config,
        "plugins/global.json",
        &plugin("shared", "USER-PLUGIN"),
    );
    write(&f.workspace, ".craftsman/AGENTS.md", "PROJECT-INSTRUCTION");
    write(
        &f.workspace,
        ".craftsman/skills/project/SKILL.md",
        "---\nname: project-skill\n---\nPROJECT-SKILL",
    );
    write(
        &f.workspace,
        ".agents/skills/shared/SKILL.md",
        "---\nname: user-skill\n---\nPROJECT-OVERRIDE",
    );
    write(
        &f.workspace,
        ".craftsman/plugins/project.json",
        &plugin("shared", "PROJECT-PLUGIN"),
    );
    // A home-style lookalike inside the explicit config directory is not a fallback root.
    write(
        &f.config,
        ".agents/skills/ambient/SKILL.md",
        "---\nname: ambient-skill\n---\nWRONG-FALLBACK",
    );
    let prepared = Prepared::prepare(f.options(SessionSelection::Ephemeral))
        .await
        .unwrap();
    let block = prepared.discovery().context_block();
    assert!(
        block.contains("USER-AGENT-CONTEXT")
            && block.contains("USER-SKILL-GUIDANCE")
            && block.contains("USER-PLUGIN")
    );
    assert!(
        !block.contains("PROJECT-INSTRUCTION")
            && !block.contains("PROJECT-SKILL")
            && !block.contains("WRONG-FALLBACK")
    );
    assert!(
        !block.contains("SKILL.md:"),
        "provided guidance must not advertise a hard-denied config file path"
    );
    assert!(!prepared.discovery().trust_workspace);
    let mut options = f.options(SessionSelection::Ephemeral);
    options.trust_workspace = true;
    let prepared = Prepared::prepare(options).await.unwrap();
    let block = prepared.discovery().context_block();
    assert!(
        block.contains("PROJECT-INSTRUCTION")
            && block.contains("PROJECT-SKILL")
            && block.contains("PROJECT-PLUGIN")
            && block.contains("PROJECT-OVERRIDE")
    );
    assert!(!block.contains("USER-PLUGIN") && !block.contains("USER-SKILL-GUIDANCE"));
    assert!(prepared.discovery().trust_workspace);
}

#[tokio::test]
async fn ambient_permissions_services_and_shell_overrides_do_not_widen_preparation() {
    let f = Fixture::new();
    write(&f.config,"config.json",&json!({"model":"mock-model","api_key":"${HARMLESS_PROVIDER_NAME}","permission_mode":"bypass-permissions","auto_approve":true,"auto_approve_web":true,
        "shell_allow_list":["sh"],"shell":{"env_scrub":false,"env_pass":["SECRET_TOKEN"],"timeout_secs":90},
        "mcp_servers":[{"env":{"AUTH":"${ANOTHER_HARMLESS_NAME}"}}],"lsp_servers":{"rust":{"command":"must-not-start"}},"max_parallel_agents":12}).to_string());
    let mut options = f.options(SessionSelection::Ephemeral);
    options.selectors.env_pass = vec!["CI_JOB_ID".into()];
    let p = Prepared::prepare_with_environment_for_tests(
        options,
        std::collections::BTreeMap::from([
            (
                "HARMLESS_PROVIDER_NAME".into(),
                "synthetic-provider-credential".into(),
            ),
            (
                "ANOTHER_HARMLESS_NAME".into(),
                "synthetic-disabled-service-credential".into(),
            ),
        ]),
    )
    .await
    .unwrap();
    assert!(p.ignored_ambient_settings());
    assert_eq!(
        p.permissions().mode(),
        super::super::permissions::Mode::Manual
    );
    assert!(
        !p.config().auto_approve
            && !p.config().auto_approve_web()
            && p.config().shell_allow_list.is_empty()
    );
    assert!(p.config().mcp_servers.is_empty() && p.config().lsp_servers.is_empty());
    assert_eq!(p.config().max_parallel_agents, 1);
    assert!(p.shell().env_scrub);
    assert_eq!(p.shell().env_pass, ["CI_JOB_ID"]);
    assert_eq!(p.shell().timeout_secs, 90);
    assert!(
        p.shell()
            .secret_vars
            .contains(&"HARMLESS_PROVIDER_NAME".into())
            && p.shell()
                .secret_vars
                .contains(&"ANOTHER_HARMLESS_NAME".into())
    );
    let reparsed = ShellSettings::from_config(p.config());
    assert!(reparsed.env_scrub);
    assert_eq!(reparsed.env_pass, ["CI_JOB_ID"]);
    let mut options = f.options(SessionSelection::Ephemeral);
    options.selectors.env_pass = vec!["HARMLESS_PROVIDER_NAME".into()];
    error(
        Prepared::prepare(options).await,
        PreparationError::Permissions,
    );
    assert!(!f.config.join("threads.db").exists());
}

#[tokio::test]
async fn invalid_inputs_fail_before_any_thread_or_provider_side_effect() {
    let f = Fixture::new();
    write(&f.config, "config.json", "{invalid json");
    error(
        Prepared::prepare(f.options(SessionSelection::Fresh)).await,
        PreparationError::Config,
    );
    write(
        &f.config,
        "config.json",
        &json!({"model":"mock","agent":"../../escape"}).to_string(),
    );
    error(
        Prepared::prepare(f.options(SessionSelection::Fresh)).await,
        PreparationError::Agent,
    );
    write(
        &f.config,
        "config.json",
        &json!({"model":"mock","agent":"craftsman"}).to_string(),
    );
    let mut options = f.options(SessionSelection::Fresh);
    options.selectors.allow = vec!["tool:unknown".into()];
    error(
        Prepared::prepare(options).await,
        PreparationError::Permissions,
    );
    let mut options = f.options(SessionSelection::Fresh);
    options.config_dir = PathBuf::from(".");
    error(Prepared::prepare(options).await, PreparationError::Scope);
    write(&f.config, "config.json", "{}");
    error(
        Prepared::prepare(f.options(SessionSelection::Fresh)).await,
        PreparationError::NoModel,
    );
    for agent in ["NUL", "COM1", "folder.", ".env-agent", "bad\nname"] {
        write(
            &f.config,
            "config.json",
            &json!({"model":"mock", "agent":agent}).to_string(),
        );
        error(
            Prepared::prepare(f.options(SessionSelection::Fresh)).await,
            PreparationError::Agent,
        );
    }
    assert!(!f.config.join("threads.db").exists());
    assert!(!f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn missing_config_is_not_materialized_and_bom_is_accepted() {
    let f = Fixture::new();
    let mut options = f.options(SessionSelection::Ephemeral);
    options.config_dir = f.root.path().join("missing/cfg");
    error(Prepared::prepare(options).await, PreparationError::NoModel);
    assert!(!f.root.path().join("missing").exists());
    write(
        &f.config,
        "config.json",
        &format!("\u{feff}{}", json!({"model":"mock"})),
    );
    assert!(
        Prepared::prepare(f.options(SessionSelection::Ephemeral))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn bounded_sources_skip_oversize_roots_and_never_materialize_authoring_sdk() {
    let f = Fixture::new();
    write(
        &f.config,
        "skills/build/SKILL.md",
        "---\nname: build-gadget\n---\nMUST-NOT-ADVERTISE-BUILD-GUIDE",
    );
    write(
        &f.config,
        "skills/large/SKILL.md",
        &"x".repeat(256 * 1024 + 1),
    );
    write(
        &f.config,
        "agents/craftsman/AGENTS.md",
        &"y".repeat(256 * 1024 + 1),
    );
    for i in 0..65 {
        write(
            &f.config,
            &format!("plugins/{i:03}.json"),
            &plugin(&format!("p-{i}"), "too many"),
        );
    }
    let p = Prepared::prepare(f.options(SessionSelection::Ephemeral))
        .await
        .unwrap();
    assert!(p.discovery().skipped_sources >= 3);
    assert!(p.discovery().extensions.is_empty());
    assert!(
        !p.discovery()
            .context_block()
            .contains("MUST-NOT-ADVERTISE-BUILD-GUIDE")
    );
    assert!(!f.config.join("gadgets/.sdk").exists());
    assert!(!f.config.join("threads.db").exists());
}

#[tokio::test]
async fn fresh_identity_collisions_do_not_overwrite_existing_threads() {
    let f = Fixture::new();
    let p = Prepared::prepare(f.options(SessionSelection::Fresh))
        .await
        .unwrap();
    let store = p.persistence().store().unwrap();
    let id = p.persistence().thread_id().unwrap();
    assert!(!store.create_headless_thread(id).await.unwrap());
    assert_eq!(store.list_threads().await.unwrap().len(), 1);
    assert_eq!(p.persistence().mode(), &SessionSelection::Fresh);
    let p2 = Prepared::prepare(f.options(SessionSelection::Fresh))
        .await
        .unwrap();
    assert_ne!(p2.persistence().thread_id(), Some(id));
    assert_eq!(
        p2.persistence()
            .store()
            .unwrap()
            .list_threads()
            .await
            .unwrap()
            .len(),
        2
    );
}

async fn seeded_store(f: &Fixture) -> LibsqlCheckpointer {
    LibsqlCheckpointer::open_at_for_headless(
        f.config.join("threads.db"),
        f.workspace
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    )
    .await
    .unwrap()
}
use crate::checkpoints::LibsqlCheckpointer;

#[tokio::test]
async fn continue_uses_updated_desc_then_id_asc_and_never_selects_other_workspaces() {
    let f = Fixture::new();
    let store = seeded_store(&f).await;
    for id in ["z-thread", "a-thread"] {
        assert!(store.create_headless_thread(id).await.unwrap());
    }
    let conn = store.connection();
    conn.execute("UPDATE threads SET updated=100", ())
        .await
        .unwrap();
    let other = f.root.path().join("other");
    std::fs::create_dir(&other).unwrap();
    store
        .set_thread_project_contexts(
            "foreign",
            &[other.canonicalize().unwrap().to_string_lossy().into_owned()],
            "unused",
        )
        .await
        .unwrap();
    conn.execute("UPDATE threads SET updated=10000 WHERE id='foreign'", ())
        .await
        .unwrap();
    let p = Prepared::prepare(f.options(SessionSelection::Continue))
        .await
        .unwrap();
    assert_eq!(p.persistence().thread_id(), Some("a-thread"));
    assert_eq!(p.persistence().mode(), &SessionSelection::Continue);
    conn.execute("UPDATE threads SET updated=101 WHERE id='z-thread'", ())
        .await
        .unwrap();
    let p = Prepared::prepare(f.options(SessionSelection::Continue))
        .await
        .unwrap();
    assert_eq!(p.persistence().thread_id(), Some("z-thread"));
}

#[tokio::test]
async fn resume_requires_canonical_primary_scope_and_preserves_legacy_metadata_on_failure() {
    let f = Fixture::new();
    let store = seeded_store(&f).await;
    store.create_headless_thread("valid").await.unwrap();
    let conn = store.connection();
    conn.execute("INSERT INTO threads(id,created,updated,title,workspace) VALUES('unscoped',1,99,'legacy','legacy-workspace')",()).await.unwrap();
    conn.execute("INSERT INTO threads(id,created,updated,title,workspace) VALUES('inconsistent',1,98,'legacy','wrong')",()).await.unwrap();
    conn.execute("INSERT INTO thread_project_contexts(thread_id,path,position) SELECT 'inconsistent',path,0 FROM thread_project_contexts WHERE thread_id='valid'",()).await.unwrap();
    error(
        Prepared::prepare(f.options(SessionSelection::Resume("unscoped".into()))).await,
        PreparationError::WorkspaceMismatch,
    );
    error(
        Prepared::prepare(f.options(SessionSelection::Resume("inconsistent".into()))).await,
        PreparationError::WorkspaceMismatch,
    );
    error(
        Prepared::prepare(f.options(SessionSelection::Resume("missing".into()))).await,
        PreparationError::MissingSession,
    );
    error(
        Prepared::prepare(f.options(SessionSelection::Resume("../invalid".into()))).await,
        PreparationError::MissingSession,
    );
    let mut rows = conn
        .query("SELECT workspace FROM threads WHERE id='unscoped'", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap(),
        "legacy-workspace"
    );
    assert!(
        Prepared::prepare(f.options(SessionSelection::Resume("valid".into())))
            .await
            .is_ok()
    );
    assert!(
        Prepared::prepare(f.options(SessionSelection::Fresh))
            .await
            .is_ok()
    );
    let mut rows = conn
        .query("SELECT workspace FROM threads WHERE id='unscoped'", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap(),
        "legacy-workspace"
    );
}

#[tokio::test]
async fn resume_restores_conversation_not_memory_permissions_or_pending_approval_execution() {
    use craftsman_harness::{AgentState, ToolCall};
    let f = Fixture::new();
    let store = seeded_store(&f).await;
    store.create_headless_thread("saved").await.unwrap();
    let mut state = AgentState::default();
    state.messages.push(Msg::user("prior input"));
    state.messages.push(Msg::Assistant {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "pending".into(),
            name: "execute".into(),
            args: json!({"command":"must not execute"}),
        }],
    });
    state.custom.insert("auto_approve".into(), json!(true));
    state.system_extras.push("stale permissions".into());
    state.files.insert(
        "/memories/secret".into(),
        craftsman_harness::FileData::text("not a headless mount"),
    );
    store.save("saved", 1, &state, None).await.unwrap();
    let p = Prepared::prepare(f.options(SessionSelection::Resume("saved".into())))
        .await
        .unwrap();
    let state = p.initial_state();
    assert!(state.custom.is_empty() && state.system_extras.is_empty() && state.files.is_empty());
    assert!(state.messages.iter().any(|m|matches!(m,Msg::Tool {tool_call_id,content,is_error:true} if tool_call_id=="pending" && content.contains("not executed"))));
    assert!(!p.config().auto_approve);
    assert!(p.filesystem().write("new", "no").await.is_err());
    assert!(!f.workspace.join("new").exists());
    assert!(!f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn continue_missing_or_foreign_only_store_does_not_create_a_conversation() {
    let f = Fixture::new();
    error(
        Prepared::prepare(f.options(SessionSelection::Continue)).await,
        PreparationError::MissingSession,
    );
    assert!(!f.config.join("threads.db").exists());
    let store = seeded_store(&f).await;
    let other = f.root.path().join("other");
    std::fs::create_dir(&other).unwrap();
    store
        .set_thread_project_contexts(
            "foreign",
            &[other.canonicalize().unwrap().to_string_lossy().into_owned()],
            "unused",
        )
        .await
        .unwrap();
    error(
        Prepared::prepare(f.options(SessionSelection::Continue)).await,
        PreparationError::MissingSession,
    );
    let mut rows = store
        .connection()
        .query("SELECT COUNT(*) FROM threads", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        1
    );
}

#[tokio::test]
async fn unknown_database_layout_is_rejected_instead_of_migrated_or_stamped() {
    let f = Fixture::new();
    let path = f.config.join("threads.db");
    let db = libsql::Builder::new_local(&path).build().await.unwrap();
    let conn = db.connect().unwrap();
    conn.execute(
        "CREATE TABLE checkpoints (foreign_key TEXT,payload TEXT)",
        (),
    )
    .await
    .unwrap();
    error(
        Prepared::prepare(f.options(SessionSelection::Fresh)).await,
        PreparationError::Schema,
    );
    error(
        Prepared::prepare(f.options(SessionSelection::Resume("legacy".into()))).await,
        PreparationError::Schema,
    );
    let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
    let mut rows = conn
        .query("PRAGMA table_info(checkpoints)", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(1)
            .unwrap(),
        "foreign_key"
    );
}

#[tokio::test]
async fn malformed_or_oversized_saved_state_does_not_trigger_writer_initialization() {
    let f = Fixture::new();
    let store = seeded_store(&f).await;
    store.create_headless_thread("saved").await.unwrap();
    let conn = store.connection();
    conn.execute(
        "INSERT INTO checkpoints(thread_id,seq,state_json) VALUES('saved',1,'invalid-json')",
        (),
    )
    .await
    .unwrap();
    error(
        Prepared::prepare(f.options(SessionSelection::Resume("saved".into()))).await,
        PreparationError::Checkpoint,
    );
    conn.execute(
        "UPDATE checkpoints SET state_json=zeroblob(16777217) WHERE thread_id='saved'",
        (),
    )
    .await
    .unwrap();
    error(
        Prepared::prepare(f.options(SessionSelection::Resume("saved".into()))).await,
        PreparationError::Checkpoint,
    );
    assert_eq!(store.list_threads().await.unwrap().len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn source_symlinks_and_store_aliases_are_not_followed_and_ephemeral_never_opens_the_store() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f.root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    write(
        &outside,
        "SKILL.md",
        "---\nname: leak\n---\nOUTSIDE-CONTENT",
    );
    std::fs::create_dir_all(f.config.join("skills")).unwrap();
    symlink(&outside, f.config.join("skills/link")).unwrap();
    write(&f.config, "plugins/link.json", &plugin("inside", "safe"));
    std::fs::remove_file(f.config.join("plugins/link.json")).unwrap();
    write(&outside, "plugin.json", &plugin("leak", "OUTSIDE-CONTENT"));
    symlink(
        outside.join("plugin.json"),
        f.config.join("plugins/link.json"),
    )
    .unwrap();
    symlink(outside.join("not-a-database"), f.config.join("threads.db")).unwrap();
    let p = Prepared::prepare(f.options(SessionSelection::Ephemeral))
        .await
        .unwrap();
    assert!(!p.discovery().context_block().contains("OUTSIDE-CONTENT"));
    error(
        Prepared::prepare(f.options(SessionSelection::Fresh)).await,
        PreparationError::Storage,
    );
    assert!(!outside.join("not-a-database").exists());
    std::fs::remove_file(f.config.join("config.json")).unwrap();
    symlink(outside.join("config.json"), f.config.join("config.json")).unwrap();
    error(
        Prepared::prepare(f.options(SessionSelection::Ephemeral)).await,
        PreparationError::Config,
    );
}

#[cfg(feature = "gadgets")]
#[tokio::test]
async fn manifest_only_scoped_gadgets_coalesce_migration_origins_without_running_guest_code() {
    let f = Fixture::new();
    write(
        &f.config,
        "config.json",
        &json!({"model":"mock","gadgets":{"enabled":true}}).to_string(),
    );
    write(
        &f.config,
        "plugins/original.json",
        &plugin("review-flow", "LEGACY-PROMPT"),
    );
    write(
        &f.config,
        "gadgets/review-flow/gadget.toml",
        "id='review-flow'\nversion='0.1.0'\nprompt='GADGET-PROMPT'\n[migration]\nplugin_name='review-flow'\norder=0\n",
    );
    // Invalid guest bytes are data here: the loader validates metadata only,
    // never compiles, instantiates or calls describe.
    write(
        &f.config,
        "gadgets/review-flow/gadget.wasm",
        "not guest executable code",
    );
    let p = Prepared::prepare(f.options(SessionSelection::Ephemeral))
        .await
        .unwrap();
    assert_eq!(p.discovery().extensions.len(), 1);
    let block = p.discovery().context_block();
    assert!(block.contains("GADGET-PROMPT") && !block.contains("LEGACY-PROMPT"));
    assert!(!f.config.join("gadgets/.grants.json").exists());
    assert!(!f.config.join("gadgets/.sdk").exists());
}

#[tokio::test]
async fn explicit_config_directories_do_not_share_models_skills_or_thread_catalogs() {
    let a = Fixture::new();
    let b = Fixture::new();
    write(
        &a.config,
        "config.json",
        &json!({"model":"model-a"}).to_string(),
    );
    write(
        &b.config,
        "config.json",
        &json!({"model":"model-b","agent":"Custom_Agent"}).to_string(),
    );
    write(
        &a.config,
        "skills/a/SKILL.md",
        "---\nname: a-only\n---\nA-GUIDANCE",
    );
    write(
        &b.config,
        "agents/Custom_Agent/skills/b/SKILL.md",
        "---\nname: b-only\n---\nB-GUIDANCE",
    );
    let pa = Prepared::prepare(a.options(SessionSelection::Fresh))
        .await
        .unwrap();
    let pb = Prepared::prepare(b.options(SessionSelection::Ephemeral))
        .await
        .unwrap();
    assert_eq!(pa.config().model.as_deref(), Some("model-a"));
    assert_eq!(pb.config().model.as_deref(), Some("model-b"));
    assert!(
        pa.discovery().context_block().contains("A-GUIDANCE")
            && !pa.discovery().context_block().contains("B-GUIDANCE")
    );
    assert!(
        pb.discovery().context_block().contains("B-GUIDANCE")
            && !pb.discovery().context_block().contains("A-GUIDANCE")
    );
    assert!(a.config.join("threads.db").exists() && !b.config.join("threads.db").exists());
    error(
        Prepared::prepare(b.options(SessionSelection::Resume(
            pa.persistence().thread_id().unwrap().into(),
        )))
        .await,
        PreparationError::MissingSession,
    );
}

#[tokio::test]
async fn additional_project_contexts_do_not_widen_the_confined_primary_workspace() {
    let f = Fixture::new();
    let store = seeded_store(&f).await;
    let other = f.root.path().join("other");
    std::fs::create_dir(&other).unwrap();
    write(&other, "secret.txt", "OTHER-WORKSPACE");
    let primary = f
        .workspace
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    store
        .set_thread_project_contexts(
            "multi",
            &[
                primary,
                other.canonicalize().unwrap().to_string_lossy().into_owned(),
            ],
            "unused",
        )
        .await
        .unwrap();
    let p = Prepared::prepare(f.options(SessionSelection::Resume("multi".into())))
        .await
        .unwrap();
    assert!(
        p.filesystem()
            .read(&other.join("secret.txt").to_string_lossy(), 0, 10)
            .await
            .is_err()
    );
    assert_eq!(p.workspace(), f.workspace.canonicalize().unwrap());
    let conn = store.connection();
    conn.execute(
        "DELETE FROM thread_project_contexts WHERE thread_id='multi' AND position=0",
        (),
    )
    .await
    .unwrap();
    error(
        Prepared::prepare(f.options(SessionSelection::Resume("multi".into()))).await,
        PreparationError::WorkspaceMismatch,
    );
}

#[cfg(unix)]
#[tokio::test]
async fn preparation_retains_config_source_authority_even_without_the_gadget_feature() {
    let f = Fixture::new();
    write(&f.config, "agents/craftsman/AGENTS.md", "ORIGINAL-CONTEXT");
    let p = Prepared::prepare(f.options(SessionSelection::Ephemeral))
        .await
        .unwrap();
    let moved = f.root.path().join("moved-config");
    std::fs::rename(&f.config, &moved).unwrap();
    write(&f.config, "agents/craftsman/AGENTS.md", "REBOUND-CONTEXT");
    let bytes = p
        .source_roots()
        .config
        .as_ref()
        .unwrap()
        .read_optional(&["agents", "craftsman", "AGENTS.md"], 256 * 1024)
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"ORIGINAL-CONTEXT");
    assert!(p.discovery().context_block().contains("ORIGINAL-CONTEXT"));
    assert!(!moved.join("threads.db").exists());
}

#[tokio::test]
async fn preparation_source_workspace_handle_is_the_same_filesystem_authority_not_a_trust_grant() {
    use cap_fs_ext::MetadataExt;
    let f = Fixture::new();
    let p = Prepared::prepare(f.options(SessionSelection::Ephemeral))
        .await
        .unwrap();
    let source = p
        .source_roots()
        .workspace
        .clone_directory()
        .unwrap()
        .dir_metadata()
        .unwrap();
    let backend = p
        .filesystem()
        .discovery_directory()
        .unwrap()
        .dir_metadata()
        .unwrap();
    assert_eq!((source.dev(), source.ino()), (backend.dev(), backend.ino()));
    assert!(!p.discovery().trust_workspace);
    assert!(!f.config.join("gadgets/.grants.json").exists());
}

#[tokio::test]
async fn captured_credentials_keep_provider_and_disabled_source_values_with_scoped_route() {
    let f = Fixture::new();
    write(&f.config,"config.json",&json!({"model":"mock-model","base_url":"http://synthetic-route", "api_key":"prefix-${LANG}",
        "mcp_servers":[{"headers":{"AUTH":"Bearer ${DISABLED_AUTH}"},"env":{"AUTH":"literal-disabled-value"}}]}).to_string());
    let p = Prepared::prepare_with_environment_for_tests(
        f.options(SessionSelection::Ephemeral),
        std::collections::BTreeMap::from([
            ("LANG".into(), "synthetic-provider-value".into()),
            ("DISABLED_AUTH".into(), "synthetic-disabled-value".into()),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(
        p.credentials().route().api_key,
        "prefix-synthetic-provider-value"
    );
    assert_eq!(p.credentials().route().base_url, "http://synthetic-route");
    assert_eq!(
        p.config().api_key,
        "prefix-${LANG}",
        "do not rewrite routing config or lose shell source names"
    );
    assert!(p.config().mcp_servers.is_empty());
    assert!(
        p.shell().secret_vars.contains(&"LANG".into())
            && p.shell().secret_vars.contains(&"DISABLED_AUTH".into())
    );
    for key in [
        "prefix-synthetic-provider-value",
        "synthetic-provider-value",
        "synthetic-disabled-value",
        "literal-disabled-value",
    ] {
        assert_eq!(p.credentials().redactor().text(key).unwrap(), "[REDACTED]");
    }
    assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
}

#[tokio::test]
async fn missing_invalid_or_oversized_credentials_fail_fixed_before_any_store_side_effect() {
    let f = Fixture::new();
    for key in ["${MISSING_TEST_CREDENTIAL}", "literal-sensitive-${", "R"] {
        write(
            &f.config,
            "config.json",
            &json!({"model":"mock","api_key":key}).to_string(),
        );
        error(
            Prepared::prepare_with_environment_for_tests(
                f.options(SessionSelection::Fresh),
                std::collections::BTreeMap::new(),
            )
            .await,
            PreparationError::Credentials,
        );
        assert!(!f.config.join("threads.db").exists() && !f.config.join("sessions-rs").exists());
    }
    write(
        &f.config,
        "config.json",
        &json!({"model":"mock","api_key":"${HUGE_CREDENTIAL}"}).to_string(),
    );
    error(
        Prepared::prepare_with_environment_for_tests(
            f.options(SessionSelection::Fresh),
            std::collections::BTreeMap::from([(
                "HUGE_CREDENTIAL".into(),
                "x".repeat(16 * 1024 + 1),
            )]),
        )
        .await,
        PreparationError::Credentials,
    );
    assert!(!f.config.join("threads.db").exists());
}

#[tokio::test]
async fn invalid_permission_selector_is_rejected_before_missing_provider_credentials() {
    let f = Fixture::new();
    write(
        &f.config,
        "config.json",
        &json!({"model":"mock","api_key":"${MISSING_CREDENTIAL}"}).to_string(),
    );
    let mut options = f.options(SessionSelection::Fresh);
    options.selectors.allow = vec!["tool:unknown".into()];
    error(
        Prepared::prepare_with_environment_for_tests(options, std::collections::BTreeMap::new())
            .await,
        PreparationError::Permissions,
    );
    assert!(!f.config.join("threads.db").exists());
}
