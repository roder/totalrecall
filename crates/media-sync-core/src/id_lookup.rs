use anyhow::Result;
use media_sync_models::{MediaIds, MediaType};
use media_sync_sources::{MediaSource, SourceError};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, warn};

/// Aggregator service that queries multiple ID lookup providers
/// 
/// This service is decoupled from specific sources and coordinates
/// lookups across all available providers, merging results.
pub struct IdLookupService {
    /// Providers sorted by priority (highest first)
    /// Maps source name to priority
    providers: Vec<(String, u8)>, // (source_name, priority)
    
    /// Cache of search timestamps per provider to avoid duplicate API calls
    /// Key: "{provider}:{title_lowercase}:{year}:{media_type}"
    /// Value: Last search timestamp
    search_timestamps: Arc<RwLock<HashMap<String, SystemTime>>>,
    
    /// Time window to skip searches (default: 7 days)
    search_cooldown: Duration,
}

impl IdLookupService {
    /// Create a new lookup service from available sources
    pub async fn new(sources: &[Arc<RwLock<Box<dyn MediaSource<Error = SourceError>>>>]) -> Self {
        let mut providers: Vec<(String, u8)> = Vec::new();
        
        for source in sources {
            let source_guard = source.read().await;
            if let Some(provider) = source_guard.as_id_lookup_provider() {
                if provider.is_lookup_available() {
                    providers.push((
                        provider.lookup_provider_name().to_string(),
                        provider.lookup_priority(),
                    ));
                    debug!("ID lookup service: Registered provider '{}' with priority {}", 
                           provider.lookup_provider_name(), provider.lookup_priority());
                } else {
                    debug!("ID lookup service: Provider '{}' is not available (likely not authenticated)", 
                           provider.lookup_provider_name());
                }
            }
        }
        
        // Sort by priority (highest first)
        providers.sort_by(|a, b| b.1.cmp(&a.1));
        
        if providers.is_empty() {
            warn!("ID lookup service: No lookup providers available! ID resolution by title will not work. Ensure at least one source (Plex, Trakt, or Simkl) is authenticated.");
        } else {
            debug!("ID lookup service: {} provider(s) available: {:?}", 
                   providers.len(), 
                   providers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>());
        }
        
        Self { 
            providers,
            search_timestamps: Arc::new(RwLock::new(HashMap::new())),
            search_cooldown: Duration::from_secs(7 * 24 * 3600), // 7 days
        }
    }
    
    /// Create a cache key for a search query (per provider)
    fn make_cache_key(provider: &str, title: &str, year: Option<u32>, media_type: &MediaType) -> String {
        let title_lower = title.trim().to_lowercase();
        let year_str = year.map(|y| y.to_string()).unwrap_or_else(|| "none".to_string());
        let type_str = format!("{:?}", media_type);
        format!("{}:{}:{}:{}", provider, title_lower, year_str, type_str)
    }
    
    /// Check if MediaIds has the required ID type
    fn has_required_id(ids: &MediaIds, required_id_type: &str) -> bool {
        ids.has_id(required_id_type)
    }
    
    /// Look up IDs using all available providers
    /// 
    /// Queries all providers concurrently and returns immediately when the first result
    /// contains the required ID. Remaining results are sent via a channel receiver for
    /// background processing and cache updates.
    /// 
    /// # Arguments
    /// * `sources` - All available media sources (to access providers)
    /// * `title` - Title to search for
    /// * `year` - Optional year
    /// * `media_type` - Type of media
    /// * `cached_ids` - Optional cached MediaIds to check for required ID before external lookup
    /// * `required_id_type` - The ID type we need (e.g., "imdb", "tmdb", "trakt"). Default: "imdb"
    /// 
    /// # Returns
    /// * `Ok((MediaIds, Some(Receiver)))` - Immediate result with channel receiver for additional results (when required ID found early)
    /// * `Ok((MediaIds, None))` - Merged results from all providers (when no required ID found)
    pub async fn lookup_ids(
        &self,
        sources: &[Arc<RwLock<Box<dyn MediaSource<Error = SourceError>>>>],
        title: &str,
        year: Option<u32>,
        media_type: &MediaType,
        cached_ids: Option<&MediaIds>,
        required_id_type: Option<&str>,
    ) -> Result<(MediaIds, Option<mpsc::Receiver<MediaIds>>)> {
        let required_id_type = required_id_type.unwrap_or("imdb");
        
        // First check: If we already have the required ID in cache, skip external lookups entirely
        if let Some(cached) = cached_ids {
            if Self::has_required_id(cached, required_id_type) {
                debug!("ID lookup: Required ID type '{}' already available in cache for '{}' (year: {:?}), skipping external lookups", 
                       required_id_type, title, year);
                return Ok((cached.clone(), None));
            }
        }
        
        // Check if any providers are available
        if self.providers.is_empty() {
            warn!("ID lookup: No providers available for '{}' (year: {:?}, type: {:?}). Cannot perform title-based lookup. Ensure at least one source (Plex, Trakt, or Simkl) is authenticated.", 
                  title, year, media_type);
            debug!("ID lookup: Provider list is empty - no lookup providers registered");
            return Ok((MediaIds::default(), None));
        }
        
        tracing::trace!("ID lookup: Attempting concurrent lookup for '{}' (year: {:?}, type: {:?}, required_id: {}) using {} provider(s): {:?}", 
               title, year, media_type, required_id_type, self.providers.len(),
               self.providers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>());
        
        // Create channels for results
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (additional_tx, additional_rx) = mpsc::channel(10);
        
        // Spawn all provider lookups as separate concurrent tasks
        let mut task_count = 0;
        let search_timestamps = self.search_timestamps.clone();
        let search_cooldown = self.search_cooldown;
        
        for (provider_name, _priority) in &self.providers {
            // Find the source that provides this lookup
            for source_arc in sources {
                let source_arc = source_arc.clone();
                let provider_name = provider_name.clone();
                let title = title.to_string();
                let media_type = media_type.clone();
                let result_tx = result_tx.clone();
                let search_timestamps = search_timestamps.clone();
                let required_id_type = required_id_type.to_string();
                
                // Create cache key for this search (per provider)
                let cache_key = Self::make_cache_key(&provider_name, &title, year, &media_type);
                
                // Check if we should skip this search due to cooldown
                let should_skip = {
                    let timestamps = search_timestamps.read().await;
                    if let Some(last_search) = timestamps.get(&cache_key) {
                        if let Ok(elapsed) = SystemTime::now().duration_since(*last_search) {
                            if elapsed < search_cooldown {
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                
                if should_skip {
                    let year_str = year.map(|y| y.to_string()).unwrap_or_else(|| "None".to_string());
                    let elapsed = search_timestamps.read().await
                        .get(&cache_key)
                        .and_then(|ts| SystemTime::now().duration_since(*ts).ok())
                        .map(|d| format!("{:.1} days", d.as_secs_f64() / 86400.0))
                        .unwrap_or_else(|| "unknown".to_string());
                    debug!("ID lookup: Skipping {} search for '{}' (year: {}) - last searched {} ago (within 7-day cooldown)", 
                           &provider_name, &title, year_str, elapsed);
                    // Send None to indicate skipped search
                    let _ = result_tx.send((provider_name, Ok(None))).await;
                    task_count += 1;
                    break;
                }
                
                // Log that we're executing the search
                let year_str = year.map(|y| y.to_string()).unwrap_or_else(|| "None".to_string());
                debug!("ID lookup: Executing {} search for '{}' (year: {}, type: {:?}, required_id: {})", 
                       &provider_name, &title, year_str, &media_type, required_id_type);
                
                // Record the search timestamp (even if it fails, we don't want to retry immediately)
                {
                    let mut timestamps = search_timestamps.write().await;
                    timestamps.insert(cache_key.clone(), SystemTime::now());
                }
                
                tokio::spawn(async move {
                    let source_guard = source_arc.read().await;
                    if source_guard.source_name() == provider_name.as_str() {
                        if let Some(provider) = source_guard.as_id_lookup_provider() {
                            match provider.lookup_ids(&title, year, &media_type).await {
                                Ok(Some(ids)) => {
                                    tracing::trace!("ID lookup via {} found IDs: imdb={:?}, trakt={:?}, tmdb={:?}", 
                                           &provider_name, ids.imdb_id, ids.trakt_id, ids.tmdb_id);
                                    let _ = result_tx.send((provider_name, Ok(Some(ids)))).await;
                                }
                                Ok(None) => {
                                    // No matches - this is normal, don't log
                                    let _ = result_tx.send((provider_name, Ok(None))).await;
                                }
                                Err(e) => {
                                    warn!("ID lookup via {} failed for '{}' (year: {:?}): {}", 
                                          &provider_name, title, year, e);
                                    let _ = result_tx.send((provider_name, Err(e))).await;
                                }
                            }
                        } else {
                            warn!("ID lookup: Source '{}' does not provide IdLookupProvider trait", &provider_name);
                            let _ = result_tx.send((provider_name, Ok(None))).await;
                        }
                    } else {
                        let _ = result_tx.send((provider_name, Ok(None))).await;
                    }
                });
                task_count += 1;
                break; // Found the source, move to next provider
            }
        }
        
        // Process results as they arrive
        let mut merged_ids = MediaIds::default();
        let mut errors = Vec::new();
        let mut found_required_id = false;
        let mut remaining_count = task_count;
        
        // Use a loop that we can break out of when we find the required ID
        loop {
            match result_rx.recv().await {
                Some((provider_name, result)) => {
                    remaining_count -= 1;
                    
                    match result {
                        Ok(Some(ids)) if Self::has_required_id(&ids, required_id_type) && !found_required_id => {
                            // First result with required ID - return immediately
                            found_required_id = true;
                            let additional_tx_clone = additional_tx.clone();
                            
                            // Drop the original result_tx to signal we're done with it
                            // (cloned senders in tasks will keep channel alive)
                            drop(result_tx);
                            
                            // Spawn task to forward remaining results to additional channel
                            // We can now move result_rx into the spawned task
                            tokio::spawn(async move {
                                while let Some((_, result)) = result_rx.recv().await {
                                    if let Ok(Some(ids)) = result {
                                        let _ = additional_tx_clone.send(ids).await; // Ignore errors if receiver dropped
                                    }
                                }
                            });
                            
                            return Ok((ids, Some(additional_rx)));
                        }
                        Ok(Some(ids)) => {
                            merged_ids.merge(&ids);
                        }
                        Ok(None) => {
                            // No matches - this is normal
                        }
                        Err(e) => {
                            errors.push(format!("{}: {}", provider_name, e));
                        }
                    }
                    
                    // If all tasks completed and we haven't found required ID, break
                    if remaining_count == 0 {
                        break;
                    }
                }
                None => {
                    // Channel closed (all senders dropped)
                    break;
                }
            }
        }
        
        // No required ID found - return merged results
        if !errors.is_empty() && merged_ids.is_empty() {
            return Err(anyhow::anyhow!(
                "All ID lookups failed: {}",
                errors.join("; ")
            ));
        }
        
        Ok((merged_ids, None))
    }
    
    /// Get list of available lookup providers
    pub fn available_providers(&self) -> Vec<&str> {
        self.providers.iter().map(|(name, _)| name.as_str()).collect()
    }
    
    /// Look up title, year, and IDs by IMDB ID (reverse lookup)
    /// 
    /// Queries providers in priority order to find title/year from an IMDB ID.
    /// This is useful when we have an IMDB ID but need the title for discover provider searches.
    /// 
    /// # Arguments
    /// * `sources` - All available media sources (to access providers)
    /// * `imdb_id` - IMDB ID to search for (e.g., "tt1234567")
    /// * `media_type` - Type of media
    /// 
    /// # Returns
    /// * `Ok(Some((title, year, MediaIds)))` - Title, year, and IDs found
    /// * `Ok(None)` - No match found
    pub async fn lookup_by_imdb_id(
        &self,
        sources: &[Arc<RwLock<Box<dyn MediaSource<Error = SourceError>>>>],
        imdb_id: &str,
        media_type: &MediaType,
    ) -> Result<Option<(String, Option<u32>, MediaIds)>> {
        // Check if any providers are available
        if self.providers.is_empty() {
            debug!("ID reverse lookup: No providers available for imdb_id={}, type={:?}", imdb_id, media_type);
            return Ok(None);
        }
        
        tracing::trace!("ID reverse lookup: Attempting lookup for imdb_id={}, type={:?} using {} provider(s): {:?}", 
               imdb_id, media_type, self.providers.len(),
               self.providers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>());
        
        // Query providers in priority order
        for (provider_name, _priority) in &self.providers {
            // Find the source that provides this lookup
            for source_arc in sources {
                let source_guard = source_arc.read().await;
                if source_guard.source_name() == provider_name.as_str() {
                    if let Some(provider) = source_guard.as_id_lookup_provider() {
                        debug!("ID reverse lookup: Executing {} search for imdb_id={}, type={:?}", 
                               provider_name, imdb_id, media_type);
                        match provider.lookup_by_imdb_id(imdb_id, media_type).await {
                            Ok(Some((title, year, ids))) => {
                                tracing::trace!("ID reverse lookup via {} found: title='{}', year={:?}, imdb={:?}, trakt={:?}, tmdb={:?}", 
                                       provider_name, title, year, ids.imdb_id, ids.trakt_id, ids.tmdb_id);
                                return Ok(Some((title, year, ids)));
                            }
                            Ok(None) => {
                                // No matches - continue to next provider
                            }
                            Err(e) => {
                                warn!("ID reverse lookup via {} failed for imdb_id={}: {}", 
                                      provider_name, imdb_id, e);
                                // Continue to next provider
                            }
                        }
                    }
                    break; // Found the source, move to next provider
                }
            }
        }
        
        Ok(None)
    }
}

