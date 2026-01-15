use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use media_sync_config::PathManager;
use media_sync_models::{MediaIds, Rating, Review, WatchHistory, WatchlistItem, NormalizedStatus, ExcludedItem, MediaType};
use media_sync_sources::{MediaSource, RatingNormalization, SourceError};
use serde::Serialize;
use serde_json;
use crate::cache::CacheManager;
use crate::diff::{filter_items_by_imdb_id, filter_missing_imdb_ids, filter_ratings_by_imdb_id_and_value, filter_reviews_by_imdb_id_and_content};
use crate::resolution::{SourceData, ResolvedData};
use crate::distribution::{DistributionStrategy, DistributionResult, DefaultDistributionStrategy, TraktDistributionStrategy, ImdbDistributionStrategy, SimklDistributionStrategy, PlexDistributionStrategy};
use crate::id_resolver::{IdResolver, IdResolverConfig};
use std::time::{Duration, Instant};
use tracing::{debug, info, instrument, warn};

/// Registry mapping source names to their indices in the sources vector
struct SourceRegistry {
    name_to_index: std::collections::HashMap<String, usize>,
}

impl SourceRegistry {
    fn new(sources: &[Box<dyn MediaSource<Error = SourceError>>]) -> Self {
        let mut name_to_index = std::collections::HashMap::new();
        for (index, source) in sources.iter().enumerate() {
            name_to_index.insert(source.source_name().to_string(), index);
        }
        Self { name_to_index }
    }

    fn get_index(&self, source_name: &str) -> Option<usize> {
        self.name_to_index.get(source_name).copied()
    }

    fn contains(&self, source_name: &str) -> bool {
        self.name_to_index.contains_key(source_name)
    }
}

pub struct SyncOrchestrator {
    sources: Vec<Box<dyn MediaSource<Error = SourceError>>>,
    registry: SourceRegistry,
    sync_options: SyncOptions,
    config_sync_options: Option<media_sync_config::SyncOptions>,
    resolution_config: media_sync_config::ResolutionConfig,
    use_cache: std::collections::HashSet<String>,
    dry_run_sources: std::collections::HashSet<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncOptions {
    pub sync_watchlist: bool,
    pub sync_ratings: bool,
    pub sync_reviews: bool,
    pub sync_watch_history: bool,
    pub force_full_sync: bool,
}

pub struct SyncResult {
    pub items_synced: usize,
    pub duration: Duration,
    pub errors: Vec<String>,
}

struct CollectedData {
    sources: Vec<(String, SourceData)>,
}

/// Data structure for dry-run JSON output
/// Captures prepared data after distribution strategy filtering
#[derive(Debug, Serialize)]
struct DryRunData {
    source: String,
    timestamp: DateTime<Utc>,
    sync_options: SyncOptions,
    watchlist: Vec<WatchlistItem>,
    watchlist_to_history: Vec<WatchHistory>, // For sources that split watchlist
    ratings: Vec<Rating>,
    reviews: Vec<Review>,
    watch_history: Vec<WatchHistory>,
    removal_list: Vec<WatchlistItem>, // Items that would be removed (watched or old)
}

impl SyncOrchestrator {
    pub fn new(
        sources: Vec<Box<dyn MediaSource<Error = SourceError>>>,
        resolution_config: media_sync_config::ResolutionConfig,
    ) -> anyhow::Result<Self> {
        // Validate source_preference
        if resolution_config.source_preference.is_empty() {
            return Err(anyhow::anyhow!("source_preference is required and cannot be empty"));
        }
        
        // Build registry
        let registry = SourceRegistry::new(&sources);
        
        // Validate that all sources in source_preference are configured
        for source_name in &resolution_config.source_preference {
            if !registry.contains(source_name) {
                return Err(anyhow::anyhow!(
                    "Source '{}' is in source_preference but not provided in sources list",
                    source_name
                ));
            }
        }
        
        Ok(Self {
            sources,
            registry,
            sync_options: SyncOptions::default(),
            config_sync_options: None,
            resolution_config,
            use_cache: std::collections::HashSet::new(),
            dry_run_sources: std::collections::HashSet::new(),
        })
    }
    
    pub fn with_resolution_config(mut self, config: media_sync_config::ResolutionConfig) -> Self {
        self.resolution_config = config;
        self
    }

    // Note: get_source_by_name and get_source_mut_by_name removed due to lifetime issues
    // Use find_source_index and sources.get/get_mut directly instead

    /// Find the index of a source by name
    fn find_source_index(&self, source_name: &str) -> Option<usize> {
        self.registry.get_index(source_name)
    }
    
    /// Helper to set force_full_sync for sources that support incremental sync
    /// Uses the CapabilityRegistry pattern for safe capability access
    fn set_force_full_sync_for_source(sources: &mut [Box<dyn MediaSource<Error = SourceError>>], source_index: usize, force: bool) {
        if let Some(source_box) = sources.get_mut(source_index) {
            // Use capability registry to safely access IncrementalSync
            if let Some(incremental_sync) = source_box.as_mut().as_incremental_sync() {
                incremental_sync.set_force_full_sync(force);
            }
        }
    }

    pub fn with_sync_options(mut self, options: SyncOptions) -> Self {
        self.sync_options = options;
        self
    }

    pub fn with_config_sync_options(mut self, options: media_sync_config::SyncOptions) -> Self {
        self.config_sync_options = Some(options);
        self
    }

    pub fn with_use_cache(mut self, use_cache: std::collections::HashSet<String>) -> Self {
        self.use_cache = use_cache;
        self
    }

    pub fn with_dry_run(mut self, sources: std::collections::HashSet<String>) -> Self {
        self.dry_run_sources = sources;
        self
    }

    pub fn enabled_sources(&self) -> Vec<&str> {
        // Return sources in source_preference order
        let mut sources = Vec::new();
        for source_name in &self.resolution_config.source_preference {
            sources.push(source_name.as_str());
        }
        sources
    }

    #[instrument(skip(self))]
    pub async fn sync(&mut self) -> Result<SyncResult> {
        let start = Instant::now();
        let mut errors = Vec::new();

        info!(
            operation = "sync_start",
            sources = ?self.enabled_sources(),
            "Starting sync operation (Collect → Resolve → Distribute)"
        );

        // Authenticate sources in source_preference order (first source = fail-fast)
        for (idx, source_name) in self.resolution_config.source_preference.iter().enumerate() {
            let is_first = idx == 0;
            if let Some(source_index) = self.find_source_index(source_name) {
                if let Some(source) = self.sources.get_mut(source_index) {
                    if let Err(e) = source.as_mut().authenticate().await {
                        let error_msg = format!("Failed to authenticate to {}: {}", source_name, e);
                        errors.push(error_msg.clone());
                        tracing::error!(
                            operation = "auth",
                            source = source_name,
                            status = "error",
                            error = %e,
                            "Failed to authenticate to {}",
                            source_name
                        );
                        if is_first {
                            return Ok(SyncResult {
                                items_synced: 0,
                                duration: start.elapsed(),
                                errors,
                            });
                        }
                    }
                } else {
                    errors.push(format!("Source '{}' not found at index {}", source_name, source_index));
                }
            } else {
                errors.push(format!("Source '{}' not found in registry", source_name));
            }
        }

        // PHASE 1: COLLECT - Fetch all data from all sources
        let path_manager = PathManager::default();
        let cache_manager = CacheManager::new(&path_manager)
            .map_err(|e| {
                let error_msg = format!("Failed to initialize cache manager: {}", e);
                errors.push(error_msg.clone());
                anyhow::anyhow!(error_msg)
            })?;
        
        // Create ID resolver for resolving missing IDs
        let mut id_resolver = IdResolver::new(
            &path_manager.cache_id_dir(),
            &self.sources,
            IdResolverConfig::default(),
        ).map_err(|e| {
            let error_msg = format!("Failed to initialize ID resolver: {}", e);
            errors.push(error_msg.clone());
            anyhow::anyhow!(error_msg)
        })?;
        
        let collected_data = match self.collect_all_data(&mut errors, &cache_manager, &mut id_resolver).await {
            Ok(data) => data,
            Err(e) => {
                errors.push(format!("Failed to collect data: {}", e));
                return Ok(SyncResult {
                    items_synced: 0,
                    duration: start.elapsed(),
                    errors,
                });
            }
        };

        // PHASE 2: RESOLVE - Resolve conflicts across all sources
        // Log collected data before resolution
        info!(
            "Collected data from {} sources",
            collected_data.sources.len()
        );
        for (name, data) in &collected_data.sources {
            info!(
                "Source '{}' data counts: watchlist={}, ratings={}, reviews={}, watch_history={}",
                name,
                data.watchlist.len(),
                data.ratings.len(),
                data.reviews.len(),
                data.watch_history.len()
            );
        }
        
        let source_data_refs: Vec<(&str, &SourceData)> = collected_data.sources
            .iter()
            .map(|(name, data)| (name.as_str(), data))
            .collect();
        let mut resolved_data = crate::resolution::resolve_all_conflicts(
            &source_data_refs,
            &self.resolution_config,
        );
        
        // Log resolved data after resolution
        info!(
            "Resolved data counts: watchlist={}, ratings={}, reviews={}, watch_history={}",
            resolved_data.watchlist.len(),
            resolved_data.ratings.len(),
            resolved_data.reviews.len(),
            resolved_data.watch_history.len()
        );

        // Advanced feature: Mark rated items as watched
        if let Some(ref config_sync_options) = self.config_sync_options {
            if config_sync_options.mark_rated_as_watched && !resolved_data.ratings.is_empty() {
                use std::collections::HashSet;
                
                info!("Running mark_rated_as_watched feature ({} resolved ratings available)", resolved_data.ratings.len());
                
                // Build set of watched IMDB IDs from resolved watch history
                let watched_ids: HashSet<String> = resolved_data.watch_history.iter()
                    .map(|h| h.imdb_id.clone())
                    .collect();
                
                let mut items_marked = 0;
                for rating in &resolved_data.ratings {
                    // Skip shows (cannot be marked as watched on Trakt, and some sources have limitations)
                    if matches!(rating.media_type, media_sync_models::media::MediaType::Show) {
                        continue;
                    }
                    
                    // Only mark if not already in watch history
                    if !watched_ids.contains(&rating.imdb_id) {
                        debug!(
                            imdb_id = %rating.imdb_id,
                            rating = rating.rating,
                            media_type = ?rating.media_type,
                            "Marking rated item as watched (mark_rated_as_watched feature)"
                        );
                        
                        let history_item = WatchHistory {
                            imdb_id: rating.imdb_id.clone(),
                            ids: rating.ids.clone(),
                            title: None,
                            year: None,
                            watched_at: rating.date_added,
                            media_type: rating.media_type.clone(),
                            source: "rated".to_string(),
                        };
                        
                        resolved_data.watch_history.push(history_item);
                        items_marked += 1;
                    }
                }
                
                if items_marked > 0 {
                    info!("Marked {} rated items as watched via mark_rated_as_watched feature (added to resolved watch history)", items_marked);
                    info!("Updated resolved watch history count: {} (was {})", 
                        resolved_data.watch_history.len(),
                        resolved_data.watch_history.len() - items_marked);
                } else {
                    info!("No new items to mark as watched (all rated items already in watch history)");
                }
            } else if config_sync_options.mark_rated_as_watched {
                info!("mark_rated_as_watched is enabled but no ratings are available to process");
            }
        }

        // PHASE 3: DISTRIBUTE - Push resolved data to all sources (filtered to only new/changed items)
        let items_synced = match self.distribute_resolved_data(&resolved_data, &collected_data, &cache_manager, &mut errors).await {
            Ok(count) => count,
            Err(e) => {
                errors.push(format!("Failed to distribute data: {}", e));
                0
            }
        };

        // Save ID resolver cache if dirty
        if let Err(e) = id_resolver.save_if_dirty() {
            warn!("Failed to save ID resolver cache: {}", e);
        }
        
        let duration = start.elapsed();
        info!(
            operation = "sync_complete",
            duration_ms = duration.as_millis(),
            items_synced = items_synced,
            "Sync operation completed"
        );

        Ok(SyncResult {
            items_synced,
            duration,
            errors,
        })
    }
    
    // Utility function for client-side timestamp filtering
    fn filter_by_timestamp<T>(
        items: Vec<T>,
        last_sync: Option<DateTime<Utc>>,
        get_timestamp: impl Fn(&T) -> Option<DateTime<Utc>>,
    ) -> Vec<T> {
        if let Some(last_sync) = last_sync {
            items.into_iter()
                .filter(|item| {
                    get_timestamp(item)
                        .map(|ts| {
                            // If timestamp is at midnight (00:00:00), compare dates only
                            // This handles IMDB exports which only have dates, not times
                            if ts.hour() == 0 && ts.minute() == 0 && ts.second() == 0 {
                                // Compare dates only: include if date >= last_sync date
                                let ts_date = ts.date_naive();
                                let last_sync_date = last_sync.date_naive();
                                ts_date >= last_sync_date
                            } else {
                                // Full timestamp comparison for sources with precise timestamps
                                ts > last_sync
                            }
                        })
                        .unwrap_or(true) // Include items without timestamps
                })
                .collect()
        } else {
            items // First sync, return all
        }
    }

    // Helper functions to fetch or load from cache (shared between collect_all_data and sync_imdb)
    async fn fetch_or_cache_watchlist(
        client: &mut (dyn MediaSource<Error = SourceError>),
        cache_manager: &CacheManager,
        source: &str,
        use_cache: &std::collections::HashSet<String>,
        force_full_sync: bool,
        errors: &mut Vec<String>,
    ) -> Vec<WatchlistItem> {
        if use_cache.contains(&source.to_lowercase()) {
            // When using cache, only use cache - never fetch from API
            if let Ok(Some(cached)) = cache_manager.load_watchlist(source) {
                return cached;
            }
            // Cache miss with use_cache: return empty (testing mode, no upstream fetch)
            warn!("Cache miss for {} watchlist with --use-cache enabled, returning empty list", source);
            return Vec::new();
        }
        // Normal mode: fetch from API and save to cache
        // Cache ALL data to maintain complete upstream state for accurate filtering
        // Call get_watchlist on trait object - handle Error type by converting to string
        let data = match client.get_watchlist().await {
            Ok(data) => data,
            Err(e) => {
                errors.push(format!("Failed to fetch {} watchlist: {}", source, e));
                Vec::new()
            }
        };
        
        // Save complete data to cache (no filtering - cache represents full upstream state)
        if let Err(e) = cache_manager.save_watchlist(source, &data) {
            warn!("Failed to save {} watchlist to cache: {}", source, e);
        }
        data
    }

    async fn fetch_or_cache_ratings(
        client: &mut (dyn MediaSource<Error = SourceError>),
        cache_manager: &CacheManager,
        source: &str,
        use_cache: &std::collections::HashSet<String>,
        force_full_sync: bool,
        errors: &mut Vec<String>,
    ) -> Vec<Rating> {
        if use_cache.contains(&source.to_lowercase()) {
            // When using cache, only use cache - never fetch from API
            if let Ok(Some(cached)) = cache_manager.load_ratings(source) {
                return cached;
            }
            // Cache miss with use_cache: return empty (testing mode, no upstream fetch)
            warn!("Cache miss for {} ratings with --use-cache enabled, returning empty list", source);
            return Vec::new();
        }
        // Normal mode: fetch from API and save to cache
        // Cache ALL data to maintain complete upstream state for accurate filtering
        let data = match client.get_ratings().await {
            Ok(data) => data,
            Err(e) => {
                errors.push(format!("Failed to fetch {} ratings: {}", source, e));
                Vec::new()
            }
        };
        
        // Save complete data to cache (no filtering - cache represents full upstream state)
        if let Err(e) = cache_manager.save_ratings(source, &data) {
            warn!("Failed to save {} ratings to cache: {}", source, e);
        }
        data
    }

    async fn fetch_or_cache_reviews(
        client: &mut (dyn MediaSource<Error = SourceError>),
        cache_manager: &CacheManager,
        source: &str,
        use_cache: &std::collections::HashSet<String>,
        force_full_sync: bool,
        errors: &mut Vec<String>,
    ) -> Vec<Review> {
        if use_cache.contains(&source.to_lowercase()) {
            // When using cache, only use cache - never fetch from API
            if let Ok(Some(cached)) = cache_manager.load_reviews(source) {
                return cached;
            }
            // Cache miss with use_cache: return empty (testing mode, no upstream fetch)
            warn!("Cache miss for {} reviews with --use-cache enabled, returning empty list", source);
            return Vec::new();
        }
        // Normal mode: fetch from API and save to cache
        let data = match client.get_reviews().await {
            Ok(data) => data,
            Err(e) => {
                errors.push(format!("Failed to fetch {} reviews: {}", source, e));
                Vec::new()
            }
        };
        if let Err(e) = cache_manager.save_reviews(source, &data) {
            warn!("Failed to save {} reviews to cache: {}", source, e);
        }
        data
    }

    async fn fetch_or_cache_watch_history(
        client: &mut (dyn MediaSource<Error = SourceError>),
        cache_manager: &CacheManager,
        source: &str,
        use_cache: &std::collections::HashSet<String>,
        force_full_sync: bool,
        errors: &mut Vec<String>,
    ) -> Vec<WatchHistory> {
        if use_cache.contains(&source.to_lowercase()) {
            // When using cache, only use cache - never fetch from API
            if let Ok(Some(cached)) = cache_manager.load_watch_history(source) {
                return cached;
            }
            
            // Cache miss: For IMDB, try to regenerate from CSV if available
            if source.to_lowercase() == "imdb" {
                let path_manager = PathManager::default();
                let cache_dir = path_manager.cache_dir();
                let csv_path = cache_dir.join("imdb_checkins.csv");
                
                if csv_path.exists() {
                    info!("IMDB watch history cache miss, regenerating from CSV: {:?}", csv_path);
                    match media_sync_sources::imdb::parser::parse_checkins_csv(&csv_path) {
                        Ok(history) => {
                            info!("Regenerated {} IMDB watch history items from CSV", history.len());
                            // Save to JSON cache for next time
                            if let Err(e) = cache_manager.save_watch_history(source, &history) {
                                warn!("Failed to save regenerated IMDB watch history to cache: {}", e);
                            }
                            return history;
                        }
                        Err(e) => {
                            warn!("Failed to parse IMDB check-ins CSV at {:?}: {}", csv_path, e);
                            errors.push(format!("Failed to parse IMDB check-ins CSV: {}", e));
                        }
                    }
                } else {
                    debug!("IMDB check-ins CSV not found at {:?}, cannot regenerate cache", csv_path);
                }
            }
            
            // Cache miss with use_cache: return empty (testing mode, no upstream fetch)
            warn!("Cache miss for {} watch history with --use-cache enabled, returning empty list", source);
            return Vec::new();
        }
        // Normal mode: fetch from API and save to cache
        // Cache ALL data to maintain complete upstream state for accurate filtering
        let data = match client.get_watch_history().await {
            Ok(data) => data,
            Err(e) => {
                errors.push(format!("Failed to fetch {} watch history: {}", source, e));
                Vec::new()
            }
        };
        
        // Save complete data to cache (no filtering - cache represents full upstream state)
        if let Err(e) = cache_manager.save_watch_history(source, &data) {
            warn!("Failed to save {} watch history to cache: {}", source, e);
        }
        data
    }

    async fn collect_all_data(&mut self, errors: &mut Vec<String>, cache_manager: &CacheManager, id_resolver: &mut IdResolver) -> Result<CollectedData> {
        // Collect from all sources in source_preference order
        let mut source_data = Vec::new();
        
        for source_name in &self.resolution_config.source_preference {
            // Get source by index for mutable access
            let source_index = match self.find_source_index(source_name) {
                Some(idx) => idx,
                None => {
                    errors.push(format!("Source '{}' not found in registry", source_name));
                    continue;
                }
            };
            
            // Handle sources that support incremental sync
            Self::set_force_full_sync_for_source(&mut self.sources, source_index, self.sync_options.force_full_sync);
            
            // Get the source for data collection
            // Wrap in a block to limit borrow scope
            let data = {
                let source = match self.sources.get_mut(source_index) {
                    Some(s) => s.as_mut() as &mut dyn MediaSource<Error = SourceError>,
                    None => {
                        errors.push(format!("Source '{}' not found at index {}", source_name, source_index));
                        continue;
                    }
                };
                
                let mut source_data = SourceData {
                watchlist: if self.sync_options.sync_watchlist {
                    Self::fetch_or_cache_watchlist(source, cache_manager, source_name, &self.use_cache, self.sync_options.force_full_sync, errors).await
                } else {
                    Vec::new()
                },
                ratings: if self.sync_options.sync_ratings {
                    Self::fetch_or_cache_ratings(source, cache_manager, source_name, &self.use_cache, self.sync_options.force_full_sync, errors).await
                } else {
                    Vec::new()
                },
                reviews: if self.sync_options.sync_reviews {
                    Self::fetch_or_cache_reviews(source, cache_manager, source_name, &self.use_cache, self.sync_options.force_full_sync, errors).await
                } else {
                    Vec::new()
                },
                watch_history: if self.sync_options.sync_watch_history {
                    Self::fetch_or_cache_watch_history(source, cache_manager, source_name, &self.use_cache, self.sync_options.force_full_sync, errors).await
                } else {
                    Vec::new()
                },
                };
                
                // Resolve IDs for items with empty imdb_id
                Self::resolve_missing_ids(&mut source_data, id_resolver, &self.sources, errors).await;
                
                source_data
            };
            
            source_data.push((source_name.clone(), data));
        }
        
        Ok(CollectedData {
            sources: source_data,
        })
    }
    
    /// Resolve missing IDs for items using IdResolver
    /// Always populates ids field, even when imdb_id exists
    async fn resolve_missing_ids(
        data: &mut SourceData,
        id_resolver: &mut IdResolver,
        sources: &[Box<dyn MediaSource<Error = SourceError>>],
        errors: &mut Vec<String>,
    ) {
        debug!("Starting ID resolution for {} watchlist items, {} ratings, {} reviews, {} watch_history items",
               data.watchlist.len(), data.ratings.len(), data.reviews.len(), data.watch_history.len());
        
        // Resolve watchlist items - always check cache first, then external lookup if needed
        for item in &mut data.watchlist {
            // Always try to populate ids field
            if item.ids.is_none() || item.ids.as_ref().map(|ids| ids.is_empty()).unwrap_or(true) {
                if !item.imdb_id.is_empty() {
                    // Item has imdb_id, start with collected IDs (if any) or create from imdb_id
                    let mut ids = item.ids.clone().unwrap_or_else(|| {
                        let mut new_ids = MediaIds::default();
                        new_ids.imdb_id = Some(item.imdb_id.clone());
                        new_ids
                    });
                    
                    // Check cache to enrich with additional IDs
                    if let Some(cached_ids) = id_resolver.find_by_any_id(&item.imdb_id) {
                        // Merge cached IDs into collected IDs (collected IDs take precedence)
                        ids.merge(&cached_ids);
                    }
                    
                    // Always cache the merged result (collected + cached)
                    item.ids = Some(ids.clone());
                    id_resolver.cache_ids_with_metadata(
                        ids,
                        Some(&item.title),
                        item.year,
                        Some(&item.media_type),
                    );
                } else {
                    // No imdb_id, try to resolve via lookup (resolve_ids_for_item checks cache first)
                    debug!("Resolving IDs for watchlist item: '{}' (year: {:?}, type: {:?})", 
                           item.title, item.year, item.media_type);
                    
                    // Check if lookup providers are available before attempting lookup
                    let available_providers: Vec<String> = id_resolver.available_lookup_providers().iter().map(|s| s.to_string()).collect();
                    if available_providers.is_empty() {
                        warn!("No lookup providers available for '{}'. Cannot perform title-based lookup. Ensure at least one source (Plex, Trakt, or Simkl) is authenticated.", item.title);
                    }
                    
                    match id_resolver.resolve_ids_for_item(
                        sources,
                        &item.title,
                        item.year,
                        &item.media_type,
                        None,
                    ).await {
                        Ok(ids) => {
                            if !ids.is_empty() {
                                debug!("Resolved IDs for '{}': imdb={:?}, tmdb={:?}, tvdb={:?}", 
                                       item.title, ids.imdb_id, ids.tmdb_id, ids.tvdb_id);
                                if let Some(imdb) = ids.imdb_id.clone() {
                                    item.imdb_id = imdb;
                                }
                                item.ids = Some(ids);
                            } else {
                                warn!("ID resolution returned empty IDs for '{}' (year: {:?}). Available providers: {:?}", 
                                      item.title, item.year, available_providers);
                            }
                        }
                        Err(e) => {
                            warn!("Failed to resolve IDs for '{}' (year: {:?}): {}", item.title, item.year, e);
                        }
                    }
                }
            } else {
                // IDs already exist, but check cache to enrich with additional IDs
                if let Some(ref mut ids) = item.ids {
                    // Try to enrich with cached IDs using any available ID
                    if let Some(any_id) = ids.get_any_id() {
                        if let Some(cached_ids) = id_resolver.find_by_any_id(&any_id) {
                            // Merge cached IDs to enrich the existing IDs
                            ids.merge(&cached_ids);
                            // Update imdb_id if we got it from cache
                            if let Some(imdb) = ids.imdb_id.clone() {
                                item.imdb_id = imdb;
                            }
                        }
                    }
                    // Cache the (potentially enriched) IDs
                    id_resolver.cache_ids_with_metadata(
                        ids.clone(),
                        Some(&item.title),
                        item.year,
                        Some(&item.media_type),
                    );
                }
            }
        }
        
        // Resolve ratings - always check cache first, then external lookup if needed
        for rating in &mut data.ratings {
            let needs_resolution = rating.ids.is_none() || rating.ids.as_ref().map(|ids| ids.is_empty()).unwrap_or(true);
            if needs_resolution {
                if !rating.imdb_id.is_empty() {
                    // Start with collected IDs (if any) or create from imdb_id
                    let mut ids = rating.ids.clone().unwrap_or_else(|| {
                        let mut new_ids = MediaIds::default();
                        new_ids.imdb_id = Some(rating.imdb_id.clone());
                        new_ids
                    });
                    
                    // Check cache to enrich with additional IDs
                    if let Some(cached_ids) = id_resolver.find_by_any_id(&rating.imdb_id) {
                        // Merge cached IDs into collected IDs (collected IDs take precedence)
                        ids.merge(&cached_ids);
                    }
                    
                    // Always cache the merged result (collected + cached)
                    rating.ids = Some(ids.clone());
                    id_resolver.cache_ids(ids);
                } else if let Some(ref existing_ids) = rating.ids {
                    // No imdb_id but have MediaIds from collected data, start with collected IDs
                    let mut resolved_ids = existing_ids.clone();
                    
                    if let Some(any_id) = resolved_ids.get_any_id() {
                        // Check cache to enrich with additional IDs
                        if let Some(cached_ids) = id_resolver.find_by_any_id(&any_id) {
                            // Merge cached IDs into collected IDs (collected IDs take precedence)
                            resolved_ids.merge(&cached_ids);
                        }
                        if let Some(imdb) = resolved_ids.imdb_id.clone() {
                            rating.imdb_id = imdb;
                        }
                        // Always cache the merged result (collected + cached)
                        rating.ids = Some(resolved_ids.clone());
                        id_resolver.cache_ids(resolved_ids);
                    } else {
                        // MediaIds exists but is empty - still cache it
                        rating.ids = Some(resolved_ids.clone());
                        id_resolver.cache_ids(resolved_ids);
                    }
                } else {
                    // No imdb_id and no MediaIds - create empty MediaIds
                    rating.ids = Some(MediaIds::default());
                }
            } else {
                // IDs already exist, but check cache to enrich with additional IDs
                if let Some(ref mut ids) = rating.ids {
                    // Try to enrich with cached IDs using any available ID
                    if let Some(any_id) = ids.get_any_id() {
                        if let Some(cached_ids) = id_resolver.find_by_any_id(&any_id) {
                            // Merge cached IDs to enrich the existing IDs
                            ids.merge(&cached_ids);
                            // Update imdb_id if we got it from cache
                            if let Some(imdb) = ids.imdb_id.clone() {
                                rating.imdb_id = imdb;
                            }
                        }
                    }
                    // Cache the (potentially enriched) IDs
                    id_resolver.cache_ids(ids.clone());
                }
            }
        }
        
        // Resolve reviews - always check cache first, then external lookup if needed
        for review in &mut data.reviews {
            let needs_resolution = review.ids.is_none() || review.ids.as_ref().map(|ids| ids.is_empty()).unwrap_or(true);
            if needs_resolution {
                if !review.imdb_id.is_empty() {
                    // Start with collected IDs (if any) or create from imdb_id
                    let mut ids = review.ids.clone().unwrap_or_else(|| {
                        let mut new_ids = MediaIds::default();
                        new_ids.imdb_id = Some(review.imdb_id.clone());
                        new_ids
                    });
                    
                    // Check cache to enrich with additional IDs
                    if let Some(cached_ids) = id_resolver.find_by_any_id(&review.imdb_id) {
                        // Merge cached IDs into collected IDs (collected IDs take precedence)
                        ids.merge(&cached_ids);
                    }
                    
                    // If we still don't have a title, try reverse lookup by IMDB ID
                    if ids.title.is_none() {
                        if let Ok(Some((title, year, lookup_ids))) = id_resolver.lookup_by_imdb_id(
                            sources,
                            &review.imdb_id,
                            &review.media_type,
                        ).await {
                            // Merge the looked up IDs (title, year, and other IDs)
                            ids.merge(&lookup_ids);
                            // Ensure title and year are set
                            if ids.title.is_none() {
                                ids.title = Some(title);
                            }
                            if ids.year.is_none() {
                                ids.year = year;
                            }
                            debug!("Review resolution: Reverse lookup found title='{}', year={:?} for imdb_id={}", 
                                   ids.title.as_deref().unwrap_or(""), ids.year, review.imdb_id);
                        }
                    }
                    
                    // Always cache the merged result (collected + cached + looked up)
                    // Use cache_ids_with_metadata to preserve title/year from merged IDs
                    let title_for_cache = ids.title.clone();
                    let year_for_cache = ids.year;
                    review.ids = Some(ids.clone());
                    id_resolver.cache_ids_with_metadata(
                        ids,
                        title_for_cache.as_deref(),
                        year_for_cache,
                        Some(&review.media_type),
                    );
                } else if let Some(ref existing_ids) = review.ids {
                    // No imdb_id but have MediaIds from collected data, start with collected IDs
                    let mut resolved_ids = existing_ids.clone();
                    
                    if let Some(any_id) = resolved_ids.get_any_id() {
                        // Check cache to enrich with additional IDs
                        if let Some(cached_ids) = id_resolver.find_by_any_id(&any_id) {
                            // Merge cached IDs into collected IDs (collected IDs take precedence)
                            resolved_ids.merge(&cached_ids);
                        }
                        if let Some(imdb) = resolved_ids.imdb_id.clone() {
                            review.imdb_id = imdb;
                        }
                        // Always cache the merged result (collected + cached)
                        // Use cache_ids_with_metadata to preserve title/year from merged IDs
                        let title_for_cache = resolved_ids.title.clone();
                        let year_for_cache = resolved_ids.year;
                        review.ids = Some(resolved_ids.clone());
                        id_resolver.cache_ids_with_metadata(
                            resolved_ids,
                            title_for_cache.as_deref(),
                            year_for_cache,
                            Some(&review.media_type),
                        );
                    } else {
                        // MediaIds exists but is empty - still cache it
                        // Use cache_ids_with_metadata to preserve title/year from merged IDs
                        let title_for_cache = resolved_ids.title.clone();
                        let year_for_cache = resolved_ids.year;
                        review.ids = Some(resolved_ids.clone());
                        id_resolver.cache_ids_with_metadata(
                            resolved_ids,
                            title_for_cache.as_deref(),
                            year_for_cache,
                            Some(&review.media_type),
                        );
                    }
                } else {
                    // No imdb_id and no MediaIds - create empty MediaIds
                    review.ids = Some(MediaIds::default());
                }
            } else {
                // IDs already exist, but check cache to enrich with additional IDs
                if let Some(ref mut ids) = review.ids {
                    // Try to enrich with cached IDs using any available ID
                    if let Some(any_id) = ids.get_any_id() {
                        if let Some(cached_ids) = id_resolver.find_by_any_id(&any_id) {
                            // Merge cached IDs to enrich the existing IDs
                            ids.merge(&cached_ids);
                            // Update imdb_id if we got it from cache
                            if let Some(imdb) = ids.imdb_id.clone() {
                                review.imdb_id = imdb;
                            }
                        }
                    }
                    // Cache the (potentially enriched) IDs
                    // Use cache_ids_with_metadata to preserve title/year from merged IDs
                    id_resolver.cache_ids_with_metadata(
                        ids.clone(),
                        ids.title.as_deref(),
                        ids.year,
                        Some(&review.media_type),
                    );
                }
            }
        }
        
        // Resolve watch history - always populate ids field and cache IDs from collected data
        // First pass: resolve by IDs and filter out items with no title and no IDs
        data.watch_history.retain_mut(|history| {
            let needs_resolution = history.ids.is_none() || history.ids.as_ref().map(|ids| ids.is_empty()).unwrap_or(true);
            
            if needs_resolution {
                if !history.imdb_id.is_empty() {
                    // Start with collected IDs (if any) or create from imdb_id
                    let mut ids = history.ids.clone().unwrap_or_else(|| {
                        let mut new_ids = MediaIds::default();
                        new_ids.imdb_id = Some(history.imdb_id.clone());
                        new_ids
                    });
                    
                    // Check cache to enrich with additional IDs
                    if let Some(cached_ids) = id_resolver.find_by_any_id(&history.imdb_id) {
                        // Merge cached IDs into collected IDs (collected IDs take precedence)
                        ids.merge(&cached_ids);
                        
                        // Update history.title and history.year from cached entry if missing
                        if history.title.is_none() {
                            history.title = ids.title.clone();
                        }
                        if history.year.is_none() {
                            history.year = ids.year;
                        }
                    }
                    
                    // Always cache the merged result (collected + cached)
                    // Use ids.title as fallback if history.title is None (to preserve cached metadata)
                    let title_for_cache = history.title.as_deref().or_else(|| ids.title.as_deref());
                    let year_for_cache = history.year.or(ids.year);
                    let ids_clone = ids.clone();
                    history.ids = Some(ids_clone.clone());
                    id_resolver.cache_ids_with_metadata(
                        ids_clone,
                        title_for_cache,
                        year_for_cache,
                        Some(&history.media_type),
                    );
                    true // Keep item - has IMDB ID
                } else if let Some(ref existing_ids) = history.ids {
                    // No imdb_id but have MediaIds from collected data, start with collected IDs
                    let mut resolved_ids = existing_ids.clone();
                    
                    if let Some(any_id) = resolved_ids.get_any_id() {
                        // Check cache to enrich with additional IDs
                        if let Some(cached_ids) = id_resolver.find_by_any_id(&any_id) {
                            // Merge cached IDs into collected IDs (collected IDs take precedence)
                            resolved_ids.merge(&cached_ids);
                            
                            // Update history.title and history.year from cached entry if missing
                            if history.title.is_none() {
                                history.title = resolved_ids.title.clone();
                            }
                            if history.year.is_none() {
                                history.year = resolved_ids.year;
                            }
                        }
                        if let Some(imdb) = resolved_ids.imdb_id.clone() {
                            history.imdb_id = imdb;
                        }
                        // Always cache the merged result (collected + cached)
                        // Use resolved_ids.title as fallback if history.title is None (to preserve cached metadata)
                        let title_for_cache = history.title.as_deref().or_else(|| resolved_ids.title.as_deref());
                        let year_for_cache = history.year.or(resolved_ids.year);
                        let resolved_ids_clone = resolved_ids.clone();
                        history.ids = Some(resolved_ids_clone.clone());
                        id_resolver.cache_ids_with_metadata(
                            resolved_ids_clone,
                            title_for_cache,
                            year_for_cache,
                            Some(&history.media_type),
                        );
                        true // Keep item - has some IDs
                    } else if history.title.is_some() {
                        // No IDs but have title - will try title-based lookup next
                        true // Keep item - has title for resolution
                    } else {
                        // No title and no IDs - can't resolve, filter out
                        warn!("Watch history item has no title and no IDs - cannot resolve, filtering out");
                        false
                    }
                } else if history.title.is_some() {
                    // No IDs but have title - will try title-based lookup next
                    true // Keep item - has title for resolution
                } else {
                    // No title and no IDs - can't resolve, filter out
                    warn!("Watch history item has no title and no IDs - cannot resolve, filtering out");
                    false
                }
            } else {
                // IDs already exist, but check cache to enrich with additional IDs
                if let Some(ref mut ids) = history.ids {
                    // Try to enrich with cached IDs using any available ID
                    if let Some(any_id) = ids.get_any_id() {
                        if let Some(cached_ids) = id_resolver.find_by_any_id(&any_id) {
                            // Merge cached IDs to enrich the existing IDs
                            ids.merge(&cached_ids);
                            // Update imdb_id if we got it from cache
                            if let Some(imdb) = ids.imdb_id.clone() {
                                history.imdb_id = imdb;
                            }
                            // Update history.title and history.year from cached entry if missing
                            if history.title.is_none() {
                                history.title = ids.title.clone();
                            }
                            if history.year.is_none() {
                                history.year = ids.year;
                            }
                        }
                    }
                    // Cache the (potentially enriched) IDs
                    // Use ids.title as fallback if history.title is None (to preserve cached metadata)
                    let title_for_cache = history.title.as_deref().or_else(|| ids.title.as_deref());
                    let year_for_cache = history.year.or(ids.year);
                    id_resolver.cache_ids_with_metadata(
                        ids.clone(),
                        title_for_cache,
                        year_for_cache,
                        Some(&history.media_type),
                    );
                }
                true // Keep item - has IDs
            }
        });
        
        // Second pass: try title-based lookup for items with title but no IDs
        for history in &mut data.watch_history {
            let needs_resolution = history.ids.is_none() || history.ids.as_ref().map(|ids| ids.is_empty()).unwrap_or(true);
            
            if needs_resolution && history.title.is_some() {
                if let Some(ref title) = history.title {
                    debug!("Resolving IDs for watch history item: '{}' (year: {:?}, type: {:?})", 
                           title, history.year, history.media_type);
                    
                    match id_resolver.resolve_ids_for_item(
                        sources,
                        title,
                        history.year,
                        &history.media_type,
                        None,
                    ).await {
                        Ok(ids) => {
                            if !ids.is_empty() {
                                debug!("Resolved IDs for '{}': imdb={:?}, tmdb={:?}, tvdb={:?}", 
                                       title, ids.imdb_id, ids.tmdb_id, ids.tvdb_id);
                                if let Some(imdb) = ids.imdb_id.clone() {
                                    history.imdb_id = imdb;
                                }
                                history.ids = Some(ids);
                            } else {
                                warn!("ID resolution returned empty IDs for '{}' (year: {:?})", title, history.year);
                                history.ids = Some(MediaIds::default());
                            }
                        }
                        Err(e) => {
                            warn!("Failed to resolve IDs for '{}' (year: {:?}): {}", title, history.year, e);
                            history.ids = Some(MediaIds::default());
                        }
                    }
                }
            }
        }
    }
    
    /// Write distribute data files for a source (split by type)
    fn write_dry_run_json(
        &self,
        source_name: &str,
        data: &DryRunData,
    ) -> Result<()> {
        let path_manager = PathManager::default();
        let cache_manager = CacheManager::new(&path_manager)
            .map_err(|e| anyhow::anyhow!("Failed to initialize cache manager: {}", e))?;
        
        // Write separate files per data type
        if !data.watchlist.is_empty() {
            cache_manager.save_distribute_data(source_name, "watchlist", &data.watchlist)?;
            info!("Distribute data written: {} watchlist ({} items)", source_name, data.watchlist.len());
        }
        
        if !data.watchlist_to_history.is_empty() {
            cache_manager.save_distribute_data(source_name, "watchlist_to_history", &data.watchlist_to_history)?;
            info!("Distribute data written: {} watchlist_to_history ({} items)", source_name, data.watchlist_to_history.len());
        }
        
        if !data.ratings.is_empty() {
            cache_manager.save_distribute_data(source_name, "ratings", &data.ratings)?;
            info!("Distribute data written: {} ratings ({} items)", source_name, data.ratings.len());
        }
        
        if !data.reviews.is_empty() {
            cache_manager.save_distribute_data(source_name, "reviews", &data.reviews)?;
            info!("Distribute data written: {} reviews ({} items)", source_name, data.reviews.len());
        }
        
        if !data.watch_history.is_empty() {
            cache_manager.save_distribute_data(source_name, "watch_history", &data.watch_history)?;
            info!("Distribute data written: {} watch_history ({} items)", source_name, data.watch_history.len());
        }
        
        if !data.removal_list.is_empty() {
            cache_manager.save_distribute_data(source_name, "removal_list", &data.removal_list)?;
            info!("Distribute data written: {} removal_list ({} items)", source_name, data.removal_list.len());
        }
        
        Ok(())
    }

    /// Prepare resolved data for a source (used for both dry-run and actual sync)
    /// Returns the prepared data that would be written to the source
    async fn prepare_resolved_data(
        &self,
        source_name: &str,
        strategy: &Box<dyn DistributionStrategy>,
        resolved: &ResolvedData,
        collected_data: &CollectedData,
        removal_lists: &std::collections::HashMap<String, Vec<WatchlistItem>>,
    ) -> Result<DryRunData> {
        // Get existing data for the source, or use empty data
        let empty_data = SourceData {
            watchlist: Vec::new(),
            ratings: Vec::new(),
            reviews: Vec::new(),
            watch_history: Vec::new(),
        };
        let existing = collected_data.sources.iter()
            .find(|(name, _)| name == source_name)
            .map(|(_, data)| data)
            .unwrap_or(&empty_data);

        // Get removal list for this source (used for filtering and output)
        // Filter out items that originated from the target source - they're already there and don't need to be shown in removal list
        // (The removal list should only show items from other sources that need to be removed)
        let mut removal_list = removal_lists.get(source_name).cloned().unwrap_or_default();
        let before_filter = removal_list.len();
        removal_list.retain(|item| item.source != source_name);
        if before_filter > removal_list.len() {
            debug!("Filtered out {} items from {} removal list (originated from target source)", 
                   before_filter - removal_list.len(), source_name);
        }
        
        // Prepare all data types using the distribution strategy
        let mut watchlist_result = if self.sync_options.sync_watchlist {
            strategy.prepare_watchlist(&resolved.watchlist, existing, self.sync_options.force_full_sync)
                .unwrap_or_else(|e| {
                    warn!("Failed to prepare watchlist for {}: {}", source_name, e);
                    DistributionResult::default()
                })
        } else {
            DistributionResult::default()
        };
        
        // Apply removal filtering to watchlist data
        if !removal_list.is_empty() {
            let removal_ids: std::collections::HashSet<String> = removal_list.iter()
                .map(|item| item.imdb_id.clone())
                .collect();
            let before_count = watchlist_result.for_watchlist.len();
            watchlist_result.for_watchlist.retain(|item| !removal_ids.contains(&item.imdb_id));
            if before_count > watchlist_result.for_watchlist.len() {
                info!("Filtered out {} items from {} watchlist additions (watched or old)", 
                    before_count - watchlist_result.for_watchlist.len(), source_name);
            }
        }

        let ratings = if self.sync_options.sync_ratings {
            strategy.prepare_ratings(&resolved.ratings, existing, self.sync_options.force_full_sync)
                .unwrap_or_else(|e| {
                    warn!("Failed to prepare ratings for {}: {}", source_name, e);
                    Vec::new()
                })
        } else {
            Vec::new()
        };

        let reviews = if self.sync_options.sync_reviews {
            strategy.prepare_reviews(&resolved.reviews, existing, self.sync_options.force_full_sync)
                .unwrap_or_else(|e| {
                    warn!("Failed to prepare reviews for {}: {}", source_name, e);
                    Vec::new()
                })
        } else {
            Vec::new()
        };

        let watch_history = if self.sync_options.sync_watch_history {
            strategy.prepare_watch_history(&resolved.watch_history, existing, self.sync_options.force_full_sync)
                .unwrap_or_else(|e| {
                    warn!("Failed to prepare watch history for {}: {}", source_name, e);
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        
        // Build resolved data structure
        Ok(DryRunData {
            source: source_name.to_string(),
            timestamp: Utc::now(),
            sync_options: self.sync_options.clone(),
            watchlist: watchlist_result.for_watchlist,
            watchlist_to_history: watchlist_result.for_watch_history,
            ratings,
            reviews,
            watch_history,
            removal_list: removal_list.clone(),
        })
    }

    /// Handle dry-run mode for a source: prepare data and write JSON file
    async fn handle_dry_run_source(
        &self,
        source_name: &str,
        strategy: &Box<dyn DistributionStrategy>,
        resolved: &ResolvedData,
        collected_data: &CollectedData,
        removal_lists: &std::collections::HashMap<String, Vec<WatchlistItem>>,
    ) -> Result<()> {
        let dry_run_data = self.prepare_resolved_data(
            source_name,
            strategy,
            resolved,
            collected_data,
            removal_lists,
        ).await?;

        // Write JSON file
        self.write_dry_run_json(source_name, &dry_run_data)?;
        
        info!("Dry-run mode: prepared data for {} (watchlist: {}, watchlist_to_history: {}, ratings: {}, reviews: {}, watch_history: {}, removals: {})",
            source_name,
            dry_run_data.watchlist.len(),
            dry_run_data.watchlist_to_history.len(),
            dry_run_data.ratings.len(),
            dry_run_data.reviews.len(),
            dry_run_data.watch_history.len(),
            dry_run_data.removal_list.len()
        );

        Ok(())
    }

    async fn distribute_resolved_data(
        &mut self,
        resolved: &ResolvedData,
        collected_data: &CollectedData,
        cache_manager: &CacheManager,
        errors: &mut Vec<String>,
    ) -> Result<usize> {
        let mut items_synced = 0;
        
        // Build set of watched IMDB IDs if remove_watched_from_watchlists is enabled
        let watched_ids: std::collections::HashSet<String> = if let Some(ref config_sync_options) = self.config_sync_options {
            if config_sync_options.remove_watched_from_watchlists {
                resolved.watch_history.iter()
                    .map(|h| h.imdb_id.clone())
                    .collect()
            } else {
                std::collections::HashSet::new()
            }
        } else {
            std::collections::HashSet::new()
        };
        
        // Calculate cutoff date if remove_watchlist_items_older_than_days is enabled
        let cutoff_date: Option<DateTime<Utc>> = if let Some(ref config_sync_options) = self.config_sync_options {
            if let Some(days) = config_sync_options.remove_watchlist_items_older_than_days {
                Some(Utc::now() - chrono::Duration::days(days as i64))
            } else {
                None
            }
        } else {
            None
        };
        
        // Build centralized removal lists for all sources
        let mut removal_lists: std::collections::HashMap<String, Vec<WatchlistItem>> = std::collections::HashMap::new();
        
        // Iterate through all sources in collected_data to build removal lists
        for (source_name, existing_data) in &collected_data.sources {
            let mut removal_list = Vec::new();
            
            // Build removal list for remove_watched_from_watchlists
            // Note: Only include items from existing_data.watchlist (items currently in the target source's watchlist)
            // Excluded items were never successfully added to the watchlist, so they shouldn't be in the removal list
            if let Some(ref config_sync_options) = self.config_sync_options {
                if config_sync_options.remove_watched_from_watchlists {
                    // Check collected watchlist items
                    for item in &existing_data.watchlist {
                        if watched_ids.contains(&item.imdb_id) {
                            removal_list.push(item.clone());
                        }
                    }
                }
                
                // Build removal list for remove_watchlist_items_older_than_days
                if let Some(cutoff) = cutoff_date {
                    // Check collected watchlist items
                    for item in &existing_data.watchlist {
                        if item.date_added < cutoff {
                            removal_list.push(item.clone());
                        }
                    }
                }
            }
            
            // Deduplicate removal list by IMDB ID
            let mut seen_ids = std::collections::HashSet::new();
            removal_list.retain(|item| seen_ids.insert(item.imdb_id.clone()));
            
            if !removal_list.is_empty() {
                removal_lists.insert(source_name.clone(), removal_list);
            }
        }
        
        // Add Simkl Dropped items to removal lists for all other sources
        if let Some((_, simkl_data)) = collected_data.sources.iter().find(|(name, _)| name == "simkl") {
            let dropped_items: Vec<WatchlistItem> = simkl_data.watchlist
                .iter()
                .filter(|item| item.status == Some(NormalizedStatus::Dropped))
                .cloned()
                .collect();
            
            if !dropped_items.is_empty() {
                info!("Found {} Dropped items in Simkl watchlist, adding to removal lists for all other sources", dropped_items.len());
                
                // Add to removal list for all other sources
                for (source_name, _) in &collected_data.sources {
                    if source_name != "simkl" {
                        let removal_list = removal_lists.entry(source_name.clone()).or_insert_with(Vec::new);
                        let before_count = removal_list.len();
                        
                        // Add dropped items, avoiding duplicates
                        let existing_ids: std::collections::HashSet<String> = removal_list.iter()
                            .map(|item| item.imdb_id.clone())
                            .collect();
                        
                        for dropped_item in &dropped_items {
                            if !existing_ids.contains(&dropped_item.imdb_id) {
                                removal_list.push(dropped_item.clone());
                            }
                        }
                        
                        let added_count = removal_list.len() - before_count;
                        if added_count > 0 {
                            info!("Added {} Simkl Dropped items to {} removal list (total: {})", 
                                added_count, source_name, removal_list.len());
                        }
                    }
                }
            }
        }
        
        // Helper to get existing data for a source
        let get_existing_data = |source_name: &str| -> Option<&SourceData> {
            collected_data.sources.iter()
                .find(|(name, _)| name == source_name)
                .map(|(_, data)| data)
        };
        
        // Helper to create distribution strategy for a target source by name
        // In the future, sources could provide their own strategy via distribution_strategy_name()
        let create_strategy_by_name = |source_name: &str, cache_manager: &CacheManager| -> Result<Box<dyn DistributionStrategy>> {
            let cache_manager_clone = cache_manager.clone();
            
            match source_name {
                "trakt" => Ok(Box::new(TraktDistributionStrategy::new()?.with_cache_manager(cache_manager_clone))),
                "imdb" => Ok(Box::new(ImdbDistributionStrategy::new()?.with_cache_manager(cache_manager_clone))),
                "simkl" => Ok(Box::new(SimklDistributionStrategy::new()?)),
                "plex" => Ok(Box::new(PlexDistributionStrategy::new()?.with_cache_manager(cache_manager_clone))),
                _ => Ok(Box::new(DefaultDistributionStrategy::new(source_name)?.with_cache_manager(cache_manager_clone))),
            }
        };
        
        // Distribute to all sources in source_preference order
        for source_name in &self.resolution_config.source_preference {
            // Check if this source is in dry-run mode
            let is_dry_run = self.dry_run_sources.contains(&source_name.to_lowercase());
            
            // Get the source name for strategy creation (we'll get mutable access later)
            // Strategy creation only needs the source name, not the full source
            let strategy = match create_strategy_by_name(source_name, cache_manager) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(format!("Failed to create distribution strategy for {}: {}", source_name, e));
                    continue;
                }
            };
            
            // Always prepare and save resolved data for inspection (regardless of dry-run setting)
            let prepared_data = match self.prepare_resolved_data(
                source_name,
                &strategy,
                resolved,
                collected_data,
                &removal_lists,
            ).await {
                Ok(data) => {
                    // Save the resolved data to JSON file
                    if let Err(e) = self.write_dry_run_json(source_name, &data) {
                        warn!("Failed to write resolved data for {}: {}", source_name, e);
                    }
                    data
                }
                Err(e) => {
                    errors.push(format!("Failed to prepare resolved data for {}: {}", source_name, e));
                    continue;
                }
            };
            
            // Handle dry-run mode: skip actual writes for dry-run sources
            if is_dry_run {
                info!("Dry-run mode: prepared data for {} (watchlist: {}, watchlist_to_history: {}, ratings: {}, reviews: {}, watch_history: {}, removals: {})",
                    source_name,
                    prepared_data.watchlist.len(),
                    prepared_data.watchlist_to_history.len(),
                    prepared_data.ratings.len(),
                    prepared_data.reviews.len(),
                    prepared_data.watch_history.len(),
                    prepared_data.removal_list.len()
                );
                continue; // Skip actual writes for dry-run sources
            }
            
            // Get source by index for mutable access (after data preparation is done)
            let source_index = match self.find_source_index(source_name) {
                Some(idx) => idx,
                None => {
                    errors.push(format!("Source '{}' not found in registry", source_name));
                    continue;
                }
            };
            
            let source = match self.sources.get_mut(source_index) {
                Some(s) => s,
                None => {
                    errors.push(format!("Source '{}' not found at index {}", source_name, source_index));
                    continue;
                }
            };
            
            match source_name.as_str() {
                "trakt" => {
                        // Strategy already created above
                        if let Some(existing) = get_existing_data("trakt") {
                            if self.sync_options.sync_watchlist {
                                match strategy.prepare_watchlist(&resolved.watchlist, existing, self.sync_options.force_full_sync) {
                                    Ok(mut result) => {
                                        // Get removal list for Trakt from centralized HashMap
                                        let removal_list = removal_lists.get("trakt").cloned().unwrap_or_default();
                                        
                                        // Filter out items that should be removed (by IMDB ID)
                                        if !removal_list.is_empty() {
                                            let removal_ids: std::collections::HashSet<String> = removal_list.iter()
                                                .map(|item| item.imdb_id.clone())
                                                .collect();
                                            let before_count = result.for_watchlist.len();
                                            result.for_watchlist.retain(|item| !removal_ids.contains(&item.imdb_id));
                                            if before_count > result.for_watchlist.len() {
                                                info!("Filtered out {} items from Trakt watchlist additions (watched or old)", before_count - result.for_watchlist.len());
                                            }
                                        }
                                        
                                        if !result.for_watchlist.is_empty() {
                                            info!("Filtering watchlist for Trakt: {} items to add to watchlist (from {} resolved)", result.for_watchlist.len(), resolved.watchlist.len());
                                            if let Err(e) = source.as_mut().add_to_watchlist(&result.for_watchlist).await {
                                                errors.push(format!("Failed to add watchlist to Trakt: {}", e));
                                            } else {
                                                items_synced += result.for_watchlist.len();
                                                if let Err(e) = strategy.on_sync_complete("watchlist", result.for_watchlist.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                        
                                        // Remove items from centralized removal list
                                        if !removal_list.is_empty() {
                                            for item in &removal_list {
                                                debug!(
                                                    imdb_id = %item.imdb_id,
                                                    title = %item.title,
                                                    media_type = ?item.media_type,
                                                    "Removing from Trakt watchlist (watched or old)"
                                                );
                                            }
                                            if let Err(e) = source.as_mut().remove_from_watchlist(&removal_list).await {
                                                errors.push(format!("Failed to remove items from Trakt watchlist: {}", e));
                                            } else {
                                                info!("Removed {} items from Trakt watchlist", removal_list.len());
                                            }
                                        }
                                        
                                        if !result.for_watch_history.is_empty() && self.sync_options.sync_watch_history {
                                            info!("Filtering watchlist for Trakt: {} items to add to watch history (from {} resolved)", result.for_watch_history.len(), resolved.watchlist.len());
                                            if let Err(e) = source.as_mut().add_watch_history(&result.for_watch_history).await {
                                                errors.push(format!("Failed to add watch history to Trakt: {}", e));
                                            } else {
                                                items_synced += result.for_watch_history.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", result.for_watch_history.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                        
                                        if result.for_watchlist.is_empty() && result.for_watch_history.is_empty() && !resolved.watchlist.is_empty() {
                                            info!("All {} resolved watchlist items already exist in Trakt", resolved.watchlist.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watchlist for Trakt: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_ratings {
                                match strategy.prepare_ratings(&resolved.ratings, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering ratings for Trakt: {} items to add/update (from {} resolved)", filtered.len(), resolved.ratings.len());
                                            if let Err(e) = source.as_mut().set_ratings(&filtered).await {
                                                errors.push(format!("Failed to set ratings on Trakt: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("ratings", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.ratings.is_empty() {
                                            info!("All {} resolved ratings already exist in Trakt with same values", resolved.ratings.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare ratings for Trakt: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_reviews {
                                match strategy.prepare_reviews(&resolved.reviews, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering reviews for Trakt: {} items to add (from {} resolved)", filtered.len(), resolved.reviews.len());
                                            if let Err(e) = source.as_mut().set_reviews(&filtered).await {
                                                errors.push(format!("Failed to set reviews on Trakt: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("reviews", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.reviews.is_empty() {
                                            info!("All {} resolved reviews already exist in Trakt", resolved.reviews.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare reviews for Trakt: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_watch_history {
                                match strategy.prepare_watch_history(&resolved.watch_history, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering watch history for Trakt: {} items to add (from {} resolved)", filtered.len(), resolved.watch_history.len());
                                            if let Err(e) = source.add_watch_history(&filtered).await {
                                                errors.push(format!("Failed to add watch history to Trakt: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.watch_history.is_empty() {
                                            info!("All {} resolved watch history items already exist in Trakt", resolved.watch_history.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watch history for Trakt: {}", e));
                                    }
                                }
                            }
                        } else {
                            // No existing data - send all resolved data (first sync scenario)
                            let empty_existing = SourceData {
                                watchlist: Vec::new(),
                                ratings: Vec::new(),
                                reviews: Vec::new(),
                                watch_history: Vec::new(),
                            };
                            
                            if self.sync_options.sync_watchlist {
                                match strategy.prepare_watchlist(&resolved.watchlist, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(mut result) => {
                                        // Filter out watched items if remove_watched_from_watchlists is enabled
                                        if !watched_ids.is_empty() {
                                            let before_count = result.for_watchlist.len();
                                            result.for_watchlist.retain(|item| !watched_ids.contains(&item.imdb_id));
                                            if before_count > result.for_watchlist.len() {
                                                info!("Filtered out {} watched items from Trakt watchlist additions", before_count - result.for_watchlist.len());
                                            }
                                        }
                                        
                                        if !result.for_watchlist.is_empty() {
                                            if let Err(e) = source.as_mut().add_to_watchlist(&result.for_watchlist).await {
                                                errors.push(format!("Failed to add watchlist to Trakt: {}", e));
                                            } else {
                                                items_synced += result.for_watchlist.len();
                                                if let Err(e) = strategy.on_sync_complete("watchlist", result.for_watchlist.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                        
                                        if !result.for_watch_history.is_empty() && self.sync_options.sync_watch_history {
                                            if let Err(e) = source.as_mut().add_watch_history(&result.for_watch_history).await {
                                                errors.push(format!("Failed to add watch history to Trakt: {}", e));
                                            } else {
                                                items_synced += result.for_watch_history.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", result.for_watch_history.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watchlist for Trakt: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_ratings {
                                match strategy.prepare_ratings(&resolved.ratings, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.as_mut().set_ratings(&filtered).await {
                                                errors.push(format!("Failed to set ratings on Trakt: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("ratings", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare ratings for Trakt: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_reviews {
                                match strategy.prepare_reviews(&resolved.reviews, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.as_mut().set_reviews(&filtered).await {
                                                errors.push(format!("Failed to set reviews to Trakt: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("reviews", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare reviews for Trakt: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_watch_history {
                                match strategy.prepare_watch_history(&resolved.watch_history, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.add_watch_history(&filtered).await {
                                                errors.push(format!("Failed to add watch history to Trakt: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watch history for Trakt: {}", e));
                                    }
                                }
                            }
                        }
                }
                "simkl" => {
                    // Strategy already created above
                    if let Some(existing) = get_existing_data("simkl") {
                            if self.sync_options.sync_watchlist {
                                info!("Preparing watchlist for Simkl: {} resolved items, {} existing in Simkl", resolved.watchlist.len(), existing.watchlist.len());
                                match strategy.prepare_watchlist(&resolved.watchlist, existing, self.sync_options.force_full_sync) {
                                    Ok(mut result) => {
                                        // Get removal list for Simkl from centralized HashMap
                                        let removal_list = removal_lists.get("simkl").cloned().unwrap_or_default();
                                        
                                        // Filter out items that should be removed (by IMDB ID)
                                        if !removal_list.is_empty() {
                                            let removal_ids: std::collections::HashSet<String> = removal_list.iter()
                                                .map(|item| item.imdb_id.clone())
                                                .collect();
                                            let before_count = result.for_watchlist.len();
                                            result.for_watchlist.retain(|item| !removal_ids.contains(&item.imdb_id));
                                            if before_count > result.for_watchlist.len() {
                                                info!("Filtered out {} items from Simkl watchlist additions (watched or old)", before_count - result.for_watchlist.len());
                                            }
                                        }
                                        
                                        if !result.for_watchlist.is_empty() {
                                            info!("Filtering watchlist for Simkl: {} items to add (from {} resolved)", result.for_watchlist.len(), resolved.watchlist.len());
                                            if let Err(e) = source.as_mut().add_to_watchlist(&result.for_watchlist).await {
                                                errors.push(format!("Failed to add watchlist to Simkl: {}", e));
                                            } else {
                                                items_synced += result.for_watchlist.len();
                                                info!("Successfully added {} watchlist items to Simkl", result.for_watchlist.len());
                                                if let Err(e) = strategy.on_sync_complete("watchlist", result.for_watchlist.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.watchlist.is_empty() {
                                            info!("All {} resolved watchlist items already exist in Simkl (filtered out during preparation)", resolved.watchlist.len());
                                        } else {
                                            info!("No watchlist items to sync to Simkl (resolved watchlist is empty)");
                                        }
                                        
                                        // Remove items from centralized removal list
                                        if !removal_list.is_empty() {
                                            for item in &removal_list {
                                                debug!(
                                                    imdb_id = %item.imdb_id,
                                                    title = %item.title,
                                                    media_type = ?item.media_type,
                                                    "Removing from Simkl watchlist (watched or old)"
                                                );
                                            }
                                            if let Err(e) = source.as_mut().remove_from_watchlist(&removal_list).await {
                                                errors.push(format!("Failed to remove items from Simkl watchlist: {}", e));
                                            } else {
                                                info!("Removed {} items from Simkl watchlist", removal_list.len());
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watchlist for Simkl: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_ratings {
                                match strategy.prepare_ratings(&resolved.ratings, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering ratings for Simkl: {} items to add/update (from {} resolved)", filtered.len(), resolved.ratings.len());
                                            if let Err(e) = source.as_mut().set_ratings(&filtered).await {
                                                errors.push(format!("Failed to set ratings on Simkl: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("ratings", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.ratings.is_empty() {
                                            info!("All {} resolved ratings already exist in Simkl with same values", resolved.ratings.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare ratings for Simkl: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_reviews {
                                match strategy.prepare_reviews(&resolved.reviews, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering reviews for Simkl: {} items to add (from {} resolved)", filtered.len(), resolved.reviews.len());
                                            if let Err(e) = source.as_mut().set_reviews(&filtered).await {
                                                errors.push(format!("Failed to set reviews on Simkl: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("reviews", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.reviews.is_empty() {
                                            info!("All {} resolved reviews already exist in Simkl", resolved.reviews.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare reviews for Simkl: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_watch_history {
                                match strategy.prepare_watch_history(&resolved.watch_history, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering watch history for Simkl: {} items to add (from {} resolved)", filtered.len(), resolved.watch_history.len());
                                            if let Err(e) = source.add_watch_history(&filtered).await {
                                                errors.push(format!("Failed to add watch history to Simkl: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.watch_history.is_empty() {
                                            info!("All {} resolved watch history items already exist in Simkl", resolved.watch_history.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watch history for Simkl: {}", e));
                                    }
                                }
                            }
                        } else {
                            // No existing data - send all resolved data
                            let empty_existing = SourceData {
                                watchlist: Vec::new(),
                                ratings: Vec::new(),
                                reviews: Vec::new(),
                                watch_history: Vec::new(),
                            };
                            
                            if self.sync_options.sync_watchlist {
                                info!("Preparing watchlist for Simkl (no existing data): {} resolved items", resolved.watchlist.len());
                                match strategy.prepare_watchlist(&resolved.watchlist, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(mut result) => {
                                        // Filter out watched items if remove_watched_from_watchlists is enabled
                                        if !watched_ids.is_empty() {
                                            let before_count = result.for_watchlist.len();
                                            result.for_watchlist.retain(|item| !watched_ids.contains(&item.imdb_id));
                                            if before_count > result.for_watchlist.len() {
                                                info!("Filtered out {} watched items from Simkl watchlist additions", before_count - result.for_watchlist.len());
                                            }
                                        }
                                        
                                        if !result.for_watchlist.is_empty() {
                                            info!("Adding {} watchlist items to Simkl (no existing data)", result.for_watchlist.len());
                                            if let Err(e) = source.as_mut().add_to_watchlist(&result.for_watchlist).await {
                                                errors.push(format!("Failed to add watchlist to Simkl: {}", e));
                                            } else {
                                                items_synced += result.for_watchlist.len();
                                                info!("Successfully added {} watchlist items to Simkl", result.for_watchlist.len());
                                                if let Err(e) = strategy.on_sync_complete("watchlist", result.for_watchlist.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else {
                                            info!("No watchlist items to add to Simkl after preparation (all filtered out)");
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watchlist for Simkl: {}", e));
                                    }
                                }
                            }
                            if self.sync_options.sync_ratings {
                                match strategy.prepare_ratings(&resolved.ratings, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.as_mut().set_ratings(&filtered).await {
                                                errors.push(format!("Failed to set ratings on Simkl: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("ratings", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare ratings for Simkl: {}", e));
                                    }
                                }
                            }
                            if self.sync_options.sync_reviews {
                                match strategy.prepare_reviews(&resolved.reviews, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.as_mut().set_reviews(&filtered).await {
                                                errors.push(format!("Failed to set reviews on Simkl: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("reviews", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare reviews for Simkl: {}", e));
                                    }
                                }
                            }
                            if self.sync_options.sync_watch_history {
                                match strategy.prepare_watch_history(&resolved.watch_history, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.add_watch_history(&filtered).await {
                                                errors.push(format!("Failed to add watch history to Simkl: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watch history for Simkl: {}", e));
                                    }
                                }
                            }
                        }
                }
                "imdb" => {
                    // Strategy already created above
                    if let Some(existing) = get_existing_data("imdb") {
                            if self.sync_options.sync_watchlist {
                                match strategy.prepare_watchlist(&resolved.watchlist, existing, self.sync_options.force_full_sync) {
                                    Ok(mut result) => {
                                        // Get removal list for IMDB from centralized HashMap
                                        let removal_list = removal_lists.get("imdb").cloned().unwrap_or_default();
                                        
                                        // Filter out items that should be removed (by IMDB ID)
                                        if !removal_list.is_empty() {
                                            let removal_ids: std::collections::HashSet<String> = removal_list.iter()
                                                .map(|item| item.imdb_id.clone())
                                                .collect();
                                            let before_count = result.for_watchlist.len();
                                            result.for_watchlist.retain(|item| !removal_ids.contains(&item.imdb_id));
                                            if before_count > result.for_watchlist.len() {
                                                info!("Filtered out {} items from IMDB watchlist additions (watched or old)", before_count - result.for_watchlist.len());
                                            }
                                        }
                                        
                                        if !result.for_watchlist.is_empty() {
                                            info!("Filtering watchlist for IMDB: {} items to add to watchlist (from {} resolved)", result.for_watchlist.len(), resolved.watchlist.len());
                                            if let Err(e) = source.as_mut().add_to_watchlist(&result.for_watchlist).await {
                                                errors.push(format!("Failed to add watchlist to IMDB: {}", e));
                                            } else {
                                                items_synced += result.for_watchlist.len();
                                                if let Err(e) = strategy.on_sync_complete("watchlist", result.for_watchlist.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                        
                                        if !result.for_watch_history.is_empty() && self.sync_options.sync_watch_history {
                                            info!("Filtering check-ins for IMDB: {} items to add (from {} resolved check-in items, {} already exist in IMDB)", result.for_watch_history.len(), resolved.watchlist.len(), existing.watch_history.len());
                                            if let Err(e) = source.as_mut().add_watch_history(&result.for_watch_history).await {
                                                errors.push(format!("Failed to add check-ins to IMDB: {}", e));
                                            } else {
                                                items_synced += result.for_watch_history.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", result.for_watch_history.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                        
                                        if result.for_watchlist.is_empty() && result.for_watch_history.is_empty() && !resolved.watchlist.is_empty() {
                                            info!("All {} resolved watchlist items already exist in IMDB", resolved.watchlist.len());
                                        }
                                        
                                        // Remove items from centralized removal list
                                        if !removal_list.is_empty() {
                                            for item in &removal_list {
                                                debug!(
                                                    imdb_id = %item.imdb_id,
                                                    title = %item.title,
                                                    media_type = ?item.media_type,
                                                    "Removing from IMDB watchlist (watched or old)"
                                                );
                                            }
                                            if let Err(e) = source.as_mut().remove_from_watchlist(&removal_list).await {
                                                errors.push(format!("Failed to remove items from IMDB watchlist: {}", e));
                                            } else {
                                                info!("Removed {} items from IMDB watchlist", removal_list.len());
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watchlist for IMDB: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_ratings {
                                match strategy.prepare_ratings(&resolved.ratings, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering ratings for IMDB: {} items to add/update (from {} resolved)", filtered.len(), resolved.ratings.len());
                                            if let Err(e) = source.as_mut().set_ratings(&filtered).await {
                                                errors.push(format!("Failed to set ratings on IMDB: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("ratings", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.ratings.is_empty() {
                                            info!("All {} resolved ratings already exist in IMDB with same values", resolved.ratings.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare ratings for IMDB: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_reviews {
                                match strategy.prepare_reviews(&resolved.reviews, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering reviews for IMDB: {} items to add (from {} resolved)", filtered.len(), resolved.reviews.len());
                                            if let Err(e) = source.as_mut().set_reviews(&filtered).await {
                                                errors.push(format!("Failed to set reviews on IMDB: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("reviews", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.reviews.is_empty() {
                                            info!("All {} resolved reviews already exist in IMDB", resolved.reviews.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare reviews for IMDB: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_watch_history {
                                match strategy.prepare_watch_history(&resolved.watch_history, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering watch history for IMDB: {} items to add (from {} resolved)", filtered.len(), resolved.watch_history.len());
                                            if let Err(e) = source.add_watch_history(&filtered).await {
                                                errors.push(format!("Failed to add watch history to IMDB: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.watch_history.is_empty() {
                                            info!("All {} resolved watch history items already exist in IMDB", resolved.watch_history.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watch history for IMDB: {}", e));
                                    }
                                }
                            }
                        } else {
                            // No existing data - send all resolved data, split by status
                            let empty_existing = SourceData {
                                watchlist: Vec::new(),
                                ratings: Vec::new(),
                                reviews: Vec::new(),
                                watch_history: Vec::new(),
                            };
                            
                            if self.sync_options.sync_watchlist {
                                match strategy.prepare_watchlist(&resolved.watchlist, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(mut result) => {
                                        // Filter out watched items if remove_watched_from_watchlists is enabled
                                        if !watched_ids.is_empty() {
                                            let before_count = result.for_watchlist.len();
                                            result.for_watchlist.retain(|item| !watched_ids.contains(&item.imdb_id));
                                            if before_count > result.for_watchlist.len() {
                                                info!("Filtered out {} watched items from IMDB watchlist additions", before_count - result.for_watchlist.len());
                                            }
                                        }
                                        
                                        if !result.for_watchlist.is_empty() {
                                            if let Err(e) = source.as_mut().add_to_watchlist(&result.for_watchlist).await {
                                                errors.push(format!("Failed to add watchlist to IMDB: {}", e));
                                            } else {
                                                items_synced += result.for_watchlist.len();
                                                if let Err(e) = strategy.on_sync_complete("watchlist", result.for_watchlist.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                        
                                        if !result.for_watch_history.is_empty() && self.sync_options.sync_watch_history {
                                            if let Err(e) = source.as_mut().add_watch_history(&result.for_watch_history).await {
                                                errors.push(format!("Failed to add check-ins to IMDB: {}", e));
                                            } else {
                                                items_synced += result.for_watch_history.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", result.for_watch_history.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watchlist for IMDB: {}", e));
                                    }
                                }
                            }
                            if self.sync_options.sync_ratings {
                                match strategy.prepare_ratings(&resolved.ratings, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.as_mut().set_ratings(&filtered).await {
                                                errors.push(format!("Failed to set ratings on IMDB: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("ratings", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare ratings for IMDB: {}", e));
                                    }
                                }
                            }
                            if self.sync_options.sync_reviews {
                                match strategy.prepare_reviews(&resolved.reviews, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.as_mut().set_reviews(&filtered).await {
                                                errors.push(format!("Failed to set reviews on IMDB: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("reviews", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare reviews for IMDB: {}", e));
                                    }
                                }
                            }
                            if self.sync_options.sync_watch_history {
                                match strategy.prepare_watch_history(&resolved.watch_history, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.add_watch_history(&filtered).await {
                                                errors.push(format!("Failed to add watch history to IMDB: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watch history for IMDB: {}", e));
                                    }
                                }
                            }
                        }
                }
                "plex" => {
                    // Strategy already created above
                    if let Some(existing) = get_existing_data("plex") {
                            if self.sync_options.sync_watchlist {
                                // Still prepare watchlist to extract Completed/Watching items for watch_history
                                match strategy.prepare_watchlist(&resolved.watchlist, existing, self.sync_options.force_full_sync) {
                                    Ok(result) => {
                                        // Handle items that should go to watch_history (Completed/Watching status)
                                        if !result.for_watch_history.is_empty() && self.sync_options.sync_watch_history {
                                            info!("Filtering watchlist for Plex: {} items to add to watch history (from {} resolved)", result.for_watch_history.len(), resolved.watchlist.len());
                                            if let Err(e) = source.as_mut().add_watch_history(&result.for_watch_history).await {
                                                errors.push(format!("Failed to add watch history to Plex: {}", e));
                                            } else {
                                                items_synced += result.for_watch_history.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", result.for_watch_history.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                        
                                        // Add items to Plex watchlist
                                        if !result.for_watchlist.is_empty() {
                                            info!("Filtering watchlist for Plex: {} items to add to watchlist (from {} resolved)", result.for_watchlist.len(), resolved.watchlist.len());
                                            if let Err(e) = source.as_mut().add_to_watchlist(&result.for_watchlist).await {
                                                errors.push(format!("Failed to add watchlist to Plex: {}", e));
                                            } else {
                                                items_synced += result.for_watchlist.len();
                                                if let Err(e) = strategy.on_sync_complete("watchlist", result.for_watchlist.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                        
                                        // Remove items from existing watchlist using centralized removal list
                                        let removal_list = removal_lists.get("plex").cloned().unwrap_or_default();
                                        
                                        if !removal_list.is_empty() {
                                            for item in &removal_list {
                                                debug!(
                                                    imdb_id = %item.imdb_id,
                                                    title = %item.title,
                                                    media_type = ?item.media_type,
                                                    "Removing from Plex watchlist (watched or old)"
                                                );
                                            }
                                            if let Err(e) = source.as_mut().remove_from_watchlist(&removal_list).await {
                                                errors.push(format!("Failed to remove items from Plex watchlist: {}", e));
                                            } else {
                                                info!("Removed {} items from Plex watchlist", removal_list.len());
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watchlist for Plex: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_ratings {
                                match strategy.prepare_ratings(&resolved.ratings, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            // Plex uses 1-5 scale, resolved ratings are in 1-10 scale
                                            let normalized_ratings: Vec<Rating> = filtered
                                                .iter()
                                                .map(|r| {
                                                    // Convert from 1-10 to 1-5
                                                    let rating = ((r.rating as f32 / 2.0).round() as u8).max(1).min(5);
                                                    Rating { rating, ..r.clone() }
                                                })
                                                .collect();
                                            info!("Filtering ratings for Plex: {} items to add/update (from {} resolved)", normalized_ratings.len(), resolved.ratings.len());
                                            if let Err(e) = source.as_mut().set_ratings(&normalized_ratings).await {
                                                errors.push(format!("Failed to set ratings on Plex: {}", e));
                                            } else {
                                                items_synced += normalized_ratings.len();
                                                if let Err(e) = strategy.on_sync_complete("ratings", normalized_ratings.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.ratings.is_empty() {
                                            info!("All {} resolved ratings already exist in Plex with same values", resolved.ratings.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare ratings for Plex: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_reviews {
                                match strategy.prepare_reviews(&resolved.reviews, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering reviews for Plex: {} items to add (from {} resolved)", filtered.len(), resolved.reviews.len());
                                            if let Err(e) = source.as_mut().set_reviews(&filtered).await {
                                                errors.push(format!("Failed to set reviews on Plex: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("reviews", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.reviews.is_empty() {
                                            info!("All {} resolved reviews already exist in Plex", resolved.reviews.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare reviews for Plex: {}", e));
                                    }
                                }
                            }
                            
                            if self.sync_options.sync_watch_history {
                                match strategy.prepare_watch_history(&resolved.watch_history, existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            info!("Filtering watch history for Plex: {} items to add (from {} resolved)", filtered.len(), resolved.watch_history.len());
                                            if let Err(e) = source.add_watch_history(&filtered).await {
                                                errors.push(format!("Failed to add watch history to Plex: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        } else if !resolved.watch_history.is_empty() {
                                            info!("All {} resolved watch history items already exist in Plex", resolved.watch_history.len());
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watch history for Plex: {}", e));
                                    }
                                }
                            }
                        } else {
                            // No existing data - send all resolved data
                            let empty_existing = SourceData {
                                watchlist: Vec::new(),
                                ratings: Vec::new(),
                                reviews: Vec::new(),
                                watch_history: Vec::new(),
                            };
                            
                            if self.sync_options.sync_watchlist {
                                match strategy.prepare_watchlist(&resolved.watchlist, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(mut result) => {
                                        // Handle items that should go to watch_history (Completed/Watching status)
                                        if !result.for_watch_history.is_empty() && self.sync_options.sync_watch_history {
                                            info!("Filtering watchlist for Plex: {} items to add to watch history (from {} resolved)", result.for_watch_history.len(), resolved.watchlist.len());
                                            if let Err(e) = source.as_mut().add_watch_history(&result.for_watch_history).await {
                                                errors.push(format!("Failed to add watch history to Plex: {}", e));
                                            } else {
                                                items_synced += result.for_watch_history.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", result.for_watch_history.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                        
                                        // Get removal list for Plex from centralized HashMap
                                        let removal_list = removal_lists.get("plex").cloned().unwrap_or_default();
                                        
                                        // Filter out items that should be removed (by IMDB ID)
                                        if !removal_list.is_empty() {
                                            let removal_ids: std::collections::HashSet<String> = removal_list.iter()
                                                .map(|item| item.imdb_id.clone())
                                                .collect();
                                            result.for_watchlist.retain(|item| !removal_ids.contains(&item.imdb_id));
                                        }
                                        
                                        // Add items to Plex watchlist
                                        if !result.for_watchlist.is_empty() {
                                            info!("Filtering watchlist for Plex: {} items to add to watchlist (from {} resolved)", result.for_watchlist.len(), resolved.watchlist.len());
                                            if let Err(e) = source.as_mut().add_to_watchlist(&result.for_watchlist).await {
                                                errors.push(format!("Failed to add watchlist to Plex: {}", e));
                                            } else {
                                                items_synced += result.for_watchlist.len();
                                                if let Err(e) = strategy.on_sync_complete("watchlist", result.for_watchlist.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watchlist for Plex: {}", e));
                                    }
                                }
                            }
                            if self.sync_options.sync_ratings {
                                match strategy.prepare_ratings(&resolved.ratings, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            // Plex uses 1-5 scale, resolved ratings are in 1-10 scale
                                            let normalized_ratings: Vec<Rating> = filtered
                                                .iter()
                                                .map(|r| {
                                                    // Convert from 1-10 to 1-5
                                                    let rating = ((r.rating as f32 / 2.0).round() as u8).max(1).min(5);
                                                    Rating { rating, ..r.clone() }
                                                })
                                                .collect();
                                            if let Err(e) = source.as_mut().set_ratings(&normalized_ratings).await {
                                                errors.push(format!("Failed to set ratings on Plex: {}", e));
                                            } else {
                                                items_synced += normalized_ratings.len();
                                                if let Err(e) = strategy.on_sync_complete("ratings", normalized_ratings.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare ratings for Plex: {}", e));
                                    }
                                }
                            }
                            if self.sync_options.sync_reviews {
                                match strategy.prepare_reviews(&resolved.reviews, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.as_mut().set_reviews(&filtered).await {
                                                errors.push(format!("Failed to set reviews on Plex: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("reviews", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare reviews for Plex: {}", e));
                                    }
                                }
                            }
                            if self.sync_options.sync_watch_history {
                                match strategy.prepare_watch_history(&resolved.watch_history, &empty_existing, self.sync_options.force_full_sync) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() {
                                            if let Err(e) = source.add_watch_history(&filtered).await {
                                                errors.push(format!("Failed to add watch history to Plex: {}", e));
                                            } else {
                                                items_synced += filtered.len();
                                                if let Err(e) = strategy.on_sync_complete("watch_history", filtered.len()) {
                                                    warn!("Failed to update sync timestamp: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("Failed to prepare watch history for Plex: {}", e));
                                    }
                                }
                            }
                        }
                }
                _ => {
                    errors.push(format!("Unknown source in source_preference: {}", source_name));
                }
            }
        }
        
        Ok(items_synced)
    }
    
    async fn sync_source_ratings_static(
        source_name: &str,
        source: &mut dyn MediaSource<Error = SourceError>,
        trakt_ratings: &[Rating],
        trakt: &mut dyn MediaSource<Error = SourceError>,
    ) -> Result<()> {
        info!(
            operation = "sync_source_start",
            source = source_name,
            "Starting {} sync",
            source_name
        );

        // Fetch source ratings
        let source_ratings = source.get_ratings().await?;
        info!(
            operation = "fetch_ratings",
            source = source_name,
            count = source_ratings.len(),
            "Fetched ratings from {}",
            source_name
        );

        // Normalize source ratings to Trakt format (1-10 scale) and push to Trakt
        let trakt_scale = 10u8;
        let source_ratings_for_trakt: Vec<Rating> = if let Some(normalizer) = source.as_rating_normalization() {
            source_ratings
                .into_iter()
                .map(|r| Rating {
                    rating: normalizer.normalize_rating(r.rating as f64, trakt_scale),
                    ..r
                })
                .collect()
        } else {
            // Fallback: assume same scale
            source_ratings
        };

        if !source_ratings_for_trakt.is_empty() {
            trakt.set_ratings(&source_ratings_for_trakt).await?;
            info!(
                operation = "push_to_trakt",
                source = source_name,
                count = source_ratings_for_trakt.len(),
                "Pushed {} ratings to Trakt",
                source_ratings_for_trakt.len()
            );
        }

        // Replicate Trakt ratings back to source (denormalize from Trakt format)
        let trakt_ratings_for_source: Vec<Rating> = if let Some(normalizer) = source.as_rating_normalization() {
            let source_scale = normalizer.native_rating_scale();
            trakt_ratings
                .iter()
                .map(|r| Rating {
                    rating: normalizer.denormalize_rating(r.rating, source_scale) as u8,
                    ..r.clone()
                })
                .collect()
        } else {
            // Fallback: assume same scale
            trakt_ratings.iter().cloned().collect()
        };

        if !trakt_ratings_for_source.is_empty() {
            source.set_ratings(&trakt_ratings_for_source).await?;
            info!(
                operation = "replicate_to_source",
                source = source_name,
                count = trakt_ratings_for_source.len(),
                "Replicated {} ratings to {}",
                trakt_ratings_for_source.len(),
                source_name
            );
        }

        info!(
            operation = "sync_source_complete",
            source = source_name,
            "{} sync completed",
            source_name
        );

        Ok(())
    }

    /// Sync IMDB with Trakt (respects sync_options to only sync what's requested)
    /// Implements all advanced features from the Python script
    /// Returns the number of items synced
    async fn sync_imdb(
        imdb: &mut dyn MediaSource<Error = SourceError>,
        trakt: &mut dyn MediaSource<Error = SourceError>,
        sync_options: &SyncOptions,
        config_sync_options: &media_sync_config::SyncOptions,
        cache_manager: &CacheManager,
        use_cache: &std::collections::HashSet<String>,
    ) -> Result<usize> {
        use std::collections::HashSet;
        use chrono::Utc;

        info!("Starting IMDB sync");

        // Fetch all data (only what's needed based on sync options)
        // When specific flags are passed, only fetch data for those flags
        // Advanced features (remove_watched_from_watchlists, mark_rated_as_watched) require additional data
        let mut imdb_watchlist = Vec::new();
        let mut trakt_watchlist = Vec::new();
        let mut imdb_ratings = Vec::new();
        let mut trakt_ratings = Vec::new();
        let mut imdb_reviews = Vec::new();
        let mut trakt_reviews = Vec::new();
        let mut imdb_history = Vec::new();
        let mut trakt_history = Vec::new();

        // Check if any specific sync options are set (if all are false, it's a full sync)
        let any_specific_sync = sync_options.sync_watchlist 
            || sync_options.sync_ratings 
            || sync_options.sync_reviews 
            || sync_options.sync_watch_history;
        
        // Only fetch watchlist if explicitly requested OR if advanced feature needs it (and we're doing a full sync)
        let should_fetch_watchlist = sync_options.sync_watchlist 
            || (config_sync_options.remove_watched_from_watchlists && !any_specific_sync);
        info!("Watchlist fetch check: sync_watchlist={}, should_fetch={}", sync_options.sync_watchlist, should_fetch_watchlist);
        if should_fetch_watchlist {
            info!("Fetching watchlist data");
            let mut errors = Vec::new();
            imdb_watchlist = filter_missing_imdb_ids(
                Self::fetch_or_cache_watchlist(imdb, cache_manager, "imdb", use_cache, sync_options.force_full_sync, &mut errors).await
            );
            info!("Fetched {} IMDB watchlist items", imdb_watchlist.len());
            
            trakt_watchlist = filter_missing_imdb_ids(
                Self::fetch_or_cache_watchlist(trakt, cache_manager, "trakt", use_cache, sync_options.force_full_sync, &mut errors).await
            );
            info!("Fetched {} Trakt watchlist items", trakt_watchlist.len());
            info!("Total: {} IMDB watchlist items, {} Trakt watchlist items", imdb_watchlist.len(), trakt_watchlist.len());
            
            // Debug: Log first few items from each source
            for (idx, item) in imdb_watchlist.iter().take(5).enumerate() {
                debug!(
                    idx = idx,
                    imdb_id = %item.imdb_id,
                    title = %item.title,
                    "IMDB watchlist item"
                );
            }
            for (idx, item) in trakt_watchlist.iter().take(5).enumerate() {
                debug!(
                    idx = idx,
                    imdb_id = %item.imdb_id,
                    title = %item.title,
                    "Trakt watchlist item"
                );
            }
        }

        if sync_options.sync_ratings || (config_sync_options.mark_rated_as_watched && !any_specific_sync) {
            info!("Fetching ratings data (sync_ratings={}, mark_rated_as_watched={}, any_specific_sync={})", 
                sync_options.sync_ratings, config_sync_options.mark_rated_as_watched, any_specific_sync);
            let mut errors = Vec::new();
            imdb_ratings = filter_missing_imdb_ids(
                Self::fetch_or_cache_ratings(imdb, cache_manager, "imdb", use_cache, sync_options.force_full_sync, &mut errors).await
            );
            info!("Fetched {} IMDB ratings", imdb_ratings.len());
            // Debug: Log first few ratings from each source
            for (idx, rating) in imdb_ratings.iter().take(5).enumerate() {
                debug!(
                    idx = idx,
                    imdb_id = %rating.imdb_id,
                    rating = rating.rating,
                    "IMDB rating"
                );
            }
            
            trakt_ratings = filter_missing_imdb_ids(
                Self::fetch_or_cache_ratings(trakt, cache_manager, "trakt", use_cache, sync_options.force_full_sync, &mut errors).await
            );
            info!("Fetched {} Trakt ratings", trakt_ratings.len());
            info!("Total: {} IMDB ratings, {} Trakt ratings", imdb_ratings.len(), trakt_ratings.len());
        } else {
            info!("Skipping ratings fetch (sync_ratings={}, mark_rated_as_watched={}, any_specific_sync={})", 
                sync_options.sync_ratings, config_sync_options.mark_rated_as_watched, any_specific_sync);
        }

        if sync_options.sync_reviews {
            info!("Fetching reviews data");
            let mut errors = Vec::new();
            imdb_reviews = Self::fetch_or_cache_reviews(imdb, cache_manager, "imdb", use_cache, sync_options.force_full_sync, &mut errors).await;
            imdb_reviews.retain(|r| !r.imdb_id.is_empty());
            info!("Fetched {} IMDB reviews", imdb_reviews.len());
            
            trakt_reviews = Self::fetch_or_cache_reviews(trakt, cache_manager, "trakt", use_cache, sync_options.force_full_sync, &mut errors).await;
            trakt_reviews.retain(|r| !r.imdb_id.is_empty());
            info!("Fetched {} Trakt reviews", trakt_reviews.len());
        }

        // Only fetch watch history if explicitly requested OR if advanced feature needs it (and we're doing a full sync)
        let should_fetch_watch_history = sync_options.sync_watch_history
            || (config_sync_options.remove_watched_from_watchlists && !any_specific_sync)
            || (config_sync_options.mark_rated_as_watched && !any_specific_sync);
        if should_fetch_watch_history {
            info!("Fetching watch history data (sync_watch_history={}, remove_watched_from_watchlists={}, mark_rated_as_watched={}, any_specific_sync={})", 
                sync_options.sync_watch_history, config_sync_options.remove_watched_from_watchlists, config_sync_options.mark_rated_as_watched, any_specific_sync);
            let mut errors = Vec::new();
            imdb_history = filter_missing_imdb_ids(
                Self::fetch_or_cache_watch_history(imdb, cache_manager, "imdb", use_cache, sync_options.force_full_sync, &mut errors).await
            );
            info!("Fetched {} IMDB watch history items", imdb_history.len());
            
            trakt_history = filter_missing_imdb_ids(
                Self::fetch_or_cache_watch_history(trakt, cache_manager, "trakt", use_cache, sync_options.force_full_sync, &mut errors).await
            );
            info!("Fetched {} Trakt watch history items", trakt_history.len());
        } else {
            info!("Skipping watch history fetch (sync_watch_history={}, remove_watched_from_watchlists={}, mark_rated_as_watched={}, any_specific_sync={})",
                sync_options.sync_watch_history, config_sync_options.remove_watched_from_watchlists, config_sync_options.mark_rated_as_watched, any_specific_sync);
        }

        // Check IMDB limits
        let imdb_watchlist_limit_reached = imdb_watchlist.len() >= 10_000;
        let imdb_watch_history_limit_reached = imdb_history.len() >= 10_000;

        // Remove duplicates from watch history
        trakt_history = crate::diff::remove_duplicates_by_imdb_id(trakt_history);

        // Calculate initial diffs
        info!("Calculating watchlist diffs: {} IMDB items, {} Trakt items", imdb_watchlist.len(), trakt_watchlist.len());
        let mut imdb_watchlist_to_set = if sync_options.sync_watchlist {
            let items = filter_items_by_imdb_id(&imdb_watchlist, &trakt_watchlist);
            info!("Found {} IMDB watchlist items to add to Trakt (after filtering against {} Trakt items)", items.len(), trakt_watchlist.len());
            if items.is_empty() && !imdb_watchlist.is_empty() {
                info!("All {} IMDB watchlist items already exist in Trakt", imdb_watchlist.len());
            }
            // Warn about potential stale data if we're adding many items
            if items.len() > 10 {
                warn!("Adding {} items from IMDB to Trakt. If items keep reappearing after removal, IMDB's export may be stale. Wait a few minutes after removing items from IMDB before syncing.", items.len());
            }
            for (idx, item) in items.iter().take(5).enumerate() {
                debug!(
                    idx = idx,
                    imdb_id = %item.imdb_id,
                    title = %item.title,
                    "IMDB watchlist item to sync to Trakt"
                );
            }
            items
        } else {
            info!("Watchlist sync disabled, skipping diff calculation");
            Vec::new()
        };
        let mut trakt_watchlist_to_set = if sync_options.sync_watchlist {
            let items = filter_items_by_imdb_id(&trakt_watchlist, &imdb_watchlist);
            info!("Found {} Trakt watchlist items to add to IMDB", items.len());
            items
        } else {
            Vec::new()
        };

        // Calculate ratings diffs
        info!("Calculating ratings diffs: {} IMDB ratings, {} Trakt ratings", imdb_ratings.len(), trakt_ratings.len());
        
        // Add debug logging before filtering ratings
        debug!(
            "Before filtering ratings: IMDB ratings count={}, Trakt ratings count={}",
            imdb_ratings.len(),
            trakt_ratings.len()
        );
        
        // Log sample IMDB ratings
        for (idx, rating) in imdb_ratings.iter().take(5).enumerate() {
            debug!(
                "IMDB rating sample[{}]: imdb_id={}, rating={}, date_added={}, media_type={:?}",
                idx, rating.imdb_id, rating.rating, rating.date_added, rating.media_type
            );
        }
        
        // Log sample Trakt ratings
        for (idx, rating) in trakt_ratings.iter().take(5).enumerate() {
            debug!(
                "Trakt rating sample[{}]: imdb_id={}, rating={}, date_added={}, media_type={:?}",
                idx, rating.imdb_id, rating.rating, rating.date_added, rating.media_type
            );
        }
        
        let mut imdb_ratings_to_set = if sync_options.sync_ratings {
            let items = filter_items_by_imdb_id(&imdb_ratings, &trakt_ratings);
            
            // Filter out episodes with placeholder season/episode (0, 0) - they likely won't match in Trakt
            let before_filter = items.len();
            let filtered_items: Vec<_> = items.into_iter()
                .filter(|rating| {
                    if let media_sync_models::media::MediaType::Episode { season, episode } = rating.media_type {
                        if season == 0 && episode == 0 {
                            debug!(
                                "Skipping Episode rating with placeholder season/episode (0, 0): imdb_id={}",
                                rating.imdb_id
                            );
                            return false;
                        }
                    }
                    true
                })
                .collect();
            
            if before_filter != filtered_items.len() {
                info!(
                    "Filtered out {} episode ratings with placeholder season/episode numbers",
                    before_filter - filtered_items.len()
                );
            }
            
            info!("Found {} IMDB ratings to add to Trakt (after filtering against {} Trakt ratings)", filtered_items.len(), trakt_ratings.len());
            
            // Log which ratings are being added
            for (idx, rating) in filtered_items.iter().take(10).enumerate() {
                debug!(
                    "IMDB rating to sync[{}]: imdb_id={}, rating={}, date_added={}, media_type={:?}",
                    idx, rating.imdb_id, rating.rating, rating.date_added, rating.media_type
                );
            }
            
            if filtered_items.is_empty() && !imdb_ratings.is_empty() {
                info!("All {} IMDB ratings already exist in Trakt", imdb_ratings.len());
            }
            filtered_items
        } else {
            info!("Ratings sync disabled, skipping diff calculation");
            Vec::new()
        };
        let mut trakt_ratings_to_set = if sync_options.sync_ratings {
            let items = filter_items_by_imdb_id(&trakt_ratings, &imdb_ratings);
            info!("Found {} Trakt ratings to add to IMDB (after filtering against {} IMDB ratings)", items.len(), imdb_ratings.len());
            items
        } else {
            Vec::new()
        };

        let mut imdb_reviews_to_set = if sync_options.sync_reviews {
            // Use content-aware filtering for reviews to prevent duplicates
            let items = crate::diff::filter_reviews_by_imdb_id_and_content(&imdb_reviews, &trakt_reviews);
            info!("Found {} IMDB reviews to add to Trakt (after filtering against {} Trakt reviews by IMDB ID and content)", items.len(), trakt_reviews.len());
            if items.is_empty() && !imdb_reviews.is_empty() {
                info!("All {} IMDB reviews already exist in Trakt", imdb_reviews.len());
            }
            items
        } else {
            Vec::new()
        };
        let mut trakt_reviews_to_set = if sync_options.sync_reviews {
            // Use content-aware filtering for reviews to prevent duplicates
            let items = crate::diff::filter_reviews_by_imdb_id_and_content(&trakt_reviews, &imdb_reviews);
            info!("Found {} Trakt reviews to add to IMDB (after filtering against {} IMDB reviews by IMDB ID and content)", items.len(), imdb_reviews.len());
            if items.is_empty() && !trakt_reviews.is_empty() {
                info!("All {} Trakt reviews already exist in IMDB", trakt_reviews.len());
            }
            items
        } else {
            Vec::new()
        };

        let mut imdb_history_to_set = if sync_options.sync_watch_history {
            // Add debug logging before filtering
            debug!(
                "Before filtering: IMDB history count={}, Trakt history count={}",
                imdb_history.len(),
                trakt_history.len()
            );
            
            // Log sample IMDB items
            for (idx, item) in imdb_history.iter().take(5).enumerate() {
                debug!(
                    "IMDB history sample[{}]: imdb_id={}, watched_at={}, media_type={:?}",
                    idx, item.imdb_id, item.watched_at, item.media_type
                );
            }
            
            // Log sample Trakt items
            for (idx, item) in trakt_history.iter().take(5).enumerate() {
                debug!(
                    "Trakt history sample[{}]: imdb_id={}, watched_at={}, media_type={:?}",
                    idx, item.imdb_id, item.watched_at, item.media_type
                );
            }
            
            let items = filter_items_by_imdb_id(&imdb_history, &trakt_history);
            let items_before_filter = items.len();
            
            // Filter out Shows BEFORE adding to the list (Trakt doesn't support shows in watch history)
            let mut filtered_items: Vec<_> = items.into_iter()
                .filter(|item| {
                    let is_show = matches!(item.media_type, media_sync_models::media::MediaType::Show);
                    if is_show {
                        debug!(
                            "Skipping Show from IMDB watch history (Trakt doesn't support shows in watch history): imdb_id={}",
                            item.imdb_id
                        );
                    }
                    !is_show
                })
                .collect();
            
            let shows_filtered = items_before_filter - filtered_items.len();
            
            // Filter out episodes with placeholder season/episode numbers (0, 0)
            let before_episode_filter = filtered_items.len();
            filtered_items.retain(|item| {
                if let media_sync_models::media::MediaType::Episode { season, episode } = item.media_type {
                    if season == 0 && episode == 0 {
                        debug!(
                            "Skipping Episode with placeholder season/episode (0, 0) from IMDB watch history: imdb_id={}",
                            item.imdb_id
                        );
                        return false;
                    }
                }
                true
            });
            
            let placeholder_episodes_filtered = before_episode_filter - filtered_items.len();
            
            if shows_filtered > 0 || placeholder_episodes_filtered > 0 {
                info!(
                    "Filtered out {} Shows and {} episodes with placeholder season/episode numbers from IMDB watch history",
                    shows_filtered,
                    placeholder_episodes_filtered
                );
            }
            
            info!("Found {} IMDB watch history items to add to Trakt (after filtering against {} Trakt watch history items)", filtered_items.len(), trakt_history.len());
            
            // Log which items are being added
            for (idx, item) in filtered_items.iter().take(10).enumerate() {
                debug!(
                    "IMDB item to sync[{}]: imdb_id={}, watched_at={}, media_type={:?}",
                    idx, item.imdb_id, item.watched_at, item.media_type
                );
            }
            
            filtered_items
        } else {
            Vec::new()
        };
        let mut trakt_history_to_set = if sync_options.sync_watch_history {
            filter_items_by_imdb_id(&trakt_history, &imdb_history)
        } else {
            Vec::new()
        };

        // Advanced feature: Mark rated as watched
        // This feature creates watch history entries from ratings, and should run whenever:
        // 1. The feature is enabled in config
        // 2. Ratings are available (either synced or fetched for this feature)
        // The created watch history entries will be synced if sync_watch_history is enabled
        if config_sync_options.mark_rated_as_watched 
            && (!imdb_ratings.is_empty() || !trakt_ratings.is_empty()) {
            info!("Running mark_rated_as_watched feature (ratings available: {} IMDB, {} Trakt)", 
                imdb_ratings.len(), trakt_ratings.len());
            
            // Ensure we have watch history data to check against (fetch if needed)
            if trakt_history.is_empty() && imdb_history.is_empty() {
                info!("Fetching watch history to check for existing entries (needed for mark_rated_as_watched)");
                let mut errors = Vec::new();
                imdb_history = filter_missing_imdb_ids(
                    Self::fetch_or_cache_watch_history(imdb, cache_manager, "imdb", use_cache, sync_options.force_full_sync, &mut errors).await
                );
                info!("Fetched {} IMDB watch history items for mark_rated_as_watched check", imdb_history.len());
                
                trakt_history = filter_missing_imdb_ids(
                    Self::fetch_or_cache_watch_history(trakt, cache_manager, "trakt", use_cache, sync_options.force_full_sync, &mut errors).await
                );
                info!("Fetched {} Trakt watch history items for mark_rated_as_watched check", trakt_history.len());
            }
            
            let mut combined_ratings = trakt_ratings.clone();
            combined_ratings.extend(imdb_ratings.clone());
            let combined_ratings = crate::diff::remove_duplicates_by_imdb_id(combined_ratings);

            let trakt_history_ids: HashSet<String> = trakt_history.iter().map(|h| h.imdb_id.clone()).collect();
            let imdb_history_ids: HashSet<String> = imdb_history.iter().map(|h| h.imdb_id.clone()).collect();

            let mut items_marked = 0;
            for rating in &combined_ratings {
                // Skip shows (cannot be marked as watched on Trakt)
                if matches!(rating.media_type, media_sync_models::media::MediaType::Show) {
                    continue;
                }

                if !trakt_history_ids.contains(&rating.imdb_id)
                    && !imdb_history_ids.contains(&rating.imdb_id)
                {
                    info!(
                        imdb_id = %rating.imdb_id,
                        rating = rating.rating,
                        media_type = ?rating.media_type,
                        "Marking rated item as watched (mark_rated_as_watched feature)"
                    );
                    let history_item = WatchHistory {
                        imdb_id: rating.imdb_id.clone(),
                        ids: rating.ids.clone(),
                        title: None,
                        year: None,
                        watched_at: rating.date_added,
                        media_type: rating.media_type.clone(),
                        source: "rated".to_string(),
                    };

                    imdb_history_to_set.push(history_item.clone());
                    trakt_history_to_set.push(history_item.clone());

                    // Add to history lists to prevent re-adding
                    imdb_history.push(history_item.clone());
                    trakt_history.push(history_item);
                    items_marked += 1;
                }
            }

            if items_marked > 0 {
                info!("Marked {} rated items as watched via mark_rated_as_watched feature", items_marked);
            } else {
                info!("No new items to mark as watched (all rated items already in watch history)");
            }

            // Remove duplicates
            imdb_history = crate::diff::remove_duplicates_by_imdb_id(imdb_history);
            trakt_history = crate::diff::remove_duplicates_by_imdb_id(trakt_history);
        } else if config_sync_options.mark_rated_as_watched {
            info!("mark_rated_as_watched is enabled but no ratings are available to process");
        }

        // Advanced feature: Rating updates (prefer more recent rating)
        if sync_options.sync_ratings {
            use std::collections::HashMap;

            let imdb_ratings_dict: HashMap<_, _> = imdb_ratings.iter().map(|r| (r.imdb_id.clone(), r.clone())).collect();
            let trakt_ratings_dict: HashMap<_, _> = trakt_ratings.iter().map(|r| (r.imdb_id.clone(), r.clone())).collect();

            let mut imdb_ratings_to_update = Vec::new();
            let mut trakt_ratings_to_update = Vec::new();

            for (imdb_id, imdb_rating) in &imdb_ratings_dict {
                if let Some(trakt_rating) = trakt_ratings_dict.get(imdb_id) {
                    if imdb_rating.rating != trakt_rating.rating {
                        // Check if dates are on different days
                        let imdb_date = imdb_rating.date_added.date_naive();
                        let trakt_date = trakt_rating.date_added.date_naive();

                        if imdb_date != trakt_date {
                            // Prefer more recent
                            if imdb_rating.date_added > trakt_rating.date_added {
                                debug!(
                                    imdb_id = %imdb_id,
                                    imdb_rating = imdb_rating.rating,
                                    trakt_rating = trakt_rating.rating,
                                    imdb_date = %imdb_rating.date_added,
                                    trakt_date = %trakt_rating.date_added,
                                    "Updating Trakt rating (IMDB rating is more recent)"
                                );
                                trakt_ratings_to_update.push((*imdb_rating).clone());
                            } else {
                                debug!(
                                    imdb_id = %imdb_id,
                                    imdb_rating = imdb_rating.rating,
                                    trakt_rating = trakt_rating.rating,
                                    imdb_date = %imdb_rating.date_added,
                                    trakt_date = %trakt_rating.date_added,
                                    "Updating IMDB rating (Trakt rating is more recent)"
                                );
                                imdb_ratings_to_update.push((*trakt_rating).clone());
                            }
                        }
                    }
                }
            }

            imdb_ratings_to_set.extend(imdb_ratings_to_update);
            trakt_ratings_to_set.extend(trakt_ratings_to_update);
        }

        // Advanced feature: Remove watched from watchlists
        let mut trakt_watchlist_to_remove = Vec::new();
        let mut imdb_watchlist_to_remove = Vec::new();

        if config_sync_options.remove_watched_from_watchlists {
            let mut watched_content = trakt_history.clone();
            watched_content.extend(imdb_history.clone());
            let watched_content = crate::diff::remove_duplicates_by_imdb_id(watched_content);

            let watched_ids: HashSet<String> = watched_content.iter().map(|h| h.imdb_id.clone()).collect();

            // Filter out watched items from to_set lists
            imdb_watchlist_to_set.retain(|item| !watched_ids.contains(&item.imdb_id));
            trakt_watchlist_to_set.retain(|item| !watched_ids.contains(&item.imdb_id));

            // Find items to remove from existing watchlists
            trakt_watchlist_to_remove = trakt_watchlist
                .iter()
                .filter(|item| {
                    let should_remove = watched_ids.contains(&item.imdb_id);
                    if should_remove {
                        debug!(
                            imdb_id = %item.imdb_id,
                            title = %item.title,
                            media_type = ?item.media_type,
                            "Removing from Trakt watchlist (item has been watched)"
                        );
                    }
                    should_remove
                })
                .cloned()
                .collect();
            imdb_watchlist_to_remove = imdb_watchlist
                .iter()
                .filter(|item| {
                    let should_remove = watched_ids.contains(&item.imdb_id);
                    if should_remove {
                        debug!(
                            imdb_id = %item.imdb_id,
                            title = %item.title,
                            media_type = ?item.media_type,
                            "Removing from IMDB watchlist (item has been watched)"
                        );
                    }
                    should_remove
                })
                .cloned()
                .collect();

            // Sort by date
            trakt_watchlist_to_remove.sort_by_key(|item| item.date_added);
            imdb_watchlist_to_remove.sort_by_key(|item| item.date_added);
        }

        // Advanced feature: Remove old watchlist items
        if let Some(days) = config_sync_options.remove_watchlist_items_older_than_days {
            let mut combined_watchlist = trakt_watchlist.clone();
            combined_watchlist.extend(imdb_watchlist.clone());
            let combined_watchlist = crate::diff::remove_duplicates_by_imdb_id(combined_watchlist);

            let cutoff = Utc::now() - chrono::Duration::days(days as i64);

            let old_items: Vec<_> = combined_watchlist
                .iter()
                .filter(|item| item.date_added < cutoff)
                .cloned()
                .collect();

            trakt_watchlist_to_remove.extend(old_items.clone());
            imdb_watchlist_to_remove.extend(old_items.clone());

            let old_ids: HashSet<String> = old_items.iter().map(|item| item.imdb_id.clone()).collect();
            imdb_watchlist_to_set.retain(|item| !old_ids.contains(&item.imdb_id));
            trakt_watchlist_to_set.retain(|item| !old_ids.contains(&item.imdb_id));
        }

        // Advanced feature: Filter reviews by length (minimum 600 characters)
        if sync_options.sync_reviews {
            let before_count = imdb_reviews_to_set.len();
            let mut filtered_count = 0;
            let mut filtered_reviews = Vec::new();
            
            imdb_reviews_to_set.retain(|review| {
                if review.content.len() >= 600 {
                    true
                } else {
                    filtered_count += 1;
                    filtered_reviews.push((review.imdb_id.clone(), review.content.len()));
                    false
                }
            });
            
            if filtered_count > 0 {
                warn!(
                    "Filtered out {} IMDB reviews shorter than 600 characters ({} remaining). Reviews filtered: {:?}",
                    filtered_count,
                    imdb_reviews_to_set.len(),
                    filtered_reviews.iter().take(5).map(|(id, len)| format!("{} ({} chars)", id, len)).collect::<Vec<_>>()
                );
            } else if before_count > 0 {
                info!("All {} IMDB reviews meet the 600 character minimum", before_count);
            }
            
            // Also filter Trakt reviews going to IMDB
            let trakt_before_count = trakt_reviews_to_set.len();
            let mut trakt_filtered_count = 0;
            trakt_reviews_to_set.retain(|review| {
                if review.content.len() >= 600 {
                    true
                } else {
                    trakt_filtered_count += 1;
                    false
                }
            });
            
            if trakt_filtered_count > 0 {
                warn!(
                    "Filtered out {} Trakt reviews shorter than 600 characters ({} remaining)",
                    trakt_filtered_count,
                    trakt_reviews_to_set.len()
                );
            } else if trakt_before_count > 0 {
                info!("All {} Trakt reviews meet the 600 character minimum", trakt_before_count);
            }
        }

        // Advanced feature: Remove shows from Trakt watch history
        trakt_history_to_set.retain(|item| !matches!(item.media_type, media_sync_models::media::MediaType::Show));

        // Sort all lists by date
        imdb_ratings_to_set.sort_by_key(|r| r.date_added);
        trakt_ratings_to_set.sort_by_key(|r| r.date_added);
        imdb_watchlist_to_set.sort_by_key(|w| w.date_added);
        trakt_watchlist_to_set.sort_by_key(|w| w.date_added);
        imdb_history_to_set.sort_by_key(|h| h.watched_at);
        trakt_history_to_set.sort_by_key(|h| h.watched_at);

        // Apply IMDB limits
        if sync_options.sync_watchlist && imdb_watchlist_limit_reached {
            imdb_watchlist_to_set.clear();
            warn!("IMDB watchlist limit (10,000) reached, skipping additions");
        }

        if (sync_options.sync_watch_history || config_sync_options.mark_rated_as_watched)
            && imdb_watch_history_limit_reached
        {
            imdb_history_to_set.clear();
            warn!("IMDB watch history limit (10,000) reached, skipping additions");
        }

        // Track total items synced
        let mut items_synced = 0;

        // Sync watchlists
        if sync_options.sync_watchlist {
            info!("Syncing watchlists: {} IMDB items to add to Trakt, {} Trakt items to add to IMDB", 
                imdb_watchlist_to_set.len(), trakt_watchlist_to_set.len());
            
            if !imdb_watchlist_to_set.is_empty() {
                info!("Preparing to add {} IMDB watchlist items to Trakt", imdb_watchlist_to_set.len());
                for item in &imdb_watchlist_to_set {
                    debug!(
                        imdb_id = %item.imdb_id,
                        title = %item.title,
                        media_type = ?item.media_type,
                        "Adding to Trakt watchlist"
                    );
                }
                trakt.add_to_watchlist(&imdb_watchlist_to_set).await?;
                items_synced += imdb_watchlist_to_set.len();
                info!("Successfully added {} items to Trakt watchlist", imdb_watchlist_to_set.len());
            } else {
                info!("No IMDB watchlist items to add to Trakt (all items already exist in Trakt)");
            }
            
            if !trakt_watchlist_to_set.is_empty() && !imdb_watchlist_limit_reached {
                for item in &trakt_watchlist_to_set {
                    debug!(
                        imdb_id = %item.imdb_id,
                        title = %item.title,
                        media_type = ?item.media_type,
                        "Adding to IMDB watchlist"
                    );
                }
                imdb.add_to_watchlist(&trakt_watchlist_to_set).await?;
                items_synced += trakt_watchlist_to_set.len();
                info!("Added {} items to IMDB watchlist", trakt_watchlist_to_set.len());
            } else if trakt_watchlist_to_set.is_empty() {
                info!("No Trakt watchlist items to add to IMDB (all items already exist in IMDB)");
            }

            // Remove from watchlists
            if !trakt_watchlist_to_remove.is_empty() {
                for item in &trakt_watchlist_to_remove {
                    debug!(
                        imdb_id = %item.imdb_id,
                        title = %item.title,
                        media_type = ?item.media_type,
                        "Removing from Trakt watchlist"
                    );
                }
                trakt.remove_from_watchlist(&trakt_watchlist_to_remove).await?;
                info!("Removed {} items from Trakt watchlist", trakt_watchlist_to_remove.len());
            }
            if !imdb_watchlist_to_remove.is_empty() && !imdb_watchlist_limit_reached {
                for item in &imdb_watchlist_to_remove {
                    debug!(
                        imdb_id = %item.imdb_id,
                        title = %item.title,
                        media_type = ?item.media_type,
                        "Removing from IMDB watchlist"
                    );
                }
                imdb.remove_from_watchlist(&imdb_watchlist_to_remove).await?;
                info!("Removed {} items from IMDB watchlist", imdb_watchlist_to_remove.len());
            }
        }

        // Sync ratings
        if sync_options.sync_ratings {
            // Normalize IMDB ratings to Trakt format (1-10 scale)
            let trakt_scale = 10u8;
            let imdb_ratings_normalized: Vec<Rating> = if let Some(normalizer) = imdb.as_rating_normalization() {
                imdb_ratings_to_set
                    .iter()
                    .map(|r| Rating {
                        rating: normalizer.normalize_rating(r.rating as f64, trakt_scale),
                        ..r.clone()
                    })
                    .collect()
            } else {
                // Fallback: assume same scale
                imdb_ratings_to_set.clone()
            };

            if !imdb_ratings_normalized.is_empty() {
                // Add logging before adding to Trakt
                debug!(
                    "About to add {} ratings to Trakt",
                    imdb_ratings_normalized.len()
                );
                
                for rating in &imdb_ratings_normalized {
                    debug!(
                        imdb_id = %rating.imdb_id,
                        rating = rating.rating,
                        media_type = ?rating.media_type,
                        date_added = %rating.date_added,
                        "Adding rating to Trakt"
                    );
                }
                
                trakt.set_ratings(&imdb_ratings_normalized).await?;
                items_synced += imdb_ratings_normalized.len();
                info!("Added {} ratings to Trakt", imdb_ratings_normalized.len());
                
                // Log the IMDB IDs that were just added
                let added_ids: Vec<String> = imdb_ratings_normalized.iter()
                    .map(|r| r.imdb_id.clone())
                    .collect();
                debug!(
                    "Just added these IMDB IDs as ratings to Trakt: {:?}",
                    added_ids
                );
            }
            if !trakt_ratings_to_set.is_empty() {
                for rating in &trakt_ratings_to_set {
                    debug!(
                        imdb_id = %rating.imdb_id,
                        rating = rating.rating,
                        media_type = ?rating.media_type,
                        date_added = %rating.date_added,
                        "Adding rating to IMDB"
                    );
                }
                imdb.set_ratings(&trakt_ratings_to_set).await?;
                items_synced += trakt_ratings_to_set.len();
                info!("Added {} ratings to IMDB", trakt_ratings_to_set.len());
            }
        }

        // Sync reviews
        if sync_options.sync_reviews {
            if !imdb_reviews_to_set.is_empty() {
                info!("Syncing {} IMDB reviews to Trakt", imdb_reviews_to_set.len());
                for review in &imdb_reviews_to_set {
                    debug!(
                        imdb_id = %review.imdb_id,
                        media_type = ?review.media_type,
                        content_length = review.content.len(),
                        is_spoiler = review.is_spoiler,
                        date_added = %review.date_added,
                        "Adding review to Trakt"
                    );
                }
                trakt.set_reviews(&imdb_reviews_to_set).await?;
                items_synced += imdb_reviews_to_set.len();
                info!("Successfully added {} reviews to Trakt", imdb_reviews_to_set.len());
                warn!("Note: Newly added Trakt reviews may take a few minutes to appear in the API. If you sync again immediately, they may appear as duplicates until the API indexes them.");
            } else {
                info!("No IMDB reviews to sync to Trakt (all reviews already exist or were filtered out)");
            }
            
            if !trakt_reviews_to_set.is_empty() {
                info!("Syncing {} Trakt reviews to IMDB", trakt_reviews_to_set.len());
                for review in &trakt_reviews_to_set {
                    debug!(
                        imdb_id = %review.imdb_id,
                        media_type = ?review.media_type,
                        content_length = review.content.len(),
                        is_spoiler = review.is_spoiler,
                        date_added = %review.date_added,
                        "Adding review to IMDB"
                    );
                }
                imdb.set_reviews(&trakt_reviews_to_set).await?;
                items_synced += trakt_reviews_to_set.len();
                info!("Successfully added {} reviews to IMDB", trakt_reviews_to_set.len());
            } else {
                info!("No Trakt reviews to sync to IMDB (all reviews already exist or were filtered out)");
            }
        } else {
            info!("Reviews sync disabled, skipping review synchronization");
        }

        // Sync watch history - only when explicitly requested
        if sync_options.sync_watch_history {
            if !imdb_history_to_set.is_empty() {
                // Add logging before adding to Trakt
                debug!(
                    "About to add {} items to Trakt watch history",
                    imdb_history_to_set.len()
                );
                
                for item in &imdb_history_to_set {
                    debug!(
                        imdb_id = %item.imdb_id,
                        media_type = ?item.media_type,
                        watched_at = %item.watched_at,
                        "Adding to Trakt watch history"
                    );
                }
                
                trakt.add_watch_history(&imdb_history_to_set).await?;
                items_synced += imdb_history_to_set.len();
                info!("Added {} items to Trakt watch history", imdb_history_to_set.len());
                
                // Log the IMDB IDs that were just added
                let added_ids: Vec<String> = imdb_history_to_set.iter()
                    .map(|item| item.imdb_id.clone())
                    .collect();
                debug!(
                    "Just added these IMDB IDs to Trakt: {:?}",
                    added_ids
                );
            }
            if !trakt_history_to_set.is_empty() && !imdb_watch_history_limit_reached {
                for item in &trakt_history_to_set {
                    debug!(
                        imdb_id = %item.imdb_id,
                        media_type = ?item.media_type,
                        watched_at = %item.watched_at,
                        "Adding to IMDB watch history"
                    );
                }
                imdb.add_watch_history(&trakt_history_to_set).await?;
                items_synced += trakt_history_to_set.len();
                info!("Added {} items to IMDB watch history", trakt_history_to_set.len());
            }
        }

        info!("IMDB sync completed: {} items synced", items_synced);

        info!("IMDB sync completed: {} items synced", items_synced);
        Ok(items_synced)
    }
}

