use super::{McpServer, server_contract};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub const PROTOCOL_VERSION: u32 = 2;
pub const CATALOG_VERSION: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogMetadata {
    pub app_version: String,
    pub protocol_version: u32,
    pub catalog_version: u32,
    pub catalog_hash: String,
    pub instructions_version: String,
    pub instructions_hash: String,
    pub build_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilityFlags {
    /// Stable semantic class used by runtime authorization. UI hints must not
    /// be treated as an authority decision.
    pub operation_class: ToolOperationClass,
    pub approval_required: bool,
    pub supports_cursor: bool,
    pub supports_content_ref: bool,
    pub mutating: bool,
    pub streaming: bool,
    pub result_schema_version: Option<u16>,
    pub deprecated_aliases: Vec<String>,
    pub risk_class: ToolRiskClass,
    pub path_fields: Vec<PathFieldRole>,
    pub supports_budget: bool,
    pub supports_dry_run: bool,
    pub supports_expected_version: bool,
}

impl ToolCapabilityFlags {
    #[must_use]
    pub const fn is_execution_policy_controlled(&self) -> bool {
        self.operation_class.is_execution_policy_controlled()
    }

    #[must_use]
    pub const fn is_permission_change(&self) -> bool {
        matches!(self.operation_class, ToolOperationClass::PermissionChange)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolOperationClass {
    MetadataRead,
    ContentRead,
    Mutation,
    ProcessExecution,
    PermissionChange,
    Lifecycle,
    StopCleanup,
}

impl ToolOperationClass {
    #[must_use]
    pub const fn is_execution_policy_controlled(self) -> bool {
        matches!(
            self,
            Self::ContentRead | Self::Mutation | Self::ProcessExecution
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolRiskClass {
    MetadataRead,
    ContentRead,
    ComputeRead,
    Create,
    Modify,
    MoveCopy,
    Destructive,
    ProcessExecution,
    Privileged,
}

impl ToolRiskClass {
    #[must_use]
    pub const fn is_safe_read(self) -> bool {
        matches!(
            self,
            Self::MetadataRead | Self::ContentRead | Self::ComputeRead
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathFieldRole {
    Path,
    Paths,
    RequestPaths,
    Source,
    Destination,
    QuarantinePath,
    WorkingDirectory,
    Cwd,
}

#[must_use]
pub fn tool_capabilities(name: &str) -> ToolCapabilityFlags {
    capability_flags(name)
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
        instructions_version: server_contract::instructions::INSTRUCTIONS_VERSION.to_owned(),
        instructions_hash: instructions_hash(),
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

pub fn instructions_hash() -> String {
    let mut descriptions = McpServer::tool_router()
        .list_all()
        .into_iter()
        .map(|tool| {
            (
                tool.name.to_string(),
                tool.description.as_deref().unwrap_or_default().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    descriptions.sort_by(|left, right| left.0.cmp(&right.0));
    hash_instruction_value(&serde_json::json!({
        "bundle": server_contract::instruction_bundle_for_hash().replace("\r\n", "\n"),
        "descriptions": descriptions,
        "version": server_contract::instructions::INSTRUCTIONS_VERSION,
    }))
}

pub(crate) fn hash_instruction_value(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).expect("instruction manifest must serialize");
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
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
    let risk_class = risk_class(name);
    let operation_class = operation_class(name);
    ToolCapabilityFlags {
        operation_class,
        approval_required: operation_class.is_execution_policy_controlled(),
        supports_cursor: matches!(name, "fs_list_v2" | "fs_find" | "fs_search" | "shell_read"),
        supports_content_ref: matches!(
            name,
            "fs_write_text"
                | "fs_write_raw"
                | "fs_apply_edits"
                | "task_artifact_read"
                | "task_artifact_create"
        ),
        mutating: is_mutating(name),
        streaming: matches!(
            name,
            "shell_read" | "fs_read_text_v2" | "fs_batch_read" | "fs_apply_edits"
        ),
        result_schema_version: matches!(name, "fs_list_v2" | "fs_find" | "fs_search")
            .then_some(chatcmd_runtime::TOOL_RESULT_SCHEMA_VERSION),
        deprecated_aliases: Vec::new(),
        risk_class,
        path_fields: path_fields(name),
        supports_budget: matches!(
            name,
            "fs_stat"
                | "fs_list_v2"
                | "fs_read_text_v2"
                | "fs_batch_read"
                | "fs_batch_stat"
                | "fs_find"
                | "fs_search"
                | "fs_apply_edits"
                | "fs_copy"
                | "fs_move"
                | "fs_delete"
        ),
        supports_dry_run: matches!(
            name,
            "fs_apply_edits" | "fs_copy" | "fs_move" | "fs_delete" | "fs_quarantine_gc"
        ),
        supports_expected_version: matches!(
            name,
            "fs_stat"
                | "fs_write_text"
                | "fs_write_raw"
                | "fs_apply_edits"
                | "fs_copy"
                | "fs_move"
                | "fs_delete"
        ),
    }
}

include!("tool_catalog/classification.rs");

fn build_id() -> &'static str {
    option_env!("CHATCMD_BUILD_ID")
        .or(option_env!("GITHUB_SHA"))
        .or(option_env!("VERGEN_GIT_SHA"))
        .unwrap_or("dev")
}
