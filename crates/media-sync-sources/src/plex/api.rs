use anyhow::{Result, Context};
use chrono::{DateTime, Utc, TimeZone};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info, warn};

const DISCOVER_BASE_URL: &str = "https://discover.provider.plex.tv";
const PLEX_TV_BASE_URL: &str = "https://plex.tv";

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub url: String,
    pub name: String,
    pub identifier: String,
}

#[derive(Debug, Clone)]
pub struct LibraryInfo {
    pub key: String,
    pub type_: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Guid {
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct MovieMetadata {
    pub rating_key: String,
    pub key: String,
    pub title: String,
    pub year: Option<u32>,
    pub user_rating: Option<f64>,
    pub view_count: Option<u32>,
    pub last_viewed_at: Option<DateTime<Utc>>,
    pub guids: Vec<Guid>,
    pub type_: String,
}

#[derive(Debug, Clone)]
pub struct ShowMetadata {
    pub rating_key: String,
    pub key: String,
    pub title: String,
    pub year: Option<u32>,
    pub user_rating: Option<f64>,
    pub view_count: Option<u32>,
    pub last_viewed_at: Option<DateTime<Utc>>,
    pub guids: Vec<Guid>,
    pub type_: String,
}

#[derive(Debug, Clone)]
pub struct MetadataItem {
    pub rating_key: String,
    pub user_rating: Option<f64>,
    pub title: String,
    pub guids: Vec<Guid>,
}

#[derive(Debug, Clone)]
pub struct WatchlistItem {
    pub rating_key: String,
    pub type_: String,
    pub title: String,
    pub year: Option<u32>,
    pub guids: Vec<Guid>,
}

#[derive(Debug, Clone)]
pub struct PlayHistoryItem {
    pub rating_key: String,
    pub type_: String,
    pub view_count: u32,
    pub last_viewed_at: DateTime<Utc>,
    pub title: Option<String>,
    pub year: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct RatingItem {
    pub rating_key: String,
    pub type_: String,
    pub user_rating: f64,
    pub guids: Vec<Guid>,
}

#[derive(Debug, Clone)]
pub struct ReviewItem {
    pub rating_key: String,
    pub type_: String,
    pub review_text: String,
}

#[derive(Debug, Deserialize)]
struct MediaContainer {
    #[serde(rename = "Metadata")]
    metadata: Option<Vec<Value>>,
    #[serde(rename = "Video")]
    video: Option<Vec<Value>>,
    #[serde(rename = "Directory")]
    directory: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
struct PlexResponse {
    #[serde(rename = "MediaContainer")]
    media_container: MediaContainer,
}

pub struct PlexHttpClient {
    client: Client,
    token: String,
    server_url: Option<String>,
    discover_base_url: String,
}

impl PlexHttpClient {
    pub fn new(token: String, server_url: Option<String>) -> Result<Self> {
        let client = Client::builder()
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(
                    reqwest::header::ACCEPT,
                    reqwest::header::HeaderValue::from_static("application/json"),
                );
                headers.insert(
                    reqwest::header::HeaderName::from_static("x-plex-token"),
                    reqwest::header::HeaderValue::from_str(&token)
                        .context("Invalid token format")?,
                );
                headers.insert(
                    reqwest::header::HeaderName::from_static("x-plex-client-identifier"),
                    reqwest::header::HeaderValue::from_static("totalrecall-cli"),
                );
                headers
            })
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            token,
            server_url,
            discover_base_url: DISCOVER_BASE_URL.to_string(),
        })
    }


    pub async fn authenticate(&self) -> Result<()> {
        let url = format!("{}/api/v2/user", PLEX_TV_BASE_URL);
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to authenticate with Plex")?;

        if response.status().is_success() {
            debug!("Plex authentication successful");
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Plex authentication failed: {}",
                response.status()
            ))
        }
    }

    pub async fn get_servers(&self) -> Result<Vec<ServerInfo>> {
        let url = format!("{}/api/v2/resources?includeHttps=1", PLEX_TV_BASE_URL);
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("X-Plex-Client-Identifier", "totalrecall-cli")
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to get Plex servers")?;

        let json: Value = response
            .json()
            .await
            .context("Failed to parse servers response")?;

        debug!("Plex server discovery: Response structure: {:?}", json);

        let mut servers = Vec::new();
        
        // The API returns a direct array of resources
        if let Some(resources_array) = json.as_array() {
            debug!("Plex server discovery: Found {} resources", resources_array.len());
            for (idx, resource) in resources_array.iter().enumerate() {
                if idx < 3 {
                    debug!("Plex server discovery: Resource[{}]: name={:?}, product={:?}, provides={:?}", 
                           idx, 
                           resource.get("name").and_then(|n| n.as_str()),
                           resource.get("product").and_then(|p| p.as_str()),
                           resource.get("provides").and_then(|p| p.as_str()));
                }
                
                // Check if this is a server (provides="server" or product="Plex Media Server")
                let provides = resource.get("provides").and_then(|p| p.as_str());
                let product = resource.get("product").and_then(|p| p.as_str());
                
                let is_server = provides.map(|p| p.contains("server")).unwrap_or(false) ||
                               product.map(|p| p == "Plex Media Server").unwrap_or(false);
                
                if is_server {
                    let name = resource
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    let identifier = resource
                        .get("clientIdentifier")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Get connection URL from connections array (lowercase)
                    if let Some(connections) = resource.get("connections") {
                        if let Some(conn_array) = connections.as_array() {
                            // Prefer local connections, then any connection
                            let mut local_uri = None;
                            let mut any_uri = None;
                            
                            for conn in conn_array {
                                if let Some(uri) = conn.get("uri").and_then(|u| u.as_str()) {
                                    let is_local = conn.get("local")
                                        .and_then(|l| l.as_bool())
                                        .unwrap_or(false);
                                    
                                    if is_local {
                                        local_uri = Some(uri.to_string());
                                    } else if any_uri.is_none() {
                                        any_uri = Some(uri.to_string());
                                    }
                                }
                            }
                            
                            if let Some(uri) = local_uri.or(any_uri) {
                                debug!("Plex server discovery: Found server '{}' at {}", name, uri);
                                servers.push(ServerInfo {
                                    url: uri,
                                    name,
                                    identifier,
                                });
                            }
                        }
                    }
                }
            }
        } else {
            // Fallback: try MediaContainer structure (for older API versions)
            let resources = json.get("MediaContainer")
                .and_then(|mc| mc.get("Device"))
                .or_else(|| json.get("MediaContainer").and_then(|mc| mc.get("Metadata")));
            
            if let Some(resources) = resources {
                if let Some(resources_array) = resources.as_array() {
                    debug!("Plex server discovery: Found {} resources (MediaContainer format)", resources_array.len());
                    for resource in resources_array {
                        let provides = resource.get("provides").and_then(|p| p.as_str());
                        if let Some(provides_str) = provides {
                            if provides_str.contains("server") {
                                let name = resource
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("Unknown")
                                    .to_string();
                                
                                // Try both Connection (capital) and connections (lowercase)
                                let connections = resource.get("Connection")
                                    .or_else(|| resource.get("connections"));
                                
                                if let Some(connections) = connections {
                                    if let Some(conn_array) = connections.as_array() {
                                        for conn in conn_array {
                                            if let Some(uri) = conn.get("uri").and_then(|u| u.as_str()) {
                                                servers.push(ServerInfo {
                                                    url: uri.to_string(),
                                                    name: name.clone(),
                                                    identifier: resource
                                                        .get("clientIdentifier")
                                                        .and_then(|i| i.as_str())
                                                        .unwrap_or("")
                                                        .to_string(),
                                                });
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        debug!("Plex server discovery: Found {} servers", servers.len());
        Ok(servers)
    }

    pub async fn get_libraries(&self, server_url: &str) -> Result<Vec<LibraryInfo>> {
        let url = format!("{}/library/sections", server_url);
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to get libraries")?;

        let json: Value = response
            .json()
            .await
            .context("Failed to parse libraries response")?;

        let mut libraries = Vec::new();
        if let Some(media_container) = json.get("MediaContainer") {
            if let Some(directory) = media_container.get("Directory") {
                if let Some(dir_array) = directory.as_array() {
                    for dir in dir_array {
                        let key = dir
                            .get("key")
                            .and_then(|k| k.as_str())
                            .unwrap_or("")
                            .to_string();
                        let type_ = dir
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        let title = dir
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();

                        libraries.push(LibraryInfo { key, type_, title });
                    }
                }
            }
        }

        Ok(libraries)
    }

    fn parse_guid_array(&self, guid_value: &Value) -> Vec<Guid> {
        let mut guids = Vec::new();
        if guid_value.is_null() {
            return guids;
        }
        
        if let Some(guid_array) = guid_value.as_array() {
            for guid_obj in guid_array {
                if let Some(id) = guid_obj.get("id").and_then(|i| i.as_str()) {
                    guids.push(Guid { id: id.to_string() });
                } else {
                    // Try alternative structure - sometimes GUIDs are just strings
                    if let Some(id_str) = guid_obj.as_str() {
                        guids.push(Guid { id: id_str.to_string() });
                    }
                }
            }
        } else if let Some(guid_obj) = guid_value.as_object() {
            if let Some(id) = guid_obj.get("id").and_then(|i| i.as_str()) {
                guids.push(Guid { id: id.to_string() });
            }
        } else if let Some(id_str) = guid_value.as_str() {
            // Sometimes GUID might be a direct string
            guids.push(Guid { id: id_str.to_string() });
        }
        guids
    }

    fn parse_timestamp(&self, timestamp: Option<&Value>) -> Option<DateTime<Utc>> {
        timestamp
            .and_then(|t| t.as_i64())
            .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
    }

    fn parse_metadata_item(&self, item: &Value, item_type: &str) -> Option<MovieMetadata> {
        let rating_key = item.get("ratingKey")?.as_str()?.to_string();
        let key = item.get("key").and_then(|k| k.as_str()).unwrap_or("").to_string();
        let title = item.get("title")?.as_str()?.to_string();
        let year = item.get("year").and_then(|y| y.as_u64()).map(|y| y as u32);
        let user_rating = item.get("userRating").and_then(|r| r.as_f64());
        let view_count = item.get("viewCount").and_then(|v| v.as_u64()).map(|v| v as u32);
        let last_viewed_at = self.parse_timestamp(item.get("lastViewedAt"));
        let guids = self.parse_guid_array(item.get("Guid").unwrap_or(&Value::Null));

        Some(MovieMetadata {
            rating_key,
            key,
            title,
            year,
            user_rating,
            view_count,
            last_viewed_at,
            guids,
            type_: item_type.to_string(),
        })
    }

    pub async fn get_movies(&self, server_url: &str, library_key: &str) -> Result<Vec<MovieMetadata>> {
        let url = format!(
            "{}/library/sections/{}/all?type=1&includeGuids=1",
            server_url, library_key
        );
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to get movies")?;

        let json: Value = response
            .json()
            .await
            .context("Failed to parse movies response")?;

        let mut movies = Vec::new();
        if let Some(media_container) = json.get("MediaContainer") {
            if let Some(metadata) = media_container.get("Metadata") {
                if let Some(meta_array) = metadata.as_array() {
                    debug!("Plex get_movies: Found {} items in library", meta_array.len());
                    let mut skipped = 0;
                    for (idx, item) in meta_array.iter().enumerate() {
                        if let Some(movie) = self.parse_metadata_item(item, "movie") {
                            movies.push(movie);
                        } else {
                            skipped += 1;
                            if skipped <= 3 {
                                let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("unknown");
                                debug!("Plex get_movies: Skipped item[{}] '{}' - parse_metadata_item returned None", idx, title);
                            }
                        }
                    }
                    if skipped > 0 {
                        debug!("Plex get_movies: Skipped {} items that couldn't be parsed", skipped);
                    }
                } else {
                    debug!("Plex get_movies: Metadata field is not an array");
                }
            } else {
                debug!("Plex get_movies: No Metadata field in MediaContainer");
            }
        } else {
            debug!("Plex get_movies: No MediaContainer in response");
        }

        Ok(movies)
    }

    pub async fn get_shows(&self, server_url: &str, library_key: &str) -> Result<Vec<ShowMetadata>> {
        let url = format!(
            "{}/library/sections/{}/all?type=2&includeGuids=1",
            server_url, library_key
        );
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to get shows")?;

        let json: Value = response
            .json()
            .await
            .context("Failed to parse shows response")?;

        let mut shows = Vec::new();
        if let Some(media_container) = json.get("MediaContainer") {
            if let Some(metadata) = media_container.get("Metadata") {
                if let Some(meta_array) = metadata.as_array() {
                    debug!("Plex get_shows: Found {} items in library", meta_array.len());
                    let mut skipped = 0;
                    for (idx, item) in meta_array.iter().enumerate() {
                        if let Some(movie) = self.parse_metadata_item(item, "show") {
                            // Reuse MovieMetadata structure for shows
                            shows.push(ShowMetadata {
                                rating_key: movie.rating_key,
                                key: movie.key,
                                title: movie.title,
                                year: movie.year,
                                user_rating: movie.user_rating,
                                view_count: movie.view_count,
                                last_viewed_at: movie.last_viewed_at,
                                guids: movie.guids,
                                type_: "show".to_string(),
                            });
                        } else {
                            skipped += 1;
                            if skipped <= 3 {
                                let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("unknown");
                                debug!("Plex get_shows: Skipped item[{}] '{}' - parse_metadata_item returned None", idx, title);
                            }
                        }
                    }
                    if skipped > 0 {
                        debug!("Plex get_shows: Skipped {} items that couldn't be parsed", skipped);
                    }
                } else {
                    debug!("Plex get_shows: Metadata field is not an array");
                }
            } else {
                debug!("Plex get_shows: No Metadata field in MediaContainer");
            }
        } else {
            debug!("Plex get_shows: No MediaContainer in response");
        }

        Ok(shows)
    }

    pub async fn get_metadata_item(&self, server_url: &str, rating_key: &str) -> Result<MetadataItem> {
        // Extract numeric ID from rating_key (e.g., "/library/metadata/123" -> "123")
        let id = rating_key
            .trim_start_matches("/library/metadata/")
            .trim();
        
        // Include GUIDs to get IMDB, TMDB, and TVDB identifiers
        let url = format!("{}/library/metadata/{}?includeGuids=1", server_url, id);
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to get metadata item")?;

        let json: Value = response
            .json()
            .await
            .context("Failed to parse metadata item response")?;

        if let Some(media_container) = json.get("MediaContainer") {
            if let Some(metadata) = media_container.get("Metadata") {
                if let Some(meta_array) = metadata.as_array() {
                    if let Some(item) = meta_array.first() {
                        let rating_key = item
                            .get("ratingKey")
                            .and_then(|k| k.as_str())
                            .unwrap_or("")
                            .to_string();
                        let user_rating = item.get("userRating").and_then(|r| r.as_f64());
                        let title = item
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        let guids = self.parse_guid_array(item.get("Guid").unwrap_or(&Value::Null));

                        return Ok(MetadataItem {
                            rating_key,
                            user_rating,
                            title,
                            guids,
                        });
                    }
                }
            }
        }

        Err(anyhow::anyhow!("Metadata item not found"))
    }


    pub async fn get_watchlist(&self) -> Result<Vec<WatchlistItem>> {
        let url = format!("{}/library/sections/watchlist/all", self.discover_base_url);
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to get watchlist")?;

        let json: Value = response
            .json()
            .await
            .context("Failed to parse watchlist response")?;

        let mut watchlist = Vec::new();
        if let Some(media_container) = json.get("MediaContainer") {
            if let Some(metadata) = media_container.get("Metadata") {
                if let Some(meta_array) = metadata.as_array() {
                    debug!("Plex watchlist API returned {} items", meta_array.len());
                    for (idx, item) in meta_array.iter().enumerate() {
                        let rating_key = item
                            .get("ratingKey")
                            .and_then(|k| k.as_str())
                            .unwrap_or("")
                            .to_string();
                        let type_ = item
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        let title = item
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        let year = item.get("year").and_then(|y| y.as_u64()).map(|y| y as u32);
                        
                        // Debug: Log GUID structure
                        let guid_value = item.get("Guid");
                        if idx < 3 {
                            debug!("Plex watchlist item[{}]: rating_key={}, title={}, Guid field: {:?}", 
                                   idx, rating_key, title, guid_value);
                        }
                        
                        let guids = self.parse_guid_array(guid_value.unwrap_or(&Value::Null));
                        if idx < 3 {
                            debug!("Plex watchlist item[{}]: parsed {} GUIDs: {:?}", 
                                   idx, guids.len(), guids.iter().map(|g| &g.id).collect::<Vec<_>>());
                        }

                        watchlist.push(WatchlistItem {
                            rating_key,
                            type_,
                            title,
                            year,
                            guids,
                        });
                    }
                } else {
                    debug!("Plex watchlist: Metadata field is not an array");
                }
            } else {
                debug!("Plex watchlist: No Metadata field in MediaContainer");
            }
        } else {
            debug!("Plex watchlist: No MediaContainer in response");
        }

        debug!("Plex watchlist: Returning {} items", watchlist.len());
        Ok(watchlist)
    }

    pub async fn add_to_watchlist(&self, rating_key: &str) -> Result<()> {
        let url = format!(
            "{}/actions/addToWatchlist?ratingKey={}",
            self.discover_base_url, rating_key
        );
        let response = self
            .client
            .put(&url)
            .header("X-Plex-Token", &self.token)
            .send()
            .await
            .context("Failed to add to watchlist")?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to add to watchlist: {}",
                response.status()
            ))
        }
    }

    pub async fn remove_from_watchlist(&self, rating_key: &str) -> Result<()> {
        let url = format!(
            "{}/actions/removeFromWatchlist?ratingKey={}",
            self.discover_base_url, rating_key
        );
        let response = self
            .client
            .delete(&url)
            .header("X-Plex-Token", &self.token)
            .send()
            .await
            .context("Failed to remove from watchlist")?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to remove from watchlist: {}",
                response.status()
            ))
        }
    }

    pub async fn get_ratings(&self, server_url: &str) -> Result<Vec<RatingItem>> {
        // Get all libraries and iterate through items to find ratings
        let libraries = self.get_libraries(server_url).await?;
        debug!("Plex get_ratings: Found {} libraries", libraries.len());
        let mut ratings = Vec::new();
        let mut total_movies = 0;
        let mut total_shows = 0;
        let mut movies_with_ratings = 0;
        let mut shows_with_ratings = 0;

        for library in libraries {
            if library.type_ == "movie" {
                let movies = self.get_movies(server_url, &library.key).await?;
                total_movies += movies.len();
                debug!("Plex get_ratings: Library '{}' has {} movies", library.title, movies.len());
                for movie in movies {
                    if let Some(user_rating) = movie.user_rating {
                        if user_rating > 0.0 {
                            movies_with_ratings += 1;
                            if ratings.len() < 3 {
                                debug!("Plex rating[{}]: rating_key={}, title={}, rating={}, guids={:?}", 
                                       ratings.len(), movie.rating_key, movie.title, user_rating,
                                       movie.guids.iter().map(|g| &g.id).collect::<Vec<_>>());
                            }
                            ratings.push(RatingItem {
                                rating_key: movie.rating_key,
                                type_: "movie".to_string(),
                                user_rating,
                                guids: movie.guids,
                            });
                        }
                    }
                }
            } else if library.type_ == "show" {
                let shows = self.get_shows(server_url, &library.key).await?;
                total_shows += shows.len();
                debug!("Plex get_ratings: Library '{}' has {} shows", library.title, shows.len());
                for show in shows {
                    if let Some(user_rating) = show.user_rating {
                        if user_rating > 0.0 {
                            shows_with_ratings += 1;
                            if ratings.len() < 3 {
                                debug!("Plex rating[{}]: rating_key={}, title={}, rating={}, guids={:?}", 
                                       ratings.len(), show.rating_key, show.title, user_rating,
                                       show.guids.iter().map(|g| &g.id).collect::<Vec<_>>());
                            }
                            ratings.push(RatingItem {
                                rating_key: show.rating_key,
                                type_: "show".to_string(),
                                user_rating,
                                guids: show.guids,
                            });
                        }
                    }
                }
            }
        }

        debug!("Plex get_ratings: {} total movies, {} total shows, {} movies with ratings, {} shows with ratings, {} total ratings", 
               total_movies, total_shows, movies_with_ratings, shows_with_ratings, ratings.len());
        Ok(ratings)
    }

    pub async fn set_rating(&self, server_url: &str, rating_key: &str, rating: f64) -> Result<()> {
        // Extract numeric ID from rating_key
        let id = rating_key
            .trim_start_matches("/library/metadata/")
            .trim();
        
        let url = format!(
            "{}/:/rate?identifier=com.plexapp.plugins.library&key={}&rating={}",
            server_url, id, rating
        );
        let response = self
            .client
            .put(&url)
            .header("X-Plex-Token", &self.token)
            .send()
            .await
            .context("Failed to set rating")?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Failed to set rating: {}", response.status()))
        }
    }

    pub async fn clear_rating(&self, server_url: &str, rating_key: &str) -> Result<()> {
        // Extract numeric ID from rating_key
        let id = rating_key
            .trim_start_matches("/library/metadata/")
            .trim();
        
        let url = format!(
            "{}/:/rate?identifier=com.plexapp.plugins.library&key={}&rating=-1",
            server_url, id
        );
        let response = self
            .client
            .put(&url)
            .header("X-Plex-Token", &self.token)
            .send()
            .await
            .context("Failed to clear rating")?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Failed to clear rating: {}", response.status()))
        }
    }

    pub async fn get_reviews(&self) -> Result<Vec<ReviewItem>> {
        // Reviews may not have a direct endpoint, so return empty for now
        // This can be enhanced later if a reviews endpoint is discovered
        warn!("Plex reviews collection is not yet implemented - no direct endpoint available");
        Ok(Vec::new())
    }

    pub async fn set_review(&self, server_url: &str, rating_key: &str, review_text: &str) -> Result<()> {
        // Similar to mark_watched, use Timeline API on local server
        // Try different identifier/key combinations for discover provider items
        // Create the full key path string before the loop
        let full_key_path = format!("/library/metadata/{}", rating_key);
        
        // Try discover provider identifier first (for items not in local library)
        let identifiers = vec![
            ("tv.plex.provider.discover", rating_key),
            ("com.plexapp.plugins.library", rating_key),
            ("tv.plex.provider.discover", &full_key_path),
        ];
        
        for (identifier, key) in identifiers {
            // Try POST with JSON body first (common format)
            let url = format!(
                "{}/:/rateAndReview?identifier={}&key={}",
                server_url, identifier, key
            );
            debug!("Plex API: Calling set_review (POST) for rating_key={}, identifier={}, key={}, url={}", 
                   rating_key, identifier, key, url);
            
            let response = self
                .client
                .post(&url)
                .header("X-Plex-Token", &self.token)
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "text": review_text
                }))
                .send()
                .await
                .context(format!("Failed to set review (identifier={}, key={})", identifier, key))?;
            
            let status = response.status();
            debug!("Plex API: set_review response status={} for rating_key={}, identifier={}, key={}", 
                   status, rating_key, identifier, key);
            
            if status.is_success() {
                info!("Plex API: Successfully set review: rating_key={}, identifier={}, key={}", 
                      rating_key, identifier, key);
                return Ok(());
            } else {
                // Try to read response body for more details
                let error_msg = if let Ok(body) = response.text().await {
                    format!("HTTP {}: {}", status, body)
                } else {
                    format!("HTTP {}", status)
                };
                debug!("Plex API: set_review failed with identifier={}, key={}: {}", 
                       identifier, key, error_msg);
                
                // Try PUT with query parameters as alternative
                let url_put = format!(
                    "{}/:/rateAndReview?identifier={}&key={}&text={}",
                    server_url, identifier, key, urlencoding::encode(review_text)
                );
                debug!("Plex API: Trying set_review (PUT) for rating_key={}, identifier={}, key={}", 
                       rating_key, identifier, key);
                
                let response_put = self
                    .client
                    .put(&url_put)
                    .header("X-Plex-Token", &self.token)
                    .send()
                    .await
                    .context(format!("Failed to set review (PUT, identifier={}, key={})", identifier, key))?;
                
                let status_put = response_put.status();
                if status_put.is_success() {
                    info!("Plex API: Successfully set review (PUT): rating_key={}, identifier={}, key={}", 
                          rating_key, identifier, key);
                    return Ok(());
                }
                // Continue to next identifier/key combination
            }
        }
        
        // All attempts failed
        Err(anyhow::anyhow!(
            "Failed to set review after trying all identifier/key combinations for rating_key={}",
            rating_key
        ))
    }

    pub async fn get_play_history(&self, server_url: &str) -> Result<Vec<PlayHistoryItem>> {
        let url = format!("{}/status/sessions/history/all", server_url);
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to get play history")?;

        let json: Value = response
            .json()
            .await
            .context("Failed to parse play history response")?;

        let mut history = Vec::new();
        if let Some(media_container) = json.get("MediaContainer") {
            // Try "Video" field first, then "Metadata" field (different API versions use different fields)
            let items_array = media_container.get("Video")
                .or_else(|| media_container.get("Metadata"))
                .and_then(|v| v.as_array());
            
            if let Some(items_array) = items_array {
                debug!("Plex play history API returned {} items", items_array.len());
                for (idx, item) in items_array.iter().enumerate() {
                    let rating_key = item
                        .get("ratingKey")
                        .and_then(|k| k.as_str())
                        .unwrap_or("")
                        .to_string();
                    let type_ = item
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let view_count = item
                        .get("viewCount")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    // Try both "lastViewedAt" and "viewedAt" (different API versions use different fields)
                    let last_viewed_at = self
                        .parse_timestamp(item.get("lastViewedAt").or_else(|| item.get("viewedAt")))
                        .unwrap_or_else(|| Utc::now());
                    
                    // Extract title and year from API response
                    let title = item.get("title").and_then(|t| t.as_str()).map(|s| s.to_string());
                    let year = item.get("year")
                        .and_then(|y| y.as_u64().or_else(|| y.as_str().and_then(|s| s.parse::<u64>().ok())))
                        .or_else(|| {
                            item.get("originallyAvailableAt").and_then(|d| {
                                // Parse year from date string like "2023-07-19"
                                d.as_str().and_then(|s| s.split('-').next()).and_then(|y| y.parse::<u64>().ok())
                            })
                        })
                        .map(|y| y as u32);

                    if idx < 3 {
                        debug!("Plex play history[{}]: rating_key={}, type={}, view_count={}, title={:?}, year={:?}", 
                               idx, rating_key, type_, view_count, title, year);
                    }

                    history.push(PlayHistoryItem {
                        rating_key,
                        type_,
                        view_count,
                        last_viewed_at,
                        title,
                        year,
                    });
                }
            } else {
                debug!("Plex play history: Neither Video nor Metadata field found as array in MediaContainer. Response structure: {:?}", 
                       media_container.get("Video").or_else(|| media_container.get("Metadata")));
            }
        } else {
            debug!("Plex play history: No MediaContainer in response. Response structure: {:?}", json);
        }

        debug!("Plex play history: Returning {} items", history.len());
        Ok(history)
    }

    /// Search for media items by title on a Plex server
    /// Returns items with their GUIDs which can be used to extract IMDB IDs
    pub async fn search_by_title(&self, server_url: &str, title: &str, year: Option<u32>, media_type: &str) -> Result<Vec<MetadataItem>> {
        // Plex search endpoint: /library/search?query={title}&type={type}
        // type: 1 for movie, 2 for show
        let type_num = match media_type {
            "movie" => "1",
            "show" => "2",
            _ => "1",
        };
        
        // URL encode the title
        let encoded_title = urlencoding::encode(title);
        
        let mut url = format!(
            "{}/library/search?query={}&type={}&includeGuids=1",
            server_url,
            encoded_title,
            type_num
        );
        
        // Add year filter if available (Plex search supports year parameter)
        if let Some(year_val) = year {
            url.push_str(&format!("&year={}", year_val));
        }
        
        debug!("Plex search: Searching for '{}' (type: {}, year: {:?})", title, media_type, year);
        
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to search Plex library")?;

        let json: Value = response
            .json()
            .await
            .context("Failed to parse search response")?;

        let mut results = Vec::new();
        if let Some(media_container) = json.get("MediaContainer") {
            // Search results can be in "Metadata" or "Video" field
            let items_array = media_container.get("Metadata")
                .or_else(|| media_container.get("Video"))
                .and_then(|v| v.as_array());
            
            if let Some(items_array) = items_array {
                debug!("Plex search: Found {} results for '{}'", items_array.len(), title);
                for (idx, item) in items_array.iter().enumerate() {
                    if let Some(metadata_item) = self.parse_metadata_item(item, media_type) {
                        if idx < 3 {
                            debug!("Plex search result[{}]: rating_key={}, title={}, year={:?}, guids={:?}", 
                                   idx, metadata_item.rating_key, metadata_item.title, 
                                   metadata_item.year, 
                                   metadata_item.guids.iter().map(|g| &g.id).collect::<Vec<_>>());
                        }
                        results.push(MetadataItem {
                            rating_key: metadata_item.rating_key,
                            user_rating: metadata_item.user_rating,
                            title: metadata_item.title,
                            guids: metadata_item.guids,
                        });
                    }
                }
            } else {
                debug!("Plex search: No results found for '{}'", title);
            }
        }
        
        Ok(results)
    }

    /// Search Plex discover provider for items not in local library
    /// Returns items with their rating_keys (metadata keys) which can be used with discover provider API
    pub async fn search_discover_provider(
        &self,
        title: &str,
        year: Option<u32>,
        media_type: &str,
    ) -> Result<Vec<MetadataItem>> {
        // Plex discover provider search requires searchProviders and searchTypes parameters
        // Movies: searchProviders=discover,PLEXAVOD, searchTypes=movies
        // Shows: searchProviders=discover,PLEXTVOD, searchTypes=tv
        let (search_providers, search_types) = match media_type {
            "movie" => ("discover,PLEXAVOD", "movies"),
            "show" => ("discover,PLEXTVOD", "tv"),
            _ => ("discover,PLEXAVOD", "movies"),
        };
        
        // URL encode the title
        let encoded_title = urlencoding::encode(title);
        
        // Build URL with required parameters
        let mut url = format!(
            "{}/library/search?query={}&includeGuids=1&searchProviders={}&searchTypes={}",
            self.discover_base_url,
            encoded_title,
            search_providers,
            search_types
        );
        
        // Add year filter if available
        if let Some(year_val) = year {
            url.push_str(&format!("&year={}", year_val));
        }
        
        debug!("Plex discover provider search: Searching for '{}' (type: {}, year: {:?})", title, media_type, year);
        
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to search Plex discover provider")?;

        // Check for HTTP errors before parsing JSON
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!("Plex discover provider search failed with status {}: {}", status, error_text));
        }

        let json: Value = response
            .json()
            .await
            .context("Failed to parse discover provider search response")?;

        // Check for error in JSON response
        if let Some(error) = json.get("Error") {
            let error_msg = error.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            return Err(anyhow::anyhow!("Plex discover provider API error: {}", error_msg));
        }

        let mut results = Vec::new();
        if let Some(media_container) = json.get("MediaContainer") {
            // Discover provider returns results in SearchResults[].SearchResult[] structure
            if let Some(search_results) = media_container.get("SearchResults").and_then(|v| v.as_array()) {
                // Iterate through each SearchResults group
                for search_result_group in search_results {
                    if let Some(search_result_array) = search_result_group
                        .get("SearchResult")
                        .and_then(|v| v.as_array())
                    {
                        // Iterate through each SearchResult in the group
                        for search_result in search_result_array {
                            if let Some(metadata) = search_result.get("Metadata") {
                                // Parse rating_key, title, year from metadata
                                let rating_key = metadata.get("ratingKey")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                
                                let title = metadata.get("title")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                
                                let year = metadata.get("year")
                                    .and_then(|v| v.as_u64())
                                    .map(|y| y as u32);
                                
                                // Discover provider uses "guid" as a string (e.g., "plex://movie/5d776841e6d55c002040ee8b")
                                // instead of Guid array. We can extract IDs from this if needed, but for now
                                // we'll create an empty GUIDs array since includeGuids=1 doesn't seem to populate
                                // a Guid array in discover provider responses
                                let mut guids = Vec::new();
                                
                                // Try to extract GUID if present (though it's a string, not an array)
                                if let Some(guid_str) = metadata.get("guid").and_then(|v| v.as_str()) {
                                    // The guid format is "plex://movie/ID" or "plex://show/ID"
                                    // We could parse this, but for now we'll just use the rating_key
                                    debug!("Plex discover provider: Found guid string: {}", guid_str);
                                }
                                
                                // Also check for Guid array (in case it's populated in some cases)
                                if let Some(guid_array) = metadata.get("Guid").and_then(|v| v.as_array()) {
                                    for guid_item in guid_array {
                                        if let Some(guid_id) = guid_item.get("id").and_then(|v| v.as_str()) {
                                            guids.push(Guid {
                                                id: guid_id.to_string(),
                                            });
                                        }
                                    }
                                }
                                
                                if let (Some(rating_key), Some(title)) = (rating_key, title) {
                                    let score = search_result.get("score")
                                        .and_then(|s| s.as_f64());
                                    
                                    debug!("Plex discover provider search result: rating_key={}, title={}, year={:?}, score={:?}", 
                                           rating_key, title, year, score);
                                    
                                    results.push(MetadataItem {
                                        rating_key,
                                        user_rating: None, // Discover provider results don't include user ratings
                                        title,
                                        guids,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            
            // Fallback: also check for direct Metadata/Video fields (for backward compatibility with local server)
            if results.is_empty() {
                let items_array = media_container.get("Metadata")
                    .or_else(|| media_container.get("Video"))
                    .and_then(|v| v.as_array());
                
                if let Some(items_array) = items_array {
                    for item in items_array {
                        if let Some(metadata_item) = self.parse_metadata_item(item, media_type) {
                            results.push(MetadataItem {
                                rating_key: metadata_item.rating_key,
                                user_rating: metadata_item.user_rating,
                                title: metadata_item.title,
                                guids: metadata_item.guids,
                            });
                        }
                    }
                }
            }
        }
        
        debug!("Plex discover provider search: Found {} results for '{}'", results.len(), title);
        
        Ok(results)
    }

    pub async fn mark_watched(&self, server_url: &str, rating_key: &str) -> Result<()> {
        // According to Plex Timeline API documentation:
        // PUT /:/scrobble?identifier={identifier}&key={key}
        // For discover provider items, try using tv.plex.provider.discover as identifier
        // The key should be the rating_key (hex string like 5d9c08742192ba001f3117cd)
        
        // Create the full key path string before the loop
        let full_key_path = format!("/library/metadata/{}", rating_key);
        
        // Try discover provider identifier first (for items not in local library)
        let identifiers = vec![
            ("tv.plex.provider.discover", rating_key),
            ("com.plexapp.plugins.library", rating_key),
            ("tv.plex.provider.discover", &full_key_path),
        ];
        
        for (identifier, key) in identifiers {
        let url = format!(
                "{}/:/scrobble?identifier={}&key={}",
                server_url, identifier, key
        );
            debug!("Plex API: Calling mark_watched (PUT) for rating_key={}, identifier={}, key={}, url={}", 
                   rating_key, identifier, key, url);
            
        let response = self
            .client
                .put(&url)
            .header("X-Plex-Token", &self.token)
            .send()
            .await
                .context(format!("Failed to mark as watched (identifier={}, key={})", identifier, key))?;

            let status = response.status();
            debug!("Plex API: mark_watched response status={} for rating_key={}, identifier={}, key={}", 
                   status, rating_key, identifier, key);
            
            if status.is_success() {
                info!("Plex API: Successfully marked as watched: rating_key={}, identifier={}, key={}", 
                      rating_key, identifier, key);
                return Ok(());
            } else {
                // Try to read response body for more details
                let error_msg = if let Ok(body) = response.text().await {
                    format!("HTTP {}: {}", status, body)
                } else {
                    format!("HTTP {}", status)
                };
                debug!("Plex API: mark_watched failed with identifier={}, key={}: {}", 
                       identifier, key, error_msg);
                // Continue to next identifier/key combination
            }
        }
        
        // All attempts failed
        Err(anyhow::anyhow!(
            "Failed to mark as watched: All identifier/key combinations returned errors for rating_key={}",
            rating_key
            ))
    }

    pub async fn mark_unwatched(&self, rating_key: &str) -> Result<()> {
        let url = format!(
            "{}/actions/unscrobble?ratingKey={}",
            self.discover_base_url, rating_key
        );
        let response = self
            .client
            .post(&url)
            .header("X-Plex-Token", &self.token)
            .send()
            .await
            .context("Failed to mark as unwatched")?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to mark as unwatched: {}",
                response.status()
            ))
        }
    }

    pub async fn update_progress(
        &self,
        server_url: &str,
        rating_key: &str,
        time: u64,
        state: &str,
    ) -> Result<()> {
        // Extract numeric ID from rating_key
        let id = rating_key
            .trim_start_matches("/library/metadata/")
            .trim();
        
        let url = format!(
            "{}/:/progress?identifier=com.plexapp.plugins.library&key={}&time={}&state={}",
            server_url, id, time, state
        );
        let response = self
            .client
            .get(&url)
            .header("X-Plex-Token", &self.token)
            .send()
            .await
            .context("Failed to update progress")?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to update progress: {}",
                response.status()
            ))
        }
    }
}

