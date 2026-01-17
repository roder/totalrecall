use anyhow::Result;
use dirs;
use std::path::{Path, PathBuf};

/// Get the container base path from environment variable, defaulting to "/app"
pub fn container_base_path() -> PathBuf {
    std::env::var("TOTALRECALL_BASE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/app"))
}

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
        let base = container_base_path();
        // In containers, match the default structure: config files at base level, data/logs in subdirs
        Self {
            config_dir: base.clone(),  // Config files go directly in base path
            data_dir: base.join("data"),
            log_dir: base.join("logs"),
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

    pub fn daemon_log_file(&self) -> PathBuf {
        self.log_dir.join("totalrecall.log")
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
        // Check if we're in a Docker container by looking for container base directory
        // This is created in the Containerfile, so its presence indicates Docker
        let base = container_base_path();
        if base.exists() {
            return Self::from_docker_env();
        }
        
        // Otherwise, use platform-specific paths (e.g., ~/.config/totalrecall on Linux)
        Self::new().unwrap_or_else(|_| Self::from_docker_env())
    }
}

