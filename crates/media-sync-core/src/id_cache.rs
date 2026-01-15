use std::collections::HashMap;
use std::sync::Arc;
use media_sync_models::{MediaIds, MediaType};

/// Key for title/year lookups: (title_lowercase, year, media_type_string)
type TitleYearKey = (String, Option<u32>, String);

/// In-memory ID cache with multi-index structure
/// 
/// Provides O(1) lookups by any ID type while avoiding duplication.
/// Uses Arc<MediaIds> to share the same MediaIds instance across all indices.
pub struct IdCache {
    /// Primary index: IMDB ID -> MediaIds (canonical entry)
    by_imdb: HashMap<String, Arc<MediaIds>>,
    
    /// Secondary indices: point to same Arc<MediaIds> (no duplication)
    by_trakt: HashMap<u64, Arc<MediaIds>>,
    by_simkl: HashMap<u64, Arc<MediaIds>>,
    by_tmdb: HashMap<u32, Arc<MediaIds>>,
    by_tvdb: HashMap<u32, Arc<MediaIds>>,
    by_slug: HashMap<String, Arc<MediaIds>>,
    by_plex_rating_key: HashMap<String, Arc<MediaIds>>,
    
    /// Title/year index for efficient title-based lookups
    /// Key: (title_lowercase, year, media_type_string)
    by_title_year: HashMap<TitleYearKey, Arc<MediaIds>>,
    
    /// Track dirty state for incremental saves
    dirty: bool,
}

impl IdCache {
    pub fn new() -> Self {
        Self {
            by_imdb: HashMap::new(),
            by_trakt: HashMap::new(),
            by_simkl: HashMap::new(),
            by_tmdb: HashMap::new(),
            by_tvdb: HashMap::new(),
            by_slug: HashMap::new(),
            by_plex_rating_key: HashMap::new(),
            by_title_year: HashMap::new(),
            dirty: false,
        }
    }
    
    /// Normalize title for indexing (lowercase, trim)
    fn normalize_title(title: &str) -> String {
        title.trim().to_lowercase()
    }
    
    /// Create a title/year key for indexing
    fn make_title_key(title: &str, year: Option<u32>, media_type: &MediaType) -> TitleYearKey {
        (Self::normalize_title(title), year, format!("{:?}", media_type))
    }
    
    /// Insert or update IDs (merges with existing if found)
    pub fn insert(&mut self, ids: MediaIds) {
        // Find existing entry by any matching ID
        let existing = self.find_existing(&ids);
        
        let canonical = if let Some(existing) = existing {
            // Merge with existing
            let mut merged = (*existing).clone();
            merged.merge(&ids);
            Arc::new(merged)
        } else {
            Arc::new(ids)
        };
        
        // Update all indices
        if let Some(ref imdb) = canonical.imdb_id {
            self.by_imdb.insert(imdb.clone(), canonical.clone());
        }
        if let Some(trakt) = canonical.trakt_id {
            self.by_trakt.insert(trakt, canonical.clone());
        }
        if let Some(simkl) = canonical.simkl_id {
            self.by_simkl.insert(simkl, canonical.clone());
        }
        if let Some(tmdb) = canonical.tmdb_id {
            self.by_tmdb.insert(tmdb, canonical.clone());
        }
        if let Some(tvdb) = canonical.tvdb_id {
            self.by_tvdb.insert(tvdb, canonical.clone());
        }
        if let Some(ref slug) = canonical.slug {
            self.by_slug.insert(slug.clone(), canonical.clone());
        }
        if let Some(ref plex_rating_key) = canonical.plex_rating_key {
            self.by_plex_rating_key.insert(plex_rating_key.clone(), canonical.clone());
        }
        
        // Update title/year index if metadata is available
        if let (Some(ref title), Some(ref media_type)) = (&canonical.title, &canonical.media_type) {
            let key = Self::make_title_key(title, canonical.year, media_type);
            self.by_title_year.insert(key, canonical.clone());
        }
        
        self.dirty = true;
    }
    
    /// Find by any ID type - O(1) lookup
    pub fn find_by_any_id(&self, id: &str) -> Option<Arc<MediaIds>> {
        // Try IMDB first (most common, format: "tt1234567")
        if id.starts_with("tt") {
            if let Some(ids) = self.by_imdb.get(id) {
                return Some(ids.clone());
            }
        }
        
        // Try other formats
        if let Some(trakt_id) = id.strip_prefix("trakt:").and_then(|s| s.parse().ok()) {
            if let Some(ids) = self.by_trakt.get(&trakt_id) {
                return Some(ids.clone());
            }
        }
        
        if let Some(simkl_id) = id.strip_prefix("simkl:").and_then(|s| s.parse().ok()) {
            if let Some(ids) = self.by_simkl.get(&simkl_id) {
                return Some(ids.clone());
            }
        }
        
        if let Some(tmdb_id) = id.strip_prefix("tmdb:").and_then(|s| s.parse().ok()) {
            if let Some(ids) = self.by_tmdb.get(&tmdb_id) {
                return Some(ids.clone());
            }
        }
        
        if let Some(tvdb_id) = id.strip_prefix("tvdb:").and_then(|s| s.parse().ok()) {
            if let Some(ids) = self.by_tvdb.get(&tvdb_id) {
                return Some(ids.clone());
            }
        }
        
        // Try slug (direct match)
        if let Some(ids) = self.by_slug.get(id) {
            return Some(ids.clone());
        }
        
        // Try plex_rating_key (direct match, could be /library/metadata/... format)
        if let Some(ids) = self.by_plex_rating_key.get(id) {
            return Some(ids.clone());
        }
        
        // Try as IMDB ID even if it doesn't start with "tt" (for backward compatibility)
        if let Some(ids) = self.by_imdb.get(id) {
            return Some(ids.clone());
        }
        
        None
    }
    
    /// Find by title and year - O(1) lookup
    /// 
    /// Returns the first matching entry found in the title/year index.
    /// This is useful for avoiding external lookups when we only have title/year.
    pub fn find_by_title_year(&self, title: &str, year: Option<u32>, media_type: &MediaType) -> Option<Arc<MediaIds>> {
        let key = Self::make_title_key(title, year, media_type);
        self.by_title_year.get(&key).cloned()
    }
    
    /// Find existing entry by any ID in the provided MediaIds
    fn find_existing(&self, ids: &MediaIds) -> Option<Arc<MediaIds>> {
        // Check all ID types to find existing entry
        if let Some(ref imdb) = ids.imdb_id {
            if let Some(existing) = self.by_imdb.get(imdb) {
                return Some(existing.clone());
            }
        }
        if let Some(trakt) = ids.trakt_id {
            if let Some(existing) = self.by_trakt.get(&trakt) {
                return Some(existing.clone());
            }
        }
        if let Some(simkl) = ids.simkl_id {
            if let Some(existing) = self.by_simkl.get(&simkl) {
                return Some(existing.clone());
            }
        }
        if let Some(tmdb) = ids.tmdb_id {
            if let Some(existing) = self.by_tmdb.get(&tmdb) {
                return Some(existing.clone());
            }
        }
        if let Some(tvdb) = ids.tvdb_id {
            if let Some(existing) = self.by_tvdb.get(&tvdb) {
                return Some(existing.clone());
            }
        }
        if let Some(ref slug) = ids.slug {
            if let Some(existing) = self.by_slug.get(slug) {
                return Some(existing.clone());
            }
        }
        if let Some(ref plex_rating_key) = ids.plex_rating_key {
            if let Some(existing) = self.by_plex_rating_key.get(plex_rating_key) {
                return Some(existing.clone());
            }
        }
        None
    }
    
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }
    
    pub fn len(&self) -> usize {
        self.by_imdb.len()
    }
    
    /// Get the size of the title/year index (for debugging)
    pub fn title_year_index_size(&self) -> usize {
        self.by_title_year.len()
    }
    
    /// Rebuild the title/year index from all entries that have metadata
    /// This is useful after loading a cache to ensure the index is populated
    pub fn rebuild_title_year_index(&mut self) {
        // Clear existing index
        self.by_title_year.clear();
        
        // Rebuild from all entries in the cache
        for ids in self.by_imdb.values() {
            if let (Some(ref title), Some(ref media_type)) = (&ids.title, &ids.media_type) {
                let key = Self::make_title_key(title, ids.year, media_type);
                self.by_title_year.insert(key, ids.clone());
            }
        }
    }
    
    /// Get all entries as a vector (for serialization)
    pub fn all_entries(&self) -> Vec<MediaIds> {
        // Use a HashSet to deduplicate by IMDB ID
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        
        for ids in self.by_imdb.values() {
            if let Some(ref imdb) = ids.imdb_id {
                if seen.insert(imdb.clone()) {
                    result.push((**ids).clone());
                }
            }
        }
        
        result
    }
}

impl Default for IdCache {
    fn default() -> Self {
        Self::new()
    }
}

