//! Real MCP/local-file checkpoint enrollment and advisory read-only hints.
use super::*;
use craftsman_harness::testing::ScriptedModel;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn run_case(readonly: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    if !readonly {
        std::fs::write(root.join("file.txt"), "before").unwrap();
    }
    let store = Arc::new(
        LibsqlCheckpointer::open_at(directory.path().join("threads.db"))
            .await
            .unwrap(),
    );
    let notices = Arc::new(AtomicUsize::new(0));
    let observed = notices.clone();
    let checkpoints = Arc::new(
        WorkspaceCheckpointManager::new(root.clone(), store, "mcp-checkpoint").with_warning_sink(
            Some(Arc::new(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
            })),
        ),
    );
    let server = MockServer::start().await;
    let mut tool = json!({"name":"fixture_action","description":"fixture action","inputSchema":{"type":"object"}});
    if readonly {
        tool["annotations"] = json!({"readOnlyHint":true});
    }
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("Mcp-Method", "tools/list"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":{"tools":[tool]}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let effect = root.clone();
    Mock::given(method("POST")).and(path("/mcp")).and(header("Mcp-Method", "tools/call"))
        .respond_with(move |_: &wiremock::Request| {
            if !readonly { std::fs::write(effect.join("file.txt"), "MCP changed").unwrap(); }
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"done"}]}}))
        }).expect(1).mount(&server).await;
    let (_, client) = crate::mcp::McpClient::for_config_server(
        &json!({"name":"fixture","url":format!("{}/mcp",server.uri())}),
    )
    .unwrap();
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].read_only_hint, readonly);
    assert_eq!(tools[0].input_schema, json!({"type":"object"}));
    let mcp = Arc::new(McpTools {
        name: "fixture".into(),
        client,
        tools,
    });
    let model = Arc::new(ScriptedModel::new(vec![
        Msg::Assistant {
            content: String::new(),
            tool_calls: vec![craftsman_harness::ToolCall {
                id: "mcp-checkpoint-call".into(),
                name: "fixture_action".into(),
                args: json!({}),
            }],
        },
        Msg::assistant("done"),
    ]));
    let agent = Agent::builder(model)
        .middleware(checkpoints.clone())
        .tool(Arc::new(McpFrameworkTool {
            def: ToolDef {
                name: "fixture_action".into(),
                description: "fixture action".into(),
                parameters: json!({"type":"object"}),
            },
            name: "fixture_action".into(),
            mcp,
            read_only_hint: readonly,
            workspace_checkpoints: Some(checkpoints.clone()),
        }))
        .build();
    if readonly {
        std::fs::remove_dir(&root).unwrap();
    }
    agent.run_turn("Run the fixture action.").await.unwrap();
    assert_eq!(notices.load(Ordering::SeqCst), 0);
    let saved = checkpoints.list().await.unwrap();
    if readonly {
        assert!(saved.is_empty());
        assert!(!root.exists());
    } else {
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).unwrap(),
            "MCP changed"
        );
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].file_count, 1);
        assert_eq!(
            checkpoints.rewind(None, false).await.unwrap().restored,
            ["file.txt"]
        );
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).unwrap(),
            "before"
        );
    }
}

#[tokio::test]
async fn readonly_mcp_hint_skips_workspace_checkpoint_scans() {
    run_case(true).await;
}

#[tokio::test]
async fn potentially_mutating_mcp_tool_enrolls_and_preserves_file_undo() {
    run_case(false).await;
}
