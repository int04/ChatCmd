use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AgentInput {
    name: String,
    enabled: bool,
    #[serde(default)]
    preset_id: Option<String>,
    tool_ids: Vec<String>,
}

pub(super) async fn list_agents(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, Problem> {
    let agents = state
        .repository
        .list_agents()
        .await
        .map_err(storage_problem)?;
    Ok(Json(Value::Array(agent_values(&state, agents).await?)))
}

pub(super) async fn get_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let id = agent_id(id)?;
    let agent = state
        .repository
        .agent(&id)
        .await
        .map_err(storage_problem)?
        .ok_or_else(not_found)?;
    Ok(Json(agent_value(&state, agent).await?))
}

pub(super) async fn create_agent(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AgentInput>,
) -> Result<(StatusCode, Json<Value>), Problem> {
    validate_agent_input(&input)?;
    let tool_ids = resolve_tools(&state, &input).await?;
    let result = state
        .repository
        .create_agent(NewMcpAgent {
            id: None,
            name: input.name,
            enabled: input.enabled,
        })
        .await
        .map_err(storage_problem)?;
    state
        .repository
        .set_agent_allowed_tools(&result.agent.id, &tool_ids)
        .await
        .map_err(storage_problem)?;
    let agent = agent_value(&state, result.agent).await?;
    let secret = result.secret.expose_once();
    let endpoint = mcp_endpoint(&state, &secret);
    state.publish(AppEvent::new(
        "agent.created",
        json!({ "agentId": agent["id"] }),
    ));
    Ok((
        StatusCode::CREATED,
        Json(json!({ "agent": agent, "endpoint": endpoint })),
    ))
}

pub(super) async fn update_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<AgentInput>,
) -> Result<Json<Value>, Problem> {
    validate_agent_input(&input)?;
    let id = agent_id(id)?;
    let tools = resolve_tools(&state, &input).await?;
    let agent = state
        .repository
        .update_agent(
            &id,
            NewMcpAgent {
                id: None,
                name: input.name,
                enabled: input.enabled,
            },
        )
        .await
        .map_err(storage_problem)?;
    state
        .repository
        .set_agent_allowed_tools(&id, &tools)
        .await
        .map_err(storage_problem)?;
    Ok(Json(agent_value(&state, agent).await?))
}

pub(super) async fn delete_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    state
        .repository
        .delete_agent(&agent_id(id)?)
        .await
        .map_err(storage_problem)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn rotate_secret(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let result = state
        .repository
        .rotate_agent_secret(&agent_id(id)?)
        .await
        .map_err(storage_problem)?;
    let agent = agent_value(&state, result.agent).await?;
    let secret = result.secret.expose_once();
    let endpoint = mcp_endpoint(&state, &secret);
    Ok(Json(json!({
        "agent": agent,
        "endpoint": endpoint
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EnabledInput {
    enabled: bool,
}
pub(super) async fn set_enabled(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<EnabledInput>,
) -> Result<Json<Value>, Problem> {
    let id = agent_id(id)?;
    state
        .repository
        .set_agent_enabled(&id, input.enabled)
        .await
        .map_err(storage_problem)?;
    let agent = state
        .repository
        .agent(&id)
        .await
        .map_err(storage_problem)?
        .ok_or_else(not_found)?;
    Ok(Json(agent_value(&state, agent).await?))
}

pub(super) async fn tools(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let tools = state
        .repository
        .list_tools()
        .await
        .map_err(storage_problem)?;
    Ok(Json(Value::Array(tools.into_iter().map(|tool| json!({
        "id": tool.id, "name": tool.title, "description": tool.description, "group": tool.group_id,
        "dangerous": tool.capabilities.contains(&ToolCapability::Destructive)
    })).collect())))
}

pub(super) async fn presets(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let presets = state
        .repository
        .list_presets()
        .await
        .map_err(storage_problem)?;
    Ok(Json(Value::Array(presets.into_iter().map(|preset| json!({ "id": preset.id, "name": preset.name, "description": preset.description, "toolIds": preset.tool_ids })).collect())))
}

async fn resolve_tools(state: &Arc<AppState>, input: &AgentInput) -> Result<Vec<String>, Problem> {
    if let Some(id) = &input.preset_id {
        let preset = state
            .repository
            .list_presets()
            .await
            .map_err(storage_problem)?
            .into_iter()
            .find(|item| &item.id == id)
            .ok_or_else(|| {
                Problem::new(
                    StatusCode::BAD_REQUEST,
                    "Invalid preset",
                    "presetId does not exist",
                )
            })?;
        if input.tool_ids.is_empty() {
            return Ok(preset.tool_ids);
        }
    }
    let known = state
        .repository
        .list_tools()
        .await
        .map_err(storage_problem)?;
    if input
        .tool_ids
        .iter()
        .any(|id| !known.iter().any(|tool| &tool.id == id))
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid tools",
            "one or more toolIds do not exist",
        ));
    }
    let mut values = input.tool_ids.clone();
    values.sort();
    values.dedup();
    Ok(values)
}
async fn agent_values(state: &Arc<AppState>, agents: Vec<McpAgent>) -> Result<Vec<Value>, Problem> {
    let mut values = Vec::with_capacity(agents.len());
    for agent in agents {
        values.push(agent_value(state, agent).await?);
    }
    Ok(values)
}
async fn agent_value(state: &Arc<AppState>, agent: McpAgent) -> Result<Value, Problem> {
    let ids = state
        .repository
        .agent_allowed_tool_ids(&agent.id)
        .await
        .map_err(storage_problem)?;
    let preset = state
        .repository
        .list_presets()
        .await
        .map_err(storage_problem)?
        .into_iter()
        .find(|preset| {
            let mut tools = preset.tool_ids.clone();
            tools.sort();
            tools == ids
        })
        .map(|preset| preset.id);
    Ok(
        json!({ "id":agent.id.as_str(),"name":agent.name,"enabled":agent.enabled,"presetId":preset,"toolIds":ids,"secretLast4":agent.secret_last4,"updatedAtUtc":iso_ms(agent.updated_at_ms) }),
    )
}
fn validate_agent_input(input: &AgentInput) -> Result<(), Problem> {
    if input.name.trim().is_empty() || input.name.chars().count() > 100 {
        Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid agent",
            "name must contain 1..100 characters",
        ))
    } else {
        Ok(())
    }
}
fn agent_id(value: String) -> Result<AgentId, Problem> {
    AgentId::new(value).map_err(|_| bad_id())
}
