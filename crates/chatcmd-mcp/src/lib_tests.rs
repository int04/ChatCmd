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

#[path = "lib_tests/http_contract_tests.rs"]
mod http_contract_tests;
#[path = "lib_tests/result_contract_tests.rs"]
mod result_contract_tests;
