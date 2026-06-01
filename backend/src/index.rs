use crate::{
    scanner::{
        LogLine, read_bzip2_context_lines, read_context_lines, read_gzip_context_lines,
        read_xz_context_lines, read_zstd_context_lines, scan_bzip2_lines, scan_gzip_lines,
        scan_lines, scan_lines_from, scan_xz_lines, scan_zstd_lines,
    },
    search::{
        AroundRequest, AroundResponse, ContextLine, SearchHit, SearchRequest, build_regex,
        contains_whole_word,
    },
    state::{
        FileKind, FileState, IndexState, can_increment, compressed_fingerprint, fingerprint,
        generation_id,
    },
};
use anyhow::{Context, anyhow};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{ErrorKind, IsTerminal, Write},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
    time::{Duration, Instant},
};
use tracing::{debug, info};

static CLI_PROGRESS_STARTED: OnceLock<Instant> = OnceLock::new();

pub(crate) fn trace_index_logs_enabled() -> bool {
    trace_index_logs_enabled_from_env(|key| std::env::var(key).ok())
}

fn trace_index_logs_enabled_from_env(get_env: impl Fn(&str) -> Option<String>) -> bool {
    matches!(
        get_env("LOG_SEARCH_TRACE_INDEX").as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

#[derive(Clone)]
pub struct LogSearchIndex {
    state_dir: Option<PathBuf>,
    status: Arc<RwLock<BTreeMap<String, IndexStatus>>>,
    memory_lines: Arc<RwLock<BTreeMap<String, Vec<LogLine>>>>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub source_id: String,
    pub path: String,
    pub kind: String,
    pub phase: String,
    pub last_indexed_lines: usize,
    pub indexed_offset: u64,
    pub file_size: u64,
    pub lag_bytes: u64,
    pub progress_percent: f64,
    pub elapsed_ms: u128,
    pub updated_unix_ms: u128,
}

struct IndexProgress<'a> {
    source_id: &'a str,
    path: &'a Path,
    kind: &'a str,
    phase: &'a str,
    last_indexed_lines: usize,
    indexed_offset: u64,
    file_size: u64,
    elapsed_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexLayout {
    pub state_dir: PathBuf,
}

impl IndexLayout {
    pub fn from_config_dir(config_dir: &Path) -> Self {
        Self {
            state_dir: config_dir.to_path_buf(),
        }
    }

    fn prepare(&self) -> anyhow::Result<()> {
        fs::create_dir_all(&self.state_dir)?;
        Ok(())
    }
}

pub fn rebuild_index_storage(config_dir: &Path) -> anyhow::Result<()> {
    let layout = IndexLayout::from_config_dir(config_dir);
    if layout.state_dir.exists() {
        fs::remove_dir_all(&layout.state_dir)?;
    }
    fs::create_dir_all(&layout.state_dir)?;
    Ok(())
}

pub fn set_cli_progress_started(started: Instant) {
    let _ = CLI_PROGRESS_STARTED.set(started);
}

impl LogSearchIndex {
    pub fn open_or_create(path: &Path) -> anyhow::Result<Self> {
        let layout = IndexLayout::from_config_dir(path);
        layout.prepare()?;
        info!(
            state_dir = %layout.state_dir.display(),
            "index.ready"
        );

        Ok(Self {
            state_dir: Some(layout.state_dir),
            status: Arc::new(RwLock::new(BTreeMap::new())),
            memory_lines: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    pub fn create_in_ram() -> anyhow::Result<Self> {
        Ok(Self {
            state_dir: None,
            status: Arc::new(RwLock::new(BTreeMap::new())),
            memory_lines: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    pub fn status_snapshot(&self) -> Vec<IndexStatus> {
        self.status
            .read()
            .map(|status| status.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn sync_file(&self, file_id: &str, path: &Path) -> anyhow::Result<usize> {
        let started = Instant::now();
        let state_dir = self
            .state_dir
            .as_ref()
            .ok_or_else(|| anyhow!("persistent state directory is required"))?;
        let mut state = IndexState::load(state_dir)?;
        let current = match fingerprint(path) {
            Ok(current) => current,
            Err(err) if is_not_found_error(&err) => {
                state.files.remove(file_id);
                state.save(state_dir)?;
                self.record_status(IndexProgress {
                    source_id: file_id,
                    path,
                    kind: "hot",
                    phase: "missing",
                    last_indexed_lines: 0,
                    indexed_offset: 0,
                    file_size: 0,
                    elapsed_ms: started.elapsed().as_millis(),
                });
                return Ok(0);
            }
            Err(err) => return Err(err),
        };
        let file_size = current.len;
        let previous = state.files.get(file_id);

        if let Some(previous) = previous
            && previous.path == path
            && can_increment(previous, &current)
            && previous.indexed_offset == current.len
        {
            self.record_status(IndexProgress {
                source_id: file_id,
                path,
                kind: "hot",
                phase: "idle",
                last_indexed_lines: 0,
                indexed_offset: previous.indexed_offset,
                file_size,
                elapsed_ms: started.elapsed().as_millis(),
            });
            debug!(
                file_id,
                path = %path.display(),
                indexed_offset = previous.indexed_offset,
                file_size,
                duration_ms = started.elapsed().as_millis(),
                "hot log unchanged"
            );
            return Ok(0);
        }

        let (start_offset, start_line_no, replace) = match previous {
            Some(previous) if previous.path == path && can_increment(previous, &current) => {
                (previous.indexed_offset, previous.indexed_line_no + 1, false)
            }
            _ => (0, 1, true),
        };
        debug!(
            file_id,
            path = %path.display(),
            start_offset,
            start_line_no,
            replace,
            file_size,
            "syncing hot log"
        );

        let (indexed_offset, indexed_line_no, count) = self.index_file_range(
            file_id,
            path,
            start_offset,
            start_line_no,
            replace,
            file_size,
            started,
        )?;

        state.files.insert(
            file_id.to_string(),
            FileState {
                path: path.to_path_buf(),
                source_id: file_id.to_string(),
                generation_id: generation_id(file_id, &current),
                kind: FileKind::Hot,
                fingerprint: current,
                indexed_offset,
                indexed_line_no,
            },
        );
        state.save(state_dir)?;
        self.record_status(IndexProgress {
            source_id: file_id,
            path,
            kind: "hot",
            phase: "complete",
            last_indexed_lines: count,
            indexed_offset,
            file_size,
            elapsed_ms: started.elapsed().as_millis(),
        });
        if trace_index_logs_enabled() {
            info!(
                file_id,
                path = %path.display(),
                indexed_lines = count,
                indexed_offset,
                file_size,
                lag_bytes = file_size.saturating_sub(indexed_offset),
                replace,
                duration_ms = started.elapsed().as_millis(),
                "index.hot_synced"
            );
        }

        Ok(count)
    }

    pub fn sync_compressed_file(
        &self,
        source_id: &str,
        path: &Path,
        kind: &str,
    ) -> anyhow::Result<usize> {
        let started = Instant::now();
        let state_dir = self
            .state_dir
            .as_ref()
            .ok_or_else(|| anyhow!("persistent state directory is required"))?;
        let mut state = IndexState::load(state_dir)?;
        let state_key = format!("{source_id}:{kind}:{}", path.display());
        let current = match compressed_fingerprint(path) {
            Ok(current) => current,
            Err(err) if is_not_found_error(&err) => {
                state.files.remove(&state_key);
                state.save(state_dir)?;
                self.record_status(IndexProgress {
                    source_id,
                    path,
                    kind,
                    phase: "missing",
                    last_indexed_lines: 0,
                    indexed_offset: 0,
                    file_size: 0,
                    elapsed_ms: started.elapsed().as_millis(),
                });
                return Ok(0);
            }
            Err(err) => return Err(err),
        };
        let file_size = current.len;
        let generation_id = generation_id(source_id, &current);

        if let Some(previous) = state.files.get(&state_key)
            && previous.fingerprint == current
        {
            debug!(
                source_id,
                path = %path.display(),
                file_size,
                kind,
                "compressed log unchanged"
            );
            return Ok(0);
        }

        let mut count = 0_usize;
        let indexed_line_no = scan_compressed_lines(path, kind, |_| {
            count += 1;
            Ok(())
        })?;

        state.files.insert(
            state_key,
            FileState {
                path: path.to_path_buf(),
                source_id: source_id.to_string(),
                generation_id,
                kind: file_kind_for_compressed(kind),
                fingerprint: current,
                indexed_offset: indexed_line_no,
                indexed_line_no,
            },
        );
        state.save(state_dir)?;
        self.record_status(IndexProgress {
            source_id,
            path,
            kind,
            phase: "complete",
            last_indexed_lines: count,
            indexed_offset: indexed_line_no,
            file_size,
            elapsed_ms: started.elapsed().as_millis(),
        });
        if trace_index_logs_enabled() {
            info!(
                source_id,
                path = %path.display(),
                indexed_lines = count,
                file_size,
                duration_ms = started.elapsed().as_millis(),
                kind,
                "index.compressed_synced"
            );
        }

        Ok(count)
    }

    fn record_status(&self, progress: IndexProgress<'_>) {
        let percent = if progress.file_size == 0 {
            100.0
        } else {
            (progress.indexed_offset as f64 / progress.file_size as f64 * 100.0).min(100.0)
        };
        let status = IndexStatus {
            source_id: progress.source_id.to_string(),
            path: progress.path.to_string_lossy().to_string(),
            kind: progress.kind.to_string(),
            phase: progress.phase.to_string(),
            last_indexed_lines: progress.last_indexed_lines,
            indexed_offset: progress.indexed_offset,
            file_size: progress.file_size,
            lag_bytes: progress.file_size.saturating_sub(progress.indexed_offset),
            progress_percent: percent,
            elapsed_ms: progress.elapsed_ms,
            updated_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        };

        if let Ok(mut statuses) = self.status.write() {
            statuses.insert(
                format!(
                    "{}:{}:{}",
                    progress.source_id,
                    progress.kind,
                    progress.path.display()
                ),
                status,
            );
        }
    }

    pub fn replace_file(&self, file_id: &str, path: &Path) -> anyhow::Result<usize> {
        if self.state_dir.is_none() {
            let lines = scan_lines(path)?;
            let count = lines.len();
            self.index_lines(file_id, &lines)?;
            return Ok(count);
        }

        let file_size = fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let (_, _, count) =
            self.index_file_range(file_id, path, 0, 1, true, file_size, Instant::now())?;
        Ok(count)
    }

    fn index_file_range(
        &self,
        file_id: &str,
        path: &Path,
        start_offset: u64,
        start_line_no: u64,
        _replace: bool,
        file_size: u64,
        started: Instant,
    ) -> anyhow::Result<(u64, u64, usize)> {
        let mut count = 0_usize;
        let mut last_progress = Instant::now();
        let mut last_progress_percent = if start_offset == 0 { 0.0 } else { -10.0 };
        self.record_status(IndexProgress {
            source_id: file_id,
            path,
            kind: "hot",
            phase: "indexing",
            last_indexed_lines: 0,
            indexed_offset: start_offset,
            file_size,
            elapsed_ms: started.elapsed().as_millis(),
        });
        let (offset, line_no) = scan_lines_from(path, start_offset, start_line_no, |line| {
            let current_offset = line.offset + line.content.len() as u64 + 1;
            count += 1;
            let progress_percent = if file_size == 0 {
                100.0
            } else {
                (current_offset.min(file_size) as f64 / file_size as f64 * 100.0).min(100.0)
            };
            let should_report_progress = last_progress.elapsed().as_secs() >= 1
                || progress_percent - last_progress_percent >= 10.0;
            if should_report_progress {
                self.record_status(IndexProgress {
                    source_id: file_id,
                    path,
                    kind: "hot",
                    phase: "indexing",
                    last_indexed_lines: count,
                    indexed_offset: current_offset.min(file_size),
                    file_size,
                    elapsed_ms: started.elapsed().as_millis(),
                });
                debug!(
                    file_id,
                    path = %path.display(),
                    indexed_lines = count,
                    indexed_offset = current_offset.min(file_size),
                    file_size,
                    progress_percent,
                    elapsed_ms = started.elapsed().as_millis(),
                    "hot log sync progress"
                );
                if std::env::args().any(|arg| arg == "rebuild-index") {
                    print_cli_progress(
                        file_id,
                        "hot",
                        count,
                        current_offset.min(file_size),
                        file_size,
                        progress_percent,
                        started.elapsed().as_millis(),
                    );
                }
                last_progress = Instant::now();
                last_progress_percent = progress_percent;
            }
            Ok(())
        })?;

        Ok((offset, line_no, count))
    }

    pub fn index_lines(&self, file_id: &str, lines: &[LogLine]) -> anyhow::Result<()> {
        self.memory_lines
            .write()
            .map_err(|_| anyhow!("memory line index lock poisoned"))?
            .insert(file_id.to_string(), lines.to_vec());
        Ok(())
    }

    pub fn search(
        &self,
        req: &SearchRequest,
    ) -> anyhow::Result<(Vec<SearchHit>, u128, bool, bool, Option<String>)> {
        let started = Instant::now();
        let limit = req.limit.clamp(1, 1_000);
        let final_regex = build_regex(req)?;
        let matcher = LineMatcher::new(req, final_regex);
        self.search_by_scanning(req, &matcher, limit, started)
    }

    fn search_by_scanning(
        &self,
        req: &SearchRequest,
        matcher: &LineMatcher,
        limit: usize,
        started: Instant,
    ) -> anyhow::Result<(Vec<SearchHit>, u128, bool, bool, Option<String>)> {
        let mut hits = Vec::with_capacity(limit);
        let cursor = decode_search_cursor(req.cursor.as_deref())?;
        let sources = self.search_sources()?;
        let selected_file_ids = req
            .file_ids
            .iter()
            .filter_map(|id| {
                let id = id.trim();
                if id.is_empty() { None } else { Some(id) }
            })
            .collect::<BTreeSet<_>>();
        let mut has_next = false;
        let mut next_cursor = None;

        for (source_idx, source) in sources.iter().enumerate().skip(cursor.source_idx) {
            if !selected_file_ids.is_empty() && !selected_file_ids.contains(source.file_id.as_str())
            {
                continue;
            }

            let start_offset = if source_idx == cursor.source_idx {
                cursor.offset
            } else {
                0
            };
            let start_line_no = if source_idx == cursor.source_idx {
                cursor.line_no.max(1)
            } else {
                u64::MAX
            };

            let source_complete = self.scan_source_for_hits(
                source,
                source_idx,
                start_offset,
                start_line_no,
                req,
                matcher,
                limit,
                &mut hits,
                &mut has_next,
                &mut next_cursor,
            )?;

            if !source_complete || has_next {
                break;
            }
        }

        let truncated = has_next;
        let elapsed_ms = started.elapsed().as_millis();
        info!(
            query = %req.query,
            regex = req.regex,
            case_insensitive = req.case_insensitive,
            whole_word = req.whole_word,
            hits = hits.len(),
            limit,
            file_ids = req.file_ids.len(),
            has_cursor = req.cursor.is_some(),
            has_next = next_cursor.is_some(),
            truncated,
            elapsed_ms,
            "search.done"
        );
        Ok((hits, elapsed_ms, truncated, has_next, next_cursor))
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_source_for_hits(
        &self,
        source: &SearchSource,
        source_idx: usize,
        start_offset: u64,
        start_line_no: u64,
        req: &SearchRequest,
        matcher: &LineMatcher,
        limit: usize,
        hits: &mut Vec<SearchHit>,
        has_next: &mut bool,
        next_cursor: &mut Option<String>,
    ) -> anyhow::Result<bool> {
        let mut matched_lines = Vec::new();
        let mut collect_line = |line: LogLine| -> anyhow::Result<()> {
            if line.line_no >= start_line_no {
                return Ok(());
            }

            if !matcher.is_match(&line.content) {
                return Ok(());
            }

            matched_lines.push(line);
            Ok(())
        };

        match source.kind {
            FileKind::Hot | FileKind::Rotated => {
                if self.state_dir.is_none() {
                    let memory_lines = self
                        .memory_lines
                        .read()
                        .map_err(|_| anyhow!("memory line index lock poisoned"))?;
                    if let Some(lines) = memory_lines.get(&source.file_id) {
                        for line in lines {
                            collect_line(line.clone())?;
                        }
                    }
                } else {
                    if let Err(err) =
                        scan_lines_from(&source.path, start_offset, 1, |line| collect_line(line))
                    {
                        if is_not_found_error(&err) {
                            return Ok(true);
                        }
                        return Err(err);
                    }
                }
            }
            kind @ (FileKind::Gzip | FileKind::Zstd | FileKind::Bzip2 | FileKind::Xz) => {
                if let Err(err) =
                    scan_compressed_lines(&source.path, kind.as_str(), |line| collect_line(line))
                {
                    if is_not_found_error(&err) {
                        return Ok(true);
                    }
                    return Err(err);
                }
            }
        }

        matched_lines.sort_by(|a, b| b.line_no.cmp(&a.line_no));

        for line in matched_lines {
            if hits.len() == limit {
                *has_next = true;
                *next_cursor = Some(encode_search_cursor(SearchCursor {
                    source_idx,
                    offset: 0,
                    line_no: line.line_no.saturating_add(1),
                }));
                return Ok(false);
            }

            let context_before = req.context_before.min(20);
            let context_after = req.context_after.min(20);
            let context = if context_before == 0 && context_after == 0 {
                Ok(Vec::new())
            } else if source.kind.is_compressed() {
                read_compressed_context_lines(
                    &source.path,
                    source.kind.as_str(),
                    line.line_no,
                    context_before,
                    context_after,
                )
            } else {
                read_context_lines(&source.path, line.line_no, context_before, context_after)
            }
            .unwrap_or_default();
            let (before, after) = split_context_lines(&context, line.line_no);

            hits.push(SearchHit {
                file_id: source.file_id.clone(),
                path: source.path.to_string_lossy().to_string(),
                line_no: line.line_no,
                offset: line.offset,
                score: 1.0,
                kind: source.kind.as_str().to_string(),
                content: line.content,
                before,
                after,
                context,
            });
        }

        Ok(true)
    }

    pub fn read_around(&self, req: &AroundRequest) -> anyhow::Result<AroundResponse> {
        let before = req.before.min(500);
        let after = req.after.min(500);
        let path = Path::new(&req.path);
        let probe_before = before.saturating_add(1).min(501);
        let probe_after = after.saturating_add(1).min(501);
        let mut lines = match if let Some(kind) = compressed_kind_for_path(path) {
            read_compressed_context_lines(path, kind, req.line_no, probe_before, probe_after)
        } else {
            read_context_lines(path, req.line_no, probe_before, probe_after)
        } {
            Ok(lines) => lines,
            Err(err) if is_not_found_error(&err) => Vec::new(),
            Err(err) => return Err(err),
        };

        let has_before = lines
            .first()
            .map(|line| line.line_no < req.line_no.saturating_sub(before as u64))
            .unwrap_or(req.line_no > 1);
        let has_after = lines
            .last()
            .map(|line| line.line_no > req.line_no.saturating_add(after as u64))
            .unwrap_or(false);
        let visible_start = req.line_no.saturating_sub(before as u64).max(1);
        let visible_end = req.line_no.saturating_add(after as u64);
        lines.retain(|line| line.line_no >= visible_start && line.line_no <= visible_end);

        Ok(AroundResponse {
            path: req.path.clone(),
            center_line_no: req.line_no,
            center_offset: req.offset,
            lines,
            has_before,
            has_after,
        })
    }

    fn search_sources(&self) -> anyhow::Result<Vec<SearchSource>> {
        if let Some(state_dir) = &self.state_dir {
            let state = IndexState::load(state_dir)?;
            let mut sources = state
                .files
                .values()
                .map(|state| SearchSource {
                    file_id: state.source_id.clone(),
                    path: state.path.clone(),
                    kind: state.kind.clone(),
                })
                .collect::<Vec<_>>();
            sources.sort_by(|a, b| {
                a.file_id
                    .cmp(&b.file_id)
                    .then_with(|| a.kind.as_str().cmp(b.kind.as_str()))
                    .then_with(|| a.path.cmp(&b.path))
            });
            return Ok(sources);
        }

        let memory_lines = self
            .memory_lines
            .read()
            .map_err(|_| anyhow!("memory line index lock poisoned"))?;
        let mut sources = memory_lines
            .iter()
            .filter_map(|(file_id, lines)| {
                lines.first().map(|line| SearchSource {
                    file_id: file_id.clone(),
                    path: line.path.clone(),
                    kind: FileKind::Hot,
                })
            })
            .collect::<Vec<_>>();
        sources.sort_by(|a, b| a.file_id.cmp(&b.file_id).then_with(|| a.path.cmp(&b.path)));
        Ok(sources)
    }
}

fn is_not_found_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .is_some_and(|io_err| io_err.kind() == ErrorKind::NotFound)
}

fn split_context_lines(context: &[ContextLine], line_no: u64) -> (Vec<String>, Vec<String>) {
    let mut before = Vec::new();
    let mut after = Vec::new();

    for line in context {
        if line.line_no < line_no {
            before.push(line.content.clone());
        } else if line.line_no > line_no {
            after.push(line.content.clone());
        }
    }

    (before, after)
}

#[derive(Debug, Clone, Copy, Default)]
struct SearchCursor {
    source_idx: usize,
    offset: u64,
    line_no: u64,
}

#[derive(Debug, Clone)]
struct SearchSource {
    file_id: String,
    path: PathBuf,
    kind: FileKind,
}

impl FileKind {
    fn as_str(&self) -> &'static str {
        match self {
            FileKind::Hot => "plain",
            FileKind::Rotated => "rotated",
            FileKind::Gzip => "gzip",
            FileKind::Zstd => "zstd",
            FileKind::Bzip2 => "bzip2",
            FileKind::Xz => "xz",
        }
    }

    fn is_compressed(&self) -> bool {
        matches!(
            self,
            FileKind::Gzip | FileKind::Zstd | FileKind::Bzip2 | FileKind::Xz
        )
    }
}

fn file_kind_for_compressed(kind: &str) -> FileKind {
    match kind {
        "zstd" => FileKind::Zstd,
        "bzip2" => FileKind::Bzip2,
        "xz" => FileKind::Xz,
        _ => FileKind::Gzip,
    }
}

fn compressed_kind_for_path(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("gz") => Some("gzip"),
        Some("zst") => Some("zstd"),
        Some("bz2") => Some("bzip2"),
        Some("xz") => Some("xz"),
        _ => None,
    }
}

fn scan_compressed_lines<F>(path: &Path, kind: &str, on_line: F) -> anyhow::Result<u64>
where
    F: FnMut(LogLine) -> anyhow::Result<()>,
{
    match kind {
        "zstd" => scan_zstd_lines(path, on_line),
        "bzip2" => scan_bzip2_lines(path, on_line),
        "xz" => scan_xz_lines(path, on_line),
        _ => scan_gzip_lines(path, on_line),
    }
}

fn read_compressed_context_lines(
    path: &Path,
    kind: &str,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<Vec<ContextLine>> {
    match kind {
        "zstd" => read_zstd_context_lines(path, line_no, before, after),
        "bzip2" => read_bzip2_context_lines(path, line_no, before, after),
        "xz" => read_xz_context_lines(path, line_no, before, after),
        _ => read_gzip_context_lines(path, line_no, before, after),
    }
}

fn encode_search_cursor(cursor: SearchCursor) -> String {
    format!("{}:{}:{}", cursor.source_idx, cursor.offset, cursor.line_no)
}

fn decode_search_cursor(cursor: Option<&str>) -> anyhow::Result<SearchCursor> {
    let Some(cursor) = cursor else {
        return Ok(SearchCursor {
            source_idx: 0,
            offset: 0,
            line_no: u64::MAX,
        });
    };

    if cursor.trim().is_empty() {
        return Ok(SearchCursor {
            source_idx: 0,
            offset: 0,
            line_no: u64::MAX,
        });
    }

    let parts = cursor.split(':').collect::<Vec<_>>();
    if parts.len() == 1 {
        return Ok(SearchCursor {
            source_idx: 0,
            offset: parts[0]
                .parse::<u64>()
                .with_context(|| format!("invalid search cursor: {cursor}"))?,
            line_no: u64::MAX,
        });
    }
    if parts.len() != 3 {
        return Err(anyhow!("invalid search cursor: {cursor}"));
    }

    Ok(SearchCursor {
        source_idx: parts[0]
            .parse::<usize>()
            .with_context(|| format!("invalid search cursor: {cursor}"))?,
        offset: parts[1]
            .parse::<u64>()
            .with_context(|| format!("invalid search cursor: {cursor}"))?,
        line_no: parts[2]
            .parse::<u64>()
            .with_context(|| format!("invalid search cursor: {cursor}"))?,
    })
}

#[cfg(test)]
fn line_matches(content: &str, req: &SearchRequest, final_regex: &regex::Regex) -> bool {
    LineMatcher::new(req, final_regex.clone()).is_match(content)
}

#[derive(Clone)]
enum LineMatcher {
    All,
    Regex(regex::Regex),
    Boolean(BooleanExpr),
    WholeWord {
        query: String,
        case_insensitive: bool,
    },
    AsciiCaseInsensitive {
        needle: Vec<u8>,
    },
    Literal {
        query: String,
        case_insensitive: bool,
    },
}

impl LineMatcher {
    fn new(req: &SearchRequest, regex: regex::Regex) -> Self {
        if req.query.trim().is_empty() {
            return Self::All;
        }

        if req.regex {
            return Self::Regex(regex);
        }

        if let Some(expr) = parse_boolean_query(req) {
            return Self::Boolean(expr);
        }

        if req.whole_word {
            return Self::WholeWord {
                query: req.query.clone(),
                case_insensitive: req.case_insensitive,
            };
        }

        if req.case_insensitive && req.query.is_ascii() {
            return Self::AsciiCaseInsensitive {
                needle: req
                    .query
                    .bytes()
                    .map(|byte| byte.to_ascii_lowercase())
                    .collect(),
            };
        }

        Self::Literal {
            query: req.query.clone(),
            case_insensitive: req.case_insensitive,
        }
    }

    fn is_match(&self, content: &str) -> bool {
        match self {
            Self::All => true,
            Self::Regex(regex) => regex.is_match(content),
            Self::Boolean(expr) => expr.is_match(content),
            Self::WholeWord {
                query,
                case_insensitive,
            } => contains_whole_word(content, query, *case_insensitive),
            Self::AsciiCaseInsensitive { needle } => {
                contains_ascii_case_insensitive(content.as_bytes(), needle)
            }
            Self::Literal {
                query,
                case_insensitive,
            } => {
                if *case_insensitive {
                    content.to_lowercase().contains(&query.to_lowercase())
                } else {
                    content.contains(query)
                }
            }
        }
    }
}

#[derive(Clone)]
enum BooleanExpr {
    Term(Box<LineMatcher>),
    And(Box<BooleanExpr>, Box<BooleanExpr>),
    Or(Box<BooleanExpr>, Box<BooleanExpr>),
}

impl BooleanExpr {
    fn is_match(&self, content: &str) -> bool {
        match self {
            Self::Term(matcher) => matcher.is_match(content),
            Self::And(left, right) => left.is_match(content) && right.is_match(content),
            Self::Or(left, right) => left.is_match(content) || right.is_match(content),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BooleanToken {
    Term(String),
    And,
    Or,
    LeftParen,
    RightParen,
}

fn parse_boolean_query(req: &SearchRequest) -> Option<BooleanExpr> {
    let tokens = tokenize_boolean_query(&req.query);
    let has_operator = tokens.iter().any(|token| {
        matches!(
            token,
            BooleanToken::And
                | BooleanToken::Or
                | BooleanToken::LeftParen
                | BooleanToken::RightParen
        )
    });
    if !has_operator {
        return None;
    }

    let mut parser = BooleanParser {
        tokens,
        index: 0,
        req,
    };
    let expr = parser.parse_or()?;
    if parser.index == parser.tokens.len() {
        Some(expr)
    } else {
        None
    }
}

fn tokenize_boolean_query(query: &str) -> Vec<BooleanToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in query.chars() {
        match ch {
            '(' | ')' => {
                push_boolean_term(&mut tokens, &mut current);
                tokens.push(if ch == '(' {
                    BooleanToken::LeftParen
                } else {
                    BooleanToken::RightParen
                });
            }
            ch if ch.is_whitespace() => push_boolean_term(&mut tokens, &mut current),
            _ => current.push(ch),
        }
    }

    push_boolean_term(&mut tokens, &mut current);
    tokens
}

fn push_boolean_term(tokens: &mut Vec<BooleanToken>, current: &mut String) {
    if current.is_empty() {
        return;
    }

    tokens.push(match current.as_str() {
        "AND" => BooleanToken::And,
        "OR" => BooleanToken::Or,
        _ => BooleanToken::Term(std::mem::take(current)),
    });
    current.clear();
}

struct BooleanParser<'a> {
    tokens: Vec<BooleanToken>,
    index: usize,
    req: &'a SearchRequest,
}

impl BooleanParser<'_> {
    fn parse_or(&mut self) -> Option<BooleanExpr> {
        let mut expr = self.parse_and()?;
        while self.consume(&BooleanToken::Or) {
            expr = BooleanExpr::Or(Box::new(expr), Box::new(self.parse_and()?));
        }
        Some(expr)
    }

    fn parse_and(&mut self) -> Option<BooleanExpr> {
        let mut expr = self.parse_primary()?;
        while self.consume(&BooleanToken::And) {
            expr = BooleanExpr::And(Box::new(expr), Box::new(self.parse_primary()?));
        }
        Some(expr)
    }

    fn parse_primary(&mut self) -> Option<BooleanExpr> {
        match self.tokens.get(self.index)?.clone() {
            BooleanToken::Term(query) => {
                self.index += 1;
                Some(BooleanExpr::Term(Box::new(term_matcher(self.req, query))))
            }
            BooleanToken::LeftParen => {
                self.index += 1;
                let expr = self.parse_or()?;
                if !self.consume(&BooleanToken::RightParen) {
                    return None;
                }
                Some(expr)
            }
            BooleanToken::And | BooleanToken::Or | BooleanToken::RightParen => None,
        }
    }

    fn consume(&mut self, expected: &BooleanToken) -> bool {
        if self.tokens.get(self.index) == Some(expected) {
            self.index += 1;
            true
        } else {
            false
        }
    }
}

fn term_matcher(req: &SearchRequest, query: String) -> LineMatcher {
    if req.whole_word {
        return LineMatcher::WholeWord {
            query,
            case_insensitive: req.case_insensitive,
        };
    }

    if req.case_insensitive && query.is_ascii() {
        return LineMatcher::AsciiCaseInsensitive {
            needle: query
                .bytes()
                .map(|byte| byte.to_ascii_lowercase())
                .collect(),
        };
    }

    LineMatcher::Literal {
        query,
        case_insensitive: req.case_insensitive,
    }
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }

    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.to_ascii_lowercase() == *right)
    })
}

fn print_cli_progress(
    source_id: &str,
    kind: &str,
    lines: usize,
    indexed_offset: u64,
    file_size: u64,
    progress_percent: f64,
    elapsed_ms: u128,
) {
    if std::io::stdout().is_terminal() {
        let total_elapsed = CLI_PROGRESS_STARTED
            .get()
            .map(Instant::elapsed)
            .unwrap_or_else(|| Duration::from_millis(elapsed_ms as u64));
        print!(
            "\r\x1b[2K    {:>5.1}%  lines={}  file={}  total={}",
            progress_percent,
            lines,
            format_duration(Duration::from_millis(elapsed_ms as u64)),
            format_duration(total_elapsed),
        );
        let _ = std::io::stdout().flush();
    }
    let _ = (source_id, kind, indexed_offset, file_size);
}

pub fn finish_cli_progress_line() {
    if std::io::stdout().is_terminal() {
        println!();
    }
}

fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();
    if total_secs < 60 {
        return format!("{}.{:03}s", total_secs, millis);
    }

    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    format!("{minutes}m{seconds:02}s")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::state_path;

    fn line(path: &str, line_no: u64, offset: u64, content: &str) -> LogLine {
        LogLine {
            path: PathBuf::from(path),
            line_no,
            offset,
            content: content.to_string(),
        }
    }

    #[test]
    fn index_trace_logging_is_off_by_default() {
        assert!(!trace_index_logs_enabled_from_env(|_| None));
        assert!(!trace_index_logs_enabled_from_env(|_| Some(
            "0".to_string()
        )));
        assert!(!trace_index_logs_enabled_from_env(|_| Some(
            "false".to_string()
        )));
    }

    #[test]
    fn index_trace_logging_accepts_explicit_opt_in_values() {
        for value in ["1", "true", "TRUE", "yes", "YES", "on", "ON"] {
            assert!(trace_index_logs_enabled_from_env(|_| Some(
                value.to_string()
            )));
        }
    }

    #[test]
    fn editor_style_matching_finds_literal_chinese_without_ngrams() {
        let req = SearchRequest {
            query: "订单创建".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let regex = build_regex(&req).unwrap();

        assert!(line_matches("INFO 订单创建失败", &req, &regex));
        assert!(!line_matches("INFO 订单支付失败", &req, &regex));
    }

    #[test]
    fn ascii_case_insensitive_matching_uses_same_literal_semantics() {
        let req = SearchRequest {
            query: "orderservice".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let regex = build_regex(&req).unwrap();

        assert!(line_matches(
            "ERROR com.foo.OrderService timeout",
            &req,
            &regex
        ));
        assert!(!line_matches(
            "ERROR com.foo.UserService timeout",
            &req,
            &regex
        ));
    }

    #[test]
    fn unicode_case_insensitive_matching_keeps_original_correctness() {
        let req = SearchRequest {
            query: "数据源".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let regex = build_regex(&req).unwrap();

        assert!(line_matches(
            "当前执行的数据源为 estate_business",
            &req,
            &regex
        ));
    }

    #[test]
    fn boolean_matching_supports_uppercase_and_or_with_parentheses() {
        let req = SearchRequest {
            query: "(timeout AND retry) OR fatal".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let regex = build_regex(&req).unwrap();

        assert!(line_matches("WARN retry after timeout", &req, &regex));
        assert!(line_matches("FATAL service stopped", &req, &regex));
        assert!(!line_matches(
            "WARN timeout without second term",
            &req,
            &regex
        ));
    }

    #[test]
    fn lowercase_and_or_remain_literal_terms() {
        let req = SearchRequest {
            query: "timeout and retry".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let regex = build_regex(&req).unwrap();

        assert!(line_matches("timeout and retry", &req, &regex));
        assert!(!line_matches("timeout then retry", &req, &regex));
    }

    #[test]
    fn opening_persistent_index_creates_only_configured_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("index");

        let _index = LogSearchIndex::open_or_create(&config_dir).unwrap();

        assert!(config_dir.exists());
        assert!(!dir.path().join("tantivy").exists());
    }

    #[test]
    fn rebuild_index_storage_removes_state_directory() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("index");
        let layout = IndexLayout::from_config_dir(&config_dir);
        fs::create_dir_all(&layout.state_dir).unwrap();
        fs::write(state_path(&layout.state_dir), "{}").unwrap();

        rebuild_index_storage(&config_dir).unwrap();

        assert!(layout.state_dir.exists());
        assert!(!state_path(&layout.state_dir).exists());
    }

    #[test]
    fn index_finds_log_fragments_with_symbols() {
        let index = LogSearchIndex::create_in_ram().unwrap();
        index
            .index_lines(
                "app",
                &[
                    line("/tmp/app.log", 1, 0, "INFO boot complete"),
                    line(
                        "/tmp/app.log",
                        2,
                        19,
                        "ERROR com.foo.OrderService order_id=123 timeout",
                    ),
                ],
            )
            .unwrap();

        let req = SearchRequest {
            query: "com.foo.OrderService".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };

        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line_no, 2);
    }

    #[test]
    fn search_cursor_returns_next_batch_without_repeating_hits() {
        let index = LogSearchIndex::create_in_ram().unwrap();
        let lines = (1..=5)
            .map(|line_no| {
                line(
                    "/tmp/app.log",
                    line_no,
                    line_no * 10,
                    &format!("ERROR timeout item={line_no}"),
                )
            })
            .collect::<Vec<_>>();
        index.index_lines("app", &lines).unwrap();

        let first_req = SearchRequest {
            query: "timeout".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 2,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let (first_hits, _, _, first_has_next, cursor) = index.search(&first_req).unwrap();

        assert_eq!(first_hits.len(), 2);
        assert_eq!(first_hits[0].line_no, 5);
        assert_eq!(first_hits[1].line_no, 4);
        assert!(first_has_next);
        assert!(cursor.is_some());

        let second_req = SearchRequest {
            cursor,
            ..first_req
        };
        let (second_hits, _, _, _, _) = index.search(&second_req).unwrap();
        let first_keys = first_hits
            .iter()
            .map(|hit| (hit.path.clone(), hit.line_no))
            .collect::<std::collections::BTreeSet<_>>();
        let second_keys = second_hits
            .iter()
            .map(|hit| (hit.path.clone(), hit.line_no))
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(second_hits.len(), 2);
        assert_eq!(second_hits[0].line_no, 3);
        assert_eq!(second_hits[1].line_no, 2);
        assert!(first_keys.is_disjoint(&second_keys));
    }

    #[test]
    fn persistent_search_scans_selected_source_after_first_source() {
        let dir = tempfile::tempdir().unwrap();
        let business_path = dir.path().join("business.log");
        let easypay_path = dir.path().join("easypay.log");
        std::fs::write(&business_path, "INFO business line\n").unwrap();
        std::fs::write(&easypay_path, "INFO settlement_batch_no = '2025021'\n").unwrap();
        let index_dir = dir.path().join("index");
        let index = LogSearchIndex::open_or_create(&index_dir).unwrap();
        index.sync_file("business", &business_path).unwrap();
        index.sync_file("easypay", &easypay_path).unwrap();

        let req = SearchRequest {
            query: "settlement_batch_no".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: vec!["easypay".to_string()],
            context_before: 0,
            context_after: 0,
        };

        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_id, "easypay");
        assert_eq!(hits[0].line_no, 1);
    }

    #[test]
    fn empty_query_returns_all_lines_newest_first() {
        let index = LogSearchIndex::create_in_ram().unwrap();
        index
            .index_lines(
                "app",
                &[
                    line("/tmp/app.log", 1, 0, "first line"),
                    line("/tmp/app.log", 2, 12, "second line"),
                    line("/tmp/app.log", 3, 24, "third line"),
                ],
            )
            .unwrap();

        let req = SearchRequest {
            query: String::new(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 2,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };

        let (first_hits, _, _, has_next, cursor) = index.search(&req).unwrap();

        assert_eq!(first_hits.len(), 2);
        assert_eq!(first_hits[0].content, "third line");
        assert_eq!(first_hits[1].content, "second line");
        assert!(has_next);
        assert!(cursor.is_some());

        let (next_hits, _, _, _, _) = index.search(&SearchRequest { cursor, ..req }).unwrap();

        assert_eq!(next_hits.len(), 1);
        assert_eq!(next_hits[0].content, "first line");
    }

    #[test]
    fn regex_search_uses_candidates_then_confirms_original_content() {
        let index = LogSearchIndex::create_in_ram().unwrap();
        index
            .index_lines(
                "app",
                &[
                    line("/tmp/app.log", 1, 0, "WARN retry after timeout"),
                    line("/tmp/app.log", 2, 25, "ERROR retry after timeout"),
                ],
            )
            .unwrap();

        let req = SearchRequest {
            query: "ERROR .* timeout".to_string(),
            regex: true,
            case_insensitive: false,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };

        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line_no, 2);
    }

    #[test]
    fn search_can_be_limited_to_selected_file_ids() {
        let index = LogSearchIndex::create_in_ram().unwrap();
        index
            .index_lines(
                "app",
                &[line("/tmp/app.log", 1, 0, "shared timeout from app")],
            )
            .unwrap();
        index
            .index_lines(
                "worker",
                &[line("/tmp/worker.log", 1, 0, "shared timeout from worker")],
            )
            .unwrap();

        let req = SearchRequest {
            query: "timeout".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: vec!["app".to_string()],
            context_before: 0,
            context_after: 0,
        };

        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_id, "app");
        assert_eq!(hits[0].content, "shared timeout from app");
    }

    #[test]
    fn sync_file_indexes_only_appended_lines() {
        use std::{fs::OpenOptions, io::Write};

        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let log_path = dir.path().join("app.log");
        std::fs::write(&log_path, "first timeout\n").unwrap();

        let index = LogSearchIndex::open_or_create(&index_dir).unwrap();
        assert_eq!(index.sync_file("app", &log_path).unwrap(), 1);
        assert_eq!(index.sync_file("app", &log_path).unwrap(), 0);

        let mut file = OpenOptions::new().append(true).open(&log_path).unwrap();
        writeln!(file, "second timeout").unwrap();

        assert_eq!(index.sync_file("app", &log_path).unwrap(), 1);

        let req = SearchRequest {
            query: "timeout".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn sync_file_treats_missing_hot_log_as_empty_source() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let missing_path = dir.path().join("missing.log");

        let index = LogSearchIndex::open_or_create(&index_dir).unwrap();

        assert_eq!(index.sync_file("app", &missing_path).unwrap(), 0);

        let req = SearchRequest {
            query: "timeout".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert!(hits.is_empty());
    }

    #[test]
    fn search_returns_empty_when_indexed_file_was_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let log_path = dir.path().join("app.log");
        std::fs::write(&log_path, "timeout before delete\n").unwrap();

        let index = LogSearchIndex::open_or_create(&index_dir).unwrap();
        index.sync_file("app", &log_path).unwrap();
        std::fs::remove_file(&log_path).unwrap();

        let req = SearchRequest {
            query: "timeout".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert!(hits.is_empty());
    }

    #[test]
    fn search_returns_context_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("app.log");
        std::fs::write(&log_path, "before\nmatch timeout\nafter\n").unwrap();

        let index = LogSearchIndex::create_in_ram().unwrap();
        index.replace_file("app", &log_path).unwrap();

        let req = SearchRequest {
            query: "timeout".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 1,
            context_after: 1,
        };
        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].before, vec!["before"]);
        assert_eq!(hits[0].after, vec!["after"]);
        assert_eq!(hits[0].context.len(), 3);
        assert_eq!(hits[0].context[0].line_no, 1);
        assert_eq!(hits[0].context[1].line_no, 2);
        assert_eq!(hits[0].context[1].content, "match timeout");
        assert_eq!(hits[0].context[2].line_no, 3);
    }

    #[test]
    fn read_around_returns_larger_window_with_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("app.log");
        std::fs::write(
            &log_path,
            "line 1\nline 2\nline 3 timeout\nline 4\nline 5\nline 6\n",
        )
        .unwrap();

        let index = LogSearchIndex::create_in_ram().unwrap();
        let around = index
            .read_around(&AroundRequest {
                path: log_path.to_string_lossy().to_string(),
                line_no: 3,
                offset: 14,
                compressed: false,
                before: 2,
                after: 2,
            })
            .unwrap();

        assert_eq!(around.lines.len(), 5);
        assert_eq!(around.lines[0].line_no, 1);
        assert_eq!(around.lines[2].line_no, 3);
        assert_eq!(around.lines[2].content, "line 3 timeout");
        assert_eq!(around.lines[4].line_no, 5);
        assert!(!around.has_before);
        assert!(around.has_after);
    }

    #[test]
    fn read_around_reports_when_no_more_lines_are_available() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("app.log");
        std::fs::write(&log_path, "line 1\nline 2 timeout\nline 3\n").unwrap();

        let index = LogSearchIndex::create_in_ram().unwrap();
        let around = index
            .read_around(&AroundRequest {
                path: log_path.to_string_lossy().to_string(),
                line_no: 2,
                offset: 7,
                compressed: false,
                before: 10,
                after: 10,
            })
            .unwrap();

        assert_eq!(around.lines.len(), 3);
        assert!(!around.has_before);
        assert!(!around.has_after);
    }

    #[test]
    fn gzip_file_can_be_indexed_and_searched() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let log_path = dir.path().join("app.log.1.gz");
        let file = std::fs::File::create(&log_path).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        write!(encoder, "before\narchived timeout\nafter\n").unwrap();
        encoder.finish().unwrap();

        let index = LogSearchIndex::open_or_create(&index_dir).unwrap();
        assert_eq!(
            index
                .sync_compressed_file("app", &log_path, "gzip")
                .unwrap(),
            3
        );
        assert_eq!(
            index
                .sync_compressed_file("app", &log_path, "gzip")
                .unwrap(),
            0
        );

        let req = SearchRequest {
            query: "archived timeout".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 1,
            context_after: 1,
        };
        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].before, vec!["before"]);
        assert_eq!(hits[0].after, vec!["after"]);
    }

    #[test]
    fn compressed_files_can_be_indexed_and_searched() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let zstd_path = dir.path().join("app.log.1.zst");
        let bzip2_path = dir.path().join("app.log.2.bz2");
        let xz_path = dir.path().join("app.log.3.xz");
        std::fs::write(
            &zstd_path,
            zstd::encode_all("one\nzstd timeout\nthree\n".as_bytes(), 0).unwrap(),
        )
        .unwrap();
        write_bzip2(&bzip2_path, "one\nbzip2 timeout\nthree\n");
        write_xz(&xz_path, "one\nxz timeout\nthree\n");

        let index = LogSearchIndex::open_or_create(&index_dir).unwrap();
        assert_eq!(
            index
                .sync_compressed_file("zstd", &zstd_path, "zstd")
                .unwrap(),
            3
        );
        assert_eq!(
            index
                .sync_compressed_file("bzip2", &bzip2_path, "bzip2")
                .unwrap(),
            3
        );
        assert_eq!(index.sync_compressed_file("xz", &xz_path, "xz").unwrap(), 3);

        let req = SearchRequest {
            query: "timeout".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let (hits, _, _, _, _) = index.search(&req).unwrap();
        let kinds = hits
            .iter()
            .map(|hit| hit.kind.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(kinds, BTreeSet::from(["bzip2", "xz", "zstd"]));
    }

    fn write_bzip2(path: &Path, content: &str) {
        use bzip2::{Compression, write::BzEncoder};
        use std::io::Write;

        let file = std::fs::File::create(path).unwrap();
        let mut encoder = BzEncoder::new(file, Compression::default());
        write!(encoder, "{content}").unwrap();
        encoder.finish().unwrap();
    }

    fn write_xz(path: &Path, content: &str) {
        use std::io::Write;
        use xz2::write::XzEncoder;

        let file = std::fs::File::create(path).unwrap();
        let mut encoder = XzEncoder::new(file, 6);
        write!(encoder, "{content}").unwrap();
        encoder.finish().unwrap();
    }

    #[test]
    fn chinese_short_phrase_can_be_indexed_and_searched() {
        let index = LogSearchIndex::create_in_ram().unwrap();
        index
            .index_lines(
                "app",
                &[line("/tmp/app.log", 1, 0, "2026-05-22 INFO 这是追加的内容")],
            )
            .unwrap();

        let req = SearchRequest {
            query: "这是".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line_no, 1);

        let req = SearchRequest {
            query: "追加".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line_no, 1);

        let req = SearchRequest {
            query: "这".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: false,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };
        let (hits, _, _, _, _) = index.search(&req).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line_no, 1);
    }
}
