//! Recover a missing MCP finalizer only from a fenced, settled browser answer.
//! Browser recovery never impersonates agent_turn_complete or verifies the work.
use super::*;
use sha2::{Digest, Sha256};

const FINALIZATION_GRACE_MS: i64 = 12_000;

/// A fast MCP claim may beat the browser's started callback. Persist only its
/// identity, never downgrade running to pending/started or reopen a terminal run.
pub(super) async fn record_claimed_identity(
    state: &Arc<AppState>,
    subagent_id: &str,
    input: &SubagentFallbackStarted,
) -> Result<Value, Problem> {
    let mut tx = state
        .repository
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(db_problem)?;
    let row = sqlx::query("SELECT child_task_id,status,fallback_state,fallback_attempts FROM subagent_runs WHERE id=?")
        .bind(subagent_id).fetch_one(&mut *tx).await.map_err(db_problem)?;
    let status: String = row.get("status");
    let attempt: i64 = row.get("fallback_attempts");
    if status != "running"
        || row.get::<String, _>("fallback_state") != "claimed"
        || attempt != input.attempt
    {
        return Ok(
            json!({"accepted":false,"status":status,"attempt":attempt,"reason":"state_changed"}),
        );
    }
    let child: String = row.get("child_task_id");
    let (Some(id), Some(url)) = (
        clean_optional(input.conversation_id.as_deref()),
        clean_optional(input.conversation_url.as_deref()),
    ) else {
        return Ok(
            json!({"accepted":false,"status":status,"attempt":attempt,"reason":"conversation_identity_required"}),
        );
    };
    super::super::chatgpt_support::validate_conversation(id, url)?;
    super::super::chatgpt_support::guard_conversation_binding(&mut tx, &child, Some(id)).await?;
    let now = now_ms();
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,'Auto',NULL,?,?) ON CONFLICT(task_id) DO UPDATE SET conversation_id=excluded.conversation_id,conversation_url=excluded.conversation_url,updated_at_ms=excluded.updated_at_ms")
        .bind(&child).bind(id).bind(url).bind(now).bind(now).execute(&mut *tx).await.map_err(db_problem)?;
    sqlx::query("UPDATE subagent_runs SET fallback_conversation_id=?,fallback_conversation_url=? WHERE id=?")
        .bind(id).bind(url).bind(subagent_id).execute(&mut *tx).await.map_err(db_problem)?;
    tx.commit().await.map_err(db_problem)?;
    Ok(json!({"accepted":true,"status":"running","attempt":attempt,"identityOnly":true}))
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserFinalEvidence {
    protocol: u8,
    user_message_id: String,
    assistant_message_id: String,
    stable_for_ms: u64,
    generating: bool,
}

pub(super) async fn recover_claimed_final(
    state: &Arc<AppState>,
    subagent_id: &str,
    input: &SubagentFallbackResult,
) -> Result<Value, Problem> {
    let mut tx = state
        .repository
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(db_problem)?;
    let row = sqlx::query("SELECT r.*,t.status AS task_status,p.status AS parent_status FROM subagent_runs r JOIN tasks t ON t.id=r.child_task_id JOIN tasks p ON p.id=r.parent_task_id WHERE r.id=?")
        .bind(subagent_id).fetch_one(&mut *tx).await.map_err(db_problem)?;
    let status: String = row.get("status");
    let attempt: i64 = row.get("fallback_attempts");
    let rejected = |reason: &str| {
        json!({"accepted":false,"completed":false,
        "status":status,"attempt":attempt,"reason":reason,"retryScheduled":false})
    };
    if attempt != input.attempt {
        return Ok(rejected("stale_attempt"));
    }
    if status != "running" || row.get::<String, _>("fallback_state") != "claimed" {
        return Ok(rejected("already_claimed_or_finished"));
    }
    let child: String = row.get("child_task_id");
    let candidate_id = format!("subagent-browser-final:{subagent_id}:{attempt}");
    let now = now_ms();
    let Some(started) = row.get::<Option<i64>, _>("started_at_ms") else {
        return Ok(rejected("child_not_started"));
    };
    if now >= started.saturating_add(row.get::<i64, _>("max_runtime_ms")) {
        return Ok(rejected("child_deadline_elapsed"));
    }
    if !matches!(
        row.get::<String, _>("task_status").as_str(),
        "pending" | "running"
    ) || !matches!(
        row.get::<String, _>("parent_status").as_str(),
        "pending" | "running"
    ) {
        return Ok(rejected("task_not_active"));
    }
    let Some(proof) = input.completion_evidence.as_ref() else {
        return Ok(rejected("browser_final_evidence_required"));
    };
    let assistant = input.assistant_content.as_deref().unwrap_or("").trim();
    let bounded_id = |id: &str| !id.trim().is_empty() && id.len() <= 512;
    if proof.protocol != 1
        || proof.generating
        || proof.stable_for_ms < FINALIZATION_GRACE_MS as u64
        || !bounded_id(&proof.user_message_id)
        || !bounded_id(&proof.assistant_message_id)
        || assistant.is_empty()
        || assistant.chars().count() > 100_000
    {
        return Ok(rejected("browser_final_evidence_invalid"));
    }
    let (Some(conversation), Some(url)) = (
        clean_optional(input.conversation_id.as_deref()),
        clean_optional(input.conversation_url.as_deref()),
    ) else {
        return Ok(rejected("conversation_identity_required"));
    };
    let valid_url = reqwest::Url::parse(url).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("chatgpt.com")
            && url.path().ends_with(&format!("/c/{conversation}"))
    });
    let bound: Option<String> =
        sqlx::query_scalar("SELECT conversation_id FROM chatgpt_conversations WHERE task_id=?")
            .bind(&child)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_problem)?;
    if !valid_url || bound.as_deref() != Some(conversation) {
        return Ok(rejected("conversation_identity_mismatch"));
    }
    // Never attach a later answer to the first delegated turn of a reused chat.
    let turns = sqlx::query_scalar::<_, String>("SELECT turn_id FROM timeline_events WHERE task_id=? AND actor='user' AND kind='message' AND turn_id IS NOT NULL AND created_at_ms>=? AND COALESCE(json_extract(payload_json,'$.provider'),'')<>'chatgpt_web' GROUP BY turn_id ORDER BY MIN(created_at_ms),turn_id LIMIT 2")
        .bind(&child).bind(started).fetch_all(&mut *tx).await.map_err(db_problem)?;
    if turns.len() != 1 {
        return Ok(rejected("delegated_turn_ambiguous"));
    }
    let turn = &turns[0];
    let descendants: bool = sqlx::query_scalar("WITH RECURSIVE children(child_task_id,status,depth) AS (SELECT child_task_id,status,1 FROM subagent_runs WHERE parent_task_id=? UNION ALL SELECT r.child_task_id,r.status,c.depth+1 FROM subagent_runs r JOIN children c ON r.parent_task_id=c.child_task_id WHERE c.depth<32) SELECT EXISTS(SELECT 1 FROM children WHERE status IN ('pending','running'))")
        .bind(&child).fetch_one(&mut *tx).await.map_err(db_problem)?;
    if descendants || state.activities.has_active_turn(&child, turn) {
        sqlx::query("DELETE FROM timeline_events WHERE event_id=?")
            .bind(&candidate_id)
            .execute(&mut *tx)
            .await
            .map_err(db_problem)?;
        tx.commit().await.map_err(db_problem)?;
        return Ok(rejected("child_work_still_active"));
    }
    // Bind the grace window to the exact content, worker attempt, and last activity.
    // A new tool/progress event resets it even if the rendered answer is unchanged.
    let activity: Option<(String, i64)> = sqlx::query_as("SELECT event_id,created_at_ms FROM timeline_events WHERE task_id=? AND event_id<>? ORDER BY created_at_ms DESC,event_id DESC LIMIT 1")
        .bind(&child).bind(&candidate_id).fetch_optional(&mut *tx).await.map_err(db_problem)?;
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&json!({
        "content":assistant,"conversation":conversation,"turn":turn,
        "userMessageId":proof.user_message_id,"assistantMessageId":proof.assistant_message_id,
        "workerAttempt":row.get::<i64, _>("attempt"),"activity":activity
    })).map_err(|_| Problem::new(StatusCode::INTERNAL_SERVER_ERROR,"Invalid final candidate","Could not serialize final candidate."))?));
    let existing: Option<(String, i64)> = sqlx::query_as("SELECT json_extract(payload_json,'$.digest'),created_at_ms FROM timeline_events WHERE event_id=? AND task_id=?")
        .bind(&candidate_id).bind(&child).fetch_optional(&mut *tx).await.map_err(db_problem)?;
    let settled = existing.is_some_and(|(old, first_seen)| {
        old == digest && now.saturating_sub(first_seen) >= FINALIZATION_GRACE_MS
    });
    if !settled {
        let payload =
            json!({"status":"finalization_pending","provider":"chatgpt_web","digest":digest});
        sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES(?,?,?,'system','status',?,?,?) ON CONFLICT(event_id) DO UPDATE SET payload_json=excluded.payload_json,created_at_ms=CASE WHEN json_extract(timeline_events.payload_json,'$.digest')=json_extract(excluded.payload_json,'$.digest') THEN timeline_events.created_at_ms ELSE excluded.created_at_ms END")
            .bind(&candidate_id).bind(&child).bind(turn).bind(&candidate_id).bind(payload.to_string()).bind(now)
            .execute(&mut *tx).await.map_err(db_problem)?;
        tx.commit().await.map_err(db_problem)?;
        return Ok(rejected("finalization_grace"));
    }
    let event_id = format!("chatgpt-result-subagent-fallback-{subagent_id}-{attempt}");
    let payload = json!({"status":"completed","content":assistant,"provider":"chatgpt_web",
        "finalizerMissing":true,"recoveredFromBrowser":true,"completionEvidence":proof});
    // Report + terminal transition + grant revocation must succeed together.
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES(?,?,?,'assistant','status',?,?,?)")
        .bind(&event_id).bind(&child).bind(turn).bind(&event_id).bind(payload.to_string()).bind(now)
        .execute(&mut *tx).await.map_err(db_problem)?;
    sqlx::query("UPDATE subagent_runs SET status='completed',terminal_reason='mcp_finalizer_missing_browser_recovered',lease_expires_at_ms=NULL,updated_at_ms=?,completed_at_ms=? WHERE id=?")
        .bind(now).bind(now).bind(subagent_id).execute(&mut *tx).await.map_err(db_problem)?;
    sqlx::query(
        "UPDATE tasks SET status='completed',active_session_id=NULL,updated_at_ms=? WHERE id=?",
    )
    .bind(now)
    .bind(&child)
    .execute(&mut *tx)
    .await
    .map_err(db_problem)?;
    sqlx::query("UPDATE approval_grants SET state='revoked',updated_at_ms=? WHERE task_id=? AND state='active'")
        .bind(now).bind(&child).execute(&mut *tx).await.map_err(db_problem)?;
    tx.commit().await.map_err(db_problem)?;
    super::super::chatgpt_support::publish(state, &event_id, "status", &child, turn, payload);
    publish_subagent_fallback_terminal(state, &row, &child, "completed", None);
    Ok(
        json!({"accepted":true,"completed":true,"status":"completed","attempt":attempt,
        "retryScheduled":false,"completionSource":"browserFinal","mcpFinalizerReceived":false}),
    )
}
