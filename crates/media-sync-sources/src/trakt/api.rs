use anyhow::{anyhow, Result};
use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem, MediaType};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktIds {
    #[serde(rename = "imdb")]
    pub imdb: Option<String>,
    #[serde(rename = "trakt")]
    pub trakt: Option<u64>,
    #[serde(rename = "tmdb")]
    pub tmdb: Option<u32>,
    #[serde(rename = "tvdb")]
    pub tvdb: Option<u32>,
    #[serde(rename = "slug")]
    pub slug: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TraktMovie {
    title: String,
    year: Option<u32>,
    ids: TraktIds,
}

#[derive(Debug, Serialize, Deserialize)]
struct TraktShow {
    title: String,
    year: Option<u32>,
    ids: TraktIds,
    status: Option<String>,
    #[serde(rename = "aired_episodes")]
    aired_episodes: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TraktEpisode {
    title: String,
    year: Option<u32>,
    season: Option<u32>,
    number: Option<u32>,
    ids: TraktIds,
    #[serde(rename = "first_aired")]
    first_aired: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TraktWatchlistItem {
    #[serde(rename = "listed_at")]
    listed_at: String,
    #[serde(rename = "type")]
    item_type: String,
    movie: Option<TraktMovie>,
    show: Option<TraktShow>,
    episode: Option<TraktEpisode>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TraktRatingItem {
    #[serde(rename = "rated_at")]
    rated_at: String,
    rating: u8,
    #[serde(rename = "type")]
    item_type: String,
    movie: Option<TraktMovie>,
    show: Option<TraktShow>,
    episode: Option<TraktEpisode>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TraktComment {
    #[serde(rename = "type")]
    item_type: String,
    movie: Option<TraktMovie>,
    show: Option<TraktShow>,
    episode: Option<TraktEpisode>,
    comment: TraktCommentDetails,
    #[serde(default)]
    spoiler: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct TraktCommentDetails {
    id: u64,
    comment: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TraktHistoryItem {
    #[serde(rename = "watched_at")]
    watched_at: String,
    #[serde(rename = "type")]
    item_type: String,
    movie: Option<TraktMovie>,
    show: Option<TraktShow>,
    episode: Option<TraktEpisode>,
}

/// Remove slashes from IMDB ID (Trakt sometimes includes them)
fn remove_slashes(s: Option<String>) -> String {
    s.unwrap_or_default().replace('/', "")
}

/// Extract MediaIds from TraktIds
fn extract_media_ids_from_trakt_ids(trakt_ids: &TraktIds) -> media_sync_models::MediaIds {
    use media_sync_models::MediaIds;
    
    let mut media_ids = MediaIds::default();
    media_ids.imdb_id = trakt_ids.imdb.as_ref().map(|s| remove_slashes(Some(s.clone())));
    media_ids.trakt_id = trakt_ids.trakt;
    media_ids.tmdb_id = trakt_ids.tmdb;
    media_ids.tvdb_id = trakt_ids.tvdb;
    media_ids.slug = trakt_ids.slug.clone();
    
    media_ids
}

/// Get encoded username from Trakt API
pub async fn get_encoded_username(client: &Client, access_token: &str, client_id: &str) -> Result<String> {
    let response = client
        .get("https://api.trakt.tv/users/me")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id) // Required for authenticated requests
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .header("Origin", "https://trakt.tv")
        .header("Referer", "https://trakt.tv/")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to get user info: {} - {}", status, error_text));
    }

    let json: serde_json::Value = response.json().await?;
    let username_slug = json["ids"]["slug"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing username slug"))?;

    Ok(urlencoding::encode(username_slug).to_string())
}

/// Fetch watchlist from Trakt
pub async fn get_watchlist(
    client: &Client,
    access_token: &str,
    encoded_username: &str,
    client_id: &str,
) -> Result<Vec<WatchlistItem>> {
    let url = format!(
        "https://api.trakt.tv/users/{}/watchlist?sort=added,asc",
        encoded_username
    );

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id) // Required for authenticated requests
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .header("Origin", "https://trakt.tv")
        .header("Referer", "https://trakt.tv/")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to fetch watchlist: {} - {}", status, error_text));
    }

    let items: Vec<TraktWatchlistItem> = response.json().await?;
    let mut watchlist = Vec::new();

    for item in items {
        let (trakt_ids, imdb_id, title, year, media_type) = match item.item_type.as_str() {
            "movie" => {
                let movie = item.movie.ok_or_else(|| anyhow!("Missing movie data"))?;
                (
                    movie.ids.clone(),
                    remove_slashes(movie.ids.imdb.clone()),
                    movie.title,
                    movie.year,
                    MediaType::Movie,
                )
            }
            "show" => {
                let show = item.show.ok_or_else(|| anyhow!("Missing show data"))?;
                (
                    show.ids.clone(),
                    remove_slashes(show.ids.imdb.clone()),
                    show.title,
                    show.year,
                    MediaType::Show,
                )
            }
            "episode" => {
                let episode = item.episode.ok_or_else(|| anyhow!("Missing episode data"))?;
                let show = item.show.ok_or_else(|| anyhow!("Missing show data for episode"))?;
                (
                    episode.ids.clone(),
                    remove_slashes(episode.ids.imdb.clone()),
                    format!("{}: {}", show.title, episode.title),
                    episode.year,
                    MediaType::Episode {
                        season: episode.season.unwrap_or(0),
                        episode: episode.number.unwrap_or(0),
                    },
                )
            }
            _ => continue,
        };

        // Extract MediaIds from TraktIds
        let media_ids = extract_media_ids_from_trakt_ids(&trakt_ids);
        
        // Don't skip items if they have any IDs (not just imdb_id)
        if media_ids.is_empty() {
            continue;
        }

        let date_added = DateTime::parse_from_rfc3339(&item.listed_at)
            .map_err(|e| anyhow!("Failed to parse date: {}", e))?
            .with_timezone(&Utc);

        watchlist.push(WatchlistItem {
            imdb_id: imdb_id.clone(),
            ids: Some(media_ids),
            title,
            year,
            media_type,
            date_added,
            source: "trakt".to_string(),
            status: Some(media_sync_models::NormalizedStatus::Watchlist), // Trakt watchlist items are always "Watchlist" status
        });
    }

    Ok(watchlist)
}

/// Fetch ratings from Trakt
pub async fn get_ratings(
    client: &Client,
    access_token: &str,
    encoded_username: &str,
    client_id: &str,
) -> Result<Vec<Rating>> {
    
    let url = format!(
        "https://api.trakt.tv/users/{}/ratings?sort=newest",
        encoded_username
    );

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id) // Required for authenticated requests
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .header("Origin", "https://trakt.tv")
        .header("Referer", "https://trakt.tv/")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to fetch ratings: {} - {}", status, error_text));
    }

    let items: Vec<TraktRatingItem> = response.json().await?;
    let mut ratings = Vec::new();
    let mut items_with_empty_imdb = 0;

    for item in items {
        let (trakt_ids, imdb_id, title, year, media_type) = match item.item_type.as_str() {
            "movie" => {
                let movie = item.movie.ok_or_else(|| anyhow!("Missing movie data"))?;
                (
                    movie.ids.clone(),
                    remove_slashes(movie.ids.imdb.clone()),
                    movie.title,
                    movie.year,
                    MediaType::Movie,
                )
            }
            "show" => {
                let show = item.show.ok_or_else(|| anyhow!("Missing show data"))?;
                (
                    show.ids.clone(),
                    remove_slashes(show.ids.imdb.clone()),
                    show.title,
                    show.year,
                    MediaType::Show,
                )
            }
            "episode" => {
                let episode = item.episode.ok_or_else(|| anyhow!("Missing episode data"))?;
                let show = item.show.ok_or_else(|| anyhow!("Missing show data for episode"))?;
                (
                    episode.ids.clone(),
                    remove_slashes(episode.ids.imdb.clone()),
                    format!("{}: {}", show.title, episode.title),
                    episode.year,
                    MediaType::Episode {
                        season: episode.season.unwrap_or(0),
                        episode: episode.number.unwrap_or(0),
                    },
                )
            }
            _ => continue,
        };

        // Extract MediaIds from TraktIds
        let media_ids = extract_media_ids_from_trakt_ids(&trakt_ids);
        
        // Don't skip items if they have any IDs (not just imdb_id)
        if media_ids.is_empty() {
            items_with_empty_imdb += 1;
            // Log first few items with empty IDs
            if items_with_empty_imdb <= 5 {
                debug!(
                    "Trakt ratings: Skipping item with no IDs, type={:?}, title={}",
                    media_type,
                    title
                );
            }
            continue;
        }

        let date_added = DateTime::parse_from_rfc3339(&item.rated_at)
            .map_err(|e| anyhow!("Failed to parse date: {}", e))?
            .with_timezone(&Utc);

        // Clone media_type for logging before moving it
        let media_type_for_log = if ratings.len() < 5 { Some(media_type.clone()) } else { None };

        // Extract MediaIds
        let media_ids = extract_media_ids_from_trakt_ids(&trakt_ids);
        
        ratings.push(Rating {
            imdb_id: imdb_id.clone(),
            ids: Some(media_ids),
            rating: item.rating,
            date_added,
            media_type,
            source: media_sync_models::RatingSource::Trakt,
        });
        
        // Log first few ratings being added
        if let Some(ref mt) = media_type_for_log {
            debug!(
                "Trakt rating[{}]: imdb_id={}, rating={}, date_added={}, media_type={:?}",
                ratings.len() - 1,
                imdb_id,
                item.rating,
                date_added,
                mt
            );
        }
    }

    debug!(
        "Fetched Trakt ratings: total_items={}, items_with_empty_imdb={}",
        ratings.len(),
        items_with_empty_imdb
    );

    Ok(ratings)
}

/// Fetch comments/reviews from Trakt with pagination
pub async fn get_comments(
    client: &Client,
    access_token: &str,
    encoded_username: &str,
    client_id: &str,
) -> Result<Vec<Review>> {
    use tracing::{debug, warn};
    
    let mut all_comments = Vec::new();
    let mut page = 1;
    let mut items_with_empty_imdb = 0;
    let mut items_with_unknown_type = 0;

    loop {
        // According to Trakt API docs: /users/{username}/comments endpoint
        // The 'type' parameter filters comment types: 'all', 'reviews', 'shouts', 'lists'
        // Try 'reviews' first to get only reviews, fallback to 'all' if needed
        let url = format!(
            "https://api.trakt.tv/users/{}/comments?sort=newest&page={}&type=reviews",
            encoded_username, page
        );

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("trakt-api-version", "2")
            .header("trakt-api-key", client_id)
            .header("Accept", "application/json")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Content-Type", "application/json")
            .header("Origin", "https://trakt.tv")
            .header("Referer", "https://trakt.tv/")
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            warn!("Trakt comments API error: {} - {}. URL: {}", status, error_text, url);
            return Err(anyhow!("Failed to fetch comments: {} - {}", status, error_text));
        }

        let total_pages: u32 = response
            .headers()
            .get("X-Pagination-Page-Count")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        // Try to parse as JSON, but capture raw text if parsing fails for debugging
        let response_text = response.text().await?;
        let items: Vec<TraktComment> = match serde_json::from_str(&response_text) {
            Ok(items) => items,
            Err(e) => {
                warn!(
                    "Failed to parse Trakt comments API response as JSON: {}. Response length: {}, URL: {}",
                    e, response_text.len(), url
                );
                if response_text.len() < 500 {
                    debug!("Trakt comments API raw response: {}", response_text);
                }
                return Err(anyhow!("Failed to parse comments response: {}", e));
            }
        };
        
        debug!(
            "Trakt comments API: page={}, total_pages={}, items_on_page={}, url={}",
            page, total_pages, items.len(), url
        );
        
        // Log raw response if empty to debug
        if items.is_empty() && page == 1 {
            warn!(
                "Trakt comments API returned empty result on first page (type=reviews). URL: {}. Response length: {}. This might indicate: 1) No reviews exist, 2) API delay in indexing new comments (newly added reviews may take a few minutes to appear - this can cause duplicates if syncing again too soon), 3) type parameter issue, or 4) Authentication/permission issue",
                url, response_text.len()
            );
            if response_text.len() < 200 {
                debug!("Trakt comments API empty response body: {}", response_text);
            }
        }
        
        // Log sample of item types if we have items
        if !items.is_empty() && page == 1 {
            let item_types: Vec<&String> = items.iter().map(|i| &i.item_type).take(5).collect();
            debug!("Trakt comments API: sample item types from first page: {:?}", item_types);
        }

        for item in items {
            let (trakt_ids, imdb_id, _title, _year, media_type) = match item.item_type.as_str() {
                "movie" => {
                    let movie = item.movie.ok_or_else(|| anyhow!("Missing movie data"))?;
                    (
                        movie.ids.clone(),
                        remove_slashes(movie.ids.imdb.clone()),
                        movie.title,
                        movie.year,
                        MediaType::Movie,
                    )
                }
                "show" => {
                    let show = item.show.ok_or_else(|| anyhow!("Missing show data"))?;
                    (
                        show.ids.clone(),
                        remove_slashes(show.ids.imdb.clone()),
                        show.title,
                        show.year,
                        MediaType::Show,
                    )
                }
                "episode" => {
                    let episode = item.episode.ok_or_else(|| anyhow!("Missing episode data"))?;
                    let show = item.show.ok_or_else(|| anyhow!("Missing show data for episode"))?;
                    (
                        episode.ids.clone(),
                        remove_slashes(episode.ids.imdb.clone()),
                        format!("{}: {}", show.title, episode.title),
                        show.year,
                        MediaType::Episode {
                            season: episode.season.unwrap_or(0),
                            episode: episode.number.unwrap_or(0),
                        },
                    )
                }
                _ => {
                    items_with_unknown_type += 1;
                    if items_with_unknown_type <= 5 {
                        debug!(
                            "Trakt comments: Skipping item with unknown type: {:?}",
                            item.item_type
                        );
                    }
                    continue;
                }
            };

            // Extract MediaIds
            let media_ids = extract_media_ids_from_trakt_ids(&trakt_ids);
            
            // Don't skip items if they have any IDs (not just imdb_id)
            if media_ids.is_empty() {
                items_with_empty_imdb += 1;
                if items_with_empty_imdb <= 5 {
                    debug!(
                        "Trakt comments: Skipping item with no IDs, type={:?}",
                        media_type
                    );
                }
                continue;
            }

            // Use current time as date_added since Trakt comments don't have a creation date in this endpoint
            all_comments.push(Review {
                imdb_id: imdb_id.clone(),
                ids: Some(media_ids),
                content: item.comment.comment.clone(),
                date_added: Utc::now(),
                media_type: media_type.clone(),
                source: "trakt".to_string(),
                is_spoiler: item.spoiler,
            });
            
            if all_comments.len() <= 5 {
                debug!(
                    "Trakt comment[{}]: imdb_id={}, content_length={}, media_type={:?}, is_spoiler={}",
                    all_comments.len() - 1,
                    imdb_id,
                    item.comment.comment.len(),
                    media_type,
                    item.spoiler
                );
            }
        }

        if page >= total_pages {
            break;
        }
        page += 1;
    }

    debug!(
        "Fetched Trakt comments/reviews: total_items={}, items_with_empty_imdb={}, items_with_unknown_type={}",
        all_comments.len(),
        items_with_empty_imdb,
        items_with_unknown_type
    );

    Ok(all_comments)
}

/// Fetch watch history from Trakt with pagination
pub async fn get_watch_history(
    client: &Client,
    access_token: &str,
    encoded_username: &str,
    client_id: &str,
) -> Result<Vec<WatchHistory>> {
    
    let mut all_history = Vec::new();
    let mut page = 1;
    let mut seen_ids = std::collections::HashSet::new();
    let mut items_with_empty_imdb = 0;

    loop {
        let url = format!(
            "https://api.trakt.tv/users/{}/history?extended=full&page={}&limit=100",
            encoded_username, page
        );

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("trakt-api-version", "2")
            .header("trakt-api-key", client_id)
            .header("Accept", "application/json")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Content-Type", "application/json")
            .header("Origin", "https://trakt.tv")
            .header("Referer", "https://trakt.tv/")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to fetch watch history: {}", response.status()));
        }

        let total_pages: u32 = response
            .headers()
            .get("X-Pagination-Page-Count")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        let items: Vec<TraktHistoryItem> = response.json().await?;

        for item in items {
            let (trakt_ids, imdb_id, media_type, _trakt_id) = match item.item_type.as_str() {
                "movie" => {
                    let movie = item.movie.ok_or_else(|| anyhow!("Missing movie data"))?;
                    let trakt_id = movie.ids.trakt;
                    if let Some(id) = trakt_id {
                        if seen_ids.contains(&id) {
                            continue;
                        }
                        seen_ids.insert(id);
                    }
                    (
                        movie.ids.clone(),
                        remove_slashes(movie.ids.imdb.clone()),
                        MediaType::Movie,
                        trakt_id,
                    )
                }
                "episode" => {
                    let episode = item.episode.ok_or_else(|| anyhow!("Missing episode data"))?;
                    let show = item.show.ok_or_else(|| anyhow!("Missing show data for episode"))?;
                    
                    // Track show
                    let show_trakt_id = show.ids.trakt;
                    if let Some(id) = show_trakt_id {
                        if !seen_ids.contains(&id) {
                            seen_ids.insert(id);
                        }
                    }
                    
                    // Track episode
                    let episode_trakt_id = episode.ids.trakt;
                    if let Some(id) = episode_trakt_id {
                        if seen_ids.contains(&id) {
                            continue;
                        }
                        seen_ids.insert(id);
                    }
                    
                    (
                        episode.ids.clone(),
                        remove_slashes(episode.ids.imdb.clone()),
                        MediaType::Episode {
                            season: episode.season.unwrap_or(0),
                            episode: episode.number.unwrap_or(0),
                        },
                        episode_trakt_id,
                    )
                }
                _ => continue,
            };

            // Extract MediaIds
            let media_ids = extract_media_ids_from_trakt_ids(&trakt_ids);
            
            // Don't skip items if they have any IDs (not just imdb_id)
            if media_ids.is_empty() {
                items_with_empty_imdb += 1;
                // Log first few items with empty IMDB IDs
                if items_with_empty_imdb <= 5 {
                    debug!(
                        "Trakt watch history: Skipping item with empty IMDB ID, type={:?}, trakt_id={:?}",
                        media_type,
                        _trakt_id
                    );
                }
                continue;
            }

            let watched_at = DateTime::parse_from_rfc3339(&item.watched_at)
                .map_err(|e| anyhow!("Failed to parse date: {}", e))?
                .with_timezone(&Utc);

            // Clone media_type for logging before moving it
            let media_type_for_log = if all_history.len() < 5 { Some(media_type.clone()) } else { None };
            
            all_history.push(WatchHistory {
                imdb_id: imdb_id.clone(),
                ids: Some(media_ids),
                title: None,
                year: None,
                watched_at,
                media_type,
                source: "trakt".to_string(),
            });
            
            // Log first few items being added
            if let Some(ref mt) = media_type_for_log {
                debug!(
                    "Trakt watch history[{}]: imdb_id={}, watched_at={}, media_type={:?}",
                    all_history.len() - 1,
                    imdb_id,
                    all_history.last().unwrap().watched_at,
                    mt
                );
            }
        }

        if page >= total_pages {
            break;
        }
        page += 1;
    }

    debug!(
        "Fetched Trakt watch history: total_items={}, items_with_empty_imdb={}, unique_trakt_ids_seen={}",
        all_history.len(),
        items_with_empty_imdb,
        seen_ids.len()
    );

    Ok(all_history)
}

/// Add items to Trakt watchlist
pub async fn add_to_watchlist(
    client: &Client,
    access_token: &str,
    items: &[WatchlistItem],
    client_id: &str,
) -> Result<()> {
    let mut movies = Vec::new();
    let mut shows = Vec::new();
    let mut episodes = Vec::new();

    for item in items {
        // Build IDs object with all available IDs from MediaIds
        let mut ids_obj = serde_json::Map::new();
        
        // Use MediaIds if available, otherwise fall back to imdb_id
        if let Some(ref media_ids) = item.ids {
            if let Some(ref imdb) = media_ids.imdb_id {
                ids_obj.insert("imdb".to_string(), serde_json::Value::String(imdb.clone()));
            }
            if let Some(trakt) = media_ids.trakt_id {
                ids_obj.insert("trakt".to_string(), serde_json::Value::Number(trakt.into()));
            }
            if let Some(tmdb) = media_ids.tmdb_id {
                ids_obj.insert("tmdb".to_string(), serde_json::Value::Number(tmdb.into()));
            }
            if let Some(tvdb) = media_ids.tvdb_id {
                ids_obj.insert("tvdb".to_string(), serde_json::Value::Number(tvdb.into()));
            }
            if let Some(ref slug) = media_ids.slug {
                ids_obj.insert("slug".to_string(), serde_json::Value::String(slug.clone()));
            }
        } else {
            // Fallback to imdb_id if MediaIds not available
            ids_obj.insert("imdb".to_string(), serde_json::Value::String(item.imdb_id.clone()));
        }
        
        let id_obj = serde_json::json!({
            "ids": ids_obj
        });

        match &item.media_type {
            MediaType::Movie => movies.push(id_obj),
            MediaType::Show => shows.push(id_obj),
            MediaType::Episode { .. } => episodes.push(id_obj),
        }
    }

    let payload = serde_json::json!({
        "movies": movies,
        "shows": shows,
        "episodes": episodes
    });

    let response = client
        .post("https://api.trakt.tv/sync/watchlist")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id)
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .header("Origin", "https://trakt.tv")
        .header("Referer", "https://trakt.tv/")
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

/// Remove items from Trakt watchlist
pub async fn remove_from_watchlist(
    client: &Client,
    access_token: &str,
    items: &[WatchlistItem],
    client_id: &str,
) -> Result<()> {
    let mut movies = Vec::new();
    let mut shows = Vec::new();
    let mut episodes = Vec::new();

    for item in items {
        // Build IDs object with all available IDs from MediaIds
        let mut ids_obj = serde_json::Map::new();
        
        // Use MediaIds if available, otherwise fall back to imdb_id
        if let Some(ref media_ids) = item.ids {
            if let Some(ref imdb) = media_ids.imdb_id {
                ids_obj.insert("imdb".to_string(), serde_json::Value::String(imdb.clone()));
            }
            if let Some(trakt) = media_ids.trakt_id {
                ids_obj.insert("trakt".to_string(), serde_json::Value::Number(trakt.into()));
            }
            if let Some(tmdb) = media_ids.tmdb_id {
                ids_obj.insert("tmdb".to_string(), serde_json::Value::Number(tmdb.into()));
            }
            if let Some(tvdb) = media_ids.tvdb_id {
                ids_obj.insert("tvdb".to_string(), serde_json::Value::Number(tvdb.into()));
            }
            if let Some(ref slug) = media_ids.slug {
                ids_obj.insert("slug".to_string(), serde_json::Value::String(slug.clone()));
            }
        } else {
            // Fallback to imdb_id if MediaIds not available
            ids_obj.insert("imdb".to_string(), serde_json::Value::String(item.imdb_id.clone()));
        }
        
        let id_obj = serde_json::json!({
            "ids": ids_obj
        });

        match &item.media_type {
            MediaType::Movie => movies.push(id_obj),
            MediaType::Show => shows.push(id_obj),
            MediaType::Episode { .. } => episodes.push(id_obj),
        }
    }

    let payload = serde_json::json!({
        "movies": movies,
        "shows": shows,
        "episodes": episodes
    });

    let response = client
        .post("https://api.trakt.tv/sync/watchlist/remove")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id)
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .header("Origin", "https://trakt.tv")
        .header("Referer", "https://trakt.tv/")
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

/// Set ratings on Trakt
pub async fn set_ratings(
    client: &Client,
    access_token: &str,
    ratings: &[Rating],
    client_id: &str,
) -> Result<()> {
    let mut movies = Vec::new();
    let mut shows = Vec::new();
    let mut episodes = Vec::new();

    for rating in ratings {
        // Build IDs object with all available IDs from MediaIds
        let mut ids_obj = serde_json::Map::new();
        
        // Use MediaIds if available, otherwise fall back to imdb_id
        if let Some(ref media_ids) = rating.ids {
            if let Some(ref imdb) = media_ids.imdb_id {
                ids_obj.insert("imdb".to_string(), serde_json::Value::String(imdb.clone()));
            }
            if let Some(trakt) = media_ids.trakt_id {
                ids_obj.insert("trakt".to_string(), serde_json::Value::Number(trakt.into()));
            }
            if let Some(tmdb) = media_ids.tmdb_id {
                ids_obj.insert("tmdb".to_string(), serde_json::Value::Number(tmdb.into()));
            }
            if let Some(tvdb) = media_ids.tvdb_id {
                ids_obj.insert("tvdb".to_string(), serde_json::Value::Number(tvdb.into()));
            }
            if let Some(ref slug) = media_ids.slug {
                ids_obj.insert("slug".to_string(), serde_json::Value::String(slug.clone()));
            }
        } else {
            // Fallback to imdb_id if MediaIds not available
            ids_obj.insert("imdb".to_string(), serde_json::Value::String(rating.imdb_id.clone()));
        }
        
        let item = serde_json::json!({
            "ids": ids_obj,
            "rating": rating.rating
        });

        match &rating.media_type {
            MediaType::Movie => movies.push(item),
            MediaType::Show => shows.push(item),
            MediaType::Episode { .. } => episodes.push(item),
        }
    }

    let payload = serde_json::json!({
        "movies": movies,
        "shows": shows,
        "episodes": episodes
    });

    let response = client
        .post("https://api.trakt.tv/sync/ratings")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id)
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .header("Origin", "https://trakt.tv")
        .header("Referer", "https://trakt.tv/")
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

/// Add comments/reviews to Trakt
pub async fn add_comments(
    client: &Client,
    access_token: &str,
    reviews: &[Review],
    client_id: &str,
) -> Result<()> {
    for review in reviews {
        let mut payload = serde_json::json!({
            "comment": review.content
        });

        match &review.media_type {
            MediaType::Movie => {
                payload["movie"] = serde_json::json!({
                    "ids": {
                        "imdb": review.imdb_id
                    }
                });
            }
            MediaType::Show => {
                payload["show"] = serde_json::json!({
                    "ids": {
                        "imdb": review.imdb_id
                    }
                });
            }
            MediaType::Episode { .. } => {
                payload["episode"] = serde_json::json!({
                    "ids": {
                        "imdb": review.imdb_id
                    }
                });
            }
        }

        let response = client
            .post("https://api.trakt.tv/comments")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("trakt-api-version", "2")
            .header("trakt-api-key", client_id)
            .header("Accept", "application/json")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Content-Type", "application/json")
            .header("Origin", "https://trakt.tv")
            .header("Referer", "https://trakt.tv/")
            .json(&payload)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to add comment: {} - {}", status, error_text));
        }
    }

    Ok(())
}

/// Add watch history to Trakt
pub async fn add_watch_history(
    client: &Client,
    access_token: &str,
    items: &[WatchHistory],
    client_id: &str,
) -> Result<()> {
    use tracing::{debug, warn};
    
    let mut movies = Vec::new();
    let mut episodes = Vec::new();
    let mut skipped_shows = 0;

    for item in items {
        // Skip shows (would mark all episodes as watched)
        if matches!(item.media_type, MediaType::Show) {
            skipped_shows += 1;
            if skipped_shows <= 5 {
                warn!(
                    "Skipping Show when adding to Trakt watch history (should have been filtered earlier): imdb_id={}",
                    item.imdb_id
                );
            }
            continue;
        }

        let mut item_obj = serde_json::json!({
            "ids": {
                "imdb": item.imdb_id
            },
            "watched_at": item.watched_at.to_rfc3339()
        });

        match &item.media_type {
            MediaType::Movie => movies.push(item_obj),
            MediaType::Episode { .. } => episodes.push(item_obj),
            MediaType::Show => continue, // Already skipped above
        }
    }
    
    if skipped_shows > 0 {
        warn!(
            "Skipped {} Shows when adding to Trakt watch history (Trakt doesn't support shows in watch history)",
            skipped_shows
        );
    }

    let payload = serde_json::json!({
        "movies": movies,
        "episodes": episodes
    });

    let response = client
        .post("https://api.trakt.tv/sync/history")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id)
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .header("Origin", "https://trakt.tv")
        .header("Referer", "https://trakt.tv/")
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

/// Normalize title for Trakt API search
/// Removes commas and normalizes whitespace to improve search matching
fn normalize_title_for_search(title: &str) -> String {
    // Replace commas with spaces, then normalize whitespace
    title
        .replace(',', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Search for media by title using Trakt API
/// Uses Trakt API text query endpoint: GET /search/{type}?query={query}
/// Reference: https://trakt.docs.apiary.io/#reference/search/text-query/get-text-query-results
pub async fn search_by_title(
    client: &Client,
    access_token: &str,
    client_id: &str,
    title: &str,
    year: Option<u32>,
    media_type: &MediaType,
) -> Result<Option<media_sync_models::MediaIds>> {
    use media_sync_models::MediaIds;
    
    let search_type = match media_type {
        MediaType::Movie => "movie",
        MediaType::Show => "show",
        MediaType::Episode { .. } => return Ok(None), // Episodes not supported in search
    };
    
    // Normalize title for search (remove commas, normalize whitespace)
    let normalized_title = normalize_title_for_search(title);
    
    // Build URL according to Trakt API: /search/{type}?query={query}&year={year}
    let mut url = format!("https://api.trakt.tv/search/{}?query={}", search_type, urlencoding::encode(&normalized_title));
    if let Some(y) = year {
        url.push_str(&format!("&year={}", y));
    }
    
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id)
        .header("Accept", "application/json")
        .send()
        .await?;
    
    let status = response.status();
    
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        warn!("Trakt search failed for '{}' (normalized: '{}'): HTTP {} - {}", title, normalized_title, status, error_text);
        return Ok(None);
    }
    
    let items: Vec<serde_json::Value> = response.json().await?;
    
    // If we got results, try to extract IDs
    if let Some(first_item) = items.first() {
        let ids_value = first_item.get(search_type)
            .and_then(|m| m.get("ids"));
        
        if let Some(ids_json) = ids_value {
            let trakt_ids: TraktIds = serde_json::from_value(ids_json.clone())?;
            
            let mut media_ids = MediaIds::default();
            media_ids.imdb_id = trakt_ids.imdb.map(|s| remove_slashes(Some(s)));
            media_ids.trakt_id = trakt_ids.trakt;
            media_ids.tmdb_id = trakt_ids.tmdb;
            media_ids.tvdb_id = trakt_ids.tvdb;
            media_ids.slug = trakt_ids.slug;
            
            debug!("Trakt search: Found IDs for '{}' (normalized: '{}'): imdb={:?}, trakt={:?}, tmdb={:?}", 
                   title, normalized_title, media_ids.imdb_id, media_ids.trakt_id, media_ids.tmdb_id);
            
            return Ok(Some(media_ids));
        }
    }
    
    Ok(None)
}

/// Search Trakt by IMDB ID to get title, year, and other IDs
/// Reference: https://trakt.docs.apiary.io/#reference/search/id-lookup/get-id-lookup-results
pub async fn search_by_imdb_id(
    client: &Client,
    access_token: &str,
    client_id: &str,
    imdb_id: &str,
    media_type: &MediaType,
) -> Result<Option<(String, Option<u32>, media_sync_models::MediaIds)>> {
    use media_sync_models::MediaIds;
    
    let expected_type = match media_type {
        MediaType::Movie => "movie",
        MediaType::Show => "show",
        MediaType::Episode { .. } => return Ok(None), // Episodes not supported in search
    };
    
    // Trakt API: /search/imdb/{imdb_id} (simplest format, returns all types, filter by type field)
    let url = format!(
        "https://api.trakt.tv/search/imdb/{}",
        urlencoding::encode(imdb_id)
    );
    
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("trakt-api-version", "2")
        .header("trakt-api-key", client_id)
        .header("Accept", "application/json")
        .send()
        .await?;
    
    let status = response.status();
    
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        warn!("Trakt IMDB ID lookup failed for '{}': HTTP {} - {}", imdb_id, status, error_text);
        return Ok(None);
    }
    
    let items: Vec<serde_json::Value> = response.json().await?;
    
    // Filter results by type and extract title, year, and IDs
    for item in items {
        // Check if the item type matches what we're looking for
        let item_type = item.get("type")
            .and_then(|t| t.as_str());
        
        if item_type != Some(expected_type) {
            continue; // Skip items that don't match the requested type
        }
        
        // Extract from either "movie" or "show" field based on type
        let media_json = item.get(expected_type);
        
        if let Some(media_json) = media_json {
            let title = media_json.get("title")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string());
            
            let year = media_json.get("year")
                .and_then(|y| y.as_u64())
                .map(|y| y as u32);
            
            let ids_value = media_json.get("ids");
            
            if let (Some(title), Some(ids_json)) = (title, ids_value) {
                let trakt_ids: TraktIds = serde_json::from_value(ids_json.clone())?;
                
                let mut media_ids = MediaIds::default();
                media_ids.imdb_id = trakt_ids.imdb.map(|s| remove_slashes(Some(s)));
                media_ids.trakt_id = trakt_ids.trakt;
                media_ids.tmdb_id = trakt_ids.tmdb;
                media_ids.tvdb_id = trakt_ids.tvdb;
                media_ids.slug = trakt_ids.slug;
                // Set title and year in MediaIds
                media_ids.title = Some(title.clone());
                media_ids.year = year;
                media_ids.media_type = Some(media_type.clone());
                
                debug!("Trakt IMDB ID lookup: Found '{}' (year: {:?}) for imdb_id={}: trakt={:?}, tmdb={:?}", 
                       title, year, imdb_id, media_ids.trakt_id, media_ids.tmdb_id);
                
                return Ok(Some((title, year, media_ids)));
            }
        }
    }
    
    Ok(None)
}

