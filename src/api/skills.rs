use super::*;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetSkillEnabled {
    enabled: Option<bool>,
    is_enabled: Option<bool>,
}
#[derive(Deserialize)]
pub(super) struct SetSkillOptions {
    #[serde(default)]
    options: HashMap<String, Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InstallSkill {
    repository_url: String,
}

pub(super) async fn skills(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let values = state.skills.list_global().await.map_err(runtime_problem)?;
    let items = values
        .into_iter()
        .map(|skill| {
            let icon_url = skill
                .icon_path
                .as_ref()
                .map(|_| format!("/api/local/skills/{}/icon", skill.id));
            json!({
                "id": skill.id,
                "title": skill.title,
                "description": skill.description,
                "iconUrl": icon_url,
                "source": skill.source,
                "sourceUrl": skill.source_url,
                "enabled": skill.enabled,
                "canDelete": skill.can_delete,
                "options": skill.options,
            })
        })
        .collect();
    Ok(Json(Value::Array(items)))
}

pub(super) async fn skill(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let skill = state.skills.read(&id).await.map_err(runtime_problem)?;
    Ok(Json(
        json!({ "id": skill.id, "name": skill.name, "source": skill.source, "enabled": true, "shadowed": false, "content": skill.instructions }),
    ))
}

pub(super) async fn set_skill_enabled(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SetSkillEnabled>,
) -> Result<Json<Value>, Problem> {
    let enabled = input.enabled.or(input.is_enabled).ok_or_else(|| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid skill setting",
            "enabled is required",
        )
    })?;
    let skill = state
        .skills
        .set_enabled(&id, enabled)
        .await
        .map_err(runtime_problem)?
        .ok_or_else(|| {
            Problem::new(
                StatusCode::NOT_FOUND,
                "Skill not found",
                "The requested skill is unavailable.",
            )
        })?;
    Ok(Json(serde_json::to_value(skill).unwrap_or(Value::Null)))
}

pub(super) async fn set_skill_options(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SetSkillOptions>,
) -> Result<Json<Value>, Problem> {
    if input.options.len() > 50 {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Too many options",
            "A skill accepts at most 50 option values.",
        ));
    }
    let skill = state
        .skills
        .set_options(&id, input.options)
        .await
        .map_err(runtime_problem)?
        .ok_or_else(|| {
            Problem::new(
                StatusCode::NOT_FOUND,
                "Skill not found",
                "The requested skill is unavailable.",
            )
        })?;
    Ok(Json(serde_json::to_value(skill).unwrap_or(Value::Null)))
}

pub(super) async fn install_skill(
    State(state): State<Arc<AppState>>,
    Json(input): Json<InstallSkill>,
) -> Result<(StatusCode, Json<Value>), Problem> {
    if input.repository_url.len() > 2048 {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid repository",
            "repositoryUrl is too long.",
        ));
    }
    let skill = state
        .skills
        .install(input.repository_url.trim())
        .await
        .map_err(|error| {
            let status = if error.code == "skill_conflict" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            Problem::new(
                status,
                if status == StatusCode::CONFLICT {
                    "Skill already installed"
                } else {
                    "Skill installation failed"
                },
                error.message,
            )
        })?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(skill).unwrap_or(Value::Null)),
    ))
}

pub(super) async fn delete_skill(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    if state.skills.delete(&id).await.map_err(runtime_problem)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Problem::new(
            StatusCode::NOT_FOUND,
            "Skill not found",
            "The requested skill is unavailable.",
        ))
    }
}

pub(super) async fn skill_icon(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, Problem> {
    let Some((path, content_type)) = state.skills.icon(&id).await.map_err(runtime_problem)? else {
        return Err(Problem::new(
            StatusCode::NOT_FOUND,
            "Skill icon not found",
            "The requested skill does not expose an icon.",
        ));
    };
    let bytes = tokio::fs::read(path).await.map_err(|error| {
        Problem::new(
            StatusCode::NOT_FOUND,
            "Skill icon not found",
            error.to_string(),
        )
    })?;
    Ok(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}
