use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Represents an item that was retrieved from a source but excluded from collection
/// (e.g., unsupported media types like music tracks)
/// 
/// The `date_added` field is used for watchlist items excluded by timestamp filters,
/// allowing age-based removal features to work with excluded items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExcludedItem {
    pub title: Option<String>,
    pub imdb_id: Option<String>,
    pub rating_key: Option<String>,
    pub media_type: String,
    pub reason: String,
    pub source: String,
    /// Date when the item was added (used for watchlist items excluded by timestamp filters)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_added: Option<DateTime<Utc>>,
}

