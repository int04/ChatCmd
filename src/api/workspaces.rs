use std::{collections::HashSet, sync::Arc};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chatcmd_core::TaskId;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::websocket::AppState;

use super::{Problem, db_problem, iso_ms, now_ms, task_delete::delete_task_by_id};

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

pub(super) async fn update_workspace_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SaveWorkspaceProject>,
) -> Result<Json<Value>, Problem> {
    let name = input.name.trim();
    let path = input.path.trim();
    validate_project_input(name, path)?;

    let existing = sqlx::query("SELECT id,path,canonical_path FROM workspace_projects WHERE id=?")
        .bind(&id)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(|| {
            Problem::new(
                StatusCode::NOT_FOUND,
                "Workspace project not found",
                "Dự án không còn tồn tại.",
            )
        })?;
    let old_canonical = existing.get::<String, _>("canonical_path");
    let canonical = canonical_project_path(path);
    let duplicate = sqlx::query_scalar::<_, String>(
        "SELECT id FROM workspace_projects WHERE canonical_path=? AND id<>?",
    )
    .bind(&canonical)
    .bind(&id)
    .fetch_optional(state.repository.pool())
    .await
    .map_err(db_problem)?;
    if duplicate.is_some() {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "Workspace project already exists",
            "Thư mục này đã được dùng bởi một dự án khác.",
        ));
    }

    let now = now_ms();
    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    sqlx::query(
        "UPDATE workspace_projects SET name=?,path=?,canonical_path=?,updated_at_ms=? WHERE id=?",
    )
    .bind(name)
    .bind(path)
    .bind(&canonical)
    .bind(now)
    .bind(&id)
    .execute(&mut *transaction)
    .await
    .map_err(db_problem)?;

    if old_canonical != canonical {
        let task_rows =
            sqlx::query("SELECT id,project_folder FROM tasks WHERE project_folder IS NOT NULL")
                .fetch_all(&mut *transaction)
                .await
                .map_err(db_problem)?;
        for row in task_rows {
            let folder = row.get::<String, _>("project_folder");
            if canonical_project_path(&folder) == old_canonical {
                sqlx::query("UPDATE tasks SET project_folder=?,updated_at_ms=? WHERE id=?")
                    .bind(path)
                    .bind(now)
                    .bind(row.get::<String, _>("id"))
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_problem)?;
            }
        }
        let request_rows = sqlx::query("SELECT id,project_folder FROM chatgpt_bridge_requests WHERE project_folder IS NOT NULL")
            .fetch_all(&mut *transaction)
            .await
            .map_err(db_problem)?;
        for row in request_rows {
            let folder = row.get::<String, _>("project_folder");
            if canonical_project_path(&folder) == old_canonical {
                sqlx::query("UPDATE chatgpt_bridge_requests SET project_folder=?,updated_at_ms=? WHERE id=?")
                    .bind(path)
                    .bind(now)
                    .bind(row.get::<String, _>("id"))
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_problem)?;
            }
        }
    }
    transaction.commit().await.map_err(db_problem)?;

    let row = sqlx::query(
        "SELECT id,name,path,created_at_ms,updated_at_ms FROM workspace_projects WHERE id=?",
    )
    .bind(&id)
    .fetch_one(state.repository.pool())
    .await
    .map_err(db_problem)?;
    Ok(Json(workspace_project_value(&row)))
}

pub(super) async fn delete_workspace_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let row = sqlx::query("SELECT id,canonical_path FROM workspace_projects WHERE id=?")
        .bind(&id)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(|| {
            Problem::new(
                StatusCode::NOT_FOUND,
                "Workspace project not found",
                "Dự án không còn tồn tại.",
            )
        })?;
    let canonical = row.get::<String, _>("canonical_path");
    let child_ids = sqlx::query_scalar::<_, String>(
        "SELECT child_task_id FROM subagent_runs WHERE child_task_id IS NOT NULL",
    )
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?
    .into_iter()
    .collect::<HashSet<_>>();
    let task_rows =
        sqlx::query("SELECT id,status,project_folder FROM tasks WHERE project_folder IS NOT NULL")
            .fetch_all(state.repository.pool())
            .await
            .map_err(db_problem)?;

    let mut deleted = 0usize;
    let mut preserved = 0usize;
    for task_row in task_rows {
        let folder = task_row.get::<String, _>("project_folder");
        if canonical_project_path(&folder) != canonical {
            continue;
        }
        let task_id = task_row.get::<String, _>("id");
        let status = task_row.get::<String, _>("status");
        if child_ids.contains(&task_id) {
            continue;
        }
        if matches!(status.as_str(), "pending" | "running")
            || has_active_descendant(&state, &task_id).await?
        {
            preserved += 1;
            continue;
        }
        let parsed = TaskId::new(&task_id).map_err(|_| {
            Problem::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Invalid stored task id",
                "Không thể xóa cuộc trò chuyện của dự án.",
            )
        })?;
        delete_task_by_id(&state, &parsed).await?;
        deleted += 1;
    }

    sqlx::query("DELETE FROM workspace_projects WHERE id=?")
        .bind(&id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    Ok(Json(
        json!({ "deleted": true, "deletedConversations": deleted, "preservedConversations": preserved }),
    ))
}

async fn has_active_descendant(state: &Arc<AppState>, task_id: &str) -> Result<bool, Problem> {
    let count = sqlx::query_scalar::<_, i64>(
        "WITH RECURSIVE descendants(id) AS (\
            SELECT child_task_id FROM subagent_runs WHERE parent_task_id=? AND child_task_id IS NOT NULL \
            UNION \
            SELECT r.child_task_id FROM subagent_runs r JOIN descendants d ON r.parent_task_id=d.id WHERE r.child_task_id IS NOT NULL\
         ) SELECT COUNT(*) FROM tasks WHERE id IN (SELECT id FROM descendants) AND status IN ('pending','running')",
    )
    .bind(task_id)
    .fetch_one(state.repository.pool())
    .await
    .map_err(db_problem)?;
    Ok(count > 0)
}

fn validate_project_input(name: &str, path: &str) -> Result<(), Problem> {
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
    Ok(())
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
