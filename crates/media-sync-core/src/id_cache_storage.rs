use anyhow::Result;
use bincode::{serialize, deserialize};
use flate2::{Compression, write::GzEncoder, read::GzDecoder};
use std::io::{Write, Read};
use std::path::{Path, PathBuf};
use tracing::{info, debug, warn};
use crate::id_cache::IdCache;

/// Efficient storage for ID cache
/// 
/// Uses binary format (bincode) with optional gzip compression for fast
/// serialization and reduced storage size.
pub struct IdCacheStorage {
    cache_path: PathBuf,
    use_compression: bool,
}

impl IdCacheStorage {
    pub fn new(cache_id_dir: &Path) -> Self {
        Self {
            cache_path: cache_id_dir.join("id_mappings.bin"),
            use_compression: true, // Enable by default for large caches
        }
    }
    
    /// Load cache from disk
    pub fn load(&self) -> Result<IdCache> {
        if !self.cache_path.exists() {
            debug!("ID cache file does not exist, creating new cache");
            return Ok(IdCache::new());
        }
        
        let start = std::time::Instant::now();
        let data = std::fs::read(&self.cache_path)?;
        
        let decoded = if self.use_compression {
            let mut decoder = GzDecoder::new(&data[..]);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed)?;
            decompressed
        } else {
            data
        };
        
        // Try to deserialize as Vec<MediaIds> first, then rebuild cache
        // If deserialization fails (e.g., format changed), start with empty cache
        let entries: Vec<media_sync_models::MediaIds> = match deserialize(&decoded) {
            Ok(entries) => entries,
            Err(e) => {
                // Cache format is incompatible (likely due to schema changes)
                // Backup the old cache and start fresh
                let backup_path = self.cache_path.with_extension("bin.bak");
                if let Err(backup_err) = std::fs::copy(&self.cache_path, &backup_path) {
                    warn!(
                        "Failed to backup incompatible cache file: {}. Starting with empty cache.",
                        backup_err
                    );
                } else {
                    info!(
                        "Cache format incompatible (error: {}). Backed up old cache to {:?} and starting with empty cache.",
                        e,
                        backup_path
                    );
                }
                return Ok(IdCache::new());
            }
        };
        
        let mut cache = IdCache::new();
        let mut entries_with_metadata = 0;
        for ids in entries {
            if ids.title.is_some() && ids.media_type.is_some() {
                entries_with_metadata += 1;
            }
            cache.insert(ids);
        }
        
        // Rebuild title/year index to ensure all entries with metadata are indexed
        // This is important because entries cached in previous runs might not have been indexed
        cache.rebuild_title_year_index();
        
        let title_year_index_size = cache.title_year_index_size();
        info!(
            "Loaded ID cache: {} total entries ({} with title/media_type metadata), {} entries in title/year index in {:?}",
            cache.len(),
            entries_with_metadata,
            title_year_index_size,
            start.elapsed()
        );
        
        if entries_with_metadata > 0 && title_year_index_size == 0 {
            warn!(
                "ID cache: {} entries have metadata but title/year index is empty after rebuild. This may indicate a cache format issue.",
                entries_with_metadata
            );
        } else if entries_with_metadata > title_year_index_size {
            warn!(
                "ID cache: {} entries have metadata but only {} are in title/year index. Some entries may be missing year or have mismatched media_type.",
                entries_with_metadata,
                title_year_index_size
            );
        }
        
        Ok(cache)
    }
    
    /// Save cache to disk
    pub fn save(&self, cache: &IdCache) -> Result<()> {
        let start = std::time::Instant::now();
        
        // Get all entries for serialization
        let entries = cache.all_entries();
        
        // Serialize to binary
        let serialized = serialize(&entries)?;
        
        let encoded = if self.use_compression {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&serialized)?;
            encoder.finish()?
        } else {
            serialized
        };
        
        // Atomic write: write to temp file, then rename
        let temp_path = self.cache_path.with_extension("tmp");
        std::fs::write(&temp_path, encoded)?;
        std::fs::rename(&temp_path, &self.cache_path)?;
        
        info!(
            "Saved ID cache: {} entries in {:?}",
            cache.len(),
            start.elapsed()
        );
        
        Ok(())
    }
    
    /// Get cache file size
    pub fn size(&self) -> Result<u64> {
        if self.cache_path.exists() {
            Ok(std::fs::metadata(&self.cache_path)?.len())
        } else {
            Ok(0)
        }
    }
    
    /// Set whether to use compression
    pub fn set_compression(&mut self, use_compression: bool) {
        self.use_compression = use_compression;
    }
    
    /// Check if cache file exists
    pub fn cache_exists(&self) -> bool {
        self.cache_path.exists()
    }
}

