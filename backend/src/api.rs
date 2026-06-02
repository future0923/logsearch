use crate::{
    config::AppConfig,
    index::{IndexStatus, LogSearchIndex},
    search::{AroundRequest, AroundResponse, SearchRequest, SearchResponse},
    tail::{TailError, TailLine, initial_tail_snapshot, read_new_tail_lines, resolve_tail_path},
    watcher::{
        DiscoveredFileKind, DiscoveredFileSource, MAX_DISCOVERED_FILES, discover_files,
        watched_directories,
    },
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, path::PathBuf, sync::Arc, time::Instant};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
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
    discovered_files_truncated: bool,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TailQuery {
    file_id: String,
    #[serde(default = "default_tail_lines")]
    lines: usize,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    next_line_no: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TailEvent {
    path: String,
    offset: u64,
    next_line_no: u64,
    lines: Vec<TailLine>,
}

pub fn router(state: AppState, static_dir: PathBuf) -> Router {
    let index_html = static_dir.join("index.html");

    Router::new()
        .route("/api/status", get(status))
        .route("/api/search", post(search))
        .route("/api/around", post(around))
        .route("/api/tail", get(tail))
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
    let discovered_files_truncated = discovered_files.len() >= MAX_DISCOVERED_FILES;
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
        discovered_files_truncated,
        index_dir: state.config.index.dir.to_string_lossy().to_string(),
        watched_directories: watched_directories(&state.config)
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
        indexing,
    })
}

async fn tail(
    State(state): State<AppState>,
    Query(query): Query<TailQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let path = resolve_tail_path(&state.config, &query.file_id).map_err(ApiError::from_tail)?;
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(32);
    let lines = query.lines;
    let offset = query.offset;
    let next_line_no = query.next_line_no;

    tokio::task::spawn_blocking(move || {
        let mut sent_initial = false;
        let mut snapshot = match (offset, next_line_no) {
            (Some(offset), Some(next_line_no)) => read_new_tail_lines(&path, offset, next_line_no),
            _ => initial_tail_snapshot(&path, lines),
        };

        loop {
            match snapshot {
                Ok(payload) => {
                    let event = TailEvent {
                        path: payload.path.to_string_lossy().to_string(),
                        offset: payload.offset,
                        next_line_no: payload.next_line_no,
                        lines: payload.lines,
                    };
                    let should_send = !sent_initial || !event.lines.is_empty();
                    sent_initial = true;
                    if should_send {
                        let Ok(data) = serde_json::to_string(&event) else {
                            break;
                        };
                        if tx
                            .blocking_send(Ok(Event::default().event("tail").data(data)))
                            .is_err()
                        {
                            break;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(700));
                    snapshot = read_new_tail_lines(&path, event.offset, event.next_line_no);
                }
                Err(err) => {
                    let data = serde_json::json!({ "error": err.to_string() }).to_string();
                    let _ = tx.blocking_send(Ok(Event::default().event("error").data(data)));
                    break;
                }
            }
        }
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()))
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
    refresh_index_metadata(&state)?;
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

fn refresh_index_metadata(state: &AppState) -> anyhow::Result<()> {
    for file in discover_files(&state.config)
        .into_iter()
        .filter(|file| file.exists)
    {
        match file.kind {
            DiscoveredFileKind::Hot => {
                state.index.sync_file_metadata(&file.id, &file.path)?;
            }
            DiscoveredFileKind::Gzip
            | DiscoveredFileKind::Zstd
            | DiscoveredFileKind::Bzip2
            | DiscoveredFileKind::Xz => {
                state.index.sync_compressed_file_metadata(
                    &file.id,
                    &file.path,
                    file.kind.as_str(),
                )?;
            }
        }
    }
    Ok(())
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

    fn from_tail(error: TailError) -> Self {
        let status = match error {
            TailError::EmptyFileId => StatusCode::BAD_REQUEST,
            TailError::NotFound(_) => StatusCode::NOT_FOUND,
            TailError::NotHot | TailError::Missing(_) => StatusCode::CONFLICT,
        };
        Self {
            status,
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

pub(crate) fn default_tail_lines() -> usize {
    10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_defaults_to_linux_tail_line_count() {
        assert_eq!(default_tail_lines(), 10);
    }
}
