use std::{path::PathBuf, sync::Arc};

use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Value, json};
use sqlx::Row;

use crate::websocket::AppState;

use super::{Problem, db_problem};

pub(super) async fn database_diagnostics(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, Problem> {
    let tables = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;

    let mut table_values = Vec::with_capacity(tables.len());
    let mut total_rows = 0_i64;
    for row in tables {
        let name: String = row.try_get("name").map_err(db_problem)?;
        let quoted = name.replace('"', "\"\"");
        let count_sql = format!("SELECT COUNT(*) FROM \"{quoted}\"");
        let row_count: i64 = sqlx::query_scalar(&count_sql)
            .fetch_one(state.repository.pool())
            .await
            .map_err(db_problem)?;
        total_rows = total_rows.saturating_add(row_count);
        table_values.push(json!({ "name": name, "rowCount": row_count }));
    }

    let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let free_page_count: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let file_size = tokio::fs::metadata(&state.database_path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or_else(|_| page_count.max(0) as u64 * page_size.max(0) as u64);
    let used_pages = page_count.saturating_sub(free_page_count).max(0);

    Ok(Json(json!({
        "path": state.database_path,
        "tableCount": table_values.len(),
        "totalRows": total_rows,
        "fileSizeBytes": file_size,
        "pageCount": page_count,
        "pageSizeBytes": page_size,
        "freePageCount": free_page_count,
        "usedSizeBytes": used_pages.saturating_mul(page_size),
        "tables": table_values,
    })))
}

pub(super) async fn diagnostic_logs() -> Result<Json<Value>, Problem> {
    let path = diagnostic_log_path();
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => {
            return Err(Problem::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Log read error",
                "could not read the local diagnostic log",
            ));
        }
    };
    let lines = content
        .lines()
        .rev()
        .take(500)
        .map(str::to_owned)
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "path": path.to_string_lossy(),
        "lineCount": content.lines().count(),
        "lines": lines,
    })))
}

fn diagnostic_log_path() -> PathBuf {
    std::env::var_os("CHATCMD_LOG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("logs").join("chatcmd.log"))
}
