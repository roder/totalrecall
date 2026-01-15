use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::media_ids::MediaIds;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatchHistory {
    pub imdb_id: String, // Keep for backward compatibility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<MediaIds>, // Normalized IDs from all sources
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>, // Title for title-based ID resolution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>, // Year for title-based ID resolution
    pub watched_at: DateTime<Utc>,
    pub media_type: crate::media::MediaType,
    pub source: String, // Which source this watch history came from
}

