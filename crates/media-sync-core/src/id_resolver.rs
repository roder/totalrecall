use anyhow::Result;
use media_sync_models::{MediaIds, MediaType};
use media_sync_sources::{MediaSource, SourceError};
use std::path::Path;
use tracing::{debug, warn};
use crate::id_cache::IdCache;
use crate::id_cache_storage::IdCacheStorage;
use crate::id_lookup::IdLookupService;

/// Configuration for ID resolver behavior
#[derive(Clone)]
pub struct IdResolverConfig {
    /// Save incrementally (only changes) or full save
    pub incremental_saves: bool,
    
    /// Full save interval (every N inserts, 0 = always incremental)
    pub full_save_interval: usize,
}

impl Default for IdResolverConfig {
    fn default() -> Self {
        Self {
            incremental_saves: true,
            full_save_interval: 0, // Always incremental by default
        }
    }
}

/// Centralized ID resolution service
/// 
/// This service combines:
/// - In-memory cache (IdCache) for fast lookups
/// - Persistent storage (IdCacheStorage) for durability
/// - Lookup service (IdLookupService) for finding missing IDs
pub struct IdResolver {
    /// In-memory cache with multi-index structure
    cache: IdCache,
    
    /// Storage layer
    storage: IdCacheStorage,
    
    /// Lookup service
    lookup_service: IdLookupService,
    
    /// Configuration
    config: IdResolverConfig,
    
    /// Track number of inserts since last full save
    inserts_since_save: usize,
}

impl IdResolver {
    pub fn new(
        cache_dir: &Path,
        sources: &[Box<dyn MediaSource<Error = SourceError>>],
        config: IdResolverConfig,
    ) -> Result<Self> {
        let storage = IdCacheStorage::new(cache_dir);
        
        // Load cache (lazy - only if file exists)
        let cache = if storage.cache_exists() {
            storage.load()?
        } else {
            IdCache::new()
        };
        
        let lookup_service = IdLookupService::new(sources);
        
        Ok(Self {
            cache,
            storage,
            lookup_service,
            config,
            inserts_since_save: 0,
        })
    }
    
    /// Resolve IDs for an item from a source
    /// 
    /// This is the main entry point. It:
    /// 1. Tries to find IDs in cache first
    /// 2. Falls back to title-based lookup if IDs are missing
    /// 3. Merges with existing cache entries
    /// 4. Updates cache
    pub async fn resolve_ids_for_item(
        &mut self,
        sources: &[Box<dyn MediaSource<Error = SourceError>>],
        title: &str,
        year: Option<u32>,
        media_type: &MediaType,
        existing_imdb_id: Option<&str>,
    ) -> Result<MediaIds> {
        let mut ids = MediaIds::default();
        
        // Step 1: If we already have an imdb_id, use it as starting point
        if let Some(imdb_id) = existing_imdb_id.filter(|id| !id.is_empty()) {
            ids.imdb_id = Some(imdb_id.to_string());
            // Check cache for existing mappings
            if let Some(cached) = self.cache.find_by_any_id(imdb_id) {
                return Ok((*cached).clone());
            }
        }
        
        // Step 2: If IDs missing, check persistent cache by title/year first
        if ids.is_empty() || ids.imdb_id.is_none() {
            // Check persistent cache by title/year before doing external lookup
            if let Some(cached) = self.cache.find_by_title_year(title, year, media_type) {
                debug!("ID resolver: Found '{}' (year: {:?}) in persistent cache by title/year, using cached IDs", title, year);
                return Ok((*cached).clone());
            }
            
            // Debug: Log why title/year lookup failed
            let index_size = self.cache.title_year_index_size();
            let cache_size = self.cache.len();
            debug!("ID resolver: Title/year cache miss for '{}' (year: {:?}, type: {:?}). Cache has {} total entries, {} title/year indexed entries. Attempting external lookup.", 
                   title, year, media_type, cache_size, index_size);
            
            debug!("ID resolver: Attempting lookup for '{}' (year: {:?}, type: {:?})", title, year, media_type);
                
                // Collect detailed lookup information
                let available_providers = self.lookup_service.available_providers();
                let provider_count = available_providers.len();
                
                match self.lookup_service.lookup_ids(sources, title, year, media_type).await {
                Ok(looked_up_ids) => {
                    if looked_up_ids.is_empty() {
                        warn!("ID resolution for '{}' (year: {:?}) returned empty IDs. This may be because: 1) No lookup providers are available (check authentication), 2) The title was not found in any provider, or 3) The providers returned no IDs for this title.", 
                              title, year);
                        debug!("ID resolver: Lookup returned empty MediaIds for '{}'. Queried {} provider(s): {:?}", 
                               title, provider_count, available_providers);
                    } else {
                        // After external lookup, check if any of the returned IDs are already in cache
                        // This avoids redundant lookups when the same item is resolved multiple times
                        let mut cached_ids_found = false;
                        
                        // Try to find in cache using any of the returned IDs
                        if let Some(ref imdb) = looked_up_ids.imdb_id {
                            if let Some(cached) = self.cache.find_by_any_id(imdb) {
                                // Found in cache - merge looked up IDs into cached (cached may have more complete data)
                                let mut merged = (*cached).clone();
                                merged.merge(&looked_up_ids);
                                // Ensure metadata is set so it's in the title/year index
                                if merged.title.is_none() {
                                    merged.title = Some(title.to_string());
                                }
                                if merged.year.is_none() {
                                    merged.year = year;
                                }
                                if merged.media_type.is_none() {
                                    merged.media_type = Some(media_type.clone());
                                }
                                // Re-insert immediately with metadata to update the title/year index
                                self.cache.insert(merged.clone());
                                self.inserts_since_save += 1;
                                ids = merged;
                                cached_ids_found = true;
                                debug!("ID resolver: Found '{}' in cache (via imdb_id={}) after external lookup, updating with metadata", title, imdb);
                            }
                        }
                        
                        // If not found via imdb_id, try other IDs
                        if !cached_ids_found {
                            if let Some(trakt_id) = looked_up_ids.trakt_id {
                                let trakt_str = format!("trakt:{}", trakt_id);
                                if let Some(cached) = self.cache.find_by_any_id(&trakt_str) {
                                    let mut merged = (*cached).clone();
                                    merged.merge(&looked_up_ids);
                                    // Ensure metadata is set so it's in the title/year index
                                    if merged.title.is_none() {
                                        merged.title = Some(title.to_string());
                                    }
                                    if merged.year.is_none() {
                                        merged.year = year;
                                    }
                                    if merged.media_type.is_none() {
                                        merged.media_type = Some(media_type.clone());
                                    }
                                    // Re-insert immediately with metadata to update the title/year index
                                    self.cache.insert(merged.clone());
                                    self.inserts_since_save += 1;
                                    ids = merged;
                                    cached_ids_found = true;
                                    debug!("ID resolver: Found '{}' in cache (via trakt_id={}) after external lookup, updating with metadata", title, trakt_id);
                                }
                            }
                        }
                        
                        if !cached_ids_found {
                            if let Some(tmdb_id) = looked_up_ids.tmdb_id {
                                let tmdb_str = format!("tmdb:{}", tmdb_id);
                                if let Some(cached) = self.cache.find_by_any_id(&tmdb_str) {
                                    let mut merged = (*cached).clone();
                                    merged.merge(&looked_up_ids);
                                    // Ensure metadata is set so it's in the title/year index
                                    if merged.title.is_none() {
                                        merged.title = Some(title.to_string());
                                    }
                                    if merged.year.is_none() {
                                        merged.year = year;
                                    }
                                    if merged.media_type.is_none() {
                                        merged.media_type = Some(media_type.clone());
                                    }
                                    // Re-insert immediately with metadata to update the title/year index
                                    self.cache.insert(merged.clone());
                                    self.inserts_since_save += 1;
                                    ids = merged;
                                    cached_ids_found = true;
                                    debug!("ID resolver: Found '{}' in cache (via tmdb_id={}) after external lookup, updating with metadata", title, tmdb_id);
                                }
                            }
                        }
                        
                        if !cached_ids_found {
                            // Not in cache, use the looked up IDs
                            ids.merge(&looked_up_ids);
                            debug!("ID resolution for '{}' found IDs: imdb={:?}, trakt={:?}, tmdb={:?}, tvdb={:?}", 
                                   title, looked_up_ids.imdb_id, looked_up_ids.trakt_id, looked_up_ids.tmdb_id, looked_up_ids.tvdb_id);
                        }
                    }
                }
                Err(e) => {
                    warn!("ID lookup failed for '{}': {}. Queried {} provider(s): {:?}", 
                          title, e, provider_count, available_providers);
                    debug!("ID resolver: Lookup error details for '{}': {:?}", title, e);
                }
            }
        }
        
        // Step 3: Update cache with title/year metadata for future lookups
        // Only insert if we haven't already inserted it above (when found in cache)
        if !ids.is_empty() {
            // Check if metadata is already set (meaning we already inserted it above)
            let needs_insert = ids.title.is_none() || ids.year.is_none() || ids.media_type.is_none();
            if needs_insert {
                // Add title/year metadata to IDs before caching
                let mut ids_with_metadata = ids.clone();
                ids_with_metadata.title = Some(title.to_string());
                ids_with_metadata.year = year;
                ids_with_metadata.media_type = Some(media_type.clone());
                self.cache.insert(ids_with_metadata);
                self.inserts_since_save += 1;
            }
        }
        
        Ok(ids)
    }
    
    /// Find MediaIds by any ID type
    pub fn find_by_any_id(&self, id: &str) -> Option<MediaIds> {
        self.cache.find_by_any_id(id).map(|arc| (*arc).clone())
    }
    
    /// Cache IDs from collected data to avoid remote lookups
    /// This is used during resolution phase to cache IDs that were extracted from source metadata
    pub fn cache_ids(&mut self, ids: MediaIds) {
        self.cache_ids_with_metadata(ids, None, None, None);
    }
    
    /// Cache IDs with optional title/year/media_type metadata for title-based lookups
    pub fn cache_ids_with_metadata(
        &mut self,
        mut ids: MediaIds,
        title: Option<&str>,
        year: Option<u32>,
        media_type: Option<&MediaType>,
    ) {
        if !ids.is_empty() {
            // Set metadata if provided (don't overwrite existing)
            if let Some(title) = title {
                if ids.title.is_none() {
                    ids.title = Some(title.to_string());
                }
            }
            if let Some(year) = year {
                if ids.year.is_none() {
                    ids.year = Some(year);
                }
            }
            if let Some(media_type) = media_type {
                if ids.media_type.is_none() {
                    ids.media_type = Some(media_type.clone());
                }
            }
            self.cache.insert(ids);
            self.inserts_since_save += 1;
        }
    }
    
    /// Periodic save (call from sync orchestrator)
    pub fn save_if_dirty(&mut self) -> Result<()> {
        if !self.cache.is_dirty() {
            return Ok(());
        }
        
        // Determine if we should do a full save
        let should_full_save = if self.config.full_save_interval > 0 {
            self.inserts_since_save >= self.config.full_save_interval
        } else {
            false // Always incremental if interval is 0
        };
        
        if should_full_save {
            self.storage.save(&self.cache)?;
            self.inserts_since_save = 0;
        } else if self.config.incremental_saves {
            // For incremental saves, we still do a full save (simplified implementation)
            // In a more advanced implementation, we could track modified entries
            self.storage.save(&self.cache)?;
        } else {
            self.storage.save(&self.cache)?;
        }
        
        self.cache.mark_clean();
        Ok(())
    }
    
    /// Get cache statistics
    pub fn cache_stats(&self) -> (usize, bool) {
        (self.cache.len(), self.cache.is_dirty())
    }
    
    /// Look up title, year, and IDs by IMDB ID (reverse lookup)
    /// 
    /// This is used when we have an IMDB ID but need the title/year for discover provider searches.
    /// Queries external lookup services to find the title and year.
    pub async fn lookup_by_imdb_id(
        &mut self,
        sources: &[Box<dyn MediaSource<Error = SourceError>>],
        imdb_id: &str,
        media_type: &MediaType,
    ) -> Result<Option<(String, Option<u32>, MediaIds)>> {
        // First check cache
        if let Some(cached_ids) = self.cache.find_by_any_id(imdb_id) {
            if let (Some(title), year) = (cached_ids.title.clone(), cached_ids.year) {
                debug!("ID reverse lookup: Found '{}' (year: {:?}) in cache for imdb_id={}", title, year, imdb_id);
                return Ok(Some((title, year, (*cached_ids).clone())));
            }
        }
        
        // Not in cache or missing title - try external lookup
        match self.lookup_service.lookup_by_imdb_id(sources, imdb_id, media_type).await {
            Ok(Some((title, year, mut ids))) => {
                // Cache the result with metadata
                ids.title = Some(title.clone());
                ids.year = year;
                ids.media_type = Some(media_type.clone());
                self.cache.insert(ids.clone());
                self.inserts_since_save += 1;
                
                debug!("ID reverse lookup: Found '{}' (year: {:?}) via external lookup for imdb_id={}", title, year, imdb_id);
                Ok(Some((title, year, ids)))
            }
            Ok(None) => {
                debug!("ID reverse lookup: No match found for imdb_id={}", imdb_id);
                Ok(None)
            }
            Err(e) => {
                warn!("ID reverse lookup failed for imdb_id={}: {}", imdb_id, e);
                Err(e)
            }
        }
    }
    
    /// Get list of available lookup providers
    pub fn available_lookup_providers(&self) -> Vec<&str> {
        self.lookup_service.available_providers()
    }
}

