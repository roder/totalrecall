use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem};
use media_sync_config::{ResolutionConfig, ResolutionStrategy};
use chrono::DateTime;
use chrono::Utc;
use std::collections::HashMap;
use tracing::debug;
use std::collections::HashSet;

pub struct SourceData {
    pub watchlist: Vec<WatchlistItem>,
    pub ratings: Vec<Rating>,
    pub reviews: Vec<Review>,
    pub watch_history: Vec<WatchHistory>,
}

#[derive(Clone)]
pub struct ResolvedData {
    pub watchlist: Vec<WatchlistItem>,
    pub ratings: Vec<Rating>,
    pub reviews: Vec<Review>,
    pub watch_history: Vec<WatchHistory>,
}

/// Resolve conflicts across all sources using configurable strategy
pub fn resolve_all_conflicts(
    source_data: &[(&str, &SourceData)],
    resolution_config: &ResolutionConfig,
) -> ResolvedData {
    ResolvedData {
        watchlist: resolve_watchlist(
            source_data,
            resolution_config,
        ),
        ratings: resolve_ratings(
            source_data,
            resolution_config,
        ),
        reviews: resolve_reviews(
            source_data,
        ),
        watch_history: resolve_watch_history(
            source_data,
        ),
    }
}

/// Generate a grouping key from MediaIds, using any available ID
fn get_grouping_key_from_rating(rating: &Rating) -> Option<String> {
    if let Some(ref ids) = rating.ids {
        ids.get_any_id()
    } else if !rating.imdb_id.is_empty() {
        Some(rating.imdb_id.clone())
    } else {
        None
    }
}

/// Check if two ratings match by any ID
fn ratings_match(rating1: &Rating, rating2: &Rating) -> bool {
    // Direct imdb_id match
    if !rating1.imdb_id.is_empty() && !rating2.imdb_id.is_empty() {
        if rating1.imdb_id == rating2.imdb_id {
            return true;
        }
    }
    
    // MediaIds match
    if let (Some(ref ids1), Some(ref ids2)) = (&rating1.ids, &rating2.ids) {
        use crate::id_matching::match_by_any_id;
        if match_by_any_id(ids1, ids2) {
            return true;
        }
    }
    
    false
}

/// Check if two watchlist items match by any ID
fn watchlist_items_match(item1: &WatchlistItem, item2: &WatchlistItem) -> bool {
    // Direct imdb_id match
    if !item1.imdb_id.is_empty() && !item2.imdb_id.is_empty() {
        if item1.imdb_id == item2.imdb_id {
            return true;
        }
    }
    
    // MediaIds match
    if let (Some(ref ids1), Some(ref ids2)) = (&item1.ids, &item2.ids) {
        use crate::id_matching::match_by_any_id;
        if match_by_any_id(ids1, ids2) {
            return true;
        }
    }
    
    false
}

fn resolve_ratings(
    source_data: &[(&str, &SourceData)],
    resolution_config: &ResolutionConfig,
) -> Vec<Rating> {
    // Build map of all ratings by any available ID
    // Use a two-pass approach: first group by key, then merge groups that match by any ID
    let mut all_ratings: Vec<(&str, &Rating)> = Vec::new();
    
    // Collect all ratings
    for (source_name, data) in source_data {
        for rating in &data.ratings {
            all_ratings.push((source_name, rating));
        }
    }
    
    // Group ratings that match by any ID
    let mut groups: Vec<Vec<(&str, &Rating)>> = Vec::new();
    let mut assigned = std::collections::HashSet::new();
    
    for (idx, (source_name, rating)) in all_ratings.iter().enumerate() {
        if assigned.contains(&idx) {
            continue;
        }
        
        let mut group = vec![(*source_name, *rating)];
        assigned.insert(idx);
        
        // Find all other ratings that match this one
        for (other_idx, (other_source, other_rating)) in all_ratings.iter().enumerate().skip(idx + 1) {
            if assigned.contains(&other_idx) {
                continue;
            }
            
            if ratings_match(rating, other_rating) {
                group.push((*other_source, *other_rating));
                assigned.insert(other_idx);
            }
        }
        
        groups.push(group);
    }
    
    // Use per-type strategy if specified, otherwise global strategy
    let strategy = resolution_config.ratings_strategy
        .as_ref()
        .unwrap_or(&resolution_config.strategy);
    
    // Resolve each group
    let mut resolved = Vec::new();
    for candidates in groups {
        if candidates.len() == 1 {
            // Only one source has it, use that
            resolved.push(candidates[0].1.clone());
        } else {
            // Multiple sources have it, resolve conflict
            let mut resolved_rating = resolve_rating_conflict(
                &candidates,
                strategy,
                resolution_config,
            );
            // Merge MediaIds from all candidates
            let mut merged_ids = resolved_rating.ids.clone().unwrap_or_default();
            for (_, rating) in &candidates {
                if let Some(ref ids) = rating.ids {
                    merged_ids.merge(ids);
                }
            }
            if !merged_ids.is_empty() {
                resolved_rating.ids = Some(merged_ids);
            }
            resolved.push(resolved_rating);
        }
    }
    
    resolved
}

fn resolve_rating_conflict(
    candidates: &[(&str, &Rating)],
    strategy: &ResolutionStrategy,
    resolution_config: &ResolutionConfig,
) -> Rating {
    // Sort by timestamp
    let mut sorted = candidates.to_vec();
    match strategy {
        ResolutionStrategy::Newest => {
            sorted.sort_by_key(|(_, rating)| std::cmp::Reverse(rating.date_added));
        }
        ResolutionStrategy::Oldest => {
            sorted.sort_by_key(|(_, rating)| rating.date_added);
        }
        ResolutionStrategy::Preference => {
            // Sort by timestamp first, but will use preference if within tolerance
            sorted.sort_by_key(|(_, rating)| std::cmp::Reverse(rating.date_added));
        }
        ResolutionStrategy::Merge => {
            // Merge not applicable for ratings (single value per item)
            // Fall back to newest
            sorted.sort_by_key(|(_, rating)| std::cmp::Reverse(rating.date_added));
        }
    }
    
    // Check if timestamps are within tolerance
    if sorted.len() > 1 {
        let first_time = sorted[0].1.date_added;
        let second_time = sorted[1].1.date_added;
        let time_diff = (first_time - second_time).num_seconds().abs();
        
        if time_diff <= resolution_config.timestamp_tolerance_seconds {
            // Timestamps are within tolerance - use preference strategy
            // Use first source from source_preference as fallback
            for preferred_source in &resolution_config.source_preference {
                if let Some(candidate) = sorted.iter().find(|(name, _)| name == preferred_source) {
                    return candidate.1.clone();
                }
            }
        }
    }
    
    // Timestamps differ significantly, or no preference match - use strategy
    match strategy {
        ResolutionStrategy::Newest | ResolutionStrategy::Preference | ResolutionStrategy::Merge => {
            // Most recent (already sorted)
            sorted[0].1.clone()
        }
        ResolutionStrategy::Oldest => {
            // Oldest (already sorted)
            sorted[0].1.clone()
        }
    }
}

fn resolve_watchlist(
    source_data: &[(&str, &SourceData)],
    resolution_config: &ResolutionConfig,
) -> Vec<WatchlistItem> {
    // Use per-type strategy if specified, otherwise global strategy
    let strategy = resolution_config.watchlist_strategy
        .as_ref()
        .unwrap_or(&resolution_config.strategy);
    
    match strategy {
        ResolutionStrategy::Merge => {
            // Merge: Union all watchlist items from all sources, matching by any ID
            let mut all_items: Vec<WatchlistItem> = Vec::new();
            
            // Add items from all sources, matching by any ID
            for (_, data) in source_data {
                for item in &data.watchlist {
                    // Try to find matching item in all_items
                    let mut found_match = false;
                    for existing in &mut all_items {
                        if watchlist_items_match(existing, item) {
                            // Merge MediaIds
                            if let Some(ref item_ids) = item.ids {
                                if let Some(ref mut existing_ids) = existing.ids {
                                    existing_ids.merge(item_ids);
                                } else {
                                    existing.ids = Some(item_ids.clone());
                                }
                            }
                            
                            // Prefer item with status if the other doesn't have one
                            let existing_has_status = existing.status.is_some();
                            let item_has_status = item.status.is_some();
                            
                            if item_has_status && !existing_has_status {
                                // New item has status, existing doesn't - use new item
                                *existing = item.clone();
                            } else if !item_has_status && existing_has_status {
                                // Existing has status, new doesn't - keep existing
                                // Do nothing
                            } else if item.date_added > existing.date_added {
                                // Both have status or both don't - keep most recent
                                *existing = item.clone();
                            }
                            found_match = true;
                            break;
                        }
                    }
                    
                    if !found_match {
                        // Log items being added without matches (for debugging)
                        if item.ids.is_none() || item.ids.as_ref().map(|ids| ids.is_empty()).unwrap_or(true) {
                            tracing::trace!("resolve_watchlist: Adding item without IDs (will be skipped in distribution): '{}' (year: {:?})", 
                                   item.title, item.year);
                        }
                        all_items.push(item.clone());
                    }
                }
            }
            
            all_items
        }
        _ => {
            // Use same conflict resolution as ratings (timestamp + preference)
            // Group items that match by any ID
            let mut all_items: Vec<(&str, &WatchlistItem)> = Vec::new();
            
            // Collect all items
            for (source_name, data) in source_data {
                for item in &data.watchlist {
                    all_items.push((source_name, item));
                }
            }
            
            // Group items that match by any ID
            let mut groups: Vec<Vec<(&str, &WatchlistItem)>> = Vec::new();
            let mut assigned = HashSet::new();
            
            for (idx, (source_name, item)) in all_items.iter().enumerate() {
                if assigned.contains(&idx) {
                    continue;
                }
                
                let mut group = vec![(*source_name, *item)];
                assigned.insert(idx);
                
                // Find all other items that match this one
                for (other_idx, (other_source, other_item)) in all_items.iter().enumerate().skip(idx + 1) {
                    if assigned.contains(&other_idx) {
                        continue;
                    }
                    
                    if watchlist_items_match(item, other_item) {
                        group.push((*other_source, *other_item));
                        assigned.insert(other_idx);
                    }
                }
                
                groups.push(group);
            }
            
            // Resolve each group (same logic as ratings)
            let mut resolved = Vec::new();
            for candidates in groups {
                if candidates.len() == 1 {
                    resolved.push(candidates[0].1.clone());
                } else {
                    // Resolve conflict using same logic as ratings
                    let mut sorted = candidates.to_vec();
                    sorted.sort_by_key(|(_, item)| std::cmp::Reverse(item.date_added));
                    
                    // Apply timestamp tolerance and preference logic (similar to ratings)
                    let mut resolved_item = resolve_watchlist_conflict(
                        &sorted,
                        strategy,
                        resolution_config,
                    );
                    // Merge MediaIds from all candidates
                    let mut merged_ids = resolved_item.ids.clone().unwrap_or_default();
                    for (_, item) in candidates {
                        if let Some(ref ids) = item.ids {
                            merged_ids.merge(ids);
                        }
                    }
                    if !merged_ids.is_empty() {
                        resolved_item.ids = Some(merged_ids);
                    }
                    resolved.push(resolved_item);
                }
            }
            
            resolved
        }
    }
}

fn resolve_watchlist_conflict(
    sorted: &[(&str, &WatchlistItem)],
    _strategy: &ResolutionStrategy,
    resolution_config: &ResolutionConfig,
) -> WatchlistItem {
    // Similar logic to resolve_rating_conflict but for WatchlistItem
    if sorted.len() > 1 {
        let first_time = sorted[0].1.date_added;
        let second_time = sorted[1].1.date_added;
        let time_diff = (first_time - second_time).num_seconds().abs();
        
        if time_diff <= resolution_config.timestamp_tolerance_seconds {
            // Use first source from source_preference as fallback
            for preferred_source in &resolution_config.source_preference {
                if let Some(candidate) = sorted.iter().find(|(name, _)| name == preferred_source) {
                    return candidate.1.clone();
                }
            }
        }
    }
    
    sorted[0].1.clone()
}

/// Check if two reviews match by any ID and content
fn reviews_match(review1: &Review, review2: &Review) -> bool {
    // Must have same content
    if review1.content != review2.content {
        return false;
    }
    
    // Direct imdb_id match
    if !review1.imdb_id.is_empty() && !review2.imdb_id.is_empty() {
        if review1.imdb_id == review2.imdb_id {
            return true;
        }
    }
    
    // MediaIds match
    if let (Some(ref ids1), Some(ref ids2)) = (&review1.ids, &review2.ids) {
        use crate::id_matching::match_by_any_id;
        if match_by_any_id(ids1, ids2) {
            return true;
        }
    }
    
    false
}

fn resolve_reviews(
    source_data: &[(&str, &SourceData)],
) -> Vec<Review> {
    // Reviews always use merge strategy - keep all reviews from all sources
    let mut all_reviews: Vec<Review> = Vec::new();
    
    for (_, data) in source_data {
        all_reviews.extend(data.reviews.iter().cloned());
    }
    
    // Deduplicate by matching any ID and content to avoid exact duplicates
    let mut deduplicated: Vec<Review> = Vec::new();
    for review in all_reviews {
        let mut is_duplicate = false;
        for existing in &deduplicated {
            if reviews_match(&review, existing) {
                is_duplicate = true;
                break;
            }
        }
        if !is_duplicate {
            deduplicated.push(review);
        }
    }
    
    // Sort by date_added (most recent first)
    deduplicated.sort_by_key(|r| std::cmp::Reverse(r.date_added));
    deduplicated
}

/// Check if two watch history entries match by any ID and watched_at
fn watch_history_match(entry1: &WatchHistory, entry2: &WatchHistory) -> bool {
    // Must have same watched_at (within small tolerance for floating point)
    let time_diff = (entry1.watched_at - entry2.watched_at).num_seconds().abs();
    if time_diff > 1 {
        return false;
    }
    
    // Direct imdb_id match
    if !entry1.imdb_id.is_empty() && !entry2.imdb_id.is_empty() {
        if entry1.imdb_id == entry2.imdb_id {
            return true;
        }
    }
    
    // MediaIds match
    if let (Some(ref ids1), Some(ref ids2)) = (&entry1.ids, &entry2.ids) {
        use crate::id_matching::match_by_any_id;
        if match_by_any_id(ids1, ids2) {
            return true;
        }
    }
    
    false
}

fn resolve_watch_history(
    source_data: &[(&str, &SourceData)],
) -> Vec<WatchHistory> {
    // Watch history always uses merge strategy - keep all entries from all sources
    let mut all_history: Vec<WatchHistory> = Vec::new();
    
    for (_, data) in source_data {
        all_history.extend(data.watch_history.iter().cloned());
    }
    
    // Deduplicate by matching any ID and watched_at - same item watched at same time
    let mut deduplicated: Vec<WatchHistory> = Vec::new();
    for entry in all_history {
        let mut is_duplicate = false;
        for existing in &deduplicated {
            if watch_history_match(&entry, existing) {
                is_duplicate = true;
                break;
            }
        }
        if !is_duplicate {
            deduplicated.push(entry);
        }
    }
    
    // Sort by watched_at (most recent first)
    deduplicated.sort_by_key(|e| std::cmp::Reverse(e.watched_at));
    deduplicated
}

