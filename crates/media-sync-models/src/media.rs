use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaItem {
    pub imdb_id: String,
    pub title: String,
    pub year: Option<u32>,
    pub media_type: MediaType,
    pub date_added: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaType {
    Movie,
    Show,
    Episode { season: u32, episode: u32 },
}

