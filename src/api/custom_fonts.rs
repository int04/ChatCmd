use std::{path::{Path, PathBuf}, sync::Arc};

use axum::{
    Json,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::websocket::AppState;

use super::Problem;

const MAX_FONT_BYTES: usize = 10 * 1024 * 1024;
const FONT_BASENAME: &str = "interface-font";
const FONT_META_FILE: &str = "interface-font.json";

pub(super) async fn upload_interface_font(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<Json<Value>, Problem> {
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), MAX_FONT_BYTES)
        .await
        .map_err(|_| Problem::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Font file too large",
            "Font file must be no larger than 10 MB.",
        ))?;
    if body.is_empty() {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid font file",
            "Font file must not be empty.",
        ));
    }
    let file_name = required_header(&headers, "x-chatcmd-font-filename")?;
    let family = required_header(&headers, "x-chatcmd-font-family")?;
    let extension = font_extension(&file_name).ok_or_else(|| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Unsupported font format",
            "Only .ttf, .otf, .woff, and .woff2 files are supported.",
        )
    })?;
    let mime_type = font_mime(extension);
    let directory = font_directory(&state);
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(font_storage_problem)?;
    remove_existing_font_files(&directory).await?;

    let destination = directory.join(format!("{FONT_BASENAME}.{extension}"));
    let temporary = directory.join(format!(".{FONT_BASENAME}.{extension}.tmp"));
    tokio::fs::write(&temporary, &body)
        .await
        .map_err(font_storage_problem)?;
    tokio::fs::rename(&temporary, &destination)
        .await
        .map_err(font_storage_problem)?;

    let metadata = json!({
        "family": family,
        "fileName": file_name,
        "mimeType": mime_type,
        "size": body.len(),
        "file": destination.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
    });
    tokio::fs::write(
        directory.join(FONT_META_FILE),
        serde_json::to_vec_pretty(&metadata).map_err(|error| font_storage_problem(error.to_string()))?,
    )
    .await
    .map_err(font_storage_problem)?;
    Ok(Json(metadata))
}

pub(super) async fn interface_font(
    State(state): State<Arc<AppState>>,
) -> Result<Response, Problem> {
    let directory = font_directory(&state);
    let metadata = read_metadata(&directory).await?;
    let file = metadata
        .get("file")
        .and_then(Value::as_str)
        .ok_or_else(font_not_found)?;
    if Path::new(file).components().count() != 1 {
        return Err(font_not_found());
    }
    let bytes = tokio::fs::read(directory.join(file))
        .await
        .map_err(|_| font_not_found())?;
    let mime_type = metadata
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime_type).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

pub(super) async fn delete_interface_font(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, Problem> {
    let directory = font_directory(&state);
    if tokio::fs::metadata(&directory).await.is_ok() {
        remove_existing_font_files(&directory).await?;
        let _ = tokio::fs::remove_file(directory.join(FONT_META_FILE)).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

fn font_directory(state: &AppState) -> PathBuf {
    Path::new(&state.database_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("fonts")
}

async fn read_metadata(directory: &Path) -> Result<Value, Problem> {
    let bytes = tokio::fs::read(directory.join(FONT_META_FILE))
        .await
        .map_err(|_| font_not_found())?;
    serde_json::from_slice(&bytes).map_err(|_| font_not_found())
}

async fn remove_existing_font_files(directory: &Path) -> Result<(), Problem> {
    for extension in ["ttf", "otf", "woff", "woff2"] {
        let path = directory.join(format!("{FONT_BASENAME}.{extension}"));
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(font_storage_problem(error)),
        }
    }
    Ok(())
}

fn required_header(headers: &HeaderMap, name: &'static str) -> Result<String, Problem> {
    let value = headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Problem::new(StatusCode::BAD_REQUEST, "Invalid font upload", format!("{name} header is required.")))?;
    Ok(value.chars().filter(|value| !value.is_control()).take(160).collect())
}

fn font_extension(file_name: &str) -> Option<&'static str> {
    let lower = file_name.to_ascii_lowercase();
    ["woff2", "woff", "ttf", "otf"]
        .into_iter()
        .find(|extension| lower.ends_with(&format!(".{extension}")))
}

fn font_mime(extension: &str) -> &'static str {
    match extension {
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn font_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "Font not found", "No uploaded interface font is available.")
}

fn font_storage_problem(error: impl std::fmt::Display) -> Problem {
    Problem::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Font storage error",
        error.to_string(),
    )
}
