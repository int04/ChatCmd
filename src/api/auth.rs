use axum::{
    Json,
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use super::Problem;
use crate::{
    gui_auth::{clear_session_cookie, session_cookie},
    websocket::AppState,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PasswordInput {
    password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetupInput {
    password: String,
    confirm_password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChangePasswordInput {
    current_password: String,
    new_password: String,
    confirm_password: String,
}

pub(super) async fn auth_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, Problem> {
    let configured = state
        .gui_auth
        .has_password()
        .await
        .map_err(auth_storage_problem)?;
    let authenticated = configured
        && state
            .gui_auth
            .authenticate_cookie(cookie_header(&headers))
            .await;
    Ok(Json(json!({
        "configured": configured,
        "authenticated": authenticated,
        "idleTimeoutSeconds": crate::gui_auth::SESSION_IDLE_SECONDS,
    })))
}

pub(super) async fn setup(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetupInput>,
) -> Result<Response, Problem> {
    if input.password != input.confirm_password {
        return Err(bad_request("Password confirmation does not match."));
    }
    let token = state
        .gui_auth
        .setup_password(input.password)
        .await
        .map_err(auth_input_problem)?;
    Ok(session_response(token))
}

pub(super) async fn login(
    State(state): State<Arc<AppState>>,
    Json(input): Json<PasswordInput>,
) -> Result<Response, Problem> {
    match state
        .gui_auth
        .login(input.password)
        .await
        .map_err(auth_input_problem)?
    {
        Some(token) => Ok(session_response(token)),
        None => Err(Problem::new(
            StatusCode::UNAUTHORIZED,
            "Invalid password",
            "The password is incorrect.",
        )),
    }
}

pub(super) async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    state.gui_auth.logout_cookie(cookie_header(&headers)).await;
    let mut response = Json(json!({ "authenticated": false })).into_response();
    set_cookie(&mut response, &clear_session_cookie());
    response
}

pub(super) async fn change_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<ChangePasswordInput>,
) -> Result<Response, Problem> {
    if !state
        .gui_auth
        .authenticate_cookie(cookie_header(&headers))
        .await
    {
        return Err(unauthorized());
    }
    if input.new_password != input.confirm_password {
        return Err(bad_request("Password confirmation does not match."));
    }
    match state
        .gui_auth
        .change_password(input.current_password, input.new_password)
        .await
        .map_err(auth_input_problem)?
    {
        Some(token) => Ok(session_response(token)),
        None => Err(Problem::new(
            StatusCode::UNAUTHORIZED,
            "Invalid password",
            "The current password is incorrect.",
        )),
    }
}

pub(super) async fn require_gui_auth(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, Problem> {
    let caller = request
        .headers()
        .get("x-chatcmdclient")
        .and_then(|value| value.to_str().ok());
    if caller == Some("chatgpt-extension") {
        // Router::nest strips /api/local from request.uri(); the allowlist uses
        // the original public path, as does encrypted_local_api's AAD binding.
        let path = request
            .extensions()
            .get::<OriginalUri>()
            .map_or_else(|| request.uri().path(), |original| original.0.path());
        if extension_route_allowed(request.method(), path) {
            return Ok(next.run(request).await);
        }
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "the ChatGPT extension cannot access this management endpoint",
        ));
    }
    if caller != Some("local-ui") {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "local UI authentication is required",
        ));
    }

    let token = cookie_token(cookie_header(request.headers())).map(str::to_owned);
    if !state
        .gui_auth
        .authenticate_cookie(cookie_header(request.headers()))
        .await
    {
        return Err(unauthorized());
    }
    let mut response = next.run(request).await;
    if let Some(token) = token {
        set_cookie(&mut response, &session_cookie(&token));
    }
    Ok(response)
}

fn extension_route_allowed(method: &Method, path: &str) -> bool {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    match (method, parts.as_slice()) {
        (&Method::GET, ["api", "local", "chatgpt", "capture", "capabilities"])
        | (&Method::POST, ["api", "local", "chatgpt", "capture", "turns"]) => true,
        (&Method::GET, ["api", "local", "chatgpt", "requests", _]) => true,
        (&Method::POST, ["api", "local", "subagents", _, "fallback", action]) => {
            matches!(*action, "started" | "result")
        }
        (&Method::POST, ["api", "local", "chatgpt", "bridge", _, action]) => {
            matches!(
                *action,
                "started" | "identity" | "result" | "browser-completed" | "observation"
            )
        }
        (&Method::GET, ["api", "local", "tasks", "approvals", "pending"])
        | (&Method::GET, ["api", "local", "tasks", "activity-approvals", "pending"])
        | (&Method::GET, ["api", "local", "plan", "questions", "pending"]) => true,
        (&Method::POST, ["api", "local", "tasks", _, action]) => {
            matches!(*action, "approve-execution" | "reject-execution")
        }
        (&Method::POST, ["api", "local", "tasks", _, "activities", _, "approval"])
        | (&Method::POST, ["api", "local", "plan", "questions", _, "answer"]) => true,
        _ => false,
    }
}

fn cookie_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
}

fn session_response(token: String) -> Response {
    let mut response = Json(json!({ "authenticated": true })).into_response();
    set_cookie(&mut response, &session_cookie(&token));
    response
}

fn set_cookie(response: &mut Response, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn cookie_token(header: Option<&str>) -> Option<&str> {
    header?.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == crate::gui_auth::SESSION_COOKIE && !value.is_empty()).then_some(value)
    })
}

fn unauthorized() -> Problem {
    Problem::new(
        StatusCode::UNAUTHORIZED,
        "Authentication required",
        "Sign in to the ChatCMD management interface.",
    )
}

fn bad_request(detail: &str) -> Problem {
    Problem::new(StatusCode::BAD_REQUEST, "Invalid password", detail)
}

fn auth_input_problem(error: anyhow::Error) -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "Authentication error",
        error.to_string(),
    )
}

fn auth_storage_problem(_: anyhow::Error) -> Problem {
    Problem::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Authentication error",
        "Unable to read GUI authentication state.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_access_is_limited_to_bridge_routes() {
        assert!(extension_route_allowed(
            &Method::POST,
            "/api/local/chatgpt/bridge/request-1/result"
        ));
        assert!(extension_route_allowed(
            &Method::GET,
            "/api/local/tasks/approvals/pending"
        ));
        assert!(!extension_route_allowed(
            &Method::GET,
            "/api/local/settings"
        ));
        assert!(!extension_route_allowed(
            &Method::GET,
            "/api/local/mcp/agents"
        ));
    }
}
