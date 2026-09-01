use chatcmd_runtime::{RuntimeError, RuntimeResult};
use sqlx::{Row as _, sqlite::SqliteRow};
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
        let rows = sqlx::query(
            r#"SELECT id,user_content,submitted_content,project_folder,created_at_ms
               FROM chatgpt_bridge_requests
               WHERE task_id IS NULL AND agent_id=?
                 AND status IN ('queued','running','stop_requested')
                 AND updated_at_ms>=?
               ORDER BY updated_at_ms DESC,id DESC
               LIMIT 32"#,
        )
        .bind(agent_id)
        .bind(cutoff)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "unbound ChatGPT bridge lookup failed"))?;

        let matching_rows = matching_unbound_rows(&rows, first_user_message);
        if matching_rows.len() != 1 {
            return Ok(None);
        }
        let row = matching_rows[0];
        let request_id = row.get::<String, _>("id");
        let user_content = row.get::<String, _>("user_content");
        let project_folder = row.get::<Option<String>, _>("project_folder");
        let created_at_ms = row.get::<i64, _>("created_at_ms");
        self.commit_unbound_chatgpt_bridge_claim(
            agent_id,
            conversation_scope,
            &PendingBridgeClaim {
                request_id,
                user_content,
                project_folder,
                created_at_ms,
            },
        )
        .await
    }

    async fn commit_unbound_chatgpt_bridge_claim(
        &self,
        agent_id: &str,
        conversation_scope: Option<&str>,
        claim: &PendingBridgeClaim,
    ) -> RuntimeResult<Option<String>> {
        let request_id = &claim.request_id;
        let task_id = pending_bridge_task_id(agent_id, request_id);
        let now = now_ms();
        let mut transaction = self.repository.pool().begin().await.map_err(|_| {
            RuntimeError::new(
                "storage_error",
                "ChatGPT bridge task claim transaction could not start",
            )
        })?;

        sqlx::query(
            "INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,allow_execute,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web',?,1,'running',NULL,1,NULL,?,?) ON CONFLICT(id) DO UPDATE SET project_folder=COALESCE(excluded.project_folder,tasks.project_folder),updated_at_ms=excluded.updated_at_ms",
        )
        .bind(&task_id)
        .bind(agent_id)
        .bind(self.device.id.as_str())
        .bind(conversation_scope)
        .bind(compact_title(&claim.user_content))
        .bind(claim.project_folder.as_deref())
        .bind(claim.created_at_ms)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RuntimeError::new("storage_error", "ChatGPT bridge task claim could not be created"))?;

        let updated = sqlx::query("UPDATE chatgpt_bridge_requests SET task_id=?,updated_at_ms=? WHERE id=? AND task_id IS NULL")
            .bind(&task_id)
            .bind(now)
            .bind(request_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RuntimeError::new("storage_error", "ChatGPT bridge request claim failed"))?;

        if updated.rows_affected() == 1 {
            transaction.commit().await.map_err(|_| {
                RuntimeError::new(
                    "storage_error",
                    "ChatGPT bridge task claim could not be committed",
                )
            })?;
            return Ok(Some(task_id));
        }

        let claimed = sqlx::query_scalar::<_, String>(
            "SELECT task_id FROM chatgpt_bridge_requests WHERE id=? AND task_id IS NOT NULL LIMIT 1",
        )
        .bind(request_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| {
            RuntimeError::new(
                "storage_error",
                "ChatGPT bridge request claim lookup failed",
            )
        })?;
        transaction.rollback().await.map_err(|_| {
            RuntimeError::new(
                "storage_error",
                "losing ChatGPT bridge task claim could not be rolled back",
            )
        })?;
        Ok(claimed)
    }
}

struct PendingBridgeClaim {
    request_id: String,
    user_content: String,
    project_folder: Option<String>,
    created_at_ms: i64,
}

fn matching_unbound_rows<'a>(rows: &'a [SqliteRow], message: Option<&str>) -> Vec<&'a SqliteRow> {
    let Some(message) = message else {
        return rows.iter().collect();
    };
    let exact = rows
        .iter()
        .filter(|row| row.get::<String, _>("submitted_content") == message)
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        return exact;
    }
    rows.iter()
        .filter(|row| {
            crate::chatgpt_message::equivalent(&row.get::<String, _>("submitted_content"), message)
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_host::user_message_tests::test_host;

    #[tokio::test]
    async fn unbound_bridge_claim_keeps_project_folder_before_browser_started_event() {
        let (host, agent_id, directory) = test_host().await;
        let project = directory.path().join("bridge-project");
        std::fs::create_dir_all(&project).expect("create bridge project");
        let project_folder = project.display().to_string();
        let request_id = "bridge-request-project-race";
        let submitted = "Sử dụng plugin @worker để kiểm tra dự án";
        let now = now_ms();

        sqlx::query(
            "INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,project_folder,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms) VALUES(?,NULL,?,?,?,?,?,?,'queued',NULL,NULL,NULL,NULL,?,?,NULL)",
        )
        .bind(request_id)
        .bind("bridge-turn-project-race")
        .bind(&agent_id)
        .bind("Auto")
        .bind("Kiểm tra dự án")
        .bind(submitted)
        .bind(&project_folder)
        .bind(now)
        .bind(now)
        .execute(host.repository.pool())
        .await
        .expect("insert unbound bridge request");

        let task_id = host
            .claim_unbound_chatgpt_bridge_task(
                &agent_id,
                Some("openai:rotated-session"),
                Some(submitted),
            )
            .await
            .expect("claim bridge request")
            .expect("claimed task");
        let stored_folder =
            sqlx::query_scalar::<_, String>("SELECT project_folder FROM tasks WHERE id=? LIMIT 1")
                .bind(&task_id)
                .fetch_one(host.repository.pool())
                .await
                .expect("read claimed task project folder");

        assert_eq!(stored_folder, project_folder);
    }

    #[tokio::test]
    async fn losing_stale_claim_rolls_back_pending_task() {
        let (host, agent_id, _directory) = test_host().await;
        let request_id = "bridge-request-already-claimed";
        let winning_task_id = "task-chatgpt-browser-winner";
        let winning_folder = "D:\\DEV\\winning-project";
        let stale_folder = "D:\\DEV\\stale-project";
        let submitted = "Sử dụng plugin @worker để kiểm tra race";
        let now = now_ms();

        sqlx::query(
            "INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,allow_execute,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web',?,1,'running',NULL,1,NULL,?,?)",
        )
        .bind(winning_task_id)
        .bind(&agent_id)
        .bind(host.device.id.as_str())
        .bind("openai:browser-winner")
        .bind("Browser winner")
        .bind(winning_folder)
        .bind(now)
        .bind(now)
        .execute(host.repository.pool())
        .await
        .expect("insert winning bridge task");

        sqlx::query(
            "INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,project_folder,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms) VALUES(?,?,?,?,?,?,?,?,'queued',NULL,NULL,NULL,NULL,?,?,NULL)",
        )
        .bind(request_id)
        .bind(winning_task_id)
        .bind("bridge-turn-already-claimed")
        .bind(&agent_id)
        .bind("Auto")
        .bind("Kiểm tra race")
        .bind(submitted)
        .bind(stale_folder)
        .bind(now)
        .bind(now)
        .execute(host.repository.pool())
        .await
        .expect("insert already claimed bridge request");

        let claimed = host
            .commit_unbound_chatgpt_bridge_claim(
                &agent_id,
                Some("openai:stale-selection"),
                &PendingBridgeClaim {
                    request_id: request_id.to_owned(),
                    user_content: "Kiểm tra race".to_owned(),
                    project_folder: Some(stale_folder.to_owned()),
                    created_at_ms: now,
                },
            )
            .await
            .expect("resolve stale bridge claim");

        assert_eq!(claimed.as_deref(), Some(winning_task_id));
        let losing_task_id = pending_bridge_task_id(&agent_id, request_id);
        let losing_task_exists: i64 =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)")
                .bind(&losing_task_id)
                .fetch_one(host.repository.pool())
                .await
                .expect("check losing pending task");
        assert_eq!(
            losing_task_exists, 0,
            "losing claim must not leave a ghost task"
        );

        let stored_winning_folder: String =
            sqlx::query_scalar("SELECT project_folder FROM tasks WHERE id=?")
                .bind(winning_task_id)
                .fetch_one(host.repository.pool())
                .await
                .expect("read winning task project folder");
        assert_eq!(stored_winning_folder, winning_folder);
    }
}
