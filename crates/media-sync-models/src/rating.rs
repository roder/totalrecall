use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::media_ids::MediaIds;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rating {
    pub imdb_id: String, // Keep for backward compatibility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<MediaIds>, // Normalized IDs from all sources
    pub rating: u8, // Normalized to Trakt format (1-10 integer)
    pub date_added: DateTime<Utc>,
    pub media_type: crate::media::MediaType,
    pub source: RatingSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RatingSource {
    Trakt,   // 1-10 integer
    Imdb,    // 1-10 with 0.5 increments
    Netflix, // TBD (likely 1-5 stars or thumbs)
    Tmdb,    // TBD (likely 1-10 or 1-5)
    Plex,    // 0-10 scale (stored as 1-10, API uses 0-10)
}

