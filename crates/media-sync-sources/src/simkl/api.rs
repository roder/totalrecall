use anyhow::{anyhow, Result};
use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem, MediaType};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

// Simkl API base URL
const API_BASE: &str = "https://api.simkl.com";

#[derive(Debug, Serialize, Deserialize)]
struct SimklIds {
    #[serde(rename = "imdb")]
    imdb: Option<String>,
    #[serde(rename = "simkl")]
    simkl: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SimklMovie {
    title: String,
    year: Option<u32>,
    ids: SimklIds,
}

#[derive(Debug, Serialize, Deserialize)]
struct SimklShow {
    title: String,
    year: Option<u32>,
    ids: SimklIds,
}

#[derive(Debug, Serialize, Deserialize)]
struct SimklWatchlistItem {
    #[serde(rename = "added_to_watchlist_at")]
    added_to_watchlist_at: Option<String>,
    #[serde(rename = "last_watched_at")]
    last_watched_at: Option<String>,
    #[serde(rename = "user_rated_at")]
    user_rated_at: Option<String>,
    status: Option<String>,
    #[serde(rename = "user_rating")]
    user_rating: Option<u8>,
    movie: Option<SimklMovie>,
    show: Option<SimklShow>,
    anime: Option<SimklShow>, // Anime uses same structure as show
}

#[derive(Debug, Serialize, Deserialize)]
struct SimklAllItemsResponse {
    shows: Option<Vec<SimklWatchlistItem>>,
    anime: Option<Vec<SimklWatchlistItem>>,
    movies: Option<Vec<SimklWatchlistItem>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SimklRatingItem {
    #[serde(rename = "last_watched_at")]
    last_watched_at: Option<String>,
    #[serde(rename = "user_rated_at")]
    user_rated_at: Option<String>,
    #[serde(rename = "user_rating")]
    user_rating: u8,
    status: Option<String>,
    movie: Option<SimklMovie>,
    show: Option<SimklShow>,
    anime: Option<SimklShow>, // Anime uses same structure as show
}

#[derive(Debug, Serialize, Deserialize)]
struct SimklRatingsResponse {
    shows: Option<Vec<SimklRatingItem>>,
    anime: Option<Vec<SimklRatingItem>>,
    movies: Option<Vec<SimklRatingItem>>,
}

// History items use the same structure as watchlist items
// They're identified by having last_watched_at field set

#[derive(Debug, Serialize, Deserialize)]
pub struct SimklActivities {
    #[serde(rename = "all")]
    pub all: Option<String>,
    pub settings: Option<SimklSettingsActivities>,
    pub tv_shows: Option<SimklMediaActivities>,
    pub anime: Option<SimklMediaActivities>,
    pub movies: Option<SimklMediaActivities>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SimklSettingsActivities {
    #[serde(rename = "all")]
    pub all: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SimklMediaActivities {
    #[serde(rename = "all")]
    pub all: Option<String>,
    #[serde(rename = "rated_at")]
    pub rated_at: Option<String>,
    pub playback: Option<String>,
    pub plantowatch: Option<String>,
    pub watching: Option<String>,
    pub completed: Option<String>,
    pub hold: Option<String>,
    pub dropped: Option<String>,
    #[serde(rename = "removed_from_list")]
    pub removed_from_list: Option<String>,
}

/// Remove slashes from IMDB ID (if present)
fn remove_slashes(s: Option<String>) -> String {
    s.unwrap_or_default().replace('/', "")
}

/// Extract MediaIds from SimklIds
fn extract_media_ids_from_simkl_ids(simkl_ids: &SimklIds) -> media_sync_models::MediaIds {
    use media_sync_models::MediaIds;
    
    let mut media_ids = MediaIds::default();
    media_ids.imdb_id = simkl_ids.imdb.as_ref().map(|s| remove_slashes(Some(s.clone())));
    media_ids.simkl_id = simkl_ids.simkl;
    
    media_ids
}

/// Get last activities from Simkl
pub async fn get_activities(
    client: &Client,
    access_token: &str,
    client_id: &str,
) -> Result<SimklActivities> {
    let url = format!("{}/sync/activities", API_BASE);
    
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("simkl-api-key", client_id)
        .header("Accept", "application/json")
        .send()
        .await?;
    
    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to fetch activities: {} - {}", status, error_text));
    }
    
    let activities: SimklActivities = response.json().await?;
    Ok(activities)
}

/// Fetch watchlist from Simkl
pub async fn get_watchlist(
    client: &Client,
    access_token: &str,
    client_id: &str,
    date_from: Option<DateTime<Utc>>,
    status_mapping: &std::collections::HashMap<String, media_sync_models::NormalizedStatus>,
) -> Result<Vec<WatchlistItem>> {
    let mut url = format!("{}/sync/all-items/", API_BASE);
    
    if let Some(date) = date_from {
        url.push_str(&format!("?date_from={}", date.to_rfc3339()));
    }

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("simkl-api-key", client_id)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to fetch watchlist: {} - {}", status, error_text));
    }

    let response_data: SimklAllItemsResponse = response.json().await?;

    let mut watchlist = Vec::new();

    // Process shows
    if let Some(shows) = response_data.shows {
        for item in shows {
            if let Some(show) = item.show {
                let imdb_id = remove_slashes(show.ids.imdb.clone());
                
                // Extract MediaIds
                let media_ids = extract_media_ids_from_simkl_ids(&show.ids);
                
                // Don't skip items if they have any IDs (not just imdb_id)
                if media_ids.is_empty() {
                    continue;
                }

                let date_added = item.added_to_watchlist_at
                    .as_ref()
                    .and_then(|d| DateTime::parse_from_rfc3339(d).ok().map(|dt| dt.with_timezone(&Utc)))
                    .or_else(|| {
                        item.added_to_watchlist_at.as_ref()
                            .and_then(|d| DateTime::parse_from_str(d, "%Y-%m-%d %H:%M:%S").ok().map(|dt| dt.with_timezone(&Utc)))
                    })
                    .unwrap_or_else(|| Utc::now());

                // Map Simkl status to normalized status
                let normalized_status = item.status
                    .as_ref()
                    .and_then(|s| status_mapping.get(s))
                    .cloned();

                watchlist.push(WatchlistItem {
                    imdb_id,
                    ids: Some(media_ids),
                    title: show.title,
                    year: show.year,
                    media_type: MediaType::Show,
                    date_added,
                    source: "simkl".to_string(),
                    status: normalized_status,
                });
            }
        }
    }

    // Process anime
    if let Some(anime) = response_data.anime {
        for item in anime {
            if let Some(show) = item.show {
                let imdb_id = remove_slashes(show.ids.imdb.clone());
                
                // Extract MediaIds
                let media_ids = extract_media_ids_from_simkl_ids(&show.ids);
                
                // Don't skip items if they have any IDs (not just imdb_id)
                if media_ids.is_empty() {
                    continue;
                }

                let date_added = item.added_to_watchlist_at
                    .as_ref()
                    .and_then(|d| DateTime::parse_from_rfc3339(d).ok().map(|dt| dt.with_timezone(&Utc)))
                    .or_else(|| {
                        item.added_to_watchlist_at.as_ref()
                            .and_then(|d| DateTime::parse_from_str(d, "%Y-%m-%d %H:%M:%S").ok().map(|dt| dt.with_timezone(&Utc)))
                    })
                    .unwrap_or_else(|| Utc::now());

                // Map Simkl status to normalized status
                let normalized_status = item.status
                    .as_ref()
                    .and_then(|s| status_mapping.get(s))
                    .cloned();

                watchlist.push(WatchlistItem {
                    imdb_id,
                    ids: Some(media_ids),
                    title: show.title,
                    year: show.year,
                    media_type: MediaType::Show,
                    date_added,
                    source: "simkl".to_string(),
                    status: normalized_status,
                });
            }
        }
    }

    // Process movies
    if let Some(movies) = response_data.movies {
        for item in movies {
            if let Some(movie) = item.movie {
                let imdb_id = remove_slashes(movie.ids.imdb.clone());
                
                // Extract MediaIds
                let media_ids = extract_media_ids_from_simkl_ids(&movie.ids);
                
                // Don't skip items if they have any IDs (not just imdb_id)
                if media_ids.is_empty() {
                    continue;
                }

                let date_added = item.added_to_watchlist_at
                    .as_ref()
                    .and_then(|d| DateTime::parse_from_rfc3339(d).ok().map(|dt| dt.with_timezone(&Utc)))
                    .or_else(|| {
                        item.added_to_watchlist_at.as_ref()
                            .and_then(|d| DateTime::parse_from_str(d, "%Y-%m-%d %H:%M:%S").ok().map(|dt| dt.with_timezone(&Utc)))
                    })
                    .unwrap_or_else(|| Utc::now());

                // Map Simkl status to normalized status
                let normalized_status = item.status
                    .as_ref()
                    .and_then(|s| status_mapping.get(s))
                    .cloned();

                watchlist.push(WatchlistItem {
                    imdb_id,
                    ids: Some(media_ids),
                    title: movie.title,
                    year: movie.year,
                    media_type: MediaType::Movie,
                    date_added,
                    source: "simkl".to_string(),
                    status: normalized_status,
                });
            }
        }
    }

    Ok(watchlist)
}

/// Fetch ratings from Simkl
pub async fn get_ratings(
    client: &Client,
    access_token: &str,
    client_id: &str,
    date_from: Option<DateTime<Utc>>,
) -> Result<Vec<Rating>> {
    // Simkl ratings endpoint is POST /sync/ratings/ (no type/rating filters for all ratings)
    let mut url = format!("{}/sync/ratings/", API_BASE);
    
    if let Some(date) = date_from {
        url.push_str(&format!("?date_from={}", date.to_rfc3339()));
    }

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("simkl-api-key", client_id)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to fetch ratings: {} - {}", status, error_text));
    }

    let response_data: SimklRatingsResponse = response.json().await?;

    let mut ratings = Vec::new();

    // Process shows
    if let Some(shows) = response_data.shows {
        for item in shows {
            if let Some(show) = item.show {
                let imdb_id = remove_slashes(show.ids.imdb.clone());
                
                // Extract MediaIds
                let media_ids = extract_media_ids_from_simkl_ids(&show.ids);
                
                // Don't skip items if they have any IDs (not just imdb_id)
                if media_ids.is_empty() {
                    continue;
                }

                let date_added = item.user_rated_at
                    .as_ref()
                    .and_then(|d| DateTime::parse_from_rfc3339(d).ok().map(|dt| dt.with_timezone(&Utc)))
                    .or_else(|| {
                        item.user_rated_at.as_ref()
                            .and_then(|d| DateTime::parse_from_str(d, "%Y-%m-%d %H:%M:%S").ok().map(|dt| dt.with_timezone(&Utc)))
                    })
                    .unwrap_or_else(|| Utc::now());

                ratings.push(Rating {
                    imdb_id,
                    ids: Some(media_ids),
                    rating: item.user_rating,
                    date_added,
                    media_type: MediaType::Show,
                    source: media_sync_models::RatingSource::Trakt, // Simkl uses same 1-10 scale
                });
            }
        }
    }

    // Process anime
    if let Some(anime) = response_data.anime {
        for item in anime {
            if let Some(show) = item.show {
                let imdb_id = remove_slashes(show.ids.imdb.clone());
                
                // Extract MediaIds
                let media_ids = extract_media_ids_from_simkl_ids(&show.ids);
                
                // Don't skip items if they have any IDs (not just imdb_id)
                if media_ids.is_empty() {
                    continue;
                }

                let date_added = item.user_rated_at
                    .as_ref()
                    .and_then(|d| DateTime::parse_from_rfc3339(d).ok().map(|dt| dt.with_timezone(&Utc)))
                    .or_else(|| {
                        item.user_rated_at.as_ref()
                            .and_then(|d| DateTime::parse_from_str(d, "%Y-%m-%d %H:%M:%S").ok().map(|dt| dt.with_timezone(&Utc)))
                    })
                    .unwrap_or_else(|| Utc::now());

                ratings.push(Rating {
                    imdb_id,
                    ids: Some(media_ids),
                    rating: item.user_rating,
                    date_added,
                    media_type: MediaType::Show,
                    source: media_sync_models::RatingSource::Trakt,
                });
            }
        }
    }

    // Process movies
    if let Some(movies) = response_data.movies {
        for item in movies {
            if let Some(movie) = item.movie {
                let imdb_id = remove_slashes(movie.ids.imdb.clone());
                
                // Extract MediaIds
                let media_ids = extract_media_ids_from_simkl_ids(&movie.ids);
                
                // Don't skip items if they have any IDs (not just imdb_id)
                if media_ids.is_empty() {
                    continue;
                }

                let date_added = item.user_rated_at
                    .as_ref()
                    .and_then(|d| DateTime::parse_from_rfc3339(d).ok().map(|dt| dt.with_timezone(&Utc)))
                    .or_else(|| {
                        item.user_rated_at.as_ref()
                            .and_then(|d| DateTime::parse_from_str(d, "%Y-%m-%d %H:%M:%S").ok().map(|dt| dt.with_timezone(&Utc)))
                    })
                    .unwrap_or_else(|| Utc::now());

                ratings.push(Rating {
                    imdb_id,
                    ids: Some(media_ids),
                    rating: item.user_rating,
                    date_added,
                    media_type: MediaType::Movie,
                    source: media_sync_models::RatingSource::Trakt,
                });
            }
        }
    }

    Ok(ratings)
}

/// Fetch reviews from Simkl
pub async fn get_reviews(
    _client: &Client,
    _access_token: &str,
    _client_id: &str,
) -> Result<Vec<Review>> {
    // Simkl may not have a reviews/comments API endpoint
    // Return empty for now - can be implemented if Simkl adds this feature
    Ok(Vec::new())
}

/// Fetch watch history from Simkl
pub async fn get_watch_history(
    client: &Client,
    access_token: &str,
    client_id: &str,
    date_from: Option<DateTime<Utc>>,
) -> Result<Vec<WatchHistory>> {
    // Watch history is items from /sync/all-items/ that have last_watched_at set
    let mut url = format!("{}/sync/all-items/", API_BASE);
    
    if let Some(date) = date_from {
        url.push_str(&format!("?date_from={}", date.to_rfc3339()));
    }

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("simkl-api-key", client_id)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to fetch watch history: {} - {}", status, error_text));
    }

    let response_data: SimklAllItemsResponse = response.json().await?;

    let mut history = Vec::new();

    // Process shows with last_watched_at
    if let Some(shows) = response_data.shows {
        for item in shows {
            if let Some(last_watched_at) = item.last_watched_at {
                if let Some(show) = item.show {
                    let imdb_id = remove_slashes(show.ids.imdb.clone());
                    
                    // Extract MediaIds
                    let media_ids = extract_media_ids_from_simkl_ids(&show.ids);
                    
                    // Don't skip items if they have any IDs (not just imdb_id)
                    if media_ids.is_empty() {
                        continue;
                    }

                    let watched_at = DateTime::parse_from_rfc3339(&last_watched_at)
                        .map(|dt| dt.with_timezone(&Utc))
                        .or_else(|_| {
                            DateTime::parse_from_str(&last_watched_at, "%Y-%m-%d %H:%M:%S")
                                .map(|dt| dt.with_timezone(&Utc))
                        })
                        .unwrap_or_else(|_| Utc::now());

                    history.push(WatchHistory {
                        imdb_id,
                        ids: Some(media_ids),
                        title: None,
                        year: None,
                        watched_at,
                        media_type: MediaType::Show,
                        source: "simkl".to_string(),
                    });
                }
            }
        }
    }

    // Process anime with last_watched_at
    if let Some(anime) = response_data.anime {
        for item in anime {
            if let Some(last_watched_at) = item.last_watched_at {
                if let Some(show) = item.show {
                    let imdb_id = remove_slashes(show.ids.imdb.clone());
                    
                    // Extract MediaIds
                    let media_ids = extract_media_ids_from_simkl_ids(&show.ids);
                    
                    // Don't skip items if they have any IDs (not just imdb_id)
                    if media_ids.is_empty() {
                        continue;
                    }

                    let watched_at = DateTime::parse_from_rfc3339(&last_watched_at)
                        .map(|dt| dt.with_timezone(&Utc))
                        .or_else(|_| {
                            DateTime::parse_from_str(&last_watched_at, "%Y-%m-%d %H:%M:%S")
                                .map(|dt| dt.with_timezone(&Utc))
                        })
                        .unwrap_or_else(|_| Utc::now());

                    history.push(WatchHistory {
                        imdb_id,
                        ids: Some(media_ids),
                        title: None,
                        year: None,
                        watched_at,
                        media_type: MediaType::Show,
                        source: "simkl".to_string(),
                    });
                }
            }
        }
    }

    // Process movies with last_watched_at
    if let Some(movies) = response_data.movies {
        for item in movies {
            if let Some(last_watched_at) = item.last_watched_at {
                if let Some(movie) = item.movie {
                    let imdb_id = remove_slashes(movie.ids.imdb.clone());
                    
                    // Extract MediaIds
                    let media_ids = extract_media_ids_from_simkl_ids(&movie.ids);
                    
                    // Don't skip items if they have any IDs (not just imdb_id)
                    if media_ids.is_empty() {
                        continue;
                    }

                    let watched_at = DateTime::parse_from_rfc3339(&last_watched_at)
                        .map(|dt| dt.with_timezone(&Utc))
                        .or_else(|_| {
                            DateTime::parse_from_str(&last_watched_at, "%Y-%m-%d %H:%M:%S")
                                .map(|dt| dt.with_timezone(&Utc))
                        })
                        .unwrap_or_else(|_| Utc::now());

                    history.push(WatchHistory {
                        imdb_id,
                        ids: Some(media_ids),
                        title: None,
                        year: None,
                        watched_at,
                        media_type: MediaType::Movie,
                        source: "simkl".to_string(),
                    });
                }
            }
        }
    }

    Ok(history)
}

/// Add items to Simkl watchlist
pub async fn add_to_watchlist(
    client: &Client,
    access_token: &str,
    client_id: &str,
    items: &[WatchlistItem],
    status_mapping: &std::collections::HashMap<media_sync_models::NormalizedStatus, String>,
) -> Result<()> {
    let mut movies = Vec::new();
    let mut shows = Vec::new();

    for item in items {
        // Map normalized status to Simkl status, default to "watching"
        let simkl_status = item.status
            .as_ref()
            .and_then(|s| status_mapping.get(s))
            .cloned()
            .unwrap_or_else(|| "watching".to_string());

        // Build IDs object with all available IDs from MediaIds
        let mut ids_obj = serde_json::Map::new();
        
        // Use MediaIds if available, otherwise fall back to imdb_id
        if let Some(ref media_ids) = item.ids {
            if let Some(ref imdb) = media_ids.imdb_id {
                ids_obj.insert("imdb".to_string(), serde_json::Value::String(imdb.clone()));
            }
            if let Some(simkl) = media_ids.simkl_id {
                ids_obj.insert("simkl".to_string(), serde_json::Value::Number(simkl.into()));
            }
        } else {
            // Fallback to imdb_id if MediaIds not available
            ids_obj.insert("imdb".to_string(), serde_json::Value::String(item.imdb_id.clone()));
        }
        
        let mut item_obj = serde_json::json!({
            "ids": ids_obj,
            "to": simkl_status
        });

        // Add title and year if available for better matching
        if !item.title.is_empty() {
            item_obj["title"] = serde_json::json!(item.title);
        }
        if let Some(year) = item.year {
            item_obj["year"] = serde_json::json!(year);
        }

        match &item.media_type {
            MediaType::Movie => movies.push(item_obj),
            MediaType::Show => shows.push(item_obj),
            MediaType::Episode { .. } => {
                // Simkl may not support episodes in watchlist
                continue;
            }
        }
    }

    let payload = serde_json::json!({
        "movies": movies,
        "shows": shows
    });

    let response = client
        .post(format!("{}/sync/add-to-list", API_BASE))
        .header("Authorization", format!("Bearer {}", access_token))
        .header("simkl-api-key", client_id)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to add to watchlist: {} - {}", status, error_text));
    }

    Ok(())
}

/// Remove items from Simkl watchlist
pub async fn remove_from_watchlist(
    client: &Client,
    access_token: &str,
    client_id: &str,
    items: &[WatchlistItem],
) -> Result<()> {
    let mut movies = Vec::new();
    let mut shows = Vec::new();

    for item in items {
        let mut item_obj = serde_json::json!({
            "ids": {
                "imdb": item.imdb_id
            }
        });

        // Add title and year if available for better matching
        if !item.title.is_empty() {
            item_obj["title"] = serde_json::json!(item.title);
        }
        if let Some(year) = item.year {
            item_obj["year"] = serde_json::json!(year);
        }

        match &item.media_type {
            MediaType::Movie => movies.push(item_obj),
            MediaType::Show => shows.push(item_obj),
            MediaType::Episode { .. } => continue,
        }
    }

    let payload = serde_json::json!({
        "movies": movies,
        "shows": shows
    });

    let response = client
        .post(format!("{}/sync/history/remove", API_BASE))
        .header("Authorization", format!("Bearer {}", access_token))
        .header("simkl-api-key", client_id)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to remove from watchlist: {} - {}", status, error_text));
    }

    Ok(())
}

/// Set ratings on Simkl
pub async fn set_ratings(
    client: &Client,
    access_token: &str,
    client_id: &str,
    ratings: &[Rating],
) -> Result<()> {
    let mut movies = Vec::new();
    let mut shows = Vec::new();

    for rating in ratings {
        // Build IDs object with all available IDs from MediaIds
        let mut ids_obj = serde_json::Map::new();
        
        // Use MediaIds if available, otherwise fall back to imdb_id
        if let Some(ref media_ids) = rating.ids {
            if let Some(ref imdb) = media_ids.imdb_id {
                ids_obj.insert("imdb".to_string(), serde_json::Value::String(imdb.clone()));
            }
            if let Some(simkl) = media_ids.simkl_id {
                ids_obj.insert("simkl".to_string(), serde_json::Value::Number(simkl.into()));
            }
        } else {
            // Fallback to imdb_id if MediaIds not available
            ids_obj.insert("imdb".to_string(), serde_json::Value::String(rating.imdb_id.clone()));
        }
        
        let mut item = serde_json::json!({
            "ids": ids_obj,
            "rating": rating.rating
        });

        // Add rated_at if available
        item["rated_at"] = serde_json::json!(rating.date_added.to_rfc3339());

        match &rating.media_type {
            MediaType::Movie => movies.push(item),
            MediaType::Show => shows.push(item),
            MediaType::Episode { .. } => continue,
        }
    }

    let payload = serde_json::json!({
        "movies": movies,
        "shows": shows
    });

    let response = client
        .post(format!("{}/sync/ratings", API_BASE))
        .header("Authorization", format!("Bearer {}", access_token))
        .header("simkl-api-key", client_id)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to set ratings: {} - {}", status, error_text));
    }

    Ok(())
}

/// Set reviews on Simkl (if supported)
pub async fn set_reviews(
    _client: &Client,
    _access_token: &str,
    _client_id: &str,
    _reviews: &[Review],
) -> Result<()> {
    // Simkl may not have a reviews/comments API endpoint
    // Return Ok for now - can be implemented if Simkl adds this feature
    Ok(())
}

/// Add watch history to Simkl
pub async fn add_watch_history(
    client: &Client,
    access_token: &str,
    client_id: &str,
    items: &[WatchHistory],
) -> Result<()> {
    let mut movies = Vec::new();
    let mut shows = Vec::new();

    for item in items {
        let mut item_obj = serde_json::json!({
            "ids": {
                "imdb": item.imdb_id
            },
            "watched_at": item.watched_at.to_rfc3339()
        });

        match &item.media_type {
            MediaType::Movie => movies.push(item_obj),
            MediaType::Show => shows.push(item_obj),
            MediaType::Episode { .. } => continue,
        }
    }

    let payload = serde_json::json!({
        "movies": movies,
        "shows": shows
    });

    let response = client
        .post(format!("{}/sync/history", API_BASE))
        .header("Authorization", format!("Bearer {}", access_token))
        .header("simkl-api-key", client_id)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to add watch history: {} - {}", status, error_text));
    }

    Ok(())
}

/// Search for media by title using Simkl API
/// Note: Simkl may not have a public search API, so this may return None
pub async fn search_by_title(
    _client: &Client,
    _access_token: &str,
    _client_id: &str,
    title: &str,
    year: Option<u32>,
    _media_type: &MediaType,
) -> Result<Option<media_sync_models::MediaIds>> {
    use tracing::debug;
    
    // Simkl doesn't appear to have a public search API endpoint
    // Return None for now - this can be implemented if Simkl adds search support
    debug!("Simkl search: Search not implemented for '{}' (year: {:?}) - Simkl API does not provide a public search endpoint", title, year);
    Ok(None)
}

