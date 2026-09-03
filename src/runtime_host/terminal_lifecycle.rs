use chatcmd_core::{TaskId, TurnId};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::json;

use super::{RuntimeHost, now_ms};

impl RuntimeHost {
    pub(super) async fn retire_previous_turn_terminals(
        &self,
        context: &OperationContext,
        task_id: &TaskId,
        turn_id: &TurnId,
    ) -> RuntimeResult<()> {
        let session_ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM terminal_sessions WHERE task_id=? AND COALESCE(turn_id,'')<>? AND status IN ('starting','running') ORDER BY created_at_ms,id",
        )
        .bind(task_id.as_str())
        .bind(turn_id.as_str())
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "previous terminal sessions unavailable"))?;
        self.retire_terminal_ids(context, session_ids).await
    }

    pub(super) async fn retire_idle_current_turn_terminals(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<()> {
        let Some(task_id) = context
            .task_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(());
        };
        let Some(turn_id) = context
            .turn_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(());
        };
        let session_ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM terminal_sessions WHERE task_id=? AND turn_id=? AND status IN ('starting','running') ORDER BY created_at_ms,id",
        )
        .bind(task_id)
        .bind(turn_id)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "current terminal sessions unavailable"))?;
        let idle = session_ids
            .into_iter()
            .filter(|session_id| !self.activities.is_shell_busy(session_id))
            .collect();
        self.retire_terminal_ids(context, idle).await
    }

    pub(super) fn publish_terminal_opened(
        &self,
        context: &OperationContext,
        info: &chatcmd_runtime::ShellSessionInfo,
    ) {
        self.publish_event(
            format!("terminal-opened:{}", info.session_id),
            "terminal.opened",
            context.task_id.clone(),
            Some(info.session_id.clone()),
            context.turn_id.clone(),
            json!({
                "id": info.session_id,
                "kind": "terminal",
                "taskId": context.task_id,
                "turnId": context.turn_id,
                "shell": info.executable,
                "processId": info.process_id,
                "status": info.status,
                "workingDirectory": info.initial_working_directory.display().to_string(),
                "createdAtUtc": crate::api::iso_now(),
                "busy": false,
                "lastSequence": info.last_sequence
            }),
        );
    }

    pub(super) fn spawn_terminal_live_bridge(
        &self,
        context: &OperationContext,
        info: &chatcmd_runtime::ShellSessionInfo,
    ) {
        let host = self.clone();
        let session_id = info.session_id.clone();
        let task_id = context.task_id.clone();
        let turn_id = context.turn_id.clone();
        tokio::spawn(async move {
            let mut cursor = 0_u64;
            loop {
                let result = match host
                    .shell
                    .read_when_available(
                        &session_id,
                        cursor,
                        2_000,
                        std::time::Duration::from_secs(25),
                    )
                    .await
                {
                    Ok(result) => result,
                    Err(_) => break,
                };
                for event in &result.events {
                    if event.sequence <= cursor {
                        continue;
                    }
                    host.publish_event(
                        format!("terminal-live:{}:{}", session_id, event.sequence),
                        "terminal.live_output",
                        task_id.clone(),
                        Some(session_id.clone()),
                        turn_id.clone(),
                        json!({
                            "sessionId": session_id,
                            "sequence": event.sequence,
                            "stream": event.stream,
                            "data": event.data
                        }),
                    );
                    cursor = event.sequence;
                }
                if result.events.is_empty() {
                    match host.shell.inspect(&session_id).await {
                        Ok(value) if value.status == "exited" => {
                            host.publish_terminal_closed(
                                task_id.clone(),
                                turn_id.clone(),
                                &session_id,
                                value.exit_code,
                            );
                            let _ = host
                                .update_session_status(&session_id, "exited", value.exit_code)
                                .await;
                            break;
                        }
                        Err(_) => break,
                        _ => {}
                    }
                }
            }
        });
    }

    fn publish_terminal_closed(
        &self,
        task_id: Option<String>,
        turn_id: Option<String>,
        session_id: &str,
        exit_code: Option<i32>,
    ) {
        self.publish_event(
            format!("terminal-closed:{}:{}", session_id, now_ms()),
            "terminal.closed",
            task_id,
            Some(session_id.to_owned()),
            turn_id,
            json!({ "sessionId": session_id, "exitCode": exit_code }),
        );
    }

    async fn retire_terminal_ids(
        &self,
        context: &OperationContext,
        session_ids: Vec<String>,
    ) -> RuntimeResult<()> {
        for session_id in session_ids {
            let _ = self.shell.close(context, &session_id, false).await;
            let now = now_ms();
            sqlx::query("UPDATE terminal_sessions SET status='closed',updated_at_ms=?,closed_at_ms=COALESCE(closed_at_ms,?) WHERE id=? AND status IN ('starting','running')")
                .bind(now).bind(now).bind(&session_id)
                .execute(self.repository.pool()).await
                .map_err(|_| RuntimeError::new("storage_error", "terminal session cleanup failed"))?;
            self.publish_terminal_closed(
                context.task_id.clone(),
                context.turn_id.clone(),
                &session_id,
                None,
            );
        }
        Ok(())
    }
}
