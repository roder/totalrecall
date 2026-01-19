use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use crate::MediaType;
use chrono::{DateTime, Utc};

/// Normalized media identifiers from all sources
/// 
/// This struct aggregates IDs from various sources (IMDB, Trakt, Simkl, TMDB, TVDB, etc.)
/// to enable flexible matching and reconciliation across platforms.
/// 
/// Optionally includes title, year, and media_type for title-based cache lookups.
/// For episodes, also includes show_title, episode_title, and original_air_date.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaIds {
    pub imdb_id: Option<String>,
    pub trakt_id: Option<u64>,
    pub simkl_id: Option<u64>,
    pub tmdb_id: Option<u32>,
    pub tvdb_id: Option<u32>,
    pub slug: Option<String>,
    pub plex_rating_key: Option<String>,
    
    /// Optional metadata for title-based cache lookups
    /// These fields are not used for ID matching but enable efficient cache queries
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<MediaType>,
    
    /// Episode-specific metadata (only used when media_type is Episode)
    /// Show title for episodes (e.g., "Code Geass" for episode "The Taste of Humiliation")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_title: Option<String>,
    /// Episode title (e.g., "The Taste of Humiliation")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
    /// Original air date of the episode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_air_date: Option<DateTime<Utc>>,
}

impl MediaIds {
    /// Create an empty MediaIds struct
    pub fn new() -> Self {
        Self {
            imdb_id: None,
            trakt_id: None,
            simkl_id: None,
            tmdb_id: None,
            tvdb_id: None,
            slug: None,
            plex_rating_key: None,
            title: None,
            year: None,
            media_type: None,
            show_title: None,
            episode_title: None,
            original_air_date: None,
        }
    }

    /// Get the primary identifier (prefer imdb_id, fallback to others)
    /// 
    /// Returns a string representation of the best available ID for backward compatibility
    pub fn primary_id(&self) -> Option<String> {
        self.imdb_id.clone()
            .or_else(|| self.trakt_id.map(|id| format!("trakt:{}", id)))
            .or_else(|| self.simkl_id.map(|id| format!("simkl:{}", id)))
            .or_else(|| self.tmdb_id.map(|id| format!("tmdb:{}", id)))
            .or_else(|| self.tvdb_id.map(|id| format!("tvdb:{}", id)))
            .or_else(|| self.slug.clone())
    }

    /// Merge IDs from another source, keeping all available IDs
    /// 
    /// This merges IDs from `other` into `self`, only filling in None values.
    /// Existing values are not overwritten.
    pub fn merge(&mut self, other: &MediaIds) {
        if self.imdb_id.is_none() {
            self.imdb_id = other.imdb_id.clone();
        }
        if self.trakt_id.is_none() {
            self.trakt_id = other.trakt_id;
        }
        if self.simkl_id.is_none() {
            self.simkl_id = other.simkl_id;
        }
        if self.tmdb_id.is_none() {
            self.tmdb_id = other.tmdb_id;
        }
        if self.tvdb_id.is_none() {
            self.tvdb_id = other.tvdb_id;
        }
        if self.slug.is_none() {
            self.slug = other.slug.clone();
        }
        if self.plex_rating_key.is_none() {
            self.plex_rating_key = other.plex_rating_key.clone();
        }
        // Merge metadata (title, year, media_type) - prefer existing if present
        if self.title.is_none() {
            self.title = other.title.clone();
        }
        if self.year.is_none() {
            self.year = other.year;
        }
        if self.media_type.is_none() {
            self.media_type = other.media_type.clone();
        }
        // Merge episode metadata
        if self.show_title.is_none() {
            self.show_title = other.show_title.clone();
        }
        if self.episode_title.is_none() {
            self.episode_title = other.episode_title.clone();
        }
        if self.original_air_date.is_none() {
            self.original_air_date = other.original_air_date;
        }
    }

    /// Check if all ID fields are empty
    pub fn is_empty(&self) -> bool {
        self.imdb_id.is_none()
            && self.trakt_id.is_none()
            && self.simkl_id.is_none()
            && self.tmdb_id.is_none()
            && self.tvdb_id.is_none()
            && self.slug.is_none()
    }
    
    /// Set metadata for title-based cache lookups
    pub fn with_metadata(mut self, title: String, year: Option<u32>, media_type: MediaType) -> Self {
        self.title = Some(title);
        self.year = year;
        self.media_type = Some(media_type);
        self
    }

    /// Get the best ID for a specific source
    /// 
    /// Returns the source-specific ID if available, otherwise falls back to imdb_id,
    /// then any other available ID.
    /// 
    /// # Arguments
    /// * `source` - The source name (e.g., "trakt", "simkl", "plex")
    /// 
    /// # Returns
    /// A string representation of the best available ID, or None if no IDs available
    pub fn get_best_id_for_source(&self, source: &str) -> Option<String> {
        match source.to_lowercase().as_str() {
            "trakt" => {
                self.trakt_id.map(|id| format!("trakt:{}", id))
                    .or_else(|| self.imdb_id.clone())
                    .or_else(|| self.get_any_id())
            }
            "simkl" => {
                self.simkl_id.map(|id| format!("simkl:{}", id))
                    .or_else(|| self.imdb_id.clone())
                    .or_else(|| self.get_any_id())
            }
            "tmdb" => {
                self.tmdb_id.map(|id| format!("tmdb:{}", id))
                    .or_else(|| self.imdb_id.clone())
                    .or_else(|| self.get_any_id())
            }
            "tvdb" => {
                self.tvdb_id.map(|id| format!("tvdb:{}", id))
                    .or_else(|| self.imdb_id.clone())
                    .or_else(|| self.get_any_id())
            }
            "plex" => {
                self.plex_rating_key.clone()
                    .or_else(|| self.imdb_id.clone())
                    .or_else(|| self.get_any_id())
            }
            _ => {
                // For unknown sources or general use, prefer imdb_id
                self.imdb_id.clone()
                    .or_else(|| self.get_any_id())
            }
        }
    }

    /// Get any available ID
    /// 
    /// Returns any available ID, preferring imdb_id, then others in order of preference.
    /// Used as fallback when no source-specific preference exists.
    pub fn get_any_id(&self) -> Option<String> {
        self.imdb_id.clone()
            .or_else(|| self.trakt_id.map(|id| format!("trakt:{}", id)))
            .or_else(|| self.simkl_id.map(|id| format!("simkl:{}", id)))
            .or_else(|| self.tmdb_id.map(|id| format!("tmdb:{}", id)))
            .or_else(|| self.tvdb_id.map(|id| format!("tvdb:{}", id)))
            .or_else(|| self.slug.clone())
    }    /// Check if a specific ID type is available
    /// 
    /// # Arguments
    /// * `id_type` - The ID type to check ("imdb", "trakt", "simkl", "tmdb", "tvdb", "slug")
    /// 
    /// # Returns
    /// True if the specified ID type is available
    pub fn has_id(&self, id_type: &str) -> bool {
        match id_type.to_lowercase().as_str() {
            "imdb" => self.imdb_id.is_some(),
            "trakt" => self.trakt_id.is_some(),
            "simkl" => self.simkl_id.is_some(),
            "tmdb" => self.tmdb_id.is_some(),
            "tvdb" => self.tvdb_id.is_some(),
            "slug" => self.slug.is_some(),
            "plex" | "plex_rating_key" => self.plex_rating_key.is_some(),
            _ => false,
        }
    }
}

impl Default for MediaIds {
    fn default() -> Self {
        Self::new()
    }
}

impl Hash for MediaIds {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.imdb_id.hash(state);
        self.trakt_id.hash(state);
        self.simkl_id.hash(state);
        self.tmdb_id.hash(state);
        self.tvdb_id.hash(state);
        self.slug.hash(state);
        self.plex_rating_key.hash(state);
    }
}
