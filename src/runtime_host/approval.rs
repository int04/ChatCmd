use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chatcmd_core::{Approval, ApprovalState, SettingsStore as _, TaskId, TaskStore as _};
use chatcmd_mcp::{PathFieldRole, ToolRiskClass, catalog_hash, tool_capabilities};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row as _;
use uuid::Uuid;

use super::{RuntimeHost, invalid, now_ms, storage_error};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
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

impl RuntimeHost {
    pub(super) async fn authorize_execution(
        &self,
        context: &OperationContext,
        tool: &str,
        arguments: &Value,
    ) -> RuntimeResult<()> {
        let capabilities = tool_capabilities(tool);
        if !capabilities.approval_required {
            return Ok(());
        }
        let task_id = TaskId::new(context.task_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("taskId", error))?;
        let mode_task_id = self.execution_mode_task_id(&task_id).await?;
        match self
            .repository
            .execution_mode(Some(&mode_task_id))
            .await
            .map_err(storage_error)?
        {
            chatcmd_core::ExecutionMode::Allow => return Ok(()),
            chatcmd_core::ExecutionMode::Deny => {
                return Err(RuntimeError::new(
                    "policy_denied",
                    "conversation access mode denied this operation",
                ));
            }
            chatcmd_core::ExecutionMode::Approval => {}
        }

        let resolved_arguments = self
            .resolve_approval_paths(context, tool, arguments)
            .await?;
        if capabilities.risk_class.is_safe_read()
            && self
                .consume_safe_read_grant(context, tool, &resolved_arguments)
                .await?
        {
            return Ok(());
        }

        let approval_id = context.request_id.clone();
        let turn_id = context.turn_id.as_deref().unwrap_or_default();
        let grant_preview = if capabilities.risk_class.is_safe_read() {
            Some(self.safe_read_grant_preview(context, tool).await?)
        } else {
            None
        };
        let operation_digest = operation_digest(tool, &resolved_arguments);
        let mut summary = approval_summary(tool, capabilities.risk_class, &resolved_arguments);
        if let Some(preview) = grant_preview.as_ref() {
            summary["grantPreview"] = preview.clone();
        }
        let request_json = json!({
            "activityId": approval_id,
            "agentId": context.agent_id,
            "tool": tool,
            "turnId": turn_id,
            "riskClass": capabilities.risk_class,
            "summary": summary,
            "operationDigest": operation_digest,
            "catalogHash": catalog_hash(),
            "grantPreview": grant_preview,
        })
        .to_string();
        let created_at_ms = now_ms();
        self.repository
            .save_approval(&Approval {
                id: approval_id.clone(),
                task_id: task_id.clone(),
                // MCP logical sessions are not terminal_sessions rows. The task/turn/activity
                // identifiers are sufficient to resolve this task-scoped approval.
                session_id: None,
                state: ApprovalState::Pending,
                request_json,
                decision_json: None,
                created_at_ms,
                resolved_at_ms: None,
            })
            .await
            .map_err(storage_error)?;
        self.publish_event(
            format!("approval-pending:{approval_id}:{created_at_ms}"),
            "approval.pending",
            Some(task_id.as_str().to_owned()),
            None,
            Some(turn_id.to_owned()),
            json!({
                "activityId": approval_id,
                "tool": tool,
                "summary": summary,
                "riskClass": capabilities.risk_class,
                "grantPreview": grant_preview,
                "approvalDeadlineUtc": time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(created_at_ms.saturating_add(120_000)) * 1_000_000)
                    .ok()
                    .and_then(|value| value.format(&time::format_description::well_known::Rfc3339).ok()),
            }),
        );
        self.append_call_event(
            context,
            tool,
            "pending_approval",
            Some(&summary),
            None,
            None,
        )
        .await?;
        self.publish_parent_subagent_approval(
            &task_id,
            &approval_id,
            turn_id,
            tool,
            &summary,
            "subagent.approval_pending",
        )
        .await?;
        self.wait_for_approval(context, &task_id, &approval_id)
            .await
    }

    async fn publish_parent_subagent_approval(
        &self,
        child_task_id: &TaskId,
        approval_id: &str,
        child_turn_id: &str,
        tool: &str,
        arguments: &Value,
        event_type: &str,
    ) -> RuntimeResult<()> {
        let row = sqlx::query(
            "SELECT id,parent_task_id,parent_turn_id,name FROM subagent_runs WHERE child_task_id=? LIMIT 1",
        )
        .bind(child_task_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent approval routing lookup failed"))?;
        let Some(row) = row else {
            return Ok(());
        };
        let subagent_id = row.get::<String, _>("id");
        let parent_task_id = row.get::<String, _>("parent_task_id");
        let parent_turn_id = row.get::<String, _>("parent_turn_id");
        let agent_name = row.get::<String, _>("name");
        self.publish_event(
            format!("subagent-approval:{approval_id}:{}", now_ms()),
            event_type,
            Some(parent_task_id),
            Some(child_task_id.as_str().to_owned()),
            Some(parent_turn_id),
            json!({
                "subagentId": subagent_id,
                "childTaskId": child_task_id.as_str(),
                "activityId": approval_id,
                "childTurnId": child_turn_id,
                "agentName": agent_name,
                "tool": tool,
                "input": arguments,
            }),
        );
        Ok(())
    }

    async fn execution_mode_task_id(&self, task_id: &TaskId) -> RuntimeResult<TaskId> {
        let parent_task_id = sqlx::query_scalar::<_, String>(
            "SELECT parent_task_id FROM subagent_runs WHERE child_task_id=? LIMIT 1",
        )
        .bind(task_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| {
            RuntimeError::new("storage_error", "sub-agent approval scope lookup failed")
        })?;
        match parent_task_id {
            Some(parent_task_id) => {
                TaskId::new(parent_task_id).map_err(|error| invalid("taskId", error))
            }
            None => Ok(task_id.clone()),
        }
    }

    async fn resolve_approval_paths(
        &self,
        context: &OperationContext,
        tool: &str,
        arguments: &Value,
    ) -> RuntimeResult<Value> {
        if !tool.starts_with("fs_") && !tool.starts_with("workspace_index_") {
            return Ok(arguments.clone());
        }
        let task_id = TaskId::new(context.task_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("taskId", error))?;
        let base = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?
            .and_then(|task| task.project_folder)
            .map(PathBuf::from);
        super::filesystem_dispatch::resolve_relative_paths(arguments.clone(), base.as_deref())
    }

    async fn safe_read_grant_preview(
        &self,
        context: &OperationContext,
        tool: &str,
    ) -> RuntimeResult<Value> {
        let scopes = self.approval_path_scopes(context).await?;
        Ok(json!({
            "allowedTools": [tool], "pathScopes": scopes, "maxCalls": SAFE_READ_MAX_CALLS,
            "maxFilesScanned": SAFE_READ_MAX_FILES, "maxBytesRead": SAFE_READ_MAX_BYTES,
            "expiresAtMs": now_ms().saturating_add(SAFE_READ_GRANT_TTL_MS),
            "optionConstraints": {"includeIgnored": false, "includeHidden": false}
        }))
    }

    async fn approval_path_scopes(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<Vec<GrantPathScope>> {
        let task_id = TaskId::new(context.task_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("taskId", error))?;
        let mut paths = self.task_user_path_scopes(context).await?;
        if let Some(project) = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?
            .and_then(|task| task.project_folder)
            .map(PathBuf::from)
        {
            paths.push(project);
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|path| {
                let canonical = std::fs::canonicalize(&path).map_err(|_| {
                    RuntimeError::new(
                        "approval_scope_invalid",
                        "approval path scope no longer exists",
                    )
                })?;
                let kind = if canonical.is_dir() {
                    GrantPathScopeKind::Subtree
                } else {
                    GrantPathScopeKind::Exact
                };
                Ok(GrantPathScope {
                    path: normalized_path(&canonical),
                    kind,
                    identity: path_identity(&canonical),
                })
            })
            .collect()
    }

    async fn consume_safe_read_grant(
        &self,
        context: &OperationContext,
        tool: &str,
        arguments: &Value,
    ) -> RuntimeResult<bool> {
        if arguments
            .get("includeIgnored")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || arguments
                .get("includeHidden")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return Ok(false);
        }
        let task_id = context.task_id.as_deref().unwrap_or_default();
        let now = now_ms();
        sqlx::query("UPDATE approval_grants SET state='expired',updated_at_ms=? WHERE task_id=? AND state='active' AND expires_at_ms<=?")
            .bind(now).bind(task_id).bind(now).execute(self.repository.pool()).await.map_err(|_| RuntimeError::new("storage_error", "approval grant expiry failed"))?;
        let rows = sqlx::query("SELECT id,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,max_files_scanned,max_bytes_read FROM approval_grants WHERE task_id=? AND owner_agent_id=? AND state='active' AND expires_at_ms>? AND catalog_hash=? AND child_attempt IS (SELECT attempt FROM subagent_runs WHERE child_task_id=? LIMIT 1) ORDER BY created_at_ms DESC")
            .bind(task_id).bind(&context.agent_id).bind(now).bind(catalog_hash()).bind(task_id).fetch_all(self.repository.pool()).await.map_err(|_| RuntimeError::new("storage_error", "approval grant lookup failed"))?;
        let paths = extract_paths(tool, arguments)?;
        let charge = requested_charge(tool, arguments);
        for row in rows {
            let tools: Vec<String> =
                serde_json::from_str(&row.get::<String, _>("allowed_tools_json"))
                    .unwrap_or_default();
            if !tools.iter().any(|value| value == tool) {
                continue;
            }
            let scopes: Vec<GrantPathScope> =
                serde_json::from_str(&row.get::<String, _>("path_scopes_json")).unwrap_or_default();
            if paths.iter().any(|path| !path_allowed(path, &scopes)) {
                record_grant_denial(
                    self.repository.pool(),
                    &row.get::<String, _>("id"),
                    task_id,
                    tool,
                    paths.len(),
                    "path scope mismatch",
                )
                .await?;
                continue;
            }
            let id = row.get::<String, _>("id");
            let mut transaction = self.repository.pool().begin().await.map_err(|_| {
                RuntimeError::new("storage_error", "approval grant transaction failed")
            })?;
            let affected = sqlx::query("UPDATE approval_grants SET used_calls=used_calls+1,used_files_scanned=used_files_scanned+?,used_bytes_read=used_bytes_read+?,updated_at_ms=?,state=CASE WHEN used_calls+1>=max_calls THEN 'exhausted' ELSE state END WHERE id=? AND state='active' AND used_calls+1<=max_calls AND (max_files_scanned IS NULL OR used_files_scanned+?<=max_files_scanned) AND (max_bytes_read IS NULL OR used_bytes_read+?<=max_bytes_read)")
                .bind(charge.files).bind(charge.bytes_read).bind(now).bind(&id).bind(charge.files).bind(charge.bytes_read).execute(&mut *transaction).await.map_err(|_| RuntimeError::new("storage_error", "approval grant budget consumption failed"))?.rows_affected();
            if affected == 1 {
                sqlx::query("INSERT INTO approval_grant_audit(id,grant_id,task_id,event,tool,path_count,calls,files_scanned,bytes_read,bytes_written,created_at_ms) VALUES(?,?,?,'used',?,?,1,?,?,0,?)")
                    .bind(Uuid::new_v4().to_string()).bind(&id).bind(task_id).bind(tool)
                    .bind(i64::try_from(paths.len()).unwrap_or(i64::MAX)).bind(charge.files).bind(charge.bytes_read).bind(now)
                    .execute(&mut *transaction).await.map_err(|_| RuntimeError::new("storage_error", "approval grant audit failed"))?;
                transaction.commit().await.map_err(|_| {
                    RuntimeError::new("storage_error", "approval grant transaction failed")
                })?;
                return Ok(true);
            }
            transaction.rollback().await.map_err(|_| {
                RuntimeError::new("storage_error", "approval grant transaction failed")
            })?;
            record_grant_denial(
                self.repository.pool(),
                &id,
                task_id,
                tool,
                paths.len(),
                "resource budget exhausted",
            )
            .await?;
        }
        Ok(false)
    }

    async fn wait_for_approval(
        &self,
        context: &OperationContext,
        task_id: &TaskId,
        approval_id: &str,
    ) -> RuntimeResult<()> {
        let deadline = Instant::now() + APPROVAL_TIMEOUT;
        loop {
            let row = sqlx::query(
                "SELECT state,decision_json FROM approvals WHERE id=? AND task_id=? LIMIT 1",
            )
            .bind(approval_id)
            .bind(task_id.as_str())
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "approval state lookup failed"))?
            .ok_or_else(|| RuntimeError::new("approval_missing", "approval request disappeared"))?;
            let state = row.get::<String, _>("state");
            let decision_json = row.get::<Option<String>, _>("decision_json");
            match state.as_str() {
                "approved" => return Ok(()),
                "rejected" => {
                    let reason = rejection_reason(decision_json.as_deref());
                    return Err(RuntimeError::new(
                        "command_rejected_by_user",
                        reason.map_or_else(
                            || "the user rejected this operation".to_owned(),
                            |value| format!("the user rejected this operation: {value}"),
                        ),
                    ));
                }
                "cancelled" => {
                    return Err(RuntimeError::new(
                        "approval_cancelled",
                        "approval was cancelled",
                    ));
                }
                "expired" => {
                    return Err(RuntimeError::new(
                        "approval_timeout",
                        "command approval timed out before the user responded",
                    ));
                }
                "pending" => {}
                _ => {
                    return Err(RuntimeError::new(
                        "approval_state_invalid",
                        "approval has an invalid state",
                    ));
                }
            }
            if Instant::now() >= deadline {
                self.expire_approval(approval_id).await?;
                return Err(RuntimeError::new(
                    "approval_timeout",
                    "command approval timed out before the user responded",
                ));
            }
            tokio::select! {
                () = context.cancellation.cancelled() => {
                    self.cancel_approval(approval_id).await?;
                    return Err(RuntimeError::new("cancelled", "operation was cancelled while waiting for approval"));
                }
                () = tokio::time::sleep(APPROVAL_POLL_INTERVAL) => {}
            }
        }
    }

    async fn expire_approval(&self, approval_id: &str) -> RuntimeResult<()> {
        self.resolve_waiting_approval(approval_id, "expired").await
    }

    async fn cancel_approval(&self, approval_id: &str) -> RuntimeResult<()> {
        self.resolve_waiting_approval(approval_id, "cancelled")
            .await
    }

    async fn resolve_waiting_approval(&self, approval_id: &str, state: &str) -> RuntimeResult<()> {
        sqlx::query("UPDATE approvals SET state=?,resolved_at_ms=? WHERE id=? AND state='pending'")
            .bind(state)
            .bind(now_ms())
            .bind(approval_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "approval state update failed"))?;
        Ok(())
    }
}

fn operation_digest(tool: &str, arguments: &Value) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(format!("{tool}\n{arguments}").as_bytes())
    )
}

fn approval_summary(tool: &str, risk: ToolRiskClass, arguments: &Value) -> Value {
    let paths = extract_paths(tool, arguments)
        .unwrap_or_default()
        .into_iter()
        .map(|path| normalized_path(&path))
        .collect::<Vec<_>>();
    json!({"operation": tool, "riskClass": risk, "paths": paths, "pathCount": paths.len(),
        "overwrite": arguments.get("overwrite"), "recursive": arguments.get("recursive"),
        "deleteMode": arguments.get("mode"), "expectedVersion": arguments.get("expectedVersion"),
        "dryRun": arguments.get("dryRun"), "budget": arguments.get("budget"),
        "editCount": edit_count(arguments), "contentBytesEstimate": content_bytes_estimate(arguments),
        "contentRedacted": true})
}

fn edit_count(arguments: &Value) -> usize {
    arguments
        .get("edits")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn content_bytes_estimate(arguments: &Value) -> usize {
    let direct = arguments
        .get("content")
        .and_then(Value::as_str)
        .map_or(0, str::len)
        .saturating_add(
            arguments
                .get("base64")
                .and_then(Value::as_str)
                .map_or(0, |v| v.len().saturating_mul(3) / 4),
        );
    arguments
        .get("edits")
        .and_then(Value::as_array)
        .map_or(direct, |edits| {
            edits.iter().fold(direct, |total, edit| {
                total.saturating_add(edit.get("text").and_then(Value::as_str).map_or(0, str::len))
            })
        })
}

fn requested_charge(tool: &str, arguments: &Value) -> GrantCharge {
    let budget = arguments.get("budget").unwrap_or(&Value::Null);
    let files = budget
        .get("maxFiles")
        .or_else(|| budget.get("maxFilesScanned"))
        .and_then(Value::as_i64)
        .unwrap_or(if matches!(tool, "fs_search" | "fs_find") {
            10_000
        } else {
            1
        })
        .clamp(0, SAFE_READ_MAX_FILES);
    let bytes_read = budget
        .get("maxBytesRead")
        .and_then(Value::as_i64)
        .or_else(|| arguments.get("maxBytes").and_then(Value::as_i64))
        .unwrap_or(if matches!(tool, "fs_search" | "fs_find") {
            64 * 1024 * 1024
        } else {
            1024 * 1024
        })
        .clamp(0, SAFE_READ_MAX_BYTES);
    GrantCharge { files, bytes_read }
}

fn extract_paths(tool: &str, arguments: &Value) -> RuntimeResult<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for role in tool_capabilities(tool).path_fields {
        match role {
            PathFieldRole::Path
            | PathFieldRole::Source
            | PathFieldRole::Destination
            | PathFieldRole::WorkingDirectory
            | PathFieldRole::Cwd => {
                let key = match role {
                    PathFieldRole::Path => "path",
                    PathFieldRole::Source => "source",
                    PathFieldRole::Destination => "destination",
                    PathFieldRole::WorkingDirectory => "workingDirectory",
                    PathFieldRole::Cwd => "cwd",
                    _ => unreachable!(),
                };
                if let Some(path) = arguments.get(key).and_then(Value::as_str) {
                    paths.push(canonical_read_path(path)?);
                }
            }
            PathFieldRole::Paths => {
                if let Some(values) = arguments.get("paths").and_then(Value::as_array) {
                    for path in values.iter().filter_map(Value::as_str) {
                        paths.push(canonical_read_path(path)?);
                    }
                }
            }
            PathFieldRole::RequestPaths => {
                if let Some(values) = arguments.get("requests").and_then(Value::as_array) {
                    for path in values
                        .iter()
                        .filter_map(|v| v.get("path"))
                        .filter_map(Value::as_str)
                    {
                        paths.push(canonical_read_path(path)?);
                    }
                }
            }
        }
    }
    Ok(paths)
}

fn canonical_read_path(path: &str) -> RuntimeResult<PathBuf> {
    std::fs::canonicalize(path).map_err(|_| {
        RuntimeError::new(
            "approval_path_invalid",
            "approval path does not exist or cannot be canonicalized",
        )
    })
}
fn normalized_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}
fn path_allowed(path: &Path, scopes: &[GrantPathScope]) -> bool {
    let path = normalized_path(path);
    scopes.iter().any(|scope| {
        let scope_path = Path::new(&scope.path);
        let scope_still_bound = std::fs::canonicalize(scope_path).is_ok_and(|canonical| {
            normalized_path(&canonical) == scope.path && scope.identity == path_identity(&canonical)
        });
        scope_still_bound
            && match scope.kind {
                GrantPathScopeKind::Exact => path == scope.path,
                GrantPathScopeKind::Subtree => {
                    path == scope.path
                        || path
                            .strip_prefix(&scope.path)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                }
            }
    })
}

fn path_identity(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        return Some(format!("unix:{}:{}", metadata.dev(), metadata.ino()));
    }
    #[cfg(not(unix))]
    {
        let created = metadata
            .created()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(format!("created:{created}:dir:{}", metadata.is_dir()))
    }
}

async fn record_grant_denial(
    pool: &sqlx::SqlitePool,
    grant_id: &str,
    task_id: &str,
    tool: &str,
    path_count: usize,
    reason: &str,
) -> RuntimeResult<()> {
    sqlx::query("INSERT INTO approval_grant_audit(id,grant_id,task_id,event,tool,path_count,reason,created_at_ms) VALUES(?,?,?,'denied',?,?,?,?)")
        .bind(Uuid::new_v4().to_string()).bind(grant_id).bind(task_id).bind(tool)
        .bind(i64::try_from(path_count).unwrap_or(i64::MAX)).bind(reason).bind(now_ms())
        .execute(pool).await.map_err(|_| RuntimeError::new("storage_error", "approval grant denial audit failed"))?;
    Ok(())
}

fn rejection_reason(decision_json: Option<&str>) -> Option<String> {
    serde_json::from_str::<Value>(decision_json?)
        .ok()?
        .get("reason")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_execution_and_workspace_tools_require_approval() {
        assert!(tool_capabilities("shell_write").approval_required);
        assert!(tool_capabilities("fs_read_text").approval_required);
        assert!(tool_capabilities("git_status").approval_required);
        assert!(tool_capabilities("process_kill").approval_required);
        assert!(!tool_capabilities("agent_progress").approval_required);
        assert!(!tool_capabilities("task_get").approval_required);
    }

    #[test]
    fn subtree_matching_does_not_allow_prefix_siblings() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("repo");
        let sibling = directory.path().join("repository");
        std::fs::create_dir_all(root.join("src")).expect("project tree");
        std::fs::create_dir_all(&sibling).expect("sibling tree");
        let root = std::fs::canonicalize(root).expect("canonical root");
        let scopes = vec![GrantPathScope {
            path: normalized_path(&root),
            kind: GrantPathScopeKind::Subtree,
            identity: path_identity(&root),
        }];
        assert!(path_allowed(&root.join("src"), &scopes));
        assert!(!path_allowed(&sibling, &scopes));
    }

    #[test]
    fn mutation_summary_redacts_content_and_digest_binds_it() {
        let first = json!({"path": ".", "content": "top-secret", "expectedVersion": "v1"});
        let second = json!({"path": ".", "content": "changed", "expectedVersion": "v1"});
        let summary = approval_summary("fs_write_text", ToolRiskClass::Modify, &first);
        assert!(!summary.to_string().contains("top-secret"));
        assert_eq!(summary["contentBytesEstimate"], 10);
        assert_ne!(
            operation_digest("fs_write_text", &first),
            operation_digest("fs_write_text", &second)
        );
    }

    #[test]
    fn apply_edits_summary_exposes_counts_without_replacement_payload() {
        let replacement = "sensitive replacement payload";
        let arguments = json!({
            "path": ".",
            "expectedVersion": "v1-test",
            "edits": [
                {"startByte": 1, "endByte": 2, "text": replacement},
                {"startByte": 4, "endByte": 4, "text": "xy"}
            ],
            "dryRun": false,
            "budget": {"maxBytesRead": 4096, "maxBytesWritten": 4096}
        });
        let summary = approval_summary("fs_apply_edits", ToolRiskClass::Modify, &arguments);
        let serialized = summary.to_string();

        assert_eq!(summary["expectedVersion"], "v1-test");
        assert_eq!(summary["editCount"], 2);
        assert_eq!(summary["contentBytesEstimate"], replacement.len() + 2);
        assert_eq!(summary["contentRedacted"], true);
        assert!(!serialized.contains(replacement));
        assert!(serialized.contains("fs_apply_edits"));
    }

    #[test]
    fn read_and_mutation_risk_classes_do_not_overlap() {
        assert!(tool_capabilities("fs_search").risk_class.is_safe_read());
        assert!(!tool_capabilities("fs_delete").risk_class.is_safe_read());
        assert!(!tool_capabilities("git_status").risk_class.is_safe_read());
    }
}
