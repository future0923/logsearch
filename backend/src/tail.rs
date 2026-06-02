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

    loop {
        let mut content = String::new();
        let start_offset = offset;
        let bytes = reader.read_line(&mut content)?;
        if bytes == 0 {
            break;
        }

        offset += bytes as u64;
        lines.push(TailLine {
            line_no,
            offset: start_offset,
            content: trim_line_ending(content),
        });
        line_no += 1;
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
}
