use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IndexState {
    pub files: BTreeMap<String, FileState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileState {
    pub path: PathBuf,
    pub source_id: String,
    pub generation_id: String,
    pub kind: FileKind,
    pub fingerprint: FileFingerprint,
    pub indexed_offset: u64,
    pub indexed_line_no: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    Hot,
    Rotated,
    Gzip,
    Zstd,
    Bzip2,
    Xz,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileFingerprint {
    pub len: u64,
    pub modified_unix_ms: u128,
    pub sha256_prefix: Option<String>,
    #[cfg(unix)]
    pub dev: u64,
    #[cfg(unix)]
    pub inode: u64,
}

impl IndexState {
    pub fn load(state_dir: &Path) -> anyhow::Result<Self> {
        let path = state_path(state_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let source = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&source)?)
    }

    pub fn save(&self, state_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(state_dir)?;
        fs::write(state_path(state_dir), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

pub fn fingerprint(path: &Path) -> anyhow::Result<FileFingerprint> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    let modified_unix_ms = modified
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    Ok(FileFingerprint {
        len: metadata.len(),
        modified_unix_ms,
        sha256_prefix: None,
        #[cfg(unix)]
        dev: unix_dev(&metadata),
        #[cfg(unix)]
        inode: unix_inode(&metadata),
    })
}

pub fn compressed_fingerprint(path: &Path) -> anyhow::Result<FileFingerprint> {
    let mut fingerprint = fingerprint(path)?;
    fingerprint.sha256_prefix = Some(sha256_prefix(path, 1024 * 1024)?);
    Ok(fingerprint)
}

pub fn generation_id(source_id: &str, fingerprint: &FileFingerprint) -> String {
    #[cfg(unix)]
    {
        format!("{source_id}@{}:{}", fingerprint.dev, fingerprint.inode)
    }

    #[cfg(not(unix))]
    {
        format!(
            "{source_id}@{}:{}",
            fingerprint.len, fingerprint.modified_unix_ms
        )
    }
}

pub fn state_path(state_dir: &Path) -> PathBuf {
    state_dir.join("log-search-state.json")
}

fn sha256_prefix(path: &Path, max_bytes: usize) -> anyhow::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut remaining = max_bytes;
    let mut buf = [0_u8; 16 * 1024];

    while remaining > 0 {
        let read_len = remaining.min(buf.len());
        let read = file.read(&mut buf[..read_len])?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        remaining -= read;
    }

    Ok(hasher
        .finalize()
        .as_slice()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(unix)]
fn unix_inode(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.ino()
}

#[cfg(unix)]
fn unix_dev(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.dev()
}

pub fn can_increment(previous: &FileState, current: &FileFingerprint) -> bool {
    same_file(&previous.fingerprint, current) && current.len >= previous.indexed_offset
}

pub fn same_file(previous: &FileFingerprint, current: &FileFingerprint) -> bool {
    #[cfg(unix)]
    {
        previous.dev == current.dev && previous.inode == current.inode
    }

    #[cfg(not(unix))]
    {
        current.len >= previous.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(len: u64, inode: u64) -> FileFingerprint {
        FileFingerprint {
            len,
            modified_unix_ms: 0,
            sha256_prefix: None,
            #[cfg(unix)]
            dev: 1,
            #[cfg(unix)]
            inode,
        }
    }

    #[test]
    fn increment_is_allowed_for_same_file_that_grew() {
        let previous = FileState {
            path: PathBuf::from("/tmp/app.log"),
            source_id: "app".to_string(),
            generation_id: "app@1:7".to_string(),
            kind: FileKind::Hot,
            fingerprint: fp(10, 7),
            indexed_offset: 10,
            indexed_line_no: 2,
        };

        assert!(can_increment(&previous, &fp(25, 7)));
    }

    #[test]
    fn increment_is_rejected_when_file_was_truncated() {
        let previous = FileState {
            path: PathBuf::from("/tmp/app.log"),
            source_id: "app".to_string(),
            generation_id: "app@1:7".to_string(),
            kind: FileKind::Hot,
            fingerprint: fp(10, 7),
            indexed_offset: 10,
            indexed_line_no: 2,
        };

        assert!(!can_increment(&previous, &fp(5, 7)));
    }

    #[test]
    fn increment_is_rejected_when_inode_changes() {
        let previous = FileState {
            path: PathBuf::from("/tmp/app.log"),
            source_id: "app".to_string(),
            generation_id: "app@1:7".to_string(),
            kind: FileKind::Hot,
            fingerprint: fp(10, 7),
            indexed_offset: 10,
            indexed_line_no: 2,
        };

        assert!(!can_increment(&previous, &fp(25, 9)));
    }

    #[test]
    fn generation_uses_source_and_file_identity() {
        assert_eq!(generation_id("app", &fp(10, 7)), "app@1:7");
    }
}
