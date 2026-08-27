use super::*;

pub(super) async fn settings(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    Ok(Json(settings_value(&state).await?))
}
pub(super) async fn save_settings(
    State(state): State<Arc<AppState>>,
    Json(value): Json<Value>,
) -> Result<Json<Value>, Problem> {
    let object = value.as_object().ok_or_else(|| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid settings",
            "JSON object required",
        )
    })?;
    let port = object.get("port").and_then(Value::as_u64).ok_or_else(|| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid settings",
            "port is required",
        )
    })?;
    if !(1..=65_535).contains(&port) {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid settings",
            "port must be 1..65535",
        ));
    }
    for (key, value) in object {
        state
            .repository
            .set_setting(&Setting {
                key: format!("ui_{key}"),
                value_json: value.to_string(),
                updated_at_ms: now_ms(),
            })
            .await
            .map_err(storage_problem)?;
    }
    Ok(Json(settings_value(&state).await?))
}

pub(super) fn mcp_endpoint_template(state: &AppState) -> String {
    format!("http://{}:{}/mcp/{{token}}", state.bind_address, state.port)
}

pub(super) fn mcp_endpoint(state: &AppState, token: &str) -> String {
    format!("http://{}:{}/mcp/{token}", state.bind_address, state.port)
}

pub(super) async fn settings_value(state: &Arc<AppState>) -> Result<Value, Problem> {
    let defaults = json!({ "bindAddress": state.bind_address, "port": state.port, "mcpEndpoint": mcp_endpoint_template(state), "databasePath": state.database_path, "databaseState": "ready", "executionMode": "approval", "workspaceRoots": [std::env::current_dir().unwrap_or_default()], "terminalExecutable": default_shell(), "taskConcurrency": 4, "sessionConcurrency": 8, "theme": "system", "language": "en", "sound": false });
    let mut object = defaults.as_object().cloned().unwrap_or_default();
    for key in [
        "executionMode",
        "workspaceRoots",
        "terminalExecutable",
        "taskConcurrency",
        "sessionConcurrency",
        "theme",
        "language",
        "sound",
    ] {
        if let Some(setting) = state
            .repository
            .setting(&format!("ui_{key}"))
            .await
            .map_err(storage_problem)?
            && let Ok(value) = serde_json::from_str(&setting.value_json)
        {
            object.insert(key.to_owned(), value);
        }
    }
    Ok(Value::Object(object))
}
