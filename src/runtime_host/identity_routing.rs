//! Resolve server-created request IDs/turn IDs before considering message similarity.
use super::{RuntimeHost, now_ms};
use crate::{
    chatgpt_routing,
    runtime_host::chatgpt_identity::{PendingBridgeClaim, pending_bridge_task_id},
};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use sqlx::Row as _;

impl RuntimeHost {
    pub(super) async fn bridge_task_from_route(
        &self,
        context: &OperationContext,
        message: Option<&str>,
        expected_task: Option<&str>,
    ) -> RuntimeResult<Option<String>> {
        let marker = message.and_then(chatgpt_routing::request_id);
        let turn = context.turn_id.as_deref();
        if marker.is_none() && turn.is_none() {
            return Ok(None);
        }
        let rows = sqlx::query("SELECT id,task_id,turn_id,user_content,submitted_content,project_folder,status,created_at_ms,completed_at_ms FROM chatgpt_bridge_requests WHERE agent_id=? AND ((? IS NOT NULL AND id=?) OR (? IS NULL AND turn_id=?)) LIMIT 2")
            .bind(&context.agent_id).bind(marker).bind(marker).bind(marker).bind(turn)
            .fetch_all(self.repository.pool()).await.map_err(route_storage_error)?;
        let Some(row) = rows.first() else {
            return if marker.is_some() {
                Err(RuntimeError::new(
                    "conversation_identity_unbound",
                    "the request marker is not owned by this authenticated agent; no conversation was created",
                ))
            } else {
                Ok(None)
            };
        };
        if rows.len() != 1 {
            return Err(ambiguous_route());
        }
        let request_id: String = row.get("id");
        let existing: Option<String> = row.get("task_id");
        let task_id = existing
            .clone()
            .unwrap_or_else(|| pending_bridge_task_id(&context.agent_id, &request_id));
        if expected_task.is_some_and(|expected| expected != task_id) {
            return Err(RuntimeError::new(
                "conversation_identity_conflict",
                "the request route conflicts with the existing task/turn/scope binding; no new conversation was created",
            ));
        }
        self.validate_resolved_task(context, &task_id).await?;
        let obsolete: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM chatgpt_compact_obsolete_requests WHERE request_id=?)",
        )
        .bind(&request_id)
        .fetch_one(self.repository.pool())
        .await
        .map_err(route_storage_error)?;
        if obsolete {
            return Err(RuntimeError::new(
                "conversation_compacting_or_archived",
                "this request belongs to an archived conversation",
            ));
        }
        if let Some(task) = existing.as_deref() {
            let latest: Option<String> = sqlx::query_scalar("SELECT id FROM chatgpt_bridge_requests WHERE task_id=? ORDER BY created_at_ms DESC,rowid DESC LIMIT 1")
                .bind(task).fetch_optional(self.repository.pool()).await.map_err(route_storage_error)?;
            if latest.as_deref() != Some(&request_id) {
                return Err(RuntimeError::new(
                    "conversation_identity_stale",
                    "this route is for an older user message; use the current ChatUI request's turnId",
                ));
            }
        }
        let status: String = row.get("status");
        let recent_completion = status == "completed"
            && row
                .get::<Option<i64>, _>("completed_at_ms")
                .is_some_and(|time| time >= now_ms().saturating_sub(120_000));
        if !matches!(status.as_str(), "queued" | "running") && !recent_completion {
            return Err(RuntimeError::new(
                "conversation_identity_stale",
                "this ChatUI request is stopped, failed or expired; reuse a live request instead of creating a conversation",
            ));
        }
        if existing.is_some() {
            return Ok(existing);
        }
        self.commit_unbound_chatgpt_bridge_claim(
            &context.agent_id,
            context.conversation_scope_id.as_deref(),
            &PendingBridgeClaim {
                request_id,
                user_content: row.get("user_content"),
                project_folder: row.get("project_folder"),
                created_at_ms: row.get("created_at_ms"),
            },
        )
        .await
    }

    /// Similarity is only a fail-closed diagnostic, never a permission/identity grant.
    pub(super) async fn guard_rewritten_chatui_message(
        &self,
        context: &OperationContext,
        message: &str,
    ) -> RuntimeResult<()> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT r.turn_id,r.submitted_content FROM chatgpt_bridge_requests r LEFT JOIN chatgpt_conversations c ON c.task_id=r.task_id WHERE r.agent_id=? AND NOT EXISTS(SELECT 1 FROM chatgpt_compact_obsolete_requests o WHERE o.request_id=r.id) AND ((r.status IN ('queued','running','stop_requested') AND (c.active_request_id=r.id OR r.updated_at_ms>=?)) OR (r.status='completed' AND r.completed_at_ms>=?)) ORDER BY r.updated_at_ms DESC,r.id DESC LIMIT 33",
        ).bind(&context.agent_id).bind(now_ms().saturating_sub(5 * 60 * 1000)).bind(now_ms().saturating_sub(120_000))
            .fetch_all(self.repository.pool()).await.map_err(route_storage_error)?;
        if rows.len() > 32 {
            return Err(ambiguous_route());
        }
        let mut matches = rows
            .iter()
            .filter(|(_, submitted)| chatgpt_routing::possible_rewritten_echo(submitted, message));
        let Some((turn, _)) = matches.next() else {
            return Ok(());
        };
        if matches.next().is_some() {
            return Err(ambiguous_route());
        }
        tracing::warn!(agent_id = %context.agent_id, "refused to create task from a rewritten ChatUI echo");
        Err(RuntimeError::new(
            "conversation_identity_required",
            format!(
                "the message appears to be a rewritten ChatUI request, not a new conversation. No task or approval was created. Retry agent_user_message using the server-recorded turnId={turn} and the exact current user message; reuse the returned taskId. Do not invent a taskId or request another approval."
            ),
        ))
    }
}

fn ambiguous_route() -> RuntimeError {
    RuntimeError::new(
        "conversation_identity_ambiguous",
        "multiple ChatUI requests could own this call; use the current request's routing turnId, not a new conversation",
    )
}
fn route_storage_error(_: sqlx::Error) -> RuntimeError {
    RuntimeError::new("storage_error", "ChatUI request identity lookup failed")
}
