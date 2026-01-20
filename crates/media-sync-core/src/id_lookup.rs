use anyhow::Result;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::FutureExt;
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
    /// Queries all providers concurrently using FuturesUnordered and returns immediately
    /// when the first result contains the required ID. If no required ID is found in any
    /// result, all results are merged and returned.
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
        
        // Build FuturesUnordered for concurrent execution
        // Use BoxFuture to allow different async blocks to have the same type
        let mut futures: FuturesUnordered<BoxFuture<'_, (String, Result<Option<MediaIds>, SourceError>)>> = FuturesUnordered::new();
        let search_timestamps = self.search_timestamps.clone();
        let search_cooldown = self.search_cooldown;
        let (additional_tx, additional_rx) = mpsc::channel(10);
        
        for (provider_name, _priority) in &self.providers {
            // Find the source that provides this lookup
            for source_arc in sources {
                let source_arc = source_arc.clone();
                let provider_name = provider_name.clone();
                let title = title.to_string();
                let media_type = media_type.clone();
                let search_timestamps = search_timestamps.clone();
                
                // Create cache key for this search (per provider)
                let cache_key = Self::make_cache_key(&provider_name, &title, year, &media_type);
                
                // Check if we should skip this search due to cooldown
                let should_skip = {
                    let timestamps = search_timestamps.read().await;
                    if let Some(last_search) = timestamps.get(&cache_key) {
                        if let Ok(elapsed) = SystemTime::now().duration_since(*last_search) {
                            elapsed < search_cooldown
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
                    // Add a future that returns None immediately for skipped searches
                    futures.push(async move {
                        (provider_name, Ok(None))
                    }.boxed());
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
                
                // Create future for this provider lookup
                let future = async move {
                    let source_guard = source_arc.read().await;
                    let result = if source_guard.source_name() == provider_name.as_str() {
                        if let Some(provider) = source_guard.as_id_lookup_provider() {
                            provider.lookup_ids(&title, year, &media_type).await
                                .map_err(|e| SourceError::new(e.to_string()))
                        } else {
                            warn!("ID lookup: Source '{}' does not provide IdLookupProvider trait", &provider_name);
                            Ok(None)
                        }
                    } else {
                        Ok(None)
                    };
                    
                    match result {
                        Ok(Some(ids)) => {
                            tracing::trace!("ID lookup via {} found IDs: imdb={:?}, trakt={:?}, tmdb={:?}", 
                                   &provider_name, ids.imdb_id, ids.trakt_id, ids.tmdb_id);
                            (provider_name, Ok(Some(ids)))
                        }
                        Ok(None) => {
                            (provider_name, Ok(None))
                        }
                        Err(e) => {
                            warn!("ID lookup via {} failed for '{}' (year: {:?}): {}", 
                                  &provider_name, title, year, e);
                            (provider_name, Err(e))
                        }
                    }
                }.boxed();
                
                futures.push(future);
                break; // Found the source, move to next provider
            }
        }
        
        // Process results as they arrive using StreamExt
        let mut merged_ids = MediaIds::default();
        let mut errors = Vec::new();
        let mut remaining_results = Vec::new();
        let mut found_early = false;
        let mut early_result: Option<MediaIds> = None;
        
        while let Some((provider_name, result)) = futures.next().await {
            match result {
                Ok(Some(ids)) if Self::has_required_id(&ids, required_id_type) && !found_early => {
                    // First result with required ID - return immediately
                    found_early = true;
                    early_result = Some(ids);
                    // Break out of loop - remaining futures will complete in background
                    // but we won't process them since we have what we need
                    break;
                }
                Ok(Some(ids)) => {
                    if found_early {
                        // We already found the required ID, collect remaining for channel
                        remaining_results.push(ids);
                    } else {
                        merged_ids.merge(&ids);
                        remaining_results.push(ids);
                    }
                }
                Ok(None) => {
                    // No matches - this is normal
                }
                Err(e) => {
                    errors.push(format!("{}: {}", provider_name, e));
                }
            }
        }
        
        // If we found the required ID early, return immediately
        if let Some(ids) = early_result {
            // Spawn task to collect remaining results and send to channel
            let additional_tx_clone = additional_tx.clone();
            tokio::spawn(async move {
                // Continue processing any remaining futures
                // Note: futures stream is consumed, so we just send what we collected
                for ids in remaining_results {
                    let _ = additional_tx_clone.send(ids).await;
                }
            });
            return Ok((ids, Some(additional_rx)));
        }
        
        // No required ID found - return merged results
        // Send remaining results to channel for background processing
        for ids in remaining_results {
            let _ = additional_tx.send(ids).await;
        }
        
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

