// Distribution strategies for preparing data before sending to target sources
// Handles filtering, transformation, normalization, and incremental sync

use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use media_sync_config::{CredentialStore, PathManager};
use media_sync_models::{Rating, RatingSource, Review, WatchHistory, WatchlistItem, NormalizedStatus, MediaType, ExcludedItem};
use std::sync::Mutex;
use std::collections::HashMap;
use tracing::{info, warn};
use crate::diff::{filter_items_by_imdb_id, filter_ratings_by_imdb_id_and_value, filter_reviews_by_imdb_id_and_content};
use crate::resolution::SourceData;
use crate::cache::CacheManager;

/// Result type for watchlist preparation (can split into watchlist + watch_history)
#[derive(Debug, Clone)]
pub struct DistributionResult<T, U> {
    pub for_watchlist: Vec<T>,
    pub for_watch_history: Vec<U>,
}

impl<T, U> Default for DistributionResult<T, U> {
    fn default() -> Self {
        Self {
            for_watchlist: Vec::new(),
            for_watch_history: Vec::new(),
        }
    }
}

/// Strategy for preparing data before distribution to a target source
/// Handles filtering, transformation, and normalization
pub trait DistributionStrategy: Send + Sync {
    /// Get the target source name for this strategy
    /// The strategy knows which source it's preparing data for
    fn target_source_name(&self) -> &str;
    
    /// Prepare watchlist items for distribution
    /// Returns: (items_for_watchlist, items_for_watch_history)
    /// Sources can split by status, transform, filter, etc.
    /// 
    /// `resolved_watch_history` contains all watch history from all sources (resolved).
    /// If `remove_watched_from_watchlists` is true, items in `resolved_watch_history` should be filtered out.
    fn prepare_watchlist(
        &self,
        items: &[WatchlistItem],
        existing: &SourceData,
        force_full_sync: bool,
        resolved_watch_history: &[WatchHistory],
        remove_watched_from_watchlists: bool,
    ) -> Result<DistributionResult<WatchlistItem, WatchHistory>>;
    
    /// Prepare ratings for distribution
    /// Can filter, normalize, transform
    fn prepare_ratings(
        &self,
        items: &[Rating],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Rating>>;
    
    /// Prepare reviews for distribution
    fn prepare_reviews(
        &self,
        items: &[Review],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Review>>;
    
    /// Prepare watch history for distribution
    fn prepare_watch_history(
        &self,
        items: &[WatchHistory],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<WatchHistory>>;
    
    /// Called after successful sync to update any state (e.g., sync timestamps)
    fn on_sync_complete(
        &self,
        data_type: &str,
        items_synced: usize,
    ) -> Result<()>;
}

/// Default strategy: incremental sync + deduplication, no transformation
pub struct DefaultDistributionStrategy {
    cred_store: Mutex<CredentialStore>,
    target_source: String,
    cache_manager: Option<CacheManager>,
}

impl DefaultDistributionStrategy {
    pub fn new(target_source: &str) -> Result<Self> {
        let path_manager = PathManager::default();
        let mut cred_store = CredentialStore::new(path_manager.credentials_file());
        cred_store.load()?;
        Ok(Self {
            cred_store: Mutex::new(cred_store),
            target_source: target_source.to_string(),
            cache_manager: None,
        })
    }
    
    pub fn with_cache_manager(mut self, cache_manager: CacheManager) -> Self {
        self.cache_manager = Some(cache_manager);
        self
    }
    
    /// Apply incremental sync timestamp filtering
    /// Returns (included_items, excluded_items)
    fn apply_incremental_sync_filter<T>(
        &self,
        items: Vec<T>,
        target_source: &str,
        data_type: &str,
        force_full_sync: bool,
        get_timestamp: impl Fn(&T) -> Option<DateTime<Utc>>,
    ) -> Result<(Vec<T>, Vec<T>)> {
        if force_full_sync {
            return Ok((items, Vec::new()));
        }
        
        let last_sync = self.cred_store.lock().unwrap().get_last_sync_timestamp(target_source, data_type);
        Ok(Self::filter_by_timestamp(items, last_sync, get_timestamp))
    }
    
    /// Save excluded items to cache, grouped by source
    /// This accumulates excluded items from all filtering stages and saves them per source
    fn save_excluded_items<T>(
        &self,
        excluded_items: &[T],
        data_type: &str,
        reason: &str,
        get_excluded_item: impl Fn(&T) -> ExcludedItem,
    ) where
        T: Clone,
    {
        if let Some(ref cache_manager) = self.cache_manager {
            if !excluded_items.is_empty() {
                // Group excluded items by source
                let mut excluded_by_source: HashMap<String, Vec<ExcludedItem>> = HashMap::new();
                
                for item in excluded_items {
                    let mut excluded = get_excluded_item(item);
                    // Update reason to include the filtering stage
                    excluded.reason = format!("{}: {}", reason, excluded.reason);
                    excluded_by_source
                        .entry(excluded.source.clone())
                        .or_insert_with(Vec::new)
                        .push(excluded);
                }
                
                // Save excluded items for each source
                for (source, excluded) in excluded_by_source {
                    if let Err(e) = cache_manager.save_excluded(&source, &excluded) {
                        warn!("Failed to save excluded items for {} {} to cache: {}", source, data_type, e);
                    } else {
                        info!("Saved {} excluded items for {} {} to cache ({})", excluded.len(), source, data_type, reason);
                    }
                }
            }
        }
    }
    
    /// Filter items by timestamp for incremental sync
    /// Returns (included_items, excluded_items)
    fn filter_by_timestamp<T>(
        items: Vec<T>,
        last_sync: Option<DateTime<Utc>>,
        get_timestamp: impl Fn(&T) -> Option<DateTime<Utc>>,
    ) -> (Vec<T>, Vec<T>) {
        if let Some(last_sync) = last_sync {
            let mut included = Vec::new();
            let mut excluded = Vec::new();
            
            for item in items {
                let should_include = get_timestamp(&item)
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
                    .unwrap_or(true); // Include items without timestamps
                
                if should_include {
                    included.push(item);
                } else {
                    excluded.push(item);
                }
            }
            
            (included, excluded)
        } else {
            // First sync, return all as included
            (items, Vec::new())
        }
    }
    
    /// Update sync timestamp after successful sync
    fn update_sync_timestamp(
        &self,
        target_source: &str,
        data_type: &str,
    ) -> Result<()> {
        let mut cred_store = self.cred_store.lock().unwrap();
        cred_store.set_last_sync_timestamp(target_source, data_type, Utc::now());
        cred_store.save()?;
        Ok(())
    }
}

impl DistributionStrategy for DefaultDistributionStrategy {
    fn target_source_name(&self) -> &str {
        &self.target_source
    }
    
    fn prepare_watchlist(
        &self,
        items: &[WatchlistItem],
        existing: &SourceData,
        force_full_sync: bool,
        resolved_watch_history: &[WatchHistory],
        remove_watched_from_watchlists: bool,
    ) -> Result<DistributionResult<WatchlistItem, WatchHistory>> {
        // 1. Apply incremental sync filtering
        let (mut filtered, excluded_timestamp) = self.apply_incremental_sync_filter(
            items.to_vec(),
            self.target_source_name(),
            "watchlist",
            force_full_sync,
            |item| Some(item.date_added),
        )?;
        
        // Save timestamp-excluded items to cache
        self.save_excluded_items(&excluded_timestamp, "watchlist", "timestamp filter", |item| {
            ExcludedItem {
                title: Some(item.title.clone()),
                imdb_id: if item.imdb_id.is_empty() { None } else { Some(item.imdb_id.clone()) },
                rating_key: None,
                media_type: format!("{:?}", item.media_type),
                reason: format!("Excluded by timestamp filter (date_added: {})", item.date_added),
                source: item.source.clone(),
                date_added: Some(item.date_added), // Preserve date_added for age-based removal features
            }
        });
        
        // 2. Filter out items that came from the target source (they already exist there)
        let target_source = self.target_source_name();
        let mut excluded_source: Vec<WatchlistItem> = Vec::new();
        filtered.retain(|item| {
            if item.source == target_source {
                excluded_source.push(item.clone());
                false
            } else {
                true
            }
        });
        
        // Save source-excluded items to cache
        self.save_excluded_items(&excluded_source, "watchlist", "source filter", |item| {
            ExcludedItem {
                title: Some(item.title.clone()),
                imdb_id: if item.imdb_id.is_empty() { None } else { Some(item.imdb_id.clone()) },
                rating_key: None,
                media_type: format!("{:?}", item.media_type),
                reason: format!("Excluded: item already exists in target source '{}'", target_source),
                source: item.source.clone(),
                date_added: None, // Not excluded by timestamp, so no date needed
            }
        });
        
        // 3. Apply IMDB ID deduplication
        let before_dedup = filtered.len();
        filtered = filter_items_by_imdb_id(&filtered, &existing.watchlist);
        let excluded_dedup_count = before_dedup - filtered.len();
        
        // For deduplication, we can't easily reconstruct excluded items, but we log the count
        if excluded_dedup_count > 0 {
            info!("Deduplication filtered out {} watchlist items (already exist in target)", excluded_dedup_count);
        }
        
        // 4. Filter out watched items if remove_watched_from_watchlists is enabled
        if remove_watched_from_watchlists {
            use crate::diff::GetImdbId;
            let watched_ids: std::collections::HashSet<String> = resolved_watch_history.iter()
                .map(|h| h.get_imdb_id())
                .filter(|id| !id.is_empty())
                .collect();
            
            let before_watched_filter = filtered.len();
            let mut excluded_watched: Vec<WatchlistItem> = Vec::new();
            filtered.retain(|item| {
                if watched_ids.contains(&item.imdb_id) {
                    excluded_watched.push(item.clone());
                    false
                } else {
                    true
                }
            });
            
            // Save watched-excluded items to cache
            self.save_excluded_items(&excluded_watched, "watchlist", "watched filter", |item| {
                ExcludedItem {
                    title: Some(item.title.clone()),
                    imdb_id: if item.imdb_id.is_empty() { None } else { Some(item.imdb_id.clone()) },
                    rating_key: None,
                    media_type: format!("{:?}", item.media_type),
                    reason: format!("Excluded: item is already in watch history (remove_watched_from_watchlists)"),
                    source: item.source.clone(),
                    date_added: None,
                }
            });
            
            if before_watched_filter > filtered.len() {
                info!("Filtered out {} watchlist items that are already watched (remove_watched_from_watchlists)", 
                    before_watched_filter - filtered.len());
            }
        }
        
        // 5. Return as-is (no status splitting)
        Ok(DistributionResult {
            for_watchlist: filtered,
            for_watch_history: Vec::new(),
        })
    }
    
    fn prepare_ratings(
        &self,
        items: &[Rating],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Rating>> {
        // 1. Apply incremental sync filtering
        let (mut filtered, excluded_timestamp) = self.apply_incremental_sync_filter(
            items.to_vec(),
            self.target_source_name(),
            "ratings",
            force_full_sync,
            |item| Some(item.date_added),
        )?;
        
        // Save timestamp-excluded items to cache
        self.save_excluded_items(&excluded_timestamp, "ratings", "timestamp filter", |item| {
            let source_str = match &item.source {
                RatingSource::Plex => "plex",
                RatingSource::Trakt => "trakt",
                RatingSource::Imdb => "imdb",
                RatingSource::Netflix => "netflix",
                RatingSource::Tmdb => "tmdb",
            };
            ExcludedItem {
                title: None, // Ratings don't have titles
                imdb_id: if item.imdb_id.is_empty() { None } else { Some(item.imdb_id.clone()) },
                rating_key: None,
                media_type: format!("{:?}", item.media_type),
                reason: format!("Excluded by timestamp filter (date_added: {}, rating: {})", item.date_added, item.rating),
                source: source_str.to_string(),
                date_added: None, // Ratings are not watchlist items
            }
        });
        
        // 2. Filter out items that came from the target source (they already exist there)
        let target_source = self.target_source_name();
        let mut excluded_source: Vec<Rating> = Vec::new();
        filtered.retain(|item| {
            // Convert RatingSource enum to lowercase string for comparison
            let item_source = match &item.source {
                RatingSource::Plex => "plex",
                RatingSource::Trakt => "trakt",
                RatingSource::Imdb => "imdb",
                RatingSource::Netflix => "netflix",
                RatingSource::Tmdb => "tmdb",
            };
            if item_source == target_source {
                excluded_source.push(item.clone());
                false
            } else {
                true
            }
        });
        
        // Save source-excluded items to cache
        self.save_excluded_items(&excluded_source, "ratings", "source filter", |item| {
            let source_str = match &item.source {
                RatingSource::Plex => "plex",
                RatingSource::Trakt => "trakt",
                RatingSource::Imdb => "imdb",
                RatingSource::Netflix => "netflix",
                RatingSource::Tmdb => "tmdb",
            };
            ExcludedItem {
                title: None, // Ratings don't have titles
                imdb_id: if item.imdb_id.is_empty() { None } else { Some(item.imdb_id.clone()) },
                rating_key: None,
                media_type: format!("{:?}", item.media_type),
                reason: format!("Excluded: item already exists in target source '{}'", target_source),
                source: source_str.to_string(),
                date_added: None, // Ratings are not watchlist items
            }
        });
        
        // 3. Apply IMDB ID + value deduplication
        let before_dedup = filtered.len();
        let result = filter_ratings_by_imdb_id_and_value(&filtered, &existing.ratings);
        let excluded_dedup_count = before_dedup - result.len();
        
        if excluded_dedup_count > 0 {
            info!("Deduplication filtered out {} ratings (already exist in target)", excluded_dedup_count);
        }
        
        Ok(result)
    }
    
    fn prepare_reviews(
        &self,
        items: &[Review],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Review>> {
        // 1. Apply incremental sync filtering
        let (mut filtered, excluded_timestamp) = self.apply_incremental_sync_filter(
            items.to_vec(),
            self.target_source_name(),
            "reviews",
            force_full_sync,
            |item| Some(item.date_added),
        )?;
        
        // Save timestamp-excluded items to cache
        self.save_excluded_items(&excluded_timestamp, "reviews", "timestamp filter", |item| {
            ExcludedItem {
                title: None, // Reviews don't have titles
                imdb_id: if item.imdb_id.is_empty() { None } else { Some(item.imdb_id.clone()) },
                rating_key: None,
                media_type: format!("{:?}", item.media_type),
                reason: format!("Excluded by timestamp filter (date_added: {})", item.date_added),
                source: item.source.clone(),
                date_added: None, // Reviews are not watchlist items
            }
        });
        
        // 2. Filter out items that came from the target source (they already exist there)
        let target_source = self.target_source_name();
        let mut excluded_source: Vec<Review> = Vec::new();
        filtered.retain(|item| {
            if item.source == target_source {
                excluded_source.push(item.clone());
                false
            } else {
                true
            }
        });
        
        // Save source-excluded items to cache
        self.save_excluded_items(&excluded_source, "reviews", "source filter", |item| {
            ExcludedItem {
                title: None, // Reviews don't have titles
                imdb_id: if item.imdb_id.is_empty() { None } else { Some(item.imdb_id.clone()) },
                rating_key: None,
                media_type: format!("{:?}", item.media_type),
                reason: format!("Excluded: item already exists in target source '{}'", target_source),
                source: item.source.clone(),
                date_added: None, // Reviews are not watchlist items
            }
        });
        
        // 3. Apply IMDB ID + content deduplication
        let before_dedup = filtered.len();
        let result = filter_reviews_by_imdb_id_and_content(&filtered, &existing.reviews);
        let excluded_dedup_count = before_dedup - result.len();
        
        if excluded_dedup_count > 0 {
            info!("Deduplication filtered out {} reviews (already exist in target)", excluded_dedup_count);
        }
        
        Ok(result)
    }
    
    fn prepare_watch_history(
        &self,
        items: &[WatchHistory],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<WatchHistory>> {
        // 1. Apply incremental sync filtering
        let (filtered, excluded_timestamp) = self.apply_incremental_sync_filter(
            items.to_vec(),
            self.target_source_name(),
            "watch_history",
            force_full_sync,
            |item| Some(item.watched_at),
        )?;
        
        // Save timestamp-excluded items to cache
        self.save_excluded_items(&excluded_timestamp, "watch_history", "timestamp filter", |item| {
            ExcludedItem {
                title: item.title.clone(),
                imdb_id: if item.imdb_id.is_empty() { None } else { Some(item.imdb_id.clone()) },
                rating_key: None,
                media_type: format!("{:?}", item.media_type),
                reason: format!("Excluded by timestamp filter (watched_at: {})", item.watched_at),
                source: item.source.clone(),
                date_added: None, // Watch history is not watchlist items
            }
        });
        
        // 2. Filter out items that came from the target source (they already exist there)
        let target_source = self.target_source_name();
        let mut excluded_source: Vec<WatchHistory> = Vec::new();
        let filtered_by_source: Vec<_> = filtered.iter()
            .filter_map(|item| {
                if item.source == target_source {
                    // Item came from target source - it already exists, filter it out
                    excluded_source.push(item.clone());
                    None
                } else {
                    Some(item.clone())
                }
            })
            .collect();
        
        // Save source-excluded items to cache
        self.save_excluded_items(&excluded_source, "watch_history", "source filter", |item| {
            ExcludedItem {
                title: item.title.clone(),
                imdb_id: if item.imdb_id.is_empty() { None } else { Some(item.imdb_id.clone()) },
                rating_key: None,
                media_type: format!("{:?}", item.media_type),
                reason: format!("Excluded: item already exists in target source '{}'", target_source),
                source: item.source.clone(),
                date_added: None, // Watch history is not watchlist items
            }
        });
        
        // 3. Apply IMDB ID deduplication against cache
        let before_dedup = filtered_by_source.len();
        let result = filter_items_by_imdb_id(&filtered_by_source, &existing.watch_history);
        let excluded_dedup_count = before_dedup - result.len();
        
        if excluded_dedup_count > 0 {
            info!("Deduplication filtered out {} watch history items (already exist in target)", excluded_dedup_count);
        }
        
        Ok(result)
    }
    
    fn on_sync_complete(
        &self,
        data_type: &str,
        _items_synced: usize,
    ) -> Result<()> {
        // Update sync timestamp after successful sync
        self.update_sync_timestamp(self.target_source_name(), data_type)
    }
}


/// Trakt-specific: splits watchlist by status (Watchlist vs Watching/Completed → watch_history)
pub struct TraktDistributionStrategy {
    base: DefaultDistributionStrategy,
}

impl TraktDistributionStrategy {
    pub fn new() -> Result<Self> {
        Ok(Self {
            base: DefaultDistributionStrategy::new("trakt")?,
        })
    }
    
    pub fn with_cache_manager(mut self, cache_manager: CacheManager) -> Self {
        self.base = self.base.with_cache_manager(cache_manager);
        self
    }
    
    fn split_by_status(items: &[WatchlistItem]) -> (Vec<WatchlistItem>, Vec<WatchHistory>) {
        let mut watchlist_items = Vec::new();
        let mut watch_history_items = Vec::new();
        
        for item in items {
            match item.status.as_ref() {
                Some(NormalizedStatus::Watching) | Some(NormalizedStatus::Completed) => {
                    // Skip Shows - Trakt doesn't support shows in watch history
                    if !matches!(item.media_type, MediaType::Show) {
                        watch_history_items.push(WatchHistory {
                            imdb_id: item.imdb_id.clone(),
                            ids: item.ids.clone(),
                            title: Some(item.title.clone()),
                            year: item.year,
                            watched_at: item.date_added,
                            media_type: item.media_type.clone(),
                            source: item.source.clone(), // Preserve original source, don't hardcode target source
                        });
                    }
                }
                _ => {
                    // Watchlist status or no status -> goes to watchlist
                    watchlist_items.push(item.clone());
                }
            }
        }
        
        (watchlist_items, watch_history_items)
    }
}

impl DistributionStrategy for TraktDistributionStrategy {
    fn target_source_name(&self) -> &str {
        self.base.target_source_name()
    }
    
    fn prepare_watchlist(
        &self,
        items: &[WatchlistItem],
        existing: &SourceData,
        force_full_sync: bool,
        resolved_watch_history: &[WatchHistory],
        remove_watched_from_watchlists: bool,
    ) -> Result<DistributionResult<WatchlistItem, WatchHistory>> {
        // 1. Apply base filtering (incremental sync + deduplication + watched filter)
        let base_result = self.base.prepare_watchlist(items, existing, force_full_sync, resolved_watch_history, remove_watched_from_watchlists)?;
        
        // 2. Split by status
        let (watchlist_items, watch_history_items) = Self::split_by_status(&base_result.for_watchlist);
        
        // 3. Deduplicate watch_history_items against existing watch_history
        let filtered_history: Vec<_> = watch_history_items.iter()
            .filter(|item| !existing.watch_history.iter().any(|e| e.imdb_id == item.imdb_id))
            .cloned()
            .collect();
        
        Ok(DistributionResult {
            for_watchlist: watchlist_items,
            for_watch_history: filtered_history,
        })
    }
    
    fn prepare_ratings(
        &self,
        items: &[Rating],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Rating>> {
        self.base.prepare_ratings(items, existing, force_full_sync)
    }
    
    fn prepare_reviews(
        &self,
        items: &[Review],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Review>> {
        self.base.prepare_reviews(items, existing, force_full_sync)
    }
    
    fn prepare_watch_history(
        &self,
        items: &[WatchHistory],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<WatchHistory>> {
        // First apply base filtering (incremental sync + deduplication)
        let mut filtered = self.base.prepare_watch_history(items, existing, force_full_sync)?;
        
        // Filter out Shows - Trakt doesn't support shows in watch history
        filtered.retain(|item| !matches!(item.media_type, MediaType::Show));
        
        Ok(filtered)
    }
    
    fn on_sync_complete(
        &self,
        data_type: &str,
        items_synced: usize,
    ) -> Result<()> {
        self.base.on_sync_complete(data_type, items_synced)
    }
}

/// IMDB-specific: converts watchlist items with Watching/Completed status to check-ins
pub struct ImdbDistributionStrategy {
    base: DefaultDistributionStrategy,
}

impl ImdbDistributionStrategy {
    pub fn new() -> Result<Self> {
        Ok(Self {
            base: DefaultDistributionStrategy::new("imdb")?,
        })
    }
    
    pub fn with_cache_manager(mut self, cache_manager: CacheManager) -> Self {
        self.base = self.base.with_cache_manager(cache_manager);
        self
    }
    
    fn transform_to_checkins(items: &[WatchlistItem]) -> Vec<WatchHistory> {
        items.iter()
            .filter_map(|item| {
                match item.status.as_ref() {
                    Some(NormalizedStatus::Watching) | Some(NormalizedStatus::Completed) => {
                        Some(WatchHistory {
                            imdb_id: item.imdb_id.clone(),
                            ids: item.ids.clone(),
                            title: Some(item.title.clone()),
                            year: item.year,
                            watched_at: item.date_added,
                            media_type: item.media_type.clone(),
                            source: item.source.clone(), // Preserve original source, don't hardcode target source
                        })
                    }
                    _ => None,
                }
            })
            .collect()
    }
}

impl DistributionStrategy for ImdbDistributionStrategy {
    fn target_source_name(&self) -> &str {
        self.base.target_source_name()
    }
    
    fn prepare_watchlist(
        &self,
        items: &[WatchlistItem],
        existing: &SourceData,
        force_full_sync: bool,
        resolved_watch_history: &[WatchHistory],
        remove_watched_from_watchlists: bool,
    ) -> Result<DistributionResult<WatchlistItem, WatchHistory>> {
        // 1. Apply base filtering (incremental sync + deduplication + watched filter)
        let base_result = self.base.prepare_watchlist(items, existing, force_full_sync, resolved_watch_history, remove_watched_from_watchlists)?;
        
        // 2. Transform Watching/Completed items to check-ins
        let checkins = Self::transform_to_checkins(&base_result.for_watchlist);
        
        // 3. Filter check-ins against existing watch_history
        let filtered_checkins = filter_items_by_imdb_id(&checkins, &existing.watch_history);
        
        // 4. Filter watchlist items (remove those that became check-ins)
        // Note: watched items are already filtered in base.prepare_watchlist if remove_watched_from_watchlists is enabled
        let watchlist_items: Vec<_> = base_result.for_watchlist.iter()
            .filter(|item| {
                item.status.as_ref()
                    .map(|s| matches!(s, NormalizedStatus::Watchlist))
                    .unwrap_or(true) // No status -> goes to watchlist
            })
            .cloned()
            .collect();
        
        Ok(DistributionResult {
            for_watchlist: watchlist_items,
            for_watch_history: filtered_checkins,
        })
    }
    
    fn prepare_ratings(
        &self,
        items: &[Rating],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Rating>> {
        self.base.prepare_ratings(items, existing, force_full_sync)
    }
    
    fn prepare_reviews(
        &self,
        items: &[Review],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Review>> {
        self.base.prepare_reviews(items, existing, force_full_sync)
    }
    
    fn prepare_watch_history(
        &self,
        items: &[WatchHistory],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<WatchHistory>> {
        self.base.prepare_watch_history(items, existing, force_full_sync)
    }
    
    fn on_sync_complete(
        &self,
        data_type: &str,
        items_synced: usize,
    ) -> Result<()> {
        self.base.on_sync_complete(data_type, items_synced)
    }
}

/// Simkl-specific: no incremental sync (has native), but still needs deduplication
pub struct SimklDistributionStrategy {
    target_source: String,
}

impl SimklDistributionStrategy {
    pub fn new() -> Result<Self> {
        Ok(Self {
            target_source: "simkl".to_string(),
        })
    }
}

impl DistributionStrategy for SimklDistributionStrategy {
    fn target_source_name(&self) -> &str {
        &self.target_source
    }
    
    fn prepare_watchlist(
        &self,
        items: &[WatchlistItem],
        existing: &SourceData,
        _force_full_sync: bool,
        resolved_watch_history: &[WatchHistory],
        remove_watched_from_watchlists: bool,
    ) -> Result<DistributionResult<WatchlistItem, WatchHistory>> {
        // 1. Filter out items that came from the target source (they already exist there)
        let target_source = self.target_source_name();
        let mut filtered_by_source: Vec<_> = items.iter()
            .filter(|item| item.source != target_source)
            .cloned()
            .collect();
        
        // 2. Only deduplication, no incremental sync (Simkl handles it natively)
        let mut deduped = filter_items_by_imdb_id(&filtered_by_source, &existing.watchlist);
        
        // 3. Filter out watched items if remove_watched_from_watchlists is enabled
        if remove_watched_from_watchlists {
            use crate::diff::GetImdbId;
            let watched_ids: std::collections::HashSet<String> = resolved_watch_history.iter()
                .map(|h| h.get_imdb_id())
                .filter(|id| !id.is_empty())
                .collect();
            
            let before_watched_filter = deduped.len();
            deduped.retain(|item| !watched_ids.contains(&item.imdb_id));
            
            if before_watched_filter > deduped.len() {
                info!("Filtered out {} Simkl watchlist items that are already watched (remove_watched_from_watchlists)", 
                    before_watched_filter - deduped.len());
            }
        }
        
        Ok(DistributionResult {
            for_watchlist: deduped,
            for_watch_history: Vec::new(),
        })
    }
    
    fn prepare_ratings(
        &self,
        items: &[Rating],
        existing: &SourceData,
        _force_full_sync: bool,
    ) -> Result<Vec<Rating>> {
        // 1. Filter out items that came from the target source (they already exist there)
        let target_source = self.target_source_name();
        let filtered_by_source: Vec<_> = items.iter()
            .filter(|item| {
                // Convert RatingSource enum to lowercase string for comparison
                let item_source = match &item.source {
                    RatingSource::Plex => "plex",
                    RatingSource::Trakt => "trakt",
                    RatingSource::Imdb => "imdb",
                    RatingSource::Netflix => "netflix",
                    RatingSource::Tmdb => "tmdb",
                };
                item_source != target_source
            })
            .cloned()
            .collect();
        
        // 2. Only deduplication
        Ok(filter_ratings_by_imdb_id_and_value(&filtered_by_source, &existing.ratings))
    }
    
    fn prepare_reviews(
        &self,
        items: &[Review],
        existing: &SourceData,
        _force_full_sync: bool,
    ) -> Result<Vec<Review>> {
        // 1. Filter out items that came from the target source (they already exist there)
        let target_source = self.target_source_name();
        let filtered_by_source: Vec<_> = items.iter()
            .filter(|item| item.source != target_source)
            .cloned()
            .collect();
        
        // 2. Only deduplication
        Ok(filter_reviews_by_imdb_id_and_content(&filtered_by_source, &existing.reviews))
    }
    
    fn prepare_watch_history(
        &self,
        items: &[WatchHistory],
        existing: &SourceData,
        _force_full_sync: bool,
    ) -> Result<Vec<WatchHistory>> {
        // 1. Filter out items that came from the target source (they already exist there)
        let target_source = self.target_source_name();
        let filtered_by_source: Vec<_> = items.iter()
            .filter(|item| item.source != target_source)
            .cloned()
            .collect();
        
        // 2. Only deduplication
        Ok(filter_items_by_imdb_id(&filtered_by_source, &existing.watch_history))
    }
    
    fn on_sync_complete(
        &self,
        _data_type: &str,
        _items_synced: usize,
    ) -> Result<()> {
        // No-op: Simkl doesn't need timestamp tracking
        Ok(())
    }
}

/// Plex-specific: splits watchlist by status (only Watchlist → watchlist, Completed/Watching → watch_history)
pub struct PlexDistributionStrategy {
    base: DefaultDistributionStrategy,
}

impl PlexDistributionStrategy {
    pub fn new() -> Result<Self> {
        Ok(Self {
            base: DefaultDistributionStrategy::new("plex")?,
        })
    }
    
    pub fn with_cache_manager(mut self, cache_manager: CacheManager) -> Self {
        self.base = self.base.with_cache_manager(cache_manager);
        self
    }
    
    fn split_by_status(items: &[WatchlistItem]) -> (Vec<WatchlistItem>, Vec<WatchHistory>) {
        let mut watchlist_items = Vec::new();
        let mut watch_history_items = Vec::new();
        
        for item in items {
            match item.status.as_ref() {
                Some(NormalizedStatus::Watchlist) => {
                    // Only Watchlist status goes to watchlist
                    watchlist_items.push(item.clone());
                }
                Some(NormalizedStatus::Completed) | Some(NormalizedStatus::Watching) => {
                    // Completed and Watching go to watch_history
                    watch_history_items.push(WatchHistory {
                        imdb_id: item.imdb_id.clone(),
                        ids: item.ids.clone(),
                        title: Some(item.title.clone()),
                        year: item.year,
                        watched_at: item.date_added,
                        media_type: item.media_type.clone(),
                        source: item.source.clone(), // Preserve original source, don't hardcode target source
                    });
                }
                _ => {
                    // No status or other statuses (Dropped, Hold) -> skip or go to watchlist?
                    // Default: skip (don't add to either)
                    // Could also add to watchlist if desired
                }
            }
        }
        
        (watchlist_items, watch_history_items)
    }
}

impl DistributionStrategy for PlexDistributionStrategy {
    fn target_source_name(&self) -> &str {
        self.base.target_source_name()
    }
    
    fn prepare_watchlist(
        &self,
        items: &[WatchlistItem],
        existing: &SourceData,
        force_full_sync: bool,
        resolved_watch_history: &[WatchHistory],
        remove_watched_from_watchlists: bool,
    ) -> Result<DistributionResult<WatchlistItem, WatchHistory>> {
        // 1. Apply base filtering (incremental sync + deduplication + watched filter)
        let base_result = self.base.prepare_watchlist(items, existing, force_full_sync, resolved_watch_history, remove_watched_from_watchlists)?;
        
        // 2. Split by status (only Watchlist → watchlist, Completed/Watching → watch_history)
        let (watchlist_items, watch_history_items) = Self::split_by_status(&base_result.for_watchlist);
        
        // 3. Deduplicate watch_history_items against existing watch_history
        let filtered_history: Vec<_> = watch_history_items.iter()
            .filter(|item| !existing.watch_history.iter().any(|e| e.imdb_id == item.imdb_id))
            .cloned()
            .collect();
        
        Ok(DistributionResult {
            for_watchlist: watchlist_items,
            for_watch_history: filtered_history,
        })
    }
    
    fn prepare_ratings(
        &self,
        items: &[Rating],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Rating>> {
        self.base.prepare_ratings(items, existing, force_full_sync)
    }
    
    fn prepare_reviews(
        &self,
        items: &[Review],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<Review>> {
        self.base.prepare_reviews(items, existing, force_full_sync)
    }
    
    fn prepare_watch_history(
        &self,
        items: &[WatchHistory],
        existing: &SourceData,
        force_full_sync: bool,
    ) -> Result<Vec<WatchHistory>> {
        self.base.prepare_watch_history(items, existing, force_full_sync)
    }
    
    fn on_sync_complete(
        &self,
        data_type: &str,
        items_synced: usize,
    ) -> Result<()> {
        self.base.on_sync_complete(data_type, items_synced)
    }
}

