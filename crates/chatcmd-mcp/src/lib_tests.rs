use super::*;

struct Accept;

impl AuthProvider for Accept {
    fn authorize<'a>(&'a self, _token: &'a str) -> BoxFuture<'a, RuntimeResult<String>> {
        Box::pin(async { Ok("agent-test".to_owned()) })
    }
}

impl OriginPolicy for Accept {
    fn authorize<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async move {
            if origin == "https://allowed.example" {
                Ok(())
            } else {
                Err(RuntimeError::new("origin_denied", "origin is not allowed"))
            }
        })
    }
}

#[derive(Clone)]
struct CatalogRuntime;

impl RuntimeApi for CatalogRuntime {
    fn call<'a>(
        &'a self,
        _tool: &'a str,
        _context: OperationContext,
        _arguments: Value,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async {
            Err(RuntimeError::new(
                "unexpected_tool_call",
                "catalog test must only list tools",
            ))
        })
    }

    fn local_device(&self) -> DeviceDescriptor {
        DeviceDescriptor {
            device_id: "catalog-test".to_owned(),
            machine_id: None,
            name: "Catalog Test".to_owned(),
            platform: "test".to_owned(),
            os_version: String::new(),
            architecture: "test".to_owned(),
            app_version: "test".to_owned(),
            online: true,
        }
    }

    fn fail_subagent<'a>(
        &'a self,
        _child_task_id: &'a str,
        _message: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async {
            Err(RuntimeError::new(
                "unexpected_subagent_failure",
                "catalog test must only list tools",
            ))
        })
    }
}

async fn list_tools_from_fresh_connection() -> Vec<String> {
    use rmcp::ServiceExt as _;

    let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
    let server = McpServer::new(Arc::new(CatalogRuntime));
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .expect("serve catalog server")
            .waiting()
            .await
            .expect("wait for catalog server");
    });
    let client = ().serve(client_transport).await.expect("serve catalog client");

    let mut names = client
        .list_tools(None)
        .await
        .expect("list tools through MCP")
        .tools
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    names.sort_unstable();

    client.cancel().await.expect("cancel catalog client");
    server_handle.await.expect("catalog server task");
    names
}

#[test]
fn catalog_names_are_stable_and_unique() {
    let mut names = TOOL_NAMES.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), TOOL_NAMES.len());
    assert_eq!(TOOL_NAMES.first(), Some(&"device_list"));
    assert_eq!(TOOL_NAMES.last(), Some(&"agent_turn_complete"));
    assert!(TOOL_NAMES.contains(&"agent_user_message"));
    assert!(TOOL_NAMES.contains(&"fs_replace_text"));
}

#[tokio::test]
async fn fresh_mcp_connections_advertise_every_declared_tool() {
    let mut declared = TOOL_NAMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    declared.sort_unstable();

    let initial = list_tools_from_fresh_connection().await;
    let reconnected = list_tools_from_fresh_connection().await;

    assert_eq!(initial, declared);
    assert_eq!(reconnected, declared);
}

#[test]
fn tool_specific_schemas_expose_required_canonical_fields() {
    let shell_read = serde_json::to_value(schemars::schema_for!(ShellReadArgs))
        .expect("serialize shell_read schema");
    assert!(shell_read["properties"].get("sessionId").is_some());
    assert!(shell_read["properties"].get("afterSequence").is_some());
    assert!(shell_read["properties"].get("maxEvents").is_some());
    assert!(
        shell_read["required"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "sessionId"))
    );

    let shell_create = serde_json::to_value(schemars::schema_for!(ShellCreateArgs))
        .expect("serialize shell_create schema");
    assert!(shell_create["properties"].get("workingDirectory").is_some());
    assert!(
        shell_create["properties"]
            .get("initialWorkingDirectory")
            .is_none()
    );

    let git_status =
        serde_json::to_value(schemars::schema_for!(CwdArgs)).expect("serialize git status schema");
    assert!(git_status["properties"].get("cwd").is_some());

    let git_commit = serde_json::to_value(schemars::schema_for!(GitCommitArgs))
        .expect("serialize git_commit schema");
    assert!(git_commit["properties"].get("message").is_some());
    assert!(
        git_commit["required"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "message"))
    );
}

#[test]
fn compatibility_aliases_deserialize_into_canonical_tool_fields() {
    let read: ShellReadArgs = serde_json::from_value(serde_json::json!({
        "sessionId": "shell-1",
        "fromSequence": 9
    }))
    .expect("fromSequence compatibility alias");
    assert_eq!(read.after_sequence, Some(9));

    let create: ShellCreateArgs = serde_json::from_value(serde_json::json!({
        "initialWorkingDirectory": "D:/DEV/CmdGPT/ChatCmdClient"
    }))
    .expect("initialWorkingDirectory compatibility alias");
    assert_eq!(
        create.working_directory.as_deref(),
        Some("D:/DEV/CmdGPT/ChatCmdClient")
    );

    let status: CwdArgs = serde_json::from_value(serde_json::json!({
        "path": "D:/DEV/CmdGPT/ChatCmdClient"
    }))
    .expect("git path compatibility alias");
    assert_eq!(status.cwd.as_deref(), Some("D:/DEV/CmdGPT/ChatCmdClient"));
}

#[test]
fn query_tokens_are_rejected() {
    assert!(has_query_token("access_token=secret"));
    assert!(!has_query_token("cursor=token-value"));
}

#[test]
fn local_session_correlation_is_stable_and_secret_free() {
    let first = local_mcp_session_id("agent-test", Some("remote-secret-session"));
    let second = local_mcp_session_id("agent-test", Some("remote-secret-session"));
    assert_eq!(first, second);
    assert!(!first.contains("remote-secret-session"));
    assert_ne!(
        first,
        local_mcp_session_id("other-agent", Some("remote-secret-session"))
    );
    assert_eq!(
        local_mcp_session_id("agent-test", None),
        local_mcp_session_id("agent-test", None)
    );
}

#[tokio::test]
async fn path_token_and_origin_fail_closed() {
    let security = HttpSecurity::new(Arc::new(Accept), Arc::new(Accept));
    let mut legacy_header = HeaderMap::new();
    legacy_header.insert("authorization", "Bearer secret".parse().expect("header"));
    legacy_header.insert("origin", "https://allowed.example".parse().expect("header"));
    assert_eq!(
        security
            .authorize("", &legacy_header, None)
            .await
            .expect_err("authorization header must not replace the path token")
            .code,
        "unauthorized"
    );

    let mut denied = HeaderMap::new();
    denied.insert("origin", "https://denied.example".parse().expect("header"));
    assert_eq!(
        security
            .authorize("secret", &denied, None)
            .await
            .expect_err("denied origin")
            .code,
        "origin_denied"
    );

    let no_origin = HeaderMap::new();
    assert_eq!(
        security
            .authorize("secret", &no_origin, None)
            .await
            .expect_err("missing origin must be decided by policy")
            .code,
        "origin_denied"
    );

    let mut allowed = HeaderMap::new();
    allowed.insert("origin", "https://allowed.example".parse().expect("header"));
    assert_eq!(
        security
            .authorize("secret", &allowed, None)
            .await
            .expect("path token and origin are valid"),
        "agent-test"
    );
    assert_eq!(
        security
            .authorize("secret", &allowed, Some("access_token=other"))
            .await
            .expect_err("query credentials stay unsupported")
            .code,
        "query_token_rejected"
    );
}
