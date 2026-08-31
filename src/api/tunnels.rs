use super::*;
use reqwest::Url;
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TunnelInput {
    base_url: String,
}

pub(super) async fn ping() -> Json<Value> {
    Json(json!({ "pong": true, "service": "ChatCMD" }))
}

pub(super) async fn managed_tunnel_status(
    State(state): State<Arc<AppState>>,
) -> Json<crate::tunnel_client::TunnelConnectionStatus> {
    Json(state.tunnel.status().await)
}

pub(super) async fn connect_managed_tunnel(
    State(state): State<Arc<AppState>>,
) -> Result<Json<crate::tunnel_client::TunnelConnectionStatus>, Problem> {
    state.tunnel.connect().await.map(Json).map_err(|error| {
        Problem::new(
            StatusCode::BAD_GATEWAY,
            "Tunnel connection failed",
            error.to_string(),
        )
    })
}

pub(super) async fn disconnect_managed_tunnel(
    State(state): State<Arc<AppState>>,
) -> Result<Json<crate::tunnel_client::TunnelConnectionStatus>, Problem> {
    state.tunnel.disconnect().await.map(Json).map_err(|error| {
        Problem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Tunnel disconnect failed",
            error.to_string(),
        )
    })
}

pub(super) async fn list_tunnels(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, Problem> {
    let rows = sqlx::query("SELECT id,base_url,created_at_ms,updated_at_ms FROM tunnels ORDER BY updated_at_ms DESC,id DESC")
        .fetch_all(state.repository.pool())
        .await
        .map_err(db_problem)?;
    Ok(Json(Value::Array(
        rows.into_iter()
            .map(|row| {
                json!({
                    "id": row.get::<i64,_>("id"),
                    "baseUrl": row.get::<String,_>("base_url"),
                    "createdAtUtc": iso_ms(row.get::<i64,_>("created_at_ms")),
                    "updatedAtUtc": iso_ms(row.get::<i64,_>("updated_at_ms"))
                })
            })
            .collect(),
    )))
}

pub(super) async fn create_tunnel(
    State(state): State<Arc<AppState>>,
    Json(input): Json<TunnelInput>,
) -> Result<(StatusCode, Json<Value>), Problem> {
    let base_url = normalize_base_url(&input.base_url)?;
    probe_tunnel(&base_url).await?;
    let now = now_ms();
    let result =
        sqlx::query("INSERT INTO tunnels(base_url,created_at_ms,updated_at_ms) VALUES(?,?,?)")
            .bind(&base_url)
            .bind(now)
            .bind(now)
            .execute(state.repository.pool())
            .await;
    let result = match result {
        Ok(value) => value,
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|db| db.is_unique_violation()) =>
        {
            return Err(Problem::new(
                StatusCode::CONFLICT,
                "Tunnel already exists",
                "this tunnel is already saved",
            ));
        }
        Err(error) => return Err(db_problem(error)),
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": result.last_insert_rowid(),
            "baseUrl": base_url,
            "createdAtUtc": iso_ms(now),
            "updatedAtUtc": iso_ms(now)
        })),
    ))
}

pub(super) async fn test_tunnel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, Problem> {
    let base_url = sqlx::query_scalar::<_, String>("SELECT base_url FROM tunnels WHERE id=?")
        .bind(id)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(not_found)?;
    probe_tunnel(&base_url).await?;
    Ok(Json(
        json!({ "ok": true, "pong": true, "baseUrl": base_url }),
    ))
}

pub(super) async fn plugin_links(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let agent_id = AgentId::new(agent_id).map_err(|_| bad_id())?;
    let raw = state
        .repository
        .ensure_agent_plugin_token(&agent_id)
        .await
        .map_err(storage_problem)?;
    let last4: String = raw
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let rows = sqlx::query("SELECT id,base_url FROM tunnels ORDER BY updated_at_ms DESC,id DESC")
        .fetch_all(state.repository.pool())
        .await
        .map_err(db_problem)?;
    Ok(Json(Value::Array(
        rows.into_iter()
            .map(|row| {
                let id = row.get::<i64, _>("id");
                let base_url = row.get::<String, _>("base_url");
                json!({
                    "tunnelId": id,
                    "baseUrl": base_url,
                    "maskedEndpoint": format!("{}/mcp/***{}", base_url.trim_end_matches('/'), last4)
                })
            })
            .collect(),
    )))
}

pub(super) async fn copy_plugin_link(
    State(state): State<Arc<AppState>>,
    Path((agent_id, tunnel_id)): Path<(String, i64)>,
) -> Result<Json<Value>, Problem> {
    let agent_id = AgentId::new(agent_id).map_err(|_| bad_id())?;
    let base_url = sqlx::query_scalar::<_, String>("SELECT base_url FROM tunnels WHERE id=?")
        .bind(tunnel_id)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(not_found)?;
    let token = state
        .repository
        .ensure_agent_plugin_token(&agent_id)
        .await
        .map_err(storage_problem)?;
    Ok(Json(json!({
        "endpoint": format!("{}/mcp/{token}", base_url.trim_end_matches('/'))
    })))
}

fn normalize_base_url(input: &str) -> Result<String, Problem> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed.chars().count() > 512 {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid tunnel",
            "enter a domain, IP address, or URL",
        ));
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let url = Url::parse(&candidate).map_err(|_| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid tunnel",
            "tunnel must be a valid http or https URL",
        )
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid tunnel",
            "use only the tunnel origin, for example https://example.com or http://1.2.3.4:8080",
        ));
    }
    let mut normalized = url;
    normalized.set_path("");
    Ok(normalized.as_str().trim_end_matches('/').to_owned())
}

async fn probe_tunnel(base_url: &str) -> Result<(), Problem> {
    let url = format!("{}/api/ping", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|_| {
            Problem::new(
                StatusCode::BAD_GATEWAY,
                "Tunnel test failed",
                "could not initialize tunnel probe",
            )
        })?;
    let response = client.get(&url).send().await.map_err(|_| {
        Problem::new(
            StatusCode::BAD_GATEWAY,
            "Tunnel test failed",
            "the domain/IP could not reach this ChatCMD server",
        )
    })?;
    if !response.status().is_success() {
        return Err(Problem::new(
            StatusCode::BAD_GATEWAY,
            "Tunnel test failed",
            format!("ping returned HTTP {}", response.status().as_u16()),
        ));
    }
    let payload = response.json::<Value>().await.map_err(|_| {
        Problem::new(
            StatusCode::BAD_GATEWAY,
            "Tunnel test failed",
            "ping response was not valid ChatCMD JSON",
        )
    })?;
    if payload.get("pong").and_then(Value::as_bool) != Some(true)
        || payload.get("service").and_then(Value::as_str) != Some("ChatCMD")
    {
        return Err(Problem::new(
            StatusCode::BAD_GATEWAY,
            "Tunnel test failed",
            "the endpoint responded, but it is not this ChatCMD server",
        ));
    }
    Ok(())
}
