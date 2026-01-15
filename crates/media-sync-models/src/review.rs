use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::media_ids::MediaIds;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Review {
    pub imdb_id: String, // Keep for backward compatibility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<MediaIds>, // Normalized IDs from all sources
    pub content: String,
    pub date_added: DateTime<Utc>,
    pub media_type: crate::media::MediaType,
    pub source: String, // Which source this review came from
    pub is_spoiler: bool, // Whether this review contains spoilers
}

