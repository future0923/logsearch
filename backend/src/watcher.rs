use crate::{
    config::{AppConfig, LogDirectoryConfig},
    index::{LogSearchIndex, trace_index_logs_enabled},
};
use notify::{Event, RecursiveMode, Watcher, recommended_watcher};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::mpsc,
    time::{Instant, sleep_until},
};
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

const MAX_CONCURRENT_INDEX_JOBS: usize = 1;
pub const MAX_DISCOVERED_FILES: usize = 2_000;
const MAX_PENDING_INDEX_JOBS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndexJob {
    Hot {
        source_id: String,
        path: PathBuf,
    },
    Compressed {
        source_id: String,
        path: PathBuf,
        kind: DiscoveredFileKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiscoveredFileKind {
    Hot,
    Gzip,
    Zstd,
    Bzip2,
    Xz,
}

impl DiscoveredFileKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
            Self::Bzip2 => "bzip2",
            Self::Xz => "xz",
        }
    }

    pub fn is_compressed(&self) -> bool {
        !matches!(self, Self::Hot)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveredFileSource {
    ConfiguredFile,
    Directory { directory_id: String },
}

impl DiscoveredFileSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ConfiguredFile => "file",
            Self::Directory { .. } => "directory",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub id: String,
    pub path: PathBuf,
    pub kind: DiscoveredFileKind,
    pub source: DiscoveredFileSource,
    pub exists: bool,
}

impl IndexJob {
    fn source_id(&self) -> &str {
        match self {
            IndexJob::Hot { source_id, .. } | IndexJob::Compressed { source_id, .. } => source_id,
        }
    }

    fn path(&self) -> &Path {
        match self {
            IndexJob::Hot { path, .. } | IndexJob::Compressed { path, .. } => path,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            IndexJob::Hot { .. } => "hot",
            IndexJob::Compressed { kind, .. } => kind.as_str(),
        }
    }
}

pub struct WatchService {
    config: Arc<AppConfig>,
    index: Arc<LogSearchIndex>,
    debounce: Duration,
}

impl WatchService {
    pub fn new(config: Arc<AppConfig>, index: Arc<LogSearchIndex>) -> Self {
        Self {
            config,
            index,
            debounce: Duration::from_millis(700),
        }
    }

    pub fn spawn(self) {
        tokio::spawn(async move {
            if let Err(err) = self.run().await {
                error!(error = %err, "watch.stopped");
            }
        });
    }

    async fn run(self) -> anyhow::Result<()> {
        let (event_tx, mut event_rx) = mpsc::channel::<PathBuf>(MAX_PENDING_INDEX_JOBS);
        let (job_tx, job_rx) = mpsc::channel::<IndexJob>(MAX_PENDING_INDEX_JOBS);
        let directories = watched_directories(&self.config);

        let mut watcher = recommended_watcher(move |event: notify::Result<Event>| match event {
            Ok(event) => {
                for path in event.paths {
                    let _ = event_tx.try_send(path);
                }
            }
            Err(err) => warn!(error = %err, "watch.event_failed"),
        })?;

        let mut watched = BTreeSet::new();
        watch_ready_directories(&mut watcher, &mut watched, &directories);

        let worker_index = self.index.clone();
        tokio::spawn(async move { run_scheduler(worker_index, job_rx).await });

        let mut pending: BTreeMap<IndexJob, Instant> = BTreeMap::new();

        loop {
            let next_due = pending.values().next().copied();
            tokio::select! {
                maybe_path = event_rx.recv() => {
                    let Some(path) = maybe_path else { break; };
                    for job in jobs_for_path(&self.config, &path) {
                        if pending.len() >= MAX_PENDING_INDEX_JOBS {
                            break;
                        }
                        pending.insert(job, Instant::now() + self.debounce);
                    }
                }
                _ = async {
                    if let Some(due) = next_due {
                        sleep_until(due).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    let now = Instant::now();
                    let ready: Vec<IndexJob> = pending
                        .iter()
                        .filter_map(|(job, due)| if *due <= now { Some(job.clone()) } else { None })
                        .collect();
                    for job in ready {
                        pending.remove(&job);
                        let _ = job_tx.try_send(job);
                    }
                }
            }
        }

        drop(watcher);
        Ok(())
    }
}

fn watch_ready_directories<W: Watcher>(
    watcher: &mut W,
    watched: &mut BTreeSet<PathBuf>,
    directories: &BTreeSet<PathBuf>,
) {
    watched.retain(|directory| directory.exists());

    for directory in ready_unwatched_directories(directories.iter().cloned(), watched) {
        match watcher.watch(&directory, RecursiveMode::NonRecursive) {
            Ok(()) => {
                watched.insert(directory.clone());
                info!(path = %directory.display(), "watch.dir");
            }
            Err(err) => warn!(
                path = %directory.display(),
                error = %err,
                "watch.dir_failed"
            ),
        }
    }
}

fn ready_unwatched_directories(
    directories: impl IntoIterator<Item = PathBuf>,
    watched: &BTreeSet<PathBuf>,
) -> Vec<PathBuf> {
    directories
        .into_iter()
        .filter(|directory| !watched.contains(directory) && directory.exists())
        .collect()
}

pub fn spawn_initial_indexing(config: Arc<AppConfig>, index: Arc<LogSearchIndex>) {
    tokio::spawn(async move {
        let jobs = reconcile_jobs(&config);
        info!(
            files = config.files.len(),
            jobs = jobs.len(),
            "index.initial_scheduled"
        );
        run_jobs_until_idle(index, jobs).await;
        info!("index.initial_drained");
    });
}

async fn run_scheduler(index: Arc<LogSearchIndex>, mut rx: mpsc::Receiver<IndexJob>) {
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<String>();
    let mut running_sources = HashSet::<String>::new();
    let mut queued = BTreeSet::<IndexJob>::new();

    loop {
        tokio::select! {
            maybe_job = rx.recv() => {
                match maybe_job {
                    Some(job) => {
                        queued.insert(job);
                        dispatch_ready_jobs(&index, &done_tx, &mut queued, &mut running_sources);
                    }
                    None => break,
                }
            }
            maybe_source = done_rx.recv() => {
                if let Some(source_id) = maybe_source {
                    running_sources.remove(&source_id);
                    dispatch_ready_jobs(&index, &done_tx, &mut queued, &mut running_sources);
                }
            }
        }
    }
}

async fn run_jobs_until_idle(index: Arc<LogSearchIndex>, jobs: BTreeSet<IndexJob>) {
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<String>();
    let mut running_sources = HashSet::<String>::new();
    let mut queued = jobs;

    dispatch_ready_jobs(&index, &done_tx, &mut queued, &mut running_sources);

    while !queued.is_empty() || !running_sources.is_empty() {
        let Some(source_id) = done_rx.recv().await else {
            break;
        };
        running_sources.remove(&source_id);
        dispatch_ready_jobs(&index, &done_tx, &mut queued, &mut running_sources);
    }
}

fn dispatch_ready_jobs(
    index: &Arc<LogSearchIndex>,
    done_tx: &mpsc::UnboundedSender<String>,
    queued: &mut BTreeSet<IndexJob>,
    running_sources: &mut HashSet<String>,
) {
    let ready = select_ready_jobs(queued, running_sources, MAX_CONCURRENT_INDEX_JOBS);

    for job in ready {
        queued.remove(&job);
        running_sources.insert(job.source_id().to_string());
        spawn_job(index.clone(), done_tx.clone(), job);
    }
}

fn select_ready_jobs(
    queued: &BTreeSet<IndexJob>,
    running_sources: &HashSet<String>,
    max_concurrent: usize,
) -> Vec<IndexJob> {
    let available_slots = max_concurrent.saturating_sub(running_sources.len());
    if available_slots == 0 {
        return Vec::new();
    }

    queued
        .iter()
        .filter(|job| !running_sources.contains(job.source_id()))
        .take(available_slots)
        .cloned()
        .collect()
}

fn spawn_job(index: Arc<LogSearchIndex>, done_tx: mpsc::UnboundedSender<String>, job: IndexJob) {
    let source_for_done = job.source_id().to_string();
    tokio::spawn(async move {
        run_worker(index, job).await;
        let _ = done_tx.send(source_for_done);
    });
}

async fn run_worker(index: Arc<LogSearchIndex>, job: IndexJob) {
    let started = Instant::now();
    debug!(
        source_id = %job.source_id(),
        kind = job.kind(),
        path = %job.path().display(),
        "index.job_started"
    );
    let result = tokio::task::spawn_blocking(move || match job {
        IndexJob::Hot { source_id, path } => {
            let count = index.sync_file_metadata(&source_id, &path)?;
            let file_size = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            Ok::<_, anyhow::Error>((source_id, path, "hot", count, file_size))
        }
        IndexJob::Compressed {
            source_id,
            path,
            kind,
        } => {
            let count = index.sync_compressed_file_metadata(&source_id, &path, kind.as_str())?;
            let file_size = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            Ok::<_, anyhow::Error>((source_id, path, kind.as_str(), count, file_size))
        }
    })
    .await;

    match result {
        Ok(Ok((source_id, path, kind, count, file_size))) => {
            if count == 0 {
                debug!(
                    %source_id,
                    kind,
                    path = %path.display(),
                    lines = count,
                    file_size,
                    duration_ms = started.elapsed().as_millis(),
                    "index.job_unchanged"
                );
            } else if trace_index_logs_enabled() {
                info!(
                    %source_id,
                    kind,
                    path = %path.display(),
                    lines = count,
                    file_size,
                    duration_ms = started.elapsed().as_millis(),
                    "index.job_completed"
                );
            }
        }
        Ok(Err(err)) => warn!(
            error = %err,
            duration_ms = started.elapsed().as_millis(),
            "index.job_failed"
        ),
        Err(err) => error!(
            error = %err,
            duration_ms = started.elapsed().as_millis(),
            "index.worker_panicked"
        ),
    }
}

pub fn watched_directories(config: &AppConfig) -> BTreeSet<PathBuf> {
    let mut directories = config
        .files
        .iter()
        .filter_map(|file| file.path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>();
    directories.extend(
        config
            .directories
            .iter()
            .map(|directory| directory.path.clone()),
    );
    directories
}

pub fn jobs_for_path(config: &AppConfig, path: &Path) -> Vec<IndexJob> {
    let mut jobs = Vec::new();
    for file in &config.files {
        if path == file.path {
            jobs.push(IndexJob::Hot {
                source_id: file.id.clone(),
                path: path.to_path_buf(),
            });
            continue;
        }
        if compressed_kind(path).is_some_and(|_| is_rotation_candidate_for(path, &file.path)) {
            jobs.push(IndexJob::Compressed {
                source_id: format!("{}:{}", file.id, path.display()),
                path: path.to_path_buf(),
                kind: discovered_kind(path),
            });
        }
    }

    for directory in &config.directories {
        if !path_is_in_directory(path, &directory.path, directory.recursive) {
            continue;
        }
        if !directory_file_matches(directory, path) {
            continue;
        }
        let id = directory_source_id(directory, path);
        let job = match discovered_kind(path) {
            DiscoveredFileKind::Hot => IndexJob::Hot {
                source_id: id,
                path: path.to_path_buf(),
            },
            kind => IndexJob::Compressed {
                source_id: id,
                path: path.to_path_buf(),
                kind,
            },
        };
        if !jobs.contains(&job) {
            jobs.push(job);
        }
    }
    jobs
}

fn path_is_in_directory(path: &Path, directory: &Path, recursive: bool) -> bool {
    if recursive {
        return path.starts_with(directory);
    }

    path.parent().is_some_and(|parent| parent == directory)
}

pub fn reconcile_jobs(config: &AppConfig) -> BTreeSet<IndexJob> {
    discover_files(config)
        .into_iter()
        .filter(|file| file.exists)
        .map(job_for_discovered_file)
        .collect()
}

pub fn discover_files(config: &AppConfig) -> Vec<DiscoveredFile> {
    discover_files_limited(config, MAX_DISCOVERED_FILES)
}

pub fn discover_files_limited(config: &AppConfig, limit: usize) -> Vec<DiscoveredFile> {
    let mut files = BTreeMap::<String, DiscoveredFile>::new();
    for file in &config.files {
        if files.len() >= limit {
            return files.into_values().collect();
        }
        files.insert(
            file.id.clone(),
            DiscoveredFile {
                id: file.id.clone(),
                path: file.path.clone(),
                kind: DiscoveredFileKind::Hot,
                source: DiscoveredFileSource::ConfiguredFile,
                exists: file.path.exists(),
            },
        );
        if let Some(parent) = file.path.parent()
            && parent.exists()
        {
            for entry in WalkDir::new(parent)
                .max_depth(1)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
            {
                if files.len() >= limit {
                    return files.into_values().collect();
                }
                let path = entry.path();
                if compressed_kind(path)
                    .is_some_and(|_| is_rotation_candidate_for(path, &file.path))
                {
                    let kind = compressed_kind(path).unwrap();
                    let id = format!("{}:{}", file.id, path.display());
                    files.insert(
                        id.clone(),
                        DiscoveredFile {
                            id,
                            path: path.to_path_buf(),
                            kind,
                            source: DiscoveredFileSource::ConfiguredFile,
                            exists: true,
                        },
                    );
                }
            }
        }
    }

    for directory in &config.directories {
        if !directory.path.exists() {
            continue;
        }

        let max_depth = if directory.recursive { usize::MAX } else { 1 };
        for entry in WalkDir::new(&directory.path)
            .max_depth(max_depth)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            if files.len() >= limit {
                return files.into_values().collect();
            }
            let path = entry.path();
            if !directory_file_matches(directory, path) {
                continue;
            }
            let id = directory_source_id(directory, path);
            files.insert(
                id.clone(),
                DiscoveredFile {
                    id,
                    path: path.to_path_buf(),
                    kind: discovered_kind(path),
                    source: DiscoveredFileSource::Directory {
                        directory_id: directory.id.clone(),
                    },
                    exists: true,
                },
            );
        }
    }

    files.into_values().collect()
}

fn directory_source_id(directory: &LogDirectoryConfig, path: &Path) -> String {
    let relative_path = path.strip_prefix(&directory.path).unwrap_or(path);
    let relative = relative_path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!("{}:{relative}", directory.id)
}

fn job_for_discovered_file(file: DiscoveredFile) -> IndexJob {
    match file.kind {
        DiscoveredFileKind::Hot => IndexJob::Hot {
            source_id: file.id,
            path: file.path,
        },
        kind @ (DiscoveredFileKind::Gzip
        | DiscoveredFileKind::Zstd
        | DiscoveredFileKind::Bzip2
        | DiscoveredFileKind::Xz) => IndexJob::Compressed {
            source_id: file.id,
            path: file.path,
            kind,
        },
    }
}

fn discovered_kind(path: &Path) -> DiscoveredFileKind {
    compressed_kind(path).unwrap_or(DiscoveredFileKind::Hot)
}

fn directory_file_matches(directory: &LogDirectoryConfig, path: &Path) -> bool {
    let relative = relative_path_for_match(directory, path);
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    let included = directory
        .include
        .iter()
        .any(|pattern| directory_pattern_matches(pattern, file_name, &relative));
    let excluded = directory
        .exclude
        .iter()
        .any(|pattern| directory_pattern_matches(pattern, file_name, &relative));

    included && !excluded
}

fn is_rotation_candidate_for(path: &Path, hot_path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(hot_name) = hot_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    file_name.starts_with(hot_name)
}

fn compressed_kind(path: &Path) -> Option<DiscoveredFileKind> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("gz") => Some(DiscoveredFileKind::Gzip),
        Some("zst") => Some(DiscoveredFileKind::Zstd),
        Some("bz2") => Some(DiscoveredFileKind::Bzip2),
        Some("xz") => Some(DiscoveredFileKind::Xz),
        _ => None,
    }
}

fn file_name_matches(file_name: &str, pattern: &str) -> bool {
    glob_matches(pattern, file_name)
}

fn relative_path_for_match(directory: &LogDirectoryConfig, path: &Path) -> String {
    path.strip_prefix(&directory.path)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn directory_pattern_matches(pattern: &str, file_name: &str, relative_path: &str) -> bool {
    if pattern.contains('/') || pattern.contains('\\') {
        glob_matches(&normalize_pattern(pattern), relative_path)
    } else {
        file_name_matches(file_name, pattern)
    }
}

fn normalize_pattern(pattern: &str) -> String {
    pattern.replace('\\', "/")
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let mut memo = vec![vec![None; value.len() + 1]; pattern.len() + 1];
    glob_match_tokens(&pattern, &value, 0, 0, &mut memo)
}

fn glob_match_tokens(
    pattern: &[char],
    value: &[char],
    pattern_idx: usize,
    value_idx: usize,
    memo: &mut [Vec<Option<bool>>],
) -> bool {
    if let Some(cached) = memo[pattern_idx][value_idx] {
        return cached;
    }

    let matched = if pattern_idx == pattern.len() {
        value_idx == value.len()
    } else if pattern[pattern_idx] == '*'
        && pattern.get(pattern_idx + 1) == Some(&'*')
        && pattern.get(pattern_idx + 2) == Some(&'/')
    {
        glob_match_tokens(pattern, value, pattern_idx + 3, value_idx, memo)
            || (value_idx < value.len()
                && glob_match_tokens(pattern, value, pattern_idx, value_idx + 1, memo))
    } else if pattern[pattern_idx] == '*' && pattern.get(pattern_idx + 1) == Some(&'*') {
        glob_match_tokens(pattern, value, pattern_idx + 2, value_idx, memo)
            || (value_idx < value.len()
                && glob_match_tokens(pattern, value, pattern_idx, value_idx + 1, memo))
    } else {
        match pattern[pattern_idx] {
            '*' => {
                glob_match_tokens(pattern, value, pattern_idx + 1, value_idx, memo)
                    || (value_idx < value.len()
                        && value[value_idx] != '/'
                        && glob_match_tokens(pattern, value, pattern_idx, value_idx + 1, memo))
            }
            '?' => {
                value_idx < value.len()
                    && value[value_idx] != '/'
                    && glob_match_tokens(pattern, value, pattern_idx + 1, value_idx + 1, memo)
            }
            literal => {
                value_idx < value.len()
                    && value[value_idx] == literal
                    && glob_match_tokens(pattern, value, pattern_idx + 1, value_idx + 1, memo)
            }
        }
    };
    memo[pattern_idx][value_idx] = Some(matched);
    matched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, IndexConfig, LogDirectoryConfig, LogFileConfig, ServerConfig};

    fn config(path: PathBuf) -> AppConfig {
        AppConfig {
            server: ServerConfig {
                addr: "127.0.0.1:0".to_string(),
            },
            index: IndexConfig {
                dir: PathBuf::from("/tmp/index"),
            },
            files: vec![LogFileConfig {
                id: "app".to_string(),
                path,
            }],
            directories: Vec::new(),
        }
    }

    fn directory_config(path: PathBuf) -> AppConfig {
        AppConfig {
            server: ServerConfig {
                addr: "127.0.0.1:0".to_string(),
            },
            index: IndexConfig {
                dir: PathBuf::from("/tmp/index"),
            },
            files: Vec::new(),
            directories: vec![LogDirectoryConfig {
                id: "release".to_string(),
                path,
                include: vec!["*.log".to_string(), "*.gz".to_string()],
                exclude: Vec::new(),
                recursive: false,
            }],
        }
    }

    fn mkdir(path: &Path) {
        std::fs::create_dir(path).unwrap();
    }

    #[test]
    fn maps_hot_file_event_to_hot_job() {
        let cfg = config(PathBuf::from("/var/log/app.log"));

        assert_eq!(
            jobs_for_path(&cfg, Path::new("/var/log/app.log")),
            vec![IndexJob::Hot {
                source_id: "app".to_string(),
                path: PathBuf::from("/var/log/app.log")
            }]
        );
    }

    #[test]
    fn maps_directory_file_event_without_discovering_entire_directory() {
        let dir = tempfile::tempdir().unwrap();
        let hot_path = dir.path().join("app.log");
        let ignored_path = dir.path().join("notes.txt");
        let cfg = directory_config(dir.path().to_path_buf());

        assert_eq!(
            jobs_for_path(&cfg, &hot_path),
            vec![IndexJob::Hot {
                source_id: "release:app.log".to_string(),
                path: hot_path,
            }]
        );
        assert!(jobs_for_path(&cfg, &ignored_path).is_empty());
    }

    #[test]
    fn configured_file_still_discovers_matching_gzip_rotations() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("app.log");
        let gzip_path = dir.path().join("app.log.1.gz");
        std::fs::write(&log_path, "hot\n").unwrap();
        std::fs::write(&gzip_path, "compressed placeholder\n").unwrap();
        let cfg = config(log_path);

        assert!(reconcile_jobs(&cfg).contains(&IndexJob::Compressed {
            source_id: format!("app:{}", gzip_path.display()),
            path: gzip_path,
            kind: DiscoveredFileKind::Gzip,
        }));
    }

    #[test]
    fn reconcile_skips_hot_job_when_parent_directory_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing_path = dir.path().join("release-gone").join("app.log");
        let cfg = config(missing_path);

        assert!(reconcile_jobs(&cfg).is_empty());
    }

    #[test]
    fn directory_config_discovers_hot_and_compressed_files() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("app.log");
        let gzip_path = dir.path().join("app.log.1.gz");
        let zstd_path = dir.path().join("app.log.2.zst");
        let bzip2_path = dir.path().join("app.log.3.bz2");
        let xz_path = dir.path().join("app.log.4.xz");
        let ignored_path = dir.path().join("notes.txt");
        std::fs::write(&log_path, "hot\n").unwrap();
        std::fs::write(&gzip_path, "compressed placeholder\n").unwrap();
        std::fs::write(&zstd_path, "compressed placeholder\n").unwrap();
        std::fs::write(&bzip2_path, "compressed placeholder\n").unwrap();
        std::fs::write(&xz_path, "compressed placeholder\n").unwrap();
        std::fs::write(&ignored_path, "ignore\n").unwrap();
        let mut cfg = directory_config(dir.path().to_path_buf());
        cfg.directories[0].include = vec![
            "*.log".to_string(),
            "*.gz".to_string(),
            "*.zst".to_string(),
            "*.bz2".to_string(),
            "*.xz".to_string(),
        ];

        let discovered = discover_files(&cfg);

        assert_eq!(
            discovered,
            vec![
                DiscoveredFile {
                    id: "release:app.log".to_string(),
                    path: log_path.clone(),
                    kind: DiscoveredFileKind::Hot,
                    source: DiscoveredFileSource::Directory {
                        directory_id: "release".to_string()
                    },
                    exists: true,
                },
                DiscoveredFile {
                    id: "release:app.log.1.gz".to_string(),
                    path: gzip_path.clone(),
                    kind: DiscoveredFileKind::Gzip,
                    source: DiscoveredFileSource::Directory {
                        directory_id: "release".to_string()
                    },
                    exists: true,
                },
                DiscoveredFile {
                    id: "release:app.log.2.zst".to_string(),
                    path: zstd_path.clone(),
                    kind: DiscoveredFileKind::Zstd,
                    source: DiscoveredFileSource::Directory {
                        directory_id: "release".to_string()
                    },
                    exists: true,
                },
                DiscoveredFile {
                    id: "release:app.log.3.bz2".to_string(),
                    path: bzip2_path.clone(),
                    kind: DiscoveredFileKind::Bzip2,
                    source: DiscoveredFileSource::Directory {
                        directory_id: "release".to_string()
                    },
                    exists: true,
                },
                DiscoveredFile {
                    id: "release:app.log.4.xz".to_string(),
                    path: xz_path.clone(),
                    kind: DiscoveredFileKind::Xz,
                    source: DiscoveredFileSource::Directory {
                        directory_id: "release".to_string()
                    },
                    exists: true,
                },
            ]
        );
        assert_eq!(
            reconcile_jobs(&cfg),
            BTreeSet::from([
                IndexJob::Hot {
                    source_id: "release:app.log".to_string(),
                    path: log_path,
                },
                IndexJob::Compressed {
                    source_id: "release:app.log.1.gz".to_string(),
                    path: gzip_path,
                    kind: DiscoveredFileKind::Gzip,
                },
                IndexJob::Compressed {
                    source_id: "release:app.log.2.zst".to_string(),
                    path: zstd_path,
                    kind: DiscoveredFileKind::Zstd,
                },
                IndexJob::Compressed {
                    source_id: "release:app.log.3.bz2".to_string(),
                    path: bzip2_path,
                    kind: DiscoveredFileKind::Bzip2,
                },
                IndexJob::Compressed {
                    source_id: "release:app.log.4.xz".to_string(),
                    path: xz_path,
                    kind: DiscoveredFileKind::Xz,
                },
            ])
        );
    }

    #[test]
    fn recursive_directory_config_keeps_duplicate_file_names_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let first_day = dir.path().join("2026-05-02");
        let second_day = dir.path().join("2026-05-03");
        mkdir(&first_day);
        mkdir(&second_day);
        let first_error = first_day.join("error.log");
        let first_info = first_day.join("info.log");
        let second_error = second_day.join("error.log");
        let second_info = second_day.join("info.log");
        std::fs::write(&first_error, "first error\n").unwrap();
        std::fs::write(&first_info, "first info\n").unwrap();
        std::fs::write(&second_error, "second error\n").unwrap();
        std::fs::write(&second_info, "second info\n").unwrap();
        let mut cfg = directory_config(dir.path().to_path_buf());
        cfg.directories[0].recursive = true;
        cfg.directories[0].include = vec!["*.log".to_string()];

        let discovered = discover_files(&cfg);

        assert_eq!(
            discovered,
            vec![
                DiscoveredFile {
                    id: "release:2026-05-02/error.log".to_string(),
                    path: first_error.clone(),
                    kind: DiscoveredFileKind::Hot,
                    source: DiscoveredFileSource::Directory {
                        directory_id: "release".to_string()
                    },
                    exists: true,
                },
                DiscoveredFile {
                    id: "release:2026-05-02/info.log".to_string(),
                    path: first_info,
                    kind: DiscoveredFileKind::Hot,
                    source: DiscoveredFileSource::Directory {
                        directory_id: "release".to_string()
                    },
                    exists: true,
                },
                DiscoveredFile {
                    id: "release:2026-05-03/error.log".to_string(),
                    path: second_error.clone(),
                    kind: DiscoveredFileKind::Hot,
                    source: DiscoveredFileSource::Directory {
                        directory_id: "release".to_string()
                    },
                    exists: true,
                },
                DiscoveredFile {
                    id: "release:2026-05-03/info.log".to_string(),
                    path: second_info,
                    kind: DiscoveredFileKind::Hot,
                    source: DiscoveredFileSource::Directory {
                        directory_id: "release".to_string()
                    },
                    exists: true,
                },
            ]
        );
        assert_eq!(
            jobs_for_path(&cfg, &first_error),
            vec![IndexJob::Hot {
                source_id: "release:2026-05-02/error.log".to_string(),
                path: first_error,
            }]
        );
        assert_eq!(
            jobs_for_path(&cfg, &second_error),
            vec![IndexJob::Hot {
                source_id: "release:2026-05-03/error.log".to_string(),
                path: second_error,
            }]
        );
    }

    #[test]
    fn recursive_directory_include_can_filter_by_relative_path_glob() {
        let dir = tempfile::tempdir().unwrap();
        let may_day = dir.path().join("2026-05-02");
        let june_day = dir.path().join("2026-06-03");
        mkdir(&may_day);
        mkdir(&june_day);
        let may_error = may_day.join("error.log");
        let may_info = may_day.join("info.log");
        let june_error = june_day.join("error.log");
        std::fs::write(&may_error, "may error\n").unwrap();
        std::fs::write(&may_info, "may info\n").unwrap();
        std::fs::write(&june_error, "june error\n").unwrap();
        let mut cfg = directory_config(dir.path().to_path_buf());
        cfg.directories[0].recursive = true;
        cfg.directories[0].include = vec!["2026-05-*/*.log".to_string()];

        let discovered = discover_files(&cfg);

        assert_eq!(
            discovered
                .iter()
                .map(|file| file.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "release:2026-05-02/error.log",
                "release:2026-05-02/info.log",
            ]
        );
        assert_eq!(
            jobs_for_path(&cfg, &may_error),
            vec![IndexJob::Hot {
                source_id: "release:2026-05-02/error.log".to_string(),
                path: may_error,
            }]
        );
        assert!(jobs_for_path(&cfg, &june_error).is_empty());
    }

    #[test]
    fn directory_exclude_filters_matching_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026-05-02");
        mkdir(&day);
        let app_log = day.join("app.log");
        let debug_log = day.join("debug.log");
        std::fs::write(&app_log, "app\n").unwrap();
        std::fs::write(&debug_log, "debug\n").unwrap();
        let mut cfg = directory_config(dir.path().to_path_buf());
        cfg.directories[0].recursive = true;
        cfg.directories[0].include = vec!["2026-05-*/*.log".to_string()];
        cfg.directories[0].exclude = vec!["**/debug.log".to_string()];

        let discovered = discover_files(&cfg);

        assert_eq!(
            discovered
                .iter()
                .map(|file| file.id.as_str())
                .collect::<Vec<_>>(),
            vec!["release:2026-05-02/app.log"]
        );
        assert!(jobs_for_path(&cfg, &debug_log).is_empty());
    }

    #[test]
    fn double_star_directory_pattern_matches_nested_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("2026-05").join("02").join("service-a");
        std::fs::create_dir_all(&nested).unwrap();
        let app_log = nested.join("app.log");
        let debug_log = nested.join("debug.log");
        std::fs::write(&app_log, "app\n").unwrap();
        std::fs::write(&debug_log, "debug\n").unwrap();
        let mut cfg = directory_config(dir.path().to_path_buf());
        cfg.directories[0].recursive = true;
        cfg.directories[0].include = vec!["2026-05/**/*.log".to_string()];
        cfg.directories[0].exclude = vec!["**/debug.log".to_string()];

        let discovered = discover_files(&cfg);

        assert_eq!(
            discovered
                .iter()
                .map(|file| file.id.as_str())
                .collect::<Vec<_>>(),
            vec!["release:2026-05/02/service-a/app.log"]
        );
        assert!(jobs_for_path(&cfg, &debug_log).is_empty());
    }

    #[test]
    fn discover_files_limited_stops_after_limit() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..10 {
            std::fs::write(dir.path().join(format!("app-{index}.log")), "line\n").unwrap();
        }
        let cfg = directory_config(dir.path().to_path_buf());

        let discovered = discover_files_limited(&cfg, 3);

        assert_eq!(discovered.len(), 3);
    }

    #[test]
    fn ready_unwatched_directories_become_watch_candidates_after_recreate() {
        let dir = tempfile::tempdir().unwrap();
        let recreated = dir.path().join("release");
        let mut watched = BTreeSet::new();

        assert!(ready_unwatched_directories([recreated.clone()], &watched).is_empty());

        std::fs::create_dir(&recreated).unwrap();
        let candidates = ready_unwatched_directories([recreated.clone()], &watched);

        assert_eq!(candidates, vec![recreated.clone()]);

        watched.insert(recreated.clone());
        assert!(ready_unwatched_directories([recreated], &watched).is_empty());
    }

    #[test]
    fn source_id_is_stable_for_scheduler() {
        let job = IndexJob::Compressed {
            source_id: "app".to_string(),
            path: PathBuf::from("/var/log/app.log.1.gz"),
            kind: DiscoveredFileKind::Gzip,
        };

        assert_eq!(job.source_id(), "app");
    }

    #[test]
    fn dispatch_selection_respects_global_concurrency_limit() {
        let queued = BTreeSet::from([
            IndexJob::Hot {
                source_id: "app".to_string(),
                path: PathBuf::from("/var/log/app.log"),
            },
            IndexJob::Hot {
                source_id: "worker".to_string(),
                path: PathBuf::from("/var/log/worker.log"),
            },
            IndexJob::Hot {
                source_id: "api".to_string(),
                path: PathBuf::from("/var/log/api.log"),
            },
        ]);
        let running_sources = HashSet::from(["app".to_string()]);

        let ready = select_ready_jobs(&queued, &running_sources, 2);

        assert_eq!(ready.len(), 1);
        assert!(!ready.iter().any(|job| job.source_id() == "app"));
    }
}
