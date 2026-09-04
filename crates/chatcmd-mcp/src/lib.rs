//! Official `rmcp` server surface for direct local ChatCMD execution.

mod request_identity;
mod server_contract;
mod subagent_protocol;
mod subagent_worker;
mod tool_catalog;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use chatcmd_runtime::{
    BoxFuture, DeviceDescriptor, OperationContext, RuntimeError, RuntimeResult, ShellInputKind,
};
use rmcp::{
    Peer, RoleServer,
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars,
    service::RequestContext,
    tool, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tower::ServiceExt;

use server_contract::error_value;

pub use tool_catalog::{
    CATALOG_VERSION, CatalogMetadata, PROTOCOL_VERSION, PathFieldRole, TOOL_NAMES,
    ToolCapabilityFlags, ToolRiskClass, canonical_manifest, catalog_hash, catalog_metadata,
    tool_capabilities,
};

/// Path-token authentication dependency injected by the HTTP host.
pub trait AuthProvider: Send + Sync {
    fn authorize<'a>(&'a self, token: &'a str) -> BoxFuture<'a, RuntimeResult<String>>;
}

/// Origin allow-list dependency injected by the HTTP host.
pub trait OriginPolicy: Send + Sync {
    fn authorize<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, RuntimeResult<()>>;
}

/// Runtime dispatch boundary. Implementations inject policy, task, device, and local runtime services.
pub trait RuntimeApi: Send + Sync {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        context: OperationContext,
        arguments: Value,
    ) -> BoxFuture<'a, RuntimeResult<Value>>;

    fn local_device(&self) -> DeviceDescriptor;

    fn project_folder<'a>(
        &'a self,
        _task_id: Option<&'a str>,
    ) -> BoxFuture<'a, RuntimeResult<Option<String>>> {
        Box::pin(async { Ok(None) })
    }

    fn fail_subagent<'a>(
        &'a self,
        child_task_id: &'a str,
        message: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<()>>;

    fn heartbeat_subagent<'a>(
        &'a self,
        _child_task_id: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn request_subagent_fallback<'a>(
        &'a self,
        parent_context: &'a OperationContext,
        registration: &'a Value,
        delegated_prompt: &'a str,
    ) -> BoxFuture<'a, RuntimeResult<Value>>;
}

/// Shared typed argument envelope. Unknown fields remain structured and never become shell text.
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolArguments {
    /// Caller-generated idempotency key.
    #[serde(default)]
    pub request_id: String,
    /// Deprecated caller identity accepted only for compatibility and discarded.
    #[serde(default)]
    #[schemars(skip)]
    pub agent_id: String,
    /// Task correlation identifier.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Turn correlation identifier.
    #[serde(default)]
    pub turn_id: Option<String>,
    /// Catalog hash last observed by the caller. When present it must match this server build.
    #[serde(default)]
    pub client_catalog_hash: Option<String>,
    /// Deprecated private caller field accepted only for compatibility and discarded.
    #[serde(default, rename = "__chatcmdMcpSessionId")]
    #[schemars(skip)]
    pub(crate) authenticated_session_id: Option<String>,
    /// Deprecated private caller field accepted only for compatibility and discarded.
    #[serde(default, rename = "__chatcmdConversationScopeId")]
    #[schemars(skip)]
    pub(crate) conversation_scope_id: Option<String>,
    /// Tool-specific typed JSON fields.
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct CommonToolArgs {
    /// Caller-generated idempotency key. Usually omit and let ChatCMD generate one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    request_id: String,
    /// Deprecated caller identity accepted only for compatibility and discarded.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schemars(skip)]
    agent_id: String,
    /// Task correlation identifier returned by agent_user_message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    /// Turn correlation identifier reused for every call in the current user turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    /// Catalog hash last observed by the caller. Send it to detect stale cached schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_catalog_hash: Option<String>,
    #[serde(
        default,
        rename = "__chatcmdMcpSessionId",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(skip)]
    authenticated_session_id: Option<String>,
    #[serde(
        default,
        rename = "__chatcmdConversationScopeId",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(skip)]
    conversation_scope_id: Option<String>,
}

macro_rules! tool_args {
    ($name:ident {}) => {
        #[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
        #[serde(rename_all = "camelCase")]
        struct $name {
            #[serde(flatten)]
            common: CommonToolArgs,
        }
    };
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty),+ $(,)? }) => {
        #[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
        #[serde(rename_all = "camelCase")]
        struct $name {
            #[serde(flatten)]
            common: CommonToolArgs,
            $(
                $(#[$meta])*
                $field: $ty,
            )+
        }
    };
}

const fn default_true() -> bool {
    true
}

tool_args!(NoArgs {});
tool_args!(DeviceGetArgs { device_id: String });
tool_args!(SessionArgs { session_id: String });
tool_args!(PathArgs { path: String });
tool_args!(StatArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version_strength: Option<chatcmd_runtime::VersionStrength>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash_algorithm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsBatchStatBudget>
});
tool_args!(BatchStatArgs {
    paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version_strength: Option<chatcmd_runtime::VersionStrength>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsStatBudget>
});
tool_args!(CwdArgs {
    #[serde(default, alias = "path", skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
tool_args!(SkillArgs {
    #[serde(alias = "id")]
    skill_id: String
});
tool_args!(ProcessArgs { process_id: u32 });
tool_args!(ArtifactArgs {
    artifact_id: String
});
tool_args!(ArtifactCreateArgs {
    content_ref: String,
    relative_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media_type: Option<String>
});
tool_args!(ExecutionModeArgs { mode: String });
tool_args!(ShellResizeArgs {
    session_id: String,
    columns: u16,
    rows: u16
});
tool_args!(TransferArgs {
    source: String,
    destination: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conflict_policy: Option<chatcmd_runtime::FsConflictPolicy>,
    #[serde(default = "default_true")]
    atomic_publish: bool,
    #[serde(default)]
    verify: chatcmd_runtime::FsVerifyMode,
    #[serde(default = "default_true")]
    preserve_metadata: bool,
    #[serde(default)]
    follow_symlinks: bool,
    #[serde(default)]
    dry_run: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_source_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_destination_version: Option<String>,
    #[serde(default)]
    budget: chatcmd_runtime::FsMutationBudget
});
tool_args!(DeleteArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recursive: Option<bool>,
    #[serde(default)]
    mode: chatcmd_runtime::FsDeleteMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    budget: chatcmd_runtime::FsMutationBudget
});
tool_args!(GitShowArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
tool_args!(GitCommitArgs {
    #[serde(default, alias = "path", skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    all: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<String>>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
tool_args!(ProcessKillArgs {
    process_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entire_tree: Option<bool>
});
tool_args!(UserMessageArgs { content: String });
tool_args!(ProgressArgs {
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_title: Option<String>
});
tool_args!(PlanQuestionArgs {
    question: String,
    options: [String; 2]
});
tool_args!(CompleteArgs {
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_title: Option<String>
});
tool_args!(ShellCreateArgs {
    #[serde(default, alias = "cwd", alias = "initialWorkingDirectory", skip_serializing_if = "Option::is_none")]
    working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arguments: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environment: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    columns: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rows: Option<u16>
});
tool_args!(ShellWriteArgs {
    session_id: String,
    #[serde(alias = "input")]
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    append_new_line: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_kind: Option<ShellInputKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sensitive: Option<bool>
});
tool_args!(ShellWaitArgs {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>
});
tool_args!(ShellReadArgs {
    session_id: String,
    #[serde(default, alias = "startSequence", alias = "fromSequence", skip_serializing_if = "Option::is_none")]
    after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_events: Option<usize>
});
tool_args!(ShellSignalArgs {
    session_id: String,
    signal: String
});
tool_args!(ShellCloseArgs {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    force: Option<bool>
});
tool_args!(ListArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>
});
tool_args!(ListV2Args {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sort: Option<chatcmd_runtime::FsListSort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata: Option<Vec<chatcmd_runtime::FsListMetadata>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsListBudget>
});
tool_args!(SearchArgs {
    path: String,
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<chatcmd_runtime::SearchMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    case_sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    word_boundary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exclude: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_ignored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_before: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_after: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_matches_per_file: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_results: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_file_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_snippet_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsSearchBudget>
});
tool_args!(FindArgs {
    path: String,
    pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pattern_mode: Option<chatcmd_runtime::FindPatternMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    case_sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry_types: Option<Vec<chatcmd_runtime::FindEntryType>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_depth: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_ignored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exclude: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_results: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsFindBudget>
});
tool_args!(ReadArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_characters: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line_count: Option<usize>
});
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ReadRangeArgs {
    unit: String,
    start: u64,
    limit: usize,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ReadBudgetArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_bytes_read: Option<u64>,
}
tool_args!(ReadV2Args {
    path: String,
    range: ReadRangeArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_line_endings: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<ReadBudgetArgs>
});
tool_args!(BatchReadArgs {
    requests: Vec<chatcmd_runtime::TextReadRequestV2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_total_output_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    concurrency: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::TextReadBudget>
});
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
enum ForbiddenContentSourceValue {}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
enum WriteTextSourceArgs {
    Inline {
        content: String,
        #[serde(
            default,
            rename = "contentRef",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(rename = "contentRef")]
        content_ref: Option<ForbiddenContentSourceValue>,
    },
    Reference {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<ForbiddenContentSourceValue>,
        #[serde(rename = "contentRef")]
        #[schemars(rename = "contentRef")]
        content_ref: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct WriteTextArgs {
    #[serde(flatten)]
    common: CommonToolArgs,
    path: String,
    #[serde(flatten)]
    source: WriteTextSourceArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata_policy: Option<chatcmd_runtime::MetadataPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    durability: Option<chatcmd_runtime::DurabilityMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_atomic: Option<bool>,
}
tool_args!(ReplaceTextArgs {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_occurrences: Option<usize>
});
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
enum ApplyEditsSourceArgs {
    Inline {
        edits: Vec<chatcmd_runtime::TextEdit>,
        #[serde(
            default,
            rename = "contentRef",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(rename = "contentRef")]
        content_ref: Option<ForbiddenContentSourceValue>,
    },
    Reference {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edits: Option<ForbiddenContentSourceValue>,
        #[serde(rename = "contentRef")]
        #[schemars(rename = "contentRef")]
        content_ref: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ApplyEditsArgs {
    #[serde(flatten)]
    common: CommonToolArgs,
    path: String,
    expected_version: String,
    coordinate_system: chatcmd_runtime::EditCoordinateSystem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    column_encoding: Option<chatcmd_runtime::EditColumnEncoding>,
    #[serde(flatten)]
    source: ApplyEditsSourceArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preserve_line_endings: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preserve_bom: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::ApplyEditsBudget>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
enum WriteRawSourceArgs {
    Inline {
        base64: String,
        #[serde(
            default,
            rename = "contentRef",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(rename = "contentRef")]
        content_ref: Option<ForbiddenContentSourceValue>,
    },
    Reference {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base64: Option<ForbiddenContentSourceValue>,
        #[serde(rename = "contentRef")]
        #[schemars(rename = "contentRef")]
        content_ref: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct WriteRawArgs {
    #[serde(flatten)]
    common: CommonToolArgs,
    path: String,
    #[serde(flatten)]
    source: WriteRawSourceArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata_policy: Option<chatcmd_runtime::MetadataPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    durability: Option<chatcmd_runtime::DurabilityMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_atomic: Option<bool>,
}
tool_args!(BlobBeginArgs {
    purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chunk_size_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ttl_seconds: Option<u64>
});
tool_args!(BlobChunkArgs {
    upload_id: String,
    offset: u64,
    data_base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chunk_sha256: Option<String>
});
tool_args!(BlobStatusArgs { upload_id: String });
tool_args!(BlobSealArgs {
    upload_id: String,
    final_size_bytes: u64,
    sha256: String
});
tool_args!(GitDiffArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staged: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stat: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
tool_args!(GitLogArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(flatten, default)]
    options: chatcmd_runtime::GitRunOptions
});
tool_args!(SubagentStartArgs {
    name: String,
    request: String
});
tool_args!(SubagentWaitArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>
});

fn into_tool_arguments<T: Serialize>(arguments: T) -> ToolArguments {
    serde_json::from_value(serde_json::to_value(arguments).expect("typed MCP arguments serialize"))
        .expect("typed MCP arguments convert to shared envelope")
}

/// Cloneable rmcp handler backed by injected services.
#[derive(Clone)]
pub struct McpServer {
    runtime: Arc<dyn RuntimeApi>,
}

impl McpServer {
    #[must_use]
    pub fn new(runtime: Arc<dyn RuntimeApi>) -> Self {
        Self { runtime }
    }

    fn prepare_call(
        &self,
        tool_name: &'static str,
        arguments: ToolArguments,
        authenticated: request_identity::AuthenticatedMcpContext,
    ) -> (OperationContext, Value) {
        let request_id = if arguments.request_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            arguments.request_id.clone()
        };
        let mut context = OperationContext::new(request_id, authenticated.agent_id, tool_name);
        context.task_id = arguments.task_id;
        context.turn_id = arguments.turn_id;
        context.mcp_session_id = Some(authenticated.local_session_id);
        context.conversation_scope_id = authenticated.conversation_scope_id;
        let value = Value::Object(arguments.fields.into_iter().collect());
        (context, value)
    }

    async fn invoke(
        &self,
        tool_name: &'static str,
        arguments: ToolArguments,
        request_context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if let Some(mismatch) = catalog_mismatch(&arguments) {
            return mismatch;
        }
        let Some(authenticated) = request_identity::authenticated_context(&request_context)
            .or_else(|| {
                request_identity::local_transport_context(&request_context, &arguments.agent_id)
            })
        else {
            return missing_authenticated_context();
        };
        let (context, value) = self.prepare_call(tool_name, arguments, authenticated);
        match self.runtime.call(tool_name, context, value).await {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(error_value(&error)),
        }
    }

    async fn invoke_subagent_start(
        &self,
        arguments: ToolArguments,
        peer: Peer<RoleServer>,
        request_context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if let Some(mismatch) = catalog_mismatch(&arguments) {
            return mismatch;
        }
        let Some(authenticated) = request_identity::authenticated_context(&request_context)
            .or_else(|| {
                request_identity::local_transport_context(&request_context, &arguments.agent_id)
            })
        else {
            return missing_authenticated_context();
        };
        let (context, value) = self.prepare_call("agent_subagent_start", arguments, authenticated);
        let request = value
            .get("request")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let registration = match self
            .runtime
            .call("agent_subagent_start", context.clone(), value)
            .await
        {
            Ok(value) => value,
            Err(error) => return CallToolResult::structured_error(error_value(&error)),
        };
        let tools = Self::tool_router().list_all();
        let registered = registration.clone();
        match subagent_worker::dispatch_registered_subagent(
            self.runtime.clone(),
            peer,
            context,
            registration,
            &request,
            tools,
        )
        .await
        {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => {
                if let Some(child_task_id) = registered
                    .get("childTaskId")
                    .or_else(|| registered.get("taskId"))
                    .and_then(Value::as_str)
                {
                    let _ = self
                        .runtime
                        .fail_subagent(child_task_id, &error.message)
                        .await;
                }
                let mut failed = registered;
                if let Some(object) = failed.as_object_mut() {
                    object.insert("status".to_owned(), Value::String("failed".to_owned()));
                    object.insert(
                        "dispatchMode".to_owned(),
                        Value::String("failed".to_owned()),
                    );
                    object.insert("workerStarted".to_owned(), Value::Bool(false));
                    object.insert(
                        "startupError".to_owned(),
                        serde_json::json!({"code": error.code, "message": error.message}),
                    );
                }
                CallToolResult::structured(failed)
            }
        }
    }
}

fn missing_authenticated_context() -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({
        "error": {
            "code": "authenticated_context_missing",
            "message": "trusted MCP request identity is unavailable",
            "retryable": true,
            "approvalRequired": false
        }
    }))
}

fn catalog_mismatch(arguments: &ToolArguments) -> Option<CallToolResult> {
    let client_hash = arguments.client_catalog_hash.as_deref()?;
    let metadata = catalog_metadata();
    if client_hash == metadata.catalog_hash {
        return None;
    }
    Some(CallToolResult::structured_error(serde_json::json!({
        "error": {
            "code": "catalog_mismatch",
            "message": "MCP tool catalog changed; refresh tool schemas and reconnect before retrying",
            "retryable": true,
            "approvalRequired": false
        },
        "serverCatalogHash": metadata.catalog_hash,
        "clientCatalogHash": client_hash,
        "catalogVersion": metadata.catalog_version,
        "protocolVersion": metadata.protocol_version,
        "recovery": "discard cached tool schemas, reconnect, initialize, and list_tools again"
    })))
}

macro_rules! tool_methods {
    ($(($method:ident, $args:ty, $description:literal)),+ $(,)?) => {
        #[tool_router]
        impl McpServer {
            $(
                #[tool(description = $description)]
                async fn $method(
                    &self,
                    Parameters(arguments): Parameters<$args>,
                    request_context: RequestContext<RoleServer>,
                ) -> CallToolResult {
                    self.invoke(
                        stringify!($method),
                        into_tool_arguments(arguments),
                        request_context,
                    ).await
                }
            )+

            #[tool(description = "Create and dispatch one child agent. Required fields: name, request. Pass the AI-chosen name and delegated request. ChatCMD delegates only through model sampling advertised by the connected ChatGPT/MCP host. If sampling is unavailable, no local executor is started and the child is returned as failed so the parent can continue or use host-native delegation when available.")]
            async fn agent_subagent_start(
                &self,
                Parameters(arguments): Parameters<SubagentStartArgs>,
                peer: Peer<RoleServer>,
                request_context: RequestContext<RoleServer>,
            ) -> CallToolResult {
                self.invoke_subagent_start(
                    into_tool_arguments(arguments),
                    peer,
                    request_context,
                ).await
            }
        }
    };
}

tool_methods!(
    (
        device_list,
        NoArgs,
        "List available execution devices. No tool-specific fields."
    ),
    (
        device_get,
        DeviceGetArgs,
        "Inspect one execution device. Required field: deviceId."
    ),
    (
        shell_create,
        ShellCreateArgs,
        "Create a persistent cross-platform PTY session. Canonical working-directory field is workingDirectory; cwd and initialWorkingDirectory are accepted compatibility aliases."
    ),
    (
        shell_write,
        ShellWriteArgs,
        "Write bounded interactive input to a PTY session. Required fields: sessionId, text. Optional inputKind is interactive or paste; bulk file/script content must use filesystem/blob tools. input is accepted as a compatibility alias for text."
    ),
    (
        shell_wait,
        ShellWaitArgs,
        "Wait without killing the PTY when timeout expires. Required field: sessionId; optional timeoutMs."
    ),
    (
        shell_read,
        ShellReadArgs,
        "Read bounded replayable PTY output. Required field: sessionId; canonical cursor field is afterSequence; startSequence and fromSequence are accepted compatibility aliases."
    ),
    (
        shell_signal,
        ShellSignalArgs,
        "Send a portable terminal signal. Required fields: sessionId, signal."
    ),
    (
        shell_resize,
        ShellResizeArgs,
        "Resize a PTY session. Required fields: sessionId, columns, rows."
    ),
    (
        shell_close,
        ShellCloseArgs,
        "Close or explicitly force-close a PTY session. Required field: sessionId; optional force."
    ),
    (
        shell_list,
        NoArgs,
        "List PTY sessions. No tool-specific fields."
    ),
    (
        shell_inspect,
        SessionArgs,
        "Inspect a PTY session. Required field: sessionId."
    ),
    (
        workspace_roots,
        NoArgs,
        "List roots granted to the current task/conversation. When the task has a project folder, this returns that folder rather than a process-wide or Agent workspace. No tool-specific fields."
    ),
    (
        blob_begin,
        BlobBeginArgs,
        "Begin an owner-scoped sequential blob upload. Required purpose=fsWriteText|fsWriteRaw|fsApplyEdits|artifact; optional expectedSizeBytes, contentType, expectedSha256, chunkSizeBytes, ttlSeconds. Returns opaque contentRef and uploadId."
    ),
    (
        blob_write_chunk,
        BlobChunkArgs,
        "Append one bounded Base64 chunk. Required uploadId, offset, dataBase64; optional chunkSha256. Offset must equal nextOffset; an identical retry is idempotent."
    ),
    (
        blob_status,
        BlobStatusArgs,
        "Inspect an owner-scoped upload and resume from nextOffset. Required uploadId."
    ),
    (
        blob_seal,
        BlobSealArgs,
        "Verify size and SHA-256, then make an upload immutable. Required uploadId, finalSizeBytes, sha256."
    ),
    (
        blob_abort,
        BlobStatusArgs,
        "Idempotently abort an owner-scoped upload and remove its temporary bytes. Required uploadId."
    ),
    (
        fs_list,
        ListArgs,
        "Compatibility directory listing with legacy offset/limit and global sorting. Required field: path; optional offset, limit. Prefer fs_list_v2 for large directories and cursor pagination."
    ),
    (
        fs_list_v2,
        ListV2Args,
        "Scalable cursor-paginated directory listing using filesystem traversal order (not global alphabetical order). Required field: path; optional cursor, limit, sort=filesystem, metadata=[type|size|readonly], includeHidden, budget {timeoutMs,maxEntriesScanned,maxStats}. Continue only with page.nextCursor for the same path/options; directory mutation invalidates continuation."
    ),
    (
        fs_search,
        SearchArgs,
        "Scalable cursor-paginated text search. Required fields: path, query. Optional mode=literal|regex (default literal), caseSensitive, wordBoundary, include/exclude globs, includeIgnored, contextBefore/contextAfter, maxMatchesPerFile, cursor, limit, maxSnippetBytes, budget {timeoutMs,maxFilesScanned,maxBytesScanned,maxOutputBytes,maxFileBytes}. Legacy maxResults/maxFileBytes remain accepted. Results include bounded match snippets, line/column/byte offsets, scan usage/warnings, truncation reason, and page.nextCursor. Continue only with page.nextCursor for the same path/query/options; workspace mutation can invalidate continuation. Use '.' for the workspace root rather than an empty path."
    ),
    (
        fs_find,
        FindArgs,
        "Scalable cursor-paginated path discovery. Required fields: path, pattern. Set patternMode=literal for filename contains, glob for workspace-relative glob matching (for example **/*.rs), or regex for workspace-relative regular expressions. Optional caseSensitive, entryTypes, maxDepth, includeIgnored, includeHidden, exclude, extensions, cursor, limit, budget {timeoutMs,maxEntriesScanned,maxMetadataCalls}. When patternMode is omitted, legacy *foo* literal-contains semantics are preserved with a warning. Continue only with page.nextCursor for the same path/options."
    ),
    (
        fs_read_text,
        ReadArgs,
        "Read UTF-8 workspace text through the compatibility adapter. Required field: path; optional maxCharacters, startLine (1-based), lineCount. Prefer fs_read_text_v2 for large files and resumable reads."
    ),
    (
        fs_read_text_v2,
        ReadV2Args,
        "Stream bounded UTF-8 workspace text without loading the whole file. Required fields: path and range {unit: line|byte, start, limit}. Optional maxBytes, includeLineEndings (default true), expectedVersion, and budget {timeoutMs,maxBytesRead}. Results include continuation offsets, truncation reason, bytesRead, sizeBytes, versionToken, encoding/BOM and newline metadata."
    ),
    (
        fs_batch_read,
        BatchReadArgs,
        "Read multiple bounded text ranges with ordered per-item outcomes, bounded concurrency, and a hard aggregate output cap. Each request uses the fs_read_text_v2 streaming contract."
    ),
    (
        fs_write_text,
        WriteTextArgs,
        "Atomically write UTF-8 workspace text. Required path and exactly one of content or contentRef; optional overwrite, expectedVersion, metadataPolicy=preserve|default, durability=none|data|full, requireAtomic. Inline content is capped at 256 KiB."
    ),
    (
        fs_replace_text,
        ReplaceTextArgs,
        "Safely edit an existing UTF-8 file by exact text replacement. Required fields: path, oldText, newText; optional expectedOccurrences (default 1). oldText must exactly match current file contents; read the target range first when content may have changed."
    ),
    (
        fs_apply_edits,
        ApplyEditsArgs,
        "Apply one or more non-overlapping UTF-8 range edits with optimistic concurrency. Required path, expectedVersion, coordinateSystem and exactly one of edits or contentRef; an fsApplyEdits blob contains the JSON edits array. Optional dryRun, preserveLineEndings, preserveBom, budget."
    ),
    (
        fs_write_raw,
        WriteRawArgs,
        "Atomically write workspace bytes. Required path and exactly one of bounded inline base64 or an fsWriteRaw contentRef; optional overwrite, expectedVersion, metadataPolicy=preserve|default, durability=none|data|full, requireAtomic."
    ),
    (
        fs_stat,
        StatArgs,
        "Inspect workspace path metadata and return a signed optimistic-concurrency versionToken. Required field: path. Optional versionStrength=metadata|sampled|content (default metadata), hashAlgorithm=sha256, budget {timeoutMs,maxBytesRead}. Metadata mode does not read file content; sampled/content hashing is bounded and cancellable. Symlinks and reparse points are not followed."
    ),
    (
        fs_batch_stat,
        BatchStatArgs,
        "Inspect up to 500 workspace paths in input order. Returns a success or structured error for every item and preserves fs_stat path authorization and version semantics."
    ),
    (
        workspace_index_status,
        PathArgs,
        "Report the path/metadata repository index generation, freshness, entry count, schema version, and last build error for an authorized workspace root."
    ),
    (
        workspace_index_rebuild,
        PathArgs,
        "Rebuild the bounded path/metadata repository index for an authorized workspace root. Content is never stored."
    ),
    (
        fs_create_directory,
        PathArgs,
        "Create a workspace directory. Required field: path."
    ),
    (
        fs_copy,
        TransferArgs,
        "Safely copy within canonical workspace scope using preflight, durable journal, verified sibling staging and atomic publish. Required source/destination; optional conflictPolicy=error|skip|replace, atomicPublish, verify=none|metadata|content, preserveMetadata, dryRun, expected versions and budget. Legacy overwrite is accepted. Symlinks are not followed."
    ),
    (
        fs_move,
        TransferArgs,
        "Safely move within canonical workspace scope. Cross-device-safe staging is verified and published before source removal. Accepts the fs_copy options and legacy overwrite."
    ),
    (
        fs_delete,
        DeleteArgs,
        "Delete within canonical workspace scope under policy. Default mode is quarantine; permanent deletion must be explicit. Optional recursive, expectedVersion, dryRun and bounded budget."
    ),
    (
        git_status,
        CwdArgs,
        "Get Git working tree status. Optional cwd; legacy path is accepted as a cwd alias."
    ),
    (
        git_diff,
        GitDiffArgs,
        "Get argument-safe Git diff output. Optional cwd, staged, stat, path. cwd selects the repository; path filters a file within it."
    ),
    (
        git_log,
        GitLogArgs,
        "Get bounded Git history. Optional cwd, count, path."
    ),
    (
        git_branch,
        CwdArgs,
        "List Git branches. Optional cwd; legacy path is accepted as a cwd alias."
    ),
    (
        git_show,
        GitShowArgs,
        "Show a validated Git revision. Required revision; optional cwd and path."
    ),
    (
        git_commit,
        GitCommitArgs,
        "Create a Git commit without shell interpolation. Required field: message; optional cwd, all (default true), paths. Never call with an empty object."
    ),
    (
        process_list,
        NoArgs,
        "List local processes. No tool-specific fields."
    ),
    (
        process_inspect,
        ProcessArgs,
        "Inspect a local process. Required field: processId."
    ),
    (
        process_kill,
        ProcessKillArgs,
        "Terminate a local process under policy. Required field: processId; optional entireTree."
    ),
    (
        skills_list,
        NoArgs,
        "After agent_user_message, discover available .agents and .codex skills before non-trivial project work; no tool-specific fields."
    ),
    (
        skill_read,
        SkillArgs,
        "Read a relevant matching skill. Required field: skillId; id is accepted as a compatibility alias."
    ),
    (
        task_get,
        NoArgs,
        "Read current task state. Uses taskId correlation from the common fields."
    ),
    (task_list, NoArgs, "List tasks. No tool-specific fields."),
    (
        task_set_execution_mode,
        ExecutionModeArgs,
        "Set task execution mode. Required field: mode."
    ),
    (
        task_artifact_list,
        NoArgs,
        "List task artifacts. Uses taskId correlation from the common fields."
    ),
    (
        task_artifact_create,
        ArtifactCreateArgs,
        "Consume a sealed artifact contentRef into an authorized workspace-relative path and register it for the current task. Required contentRef and relativePath; optional mediaType."
    ),
    (
        task_artifact_read,
        ArtifactArgs,
        "Read a task artifact. Required field: artifactId."
    ),
    (
        agent_user_message,
        UserMessageArgs,
        "MANDATORY FIRST TOOL: call exactly once at the start of every user turn. Required field: content containing the exact current user message. Reuse the same turnId for all calls in that turn. Never use agent_user_message for progress, reflections, findings, or commentary after tool results; use agent_progress for those updates. After this call, inspect the returned toolRecovery directive. If a needed ChatCMD schema is not visible, treat it as host lazy-loading rather than a missing server capability: use the host connector/resource discovery mechanism (for ChatGPT, for example api_tool.list_resources on the current connector with a focused query such as fs_ or shell_), load the needed schema in the same turn, and continue. Never reply that a ChatCMD tool is unavailable/not loaded before attempting that discovery."
    ),
    (
        agent_progress,
        ProgressArgs,
        "Publish one concise user-visible progress milestone. Required field: message; optional suggestedTitle. For non-trivial work, call once immediately after agent_user_message with a summary of the request and next action, then keep using it throughout the turn. Strongly prefer updates after meaningful filesystem/search/read/edit results, Git/process results, pending shell work, incomplete sub-agent waits, and task-relevant failures/non-zero command results before retry or fallback. This is an AI-side execution rule, not a server-side gate: group tightly related low-level operations when useful so progress updates do not materially slow the task or add unnecessary MCP round trips. If another ChatCMD schema needed for the work is not currently visible, do not treat the visible subset as the server capability boundary: use the host connector/resource discovery mechanism in the same turn (for ChatGPT, for example api_tool.list_resources with a focused query such as fs_ or shell_) and continue instead of reporting that the tool is not loaded. Error updates should summarize the observable failure and next recovery/alternative. Report observable findings and decisions only, never private chain-of-thought. Do not call after agent_turn_complete."
    ),
    (
        agent_plan_question,
        PlanQuestionArgs,
        "PLAN MODE ONLY: ask one missing-information question and wait inside the current turn for up to 120 seconds. Required fields: question and options containing exactly two distinct choices. The UI also lets the user submit a custom answer. When the call returns an answer, immediately publish the returned agentProgressMessage with agent_progress before any further reasoning or tool call. If timedOut=true, choose one of the returned options yourself, publish the question plus chosen answer with agent_progress, then continue. Ask only information that materially changes the plan; do not ask questions whose answers are already known."
    ),
    (
        agent_subagent_wait,
        SubagentWaitArgs,
        "Wait for child agents registered by the current parent turn. Optional timeoutMs. Repeat while allFinished=false before finalizing."
    ),
    (
        agent_turn_complete,
        CompleteArgs,
        "MANDATORY FINALIZATION: call exactly once immediately before replying after every other tool call has finished. Required field: content with the exact final user-facing response; optional suggestedTitle only on the first message of a chat."
    ),
);

/// Fail-closed HTTP security state.
#[derive(Clone)]
pub struct HttpSecurity {
    auth: Arc<dyn AuthProvider>,
    origins: Arc<dyn OriginPolicy>,
}

impl HttpSecurity {
    #[must_use]
    pub fn new(auth: Arc<dyn AuthProvider>, origins: Arc<dyn OriginPolicy>) -> Self {
        Self { auth, origins }
    }

    async fn authorize(
        &self,
        token: &str,
        headers: &HeaderMap,
        query: Option<&str>,
    ) -> RuntimeResult<String> {
        if query.is_some_and(has_query_token) {
            return Err(RuntimeError::new(
                "query_token_rejected",
                "authentication token in query is forbidden",
            ));
        }
        if token.is_empty() || token.len() > 512 {
            return Err(RuntimeError::new(
                "unauthorized",
                "valid MCP path token is required",
            ));
        }
        let agent_id = self.auth.authorize(token).await?;
        self.origins
            .authorize(
                headers
                    .get("origin")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
            )
            .await?;
        Ok(agent_id)
    }
}

fn has_query_token(query: &str) -> bool {
    query.split('&').any(|pair| {
        let name = pair.split_once('=').map_or(pair, |(name, _)| name);
        matches!(
            name.to_ascii_lowercase().as_str(),
            "token" | "access_token" | "bearer_token"
        )
    })
}

#[derive(Clone)]
struct McpHttpState {
    security: HttpSecurity,
    service: StreamableHttpService<McpServer, LocalSessionManager>,
    session_owners: Arc<tokio::sync::RwLock<HashMap<String, String>>>,
}

async fn mcp_handler(
    State(state): State<McpHttpState>,
    Path(token): Path<String>,
    mut request: Request<Body>,
) -> Response {
    let agent_id = match state
        .security
        .authorize(&token, request.headers(), request.uri().query())
        .await
    {
        Ok(agent_id) => agent_id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let header_session = request
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if let Some(remote_session) = header_session.as_deref()
        && state
            .session_owners
            .read()
            .await
            .get(remote_session)
            .is_some_and(|owner| owner != &agent_id)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let session_id = local_mcp_session_id(&agent_id, header_session.as_deref());
    let metadata = catalog_metadata();
    tracing::info!(
        target: "chatcmd_mcp_catalog",
        app_version = %metadata.app_version,
        protocol_version = metadata.protocol_version,
        catalog_version = metadata.catalog_version,
        catalog_hash = %metadata.catalog_hash,
        build_id = %metadata.build_id,
        transport = "streamable_http",
        agent_id = %agent_id,
        mcp_session_id = %session_id,
        "mcp_session_catalog"
    );

    // rmcp forwards the original HTTP parts through RequestContext, including this
    // server-owned extension. The request body remains untouched and is parsed once.
    request_identity::bind_authenticated_context(
        &mut request,
        request_identity::AuthenticatedMcpContext::new(agent_id.clone(), session_id),
    );

    // Keep the credential at the HTTP boundary. Downstream rmcp handlers receive
    // a stable credential-free URI and the authenticated agent identity only.
    *request.uri_mut() = Uri::from_static("/mcp");
    match state.service.clone().oneshot(request).await {
        Ok(response) => {
            if let Some(remote_session) = response
                .headers()
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
            {
                state
                    .session_owners
                    .write()
                    .await
                    .insert(remote_session.to_owned(), agent_id);
            }
            response.into_response()
        }
        Err(infallible) => match infallible {},
    }
}

async fn catalog_handler(
    State(state): State<McpHttpState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if state
        .security
        .authorize(&token, &headers, uri.query())
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(serde_json::json!({
        "metadata": catalog_metadata(),
        "manifest": canonical_manifest()
    }))
    .into_response()
}

fn local_mcp_session_id(agent_id: &str, header_session: Option<&str>) -> String {
    let scope = header_session.unwrap_or("agent-fallback");
    let material = format!("agent:{agent_id}\0session:{scope}");
    format!(
        "mcp-session-{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

/// Build reusable Streamable HTTP service with local rmcp session management.
pub fn streamable_http_service(
    server: McpServer,
) -> StreamableHttpService<McpServer, LocalSessionManager> {
    streamable_http_service_with_config(server, StreamableHttpServerConfig::default())
}

fn streamable_http_service_with_config(
    server: McpServer,
    config: StreamableHttpServerConfig,
) -> StreamableHttpService<McpServer, LocalSessionManager> {
    let config = config.with_max_request_body_bytes(request_identity::MCP_CONTROL_BODY_BYTES);
    StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        config,
    )
}

/// Build an Axum router protected by a token path segment and Origin checks.
pub fn axum_router(server: McpServer, security: HttpSecurity) -> Router {
    axum_router_with_host_validation(server, security, true)
}

/// Build an Axum router while optionally disabling rmcp Host validation.
///
/// Host validation should only be disabled when the listener itself is loopback-only
/// and an external reverse proxy is the sole public ingress. Token and Origin checks
/// remain active at the ChatCmdClient boundary.
pub fn axum_router_with_host_validation(
    server: McpServer,
    security: HttpSecurity,
    validate_host: bool,
) -> Router {
    let config = if validate_host {
        StreamableHttpServerConfig::default()
    } else {
        StreamableHttpServerConfig::default().disable_allowed_hosts()
    };
    let state = McpHttpState {
        security,
        service: streamable_http_service_with_config(server, config),
        session_owners: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
    };
    Router::new()
        .route("/mcp/{token}", any(mcp_handler))
        .route("/mcp/{token}/catalog", get(catalog_handler))
        .with_state(state)
}

#[cfg(test)]
mod lib_tests;
