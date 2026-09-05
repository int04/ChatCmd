use super::*;

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
    assert!(!legacy["resultSchema"].is_null());
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
    assert!(!legacy["resultSchema"].is_null());

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

pub(super) struct TokenAuth;

impl AuthProvider for TokenAuth {
    fn authorize<'a>(&'a self, token: &'a str) -> BoxFuture<'a, RuntimeResult<String>> {
        Box::pin(async move { Ok(token.to_owned()) })
    }
}

#[derive(Clone, Default)]
pub(super) struct MutableAuth {
    tokens: Arc<std::sync::Mutex<HashMap<String, String>>>,
}

impl MutableAuth {
    pub(super) fn allow(&self, token: &str, agent_id: &str) {
        self.tokens
            .lock()
            .expect("mutable auth lock")
            .insert(token.to_owned(), agent_id.to_owned());
    }

    pub(super) fn revoke(&self, token: &str) {
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

pub(super) struct DisconnectingBody {
    pub(super) state: u8,
}

pub(super) struct CountingBody {
    pub(super) data: Option<axum::body::Bytes>,
    pub(super) bytes_polled: Arc<std::sync::atomic::AtomicUsize>,
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
pub(super) struct RecordingRuntime {
    pub(super) calls: Arc<std::sync::Mutex<Vec<(OperationContext, Value)>>>,
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
