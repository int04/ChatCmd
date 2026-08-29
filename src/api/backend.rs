use std::sync::Arc;

use axum::{
    body::to_bytes,
    extract::{OriginalUri, Path, Request, State},
    http::{StatusCode, header},
    response::Response,
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
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid method",
            "unsupported HTTP method",
        )
    })?;
    let query = original_uri
        .query()
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    let backend_path = format!("/api/{relative}{query}");
    let accept_language = request
        .headers()
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (_, body) = request.into_parts();
    let body = to_bytes(body, MAX_GATEWAY_BODY_BYTES).await.map_err(|_| {
        Problem::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request too large",
            "backend request body is too large",
        )
    })?;

    super::auth::authorized_request(
        &state,
        method,
        &backend_path,
        body.as_ref(),
        accept_language.as_deref(),
    )
    .await
}
