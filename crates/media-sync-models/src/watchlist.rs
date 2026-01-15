use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::status::NormalizedStatus;
use crate::media_ids::MediaIds;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatchlistItem {
    pub imdb_id: String, // Keep for backward compatibility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<MediaIds>, // Normalized IDs from all sources
    pub title: String,
    pub year: Option<u32>,
    pub media_type: crate::media::MediaType,
    pub date_added: DateTime<Utc>,
    pub source: String, // Which source this watchlist item came from
    pub status: Option<NormalizedStatus>, // Normalized status (Watchlist, Watching, Completed, Dropped, Hold)
}

