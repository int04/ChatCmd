//! Enroll a public browser turn without requiring an MCP call or a ChatCMD send.
use super::chatgpt_observation::{
    Observation, persist_observation, publish_observation, validate_identity,
};
use super::chatgpt_support::{compact_title, openai_scope, request_json, validate_message};
use super::{Problem, db_problem, now_ms};
use crate::websocket::AppState;
use axum::{Json, extract::State, http::StatusCode};
use chatcmd_storage::SqliteRepository;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

pub(super) const RECORDER_AGENT_NAME: &str = "ChatGPT Browser Recorder";
const RECORDER_AGENT_ID: &str = "chatgpt-browser-recorder";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NativeTurnInput {
    pub conversation_id: String,
    pub conversation_url: String,
    pub user_message_id: String,
    pub content: String,
}

pub(super) async fn native_turn(
    State(state): State<Arc<AppState>>,
    Json(input): Json<NativeTurnInput>,
) -> Result<Json<Value>, Problem> {
    let request_id = enroll(&state.repository, state.device.id.as_str(), &input).await?;
    // Publish the question immediately. No assistant text or MCP is needed to create a bubble.
    let events = persist_observation(
        &state.repository,
        &request_id,
        &Observation {
            conversation_id: input.conversation_id,
            conversation_url: input.conversation_url,
            user_message_id: Some(input.user_message_id),
            revision: 1,
            messages: Vec::new(),
            completed: false,
        },
    )
    .await?;
    publish_observation(&state, events);
    request_json(&state, &request_id).await
}

pub(super) async fn enroll(
    repository: &SqliteRepository,
    device_id: &str,
    input: &NativeTurnInput,
) -> Result<String, Problem> {
    validate_identity(&input.conversation_id, &input.conversation_url)?;
    validate_message(&input.content)?;
    if input.user_message_id.trim().is_empty() || input.user_message_id.len() > 240 {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid browser user identity",
            "A bounded user message identity is required.",
        ));
    }
    let id = format!(
        "chatgpt-native-{}",
        Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("{}\0{}", input.conversation_id, input.user_message_id).as_bytes()
        )
    );
    // This short transaction reads before writing. Reserve the SQLite writer first
    // so concurrent chat captures wait under busy_timeout instead of failing an upgrade.
    let mut tx = repository
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(db_problem)?;
    // A ChatCMD-dispatched turn may already have been captured before a page reload.
    // Only its exact browser user identity can reuse it; repeated prompt text is not identity.
    let prior: Option<String> = sqlx::query_scalar("SELECT r.id FROM chatgpt_bridge_requests r LEFT JOIN timeline_events e ON e.event_id='chatgpt-think-'||r.id WHERE r.conversation_id=? AND (r.id=? OR json_extract(e.payload_json,'$.browserUserId')=?) LIMIT 1")
        .bind(&input.conversation_id).bind(&id).bind(&input.user_message_id)
        .fetch_optional(&mut *tx).await.map_err(db_problem)?;
    if let Some(prior) = prior {
        return Ok(prior);
    }
    let scope = openai_scope(&input.conversation_id);
    let bound = sqlx::query("SELECT t.id,t.agent_id FROM tasks t WHERE t.id=(SELECT task_id FROM chatgpt_conversations WHERE conversation_id=?) OR t.conversation_scope_hash=? ORDER BY CASE WHEN t.id=(SELECT task_id FROM chatgpt_conversations WHERE conversation_id=?) THEN 0 ELSE 1 END,t.created_at_ms LIMIT 2")
        .bind(&input.conversation_id).bind(&scope).bind(&input.conversation_id)
        .fetch_all(&mut *tx).await.map_err(db_problem)?;
    let now = now_ms();
    let (task_id, agent_id) = if let Some(row) = bound.first() {
        (row.get::<String, _>("id"), row.get::<String, _>("agent_id"))
    } else {
        // Recorder-only identity has no published secret and no tool permissions. Recording
        // public text must never grant execution or silently pick an unrelated MCP agent.
        sqlx::query("INSERT INTO mcp_agents(id,name,secret_hash,secret_last4,enabled,created_at_ms,updated_at_ms) VALUES(?,?,randomblob(32),'none',0,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(RECORDER_AGENT_ID).bind(RECORDER_AGENT_NAME).bind(now).bind(now)
            .execute(&mut *tx).await.map_err(db_problem)?;
        let task = format!(
            "task-chat-{}",
            Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                format!("browser-conversation\0{}", input.conversation_id).as_bytes()
            )
        );
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,allow_execute,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web',0,'running',1,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(&task).bind(RECORDER_AGENT_ID).bind(device_id).bind(&scope)
            .bind(compact_title(&input.content)).bind(now).bind(now)
            .execute(&mut *tx).await.map_err(db_problem)?;
        (task, RECORDER_AGENT_ID.to_owned())
    };
    let turn_id = format!("chatgpt-turn-{id}");
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'Auto',?,?,'running',?,?,?,?)")
        .bind(&id).bind(&task_id).bind(&turn_id).bind(&agent_id).bind(input.content.trim())
        .bind(input.content.trim()).bind(&input.conversation_id).bind(&input.conversation_url).bind(now).bind(now)
        .execute(&mut *tx).await.map_err(db_problem)?;
    // A new, identified user turn supersedes only the browser request, not any local tools.
    sqlx::query("UPDATE chatgpt_bridge_requests SET status='stopped',completed_at_ms=?,updated_at_ms=? WHERE id=(SELECT active_request_id FROM chatgpt_conversations WHERE task_id=?) AND id<>? AND status IN ('queued','running','stop_requested')")
        .bind(now).bind(now).bind(&task_id).bind(&id).execute(&mut *tx).await.map_err(db_problem)?;
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,'Auto',?,?,?) ON CONFLICT(task_id) DO UPDATE SET active_request_id=excluded.active_request_id,conversation_url=excluded.conversation_url,updated_at_ms=excluded.updated_at_ms")
        .bind(&task_id).bind(&input.conversation_id).bind(&input.conversation_url).bind(&id).bind(now).bind(now)
        .execute(&mut *tx).await.map_err(db_problem)?;
    sqlx::query("UPDATE tasks SET source='chatgpt_web',status=CASE WHEN status='stopped' THEN status ELSE 'running' END,updated_at_ms=? WHERE id=?")
        .bind(now).bind(&task_id).execute(&mut *tx).await.map_err(db_problem)?;
    tx.commit().await.map_err(db_problem)?;
    Ok(id)
}

pub(super) async fn capture_capabilities() -> Json<Value> {
    Json(json!({"provider":"chatcmd","captureProtocol":2,"nativeTurns":true,"snapshots":true}))
}
