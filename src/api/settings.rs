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
    if let Some(retention) = object.get("dataRetention").and_then(Value::as_str)
        && !matches!(
            retention,
            "1h" | "5h" | "10h" | "1d" | "3d" | "5d" | "10d" | "off"
        )
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid data retention",
            "dataRetention must be one of: 1h, 5h, 10h, 1d, 3d, 5d, 10d, off.",
        ));
    }
    if let Some(mode) = object.get("executionMode").and_then(Value::as_str) {
        let persisted = match mode {
            "approval" => "approval",
            "allowAll" | "allow" | "safe" | "unrestricted" => "allow",
            _ => {
                return Err(Problem::new(
                    StatusCode::BAD_REQUEST,
                    "Invalid execution mode",
                    "executionMode must be 'approval' or 'allowAll'.",
                ));
            }
        };
        state
            .repository
            .set_setting(&Setting {
                key: "command_execution_mode".to_owned(),
                value_json: serde_json::to_string(persisted)
                    .unwrap_or_else(|_| "\"allow\"".to_owned()),
                updated_at_ms: now_ms(),
            })
            .await
            .map_err(storage_problem)?;
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
    let defaults = json!({ "bindAddress": state.bind_address, "port": state.port, "mcpEndpoint": mcp_endpoint_template(state), "databasePath": state.database_path, "databaseState": "ready", "executionMode": "allowAll", "approveNewConversations": true, "terminalExecutable": default_shell(), "taskConcurrency": 4, "sessionConcurrency": 8, "theme": "dark", "fontFamily": "Inter", "language": "en", "sound": true, "newAgentSound": true, "finishedTaskSound": true, "dataRetention": "1d" });
    let mut object = defaults.as_object().cloned().unwrap_or_default();
    for key in [
        "executionMode",
        "approveNewConversations",
        "terminalExecutable",
        "taskConcurrency",
        "sessionConcurrency",
        "theme",
        "fontFamily",
        "language",
        "sound",
        "newAgentSound",
        "finishedTaskSound",
        "dataRetention",
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
    let legacy_sound = object.get("sound").cloned().unwrap_or(Value::Bool(true));
    for key in ["newAgentSound", "finishedTaskSound"] {
        if state
            .repository
            .setting(&format!("ui_{key}"))
            .await
            .map_err(storage_problem)?
            .is_none()
        {
            object.insert(key.to_owned(), legacy_sound.clone());
        }
    }
    let execution_mode = state
        .repository
        .execution_mode(None)
        .await
        .map_err(storage_problem)?;
    object.insert(
        "executionMode".to_owned(),
        Value::String(
            match execution_mode {
                chatcmd_core::ExecutionMode::Allow => "allowAll",
                chatcmd_core::ExecutionMode::Approval | chatcmd_core::ExecutionMode::Deny => {
                    "approval"
                }
            }
            .to_owned(),
        ),
    );
    Ok(Value::Object(object))
}
