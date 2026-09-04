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

    fn request_subagent_fallback<'a>(
        &'a self,
        _parent_context: &'a OperationContext,
        _registration: &'a Value,
        _delegated_prompt: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async {
            Err(RuntimeError::new(
                "unexpected_subagent_fallback",
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
fn catalog_names_are_sorted_stable_and_unique() {
    let mut sorted = TOOL_NAMES.to_vec();
    sorted.sort_unstable();
    assert_eq!(TOOL_NAMES.as_slice(), sorted.as_slice());
    sorted.dedup();
    assert_eq!(sorted.len(), TOOL_NAMES.len());
    assert!(TOOL_NAMES.iter().any(|name| name == "agent_user_message"));
    assert!(TOOL_NAMES.iter().any(|name| name == "fs_replace_text"));
    assert!(TOOL_NAMES.iter().any(|name| name == "fs_apply_edits"));
    assert!(TOOL_NAMES.iter().any(|name| name == "fs_list_v2"));
    assert!(TOOL_NAMES.iter().any(|name| name == "fs_read_text_v2"));
    assert!(TOOL_NAMES.iter().any(|name| name == "fs_batch_read"));
    assert!(TOOL_NAMES.iter().any(|name| name == "fs_batch_stat"));
    assert!(TOOL_NAMES.iter().any(|name| name == "task_artifact_create"));
    assert!(
        TOOL_NAMES
            .iter()
            .any(|name| name == "workspace_index_status")
    );
    for name in [
        "blob_begin",
        "blob_write_chunk",
        "blob_status",
        "blob_seal",
        "blob_abort",
    ] {
        assert!(TOOL_NAMES.iter().any(|candidate| candidate == name));
    }
}

#[test]
fn blob_schemas_expose_bounded_caller_budget() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("manifest tools");
    for name in [
        "blob_begin",
        "blob_write_chunk",
        "blob_status",
        "blob_seal",
        "blob_abort",
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        let schema = tool["schema"].to_string();
        assert!(schema.contains("budget"), "{name} budget missing");
        assert!(schema.contains("timeoutMs"), "{name} timeout missing");
        assert!(
            schema.contains("maxBytesRead"),
            "{name} read budget missing"
        );
        assert!(
            schema.contains("maxBytesWritten"),
            "{name} write budget missing"
        );
        assert!(
            schema.contains("maxOpenFiles"),
            "{name} open-file budget missing"
        );
    }
}

#[test]
fn fs_write_text_schema_requires_exactly_one_content_source() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("manifest tools");
    let write = tools
        .iter()
        .find(|tool| tool["name"] == "fs_write_text")
        .expect("fs_write_text");
    let alternatives = write["schema"]["anyOf"]
        .as_array()
        .expect("source alternatives");
    assert_eq!(alternatives.len(), 2);
    let schema = write["schema"].to_string();
    assert!(schema.contains("contentRef"));
    assert!(schema.contains("content"));
    assert!(
        serde_json::from_value::<WriteTextArgs>(serde_json::json!({
            "path": "file.txt",
            "content": "inline"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WriteTextArgs>(serde_json::json!({
            "path": "file.txt",
            "contentRef": "blob:v1:test"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WriteTextArgs>(serde_json::json!({
            "path": "file.txt",
            "content": "inline",
            "contentRef": "blob:v1:test"
        }))
        .is_err()
    );
}

#[test]
fn fs_write_raw_schema_requires_exactly_one_content_source() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("manifest tools");
    let write = tools
        .iter()
        .find(|tool| tool["name"] == "fs_write_raw")
        .expect("fs_write_raw");
    let alternatives = write["schema"]["anyOf"]
        .as_array()
        .expect("source alternatives");
    assert_eq!(alternatives.len(), 2);
    let schema = write["schema"].to_string();
    assert!(schema.contains("base64"));
    assert!(schema.contains("contentRef"));
    assert!(
        serde_json::from_value::<WriteRawArgs>(serde_json::json!({
            "path": "file.bin",
            "base64": "YWJj"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WriteRawArgs>(serde_json::json!({
            "path": "file.bin",
            "contentRef": "blob:v1:test"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WriteRawArgs>(serde_json::json!({
            "path": "file.bin",
            "base64": "YWJj",
            "contentRef": "blob:v1:test"
        }))
        .is_err()
    );
}

#[test]
fn task_artifact_create_advertises_content_ref_contract() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("manifest tools");
    let create = tools
        .iter()
        .find(|tool| tool["name"] == "task_artifact_create")
        .expect("task_artifact_create");
    let properties = &create["schema"]["properties"];
    assert!(properties.get("contentRef").is_some());
    assert!(properties.get("relativePath").is_some());
    assert!(properties.get("mediaType").is_some());
    assert_eq!(create["capabilities"]["supportsContentRef"], true);
    assert_eq!(create["capabilities"]["mutating"], true);
}

#[test]
fn fs_apply_edits_advertises_versioned_streaming_contract() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("manifest tools");
    let apply = tools
        .iter()
        .find(|tool| tool["name"] == "fs_apply_edits")
        .expect("fs_apply_edits");
    let properties = &apply["schema"]["properties"];
    for field in [
        "path",
        "expectedVersion",
        "coordinateSystem",
        "columnEncoding",
        "dryRun",
        "preserveLineEndings",
        "preserveBom",
        "budget",
    ] {
        assert!(
            properties.get(field).is_some(),
            "missing fs_apply_edits field {field}"
        );
    }
    let alternatives = apply["schema"]["anyOf"]
        .as_array()
        .expect("source alternatives");
    assert_eq!(alternatives.len(), 2);
    let schema = apply["schema"].to_string();
    assert!(schema.contains("edits"));
    assert!(schema.contains("contentRef"));
    assert!(
        serde_json::from_value::<ApplyEditsArgs>(serde_json::json!({
            "path": "file.txt",
            "expectedVersion": "v1-test",
            "coordinateSystem": "byte",
            "edits": []
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<ApplyEditsArgs>(serde_json::json!({
            "path": "file.txt",
            "expectedVersion": "v1-test",
            "coordinateSystem": "byte",
            "contentRef": "blob:v1:test"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<ApplyEditsArgs>(serde_json::json!({
            "path": "file.txt",
            "expectedVersion": "v1-test",
            "coordinateSystem": "byte",
            "edits": [],
            "contentRef": "blob:v1:test"
        }))
        .is_err()
    );
    assert_eq!(apply["capabilities"]["mutating"], true);
    assert_eq!(apply["capabilities"]["streaming"], true);
    assert!(
        apply["resultSchema"]["properties"]
            .get("newVersion")
            .is_some()
    );
    assert!(
        apply["resultSchema"]["properties"]
            .get("commitState")
            .is_some()
    );
}

#[tokio::test]
async fn fresh_mcp_connections_advertise_every_declared_tool() {
    let declared = TOOL_NAMES.to_vec();
    let initial = list_tools_from_fresh_connection().await;
    let reconnected = list_tools_from_fresh_connection().await;

    assert_eq!(initial, declared);
    assert_eq!(reconnected, declared);
}

#[test]
fn canonical_manifest_has_schema_for_every_tool() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("manifest tools");
    assert_eq!(tools.len(), TOOL_NAMES.len());
    for (index, tool) in tools.iter().enumerate() {
        assert_eq!(tool["name"].as_str(), Some(TOOL_NAMES[index].as_str()));
        assert!(
            !tool["schema"].is_null(),
            "{} has no schema",
            TOOL_NAMES[index]
        );
        assert!(tool["capabilities"].is_object());
    }
}

#[test]
fn fs_list_v2_advertises_versioned_result_schema_without_changing_legacy_input() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("manifest tools");
    let legacy = tools
        .iter()
        .find(|tool| tool["name"] == "fs_list")
        .expect("legacy fs_list");
    let v2 = tools
        .iter()
        .find(|tool| tool["name"] == "fs_list_v2")
        .expect("fs_list_v2");

    assert!(legacy["schema"]["properties"].get("offset").is_some());
    assert!(legacy["schema"]["properties"].get("cursor").is_none());
    assert!(legacy["resultSchema"].is_null());
    assert_eq!(legacy["capabilities"]["resultSchemaVersion"], Value::Null);

    assert!(v2["schema"]["properties"].get("cursor").is_some());
    assert!(v2["schema"]["properties"].get("offset").is_none());
    assert!(v2["schema"]["properties"].get("sort").is_some());
    assert!(v2["schema"]["properties"].get("metadata").is_some());
    assert!(v2["schema"]["properties"].get("includeHidden").is_some());
    assert!(v2["schema"]["properties"].get("budget").is_some());
    assert_eq!(v2["capabilities"]["resultSchemaVersion"], 1);
    assert_eq!(v2["capabilities"]["supportsCursor"], true);
    assert_eq!(
        v2["resultSchema"]["properties"]["schemaVersion"]["type"],
        "integer"
    );
    assert!(v2["resultSchema"]["properties"].get("page").is_some());
    assert!(v2["resultSchema"]["properties"].get("truncation").is_some());
    assert!(v2["resultSchema"]["properties"].get("contentRef").is_some());
    let data = &v2["resultSchema"]["properties"]["data"];
    let data_ref = data["$ref"].as_str().expect("fs_list_v2 data schema ref");
    let definition = data_ref
        .rsplit('/')
        .next()
        .expect("fs_list_v2 data definition name");
    let data_schema = &v2["resultSchema"]["$defs"][definition];
    assert!(data_schema["properties"].get("items").is_some());
    assert!(data_schema["properties"].get("directoryVersion").is_some());
    assert!(data_schema["properties"].get("sort").is_some());
}

#[test]
fn fs_search_advertises_v2_cursor_budget_schema_and_legacy_fields() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("manifest tools");
    let search = tools
        .iter()
        .find(|tool| tool["name"] == "fs_search")
        .expect("fs_search");
    let properties = &search["schema"]["properties"];
    for field in [
        "mode",
        "caseSensitive",
        "wordBoundary",
        "include",
        "exclude",
        "includeIgnored",
        "contextBefore",
        "contextAfter",
        "maxMatchesPerFile",
        "cursor",
        "limit",
        "maxSnippetBytes",
        "budget",
        "maxResults",
        "maxFileBytes",
    ] {
        assert!(
            properties.get(field).is_some(),
            "missing fs_search schema field {field}"
        );
    }
    assert_eq!(search["capabilities"]["supportsCursor"], true);
    assert_eq!(search["capabilities"]["resultSchemaVersion"], 1);
    assert!(search["resultSchema"]["properties"].get("page").is_some());
    assert!(
        search["resultSchema"]["properties"]
            .get("truncation")
            .is_some()
    );
    assert!(search["resultSchema"]["properties"].get("usage").is_some());

    let legacy: SearchArgs = serde_json::from_value(serde_json::json!({
        "path": ".",
        "query": "needle",
        "maxResults": 3,
        "maxFileBytes": 4096
    }))
    .expect("legacy fs_search request");
    assert!(legacy.mode.is_none());
    assert_eq!(legacy.max_results, Some(3));
    assert_eq!(legacy.max_file_bytes, Some(4096));
}

#[test]
fn fs_read_text_v2_advertises_streaming_range_contract_and_result_metadata() {
    let manifest = canonical_manifest();
    let tools = manifest["tools"].as_array().expect("manifest tools");
    let legacy = tools
        .iter()
        .find(|tool| tool["name"] == "fs_read_text")
        .expect("legacy fs_read_text");
    let v2 = tools
        .iter()
        .find(|tool| tool["name"] == "fs_read_text_v2")
        .expect("fs_read_text_v2");

    assert!(legacy["schema"]["properties"].get("startLine").is_some());
    assert!(legacy["schema"]["properties"].get("range").is_none());
    assert!(legacy["resultSchema"].is_null());

    assert!(v2["schema"]["properties"].get("range").is_some());
    assert!(v2["schema"]["properties"].get("maxBytes").is_some());
    assert!(v2["schema"]["properties"].get("expectedVersion").is_some());
    assert_eq!(v2["capabilities"]["streaming"], true);
    assert!(
        v2["resultSchema"]["properties"]
            .get("nextStartLine")
            .is_some()
    );
    assert!(
        v2["resultSchema"]["properties"]
            .get("nextByteOffset")
            .is_some()
    );
    assert!(
        v2["resultSchema"]["properties"]
            .get("versionToken")
            .is_some()
    );
    assert!(v2["resultSchema"]["properties"].get("lineEnding").is_some());
}

#[test]
fn contract_hash_changes_for_schema_changes_but_not_descriptions() {
    let base = serde_json::json!({
        "name": "demo",
        "description": "first wording",
        "schema": {"type":"object", "properties":{"path":{"type":"string", "description":"old"}}}
    });
    let wording_only = serde_json::json!({
        "name": "demo",
        "description": "different wording",
        "schema": {"type":"object", "properties":{"path":{"type":"string", "description":"new"}}}
    });
    let schema_changed = serde_json::json!({
        "name": "demo",
        "description": "different wording",
        "schema": {"type":"object", "properties":{"path":{"type":"integer", "description":"new"}}}
    });
    assert_eq!(
        tool_catalog::hash_manifest_value(&base),
        tool_catalog::hash_manifest_value(&wording_only)
    );
    assert_ne!(
        tool_catalog::hash_manifest_value(&base),
        tool_catalog::hash_manifest_value(&schema_changed)
    );
}

#[test]
fn metadata_hash_matches_canonical_manifest() {
    let metadata = catalog_metadata();
    assert_eq!(metadata.catalog_hash, catalog_hash());
    assert_eq!(metadata.protocol_version, PROTOCOL_VERSION);
    assert_eq!(metadata.catalog_version, CATALOG_VERSION);
    assert!(metadata.catalog_hash.starts_with("sha256:"));
}

#[test]
fn common_tool_schema_exposes_catalog_hash_for_stale_cache_detection() {
    let schema = serde_json::to_value(schemars::schema_for!(CommonToolArgs))
        .expect("serialize common schema");
    assert!(schema["properties"].get("clientCatalogHash").is_some());
    assert!(schema["properties"].get("agentId").is_none());
}

#[test]
fn stale_catalog_diagnostic_contains_both_hashes() {
    let arguments = ToolArguments {
        client_catalog_hash: Some("sha256:stale".to_owned()),
        ..ToolArguments::default()
    };
    let result = catalog_mismatch(&arguments).expect("stale hash must fail");
    let structured = result.structured_content.expect("structured mismatch");
    assert_eq!(structured["clientCatalogHash"], "sha256:stale");
    assert_eq!(structured["serverCatalogHash"], catalog_hash());
    assert_eq!(structured["error"]["code"], "catalog_mismatch");
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

    let shell_write = serde_json::to_value(schemars::schema_for!(ShellWriteArgs))
        .expect("serialize shell_write schema");
    assert!(shell_write["properties"].get("inputKind").is_some());
    assert!(shell_write["properties"].get("sensitive").is_some());

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

struct TokenAuth;

impl AuthProvider for TokenAuth {
    fn authorize<'a>(&'a self, token: &'a str) -> BoxFuture<'a, RuntimeResult<String>> {
        Box::pin(async move { Ok(token.to_owned()) })
    }
}

#[derive(Clone, Default)]
struct MutableAuth {
    tokens: Arc<std::sync::Mutex<HashMap<String, String>>>,
}

impl MutableAuth {
    fn allow(&self, token: &str, agent_id: &str) {
        self.tokens
            .lock()
            .expect("mutable auth lock")
            .insert(token.to_owned(), agent_id.to_owned());
    }

    fn revoke(&self, token: &str) {
        self.tokens.lock().expect("mutable auth lock").remove(token);
    }
}

impl AuthProvider for MutableAuth {
    fn authorize<'a>(&'a self, token: &'a str) -> BoxFuture<'a, RuntimeResult<String>> {
        Box::pin(async move {
            self.tokens
                .lock()
                .expect("mutable auth lock")
                .get(token)
                .cloned()
                .ok_or_else(|| RuntimeError::new("unauthorized", "token is not authorized"))
        })
    }
}

struct DisconnectingBody {
    state: u8,
}

struct CountingBody {
    data: Option<axum::body::Bytes>,
    bytes_polled: Arc<std::sync::atomic::AtomicUsize>,
}

impl http_body::Body for CountingBody {
    type Data = axum::body::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let Some(data) = self.data.take() else {
            return std::task::Poll::Ready(None);
        };
        self.bytes_polled
            .fetch_add(data.len(), std::sync::atomic::Ordering::Relaxed);
        std::task::Poll::Ready(Some(Ok(http_body::Frame::data(data))))
    }
}

impl http_body::Body for DisconnectingBody {
    type Data = axum::body::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let next = match self.state {
            0 => Some(Ok(http_body::Frame::data(axum::body::Bytes::from_static(
                b"{\"jsonrpc\":\"2.0\",",
            )))),
            1 => Some(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "simulated client disconnect",
            ))),
            _ => None,
        };
        self.state = self.state.saturating_add(1);
        std::task::Poll::Ready(next)
    }
}

#[derive(Clone, Default)]
struct RecordingRuntime {
    calls: Arc<std::sync::Mutex<Vec<(OperationContext, Value)>>>,
}

impl RuntimeApi for RecordingRuntime {
    fn call<'a>(
        &'a self,
        _tool: &'a str,
        context: OperationContext,
        arguments: Value,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("recording runtime lock")
                .push((context, arguments));
            Ok(serde_json::json!({"ok": true}))
        })
    }

    fn local_device(&self) -> DeviceDescriptor {
        CatalogRuntime.local_device()
    }

    fn fail_subagent<'a>(
        &'a self,
        _child_task_id: &'a str,
        _message: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async { Ok(()) })
    }

    fn request_subagent_fallback<'a>(
        &'a self,
        _parent_context: &'a OperationContext,
        _registration: &'a Value,
        _delegated_prompt: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async { Ok(Value::Null) })
    }
}

fn mcp_post(path: &str, body: impl Into<Body>, session_id: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("host", "localhost")
        .header("origin", "https://allowed.example")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(session_id) = session_id {
        builder = builder
            .header("mcp-session-id", session_id)
            .header("mcp-protocol-version", "2025-03-26");
    }
    builder.body(body.into()).expect("MCP request")
}

fn mcp_delete(path: &str, session_id: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(path)
        .header("host", "localhost")
        .header("origin", "https://allowed.example")
        .header("mcp-session-id", session_id)
        .header("mcp-protocol-version", "2025-03-26")
        .body(Body::empty())
        .expect("MCP delete request")
}

async fn initialize_existing_router(router: &Router, token: &str) -> String {
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "identity-test", "version": "1"}
        }
    })
    .to_string();
    let response = router
        .clone()
        .oneshot(mcp_post(&format!("/mcp/{token}"), initialize, None))
        .await
        .expect("initialize response");
    if response.status() != StatusCode::OK {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body");
        panic!(
            "initialize failed with {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session header")
        .to_owned();
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
    .to_string();
    let response = router
        .clone()
        .oneshot(mcp_post(
            &format!("/mcp/{token}"),
            initialized,
            Some(&session_id),
        ))
        .await
        .expect("initialized response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    session_id
}

async fn initialized_router(runtime: RecordingRuntime, agent: &str) -> (Router, String) {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router =
        axum_router_with_host_validation(McpServer::new(Arc::new(runtime)), security, false);
    let session_id = initialize_existing_router(&router, agent).await;
    (router, session_id)
}

#[tokio::test]
async fn streamable_http_uses_trusted_identity_and_parsed_request_meta() {
    let runtime = RecordingRuntime::default();
    let (router, session_id) = initialized_router(runtime.clone(), "agent-a").await;
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "device_list",
            "_meta": {"openai/session": "chat-a"},
            "arguments": {
                "agentId": "spoofed-agent",
                "__chatcmdMcpSessionId": "spoofed-session",
                "__chatcmdConversationScopeId": "spoofed-scope"
            }
        }
    })
    .to_string();
    let response = router
        .oneshot(mcp_post("/mcp/agent-a", call, Some(&session_id)))
        .await
        .expect("tool response");
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("tool response body");

    let calls = runtime.calls.lock().expect("recorded calls");
    let (context, arguments) = calls.first().unwrap_or_else(|| {
        panic!(
            "runtime tool call; response was {}",
            String::from_utf8_lossy(&response_body)
        )
    });
    assert_eq!(context.agent_id, "agent-a");
    assert_eq!(
        context.mcp_session_id.as_deref(),
        Some(local_mcp_session_id("agent-a", Some(&session_id)).as_str())
    );
    assert!(
        context
            .conversation_scope_id
            .as_deref()
            .is_some_and(|scope| scope.starts_with("openai:"))
    );
    assert_eq!(arguments, &serde_json::json!({}));
}

#[tokio::test]
async fn streamable_http_session_cannot_be_reused_by_another_agent() {
    let runtime = RecordingRuntime::default();
    let (router, session_id) = initialized_router(runtime, "agent-a").await;
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "device_list", "arguments": {}}
    })
    .to_string();
    let response = router
        .oneshot(mcp_post("/mcp/agent-b", call, Some(&session_id)))
        .await
        .expect("cross-agent response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn streamable_http_rejects_empty_and_malformed_control_json() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );

    let empty = router
        .clone()
        .oneshot(mcp_post("/mcp/agent-a", "", None))
        .await
        .expect("empty control response");
    assert_eq!(empty.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let malformed = router
        .oneshot(mcp_post("/mcp/agent-a", "{not-json", None))
        .await
        .expect("malformed control response");
    assert_eq!(malformed.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn streamable_http_delete_terminates_owned_session() {
    let runtime = RecordingRuntime::default();
    let (router, session_id) = initialized_router(runtime, "agent-a").await;
    let response = router
        .oneshot(mcp_delete("/mcp/agent-a", &session_id))
        .await
        .expect("delete response");
    assert!(response.status().is_success());
}

#[tokio::test]
async fn streamable_http_rejects_declared_control_body_over_cap_before_polling_body() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );
    let mut request = mcp_post("/mcp/agent-a", "{}", None);
    request.headers_mut().insert(
        http::header::CONTENT_LENGTH,
        http::HeaderValue::from_str(&(request_identity::MCP_CONTROL_BODY_BYTES + 1).to_string())
            .expect("content length"),
    );
    let response = router
        .oneshot(request)
        .await
        .expect("declared oversized response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn streamable_http_uses_actual_streamed_bytes_when_content_length_is_too_small() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );
    let oversized = vec![b'x'; request_identity::MCP_CONTROL_BODY_BYTES + 1];
    let mut request = mcp_post("/mcp/agent-a", oversized, None);
    request.headers_mut().insert(
        http::header::CONTENT_LENGTH,
        http::HeaderValue::from_static("1"),
    );
    let response = router
        .oneshot(request)
        .await
        .expect("actual oversized response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn streamable_http_body_disconnect_fails_closed() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );
    let body = Body::new(DisconnectingBody { state: 0 });
    let response = router
        .oneshot(mcp_post("/mcp/agent-a", body, None))
        .await
        .expect("disconnect response");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn streamable_http_revoked_token_cannot_reuse_active_session_and_rotation_can() {
    let auth = MutableAuth::default();
    auth.allow("token-old", "agent-a");
    let runtime = RecordingRuntime::default();
    let security = HttpSecurity::new(Arc::new(auth.clone()), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(runtime.clone())),
        security,
        false,
    );
    let session_id = initialize_existing_router(&router, "token-old").await;
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "device_list", "arguments": {}}
    })
    .to_string();

    auth.revoke("token-old");
    let revoked = router
        .clone()
        .oneshot(mcp_post("/mcp/token-old", call.clone(), Some(&session_id)))
        .await
        .expect("revoked response");
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    auth.allow("token-new", "agent-a");
    let rotated = router
        .oneshot(mcp_post("/mcp/token-new", call, Some(&session_id)))
        .await
        .expect("rotated response");
    assert_eq!(rotated.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(rotated.into_body(), 64 * 1024)
        .await
        .expect("rotated response body");
    assert_eq!(runtime.calls.lock().expect("recorded calls").len(), 1);
}

#[tokio::test]
async fn streamable_http_restart_invalidates_old_remote_session() {
    let runtime = RecordingRuntime::default();
    let (first_router, session_id) = initialized_router(runtime.clone(), "agent-a").await;
    drop(first_router);

    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let restarted =
        axum_router_with_host_validation(McpServer::new(Arc::new(runtime)), security, false);
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "device_list", "arguments": {}}
    })
    .to_string();
    let stale = restarted
        .clone()
        .oneshot(mcp_post("/mcp/agent-a", call, Some(&session_id)))
        .await
        .expect("stale session response");
    assert_eq!(stale.status(), StatusCode::NOT_FOUND);

    let fresh_session = initialize_existing_router(&restarted, "agent-a").await;
    assert_ne!(fresh_session, session_id);
}

#[tokio::test]
async fn streamable_http_near_cap_body_is_consumed_once_by_transport() {
    let runtime = RecordingRuntime::default();
    let (router, session_id) = initialized_router(runtime.clone(), "agent-a").await;
    let padding = "x".repeat(request_identity::MCP_CONTROL_BODY_BYTES - 4096);
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "device_list",
            "arguments": {"padding": padding}
        }
    })
    .to_string();
    assert!(call.len() < request_identity::MCP_CONTROL_BODY_BYTES);
    assert!(call.len() > request_identity::MCP_CONTROL_BODY_BYTES - 8192);

    let bytes_polled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let body = Body::new(CountingBody {
        data: Some(axum::body::Bytes::from(call.clone())),
        bytes_polled: bytes_polled.clone(),
    });
    let response = router
        .oneshot(mcp_post("/mcp/agent-a", body, Some(&session_id)))
        .await
        .expect("near-cap tool response");
    assert_eq!(response.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("near-cap response body");
    assert_eq!(
        bytes_polled.load(std::sync::atomic::Ordering::Relaxed),
        call.len(),
        "identity middleware must not pre-consume or replay the request body"
    );
    assert_eq!(runtime.calls.lock().expect("recorded calls").len(), 1);
}

#[tokio::test]
async fn streamable_http_enforces_control_body_cap_without_content_length_header() {
    let security = HttpSecurity::new(Arc::new(TokenAuth), Arc::new(Accept));
    let router = axum_router_with_host_validation(
        McpServer::new(Arc::new(RecordingRuntime::default())),
        security,
        false,
    );
    let oversized = vec![b'x'; request_identity::MCP_CONTROL_BODY_BYTES + 1];
    let request = mcp_post("/mcp/agent-a", oversized, None);
    assert!(
        request
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .is_none()
    );
    let response = router.oneshot(request).await.expect("oversized response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
