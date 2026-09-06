use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chatcmd_core::{SettingsStore as _, TaskId, TaskStore as _};
use chatcmd_mcp::{PathFieldRole, TOOL_NAMES, ToolRiskClass, catalog_hash, tool_capabilities};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row as _, Sqlite, Transaction};
use uuid::Uuid;

use super::inputs::SubagentApprovalGrantInput;
use super::{RuntimeHost, invalid, now_ms, storage_error};

#[cfg(not(test))]
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(test)]
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(2);
const APPROVAL_POLL_INTERVAL: Duration = Duration::from_millis(150);
const SAFE_READ_GRANT_TTL_MS: i64 = 15 * 60 * 1_000;
const SAFE_READ_MAX_CALLS: i64 = 256;
const SAFE_READ_MAX_FILES: i64 = 100_000;
const SAFE_READ_MAX_BYTES: i64 = 1_073_741_824;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantPathScope {
    path: String,
    kind: GrantPathScopeKind,
    #[serde(default)]
    identity: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum GrantPathScopeKind {
    Exact,
    Subtree,
}

#[derive(Debug, Clone, Copy)]
struct GrantCharge {
    files: i64,
    bytes_read: i64,
}

pub(super) struct SubagentGrantInheritance<'a> {
    pub owner_agent_id: &'a str,
    pub parent_task_id: &'a str,
    pub parent_turn_id: &'a str,
    pub child_task_id: &'a str,
    pub child_turn_id: Option<&'a str>,
    pub child_attempt: i64,
    pub lease_expires_at_ms: i64,
}
