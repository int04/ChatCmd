use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    extract::{OriginalUri, Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use reqwest::Method;

use super::Problem;
use crate::websocket::AppState;

const MAX_GATEWAY_BODY_BYTES: usize = 16 * 1024 * 1024;

pub(super) async fn proxy(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
    OriginalUri(original_uri): OriginalUri,
    request: Request,
) -> Result<Response, Problem> {
    let relative = path.trim_matches('/');
    if relative.is_empty() || relative.starts_with("crypto/") {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid backend path",
            "backend API path is not allowed",
        ));
    }

    let method = Method::from_bytes(request.method().as_str().as_bytes()).map_err(|_| {
        Problem::new(StatusCode::BAD_REQUEST, "Invalid method", "unsupported HTTP method")
    })?;
    let query = original_uri.query().map(|value| format!("?{value}")).unwrap_or_default();
    let backend_path = format!("/api/{relative}{query}");
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let accept_language = request
        .headers()
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (_, body) = request.into_parts();
    let body = to_bytes(body, MAX_GATEWAY_BODY_BYTES).await.map_err(|_| {
        Problem::new(StatusCode::PAYLOAD_TOO_LARGE, "Request too large", "backend request body is too large")
    })?;

    let backend = state
        .backend_api
        .request(
            method,
            &backend_path,
            body.as_ref(),
            authorization.as_deref(),
            accept_language.as_deref(),
        )
        .await
        .map_err(|_| {
            Problem::new(
                StatusCode::BAD_GATEWAY,
                "Backend unavailable",
                "encrypted backend API request failed",
            )
        })?;

    let status = StatusCode::from_u16(backend.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::new(Body::from(backend.body));
    *response.status_mut() = status;
    if let Some(content_type) = backend.content_type
        && let Ok(value) = HeaderValue::from_str(&content_type)
    {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    Ok(response.into_response())
}
