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
    #[serde(default)]
    pub directories: Vec<LogDirectoryConfig>,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogFileConfig {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogDirectoryConfig {
    pub id: String,
    pub path: PathBuf,
    #[serde(default = "default_directory_include")]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub recursive: bool,
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

        for directory in &mut cfg.directories {
            validate_directory_path(&directory.path)?;
            if directory.path.is_relative() {
                directory.path = base.join(&directory.path);
            }
            directory.path = absolutize(&directory.path)?;
        }

        Ok(cfg)
    }
}

fn validate_directory_path(path: &Path) -> anyhow::Result<()> {
    let source = path.to_string_lossy();
    if source.contains('*') || source.contains('?') || source.contains('[') {
        anyhow::bail!(
            "directories.path must be a real directory, put patterns in include instead: path = {:?}",
            path
        );
    }
    Ok(())
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

fn default_directory_include() -> Vec<String> {
    vec![
        "*.log".to_string(),
        "*.gz".to_string(),
        "*.zst".to_string(),
        "*.bz2".to_string(),
        "*.xz".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_directory_sources_with_default_log_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir(&logs_dir).unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[index]
dir = "./index"

[[directories]]
id = "release"
path = "./logs"
"#,
        )
        .unwrap();

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(config.directories.len(), 1);
        assert_eq!(config.directories[0].id, "release");
        assert_eq!(config.directories[0].path, logs_dir.canonicalize().unwrap());
        assert_eq!(
            config.directories[0].include,
            vec!["*.log", "*.gz", "*.zst", "*.bz2", "*.xz"]
        );
        assert!(config.directories[0].exclude.is_empty());
        assert!(!config.directories[0].recursive);
    }

    #[test]
    fn rejects_glob_patterns_in_directory_path() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[[directories]]
id = "release"
path = "./logs/**"
include = ["*.log"]
"#,
        )
        .unwrap();

        let err = AppConfig::load(&config_path).unwrap_err();

        assert!(err.to_string().contains("directories.path must be a real directory"));
        assert!(err.to_string().contains("put patterns in include instead"));
    }
}
