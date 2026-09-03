use super::McpServer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub const PROTOCOL_VERSION: u32 = 2;
pub const CATALOG_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogMetadata {
    pub app_version: String,
    pub protocol_version: u32,
    pub catalog_version: u32,
    pub catalog_hash: String,
    pub build_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilityFlags {
    pub supports_cursor: bool,
    pub supports_content_ref: bool,
    pub mutating: bool,
    pub streaming: bool,
    pub result_schema_version: Option<u16>,
    pub deprecated_aliases: Vec<String>,
}

/// Tool names are derived from the same rmcp router that advertises schemas.
/// This avoids maintaining a second hand-written catalog that can drift.
pub static TOOL_NAMES: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut names = McpServer::tool_router()
        .list_all()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
});

pub fn catalog_metadata() -> CatalogMetadata {
    CatalogMetadata {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        catalog_version: CATALOG_VERSION,
        catalog_hash: catalog_hash(),
        build_id: build_id().to_owned(),
    }
}

pub fn canonical_manifest() -> Value {
    let mut tools = McpServer::tool_router()
        .list_all()
        .into_iter()
        .map(|tool| {
            let serialized = serde_json::to_value(&tool).expect("rmcp tool must serialize");
            let name = serialized
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let schema = serialized
                .get("inputSchema")
                .or_else(|| serialized.get("input_schema"))
                .cloned()
                .unwrap_or(Value::Null);
            serde_json::json!({
                "name": name,
                "schema": canonicalize_contract(schema),
                "resultSchema": result_schema(&name),
                "capabilities": capability_flags(&name),
            })
        })
        .collect::<Vec<_>>();

    tools.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });

    canonicalize_contract(serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "catalogVersion": CATALOG_VERSION,
        "tools": tools,
    }))
}

pub fn catalog_hash() -> String {
    hash_manifest_value(&canonical_manifest())
}

pub(crate) fn hash_manifest_value(value: &Value) -> String {
    let canonical = canonicalize_contract(value.clone());
    let bytes = serde_json::to_vec(&canonical).expect("canonical manifest must serialize");
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

pub(crate) fn canonicalize_contract(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut sorted = object
                .into_iter()
                .filter(|(key, _)| key != "description" && key != "title")
                .map(|(key, value)| (key, canonicalize_contract(value)))
                .collect::<Vec<_>>();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(sorted.into_iter().collect::<Map<_, _>>())
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(canonicalize_contract)
                .collect::<Vec<_>>(),
        ),
        other => other,
    }
}

fn capability_flags(name: &str) -> ToolCapabilityFlags {
    ToolCapabilityFlags {
        supports_cursor: matches!(name, "fs_list_v2" | "fs_find" | "shell_read"),
        supports_content_ref: name == "fs_list_v2",
        mutating: is_mutating(name),
        streaming: matches!(name, "shell_read" | "fs_read_text_v2"),
        result_schema_version: matches!(name, "fs_list_v2" | "fs_find")
            .then_some(chatcmd_runtime::TOOL_RESULT_SCHEMA_VERSION),
        deprecated_aliases: Vec::new(),
    }
}

fn result_schema(name: &str) -> Value {
    let schema = match name {
        "fs_list_v2" => serde_json::to_value(schemars::schema_for!(
            chatcmd_runtime::ToolResultEnvelope<chatcmd_runtime::FsListPageData>
        )),
        "fs_find" => serde_json::to_value(schemars::schema_for!(
            chatcmd_runtime::ToolResultEnvelope<chatcmd_runtime::FsFindPageData>
        )),
        "fs_read_text_v2" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::TextReadResultV2))
        }
        _ => return Value::Null,
    }
    .expect("result schema must serialize");
    canonicalize_contract(schema)
}

fn is_mutating(name: &str) -> bool {
    name.starts_with("fs_write")
        || name.starts_with("fs_replace")
        || name.starts_with("fs_create")
        || matches!(
            name,
            "fs_copy" | "fs_move" | "fs_delete" | "git_commit" | "process_kill"
        )
        || name.starts_with("shell_write")
        || name.starts_with("shell_signal")
        || name.starts_with("shell_resize")
        || name.starts_with("shell_close")
        || name.starts_with("task_set_")
        || name.starts_with("agent_")
}

fn build_id() -> &'static str {
    option_env!("CHATCMD_BUILD_ID")
        .or(option_env!("GITHUB_SHA"))
        .or(option_env!("VERGEN_GIT_SHA"))
        .unwrap_or("dev")
}
