use anyhow::Result;
use dirs;
use std::path::{Path, PathBuf};

pub struct PathManager {
    config_dir: PathBuf,
    data_dir: PathBuf,
    log_dir: PathBuf,
}

impl PathManager {
    pub fn new() -> Result<Self> {
        let base_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
            .join("totalrecall");

        Ok(Self {
            config_dir: base_dir.clone(),
            data_dir: base_dir.join("data"),
            log_dir: base_dir.join("logs"),
        })
    }

    pub fn from_docker_env() -> Self {
        Self {
            config_dir: PathBuf::from("/app/config"),
            data_dir: PathBuf::from("/app/data"),
            log_dir: PathBuf::from("/app/logs"),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache")
    }

    pub fn cache_collect_dir(&self) -> PathBuf {
        self.cache_dir().join("collect")
    }

    pub fn cache_distribute_dir(&self) -> PathBuf {
        self.cache_dir().join("distribute")
    }

    pub fn cache_id_dir(&self) -> PathBuf {
        self.cache_dir().join("id")
    }

    pub fn cache_csv_dir(&self, source: &str) -> PathBuf {
        self.cache_dir().join("csv").join(source)
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn credentials_file(&self) -> PathBuf {
        self.config_dir.join("credentials.toml")
    }

    pub fn ensure_directories(&self) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.log_dir)?;
        std::fs::create_dir_all(&self.cache_dir())?;
        std::fs::create_dir_all(&self.cache_collect_dir())?;
        std::fs::create_dir_all(&self.cache_distribute_dir())?;
        std::fs::create_dir_all(&self.cache_id_dir())?;
        Ok(())
    }
}

impl Default for PathManager {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| Self::from_docker_env())
    }
}

