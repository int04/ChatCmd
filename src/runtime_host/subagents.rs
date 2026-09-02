use std::time::Duration;

use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use sqlx::Row as _;
use tokio::time::{Instant, sleep};
use uuid::Uuid;

use super::{RuntimeHost, now_ms};

const SUBAGENT_MARKER_PREFIX: &str = "CMDGPT_SUBAGENT_ID=";
const MAX_SUBAGENT_NAME_CHARS: usize = 120;
const MAX_SUBAGENT_REQUEST_CHARS: usize = 20_000;
const UNCLAIMED_SUBAGENT_TIMEOUT_MS: i64 = 60_000;
const EXTENSION_UNCLAIMED_SUBAGENT_TIMEOUT_MS: i64 = 180_000;

impl RuntimeHost {
    pub(super) async fn register_subagent(
        &self,
        context: &OperationContext,
        name: &str,
        request: &str,
    ) -> RuntimeResult<Value> {
        let parent_task_id = required_context_value(context.task_id.as_deref(), "taskId")?;
        let parent_turn_id = required_context_value(context.turn_id.as_deref(), "turnId")?;
        let name = validate_text("name", name, MAX_SUBAGENT_NAME_CHARS)?;
        let request = validate_text("request", request, MAX_SUBAGENT_REQUEST_CHARS)?;
        let deterministic_id =
            subagent_id_for_registration(parent_task_id, parent_turn_id, name, request);
        let deterministic_task_id = child_task_id_for_subagent(&deterministic_id);
        let now = now_ms();
        let mut transaction = self
            .repository
            .pool()
            .begin()
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent transaction failed"))?;

        if let Some(row) = sqlx::query(
            "SELECT id,child_task_id,status FROM subagent_runs WHERE parent_task_id=? AND parent_turn_id=? AND name=? AND request=? ORDER BY created_at_ms,id LIMIT 1",
        )
        .bind(parent_task_id)
        .bind(parent_turn_id)
        .bind(name)
        .bind(request)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent idempotency lookup failed"))?
        {
            let subagent_id = row.get::<String, _>("id");
            let child_task_id = row
                .get::<Option<String>, _>("child_task_id")
                .unwrap_or_else(|| child_task_id_for_subagent(&subagent_id));
            let status = row.get::<String, _>("status");
            transaction.commit().await.map_err(|_| {
                RuntimeError::new("storage_error", "sub-agent transaction commit failed")
            })?;
            return Ok(subagent_registration_value(
                &subagent_id,
                &child_task_id,
                name,
                &status,
                true,
            ));
        }

        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) SELECT ?,agent_id,device_id,NULL,?,'mcp',project_folder,'pending',NULL,1,NULL,?,? FROM tasks WHERE id=? ON CONFLICT(id) DO NOTHING")
            .bind(&deterministic_task_id)
            .bind(name)
            .bind(now)
            .bind(now)
            .bind(parent_task_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent task reservation failed"))?;
        let inserted = sqlx::query("INSERT INTO subagent_runs(id,parent_task_id,parent_turn_id,child_task_id,name,request,status,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,'pending',?,?) ON CONFLICT(id) DO NOTHING")
            .bind(&deterministic_id)
            .bind(parent_task_id)
            .bind(parent_turn_id)
            .bind(&deterministic_task_id)
            .bind(name)
            .bind(request)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent registration failed"))?
            .rows_affected();
        let status = if inserted == 0 {
            sqlx::query_scalar::<_, String>("SELECT status FROM subagent_runs WHERE id=?")
                .bind(&deterministic_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|_| {
                    RuntimeError::new("storage_error", "sub-agent duplicate lookup failed")
                })?
        } else {
            "pending".to_owned()
        };
        transaction.commit().await.map_err(|_| {
            RuntimeError::new("storage_error", "sub-agent transaction commit failed")
        })?;

        if inserted != 0 {
            self.publish_subagent_status(
                parent_task_id,
                parent_turn_id,
                &deterministic_id,
                Some(&deterministic_task_id),
                name,
                "pending",
            );
        }
        Ok(subagent_registration_value(
            &deterministic_id,
            &deterministic_task_id,
            name,
            &status,
            inserted == 0,
        ))
    }

    pub(super) async fn delegated_subagent_task_id(
        &self,
        context: &OperationContext,
        first_user_message: Option<&str>,
    ) -> RuntimeResult<Option<String>> {
        let Some(subagent_id) = first_user_message.and_then(extract_subagent_id) else {
            return Ok(None);
        };
        let row = sqlx::query("SELECT r.child_task_id FROM subagent_runs r JOIN tasks parent ON parent.id=r.parent_task_id WHERE r.id=? AND parent.agent_id=? LIMIT 1")
            .bind(&subagent_id)
            .bind(&context.agent_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent task lookup failed"))?;
        let Some(row) = row else {
            return Err(RuntimeError::new(
                "subagent_not_found",
                "delegated sub-agent marker is invalid or belongs to another agent",
            ));
        };
        Ok(Some(
            row.get::<Option<String>, _>("child_task_id")
                .unwrap_or_else(|| child_task_id_for_subagent(&subagent_id)),
        ))
    }

    pub(super) async fn claim_subagent_from_message(
        &self,
        context: &OperationContext,
        child_task_id: &str,
        first_user_message: Option<&str>,
    ) -> RuntimeResult<()> {
        let Some(subagent_id) = first_user_message.and_then(extract_subagent_id) else {
            return Ok(());
        };
        let row = sqlx::query("SELECT r.parent_task_id,r.parent_turn_id,r.name,r.child_task_id,r.status AS registered_status,r.fallback_state FROM subagent_runs r JOIN tasks parent ON parent.id=r.parent_task_id WHERE r.id=? AND parent.agent_id=? LIMIT 1")
            .bind(&subagent_id)
            .bind(&context.agent_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent claim lookup failed"))?;
        let Some(row) = row else {
            return Err(RuntimeError::new(
                "subagent_not_found",
                "delegated sub-agent marker is invalid or belongs to another agent",
            ));
        };
        let parent_task_id = row.get::<String, _>("parent_task_id");
        if parent_task_id == child_task_id {
            return Err(RuntimeError::new(
                "invalid_subagent",
                "a task cannot be its own sub-agent",
            ));
        }
        let existing_child = row.get::<Option<String>, _>("child_task_id");
        if existing_child
            .as_deref()
            .is_some_and(|value| value != child_task_id)
        {
            return Err(RuntimeError::new(
                "subagent_already_claimed",
                "delegated sub-agent marker was already claimed by another task",
            ));
        }
        let registered_status = row.get::<String, _>("registered_status");
        if !matches!(registered_status.as_str(), "pending" | "running") {
            return Err(RuntimeError::new(
                "subagent_not_active",
                format!("delegated sub-agent is already {registered_status}"),
            ));
        }
        if registered_status == "running" {
            return Ok(());
        }
        let parent_turn_id = row.get::<String, _>("parent_turn_id");
        let name = row.get::<String, _>("name");
        let fallback_state = row.get::<String, _>("fallback_state");
        let fallback_claim = matches!(fallback_state.as_str(), "requested" | "started");
        let now = now_ms();
        let claimed = sqlx::query("UPDATE subagent_runs SET child_task_id=?,status='running',fallback_state=CASE WHEN fallback_state IN ('requested','started') THEN 'claimed' ELSE fallback_state END,updated_at_ms=?,completed_at_ms=NULL WHERE id=? AND status='pending' AND (child_task_id IS NULL OR child_task_id=?)")
            .bind(child_task_id)
            .bind(now)
            .bind(&subagent_id)
            .bind(child_task_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent claim failed"))?;
        if claimed.rows_affected() != 1 {
            let current =
                sqlx::query("SELECT child_task_id,status FROM subagent_runs WHERE id=? LIMIT 1")
                    .bind(&subagent_id)
                    .fetch_optional(self.repository.pool())
                    .await
                    .map_err(|_| {
                        RuntimeError::new("storage_error", "sub-agent claim refresh failed")
                    })?;
            if current.as_ref().is_some_and(|current| {
                current.get::<String, _>("status") == "running"
                    && current.get::<Option<String>, _>("child_task_id").as_deref()
                        == Some(child_task_id)
            }) {
                return Ok(());
            }
            let current_status = current
                .as_ref()
                .map(|current| current.get::<String, _>("status"))
                .unwrap_or_else(|| "missing".to_owned());
            return Err(RuntimeError::new(
                "subagent_not_active",
                format!("delegated sub-agent is already {current_status}"),
            ));
        }
        self.publish_subagent_status(
            &parent_task_id,
            &parent_turn_id,
            &subagent_id,
            Some(child_task_id),
            &name,
            "running",
        );
        if fallback_claim {
            self.publish_event(
                format!("subagent-fallback-claimed-{subagent_id}-{now}"),
                "subagent.fallback_claimed",
                Some(parent_task_id),
                None,
                Some(parent_turn_id),
                json!({
                    "subagentId": subagent_id,
                    "childTaskId": child_task_id,
                    "name": name
                }),
            );
        }
        Ok(())
    }

    pub(super) async fn wait_for_subagents(
        &self,
        context: &OperationContext,
        timeout_ms: u64,
    ) -> RuntimeResult<Value> {
        let parent_task_id = required_context_value(context.task_id.as_deref(), "taskId")?;
        let parent_turn_id = required_context_value(context.turn_id.as_deref(), "turnId")?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.clamp(250, 40_000));
        loop {
            self.expire_unclaimed_subagents(parent_task_id, parent_turn_id)
                .await?;
            let runs = self
                .subagent_values(parent_task_id, Some(parent_turn_id))
                .await?;
            let pending_count = runs
                .iter()
                .filter(|run| run.get("status").and_then(Value::as_str) == Some("pending"))
                .count();
            let running_count = runs
                .iter()
                .filter(|run| run.get("status").and_then(Value::as_str) == Some("running"))
                .count();
            let active_count = pending_count + running_count;
            if active_count == 0 || Instant::now() >= deadline {
                let all_completed = !runs.is_empty()
                    && runs
                        .iter()
                        .all(|run| run.get("status").and_then(Value::as_str) == Some("completed"));
                return Ok(json!({
                    "allFinished": active_count == 0,
                    "allCompleted": all_completed,
                    "pendingCount": pending_count,
                    "runningCount": running_count,
                    "activeCount": active_count,
                    "subagents": runs,
                    "instruction": if active_count == 0 { "All registered sub-agents are finished. You may finalize the parent turn." } else if pending_count > 0 && running_count == 0 { "Some sub-agents are still pending and have not started. If dispatchMode was native, create the host-native children using delegatedPrompt. Otherwise call agent_subagent_wait again." } else { "Some sub-agents are still pending or running. Call agent_subagent_wait again before agent_turn_complete." }
                }));
            }
            sleep(Duration::from_millis(250)).await;
        }
    }

    pub(super) async fn ensure_subagents_finished(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<()> {
        let parent_task_id = required_context_value(context.task_id.as_deref(), "taskId")?;
        let parent_turn_id = required_context_value(context.turn_id.as_deref(), "turnId")?;
        self.expire_unclaimed_subagents(parent_task_id, parent_turn_id)
            .await?;
        let runs = self
            .subagent_values(parent_task_id, Some(parent_turn_id))
            .await?;
        let active = runs
            .iter()
            .filter_map(|run| {
                let status = run.get("status").and_then(Value::as_str)?;
                is_pending_status(Some(status)).then(|| {
                    format!(
                        "{} ({status})",
                        run.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unnamed child")
                    )
                })
            })
            .collect::<Vec<_>>();
        if active.is_empty() {
            return Ok(());
        }
        Err(RuntimeError::new(
            "subagents_still_running",
            format!(
                "{} child agent(s) are still pending/running: {}. Call agent_subagent_wait until allFinished=true before agent_turn_complete",
                active.len(),
                active.join(", ")
            ),
        ))
    }

    pub(super) async fn finish_subagent_for_child(
        &self,
        child_task_id: &str,
        status: &str,
    ) -> RuntimeResult<()> {
        let row = sqlx::query("SELECT id,parent_task_id,parent_turn_id,name FROM subagent_runs WHERE child_task_id=? LIMIT 1")
            .bind(child_task_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent completion lookup failed"))?;
        let Some(row) = row else {
            return Ok(());
        };
        let terminal_status = normalize_terminal_status(status);
        let now = now_ms();
        sqlx::query("UPDATE subagent_runs SET status=?,updated_at_ms=?,completed_at_ms=? WHERE child_task_id=?")
            .bind(terminal_status)
            .bind(now)
            .bind(now)
            .bind(child_task_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent completion update failed"))?;
        let subagent_id = row.get::<String, _>("id");
        let parent_task_id = row.get::<String, _>("parent_task_id");
        let parent_turn_id = row.get::<String, _>("parent_turn_id");
        let name = row.get::<String, _>("name");
        self.publish_subagent_status(
            &parent_task_id,
            &parent_turn_id,
            &subagent_id,
            Some(child_task_id),
            &name,
            terminal_status,
        );
        Ok(())
    }

    pub(super) async fn fail_subagent_worker(
        &self,
        child_task_id: &str,
        message: &str,
    ) -> RuntimeResult<()> {
        let now = now_ms();
        let affected = sqlx::query("UPDATE tasks SET status='failed',active_session_id=NULL,updated_at_ms=? WHERE id=? AND status NOT IN ('completed','stopped')")
            .bind(now)
            .bind(child_task_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "child task failure state update failed"))?
            .rows_affected();
        if affected == 0 {
            return Ok(());
        }
        self.finish_subagent_for_child(child_task_id, "failed")
            .await?;
        self.publish_event(
            format!("subagent-worker-failed:{child_task_id}:{now}"),
            "status",
            Some(child_task_id.to_owned()),
            None,
            None,
            json!({ "status": "failed", "content": message }),
        );
        Ok(())
    }

    async fn expire_unclaimed_subagents(
        &self,
        parent_task_id: &str,
        parent_turn_id: &str,
    ) -> RuntimeResult<()> {
        let now = now_ms();
        let native_cutoff = now.saturating_sub(UNCLAIMED_SUBAGENT_TIMEOUT_MS);
        let extension_cutoff = now.saturating_sub(EXTENSION_UNCLAIMED_SUBAGENT_TIMEOUT_MS);
        let rows = sqlx::query("SELECT child_task_id,fallback_state FROM subagent_runs WHERE parent_task_id=? AND parent_turn_id=? AND status='pending' AND child_task_id IS NOT NULL AND ((fallback_state='none' AND created_at_ms<=?) OR (fallback_state IN ('requested','started') AND updated_at_ms<=?))")
            .bind(parent_task_id)
            .bind(parent_turn_id)
            .bind(native_cutoff)
            .bind(extension_cutoff)
            .fetch_all(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "stale sub-agent lookup failed"))?;
        for row in rows {
            let child_task_id = row.get::<String, _>("child_task_id");
            let fallback_state = row.get::<String, _>("fallback_state");
            let message = if matches!(fallback_state.as_str(), "requested" | "started") {
                "Child agent browser fallback did not claim MCP or return a final response before the fallback timeout."
            } else {
                "Child agent was registered but no worker claimed it within 60 seconds."
            };
            self.fail_subagent_worker(&child_task_id, message).await?;
        }
        Ok(())
    }

    async fn subagent_values(
        &self,
        parent_task_id: &str,
        parent_turn_id: Option<&str>,
    ) -> RuntimeResult<Vec<Value>> {
        let rows = if let Some(turn_id) = parent_turn_id {
            sqlx::query("SELECT r.id,r.parent_turn_id,r.child_task_id,r.name,r.request,r.status AS registered_status,r.created_at_ms,r.updated_at_ms,r.completed_at_ms,t.status AS task_status FROM subagent_runs r LEFT JOIN tasks t ON t.id=r.child_task_id WHERE r.parent_task_id=? AND r.parent_turn_id=? ORDER BY r.created_at_ms,r.id")
                .bind(parent_task_id)
                .bind(turn_id)
                .fetch_all(self.repository.pool())
                .await
        } else {
            sqlx::query("SELECT r.id,r.parent_turn_id,r.child_task_id,r.name,r.request,r.status AS registered_status,r.created_at_ms,r.updated_at_ms,r.completed_at_ms,t.status AS task_status FROM subagent_runs r LEFT JOIN tasks t ON t.id=r.child_task_id WHERE r.parent_task_id=? ORDER BY r.created_at_ms,r.id")
                .bind(parent_task_id)
                .fetch_all(self.repository.pool())
                .await
        }
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent state lookup failed"))?;

        Ok(rows.iter().map(subagent_row_value).collect())
    }

    fn publish_subagent_status(
        &self,
        parent_task_id: &str,
        parent_turn_id: &str,
        subagent_id: &str,
        child_task_id: Option<&str>,
        name: &str,
        status: &str,
    ) {
        self.publish_event(
            format!("subagent:{subagent_id}:{status}:{}", now_ms()),
            "subagent.status",
            Some(parent_task_id.to_owned()),
            child_task_id.map(str::to_owned),
            Some(parent_turn_id.to_owned()),
            json!({
                "subagentId": subagent_id,
                "childTaskId": child_task_id,
                "name": name,
                "status": status
            }),
        );
    }
}

fn subagent_row_value(row: &sqlx::sqlite::SqliteRow) -> Value {
    let registered = row.get::<String, _>("registered_status");
    let task_status = row.get::<Option<String>, _>("task_status");
    let status = effective_status(&registered, task_status.as_deref());
    json!({
        "id": row.get::<String, _>("id"),
        "parentTurnId": row.get::<String, _>("parent_turn_id"),
        "taskId": row.get::<Option<String>, _>("child_task_id"),
        "name": row.get::<String, _>("name"),
        "request": row.get::<String, _>("request"),
        "status": status,
        "createdAtMs": row.get::<i64, _>("created_at_ms"),
        "updatedAtMs": row.get::<i64, _>("updated_at_ms"),
        "completedAtMs": row.get::<Option<i64>, _>("completed_at_ms")
    })
}

fn effective_status<'a>(registered: &'a str, task_status: Option<&'a str>) -> &'a str {
    match task_status {
        Some("completed") => "completed",
        Some("failed") => "failed",
        Some("stopped") => "stopped",
        Some("interrupted") => "interrupted",
        Some("running") if registered == "pending" => "running",
        _ => registered,
    }
}

fn normalize_terminal_status(status: &str) -> &str {
    match status {
        "completed" => "completed",
        "failed" => "failed",
        "stopped" => "stopped",
        "interrupted" => "interrupted",
        _ => "completed",
    }
}

fn is_pending_status(status: Option<&str>) -> bool {
    matches!(status, Some("pending" | "running"))
}

fn required_context_value<'a>(value: Option<&'a str>, field: &str) -> RuntimeResult<&'a str> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::new("invalid_context", format!("{field} is required")))
}

fn validate_text<'a>(field: &str, value: &'a str, max_chars: usize) -> RuntimeResult<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(RuntimeError::new(
            "invalid_arguments",
            format!("{field} must not be empty"),
        ));
    }
    if value.chars().count() > max_chars {
        return Err(RuntimeError::new(
            "invalid_arguments",
            format!("{field} exceeds {max_chars} characters"),
        ));
    }
    Ok(value)
}

fn subagent_id_for_registration(
    parent_task_id: &str,
    parent_turn_id: &str,
    name: &str,
    request: &str,
) -> String {
    let material = format!("{parent_task_id}\0{parent_turn_id}\0{name}\0{request}");
    format!(
        "subagent-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

fn subagent_registration_value(
    subagent_id: &str,
    child_task_id: &str,
    name: &str,
    status: &str,
    duplicate: bool,
) -> Value {
    json!({
        "subagentId": subagent_id,
        "taskId": child_task_id,
        "childTaskId": child_task_id,
        "name": name,
        "status": status,
        "duplicate": duplicate,
        "delegationMarker": format!("{SUBAGENT_MARKER_PREFIX}{subagent_id}"),
        "instruction": "Include delegationMarker verbatim in the child agent request. The child must preserve it in its first agent_user_message call."
    })
}

fn child_task_id_for_subagent(subagent_id: &str) -> String {
    format!("task-{subagent_id}")
}

fn extract_subagent_id(message: &str) -> Option<String> {
    message.lines().find_map(|line| {
        let marker = line.find(SUBAGENT_MARKER_PREFIX)?;
        let tail = &line[marker + SUBAGENT_MARKER_PREFIX.len()..];
        let id = tail
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .collect::<String>();
        id.starts_with("subagent-").then_some(id)
    })
}

#[cfg(test)]
mod tests {
    use super::{child_task_id_for_subagent, extract_subagent_id, subagent_id_for_registration};

    #[test]
    fn registration_id_is_stable_for_semantic_retry() {
        let first = subagent_id_for_registration("parent", "turn", "Reader", "Read lib.rs");
        assert_eq!(
            first,
            subagent_id_for_registration("parent", "turn", "Reader", "Read lib.rs")
        );
        assert_ne!(
            first,
            subagent_id_for_registration("parent", "turn", "Reader", "Read another.rs")
        );
    }

    #[test]
    fn child_task_id_is_stable_from_subagent_id() {
        assert_eq!(
            child_task_id_for_subagent("subagent-1234-abcd"),
            "task-subagent-1234-abcd"
        );
    }

    #[test]
    fn extracts_subagent_marker_from_delegated_prompt() {
        assert_eq!(
            extract_subagent_id(
                "Please inspect this.\nCMDGPT_SUBAGENT_ID=subagent-1234-abcd\nKeep going."
            ),
            Some("subagent-1234-abcd".to_owned())
        );
    }

    #[test]
    fn ignores_unrelated_text() {
        assert_eq!(extract_subagent_id("normal delegated request"), None);
    }
}
