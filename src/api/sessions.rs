use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row as _;
use sysinfo::{Pid, System};

use super::{Problem, db_problem, iso_ms, not_found, now_ms, runtime_problem, timeline_row};
use crate::websocket::AppState;

pub(super) async fn sessions(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let rows = sqlx::query(
        "SELECT 'mcp' AS kind,session_id AS id,task_id,NULL AS executable,NULL AS process_id,status,NULL AS working_directory,created_at_ms,updated_at_ms,NULL AS closed_at_ms FROM task_sessions UNION ALL SELECT 'terminal' AS kind,id,task_id,executable,process_id,status,working_directory,created_at_ms,updated_at_ms,closed_at_ms FROM terminal_sessions ORDER BY updated_at_ms DESC LIMIT 500",
    )
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;
    Ok(Json(Value::Array(rows.iter().map(session_row).collect())))
}

pub(super) async fn live_terminals(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, Problem> {
    let active = state.shell.list().await.map_err(runtime_problem)?;
    let mut system = System::new_all();
    system.refresh_all();
    let mut items = Vec::with_capacity(active.len());
    for info in active {
        let row =
            sqlx::query("SELECT task_id,turn_id,updated_at_ms FROM terminal_sessions WHERE id=?")
                .bind(&info.session_id)
                .fetch_optional(state.repository.pool())
                .await
                .map_err(db_problem)?;
        let process = info
            .process_id
            .and_then(|pid| system.process(Pid::from_u32(pid)));
        items.push(json!({
            "kind": "terminal",
            "id": info.session_id,
            "taskId": row.as_ref().and_then(|value| value.get::<Option<String>,_>("task_id")),
            "turnId": row.as_ref().and_then(|value| value.get::<Option<String>,_>("turn_id")),
            "shell": info.executable,
            "processId": info.process_id,
            "status": info.status,
            "workingDirectory": info.initial_working_directory.display().to_string(),
            "createdAtUtc": iso_ms(i64::try_from(info.created_at_unix_ms).unwrap_or(i64::MAX)),
            "updatedAtUtc": row.as_ref().map(|value| iso_ms(value.get("updated_at_ms"))),
            "cpuPercent": process.map(|value| value.cpu_usage()),
            "memoryBytes": process.map(|value| value.memory()),
            "busy": state.activities.is_shell_busy(&info.session_id),
            "lastSequence": info.last_sequence
        }));
    }
    Ok(Json(Value::Array(items)))
}

#[derive(Default, Deserialize)]
pub(super) struct Cursor {
    cursor: Option<String>,
}

#[derive(Default, Deserialize)]
pub(super) struct LiveCursor {
    #[serde(rename = "afterSequence")]
    after_sequence: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct TerminalInput {
    text: String,
}

pub(super) async fn session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(cursor): Query<Cursor>,
) -> Result<Json<Value>, Problem> {
    session_detail(&state, &id, cursor.cursor.as_deref()).await
}

pub(super) async fn terminal_live_output(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(cursor): Query<LiveCursor>,
) -> Result<Json<Value>, Problem> {
    let result = state
        .shell
        .read(&id, cursor.after_sequence.unwrap_or(0), 2_000)
        .await
        .map_err(runtime_problem)?;
    Ok(Json(json!({
        "sessionId": result.session_id,
        "oldestAvailableSequence": result.oldest_available_sequence,
        "latestAvailableSequence": result.latest_available_sequence,
        "replayTruncated": result.replay_truncated,
        "events": result.events.iter().map(|event| json!({
            "sequence": event.sequence,
            "occurredAtUtc": iso_ms(i64::try_from(event.timestamp_unix_ms).unwrap_or(i64::MAX)),
            "stream": event.stream,
            "data": event.data
        })).collect::<Vec<_>>()
    })))
}

pub(super) async fn terminal_input(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<TerminalInput>,
) -> Result<Json<Value>, Problem> {
    if input.text.is_empty() {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid terminal input",
            "terminal input cannot be empty",
        ));
    }
    if state.activities.is_shell_busy(&id) {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "Terminal is busy",
            "the Agent is currently using this terminal",
        ));
    }
    let context = chatcmd_runtime::OperationContext::new(
        uuid::Uuid::new_v4().to_string(),
        "local-ui",
        "shell_write",
    );
    let written = state
        .shell
        .write(
            &context,
            chatcmd_runtime::ShellWriteRequest {
                request_id: context.request_id.clone(),
                session_id: id,
                text: input.text,
                append_new_line: false,
            },
        )
        .await
        .map_err(runtime_problem)?;
    Ok(Json(json!({ "accepted": true, "writtenBytes": written })))
}

pub(super) async fn session_action(
    State(state): State<Arc<AppState>>,
    Path((id, action)): Path<(String, String)>,
) -> Result<Json<Value>, Problem> {
    let status = match action.as_str() {
        "close" => "closed",
        "stop" | "kill" => "interrupted",
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid action",
                "unsupported session action",
            ));
        }
    };
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM terminal_sessions WHERE id=?)")
            .bind(&id)
            .fetch_one(state.repository.pool())
            .await
            .map_err(db_problem)?;
    if !exists {
        return Err(not_found());
    }

    let context = chatcmd_runtime::OperationContext::new(
        uuid::Uuid::new_v4().to_string(),
        "local-ui",
        "shell_close",
    );
    if let Err(error) = state
        .shell
        .close(&context, &id, matches!(action.as_str(), "stop" | "kill"))
        .await
        && error.code != "session_not_found"
    {
        return Err(runtime_problem(error));
    }
    let now = now_ms();
    sqlx::query("UPDATE terminal_sessions SET status=?,closed_at_ms=?,updated_at_ms=? WHERE id=?")
        .bind(status)
        .bind(now)
        .bind(now)
        .bind(&id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    session_detail(&state, &id, None).await
}

async fn session_detail(
    state: &Arc<AppState>,
    id: &str,
    cursor: Option<&str>,
) -> Result<Json<Value>, Problem> {
    if let Some(row) = sqlx::query("SELECT 'mcp' AS kind,session_id AS id,task_id,NULL AS executable,NULL AS process_id,status,NULL AS working_directory,created_at_ms,updated_at_ms,NULL AS closed_at_ms FROM task_sessions WHERE session_id=?")
        .bind(id).fetch_optional(state.repository.pool()).await.map_err(db_problem)?
    {
        let events = sqlx::query("SELECT event_id,turn_id,session_id,kind,payload_json,created_at_ms FROM timeline_events WHERE session_id=? ORDER BY created_at_ms,event_id LIMIT 1000")
            .bind(id).fetch_all(state.repository.pool()).await.map_err(db_problem)?;
        return Ok(Json(json!({
            "session": session_row(&row),
            "events": events.iter().map(timeline_row).collect::<Vec<_>>(),
            "nextCursor": null,
            "truncated": false
        })));
    }
    terminal_detail(state, id, cursor).await
}

async fn terminal_detail(
    state: &Arc<AppState>,
    id: &str,
    cursor: Option<&str>,
) -> Result<Json<Value>, Problem> {
    let row = sqlx::query("SELECT 'terminal' AS kind,id,task_id,executable,process_id,status,working_directory,created_at_ms,updated_at_ms,closed_at_ms FROM terminal_sessions WHERE id=?")
        .bind(id).fetch_optional(state.repository.pool()).await.map_err(db_problem)?.ok_or_else(not_found)?;
    let after = cursor
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(-1);
    let rows = sqlx::query("SELECT sequence,event_id,task_id,turn_id,kind,payload,created_at_ms FROM terminal_event_chunks WHERE session_id=? AND sequence>? ORDER BY sequence LIMIT 1001")
        .bind(id).bind(after).fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    let events = rows.iter().take(1000).map(|event| json!({
        "id": event.get::<String,_>("event_id"), "type": event.get::<String,_>("kind"),
        "occurredAt": iso_ms(event.get("created_at_ms")), "taskId": event.get::<Option<String>,_>("task_id"),
        "sessionId": id, "turnId": event.get::<Option<String>,_>("turn_id"),
        "payload": { "data": String::from_utf8_lossy(&event.get::<Vec<u8>,_>("payload")) }
    })).collect::<Vec<_>>();
    let next = rows
        .iter()
        .take(1000)
        .next_back()
        .map(|event| event.get::<i64, _>("sequence").to_string());
    Ok(Json(json!({
        "session": session_row(&row), "events": events, "nextCursor": next,
        "truncated": rows.len() > 1000
    })))
}

fn session_row(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "kind": row.get::<String,_>("kind"), "id": row.get::<String,_>("id"),
        "taskId": row.get::<Option<String>,_>("task_id"),
        "shell": row.get::<Option<String>,_>("executable"),
        "processId": row.get::<Option<i64>,_>("process_id"), "status": row.get::<String,_>("status"),
        "workingDirectory": row.get::<Option<String>,_>("working_directory"),
        "createdAtUtc": iso_ms(row.get("created_at_ms")), "updatedAtUtc": iso_ms(row.get("updated_at_ms")),
        "closedAtUtc": row.get::<Option<i64>,_>("closed_at_ms").map(iso_ms)
    })
}
