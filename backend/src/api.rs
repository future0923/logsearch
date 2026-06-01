use crate::{
    config::AppConfig,
    index::{IndexStatus, LogSearchIndex},
    search::{AroundRequest, AroundResponse, SearchRequest, SearchResponse},
    watcher::{DiscoveredFileSource, discover_files, watched_directories},
};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use std::{path::PathBuf, sync::Arc, time::Instant};
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub index: Arc<LogSearchIndex>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    files: usize,
    directories: usize,
    file_sources: Vec<FileSourceResponse>,
    configured_directories: Vec<DirectorySourceResponse>,
    discovered_files: Vec<DiscoveredFileResponse>,
    index_dir: String,
    watched_directories: Vec<String>,
    indexing: Vec<IndexStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSourceResponse {
    id: String,
    path: String,
    kind: String,
    source: String,
    directory_id: Option<String>,
    exists: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectorySourceResponse {
    id: String,
    path: String,
    recursive: bool,
    exists: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveredFileResponse {
    id: String,
    path: String,
    kind: String,
    source: String,
    directory_id: Option<String>,
    exists: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorResponse {
    error: String,
}

pub fn router(state: AppState, static_dir: PathBuf) -> Router {
    let index_html = static_dir.join("index.html");

    Router::new()
        .route("/api/status", get(status))
        .route("/api/search", post(search))
        .route("/api/around", post(around))
        .fallback_service(ServeDir::new(&static_dir).fallback(ServeFile::new(index_html)))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn around(
    State(state): State<AppState>,
    Json(req): Json<AroundRequest>,
) -> Result<Json<AroundResponse>, ApiError> {
    req.validate().map_err(ApiError::bad_request)?;
    Ok(Json(state.index.read_around(&req)?))
}

async fn status(State(state): State<AppState>) -> Json<StatusResponse> {
    let indexing = state.index.status_snapshot();
    let discovered_files = discover_files(&state.config);
    Json(StatusResponse {
        files: state.config.files.len(),
        directories: state.config.directories.len(),
        file_sources: discovered_files.iter().map(file_source_response).collect(),
        configured_directories: state
            .config
            .directories
            .iter()
            .map(|directory| DirectorySourceResponse {
                id: directory.id.clone(),
                path: directory.path.to_string_lossy().to_string(),
                recursive: directory.recursive,
                exists: directory.path.exists(),
            })
            .collect(),
        discovered_files: discovered_files
            .iter()
            .map(discovered_file_response)
            .collect(),
        index_dir: state.config.index.dir.to_string_lossy().to_string(),
        watched_directories: watched_directories(&state.config)
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
        indexing,
    })
}

fn file_source_response(file: &crate::watcher::DiscoveredFile) -> FileSourceResponse {
    let directory_id = match &file.source {
        DiscoveredFileSource::Directory { directory_id } => Some(directory_id.clone()),
        DiscoveredFileSource::ConfiguredFile => None,
    };
    FileSourceResponse {
        id: file.id.clone(),
        path: file.path.to_string_lossy().to_string(),
        kind: file.kind.as_str().to_string(),
        source: file.source.as_str().to_string(),
        directory_id,
        exists: file.exists,
    }
}

fn discovered_file_response(file: &crate::watcher::DiscoveredFile) -> DiscoveredFileResponse {
    let source = file_source_response(file);
    DiscoveredFileResponse {
        id: source.id,
        path: source.path,
        kind: source.kind,
        source: source.source,
        directory_id: source.directory_id,
        exists: source.exists,
    }
}

async fn search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    req.validate().map_err(ApiError::bad_request)?;
    let started = Instant::now();
    let (hits, elapsed_ms, truncated, has_next, next_cursor) = state.index.search(&req)?;

    Ok(Json(SearchResponse {
        total: hits.len(),
        hits,
        truncated,
        has_next,
        next_cursor,
        elapsed_ms: elapsed_ms.max(started.elapsed().as_millis()),
    }))
}

pub struct ApiError {
    status: StatusCode,
    error: anyhow::Error,
}

impl ApiError {
    fn bad_request(error: impl Into<anyhow::Error>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: error.into(),
        }
    }
}

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(value: E) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: value.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.error.to_string(),
            }),
        )
            .into_response()
    }
}
