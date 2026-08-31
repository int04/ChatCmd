use std::{collections::HashSet, sync::Arc};

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::websocket::AppState;

use super::{Problem, db_problem, iso_ms, now_ms};

const MAX_PROJECT_NAME_CHARS: usize = 160;
const MAX_PROJECT_PATH_CHARS: usize = 4_096;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SaveWorkspaceProject {
    name: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReorderWorkspaceProjects {
    project_ids: Vec<String>,
}

pub(super) async fn workspace_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, Problem> {
    let rows = sqlx::query(
        "SELECT id,name,path,sort_order,created_at_ms,updated_at_ms FROM workspace_projects ORDER BY COALESCE(sort_order, 2147483647), updated_at_ms DESC, name COLLATE NOCASE",
    )
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;
    Ok(Json(Value::Array(
        rows.iter().map(workspace_project_value).collect(),
    )))
}

pub(super) async fn save_workspace_project(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SaveWorkspaceProject>,
) -> Result<Json<Value>, Problem> {
    let name = input.name.trim();
    let path = input.path.trim();
    if name.is_empty() || path.is_empty() {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid workspace project",
            "name and path are required",
        ));
    }
    if name.chars().count() > MAX_PROJECT_NAME_CHARS
        || path.chars().count() > MAX_PROJECT_PATH_CHARS
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid workspace project",
            "name or path is too long",
        ));
    }
    let canonical = canonical_project_path(path);
    let id = format!("project-{}", Uuid::new_v4());
    let now = now_ms();
    sqlx::query(
        "INSERT INTO workspace_projects(id,name,path,canonical_path,sort_order,created_at_ms,updated_at_ms) VALUES(?,?,?,?,(SELECT COALESCE(MAX(sort_order),-1)+1 FROM workspace_projects),?,?) ON CONFLICT(canonical_path) DO UPDATE SET name=excluded.name,path=excluded.path,updated_at_ms=excluded.updated_at_ms",
    )
    .bind(&id)
    .bind(name)
    .bind(path)
    .bind(&canonical)
    .bind(now)
    .bind(now)
    .execute(state.repository.pool())
    .await
    .map_err(db_problem)?;
    let row = sqlx::query(
        "SELECT id,name,path,created_at_ms,updated_at_ms FROM workspace_projects WHERE canonical_path=?",
    )
    .bind(&canonical)
    .fetch_one(state.repository.pool())
    .await
    .map_err(db_problem)?;
    Ok(Json(workspace_project_value(&row)))
}

pub(super) async fn reorder_workspace_projects(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ReorderWorkspaceProjects>,
) -> Result<StatusCode, Problem> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_projects")
        .fetch_one(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let unique_ids = input.project_ids.iter().collect::<HashSet<_>>();
    if input.project_ids.len() as i64 != count || unique_ids.len() != input.project_ids.len() {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid workspace project order",
            "projectIds must contain every workspace project exactly once",
        ));
    }
    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    for (position, project_id) in input.project_ids.iter().enumerate() {
        let result = sqlx::query("UPDATE workspace_projects SET sort_order=? WHERE id=?")
            .bind(position as i64)
            .bind(project_id)
            .execute(&mut *transaction)
            .await
            .map_err(db_problem)?;
        if result.rows_affected() != 1 {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid workspace project order",
                "projectIds contains an unknown or duplicate project",
            ));
        }
    }
    transaction.commit().await.map_err(db_problem)?;
    Ok(StatusCode::NO_CONTENT)
}

fn canonical_project_path(path: &str) -> String {
    let mut value = path.trim().replace('\\', "/");
    while value.len() > 1 && value.ends_with('/') {
        value.pop();
    }
    let bytes = value.as_bytes();
    let is_windows_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if is_windows_drive || value.starts_with("//") {
        value.make_ascii_lowercase();
    }
    value
}

fn workspace_project_value(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<String, _>("id"),
        "name": row.get::<String, _>("name"),
        "path": row.get::<String, _>("path"),
        "createdAtUtc": iso_ms(row.get::<i64, _>("created_at_ms")),
        "updatedAtUtc": iso_ms(row.get::<i64, _>("updated_at_ms"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_windows_workspace_folder_normalizes_identity() {
        assert_eq!(
            canonical_project_path(r" D:\DEV\Dotty\ "),
            canonical_project_path("d:/dev/dotty")
        );
    }

    #[test]
    fn canonical_posix_workspace_folder_preserves_case() {
        assert_eq!(canonical_project_path(" /Work/Client/ "), "/Work/Client");
        assert_ne!(
            canonical_project_path("/Work/Client"),
            canonical_project_path("/work/client")
        );
    }
}
