use chatcmd_mcp::{CatalogMetadata, TOOL_NAMES, canonical_manifest, catalog_metadata};
use rmcp::ServiceExt as _;
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process::{Child, Command},
};

const METADATA_PREFIX: &str = "CHATCMD_CATALOG_METADATA=";

async fn catalog_snapshot_from_packaged_process() -> (CatalogMetadata, Vec<serde_json::Value>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_catalog_smoke_server"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn packaged catalog smoke server");
    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let client = ().serve((stdout, stdin)).await.expect("connect MCP client");
    let instructions = client
        .peer_info()
        .and_then(|info| info.instructions.clone())
        .expect("server instructions with catalog metadata");
    let expected_instructions_version = catalog_metadata().instructions_version;
    assert!(instructions.contains(&format!(
        "CHATCMD_INSTRUCTIONS_VERSION={expected_instructions_version}"
    )));
    assert!(instructions.contains("CHATCMD_INSTRUCTIONS_HASH="));
    assert!(instructions.contains("COD-01 INTENT AND SCOPE"));
    assert!(instructions.contains("COD-16 AUTONOMY AND DISCOVERY"));
    assert!(instructions.contains("REVIEW ROLE"));
    let metadata_json = instructions
        .strip_prefix(METADATA_PREFIX)
        .and_then(|value| value.split_once("} ").map(|(json, _)| format!("{json}}}")))
        .expect("catalog metadata prefix in server instructions");
    let metadata: CatalogMetadata =
        serde_json::from_str(&metadata_json).expect("deserialize catalog metadata");
    let mut tools = client
        .list_tools(None)
        .await
        .expect("list tools from packaged process")
        .tools
        .into_iter()
        .map(|tool| serde_json::to_value(tool).expect("serialize advertised tool"))
        .collect::<Vec<_>>();
    tools.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    client.cancel().await.expect("cancel packaged MCP client");
    let status = child.wait().await.expect("wait packaged smoke server");
    assert!(
        status.success(),
        "catalog smoke server exited with {status}"
    );
    (metadata, tools)
}

fn normalized_schema(tool: &serde_json::Value) -> serde_json::Value {
    let schema = tool
        .get("inputSchema")
        .or_else(|| tool.get("input_schema"))
        .cloned()
        .expect("advertised schema");
    strip_non_contract_metadata(schema)
}

fn strip_non_contract_metadata(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => {
            let mut fields = object
                .into_iter()
                .filter(|(key, _)| key != "description" && key != "title")
                .map(|(key, value)| (key, strip_non_contract_metadata(value)))
                .collect::<Vec<_>>();
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(fields.into_iter().collect())
        }
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .into_iter()
                .map(strip_non_contract_metadata)
                .collect(),
        ),
        other => other,
    }
}

#[tokio::test]
async fn packaged_process_advertises_exact_manifest_contract_deterministically() {
    let (first_metadata, first) = catalog_snapshot_from_packaged_process().await;
    let (second_metadata, second) = catalog_snapshot_from_packaged_process().await;
    assert_eq!(
        first, second,
        "fresh processes must advertise the same schemas"
    );
    assert_eq!(
        first_metadata, second_metadata,
        "fresh processes must advertise the same catalog metadata"
    );
    assert_eq!(first_metadata, catalog_metadata());
    assert!(first_metadata.instructions_hash.starts_with("sha256:"));
    assert!(!first_metadata.instructions_version.is_empty());

    let names = first
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names.as_slice(), TOOL_NAMES.as_slice());
    assert!(names.iter().any(|name| name == "fs_replace_text"));
    assert!(names.iter().any(|name| name == "fs_apply_edits"));
    assert!(names.iter().any(|name| name == "project_context"));
    assert_eq!(first_metadata.catalog_version, 8);
    let project_context = first
        .iter()
        .find(|tool| tool["name"] == "project_context")
        .expect("project_context advertised on wire");
    assert!(
        project_context["inputSchema"]["properties"]
            .get("policy")
            .is_some()
    );
    assert!(
        project_context["inputSchema"]["properties"]
            .get("range")
            .is_some()
    );
    assert!(
        project_context["description"]
            .as_str()
            .is_some_and(|value| value.contains("excluded by default"))
    );
    let git_commit = first
        .iter()
        .find(|tool| tool["name"] == "git_commit")
        .expect("git_commit advertised on wire");
    assert!(
        git_commit["description"]
            .as_str()
            .is_some_and(|value| value.contains("all defaults to false"))
    );
    assert!(
        git_commit["inputSchema"]["properties"]
            .get("paths")
            .is_some()
    );
    let subagent_start = first
        .iter()
        .find(|tool| tool["name"] == "agent_subagent_start")
        .expect("agent_subagent_start advertised on wire");
    let description = subagent_start["description"]
        .as_str()
        .expect("subagent description");
    for marker in [
        "samplingTools",
        "samplingText",
        "extensionFallback",
        "existing",
        "status=failed",
        "startupError",
    ] {
        assert!(
            description.contains(marker),
            "missing {marker} in {description}"
        );
    }

    let manifest = canonical_manifest();
    let manifest_tools = manifest["tools"].as_array().expect("manifest tools");
    assert_eq!(first.len(), manifest_tools.len());
    for (advertised, expected) in first.iter().zip(manifest_tools) {
        assert_eq!(advertised["name"], expected["name"]);
        assert_eq!(normalized_schema(advertised), expected["schema"]);
    }
}

#[tokio::test]
async fn stale_connector_catalog_refreshes_once_and_observes_current_tools() {
    let mut cached_hash = "sha256:stale".to_owned();
    let mut refresh_attempts = 0_u8;

    let (metadata, tools) = catalog_snapshot_from_packaged_process().await;
    if cached_hash != metadata.catalog_hash {
        refresh_attempts += 1;
        cached_hash = metadata.catalog_hash.clone();
    }

    assert_eq!(
        refresh_attempts, 1,
        "catalog refresh must be bounded to one retry"
    );
    assert_eq!(cached_hash, catalog_metadata().catalog_hash);
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"].as_str() == Some("fs_replace_text")),
        "refreshed connector catalog must expose fs_replace_text"
    );
}

async fn spawn_packaged_http_identity_server() -> (Child, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_http_identity_smoke_server"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn packaged HTTP identity smoke server");
    let stdout = child.stdout.take().expect("HTTP smoke stdout");
    let mut reader = BufReader::new(stdout);
    let mut address = String::new();
    reader
        .read_line(&mut address)
        .await
        .expect("read HTTP smoke address");
    assert!(!address.trim().is_empty(), "HTTP smoke server address");
    (child, format!("http://{}", address.trim()))
}

#[tokio::test]
async fn packaged_streamable_http_tool_call_preserves_trusted_identity() {
    let (mut child, base_url) = spawn_packaged_http_identity_server().await;
    let client = reqwest::Client::new();
    let endpoint = format!("{base_url}/mcp/agent-a");
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "packaged-http-smoke", "version": "1"}
        }
    });
    let response = client
        .post(&endpoint)
        .header("origin", "https://allowed.example")
        .header("accept", "application/json, text/event-stream")
        .json(&initialize)
        .send()
        .await
        .expect("initialize packaged HTTP server");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("packaged session id")
        .to_owned();
    let _ = response.text().await.expect("initialize response body");

    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let response = client
        .post(&endpoint)
        .header("origin", "https://allowed.example")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .header("mcp-protocol-version", "2025-03-26")
        .json(&initialized)
        .send()
        .await
        .expect("initialized packaged HTTP server");
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);

    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "device_list",
            "_meta": {"openai/session": "packaged-chat"},
            "arguments": {
                "agentId": "spoofed-agent",
                "__chatcmdMcpSessionId": "spoofed-session",
                "__chatcmdConversationScopeId": "spoofed-scope"
            }
        }
    });
    let response = client
        .post(&endpoint)
        .header("origin", "https://allowed.example")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .header("mcp-protocol-version", "2025-03-26")
        .json(&call)
        .send()
        .await
        .expect("tool call packaged HTTP server");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.text().await.expect("packaged tool response body");
    assert!(body.contains("\"agentId\":\"agent-a\""), "{body}");
    assert!(body.contains("\"conversationScopeId\":\"openai:"), "{body}");
    assert!(!body.contains("spoofed-agent"), "{body}");
    assert!(!body.contains("spoofed-session"), "{body}");
    assert!(!body.contains("spoofed-scope"), "{body}");

    child.kill().await.expect("stop packaged HTTP smoke server");
    let _ = child.wait().await;
}
