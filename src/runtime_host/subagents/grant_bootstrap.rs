use super::*;
use sqlx::Acquire as _;

impl RuntimeHost {
    /// Optional inherited approval is separate from turn synchronization.
    /// A denied grant creates no authority: every operation still passes authorize_execution.
    pub(super) async fn initialize_subagent_grant(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        subagent_id: &str,
        inheritance: SubagentGrantInheritance<'_>,
        request: Option<&SubagentApprovalGrantInput>,
    ) -> RuntimeResult<()> {
        let Some(request) = request else {
            return Ok(());
        };
        let child_task_id = inheritance.child_task_id;
        let child_turn_id = inheritance.child_turn_id;
        let attempt = inheritance.child_attempt;
        // Isolate grant reservations from the lifecycle claim. On any denied request,
        // roll back all grant writes before recording the diagnostic.
        let mut grant_tx = transaction
            .begin()
            .await
            .map_err(|_| RuntimeError::new("storage_error", "child grant savepoint failed"))?;
        let result = self
            .inherit_subagent_approval_grant(&mut grant_tx, inheritance, request)
            .await;
        let diagnostic = match result {
            Ok(grant_id) => {
                grant_tx.commit().await.map_err(|_| {
                    RuntimeError::new("storage_error", "child grant savepoint commit failed")
                })?;
                json!({"status":"inherited","grantId":grant_id})
            }
            Err(error) => {
                grant_tx.rollback().await.map_err(|_| {
                    RuntimeError::new("storage_error", "child grant savepoint rollback failed")
                })?;
                if !matches!(
                    error.code.as_str(),
                    "approval_grant_inheritance_denied" | "invalid_arguments"
                ) {
                    return Err(error);
                }
                json!({"status":"notInherited","errorCode":error.code,"message":error.message,
                    "instruction":"No reusable approval was inherited. Turn synchronization grants no execution permission. Use the normal per-operation approval flow; report a blocker if access is denied. Do not change execution mode or treat this diagnostic as approval."})
            }
        };
        let key = chatcmd_storage::subagent_approval::event_id(subagent_id, attempt);
        let payload = json!({"status":"subagent_approval","subagentId":subagent_id,
            "attempt":attempt,"subagentApproval":diagnostic});
        sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,session_id,actor,kind,idempotency_key,payload_json,metadata_json,created_at_ms) VALUES(?,?,?,NULL,'system','status',?,?,NULL,?) ON CONFLICT(event_id) DO NOTHING")
            .bind(&key).bind(child_task_id).bind(child_turn_id).bind(&key)
            .bind(payload.to_string()).bind(now_ms()).execute(&mut **transaction).await
            .map_err(|_| RuntimeError::new("storage_error", "child grant diagnostic could not be persisted"))?;
        Ok(())
    }
}
