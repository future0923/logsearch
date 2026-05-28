use crate::{
    config::AppConfig,
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
    time::{Instant, interval_at, sleep_until},
};
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndexJob {
    Hot { source_id: String, path: PathBuf },
    Gzip { source_id: String, path: PathBuf },
}

impl IndexJob {
    fn source_id(&self) -> &str {
        match self {
            IndexJob::Hot { source_id, .. } | IndexJob::Gzip { source_id, .. } => source_id,
        }
    }

    fn path(&self) -> &Path {
        match self {
            IndexJob::Hot { path, .. } | IndexJob::Gzip { path, .. } => path,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            IndexJob::Hot { .. } => "hot",
            IndexJob::Gzip { .. } => "gzip",
        }
    }
}

pub struct WatchService {
    config: Arc<AppConfig>,
    index: Arc<LogSearchIndex>,
    debounce: Duration,
    reconcile_interval: Duration,
}

impl WatchService {
    pub fn new(config: Arc<AppConfig>, index: Arc<LogSearchIndex>) -> Self {
        Self {
            config,
            index,
            debounce: Duration::from_millis(700),
            reconcile_interval: Duration::from_secs(15),
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
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<PathBuf>();
        let (job_tx, job_rx) = mpsc::unbounded_channel::<IndexJob>();
        let directories = watched_directories(&self.config);

        let mut watcher = recommended_watcher(move |event: notify::Result<Event>| match event {
            Ok(event) => {
                for path in event.paths {
                    let _ = event_tx.send(path);
                }
            }
            Err(err) => warn!(error = %err, "watch.event_failed"),
        })?;

        for directory in &directories {
            watcher.watch(directory, RecursiveMode::NonRecursive)?;
            info!(path = %directory.display(), "watch.dir");
        }

        let worker_index = self.index.clone();
        tokio::spawn(async move { run_scheduler(worker_index, job_rx).await });

        let mut pending: BTreeMap<IndexJob, Instant> = BTreeMap::new();
        let mut reconcile = interval_at(
            Instant::now() + self.reconcile_interval,
            self.reconcile_interval,
        );

        loop {
            let next_due = pending.values().next().copied();
            tokio::select! {
                maybe_path = event_rx.recv() => {
                    let Some(path) = maybe_path else { break; };
                    for job in jobs_for_path(&self.config, &path) {
                        pending.insert(job, Instant::now() + self.debounce);
                    }
                }
                _ = reconcile.tick() => {
                    for job in reconcile_jobs(&self.config) {
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
                        let _ = job_tx.send(job);
                    }
                }
            }
        }

        drop(watcher);
        Ok(())
    }
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

async fn run_scheduler(index: Arc<LogSearchIndex>, mut rx: mpsc::UnboundedReceiver<IndexJob>) {
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
    let ready: Vec<IndexJob> = queued
        .iter()
        .filter(|job| !running_sources.contains(job.source_id()))
        .cloned()
        .collect();

    for job in ready {
        queued.remove(&job);
        running_sources.insert(job.source_id().to_string());
        spawn_job(index.clone(), done_tx.clone(), job);
    }
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
            let count = index.sync_file(&source_id, &path)?;
            let file_size = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            Ok::<_, anyhow::Error>((source_id, path, "hot", count, file_size))
        }
        IndexJob::Gzip { source_id, path } => {
            let count = index.sync_gzip_file(&source_id, &path)?;
            let file_size = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            Ok::<_, anyhow::Error>((source_id, path, "gzip", count, file_size))
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
    config
        .files
        .iter()
        .filter_map(|file| file.path.parent().map(Path::to_path_buf))
        .collect()
}

pub fn jobs_for_path(config: &AppConfig, path: &Path) -> Vec<IndexJob> {
    let mut jobs = Vec::new();
    for file in &config.files {
        if path == file.path {
            jobs.push(IndexJob::Hot {
                source_id: file.id.clone(),
                path: file.path.clone(),
            });
        }

        if is_gzip_candidate_for(path, &file.path) {
            jobs.push(IndexJob::Gzip {
                source_id: file.id.clone(),
                path: path.to_path_buf(),
            });
        }
    }
    jobs
}

pub fn reconcile_jobs(config: &AppConfig) -> BTreeSet<IndexJob> {
    let mut jobs = BTreeSet::new();
    for file in &config.files {
        jobs.insert(IndexJob::Hot {
            source_id: file.id.clone(),
            path: file.path.clone(),
        });

        if let Some(parent) = file.path.parent() {
            for entry in WalkDir::new(parent)
                .max_depth(1)
                .into_iter()
                .filter_map(Result::ok)
            {
                let path = entry.path();
                if is_gzip_candidate_for(path, &file.path) {
                    jobs.insert(IndexJob::Gzip {
                        source_id: file.id.clone(),
                        path: path.to_path_buf(),
                    });
                }
            }
        }
    }
    jobs
}

fn is_gzip_candidate_for(path: &Path, hot_path: &Path) -> bool {
    if path.extension().and_then(|ext| ext.to_str()) != Some("gz") {
        return false;
    }

    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(hot_name) = hot_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    file_name.starts_with(hot_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, IndexConfig, LogFileConfig, ServerConfig};

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
        }
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
    fn maps_gzip_rotation_to_gzip_job() {
        let cfg = config(PathBuf::from("/var/log/app.log"));

        assert_eq!(
            jobs_for_path(&cfg, Path::new("/var/log/app.log.1.gz")),
            vec![IndexJob::Gzip {
                source_id: "app".to_string(),
                path: PathBuf::from("/var/log/app.log.1.gz")
            }]
        );
    }

    #[test]
    fn source_id_is_stable_for_scheduler() {
        let job = IndexJob::Gzip {
            source_id: "app".to_string(),
            path: PathBuf::from("/var/log/app.log.1.gz"),
        };

        assert_eq!(job.source_id(), "app");
    }
}
