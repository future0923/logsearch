use crate::{
    api::AppState,
    config::AppConfig,
    index::LogSearchIndex,
    watcher::{WatchService, spawn_initial_indexing},
};
use std::sync::Arc;

pub fn build_app_state(config: Arc<AppConfig>, index: Arc<LogSearchIndex>) -> AppState {
    spawn_initial_indexing(config.clone(), index.clone());
    WatchService::new(config.clone(), index.clone()).spawn();
    AppState { config, index }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, IndexConfig, LogFileConfig, ServerConfig};
    use std::{path::PathBuf, time::Instant};

    #[tokio::test]
    async fn build_app_state_does_not_block_on_initial_file_indexing() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("large.log");
        let index_dir = dir.path().join("index");
        let line = format!("{}\n", "x".repeat(8_192));
        std::fs::write(&log_path, line.repeat(20_000)).unwrap();

        let config = Arc::new(AppConfig {
            server: ServerConfig {
                addr: "127.0.0.1:0".to_string(),
            },
            index: IndexConfig {
                dir: index_dir.clone(),
            },
            files: vec![LogFileConfig {
                id: "large".to_string(),
                path: PathBuf::from(&log_path),
            }],
            directories: Vec::new(),
        });
        let index = Arc::new(LogSearchIndex::open_or_create(&index_dir).unwrap());

        let started = Instant::now();
        let _state = build_app_state(config, index);

        assert!(
            started.elapsed().as_millis() < 200,
            "startup should not wait for large log indexing"
        );
    }
}
