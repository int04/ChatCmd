use chatcmd_mcp::{CatalogMetadata, TOOL_NAMES, canonical_manifest, catalog_metadata};
use rmcp::ServiceExt as _;
use std::process::Stdio;
use tokio::process::Command;

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

    let names = first
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names.as_slice(), TOOL_NAMES.as_slice());
    assert!(names.iter().any(|name| name == "fs_replace_text"));

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
