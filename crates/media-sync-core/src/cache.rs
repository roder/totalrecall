use anyhow::{anyhow, Result};
use media_sync_config::PathManager;
use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem, ExcludedItem};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct CacheManager {
    collect_dir: PathBuf,
    distribute_dir: PathBuf,
}

impl CacheManager {
    pub fn new(path_manager: &PathManager) -> Result<Self> {
        let collect_dir = path_manager.cache_collect_dir();
        let distribute_dir = path_manager.cache_distribute_dir();
        std::fs::create_dir_all(&collect_dir)?;
        std::fs::create_dir_all(&distribute_dir)?;
        Ok(Self { collect_dir, distribute_dir })
    }

    fn get_cache_path(&self, source: &str, data_type: &str) -> PathBuf {
        self.collect_dir.join(source).join(format!("{}.json", data_type))
    }

    fn get_distribute_path(&self, source: &str, data_type: &str) -> PathBuf {
        self.distribute_dir.join(source).join(format!("{}.json", data_type))
    }

    pub fn cache_exists(&self, source: &str, data_type: &str) -> bool {
        self.get_cache_path(source, data_type).exists()
    }

    pub fn load_watchlist(&self, source: &str) -> Result<Option<Vec<WatchlistItem>>> {
        self.load_source_data(source, "watchlist")
    }

    pub fn save_watchlist(&self, source: &str, data: &[WatchlistItem]) -> Result<()> {
        self.save_source_data(source, "watchlist", data)
    }

    pub fn load_ratings(&self, source: &str) -> Result<Option<Vec<Rating>>> {
        self.load_source_data(source, "ratings")
    }

    pub fn save_ratings(&self, source: &str, data: &[Rating]) -> Result<()> {
        self.save_source_data(source, "ratings", data)
    }

    pub fn load_reviews(&self, source: &str) -> Result<Option<Vec<Review>>> {
        self.load_source_data(source, "reviews")
    }

    pub fn save_reviews(&self, source: &str, data: &[Review]) -> Result<()> {
        self.save_source_data(source, "reviews", data)
    }

    pub fn load_watch_history(&self, source: &str) -> Result<Option<Vec<WatchHistory>>> {
        self.load_source_data(source, "watch_history")
    }

    pub fn save_watch_history(&self, source: &str, data: &[WatchHistory]) -> Result<()> {
        self.save_source_data(source, "watch_history", data)
    }

    pub fn load_excluded(&self, source: &str) -> Result<Option<Vec<ExcludedItem>>> {
        self.load_source_data(source, "excluded")
    }

    pub fn save_excluded(&self, source: &str, data: &[ExcludedItem]) -> Result<()> {
        // Excluded items from distribution phase (filtered by timestamp/source) go to distribute directory
        self.save_distribute_data(source, "excluded", data)
    }
    
    pub fn save_excluded_collect(&self, source: &str, data: &[ExcludedItem]) -> Result<()> {
        // Excluded items from collect phase (unsupported media types) go to collect directory
        self.save_source_data(source, "excluded", data)
    }

    fn load_source_data<T>(&self, source: &str, data_type: &str) -> Result<Option<Vec<T>>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let cache_path = self.get_cache_path(source, data_type);
        
        if !cache_path.exists() {
            debug!("Cache miss: {} {} (file does not exist)", source, data_type);
            return Ok(None);
        }

        match std::fs::read_to_string(&cache_path) {
            Ok(content) => {
                match serde_json::from_str::<Vec<T>>(&content) {
                    Ok(data) => {
                        info!("Cache hit: {} {} (loaded {} items)", source, data_type, data.len());
                        Ok(Some(data))
                    }
                    Err(e) => {
                        warn!(
                            "Cache corruption detected for {} {}: {}. Deleting corrupted file.",
                            source, data_type, e
                        );
                        if let Err(rm_err) = std::fs::remove_file(&cache_path) {
                            warn!("Failed to delete corrupted cache file: {}", rm_err);
                        }
                        Ok(None)
                    }
                }
            }
            Err(e) => {
                warn!("Failed to read cache file for {} {}: {}", source, data_type, e);
                Ok(None)
            }
        }
    }

    fn save_source_data<T>(&self, source: &str, data_type: &str, data: &[T]) -> Result<()>
    where
        T: Serialize,
    {
        let cache_path = self.get_cache_path(source, data_type);
        
        // Ensure source directory exists
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        match serde_json::to_string_pretty(data) {
            Ok(json) => {
                match std::fs::write(&cache_path, json) {
                    Ok(_) => {
                        debug!("Cache saved: {} {} (saved {} items)", source, data_type, data.len());
                        Ok(())
                    }
                    Err(e) => {
                        warn!("Failed to write cache file for {} {}: {}", source, data_type, e);
                        Err(anyhow!("Failed to write cache: {}", e))
                    }
                }
            }
            Err(e) => {
                warn!("Failed to serialize cache data for {} {}: {}", source, data_type, e);
                Err(anyhow!("Failed to serialize cache: {}", e))
            }
        }
    }

    pub fn save_distribute_data<T>(&self, source: &str, data_type: &str, data: &[T]) -> Result<()>
    where
        T: Serialize,
    {
        let distribute_path = self.get_distribute_path(source, data_type);
        
        // Ensure source directory exists
        if let Some(parent) = distribute_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        match serde_json::to_string_pretty(data) {
            Ok(json) => {
                match std::fs::write(&distribute_path, json) {
                    Ok(_) => {
                        debug!("Distribute data saved: {} {} (saved {} items)", source, data_type, data.len());
                        Ok(())
                    }
                    Err(e) => {
                        warn!("Failed to write distribute file for {} {}: {}", source, data_type, e);
                        Err(anyhow!("Failed to write distribute data: {}", e))
                    }
                }
            }
            Err(e) => {
                warn!("Failed to serialize distribute data for {} {}: {}", source, data_type, e);
                Err(anyhow!("Failed to serialize distribute data: {}", e))
            }
        }
    }

    pub fn load_distribute_data<T>(&self, source: &str, data_type: &str) -> Result<Option<Vec<T>>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let distribute_path = self.get_distribute_path(source, data_type);
        
        if !distribute_path.exists() {
            debug!("Distribute data miss: {} {} (file does not exist)", source, data_type);
            return Ok(None);
        }

        match std::fs::read_to_string(&distribute_path) {
            Ok(content) => {
                match serde_json::from_str::<Vec<T>>(&content) {
                    Ok(data) => {
                        info!("Distribute data hit: {} {} (loaded {} items)", source, data_type, data.len());
                        Ok(Some(data))
                    }
                    Err(e) => {
                        warn!(
                            "Distribute data corruption detected for {} {}: {}. Deleting corrupted file.",
                            source, data_type, e
                        );
                        if let Err(rm_err) = std::fs::remove_file(&distribute_path) {
                            warn!("Failed to delete corrupted distribute file: {}", rm_err);
                        }
                        Ok(None)
                    }
                }
            }
            Err(e) => {
                warn!("Failed to read distribute file for {} {}: {}", source, data_type, e);
                Ok(None)
            }
        }
    }

    pub fn clear_cache(&self) -> Result<()> {
        if self.collect_dir.exists() {
            std::fs::remove_dir_all(&self.collect_dir)?;
            std::fs::create_dir_all(&self.collect_dir)?;
            info!("Cleared collect cache directory: {:?}", self.collect_dir);
        }
        if self.distribute_dir.exists() {
            std::fs::remove_dir_all(&self.distribute_dir)?;
            std::fs::create_dir_all(&self.distribute_dir)?;
            info!("Cleared distribute cache directory: {:?}", self.distribute_dir);
        }
        Ok(())
    }
}


