//! Request-scoped browser/MCP correlation. Browser observations never grant tool authority.
use chatcmd_storage::SqliteRepository;
use sqlx::{Row, Sqlite, Transaction};

pub(crate) struct BridgeTurn {
    pub request_id: String,
    pub browser_turn_id: String,
}

/// Only the latest request in this task can claim a newly arriving MCP user turn.
/// An already claimed request cannot absorb a later identical question.
pub(crate) async fn request_for_turn(
    repository: &SqliteRepository,
    task_id: &str,
    turn_id: &str,
    content: &str,
) -> Result<Option<BridgeTurn>, sqlx::Error> {
    let row = sqlx::query("SELECT id,turn_id,submitted_content FROM chatgpt_bridge_requests WHERE task_id=? ORDER BY created_at_ms DESC,rowid DESC LIMIT 1")
        .bind(task_id).fetch_optional(repository.pool()).await?;
    let Some(row) = row else { return Ok(None) };
    if !crate::chatgpt_message::equivalent(&row.get::<String, _>("submitted_content"), content) {
        return Ok(None);
    }
    let request_id = row.get::<String, _>("id");
    let claimed: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM timeline_events WHERE task_id=? AND actor='user' AND kind='message' AND COALESCE(json_extract(payload_json,'$.provider'),'')<>'chatgpt_web' AND json_extract(payload_json,'$.bridgeRequestId')=? AND turn_id<>?)")
        .bind(task_id).bind(&request_id).bind(turn_id).fetch_one(repository.pool()).await?;
    Ok((claimed == 0).then(|| BridgeTurn {
        request_id,
        browser_turn_id: row.get("turn_id"),
    }))
}

/// Prefer the explicit server-owned link; old clients may only have an exact prompt echo.
/// The next request is a hard upper bound, so repeated questions never cross that boundary.
pub(crate) async fn mcp_turn(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    request_id: &str,
    created_at_ms: i64,
    submitted: &str,
) -> Result<Option<String>, sqlx::Error> {
    let linked: Option<String> = sqlx::query_scalar("SELECT turn_id FROM timeline_events WHERE task_id=? AND actor='user' AND kind='message' AND COALESCE(json_extract(payload_json,'$.provider'),'')<>'chatgpt_web' AND json_extract(payload_json,'$.bridgeRequestId')=? AND turn_id IS NOT NULL LIMIT 1")
        .bind(task_id).bind(request_id).fetch_optional(&mut **tx).await?;
    if linked.is_some() {
        return Ok(linked);
    }
    let rows = sqlx::query("SELECT turn_id,json_extract(payload_json,'$.content') AS content FROM timeline_events WHERE task_id=? AND actor='user' AND kind='message' AND turn_id IS NOT NULL AND COALESCE(json_extract(payload_json,'$.provider'),'')<>'chatgpt_web' AND json_extract(payload_json,'$.bridgeRequestId') IS NULL AND created_at_ms>=? AND created_at_ms<COALESCE((SELECT MIN(created_at_ms) FROM chatgpt_bridge_requests WHERE task_id=? AND id<>? AND created_at_ms>=?),9223372036854775807) ORDER BY created_at_ms LIMIT 64")
        .bind(task_id).bind(created_at_ms).bind(task_id).bind(request_id).bind(created_at_ms)
        .fetch_all(&mut **tx).await?;
    let mut matches = rows.iter().filter(|row| {
        row.get::<Option<String>, _>("content")
            .is_some_and(|text| crate::chatgpt_message::equivalent(&text, submitted))
    });
    let first = matches.next().map(|row| row.get::<String, _>("turn_id"));
    Ok(if matches.next().is_some() {
        None
    } else {
        first
    })
}

pub(crate) async fn rehome_events(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    request_id: &str,
    turn_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE timeline_events SET turn_id=? WHERE task_id=? AND event_id IN (?,?,?) AND json_extract(payload_json,'$.provider')='chatgpt_web'")
        .bind(turn_id).bind(task_id)
        .bind(format!("chatgpt-user-{request_id}"))
        .bind(format!("chatgpt-think-{request_id}"))
        .bind(format!("chatgpt-result-{request_id}"))
        .execute(&mut **tx).await?;
    Ok(())
}
