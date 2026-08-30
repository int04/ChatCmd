use chatcmd_runtime::{RuntimeError, RuntimeResult};
use sqlx::Row as _;
use uuid::Uuid;

use super::{RuntimeHost, now_ms};

impl RuntimeHost {
    pub(super) async fn claim_unbound_chatgpt_bridge_task(
        &self,
        agent_id: &str,
        conversation_scope: Option<&str>,
        first_user_message: Option<&str>,
    ) -> RuntimeResult<Option<String>> {
        let cutoff = now_ms().saturating_sub(90_000);
        let rows = if let Some(message) = first_user_message {
            sqlx::query(
                r#"SELECT id,user_content,submitted_content,created_at_ms
                   FROM chatgpt_bridge_requests
                   WHERE task_id IS NULL AND agent_id=?
                     AND submitted_content=?
                     AND status IN ('queued','running','stop_requested')
                     AND updated_at_ms>=?
                   ORDER BY updated_at_ms DESC,id DESC
                   LIMIT 2"#,
            )
            .bind(agent_id)
            .bind(message)
            .bind(cutoff)
            .fetch_all(self.repository.pool())
            .await
        } else {
            sqlx::query(
                r#"SELECT id,user_content,submitted_content,created_at_ms
                   FROM chatgpt_bridge_requests
                   WHERE task_id IS NULL AND agent_id=?
                     AND status IN ('queued','running','stop_requested')
                     AND updated_at_ms>=?
                   ORDER BY updated_at_ms DESC,id DESC
                   LIMIT 2"#,
            )
            .bind(agent_id)
            .bind(cutoff)
            .fetch_all(self.repository.pool())
            .await
        }
        .map_err(|_| RuntimeError::new("storage_error", "unbound ChatGPT bridge lookup failed"))?;

        if rows.len() != 1 {
            return Ok(None);
        }
        let row = &rows[0];
        let request_id = row.get::<String, _>("id");
        let user_content = row.get::<String, _>("user_content");
        let created_at_ms = row.get::<i64, _>("created_at_ms");
        let task_id = pending_bridge_task_id(agent_id, &request_id);
        let now = now_ms();

        sqlx::query(
            "INSERT OR IGNORE INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,allow_execute,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web',1,'running',NULL,1,NULL,?,?)",
        )
        .bind(&task_id)
        .bind(agent_id)
        .bind(self.device.id.as_str())
        .bind(conversation_scope)
        .bind(compact_title(&user_content))
        .bind(created_at_ms)
        .bind(now)
        .execute(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "ChatGPT bridge task claim could not be created"))?;

        sqlx::query("UPDATE chatgpt_bridge_requests SET task_id=?,updated_at_ms=? WHERE id=? AND task_id IS NULL")
            .bind(&task_id)
            .bind(now)
            .bind(&request_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "ChatGPT bridge request claim failed"))?;

        let claimed = sqlx::query_scalar::<_, String>(
            "SELECT task_id FROM chatgpt_bridge_requests WHERE id=? LIMIT 1",
        )
        .bind(&request_id)
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| {
            RuntimeError::new(
                "storage_error",
                "ChatGPT bridge request claim lookup failed",
            )
        })?;
        Ok(claimed)
    }
}

fn pending_bridge_task_id(agent_id: &str, request_id: &str) -> String {
    let material = format!("task-chat\0agent:{agent_id}\0bridge-request:{request_id}");
    format!(
        "task-chat-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

fn compact_title(content: &str) -> String {
    content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(72)
        .collect()
}
