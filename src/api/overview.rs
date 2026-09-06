use std::{collections::BTreeMap, sync::Arc};

use axum::{Json, extract::State};
use serde_json::{Value, json};
use sqlx::Row;

use crate::websocket::AppState;

use super::{Problem, db_problem, now_ms, settings::mcp_endpoint_template};

const SUBAGENT_METRICS_SQL: &str = "SELECT COALESCE(SUM(CASE WHEN status='running' THEN 1 ELSE 0 END),0) AS active_leases,COALESCE(SUM(CASE WHEN status='timedOut' THEN 1 ELSE 0 END),0) AS expired_total,COALESCE(MAX(CASE WHEN status='running' THEN MAX(0,?-COALESCE(last_heartbeat_at_ms,lease_acquired_at_ms,updated_at_ms)) ELSE 0 END),0) AS max_heartbeat_lag_ms,CAST(COALESCE(AVG(CASE WHEN completed_at_ms IS NOT NULL AND started_at_ms IS NOT NULL THEN MAX(0,completed_at_ms-started_at_ms) END),0) AS REAL) AS average_runtime_ms,COALESCE(SUM(CASE WHEN attempt>1 THEN attempt-1 ELSE 0 END),0)+COALESCE(SUM(CASE WHEN fallback_attempts>1 THEN fallback_attempts-1 ELSE 0 END),0) AS retry_attempts FROM subagent_runs";

pub(super) async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "ChatCmdClient" }))
}

pub(super) async fn info(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "name": "ChatCmdClient", "version": crate::version::app_version(),
        "api": "/api", "mcp": "/mcp/{token}", "websocket": "/ws",
        "connectedClients": state.connected_clients()
    }))
}

pub(super) async fn overview(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let task_counts = sqlx::query("SELECT status,COUNT(*) AS count FROM tasks GROUP BY status")
        .fetch_all(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let session_counts =
        sqlx::query("SELECT status,COUNT(*) AS count FROM terminal_sessions GROUP BY status")
            .fetch_all(state.repository.pool())
            .await
            .map_err(db_problem)?;
    let approval_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM approvals WHERE state='pending'")
            .fetch_one(state.repository.pool())
            .await
            .map_err(db_problem)?;
    let now = now_ms();
    let subagent_metrics = sqlx::query(SUBAGENT_METRICS_SQL)
        .bind(now)
        .fetch_one(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let active_leases = subagent_metrics
        .try_get::<i64, _>("active_leases")
        .map_err(db_problem)?;
    let expired_total = subagent_metrics
        .try_get::<i64, _>("expired_total")
        .map_err(db_problem)?;
    let max_heartbeat_lag_ms = subagent_metrics
        .try_get::<i64, _>("max_heartbeat_lag_ms")
        .map_err(db_problem)?;
    let average_runtime_ms = subagent_metrics
        .try_get::<f64, _>("average_runtime_ms")
        .map_err(db_problem)?;
    let retry_attempts = subagent_metrics
        .try_get::<i64, _>("retry_attempts")
        .map_err(db_problem)?;
    let tasks = counts(&task_counts);
    let terminal = counts(&session_counts);
    Ok(Json(json!({
        "app": { "version": crate::version::app_version(), "startedAtUtc": state.started_at, "state": "ready" },
        "device": { "id": state.device.id.as_str(), "machineId": state.device.machine_id, "name": state.device.name, "platform": state.device.platform, "osVersion": state.device.os_version, "architecture": state.device.architecture },
        "mcp": { "state": "listening", "endpoint": mcp_endpoint_template(&state), "connectedClients": state.connected_clients() },
        "database": { "state": "ready", "path": state.database_path, "schemaVersion": chatcmd_storage::CURRENT_SCHEMA_VERSION.to_string() },
        "terminal": { "defaultShell": default_shell(), "activeSessions": count_active(&terminal), "totalSessions": total(&terminal), "failedSessions": *terminal.get("failed").unwrap_or(&0) },
        "tasks": { "running": *tasks.get("running").unwrap_or(&0), "completed": *tasks.get("completed").unwrap_or(&0), "failed": *tasks.get("failed").unwrap_or(&0), "approvals": approval_count },
        "subagents": { "activeLeases": active_leases, "expiredTotal": expired_total, "maxHeartbeatLagMs": max_heartbeat_lag_ms, "averageRuntimeMs": average_runtime_ms, "retryAttempts": retry_attempts },
        "sessions": { "active": count_active(&terminal), "total": total(&terminal) }, "recentEvents": []
    })))
}

pub(super) async fn mcp_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(
        json!({ "state": "listening", "endpoint": mcp_endpoint_template(&state), "connectedClients": state.connected_clients() }),
    )
}

fn counts(rows: &[sqlx::sqlite::SqliteRow]) -> BTreeMap<String, i64> {
    rows.iter()
        .map(|row| (row.get("status"), row.get("count")))
        .collect()
}

fn total(values: &BTreeMap<String, i64>) -> i64 {
    values.values().sum()
}

fn count_active(values: &BTreeMap<String, i64>) -> i64 {
    values.get("running").unwrap_or(&0) + values.get("starting").unwrap_or(&0)
}

pub(super) fn default_shell() -> String {
    if cfg!(windows) {
        "powershell.exe".to_owned()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Row as _;

    use super::SUBAGENT_METRICS_SQL;

    #[tokio::test]
    async fn empty_subagent_runtime_average_decodes_as_real_zero() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let (repository, _) =
            chatcmd_storage::SqliteRepository::open(&directory.path().join("overview.sqlite3"), 1)
                .await
                .expect("open repository");

        let row = sqlx::query(SUBAGENT_METRICS_SQL)
            .bind(0_i64)
            .fetch_one(repository.pool())
            .await
            .expect("query empty metrics");

        assert_eq!(
            row.try_get::<f64, _>("average_runtime_ms")
                .expect("real average"),
            0.0
        );
    }
}
