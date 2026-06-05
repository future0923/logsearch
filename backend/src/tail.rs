use crate::{
    config::AppConfig,
    watcher::{DiscoveredFileKind, discover_files},
};
use serde::Serialize;
use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const MAX_INITIAL_LINES: usize = 1_000;
const MAX_TAIL_BATCH_LINES: usize = 500;
const MAX_TAIL_BATCH_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailLine {
    pub line_no: u64,
    pub offset: u64,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailSnapshot {
    pub path: PathBuf,
    pub offset: u64,
    pub next_line_no: u64,
    pub lines: Vec<TailLine>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum TailError {
    #[error("file id cannot be empty")]
    EmptyFileId,
    #[error("tail source not found: {0}")]
    NotFound(String),
    #[error("tail only supports hot log files")]
    NotHot,
    #[error("tail source is missing: {0}")]
    Missing(String),
}

pub fn resolve_tail_path(config: &AppConfig, file_id: &str) -> Result<PathBuf, TailError> {
    if file_id.trim().is_empty() {
        return Err(TailError::EmptyFileId);
    }

    let file = discover_files(config)
        .into_iter()
        .find(|file| file.id == file_id)
        .ok_or_else(|| TailError::NotFound(file_id.to_string()))?;

    if file.kind != DiscoveredFileKind::Hot {
        return Err(TailError::NotHot);
    }

    if !file.exists {
        return Err(TailError::Missing(file.path.display().to_string()));
    }

    Ok(file.path)
}

pub fn initial_tail_snapshot(path: &Path, requested_lines: usize) -> anyhow::Result<TailSnapshot> {
    let line_limit = requested_lines.clamp(1, MAX_INITIAL_LINES);
    let mut reader = BufReader::new(File::open(path)?);
    let mut retained = Vec::<TailLine>::new();
    let mut line_no = 1_u64;
    let mut offset = 0_u64;

    loop {
        let mut content = String::new();
        let start_offset = offset;
        let bytes = reader.read_line(&mut content)?;
        if bytes == 0 {
            break;
        }

        offset += bytes as u64;
        retained.push(TailLine {
            line_no,
            offset: start_offset,
            content: trim_line_ending(content),
        });
        if retained.len() > line_limit {
            retained.remove(0);
        }
        line_no += 1;
    }

    Ok(TailSnapshot {
        path: path.to_path_buf(),
        offset,
        next_line_no: line_no,
        lines: retained,
    })
}

pub fn read_new_tail_lines(
    path: &Path,
    offset: u64,
    next_line_no: u64,
) -> anyhow::Result<TailSnapshot> {
    let mut reader = BufReader::new(File::open(path)?);
    let file_len = reader.get_ref().metadata()?.len();
    let mut offset = offset.min(file_len);
    reader.seek(SeekFrom::Start(offset))?;

    let mut lines = Vec::<TailLine>::new();
    let mut line_no = next_line_no;
    let mut batch_bytes = 0_usize;

    loop {
        let mut content = String::new();
        let start_offset = offset;
        let bytes = reader.read_line(&mut content)?;
        if bytes == 0 {
            break;
        }

        offset += bytes as u64;
        batch_bytes += bytes;
        lines.push(TailLine {
            line_no,
            offset: start_offset,
            content: trim_line_ending(content),
        });
        line_no += 1;

        if lines.len() >= MAX_TAIL_BATCH_LINES || batch_bytes >= MAX_TAIL_BATCH_BYTES {
            break;
        }
    }

    Ok(TailSnapshot {
        path: path.to_path_buf(),
        offset,
        next_line_no: line_no,
        lines,
    })
}

fn trim_line_ending(mut content: String) -> String {
    if content.ends_with('\n') {
        content.pop();
        if content.ends_with('\r') {
            content.pop();
        }
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_tail_snapshot_keeps_last_requested_lines_and_end_offset() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("app.log");
        std::fs::write(&log_path, "one\ntwo\nthree\n").unwrap();

        let snapshot = initial_tail_snapshot(&log_path, 2).unwrap();

        assert_eq!(snapshot.offset, 14);
        assert_eq!(snapshot.next_line_no, 4);
        assert_eq!(
            snapshot.lines,
            vec![
                TailLine {
                    line_no: 2,
                    offset: 4,
                    content: "two".to_string(),
                },
                TailLine {
                    line_no: 3,
                    offset: 8,
                    content: "three".to_string(),
                },
            ]
        );
    }

    #[test]
    fn read_new_tail_lines_reads_only_after_offset() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("app.log");
        std::fs::write(&log_path, "one\n").unwrap();
        let initial = initial_tail_snapshot(&log_path, 200).unwrap();
        std::fs::write(&log_path, "one\ntwo\nthree\n").unwrap();

        let snapshot =
            read_new_tail_lines(&log_path, initial.offset, initial.next_line_no).unwrap();

        assert_eq!(snapshot.offset, 14);
        assert_eq!(snapshot.next_line_no, 4);
        assert_eq!(
            snapshot.lines,
            vec![
                TailLine {
                    line_no: 2,
                    offset: 4,
                    content: "two".to_string(),
                },
                TailLine {
                    line_no: 3,
                    offset: 8,
                    content: "three".to_string(),
                },
            ]
        );
    }

    #[test]
    fn read_new_tail_lines_limits_each_batch_and_keeps_resume_offset() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("app.log");
        let content = (1..=600)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        std::fs::write(&log_path, content).unwrap();

        let first = read_new_tail_lines(&log_path, 0, 1).unwrap();

        assert_eq!(first.lines.len(), 500);
        assert_eq!(first.next_line_no, 501);
        assert_eq!(first.lines.first().unwrap().content, "line 1");
        assert_eq!(first.lines.last().unwrap().content, "line 500");

        let second = read_new_tail_lines(&log_path, first.offset, first.next_line_no).unwrap();

        assert_eq!(second.lines.len(), 100);
        assert_eq!(second.next_line_no, 601);
        assert_eq!(second.lines.first().unwrap().content, "line 501");
        assert_eq!(second.lines.last().unwrap().content, "line 600");
    }

    #[test]
    fn read_new_tail_lines_limits_each_batch_by_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("app.log");
        let line = format!("{}\n", "x".repeat(4095));
        let line_count = (MAX_TAIL_BATCH_BYTES / line.len()) + 20;
        std::fs::write(&log_path, line.repeat(line_count)).unwrap();

        let first = read_new_tail_lines(&log_path, 0, 1).unwrap();

        assert!(first.lines.len() < line_count);
        assert!(first.lines.iter().map(|line| line.content.len()).sum::<usize>() <= MAX_TAIL_BATCH_BYTES);

        let second = read_new_tail_lines(&log_path, first.offset, first.next_line_no).unwrap();

        assert!(!second.lines.is_empty());
        assert_eq!(second.lines.first().unwrap().line_no, first.next_line_no);
    }
}
