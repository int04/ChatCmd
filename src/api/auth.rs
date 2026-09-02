use std::sync::Arc;

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[cfg(target_os = "macos")]
use std::{
    io::Write as _,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    process::{Command, Stdio},
};

use super::Problem;
use crate::{
    backend_api::BackendApiResponse,
    websocket::{AppState, AuthCredentialCache},
};

// macOS releases before Developer ID signing stored this service from an
// unsigned/ad-hoc process. Those Keychain ACLs cannot reliably recognize the
// stable signed app and can show an Allow dialog for every access. Use a new
// service name so the signed app creates a clean credential item. Keep the
// existing service on other platforms to avoid an unrelated forced sign-in.
#[cfg(target_os = "macos")]
const AUTH_KEYRING_SERVICE: &str = "com.chatcmd.client.auth";
#[cfg(not(target_os = "macos"))]
const AUTH_KEYRING_SERVICE: &str = "chatcmd.client.auth";

#[cfg(target_os = "macos")]
const ELEVATED_AUTH_DIRECTORY: &str = "/var/root/Library/Application Support/ChatCmdClient";
#[cfg(target_os = "macos")]
const ELEVATED_AUTH_FILE: &str =
    "/var/root/Library/Application Support/ChatCmdClient/auth-session.json";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CredentialsInput {
    email: String,
    password: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangePasswordInput {
    current_password: String,
    new_password: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    access_token_expires_at: String,
    refresh_token_expires_at: String,
}

#[cfg(target_os = "macos")]
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ElevatedCredential {
    account: String,
    secret: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceAuthRequest<'a> {
    email: &'a str,
    password: &'a str,
    machine_id: &'a str,
    name_device: Option<&'a str>,
    platform: Option<&'a str>,
    os_version: Option<&'a str>,
    app_version: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(super) struct StoredAuthSession {
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthUsageResponse {
    use_next_time: Option<String>,
    use_next_reset: Option<String>,
    plan: AuthPlanResponse,
}

#[derive(Debug, Deserialize)]
struct AuthPlanResponse {
    #[serde(rename = "type")]
    plan_type: i64,
}

pub(super) async fn register(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response, Problem> {
    authenticate(&state, "/api/auth/register", parse_credentials(&body)?).await
}

pub(super) async fn login(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response, Problem> {
    authenticate(&state, "/api/auth/login", parse_credentials(&body)?).await
}

pub(super) async fn info(State(state): State<Arc<AppState>>) -> Result<Response, Problem> {
    authorized_request(&state, Method::GET, "/api/auth/info", &[], None).await
}

pub(super) async fn change_password(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response, Problem> {
    let input: ChangePasswordInput = serde_json::from_slice(&body).map_err(|_| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid request body",
            "Password change request body must be valid JSON.",
        )
    })?;
    let payload = serde_json::to_vec(&input).map_err(|_| internal_problem())?;
    let response = authorized_request(
        &state,
        Method::POST,
        "/api/auth/change-password",
        &payload,
        None,
    )
    .await?;
    if response.status().is_success() {
        clear_session(&state).await?;
    }
    Ok(response)
}

pub(super) async fn logout(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    clear_session(&state).await?;
    Ok(Json(json!({ "authenticated": false })))
}

pub(super) async fn require_auth(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, Problem> {
    if load_session(&state).await?.is_none() {
        return Err(unauthorized(
            "Authentication required",
            "Sign in before using the local API.",
        ));
    }
    Ok(next.run(request).await)
}

pub(super) async fn authorized_request(
    state: &Arc<AppState>,
    method: Method,
    backend_path: &str,
    body: &[u8],
    accept_language: Option<&str>,
) -> Result<Response, Problem> {
    let initial = load_session(state).await?.ok_or_else(|| {
        unauthorized(
            "Authentication required",
            "Sign in before using the local API.",
        )
    })?;
    let response = send_authorized(
        state,
        method.clone(),
        backend_path,
        body,
        &initial.access_token,
        accept_language,
    )
    .await?;
    if response.status != StatusCode::UNAUTHORIZED.as_u16() || is_business_unauthorized(&response) {
        return Ok(to_response(response));
    }

    let _refresh_guard = state.auth_refresh_lock.lock().await;
    let current = load_session(state).await?.ok_or_else(|| {
        unauthorized(
            "Authentication required",
            "Your local authentication session is unavailable.",
        )
    })?;

    if current.access_token != initial.access_token {
        let retry = send_authorized(
            state,
            method.clone(),
            backend_path,
            body,
            &current.access_token,
            accept_language,
        )
        .await?;
        if retry.status != StatusCode::UNAUTHORIZED.as_u16() || is_business_unauthorized(&retry) {
            return Ok(to_response(retry));
        }
    }

    let refreshed = match refresh(state, &current).await {
        Ok(tokens) => tokens,
        Err(problem) => {
            clear_session(state).await?;
            return Err(problem);
        }
    };
    let retry = send_authorized(
        state,
        method,
        backend_path,
        body,
        &refreshed.access_token,
        accept_language,
    )
    .await?;
    if retry.status == StatusCode::UNAUTHORIZED.as_u16() {
        clear_session(state).await?;
    }
    Ok(to_response(retry))
}

fn parse_credentials(body: &[u8]) -> Result<CredentialsInput, Problem> {
    serde_json::from_slice(body).map_err(|_| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid request body",
            "Authentication request body must be valid JSON.",
        )
    })
}

async fn authenticate(
    state: &Arc<AppState>,
    path: &str,
    input: CredentialsInput,
) -> Result<Response, Problem> {
    let machine_id = state.device.machine_id.as_deref().ok_or_else(|| {
        Problem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Machine identity unavailable",
            "ChatCMD could not resolve a stable machine identifier on this device.",
        )
    })?;
    let payload = DeviceAuthRequest {
        email: input.email.trim(),
        password: &input.password,
        machine_id,
        name_device: Some(state.device.name.as_str()),
        platform: Some(state.device.platform.as_str()),
        os_version: state.device.os_version.as_deref(),
        app_version: Some(state.device.app_version.as_str()),
    };
    let body = serde_json::to_vec(&payload).map_err(|_| internal_problem())?;
    let backend = state
        .backend_api
        .request_with_fresh_session(Method::POST, path, &body, None, None)
        .await
        .map_err(|error| {
            tracing::warn!(error = %error, path, "encrypted backend authentication request failed");
            backend_unavailable()
        })?;
    if backend.status < 200 || backend.status >= 300 {
        return Ok(to_response(backend));
    }
    let tokens: TokenResponse = serde_json::from_slice(&backend.body).map_err(|_| {
        Problem::new(
            StatusCode::BAD_GATEWAY,
            "Invalid backend response",
            "Authentication response could not be parsed.",
        )
    })?;
    save_session(state, &tokens).await?;
    Ok((StatusCode::OK, Json(json!({ "authenticated": true }))).into_response())
}

async fn refresh(
    state: &Arc<AppState>,
    session: &StoredAuthSession,
) -> Result<TokenResponse, Problem> {
    let body = serde_json::to_vec(&json!({ "refreshToken": session.refresh_token }))
        .map_err(|_| internal_problem())?;
    let backend = state
        .backend_api
        .request(Method::POST, "/api/auth/refresh", &body, None, None)
        .await
        .map_err(|_| backend_unavailable())?;
    if backend.status < 200 || backend.status >= 300 {
        let detail = backend_error_message(&backend.body)
            .unwrap_or_else(|| "Refresh token is no longer valid.".to_owned());
        return Err(unauthorized("Authentication expired", detail));
    }
    let tokens: TokenResponse = serde_json::from_slice(&backend.body).map_err(|_| {
        Problem::new(
            StatusCode::BAD_GATEWAY,
            "Invalid backend response",
            "Refresh response could not be parsed.",
        )
    })?;
    save_session(state, &tokens).await?;
    Ok(tokens)
}

async fn send_authorized(
    state: &Arc<AppState>,
    method: Method,
    path: &str,
    body: &[u8],
    access_token: &str,
    accept_language: Option<&str>,
) -> Result<BackendApiResponse, Problem> {
    let authorization = format!("Bearer {access_token}");
    let response = state
        .backend_api
        .request(method, path, body, Some(&authorization), accept_language)
        .await
        .map_err(|_| backend_unavailable())?;

    if path == "/api/auth/info"
        && (200..300).contains(&response.status)
        && let Ok(info) = serde_json::from_slice::<AuthUsageResponse>(&response.body)
    {
        let mut cache = state.auth_usage_cache.write().await;
        cache.authenticated = true;
        cache.use_next_time = info.use_next_time;
        cache.use_next_reset = info.use_next_reset;
        cache.plan_type = Some(info.plan.plan_type);
    }

    Ok(response)
}

pub(super) async fn warm_runtime_cache(state: &Arc<AppState>) {
    let _ = authorized_request(state, Method::GET, "/api/auth/info", &[], None).await;
}

pub(super) async fn load_session(
    state: &Arc<AppState>,
) -> Result<Option<StoredAuthSession>, Problem> {
    let secret = {
        // Serialize the first read and cache it for the process lifetime. On
        // macOS, every Keychain read by an elevated process can show a new
        // confirmation dialog when the user chose Allow instead of Always
        // Allow. API authentication must not touch Keychain on every request.
        let mut cache = state.auth_credential_cache.lock().await;
        match &*cache {
            AuthCredentialCache::Ready(secret) => secret.clone(),
            AuthCredentialCache::Unavailable => return Err(secure_store_problem()),
            AuthCredentialCache::Uninitialized => {
                let account = state.backend_api.base_url().to_owned();
                let read_result = tokio::task::spawn_blocking(move || {
                    #[cfg(target_os = "macos")]
                    if is_macos_elevated() {
                        return read_elevated_credential(&account);
                    }
                    read_keyring_credential(&account)
                })
                .await
                .map_err(|_| secure_store_problem())?;
                match read_result {
                    Ok(secret) => {
                        *cache = AuthCredentialCache::Ready(secret.clone());
                        secret
                    }
                    Err(error) => {
                        tracing::warn!(error, "read auth session from OS credential vault failed");
                        *cache = AuthCredentialCache::Unavailable;
                        return Err(secure_store_problem());
                    }
                }
            }
        }
    };
    let Some(secret) = secret else {
        state.auth_usage_cache.write().await.authenticated = false;
        return Ok(None);
    };
    let tokens: TokenResponse =
        serde_json::from_str(&secret).map_err(|_| secure_store_problem())?;
    state.auth_usage_cache.write().await.authenticated = true;
    Ok(Some(StoredAuthSession {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
    }))
}

async fn save_session(state: &Arc<AppState>, tokens: &TokenResponse) -> Result<(), Problem> {
    let account = state.backend_api.base_url().to_owned();
    let secret = serde_json::to_string(tokens).map_err(|_| internal_problem())?;
    let stored_secret = secret.clone();
    tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        if is_macos_elevated() {
            return write_elevated_credential(&account, &stored_secret);
        }
        write_keyring_credential(&account, &stored_secret)
    })
    .await
    .map_err(|_| secure_store_problem())?
    .map_err(|error| {
        tracing::warn!(error, "write auth session to OS credential vault failed");
        secure_store_problem()
    })?;
    *state.auth_credential_cache.lock().await = AuthCredentialCache::Ready(Some(secret));
    state.auth_usage_cache.write().await.authenticated = true;
    Ok(())
}

async fn clear_session(state: &Arc<AppState>) -> Result<(), Problem> {
    let account = state.backend_api.base_url().to_owned();
    tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        if is_macos_elevated() {
            return delete_elevated_credential();
        }
        delete_keyring_credential(&account)
    })
    .await
    .map_err(|_| secure_store_problem())?
    .map_err(|error| {
        tracing::warn!(error, "delete auth session from OS credential vault failed");
        secure_store_problem()
    })?;
    *state.auth_credential_cache.lock().await = AuthCredentialCache::Ready(None);
    *state.auth_usage_cache.write().await = Default::default();
    state.backend_api.reset_session().await;
    Ok(())
}

fn read_keyring_credential(account: &str) -> Result<Option<String>, String> {
    let entry =
        keyring::Entry::new(AUTH_KEYRING_SERVICE, account).map_err(|error| error.to_string())?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn write_keyring_credential(account: &str, secret: &str) -> Result<(), String> {
    keyring::Entry::new(AUTH_KEYRING_SERVICE, account)
        .and_then(|entry| entry.set_password(secret))
        .map_err(|error| error.to_string())
}

fn delete_keyring_credential(account: &str) -> Result<(), String> {
    let entry =
        keyring::Entry::new(AUTH_KEYRING_SERVICE, account).map_err(|error| error.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(target_os = "macos")]
fn is_macos_elevated() -> bool {
    Command::new("/usr/bin/id")
        .arg("-u")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
}

#[cfg(target_os = "macos")]
fn read_elevated_credential(account: &str) -> Result<Option<String>, String> {
    let content = match std::fs::read_to_string(ELEVATED_AUTH_FILE) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let credential: ElevatedCredential =
        serde_json::from_str(&content).map_err(|error| error.to_string())?;
    Ok((credential.account == account).then_some(credential.secret))
}

#[cfg(target_os = "macos")]
fn write_elevated_credential(account: &str, secret: &str) -> Result<(), String> {
    let directory = Path::new(ELEVATED_AUTH_DIRECTORY);
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let temporary = directory.join(format!(".auth-session-{}.tmp", uuid::Uuid::new_v4()));
    let content = serde_json::to_vec(&ElevatedCredential {
        account: account.to_owned(),
        secret: secret.to_owned(),
    })
    .map_err(|error| error.to_string())?;
    let result = write_private_file(&temporary, &content).and_then(|()| {
        std::fs::rename(&temporary, ELEVATED_AUTH_FILE).map_err(|error| error.to_string())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(target_os = "macos")]
fn write_private_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(content).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn delete_elevated_credential() -> Result<(), String> {
    match std::fs::remove_file(ELEVATED_AUTH_FILE) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn to_response(backend: BackendApiResponse) -> Response {
    let status = StatusCode::from_u16(backend.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::new(Body::from(backend.body));
    *response.status_mut() = status;
    if let Some(content_type) = backend.content_type
        && let Ok(value) = HeaderValue::from_str(&content_type)
    {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

fn backend_error_message(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    value
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn is_business_unauthorized(response: &BackendApiResponse) -> bool {
    serde_json::from_slice::<Value>(&response.body)
        .ok()
        .and_then(|value| value.get("code").and_then(Value::as_str).map(str::to_owned))
        .is_some_and(|code| code == "invalid_current_password")
}

fn unauthorized(title: impl Into<String>, detail: impl Into<String>) -> Problem {
    Problem::new(StatusCode::UNAUTHORIZED, title, detail)
}

fn backend_unavailable() -> Problem {
    Problem::new(
        StatusCode::BAD_GATEWAY,
        "Backend unavailable",
        "Encrypted backend API request failed.",
    )
}

fn internal_problem() -> Problem {
    Problem::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Internal error",
        "Local authentication state could not be prepared.",
    )
}

fn secure_store_problem() -> Problem {
    Problem::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Secure credential store unavailable",
        "ChatCMD could not access the operating system credential vault.",
    )
}
