//! Bounded public child reports read from the existing durable timeline.
//! Callers must authorize the run against their own parent task AND turn first.
use serde_json::{Value, json};
use sqlx::{Row as _, SqlitePool};

pub const REPORT_PAGE_CHARS: i64 = 12_000;
const QUALITY_JSON_CHARS: i64 = 64_000;

/// Read the final answer for the delegated turn, never progress or tool output.
/// Offsets count Unicode scalar values, matching SQLite's text substr operation.
/// The first owned user turn fences off later messages in a reused child chat.
pub async fn report_page(
    pool: &SqlitePool,
    subagent_id: &str,
    offset: i64,
    limit: i64,
) -> Result<Option<Value>, sqlx::Error> {
    let row = sqlx::query(
        "WITH owned_run AS (SELECT * FROM subagent_runs WHERE id=?),
        owned_turn AS (
            SELECT u.turn_id FROM timeline_events u JOIN owned_run r ON u.task_id=r.child_task_id
            WHERE u.actor='user' AND u.kind='message' AND u.turn_id IS NOT NULL
              AND u.created_at_ms>=COALESCE(r.started_at_ms,r.created_at_ms)
              AND (r.completed_at_ms IS NULL OR u.created_at_ms<=r.completed_at_ms
                   OR u.turn_id='turn-'||r.id)
            ORDER BY u.created_at_ms,u.event_id LIMIT 1
        ), final AS (
            SELECT e.* FROM timeline_events e JOIN owned_run r ON e.task_id=r.child_task_id
            WHERE r.status='completed' AND e.actor='assistant' AND e.kind='status'
              AND e.turn_id=(SELECT turn_id FROM owned_turn)
              AND e.created_at_ms>=COALESCE(r.started_at_ms,r.created_at_ms)
              AND json_extract(e.payload_json,'$.status')='completed'
              AND json_type(e.payload_json,'$.content')='text'
              AND length(trim(json_extract(e.payload_json,'$.content')))>0
              AND (json_extract(e.payload_json,'$.tool')='agent_turn_complete'
                   OR json_extract(e.payload_json,'$.provider')='chatgpt_web')
            ORDER BY CASE WHEN json_extract(e.payload_json,'$.tool')='agent_turn_complete' THEN 0 ELSE 1 END,
                     e.created_at_ms,e.event_id LIMIT 1
        )
        SELECT f.event_id,f.turn_id,f.created_at_ms,
               CASE WHEN json_extract(f.payload_json,'$.tool')='agent_turn_complete'
                    THEN 'mcpFinal' ELSE 'browserFinal' END AS source,
               length(json_extract(f.payload_json,'$.content')) AS total_chars,
               substr(json_extract(f.payload_json,'$.content'),?,?) AS content,
               (SELECT substr(json_extract(q.payload_json,'$.qualityReport'),1,?)
                FROM timeline_events q JOIN owned_run r ON q.task_id=r.child_task_id
                WHERE q.turn_id=f.turn_id AND q.actor='assistant' AND q.kind='status'
                  AND json_extract(q.payload_json,'$.status')='quality'
                  AND json_type(q.payload_json,'$.qualityReport')='object'
                  AND q.created_at_ms>=COALESCE(r.started_at_ms,r.created_at_ms)
                  AND (json_extract(q.payload_json,'$.finalEventId')=f.event_id
                       OR json_type(q.payload_json,'$.finalEventId') IS NULL)
                ORDER BY CASE WHEN json_type(q.payload_json,'$.finalEventId') IS NOT NULL THEN 0 ELSE 1 END,
                         q.created_at_ms,q.event_id LIMIT 1) AS quality_json
        FROM final f",
    )
    .bind(subagent_id)
    .bind(offset.saturating_add(1))
    .bind(limit.clamp(0, REPORT_PAGE_CHARS))
    .bind(QUALITY_JSON_CHARS + 1)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let total: i64 = row.get("total_chars");
    let content: String = row.get("content");
    let returned = i64::try_from(content.chars().count()).unwrap_or(i64::MAX);
    let next = offset.saturating_add(returned);
    let quality_json: Option<String> = row.get("quality_json");
    let quality = quality_json.as_deref().and_then(|raw| {
        (raw.chars().count() <= QUALITY_JSON_CHARS as usize)
            .then(|| serde_json::from_str::<Value>(raw).ok())
            .flatten()
    });
    let quality = quality.as_ref();
    let outcome = enum_field(quality, "workOutcome", &["completed", "partial", "blocked"]);
    let verification = enum_field(
        quality,
        "verification",
        &[
            "passed",
            "failed",
            "notRun",
            "notApplicable",
            "stale",
            "unknown",
        ],
    );
    let evidence = quality
        .and_then(|q| q.get("evidence"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.get("executionId").and_then(Value::as_str))
        .take(64)
        .map(|s| s.chars().take(200).collect::<String>())
        .collect::<Vec<_>>();
    Ok(Some(json!({
        "availability": "available",
        "content": content,
        "eventId": row.get::<String, _>("event_id"),
        "turnId": row.get::<String, _>("turn_id"),
        "source": row.get::<String, _>("source"),
        "createdAtMs": row.get::<i64, _>("created_at_ms"),
        "totalChars": total, "offset": offset,
        "truncated": next < total,
        "nextOffset": (next < total).then_some(next),
        "workOutcome": outcome.unwrap_or("unknown"),
        "workOutcomeProvenance": enum_field(quality, "workOutcomeProvenance", &["agentDeclared", "legacyDefault"]).unwrap_or("unavailable"),
        "verification": verification.unwrap_or("unknown"),
        "verificationIsChildSnapshot": true,
        "evidenceRefs": evidence,
        "blockers": text_list(quality, "blockers"),
        "limitations": text_list(quality, "limitations"),
        "metadataUnavailable": quality.is_none(),
        "metadataTruncated": quality_json.as_ref().is_some_and(|s| s.chars().count() > QUALITY_JSON_CHARS as usize)
    })))
}

fn enum_field<'a>(value: Option<&'a Value>, key: &str, allowed: &[&str]) -> Option<&'a str> {
    value?.get(key)?.as_str().filter(|s| allowed.contains(s))
}

fn text_list(value: Option<&Value>, key: &str) -> Vec<String> {
    value
        .and_then(|v| v.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .take(32)
        .map(|s| s.chars().take(1_000).collect())
        .collect()
}
