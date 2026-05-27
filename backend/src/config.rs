use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub files: Vec<LogFileConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_addr")]
    pub addr: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexConfig {
    #[serde(default = "default_index_dir")]
    pub dir: PathBuf,
    #[serde(default = "default_commit_batch_size")]
    pub commit_batch_size: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogFileConfig {
    pub id: String,
    pub path: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: default_addr(),
        }
    }
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            dir: default_index_dir(),
            commit_batch_size: default_commit_batch_size(),
        }
    }
}

impl AppConfig {
    pub fn load(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let source = fs::read_to_string(&path)?;
        let mut cfg: Self = toml::from_str(&source)?;
        let config_path = absolutize(&path)?;
        let base = config_path.parent().unwrap_or_else(|| Path::new("."));

        if cfg.index.dir.is_relative() {
            cfg.index.dir = base.join(&cfg.index.dir);
        }
        cfg.index.dir = absolutize(&cfg.index.dir)?;

        for file in &mut cfg.files {
            if file.path.is_relative() {
                file.path = base.join(&file.path);
            }
            file.path = absolutize(&file.path)?;
        }

        Ok(cfg)
    }
}

fn absolutize(path: &Path) -> anyhow::Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }

    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(std::env::current_dir()?.join(path))
}

fn default_addr() -> String {
    "0.0.0.0:12457".to_string()
}

fn default_index_dir() -> PathBuf {
    PathBuf::from("./data/index")
}

fn default_commit_batch_size() -> usize {
    5_000
}
