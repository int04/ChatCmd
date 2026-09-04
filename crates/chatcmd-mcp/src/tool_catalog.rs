use super::McpServer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub const PROTOCOL_VERSION: u32 = 2;
pub const CATALOG_VERSION: u32 = 4;

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
    let risk_class = risk_class(name);
    ToolCapabilityFlags {
        approval_required: name == "shell_write"
            || name.starts_with("fs_")
            || name.starts_with("git_")
            || name == "process_kill"
            || name == "workspace_index_rebuild"
            || name == "task_artifact_create",
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

fn risk_class(name: &str) -> ToolRiskClass {
    match name {
        "device_list"
        | "device_get"
        | "workspace_roots"
        | "fs_list"
        | "fs_list_v2"
        | "fs_stat"
        | "fs_batch_stat"
        | "workspace_index_status"
        | "process_list"
        | "process_inspect"
        | "shell_list"
        | "shell_inspect"
        | "task_get"
        | "task_list"
        | "task_artifact_list"
        | "blob_status" => ToolRiskClass::MetadataRead,
        "fs_read_text" | "fs_read_text_v2" | "fs_batch_read" | "task_artifact_read"
        | "skill_read" | "shell_read" => ToolRiskClass::ContentRead,
        "fs_find" | "fs_search" | "skills_list" => ToolRiskClass::ComputeRead,
        "fs_create_directory" | "blob_begin" | "blob_write_chunk" | "blob_seal" => {
            ToolRiskClass::Create
        }
        "fs_write_text"
        | "fs_write_raw"
        | "fs_replace_text"
        | "fs_apply_edits"
        | "workspace_index_rebuild"
        | "shell_write"
        | "shell_resize"
        | "task_artifact_create" => ToolRiskClass::Modify,
        "fs_copy" | "fs_move" | "fs_restore_quarantine" => ToolRiskClass::MoveCopy,
        "fs_delete" | "fs_quarantine_gc" | "process_kill" | "blob_abort" | "shell_close" => {
            ToolRiskClass::Destructive
        }
        "shell_create" | "shell_wait" | "shell_signal" | "git_status" | "git_diff" | "git_log"
        | "git_branch" | "git_show" | "git_commit" => ToolRiskClass::ProcessExecution,
        "task_set_execution_mode"
        | "agent_user_message"
        | "agent_progress"
        | "agent_plan_question"
        | "agent_subagent_start"
        | "agent_subagent_wait"
        | "agent_turn_complete" => ToolRiskClass::Privileged,
        _ => ToolRiskClass::Privileged,
    }
}

fn path_fields(name: &str) -> Vec<PathFieldRole> {
    use PathFieldRole::{
        Cwd, Destination, Path, Paths, QuarantinePath, RequestPaths, Source, WorkingDirectory,
    };
    match name {
        "fs_batch_stat" | "fs_batch_read" => vec![Paths, RequestPaths],
        "fs_copy" | "fs_move" => vec![Source, Destination],
        "fs_restore_quarantine" => vec![QuarantinePath, Destination],
        "git_commit" => vec![Cwd, Paths],
        "git_diff" | "git_log" | "git_show" => vec![Cwd, Path],
        "git_status" | "git_branch" => vec![Cwd],
        "shell_create" => vec![WorkingDirectory],
        name if name.starts_with("fs_") || name.starts_with("workspace_index_") => vec![Path],
        _ => Vec::new(),
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
        "fs_search" => serde_json::to_value(schemars::schema_for!(
            chatcmd_runtime::ToolResultEnvelope<chatcmd_runtime::FsSearchPageData>
        )),
        "fs_read_text_v2" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::TextReadResultV2))
        }
        "fs_batch_read" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::FsBatchReadResult))
        }
        "fs_batch_stat" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::FsBatchStatResult))
        }
        "workspace_index_status" | "workspace_index_rebuild" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::WorkspaceIndexStatus))
        }
        "fs_apply_edits" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::ApplyEditsResult))
        }
        "fs_write_text" | "fs_write_raw" => {
            serde_json::to_value(schemars::schema_for!(chatcmd_runtime::AtomicWriteResult))
        }
        _ => return Value::Null,
    }
    .expect("result schema must serialize");
    canonicalize_contract(schema)
}

fn is_mutating(name: &str) -> bool {
    name.starts_with("blob_")
        || name.starts_with("fs_write")
        || name.starts_with("fs_replace")
        || name == "fs_apply_edits"
        || name == "workspace_index_rebuild"
        || name.starts_with("fs_create")
        || matches!(
            name,
            "fs_copy"
                | "fs_move"
                | "fs_delete"
                | "fs_restore_quarantine"
                | "fs_quarantine_gc"
                | "git_commit"
                | "process_kill"
        )
        || name.starts_with("shell_write")
        || name.starts_with("shell_signal")
        || name.starts_with("shell_resize")
        || name.starts_with("shell_close")
        || name.starts_with("task_set_")
        || name == "task_artifact_create"
        || name.starts_with("agent_")
}

fn build_id() -> &'static str {
    option_env!("CHATCMD_BUILD_ID")
        .or(option_env!("GITHUB_SHA"))
        .or(option_env!("VERGEN_GIT_SHA"))
        .unwrap_or("dev")
}
