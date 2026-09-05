//! Durable, replaceable snapshots of public ChatGPT page content, independent of MCP.
use super::{
    Problem,
    chatgpt_support::{not_found_chat, publish, validate_conversation},
    db_problem, now_ms,
};
use crate::{chatgpt_transcript, websocket::AppState};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chatcmd_storage::SqliteRepository;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use std::{collections::HashSet, sync::Arc};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BrowserMessage {
    pub id: String,
    pub kind: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Observation {
    pub revision: u64,
    pub conversation_id: String,
    pub conversation_url: String,
    pub messages: Vec<BrowserMessage>,
    #[serde(default)]
    pub user_message_id: Option<String>,
    #[serde(default)]
    pub completed: bool,
}

pub(super) struct ObservationEvents {
    pub task_id: String,
    pub turn_id: String,
    pub user: Option<(String, Value)>,
    pub snapshot: Option<(String, Value)>,
    pub revision: u64,
}

pub(super) async fn bridge_observation(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
    Json(input): Json<Observation>,
) -> Result<Json<Value>, Problem> {
    let events = persist_observation(&state.repository, request_id.trim(), &input).await?;
    let revision = events.revision;
    publish_observation(&state, events);
    Ok(Json(json!({"accepted":true,"revision":revision})))
}

fn validate(input: &Observation) -> Result<(), Problem> {
    validate_identity(&input.conversation_id, &input.conversation_url)?;
    let mut ids = HashSet::new();
    let invalid = input
        .user_message_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 240)
        || input.revision == 0
        || input.revision > 9_007_199_254_740_991
        || input.messages.len() > 128
        || input
            .messages
            .iter()
            .map(|message| message.content.chars().count())
            .sum::<usize>()
            > 100_000
        || input.messages.iter().any(|message| {
            message.id.is_empty()
                || message.id.len() > 240
                || !ids.insert(&message.id)
                || !matches!(message.kind.as_str(), "commentary" | "answer")
                || message.content.trim().is_empty()
        });
    if invalid {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid ChatGPT observation",
            "A positive safe revision and at most 128 unique public messages / 100,000 characters are required.",
        ));
    }
    Ok(())
}

pub(super) fn validate_identity(id: &str, url: &str) -> Result<(), Problem> {
    validate_conversation(id, url)?;
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid ChatGPT URL",
            "A conversation URL is required.",
        )
    })?;
    let path = parsed
        .path()
        .trim_end_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let segment = match path.as_slice() {
        ["", "c", value] | ["", "g", _, "c", value] => Some(*value),
        _ => None,
    };
    if segment.and_then(decode_url_segment).as_deref() != Some(id) {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "ChatGPT identity mismatch",
            "The observation URL must identify its conversation.",
        ));
    }
    Ok(())
}

fn decode_url_segment(segment: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(segment.len());
    let mut input = segment.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = char::from(input.next()?).to_digit(16)?;
            let low = char::from(input.next()?).to_digit(16)?;
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

pub(super) async fn persist_observation(
    repository: &SqliteRepository,
    request_id: &str,
    input: &Observation,
) -> Result<ObservationEvents, Problem> {
    validate(input)?;
    let mut tx = repository.pool().begin().await.map_err(db_problem)?;
    let row = sqlx::query("SELECT task_id,turn_id,user_content,submitted_content,conversation_id,created_at_ms FROM chatgpt_bridge_requests WHERE id=?")
        .bind(request_id).fetch_optional(&mut *tx).await.map_err(db_problem)?.ok_or_else(not_found_chat)?;
    let task_id = row.get::<Option<String>, _>("task_id").ok_or_else(|| {
        Problem::new(
            StatusCode::CONFLICT,
            "ChatGPT conversation not bound",
            "Bind the request before recording observations.",
        )
    })?;
    let identity = row.get::<Option<String>, _>("conversation_id");
    if identity.as_deref() != Some(input.conversation_id.as_str()) {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "ChatGPT conversation changed",
            "The observation does not belong to the bound request conversation.",
        ));
    }
    let submitted = row.get::<String, _>("submitted_content");
    let created_at = row.get::<i64, _>("created_at_ms");
    let mcp_turn =
        chatgpt_transcript::mcp_turn(&mut tx, &task_id, request_id, created_at, &submitted)
            .await
            .map_err(db_problem)?;
    let turn_id = mcp_turn.clone().unwrap_or_else(|| row.get("turn_id"));
    chatgpt_transcript::rehome_events(&mut tx, &task_id, request_id, &turn_id)
        .await
        .map_err(db_problem)?;
    let user_id = format!("chatgpt-user-{request_id}");
    let user_payload = json!({"role":"user","content":row.get::<String,_>("user_content"),
        "submittedContent":submitted,"provider":"chatgpt_web","bridgeRequestId":request_id});
    let inserted_user = if mcp_turn.is_none() {
        sqlx::query("INSERT OR IGNORE INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES(?,?,?,'user','message',?,?,?)")
            .bind(&user_id).bind(&task_id).bind(&turn_id).bind(&user_id).bind(user_payload.to_string()).bind(created_at)
            .execute(&mut *tx).await.map_err(db_problem)?.rows_affected() > 0
    } else {
        false
    };
    let event_id = format!("chatgpt-think-{request_id}");
    let payload = json!({"provider":"chatgpt_web","source":"chatgpt","bridgeRequestId":request_id,
        "browserUserId":input.user_message_id,"revision":input.revision,"messages":input.messages,"completed":input.completed,
        "requestCreatedAtMs":created_at,"observedAtMs":now_ms()});
    let changed = sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,actor,kind,idempotency_key,payload_json,created_at_ms) VALUES(?,?,?,'assistant','chatgpt_think',?,?,?) ON CONFLICT(event_id) DO UPDATE SET turn_id=excluded.turn_id,payload_json=excluded.payload_json WHERE COALESCE(json_extract(timeline_events.payload_json,'$.revision'),0)<json_extract(excluded.payload_json,'$.revision') AND (COALESCE(json_extract(timeline_events.payload_json,'$.completed'),0)=0 OR json_extract(excluded.payload_json,'$.completed')=1)")
        .bind(&event_id).bind(&task_id).bind(&turn_id).bind(&event_id).bind(payload.to_string()).bind(created_at + 1)
        .execute(&mut *tx).await.map_err(db_problem)?.rows_affected() > 0;
    let revision: i64 = sqlx::query_scalar(
        "SELECT json_extract(payload_json,'$.revision') FROM timeline_events WHERE event_id=?",
    )
    .bind(&event_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_problem)?;
    tx.commit().await.map_err(db_problem)?;
    Ok(ObservationEvents {
        task_id,
        turn_id,
        user: inserted_user.then_some((user_id, user_payload)),
        snapshot: changed.then_some((event_id, payload)),
        revision: revision as u64,
    })
}

pub(super) fn publish_observation(state: &Arc<AppState>, events: ObservationEvents) {
    if let Some((id, payload)) = events.user {
        publish(
            state,
            &id,
            "message",
            &events.task_id,
            &events.turn_id,
            payload,
        );
    }
    if let Some((id, payload)) = events.snapshot {
        publish(
            state,
            &id,
            "chatgpt_think",
            &events.task_id,
            &events.turn_id,
            payload,
        );
    }
}

/// Compatibility for a final callback from an older extension, or an MCP final that arrived first.
/// Retain all earlier commentary; adding a final answer never replaces it with the MCP answer.
pub(super) async fn retain_result(
    state: &Arc<AppState>,
    request_id: &str,
    content: &str,
    completed: bool,
) -> Result<(), Problem> {
    if content.trim().is_empty() {
        return Ok(());
    }
    let row = sqlx::query("SELECT r.task_id,r.conversation_id,r.conversation_url,e.payload_json FROM chatgpt_bridge_requests r LEFT JOIN timeline_events e ON e.event_id='chatgpt-think-'||r.id WHERE r.id=?")
        .bind(request_id).fetch_optional(state.repository.pool()).await.map_err(db_problem)?.ok_or_else(not_found_chat)?;
    let (Some(_), Some(conversation_id), Some(conversation_url)) = (
        row.get::<Option<String>, _>("task_id"),
        row.get::<Option<String>, _>("conversation_id"),
        row.get::<Option<String>, _>("conversation_url"),
    ) else {
        return Ok(());
    };
    let previous = row
        .get::<Option<String>, _>("payload_json")
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    let mut messages: Vec<BrowserMessage> = previous
        .get("messages")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    if messages
        .iter()
        .any(|message| message.content.trim() == content.trim())
        && (!completed || previous["completed"] == true)
    {
        return Ok(());
    }
    if let Some(message) = messages
        .last_mut()
        .filter(|message| message.kind == "answer")
    {
        message.content = content.to_owned();
    } else {
        messages.push(BrowserMessage {
            id: "browser-final".to_owned(),
            kind: "answer".to_owned(),
            content: content.to_owned(),
        });
    }
    // Existing bounded snapshots can be full; preserve the final answer and newest commentary.
    while messages.len() > 128
        || messages
            .iter()
            .map(|message| message.content.chars().count())
            .sum::<usize>()
            > 100_000
    {
        if messages.len() == 1 {
            messages[0].content = messages[0].content.chars().take(100_000).collect();
            break;
        }
        messages.remove(0);
    }
    let input = Observation {
        user_message_id: previous["browserUserId"].as_str().map(str::to_owned),
        revision: previous["revision"].as_u64().unwrap_or(0).saturating_add(1),
        conversation_id,
        conversation_url,
        messages,
        completed,
    };
    let events = persist_observation(&state.repository, request_id, &input).await?;
    publish_observation(state, events);
    Ok(())
}
