// ID matching utilities for flexible matching using MediaIds

use media_sync_models::MediaIds;
use crate::id_resolver::IdResolver;

/// Check if two MediaIds share any common ID
/// 
/// Returns true if both have at least one matching ID type.
pub fn match_by_any_id(ids1: &MediaIds, ids2: &MediaIds) -> bool {
    // Check each ID type for matches
    if let (Some(ref imdb1), Some(ref imdb2)) = (&ids1.imdb_id, &ids2.imdb_id) {
        if imdb1 == imdb2 {
            return true;
        }
    }
    
    if let (Some(trakt1), Some(trakt2)) = (ids1.trakt_id, ids2.trakt_id) {
        if trakt1 == trakt2 {
            return true;
        }
    }
    
    if let (Some(simkl1), Some(simkl2)) = (ids1.simkl_id, ids2.simkl_id) {
        if simkl1 == simkl2 {
            return true;
        }
    }
    
    if let (Some(tmdb1), Some(tmdb2)) = (ids1.tmdb_id, ids2.tmdb_id) {
        if tmdb1 == tmdb2 {
            return true;
        }
    }
    
    if let (Some(tvdb1), Some(tvdb2)) = (ids1.tvdb_id, ids2.tvdb_id) {
        if tvdb1 == tvdb2 {
            return true;
        }
    }
    
    if let (Some(ref slug1), Some(ref slug2)) = (&ids1.slug, &ids2.slug) {
        if slug1 == slug2 {
            return true;
        }
    }
    
    false
}

/// Find a matching item in a collection using ID cache for cross-ID resolution
/// 
/// This function uses the ID resolver to find matches even when items have different
/// ID types (e.g., one has trakt_id, another has imdb_id).
pub fn find_matching_item<T, F>(
    item: &T,
    collection: &[T],
    get_ids: F,
    id_resolver: &IdResolver,
) -> Option<usize>
where
    F: Fn(&T) -> Option<MediaIds>,
{
    let item_ids = get_ids(item)?;
    
    for (index, candidate) in collection.iter().enumerate() {
        if let Some(candidate_ids) = get_ids(candidate) {
            // Direct match
            if match_by_any_id(&item_ids, &candidate_ids) {
                return Some(index);
            }
            
            // Cross-ID match using resolver
            // Check if any ID from item matches any ID from candidate via cache
            if let Some(imdb) = &item_ids.imdb_id {
                if let Some(cached) = id_resolver.find_by_any_id(imdb) {
                    if match_by_any_id(&cached, &candidate_ids) {
                        return Some(index);
                    }
                }
            }
            
            if let Some(trakt) = item_ids.trakt_id {
                let trakt_str = format!("trakt:{}", trakt);
                if let Some(cached) = id_resolver.find_by_any_id(&trakt_str) {
                    if match_by_any_id(&cached, &candidate_ids) {
                        return Some(index);
                    }
                }
            }
            
            if let Some(simkl) = item_ids.simkl_id {
                let simkl_str = format!("simkl:{}", simkl);
                if let Some(cached) = id_resolver.find_by_any_id(&simkl_str) {
                    if match_by_any_id(&cached, &candidate_ids) {
                        return Some(index);
                    }
                }
            }
            
            if let Some(tmdb) = item_ids.tmdb_id {
                let tmdb_str = format!("tmdb:{}", tmdb);
                if let Some(cached) = id_resolver.find_by_any_id(&tmdb_str) {
                    if match_by_any_id(&cached, &candidate_ids) {
                        return Some(index);
                    }
                }
            }
            
            if let Some(tvdb) = item_ids.tvdb_id {
                let tvdb_str = format!("tvdb:{}", tvdb);
                if let Some(cached) = id_resolver.find_by_any_id(&tvdb_str) {
                    if match_by_any_id(&cached, &candidate_ids) {
                        return Some(index);
                    }
                }
            }
        }
    }
    
    None
}

/// Group items by any matching ID using ID cache
/// 
/// Groups items that share any common ID, using the ID cache to resolve
/// cross-ID matches (e.g., trakt_id matches imdb_id via cache).
pub fn group_by_media_ids<T, F>(
    items: &[T],
    get_ids: F,
    id_resolver: &IdResolver,
) -> Vec<Vec<usize>>
where
    F: Fn(&T) -> Option<MediaIds>,
{
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut assigned = std::collections::HashSet::new();
    
    for (index, item) in items.iter().enumerate() {
        if assigned.contains(&index) {
            continue;
        }
        
        let item_ids = match get_ids(item) {
            Some(ids) => ids,
            None => continue,
        };
        
        let mut group = vec![index];
        assigned.insert(index);
        
        // Find all items that match this one
        for (other_index, other_item) in items.iter().enumerate().skip(index + 1) {
            if assigned.contains(&other_index) {
                continue;
            }
            
            if let Some(other_ids) = get_ids(other_item) {
                // Direct match
                if match_by_any_id(&item_ids, &other_ids) {
                    group.push(other_index);
                    assigned.insert(other_index);
                    continue;
                }
                
                // Cross-ID match via cache
                let mut matched = false;
                
                // Check all IDs from item_ids against cache
                if let Some(imdb) = &item_ids.imdb_id {
                    if let Some(cached) = id_resolver.find_by_any_id(imdb) {
                        if match_by_any_id(&cached, &other_ids) {
                            matched = true;
                        }
                    }
                }
                
                if !matched {
                    // Try other ID types
                    for id_str in [
                        item_ids.trakt_id.map(|id| format!("trakt:{}", id)),
                        item_ids.simkl_id.map(|id| format!("simkl:{}", id)),
                        item_ids.tmdb_id.map(|id| format!("tmdb:{}", id)),
                        item_ids.tvdb_id.map(|id| format!("tvdb:{}", id)),
                        item_ids.slug.clone(),
                    ].into_iter().flatten() {
                        if let Some(cached) = id_resolver.find_by_any_id(&id_str) {
                            if match_by_any_id(&cached, &other_ids) {
                                matched = true;
                                break;
                            }
                        }
                    }
                }
                
                if matched {
                    group.push(other_index);
                    assigned.insert(other_index);
                }
            }
        }
        
        groups.push(group);
    }
    
    groups
}


