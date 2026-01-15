// Diff computation logic for syncing between sources

/// Filter items from source that are not present in target based on any ID (MediaIds or imdb_id)
/// This uses MediaIds for flexible matching when available, falls back to imdb_id
pub fn filter_items_by_any_id<T>(source: &[T], target: &[T]) -> Vec<T>
where
    T: Clone + GetImdbId + GetMediaIds,
{
    use tracing::debug;
    use crate::id_matching::match_by_any_id;
    
    // Build set of target items with their IDs for matching
    let target_items: Vec<&T> = target.iter().collect();
    
    debug!(
        "filter_items_by_any_id: source_count={}, target_count={}",
        source.len(),
        target.len()
    );

    let mut filtered = Vec::new();
    let mut skipped_empty = 0;
    let mut skipped_existing = 0;

    for item in source {
        let item_ids = item.get_media_ids();
        let item_imdb_id = item.get_imdb_id();
        
        // Skip if no IDs at all
        if item_ids.as_ref().map(|ids| ids.is_empty()).unwrap_or(true) && item_imdb_id.is_empty() {
            skipped_empty += 1;
            continue;
        }
        
        // Check if item matches any target item by any ID
        let mut found_match = false;
        for target_item in &target_items {
            let target_ids = target_item.get_media_ids();
            let target_imdb_id = target_item.get_imdb_id();
            
            // Direct imdb_id match
            if !item_imdb_id.is_empty() && !target_imdb_id.is_empty() {
                if item_imdb_id == target_imdb_id {
                    found_match = true;
                    break;
                }
            }
            
            // MediaIds match
            if let (Some(ref item_ids_val), Some(ref target_ids_val)) = (&item_ids, &target_ids) {
                if match_by_any_id(item_ids_val, target_ids_val) {
                    found_match = true;
                    break;
                }
            }
        }
        
        if found_match {
            skipped_existing += 1;
            if skipped_existing <= 5 {
                debug!(
                    "filter_items_by_any_id: Skipping item (already in target)"
                );
            }
            continue;
        }
        
        filtered.push(item.clone());
        if filtered.len() <= 5 {
            debug!(
                "filter_items_by_any_id: Adding item (not in target)"
            );
        }
    }

    debug!(
        "filter_items_by_any_id: result_count={}, skipped_empty={}, skipped_existing={}",
        filtered.len(),
        skipped_empty,
        skipped_existing
    );

    filtered
}

/// Filter items from source that are not present in target based on IMDB_ID
/// This is equivalent to the Python `filter_items()` function
/// Now also checks MediaIds as fallback when imdb_id is empty
pub fn filter_items_by_imdb_id<T>(source: &[T], target: &[T]) -> Vec<T>
where
    T: Clone + GetImdbId + GetMediaIds,
{
    use tracing::debug;
    
    let target_ids: std::collections::HashSet<String> = target
        .iter()
        .filter_map(|item| {
            let id = item.get_imdb_id();
            if id.is_empty() {
                None
            } else {
                Some(id)
            }
        })
        .collect();

    debug!(
        "filter_items_by_imdb_id: source_count={}, target_count={}, target_unique_ids={}",
        source.len(),
        target.len(),
        target_ids.len()
    );

    // Log sample of target IDs for debugging
    let target_ids_sample: Vec<String> = target_ids.iter().take(10).cloned().collect();
    if !target_ids_sample.is_empty() {
        debug!(
            "filter_items_by_imdb_id: sample target IMDB IDs: {:?}",
            target_ids_sample
        );
    }

    let mut filtered = Vec::new();
    let mut skipped_empty = 0;
    let mut skipped_existing = 0;

    for item in source {
        let id = item.get_imdb_id();
        
        // If imdb_id is empty, try to use MediaIds
        if id.is_empty() {
            if let Some(media_ids) = item.get_media_ids() {
                if !media_ids.is_empty() {
                    // Check if any ID from MediaIds matches target
                    let mut found_match = false;
                    if let Some(imdb) = &media_ids.imdb_id {
                        if target_ids.contains(imdb) {
                            found_match = true;
                        }
                    }
                    // Also check other ID types by converting to string format
                    if !found_match {
                        for _id_str in [
                            media_ids.trakt_id.map(|id| format!("trakt:{}", id)),
                            media_ids.simkl_id.map(|id| format!("simkl:{}", id)),
                            media_ids.tmdb_id.map(|id| format!("tmdb:{}", id)),
                            media_ids.tvdb_id.map(|id| format!("tvdb:{}", id)),
                            media_ids.slug.clone(),
                        ].into_iter().flatten() {
                            // For cross-ID matching, we'd need IdResolver, but for now just check imdb
                            // This is a simplified version - full implementation would use IdResolver
                        }
                    }
                    if found_match {
                        skipped_existing += 1;
                        continue;
                    }
                    // Item has MediaIds but no match - include it (it will be distributed)
                    filtered.push(item.clone());
                    if filtered.len() <= 5 {
                        debug!(
                            "filter_items_by_imdb_id: Adding item (not in target, has MediaIds but no imdb_id) imdb_id={:?}",
                            media_ids.imdb_id
                        );
                    }
                    continue;
                }
            }
            // No IDs at all - skip (this should be rare after resolution)
            skipped_empty += 1;
            if skipped_empty <= 5 {
                debug!(
                    "filter_items_by_imdb_id: Skipping item (no IDs at all) title={:?}",
                    // Try to get title for logging - this is a generic function so we can't easily access title
                    "unknown"
                );
            }
            continue;
        }
        
        if target_ids.contains(&id) {
            skipped_existing += 1;
            // Log first few items being skipped
            if skipped_existing <= 5 {
                debug!(
                    "filter_items_by_imdb_id: Skipping item (already in target) imdb_id={}",
                    id
                );
            }
            continue;
        }
        filtered.push(item.clone());
        // Log first few items being added
        if filtered.len() <= 5 {
            debug!(
                "filter_items_by_imdb_id: Adding item (not in target) imdb_id={}",
                id
            );
        }
    }

    debug!(
        "filter_items_by_imdb_id: result_count={}, skipped_empty={}, skipped_existing={}",
        filtered.len(),
        skipped_empty,
        skipped_existing
    );

    filtered
}

/// Remove duplicates from a list by any ID (MediaIds or imdb_id), keeping the first occurrence
pub fn remove_duplicates_by_id<T>(items: Vec<T>) -> Vec<T>
where
    T: Clone + GetImdbId + GetMediaIds,
{
    use crate::id_matching::match_by_any_id;
    
    let mut result: Vec<T> = Vec::new();

    for item in items {
        let mut is_duplicate = false;
        let item_ids = item.get_media_ids();
        let item_imdb_id = item.get_imdb_id();
        
        // Check against all existing items
        for existing in &result {
            let existing_ids = existing.get_media_ids();
            let existing_imdb_id = existing.get_imdb_id();
            
            // Direct imdb_id match
            if !item_imdb_id.is_empty() && !existing_imdb_id.is_empty() {
                if item_imdb_id == existing_imdb_id {
                    is_duplicate = true;
                    break;
                }
            }
            
            // MediaIds match
            if let (Some(ref item_ids_val), Some(ref existing_ids_val)) = (&item_ids, &existing_ids) {
                if match_by_any_id(item_ids_val, existing_ids_val) {
                    is_duplicate = true;
                    break;
                }
            }
        }
        
        if !is_duplicate {
            result.push(item);
        }
    }

    result
}

/// Remove duplicates from a list by IMDB_ID, keeping the first occurrence
/// Now also checks MediaIds in addition to imdb_id
pub fn remove_duplicates_by_imdb_id<T>(items: Vec<T>) -> Vec<T>
where
    T: Clone + GetImdbId + GetMediaIds,
{
    remove_duplicates_by_id(items)
}

/// Filter out items with missing IDs (only filter if ALL IDs are missing)
/// Items are kept if they have any ID (imdb_id or any ID in MediaIds)
pub fn filter_missing_ids<T>(items: Vec<T>) -> Vec<T>
where
    T: GetImdbId + GetMediaIds,
{
    items
        .into_iter()
        .filter(|item| {
            let imdb_id = item.get_imdb_id();
            let media_ids = item.get_media_ids();
            
            // Keep item if it has imdb_id
            if !imdb_id.is_empty() {
                return true;
            }
            
            // Keep item if it has any ID in MediaIds
            if let Some(ids) = media_ids {
                return !ids.is_empty();
            }
            
            // Filter out if no IDs at all
            false
        })
        .collect()
}

/// Filter out items with missing IMDB_ID
/// Now uses filter_missing_ids which checks all IDs
pub fn filter_missing_imdb_ids<T>(items: Vec<T>) -> Vec<T>
where
    T: GetImdbId + GetMediaIds,
{
    filter_missing_ids(items)
}

/// Trait for types that have an IMDB_ID
pub trait GetImdbId {
    fn get_imdb_id(&self) -> String;
}

/// Trait for types that have MediaIds
pub trait GetMediaIds {
    fn get_media_ids(&self) -> Option<media_sync_models::MediaIds>;
}

// Implement GetImdbId for common types
impl GetImdbId for media_sync_models::WatchlistItem {
    fn get_imdb_id(&self) -> String {
        self.imdb_id.clone()
    }
}

impl GetMediaIds for media_sync_models::WatchlistItem {
    fn get_media_ids(&self) -> Option<media_sync_models::MediaIds> {
        self.ids.clone()
    }
}

impl GetImdbId for media_sync_models::Rating {
    fn get_imdb_id(&self) -> String {
        self.imdb_id.clone()
    }
}

impl GetMediaIds for media_sync_models::Rating {
    fn get_media_ids(&self) -> Option<media_sync_models::MediaIds> {
        self.ids.clone()
    }
}

impl GetImdbId for media_sync_models::Review {
    fn get_imdb_id(&self) -> String {
        self.imdb_id.clone()
    }
}

impl GetMediaIds for media_sync_models::Review {
    fn get_media_ids(&self) -> Option<media_sync_models::MediaIds> {
        self.ids.clone()
    }
}

/// Filter reviews from source that are not present in target based on IMDB_ID and content similarity
/// This prevents duplicate reviews when the same review content exists for the same movie
pub fn filter_reviews_by_imdb_id_and_content(
    source: &[media_sync_models::Review],
    target: &[media_sync_models::Review],
) -> Vec<media_sync_models::Review> {
    use tracing::debug;
    
    // Create a set of (imdb_id, content_hash) pairs from target
    // Use a simple content hash (first 100 chars + length) for comparison
    let target_keys: std::collections::HashSet<(String, String)> = target
        .iter()
        .filter_map(|review| {
            let id = review.imdb_id.clone();
            if id.is_empty() {
                return None;
            }
            // Create a content key: first 100 chars + total length (to catch content differences)
            let content_key = if review.content.len() > 100 {
                format!("{}:{}", &review.content[..100], review.content.len())
            } else {
                format!("{}:{}", review.content, review.content.len())
            };
            Some((id, content_key))
        })
        .collect();

    debug!(
        "filter_reviews_by_imdb_id_and_content: source_count={}, target_count={}, target_unique_keys={}",
        source.len(),
        target.len(),
        target_keys.len()
    );

    let mut filtered = Vec::new();
    let mut skipped_empty = 0;
    let mut skipped_existing = 0;

    for review in source {
        let id = review.imdb_id.clone();
        if id.is_empty() {
            skipped_empty += 1;
            continue;
        }
        
        // Create content key for this review
        let content_key = if review.content.len() > 100 {
            format!("{}:{}", &review.content[..100], review.content.len())
        } else {
            format!("{}:{}", review.content, review.content.len())
        };
        
        // Check if this (imdb_id, content) combination already exists
        if target_keys.contains(&(id.clone(), content_key)) {
            skipped_existing += 1;
            if skipped_existing <= 5 {
                debug!(
                    "filter_reviews_by_imdb_id_and_content: Skipping review (already in target) imdb_id={}, content_length={}",
                    id, review.content.len()
                );
            }
            continue;
        }
        
        filtered.push(review.clone());
        if filtered.len() <= 5 {
            debug!(
                "filter_reviews_by_imdb_id_and_content: Adding review (not in target) imdb_id={}, content_length={}",
                id, review.content.len()
            );
        }
    }

    debug!(
        "filter_reviews_by_imdb_id_and_content: result_count={}, skipped_empty={}, skipped_existing={}",
        filtered.len(),
        skipped_empty,
        skipped_existing
    );

    filtered
}

impl GetImdbId for media_sync_models::WatchHistory {
    fn get_imdb_id(&self) -> String {
        self.imdb_id.clone()
    }
}

impl GetMediaIds for media_sync_models::WatchHistory {
    fn get_media_ids(&self) -> Option<media_sync_models::MediaIds> {
        self.ids.clone()
    }
}

/// Filter ratings that are new or have changed values
/// Returns ratings from source that either:
/// - Don't exist in target (new ratings)
/// - Exist in target but have different rating values (changed ratings)
pub fn filter_ratings_by_imdb_id_and_value(
    source: &[media_sync_models::Rating],
    target: &[media_sync_models::Rating],
) -> Vec<media_sync_models::Rating> {
    use tracing::debug;
    
    // Build map of target ratings by IMDB ID
    let target_ratings: std::collections::HashMap<String, u8> = target
        .iter()
        .filter_map(|rating| {
            if rating.imdb_id.is_empty() {
                None
            } else {
                Some((rating.imdb_id.clone(), rating.rating))
            }
        })
        .collect();
    
    let mut filtered = Vec::new();
    let mut skipped_unchanged = 0;
    let mut skipped_empty = 0;
    
    for rating in source {
        if rating.imdb_id.is_empty() {
            skipped_empty += 1;
            continue;
        }
        
        match target_ratings.get(&rating.imdb_id) {
            None => {
                // New rating - doesn't exist in target
                filtered.push(rating.clone());
                if filtered.len() <= 5 {
                    debug!(
                        "filter_ratings_by_imdb_id_and_value: Adding new rating imdb_id={}, rating={}",
                        rating.imdb_id, rating.rating
                    );
                }
            }
            Some(&existing_rating) => {
                if rating.rating != existing_rating {
                    // Rating changed - different value
                    filtered.push(rating.clone());
                    if filtered.len() <= 5 {
                        debug!(
                            "filter_ratings_by_imdb_id_and_value: Adding changed rating imdb_id={}, old_rating={}, new_rating={}",
                            rating.imdb_id, existing_rating, rating.rating
                        );
                    }
                } else {
                    // Rating unchanged - skip
                    skipped_unchanged += 1;
                    if skipped_unchanged <= 5 {
                        debug!(
                            "filter_ratings_by_imdb_id_and_value: Skipping unchanged rating imdb_id={}, rating={}",
                            rating.imdb_id, rating.rating
                        );
                    }
                }
            }
        }
    }
    
    debug!(
        "filter_ratings_by_imdb_id_and_value: source_count={}, target_count={}, \
         filtered_count={}, skipped_unchanged={}, skipped_empty={}",
        source.len(),
        target.len(),
        filtered.len(),
        skipped_unchanged,
        skipped_empty
    );
    
    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_sync_models::{MediaType, Rating, RatingSource, WatchlistItem};
    use chrono::Utc;

    fn create_watchlist_item(imdb_id: &str, title: &str) -> WatchlistItem {
        WatchlistItem {
            imdb_id: imdb_id.to_string(),
            ids: None,
            title: title.to_string(),
            year: Some(2020),
            media_type: MediaType::Movie,
            date_added: Utc::now(),
            source: "test".to_string(),
            status: None,
        }
    }

    fn create_rating(imdb_id: &str, rating: u8) -> Rating {
        Rating {
            imdb_id: imdb_id.to_string(),
            ids: None,
            rating,
            date_added: Utc::now(),
            media_type: MediaType::Movie,
            source: RatingSource::Imdb,
        }
    }

    #[test]
    fn test_filter_items_by_imdb_id() {
        let source = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("tt002", "Movie 2"),
            create_watchlist_item("tt003", "Movie 3"),
        ];
        let target = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("tt002", "Movie 2"),
        ];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].imdb_id, "tt003");
    }

    #[test]
    fn test_filter_items_by_imdb_id_empty_target() {
        let source = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("tt002", "Movie 2"),
        ];
        let target = vec![];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_items_by_imdb_id_empty_source() {
        let source = vec![];
        let target = vec![create_watchlist_item("tt001", "Movie 1")];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_filter_items_by_imdb_id_with_empty_imdb_id() {
        let source = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("", "Movie 2"), // Empty IMDB ID
        ];
        let target = vec![];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 1); // Empty IMDB ID should be filtered
        assert_eq!(filtered[0].imdb_id, "tt001");
    }

    #[test]
    fn test_remove_duplicates_by_imdb_id() {
        let items = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("tt001", "Movie 1 Duplicate"),
            create_watchlist_item("tt002", "Movie 2"),
            create_watchlist_item("tt002", "Movie 2 Duplicate"),
        ];

        let deduped = remove_duplicates_by_imdb_id(items);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].imdb_id, "tt001");
        assert_eq!(deduped[1].imdb_id, "tt002");
    }

    #[test]
    fn test_remove_duplicates_by_imdb_id_with_empty_ids() {
        let items = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("", "Movie 2"),
            create_watchlist_item("", "Movie 3"),
        ];

        let deduped = remove_duplicates_by_imdb_id(items);
        assert_eq!(deduped.len(), 1); // Empty IDs are filtered
        assert_eq!(deduped[0].imdb_id, "tt001");
    }

    #[test]
    fn test_filter_missing_imdb_ids() {
        let items = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("", "Movie 2"),
            create_watchlist_item("tt003", "Movie 3"),
        ];

        let filtered = filter_missing_imdb_ids(items);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].imdb_id, "tt001");
        assert_eq!(filtered[1].imdb_id, "tt003");
    }

    #[test]
    fn test_filter_items_by_imdb_id_with_ratings() {
        let source = vec![
            create_rating("tt001", 10),
            create_rating("tt002", 9),
            create_rating("tt003", 8),
        ];
        let target = vec![
            create_rating("tt001", 10),
        ];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].imdb_id, "tt002");
        assert_eq!(filtered[1].imdb_id, "tt003");
    }
}

