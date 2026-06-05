use crate::{
    config::{AppConfig, AuthConfig},
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
    http::{Request, StatusCode, header},
    middleware::{self, Next},
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
    let auth = state
        .config
        .server
        .auth
        .as_ref()
        .filter(|auth| auth.enabled)
        .cloned();

    let app = Router::new()
        .route("/api/status", get(status))
        .route("/api/search", post(search))
        .route("/api/around", post(around))
        .route("/api/tail", get(tail))
        .fallback_service(ServeDir::new(&static_dir).fallback(ServeFile::new(index_html)))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    match auth {
        Some(auth) => app
            .layer(middleware::from_fn_with_state(auth, require_basic_auth))
            .with_state(state),
        None => app.with_state(state),
    }
}

async fn require_basic_auth(
    State(auth): State<AuthConfig>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_basic_credentials)
        .is_some_and(|(username, password)| username == auth.username && password == auth.password);

    if authorized {
        return next.run(req).await;
    }

    unauthorized_response(&auth.realm)
}

fn parse_basic_credentials(value: &str) -> Option<(String, String)> {
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = decode_base64(encoded.trim())?;
    let credentials = String::from_utf8(decoded).ok()?;
    let (username, password) = credentials.split_once(':')?;
    Some((username.to_string(), password.to_string()))
}

fn unauthorized_response(realm: &str) -> Response {
    let challenge = format!("Basic realm=\"{}\"", realm.replace('"', ""));
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, challenge)],
    )
        .into_response()
}

fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return None;
    }

    let mut output = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks_exact(4) {
        let mut values = [0_u8; 4];
        let mut padding = 0;
        for (idx, byte) in chunk.iter().enumerate() {
            values[idx] = match *byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if idx >= 2 => {
                    padding += 1;
                    0
                }
                _ => return None,
            };
        }
        if padding > 0 && !chunk[4 - padding..].iter().all(|byte| *byte == b'=') {
            return None;
        }

        output.push((values[0] << 2) | (values[1] >> 4));
        if padding < 2 {
            output.push((values[1] << 4) | (values[2] >> 2));
        }
        if padding == 0 {
            output.push((values[2] << 6) | values[3]);
        }
    }

    Some(output)
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
    use crate::{
        config::{IndexConfig, LogFileConfig, ServerConfig},
        index::LogSearchIndex,
    };
    use axum::{
        body::Body,
        http::{Request, header},
    };
    use tower::ServiceExt;

    #[test]
    fn tail_defaults_to_linux_tail_line_count() {
        assert_eq!(default_tail_lines(), 10);
    }

    #[tokio::test]
    async fn router_rejects_requests_without_basic_auth_when_enabled() {
        let (app, _dir) = test_router_with_auth("admin", "secret");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Basic realm=\"Log Search\""
        );
    }

    #[tokio::test]
    async fn router_accepts_requests_with_valid_basic_auth_when_enabled() {
        let (app, _dir) = test_router_with_auth("admin", "secret");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .header(header::AUTHORIZATION, "Basic YWRtaW46c2VjcmV0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn router_rejects_requests_with_invalid_basic_auth_when_enabled() {
        let (app, _dir) = test_router_with_auth("admin", "secret");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .header(header::AUTHORIZATION, "Basic YWRtaW46d3Jvbmc=")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Basic realm=\"Log Search\""
        );
    }

    fn test_router_with_auth(username: &str, password: &str) -> (Router, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let static_dir = dir.path().join("static");
        std::fs::create_dir(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<!doctype html>").unwrap();
        let config = Arc::new(AppConfig {
            server: ServerConfig {
                addr: "127.0.0.1:0".to_string(),
                auth: Some(crate::config::AuthConfig {
                    enabled: true,
                    username: username.to_string(),
                    password: password.to_string(),
                    realm: "Log Search".to_string(),
                }),
            },
            index: IndexConfig { dir: index_dir },
            files: vec![LogFileConfig {
                id: "app".to_string(),
                path: dir.path().join("app.log"),
            }],
            directories: Vec::new(),
        });
        std::fs::write(&config.files[0].path, "").unwrap();
        let index = Arc::new(LogSearchIndex::open_or_create(&config.index.dir).unwrap());
        (router(AppState { config, index }, static_dir), dir)
    }
}
