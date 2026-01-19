use crate::traits::MediaSource;
use crate::capabilities::{RatingNormalization, CapabilityRegistry, StatusMapping, IncrementalSync, IdExtraction, IdLookupProvider};
use crate::plex::api::{PlexHttpClient, MovieMetadata, ShowMetadata, WatchlistItem as ApiWatchlistItem, PlayHistoryItem, RatingItem, MetadataItem};
use crate::ProgressTracker;
use anyhow::Result;
use chrono::Utc;
use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem, MediaType, NormalizedStatus, MediaIds};
use media_sync_config::StatusMapping as StatusMappingConfig;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, trace, warn};
use async_trait::async_trait;

pub struct PlexClient {
    token: String,
    server_url: Option<String>,
    authenticated: bool,
    status_mapping: StatusMappingConfig,
    // Cache mapping IMDB ID -> rating_key for efficient lookups
    imdb_to_rating_key_cache: Arc<RwLock<HashMap<String, String>>>,
    // Cache library contents to avoid repeated fetches
    library_movies_cache: Arc<RwLock<HashMap<String, Vec<MovieMetadata>>>>,
    library_shows_cache: Arc<RwLock<HashMap<String, Vec<ShowMetadata>>>>,
    // Cache discovered server URL to avoid repeated discovery
    discovered_server_url: Arc<RwLock<Option<String>>>,
    // Track excluded items (items retrieved but not collected due to unsupported types)
    excluded_items: Arc<RwLock<Vec<(String, Option<String>, String)>>>, // (title, rating_key, type_)
}

impl PlexClient {
    pub fn new(token: String, status_mapping: StatusMappingConfig) -> Self {
        Self::with_server_url(token, None, status_mapping)
    }

    pub fn with_server_url(token: String, server_url: Option<String>, status_mapping: StatusMappingConfig) -> Self {
        Self {
            token,
            server_url,
            authenticated: false,
            status_mapping,
            imdb_to_rating_key_cache: Arc::new(RwLock::new(HashMap::new())),
            library_movies_cache: Arc::new(RwLock::new(HashMap::new())),
            library_shows_cache: Arc::new(RwLock::new(HashMap::new())),
            discovered_server_url: Arc::new(RwLock::new(None)),
            excluded_items: Arc::new(RwLock::new(Vec::new())),
        }
    }
    
    /// Get excluded items from the last collection (items retrieved but not collected)
    pub async fn get_excluded_items(&self) -> Vec<(String, Option<String>, String)> {
        self.excluded_items.read().await.clone()
    }
    
    /// Clear excluded items (call before a new collection)
    pub async fn clear_excluded_items(&self) {
        self.excluded_items.write().await.clear();
    }

    pub async fn authenticate(&mut self) -> Result<()> {
        use media_sync_config::CredentialStore;
        use media_sync_config::PathManager;

        let path_manager = PathManager::default();
        let mut cred_store = CredentialStore::new(path_manager.credentials_file());
        cred_store.load()?;

        // Get token from credentials or use provided token
        let token = if self.token.is_empty() {
            cred_store.get_plex_token()
                .ok_or_else(|| anyhow::anyhow!("Plex token not found in credentials. Run 'totalrecall config plex' first"))?
                .clone()
        } else {
            self.token.clone()
        };

        // Create HTTP client and authenticate to verify token
        let api_client = PlexHttpClient::new(token.clone(), self.server_url.clone())?;
        api_client.authenticate().await?;
        
        self.token = token;
        self.authenticated = true;

        info!("Authenticated to Plex");
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    /// Get API client, ensuring authentication
    async fn get_api_client(&self) -> Result<PlexHttpClient> {
        if !self.authenticated {
            return Err(anyhow::anyhow!("Not authenticated to Plex"));
        }
        PlexHttpClient::new(self.token.clone(), self.server_url.clone())
    }

    /// Get server URL - use configured URL or discover first available server
    /// Caches discovered server URL to avoid repeated discovery calls
    async fn get_server_url(&self) -> Result<String> {
        // First check configured URL
        if let Some(ref server_url) = self.server_url {
            if !server_url.is_empty() {
                debug!("Plex: Using configured server URL: {}", server_url);
                return Ok(server_url.clone());
            }
        }

        // Check cached discovered URL
        {
            let cached = self.discovered_server_url.read().await;
            if let Some(ref url) = *cached {
                debug!("Plex: Using cached discovered server URL: {}", url);
                return Ok(url.clone());
            }
        }

        // Discover servers (only if not cached)
        debug!("Plex: No configured server URL, discovering servers...");
        let client = self.get_api_client().await?;
        let servers = client.get_servers().await?;
        debug!("Plex: Discovered {} servers", servers.len());
        
        if let Some(server) = servers.first() {
            let server_url = server.url.clone();
            debug!("Plex: Using discovered server: {} ({})", server.name, server_url);
            
            // Cache the discovered URL
            {
                let mut cached = self.discovered_server_url.write().await;
                *cached = Some(server_url.clone());
            }
            
            Ok(server_url)
        } else {
            Err(anyhow::anyhow!("No Plex servers available"))
        }
    }

    /// Extract IMDB ID from GUID array
    fn extract_imdb_id_from_guids(guids: &[crate::plex::api::Guid]) -> Option<String> {
        for guid in guids {
            if let Some(imdb_id) = Self::parse_imdb_from_guid(&guid.id) {
                return Some(imdb_id);
            }
        }
        None
    }

    /// Parse IMDB ID from Plex GUID string
    /// GUIDs can be in formats like:
    /// - "imdb://tt1234567"
    /// - "com.plexapp.agents.imdb://tt1234567?lang=en"
    /// - "plex://movie/5d776b5e1e5c36001f8e9b8a" (not IMDB, skip)
    fn parse_imdb_from_guid(guid: &str) -> Option<String> {
        // Look for IMDB pattern in GUID
        if guid.contains("imdb://") {
            // Extract IMDB ID from GUID
            // Pattern: "imdb://tt1234567" or "com.plexapp.agents.imdb://tt1234567?lang=en"
            if let Some(start) = guid.find("imdb://") {
                let imdb_part = &guid[start + 7..]; // Skip "imdb://"
                // Extract IMDB ID (format: tt followed by digits)
                // May have query parameters like "?lang=en", so split on '?' or '&'
                let imdb_id = imdb_part.split('?').next()
                    .and_then(|s| s.split('&').next())
                    .map(|s| s.trim().to_string());
                
                // Validate it looks like an IMDB ID (starts with "tt" followed by digits)
                if let Some(id) = imdb_id {
                    if id.starts_with("tt") && id.len() >= 9 && id[2..].chars().all(|c| c.is_ascii_digit()) {
                        return Some(id);
                    }
                }
            }
        }
        None
    }
    
    /// Parse TMDB ID from Plex GUID string
    fn parse_tmdb_from_guid(guid: &str) -> Option<u32> {
        if guid.contains("tmdb://") {
            if let Some(start) = guid.find("tmdb://") {
                let tmdb_part = &guid[start + 7..];
                let tmdb_id = tmdb_part.split('?').next()
                    .and_then(|s| s.split('&').next())
                    .and_then(|s| s.trim().parse::<u32>().ok());
                return tmdb_id;
            }
        }
        None
    }
    
    /// Parse TVDB ID from Plex GUID string
    fn parse_tvdb_from_guid(guid: &str) -> Option<u32> {
        if guid.contains("tvdb://") {
            if let Some(start) = guid.find("tvdb://") {
                let tvdb_part = &guid[start + 7..];
                let tvdb_id = tvdb_part.split('?').next()
                    .and_then(|s| s.split('&').next())
                    .and_then(|s| s.trim().parse::<u32>().ok());
                return tvdb_id;
            }
        }
        None
    }
    
    /// Extract all IDs from GUID array
    fn extract_ids_from_guids(guids: &[crate::plex::api::Guid]) -> MediaIds {
        let mut media_ids = MediaIds::default();
        
        for guid in guids {
            // Try IMDB
            if media_ids.imdb_id.is_none() {
                if let Some(imdb_id) = Self::parse_imdb_from_guid(&guid.id) {
                    media_ids.imdb_id = Some(imdb_id);
                }
            }
            
            // Try TMDB
            if media_ids.tmdb_id.is_none() {
                if let Some(tmdb_id) = Self::parse_tmdb_from_guid(&guid.id) {
                    media_ids.tmdb_id = Some(tmdb_id);
                }
            }
            
            // Try TVDB
            if media_ids.tvdb_id.is_none() {
                if let Some(tvdb_id) = Self::parse_tvdb_from_guid(&guid.id) {
                    media_ids.tvdb_id = Some(tvdb_id);
                }
            }
        }
        
        media_ids
    }

    /// Extract IMDB ID from metadata item
    fn extract_imdb_id_from_metadata(metadata: &MovieMetadata) -> Option<String> {
        Self::extract_imdb_id_from_guids(&metadata.guids)
    }

    /// Extract all IDs from metadata item
    fn extract_ids_from_metadata(metadata: &MovieMetadata) -> MediaIds {
        Self::extract_ids_from_guids(&metadata.guids)
    }

    /// Extract IMDB ID from show metadata
    fn extract_imdb_id_from_show(metadata: &ShowMetadata) -> Option<String> {
        Self::extract_imdb_id_from_guids(&metadata.guids)
    }

    /// Extract all IDs from show metadata
    fn extract_ids_from_show(metadata: &ShowMetadata) -> MediaIds {
        Self::extract_ids_from_guids(&metadata.guids)
    }

    /// Extract IMDB ID from watchlist item
    fn extract_imdb_id_from_watchlist(item: &ApiWatchlistItem) -> Option<String> {
        Self::extract_imdb_id_from_guids(&item.guids)
    }

    /// Look up IMDB ID via TMDB API when item is not found on Plex server
    /// TMDB API is free and doesn't require an API key for basic searches
    async fn lookup_imdb_id_via_tmdb(title: &str, year: Option<u32>) -> Option<String> {
        use reqwest::Client;
        use urlencoding::encode;
        
        let client = Client::new();
        let encoded_title = encode(title);
        let mut url = format!("https://api.themoviedb.org/3/search/movie?query={}&language=en-US", encoded_title);
        if let Some(y) = year {
            url.push_str(&format!("&year={}", y));
        }
        
        match client.get(&url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    if let Ok(json) = response.json::<serde_json::Value>().await {
                        if let Some(results) = json.get("results").and_then(|r| r.as_array()) {
                            if let Some(first_result) = results.first() {
                                // TMDB returns imdb_id in the external_ids field, but we need to make another call
                                // For now, try to get it from the first result's id and make a details call
                                if let Some(id) = first_result.get("id").and_then(|i| i.as_u64()) {
                                    let details_url = format!("https://api.themoviedb.org/3/movie/{}?append_to_response=external_ids", id);
                                    if let Ok(details_response) = client.get(&details_url).send().await {
                                        if details_response.status().is_success() {
                                            if let Ok(details_json) = details_response.json::<serde_json::Value>().await {
                                                if let Some(imdb_id) = details_json
                                                    .get("external_ids")
                                                    .and_then(|e| e.get("imdb_id"))
                                                    .and_then(|i| i.as_str())
                                                {
                                                    if !imdb_id.is_empty() && imdb_id.starts_with("tt") {
                                                        debug!("TMDB lookup found IMDB ID {} for '{}'", imdb_id, title);
                                                        return Some(imdb_id.to_string());
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
            }
            Err(e) => {
                debug!("TMDB API lookup failed for '{}': {}", title, e);
            }
        }
        None
    }

    /// Match rating_key to library items to get IMDB ID
    /// This is used for play history items which only have rating_key
    async fn get_imdb_id_from_rating_key(&self, rating_key: &str, server_url: &str) -> Option<String> {
        let client = self.get_api_client().await.ok()?;
        
        // Try to get metadata item
        if let Ok(metadata_item) = client.get_metadata_item(server_url, rating_key).await {
            if let Some(imdb_id) = Self::extract_imdb_id_from_guids(&metadata_item.guids) {
                return Some(imdb_id);
            }
            // If metadata item has no IMDB ID in GUIDs, try searching by title
            debug!("Plex: Metadata item '{}' (rating_key: {}) has no IMDB ID in GUIDs, trying search by title", 
                   metadata_item.title, rating_key);
            if let Ok(search_results) = client.search_by_title(server_url, &metadata_item.title, None, "movie").await {
                for result in search_results {
                    if let Some(imdb_id) = Self::extract_imdb_id_from_guids(&result.guids) {
                        debug!("Plex: Found IMDB ID {} for '{}' via search API", imdb_id, metadata_item.title);
                        return Some(imdb_id);
                    }
                }
            }
        } else {
            // Fallback: search through libraries (with caching)
            if let Ok(libraries) = client.get_libraries(server_url).await {
                for library in libraries {
                    if library.type_ == "movie" {
                        // Check cache first
                        let movies = {
                            let cache = self.library_movies_cache.read().await;
                            if let Some(cached) = cache.get(&library.key) {
                                cached.clone()
                            } else {
                                // Cache miss - fetch and cache
                                drop(cache);
                                if let Ok(fetched) = client.get_movies(server_url, &library.key).await {
                                    let mut cache = self.library_movies_cache.write().await;
                                    cache.insert(library.key.clone(), fetched.clone());
                                    fetched
                                } else {
                                    continue;
                                }
                            }
                        };
                        
                        for movie in movies {
                            if movie.rating_key == rating_key {
                                if let Some(imdb_id) = Self::extract_imdb_id_from_metadata(&movie) {
                                    return Some(imdb_id);
                                }
                                // If no IMDB ID in GUIDs, try search API
                                debug!("Plex: Movie '{}' (rating_key: {}) has no IMDB ID in GUIDs, trying search API", 
                                       movie.title, rating_key);
                                if let Ok(search_results) = client.search_by_title(server_url, &movie.title, movie.year, "movie").await {
                                    for result in search_results {
                                        if let Some(imdb_id) = Self::extract_imdb_id_from_guids(&result.guids) {
                                            debug!("Plex: Found IMDB ID {} for '{}' via search API", imdb_id, movie.title);
                                            return Some(imdb_id);
                                        }
                                    }
                                }
                                return None;
                            }
                        }
                    } else if library.type_ == "show" {
                        // Check cache first
                        let shows = {
                            let cache = self.library_shows_cache.read().await;
                            if let Some(cached) = cache.get(&library.key) {
                                cached.clone()
                            } else {
                                // Cache miss - fetch and cache
                                drop(cache);
                                if let Ok(fetched) = client.get_shows(server_url, &library.key).await {
                                    let mut cache = self.library_shows_cache.write().await;
                                    cache.insert(library.key.clone(), fetched.clone());
                                    fetched
                                } else {
                                    continue;
                                }
                            }
                        };
                        
                        for show in shows {
                            if show.rating_key == rating_key {
                                if let Some(imdb_id) = Self::extract_imdb_id_from_show(&show) {
                                    return Some(imdb_id);
                                }
                                // If no IMDB ID in GUIDs, try search API
                                debug!("Plex: Show '{}' (rating_key: {}) has no IMDB ID in GUIDs, trying search API", 
                                       show.title, rating_key);
                                if let Ok(search_results) = client.search_by_title(server_url, &show.title, show.year, "show").await {
                                    for result in search_results {
                                        if let Some(imdb_id) = Self::extract_imdb_id_from_guids(&result.guids) {
                                            debug!("Plex: Found IMDB ID {} for '{}' via search API", imdb_id, show.title);
                                            return Some(imdb_id);
                                        }
                                    }
                                }
                                return None;
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Cache IMDB ID -> rating_key mapping
    async fn cache_imdb_to_rating_key(&self, imdb_id: String, rating_key: String) {
        let mut cache = self.imdb_to_rating_key_cache.write().await;
        cache.insert(imdb_id, rating_key);
    }

    /// Find rating_key from MediaIds, trying multiple ID types (imdb, tmdb, tvdb)
    /// Falls back to discover provider search if local server lookup fails
    /// 
    /// # Arguments
    /// * `ids` - MediaIds to search for
    /// * `server_url` - Plex server URL
    /// * `fallback_title` - Optional fallback title (e.g., from item.title) if ids.title is missing or different
    /// * `fallback_year` - Optional fallback year (e.g., from item.year) if ids.year is missing
    /// * `fallback_media_type` - Optional fallback media_type (e.g., from item.media_type) if ids.media_type is missing
    /// * `require_discover_provider_key` - If true, skip local server lookups and use discover provider search only
    ///                                     (required for watchlist operations which need discover provider metadata keys)
    async fn get_rating_key_from_media_ids(
        &self, 
        ids: &MediaIds, 
        server_url: &str,
        fallback_title: Option<&str>,
        fallback_year: Option<u32>,
        fallback_media_type: Option<&MediaType>,
        require_discover_provider_key: bool,
    ) -> Option<String> {
        // First, check if plex_rating_key is already cached
        if let Some(ref plex_rating_key) = ids.plex_rating_key {
            if !plex_rating_key.is_empty() {
                debug!("Plex: Using cached plex_rating_key: {}", plex_rating_key);
                // If we require discover provider key, check if cached key is in discover provider format
                // Discover provider keys are long hex strings (20+ chars), local server keys are short numeric
                if require_discover_provider_key && plex_rating_key.len() < 20 {
                    debug!("Plex: Cached rating_key '{}' appears to be local server format, will search discover provider", plex_rating_key);
                } else {
                    return Some(plex_rating_key.clone());
                }
            }
        }
        
        // Skip local server lookups if we need discover provider key (for watchlist operations)
        if !require_discover_provider_key {
            // Try imdb_id first (most common)
            if let Some(ref imdb) = ids.imdb_id {
                if !imdb.is_empty() {
                    debug!("Plex: Trying local server lookup for IMDB ID: {} (title: {:?})", imdb, ids.title.as_deref().or(fallback_title));
                    if let Some(rating_key) = self.get_rating_key_from_imdb_id(imdb, server_url).await {
                        debug!("Plex: Found rating_key {} for IMDB ID {} in local server", rating_key, imdb);
                        return Some(rating_key);
                    }
                    debug!("Plex: IMDB ID {} not found in local server, will try discover provider", imdb);
                }
            }
            
            // Try tmdb_id
            if let Some(tmdb) = ids.tmdb_id {
                debug!("Plex: Trying local server lookup for TMDB ID: {} (title: {:?})", tmdb, ids.title.as_deref().or(fallback_title));
                if let Some(rating_key) = self.get_rating_key_from_tmdb_id(tmdb, server_url).await {
                    debug!("Plex: Found rating_key {} for TMDB ID {} in local server", rating_key, tmdb);
                    return Some(rating_key);
                }
                debug!("Plex: TMDB ID {} not found in local server, will try discover provider", tmdb);
            }
            
            // Try tvdb_id
            if let Some(tvdb) = ids.tvdb_id {
                debug!("Plex: Trying local server lookup for TVDB ID: {} (title: {:?})", tvdb, ids.title.as_deref().or(fallback_title));
                if let Some(rating_key) = self.get_rating_key_from_tvdb_id(tvdb, server_url).await {
                    debug!("Plex: Found rating_key {} for TVDB ID {} in local server", rating_key, tvdb);
                    return Some(rating_key);
                }
                debug!("Plex: TVDB ID {} not found in local server, will try discover provider", tvdb);
            }
        } else {
            debug!("Plex: Skipping local server lookups (require_discover_provider_key=true for watchlist operations)");
        }
        
        // All local server lookups failed - try discover provider search
        // Try with ids.title first, then fallback to item.title
        let title = ids.title.as_deref().or(fallback_title);
        let year = ids.year.or(fallback_year);
        // Use fallback media_type if ids.media_type is missing
        let media_type = ids.media_type.as_ref().or(fallback_media_type)?;
        
        debug!("Plex: All local server lookups failed, attempting discover provider search for title: {:?}, year: {:?}, media_type: {:?}", 
               title, year, media_type);
        
        if let Some(title) = title {
            // Try with ids.title (or fallback title)
            debug!("Plex: Discover provider search attempt 1: '{}' (year: {:?})", title, year);
            if let Some(rating_key) = self.get_rating_key_from_discover_provider(
                title, 
                year, 
                media_type
            ).await {
                debug!("Plex: Discover provider search succeeded with rating_key: {}", rating_key);
                return Some(rating_key);
            }
            debug!("Plex: Discover provider search failed for '{}'", title);
            
            // If that failed and we have a different fallback title, try that too
            if let Some(fallback_title) = fallback_title {
                if fallback_title != title {
                    debug!("Plex: Discover provider search attempt 2: '{}' (year: {:?}) - different from ids.title", fallback_title, year);
                    if let Some(rating_key) = self.get_rating_key_from_discover_provider(
                        fallback_title,
                        year,
                        media_type
                    ).await {
                        debug!("Plex: Discover provider search succeeded with fallback title, rating_key: {}", rating_key);
                        return Some(rating_key);
                    }
                    debug!("Plex: Discover provider search failed for fallback title '{}'", fallback_title);
                }
            }
        } else {
            trace!("Plex: Cannot search discover provider - no title available (ids.title: {:?}, fallback_title: {:?})", 
                  ids.title.as_deref(), fallback_title);
        }
        
        debug!("Plex: All lookup methods failed for item (ids.title: {:?}, fallback_title: {:?}, imdb: {:?}, tmdb: {:?})", 
               ids.title.as_deref(), fallback_title, ids.imdb_id, ids.tmdb_id);
        None
    }
    
    /// Search discover provider for rating_key by title/year
    /// Tries with year first if provided, then retries without year if no results
    async fn get_rating_key_from_discover_provider(
        &self,
        title: &str,
        year: Option<u32>,
        media_type: &MediaType,
    ) -> Option<String> {
        let client = self.get_api_client().await.ok()?;
        
        let search_type = match media_type {
            MediaType::Movie => "movie",
            MediaType::Show => "show",
            MediaType::Episode { .. } => return None, // Episodes not supported
        };
        
        // Helper function to process search results
        let process_results = |search_results: Vec<MetadataItem>| -> Option<String> {
            if search_results.is_empty() {
                return None;
            }
            
            // Log first few results for debugging
            for (idx, result) in search_results.iter().take(3).enumerate() {
                debug!("Plex discover provider result[{}]: title='{}', rating_key='{}'", 
                       idx, result.title, result.rating_key);
            }
            
            // Find the best match (exact title match first, case-insensitive)
            for result in &search_results {
                if result.title.eq_ignore_ascii_case(title) {
                    debug!("Plex discover provider: Found exact match (case-insensitive) for '{}' with rating_key: {}", 
                           title, result.rating_key);
                    return Some(result.rating_key.clone());
                }
            }
            
            // If no exact match, use first result (discover provider usually returns best match first)
            if let Some(first_result) = search_results.first() {
                debug!("Plex discover provider: Using first result '{}' (searched for '{}') with rating_key: {}", 
                       first_result.title, title, first_result.rating_key);
                return Some(first_result.rating_key.clone());
            }
            
            None
        };
        
        // First try with year if provided
        if let Some(year_val) = year {
            match client.search_discover_provider(title, Some(year_val), search_type).await {
                Ok(search_results) => {
                    debug!("Plex discover provider search: Found {} results for '{}' (year: {}, type: {})", 
                           search_results.len(), title, year_val, search_type);
                    
                    if let Some(rating_key) = process_results(search_results) {
                        return Some(rating_key);
                    }
                    
                    // No results with year, try without year
                    debug!("Plex discover provider: No results with year {}, trying without year filter", year_val);
                }
                Err(e) => {
                    debug!("Plex discover provider search failed with year for '{}': {}, trying without year", title, e);
                }
            }
        }
        
        // Try without year (or if year search failed)
        match client.search_discover_provider(title, None, search_type).await {
            Ok(search_results) => {
                debug!("Plex discover provider search: Found {} results for '{}' (no year filter, type: {})", 
                       search_results.len(), title, search_type);
                process_results(search_results)
            }
            Err(e) => {
                warn!("Plex discover provider search failed for '{}': {}", title, e);
                None
            }
        }
    }
    
    /// Find rating_key from TMDB ID by searching through libraries
    async fn get_rating_key_from_tmdb_id(&self, tmdb_id: u32, server_url: &str) -> Option<String> {
        let client = self.get_api_client().await.ok()?;
        
        if let Ok(libraries) = client.get_libraries(server_url).await {
            for library in libraries {
                if library.type_ == "movie" {
                    if let Ok(movies) = client.get_movies(server_url, &library.key).await {
                        for movie in movies {
                            let media_ids = Self::extract_ids_from_guids(&movie.guids);
                            if media_ids.tmdb_id == Some(tmdb_id) {
                                return Some(movie.rating_key.clone());
                            }
                        }
                    }
                } else if library.type_ == "show" {
                    if let Ok(shows) = client.get_shows(server_url, &library.key).await {
                        for show in shows {
                            let media_ids = Self::extract_ids_from_guids(&show.guids);
                            if media_ids.tmdb_id == Some(tmdb_id) {
                                return Some(show.rating_key.clone());
                            }
                        }
                    }
                }
            }
        }
        None
    }
    
    /// Find rating_key from TVDB ID by searching through libraries
    async fn get_rating_key_from_tvdb_id(&self, tvdb_id: u32, server_url: &str) -> Option<String> {
        let client = self.get_api_client().await.ok()?;
        
        if let Ok(libraries) = client.get_libraries(server_url).await {
            for library in libraries {
                if library.type_ == "show" {
                    if let Ok(shows) = client.get_shows(server_url, &library.key).await {
                        for show in shows {
                            let media_ids = Self::extract_ids_from_guids(&show.guids);
                            if media_ids.tvdb_id == Some(tvdb_id) {
                                return Some(show.rating_key.clone());
                            }
                        }
                    }
                }
            }
        }
        None
    }
    
    /// Find rating_key from IMDB ID using cache first, then searching through libraries
    /// This is the inverse of get_imdb_id_from_rating_key
    async fn get_rating_key_from_imdb_id(&self, imdb_id: &str, server_url: &str) -> Option<String> {
        // First, check cache
        {
            let cache = self.imdb_to_rating_key_cache.read().await;
            if let Some(rating_key) = cache.get(imdb_id) {
                return Some(rating_key.clone());
            }
        }
        
        // Cache miss - search through libraries
        let client = self.get_api_client().await.ok()?;
        
        if let Ok(libraries) = client.get_libraries(server_url).await {
            for library in libraries {
                // Search movies
                if library.type_ == "movie" {
                    if let Ok(movies) = client.get_movies(server_url, &library.key).await {
                        for movie in movies {
                            if let Some(item_imdb_id) = Self::extract_imdb_id_from_metadata(&movie) {
                                if item_imdb_id == imdb_id {
                                    let rating_key = movie.rating_key.clone();
                                    // Cache the mapping for future use
                                    self.cache_imdb_to_rating_key(item_imdb_id, rating_key.clone()).await;
                                    return Some(rating_key);
                                }
                            }
                        }
                    }
                }
                // Search shows
                else if library.type_ == "show" {
                    if let Ok(shows) = client.get_shows(server_url, &library.key).await {
                        for show in shows {
                            if let Some(item_imdb_id) = Self::extract_imdb_id_from_show(&show) {
                                if item_imdb_id == imdb_id {
                                    let rating_key = show.rating_key.clone();
                                    // Cache the mapping for future use
                                    self.cache_imdb_to_rating_key(item_imdb_id, rating_key.clone()).await;
                                    return Some(rating_key);
                                }
                            }
                        }
                    }
                }
            }
        }
        
        None
    }

    /// Convert API watchlist item to WatchlistItem
    /// Always returns an item, even if it has no IDs (IDs will be resolved later in resolve_missing_ids)
    fn api_watchlist_to_watchlist_item(item: &ApiWatchlistItem) -> WatchlistItem {
        // Extract all IDs from GUIDs
        let media_ids = Self::extract_ids_from_guids(&item.guids);
        
        // Try to extract IMDB ID, but don't fail if not found - use empty string
        let imdb_id = media_ids.imdb_id.clone().unwrap_or_else(|| {
            // Log missing IMDB ID for debugging
            if !item.guids.is_empty() {
                debug!("Plex watchlist: Item '{}' (rating_key: {}) has {} GUIDs but no IMDB ID. GUIDs: {:?}", 
                       item.title, item.rating_key, item.guids.len(),
                       item.guids.iter().map(|g| &g.id).collect::<Vec<_>>());
            }
            String::new()
        });
        
        let media_type = match item.type_.as_str() {
            "movie" => MediaType::Movie,
            "episode" => {
                // For episodes, use Episode type with season/episode numbers
                if let (Some(season), Some(episode_num)) = (item.season, item.episode_number) {
                    MediaType::Episode { season, episode: episode_num }
                } else {
                    MediaType::Show  // Fallback if season/episode not available
                }
            },
            "show" => MediaType::Show,
            _ => MediaType::Movie,
        };

        WatchlistItem {
            imdb_id,
            ids: if media_ids.is_empty() { None } else { Some(media_ids) },
            title: if item.type_ == "episode" {
                // For episodes, prefer episode title, fallback to show title
                item.episode_title.clone().or(item.show_title.clone())
            } else {
                item.title.clone()
            },
            year: item.year,
            media_type,
            date_added: Utc::now(),
            source: "plex".to_string(),
            status: Some(NormalizedStatus::Watchlist),
        }
    }

    /// Convert rating item to Rating
    /// Returns rating even if no IMDB ID is found - IDs can be resolved later
    async fn rating_item_to_rating(&self, item: &RatingItem, _server_url: &str) -> Option<Rating> {
        // Extract all IDs from GUIDs (IMDB, TMDB, TVDB, etc.)
        let media_ids = Self::extract_ids_from_guids(&item.guids);
        
        // Extract IMDB ID for backward compatibility (use empty string if not found)
        let imdb_id = media_ids.imdb_id.clone().unwrap_or_default();
        
        let media_type = match item.type_.as_str() {
            "movie" => MediaType::Movie,
            "episode" => {
                // For episodes, use Episode type with season/episode numbers
                if let (Some(season), Some(episode_num)) = (item.season, item.episode_number) {
                    MediaType::Episode { season, episode: episode_num }
                } else {
                    MediaType::Show  // Fallback if season/episode not available
                }
            },
            "show" => MediaType::Show,
            _ => MediaType::Movie,
        };

        // Convert 0.0-10.0 rating to 1-10 scale (Plex uses 0.0-10.0)
        let rating_10 = item.user_rating.round() as u8;
        if rating_10 == 0 {
            return None; // Skip zero ratings
        }

        // Always include rating, even if no IDs found - IDs can be resolved later
        Some(Rating {
            imdb_id,
            ids: Some(media_ids),
            rating: rating_10,
            date_added: Utc::now(),
            media_type,
            source: media_sync_models::RatingSource::Plex,
        })
    }

    /// Get all IDs from rating_key metadata (IMDB, TMDB, TVDB, etc.)
    /// Returns MediaIds with all available IDs from Plex metadata
    async fn get_all_ids_from_rating_key(&self, rating_key: &str, server_url: &str) -> MediaIds {
        let client = match self.get_api_client().await {
            Ok(c) => c,
            Err(_) => return MediaIds::default(),
        };
        
        // Try to get metadata item
        if let Ok(metadata_item) = client.get_metadata_item(server_url, rating_key).await {
            // Extract all IDs from GUIDs (IMDB, TMDB, TVDB, etc.)
            return Self::extract_ids_from_guids(&metadata_item.guids);
        }
        
        // Fallback: search through libraries (with caching)
        if let Ok(libraries) = client.get_libraries(server_url).await {
            for library in libraries {
                if library.type_ == "movie" {
                    // Check cache first
                    let movies = {
                        let cache = self.library_movies_cache.read().await;
                        if let Some(cached) = cache.get(&library.key) {
                            cached.clone()
                        } else {
                            // Cache miss - fetch and cache
                            drop(cache);
                            if let Ok(fetched) = client.get_movies(server_url, &library.key).await {
                                let mut cache = self.library_movies_cache.write().await;
                                cache.insert(library.key.clone(), fetched.clone());
                                fetched
                            } else {
                                continue;
                            }
                        }
                    };
                    
                    for movie in movies {
                        if movie.rating_key == rating_key {
                            return Self::extract_ids_from_metadata(&movie);
                        }
                    }
                } else if library.type_ == "show" {
                    // Check cache first
                    let shows = {
                        let cache = self.library_shows_cache.read().await;
                        if let Some(cached) = cache.get(&library.key) {
                            cached.clone()
                        } else {
                            // Cache miss - fetch and cache
                            drop(cache);
                            if let Ok(fetched) = client.get_shows(server_url, &library.key).await {
                                let mut cache = self.library_shows_cache.write().await;
                                cache.insert(library.key.clone(), fetched.clone());
                                fetched
                            } else {
                                continue;
                            }
                        }
                    };
                    
                    for show in shows {
                        if show.rating_key == rating_key {
                            return Self::extract_ids_from_show(&show);
                        }
                    }
                }
            }
        }
        
        MediaIds::default()
    }

    async fn play_history_to_watch_history(&self, item: &PlayHistoryItem, _server_url: &str) -> Option<WatchHistory> {
        // Filter out unsupported media types (e.g., "track" for music)
        // Only support "movie" and "show" (episodes)
        let media_type = match item.type_.as_str() {
            "movie" => MediaType::Movie,
            "episode" => {
                // For episodes, use Episode type with season/episode numbers
                if let (Some(season), Some(episode_num)) = (item.season, item.episode_number) {
                    MediaType::Episode { season, episode: episode_num }
                } else {
                    MediaType::Show  // Fallback if season/episode not available
                }
            },
            "show" => MediaType::Show,
            _ => {
                // Unsupported type (e.g., "track", "artist", "album")
                // Return None to filter it out - caller will track excluded items
                return None;
            }
        };

        // Skip all ID lookups during collection to avoid hundreds of API calls.
        // The resolution phase will handle all ID lookups using title/year, which is more efficient:
        // - Uses the ID cache (including title/year index)
        // - Can batch lookups
        // - External lookups (Trakt, Simkl) are more efficient than individual Plex API calls
        // - Avoids redundant requests when the same title appears multiple times
        let mut media_ids = MediaIds::default();
        
        // Preserve episode metadata if available
        if item.type_ == "episode" {
            media_ids.show_title = item.show_title.clone();
            media_ids.episode_title = item.episode_title.clone();
            media_ids.original_air_date = item.original_air_date;
        }
        
        // Extract IMDB ID for backward compatibility
        let imdb_id = media_ids.imdb_id.clone().unwrap_or_default();

        // Always include item, even if no IDs found - IDs can be resolved later using title/year
        Some(WatchHistory {
            imdb_id,
            ids: Some(media_ids),
            title: if item.type_ == "episode" {
                // For episodes, prefer episode title, fallback to show title
                item.episode_title.clone().or(item.show_title.clone())
            } else {
                item.title.clone()
            },
            year: item.year,
            watched_at: item.last_viewed_at,
            media_type,
            source: "plex".to_string(),
        })
    }
}

#[async_trait::async_trait]
impl MediaSource for PlexClient {
    type Error = crate::error::SourceError;

    fn source_name(&self) -> &str {
        "plex"
    }

    async fn authenticate(&mut self) -> Result<(), Self::Error> {
        match self.authenticate().await {
            Ok(()) => Ok(()),
            Err(e) => Err(crate::error::SourceError::new(format!("{}", e))),
        }
    }

    fn is_authenticated(&self) -> bool {
        self.is_authenticated()
    }

    async fn get_watchlist(&self) -> Result<Vec<WatchlistItem>, Self::Error> {
        let client = self.get_api_client().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        let api_items = client.get_watchlist().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        info!("Plex watchlist: Received {} items from API", api_items.len());
        
        let mut watchlist = Vec::new();
        let mut items_without_imdb = 0;
        
        // If we have items without GUIDs, try to get server URL to look them up
        let server_url = self.get_server_url().await.ok();
        if let Some(ref url) = server_url {
            info!("Plex watchlist: Server URL available: {}", url);
        } else {
            warn!("Plex watchlist: No server URL available for IMDB ID lookup");
        }
        
        for item in api_items {
            // Check if item already has an IMDB ID in its GUIDs
            let has_imdb_id = Self::extract_imdb_id_from_watchlist(&item).is_some();
            debug!("Plex watchlist: Processing item '{}' (rating_key: {}), has_imdb_id: {}, guids: {}",
                   item.title, item.rating_key, has_imdb_id, item.guids.len());
            
            // If item has no IMDB ID, try to look it up from the server
            // Discovery API rating_key is often a GUID, so we search by title/year instead
            let mut item_with_guids = item.clone();
            if !has_imdb_id {
                trace!("Plex watchlist: Item '{}' (rating_key: {}) has no IMDB ID, attempting lookup", 
                      item_with_guids.title, item_with_guids.rating_key);
                if let Some(ref server_url) = server_url {
                    trace!("Plex watchlist: Item '{}' (rating_key: {}) has no IMDB ID in GUIDs, searching server by title/year", 
                           item_with_guids.title, item_with_guids.rating_key);
                    
                    // Check if rating_key looks like a server rating_key (numeric or /library/metadata/...)
                    // Discovery API uses GUIDs like "plex://movie/..." which won't work with server API
                    let rating_key_is_server_format = item_with_guids.rating_key.parse::<u32>().is_ok() ||
                                                       item_with_guids.rating_key.starts_with("/library/metadata/");
                    
                    let mut found_via_rating_key = false;
                    if rating_key_is_server_format && !item_with_guids.rating_key.is_empty() {
                        // Only try direct lookup if rating_key looks like a server format
                        match client.get_metadata_item(server_url, &item_with_guids.rating_key).await {
                            Ok(metadata_item) => {
                                if !metadata_item.guids.is_empty() {
                                    debug!("Plex watchlist: Fetched metadata for '{}' via rating_key, found {} GUIDs", 
                                           item_with_guids.title, metadata_item.guids.len());
                                    item_with_guids.guids = metadata_item.guids;
                                    found_via_rating_key = true;
                                }
                            }
                            Err(e) => {
                                debug!("Plex watchlist: Failed to fetch metadata via rating_key '{}': {}", 
                                       item_with_guids.rating_key, e);
                            }
                        }
                    } else {
                        trace!("Plex watchlist: Rating_key '{}' appears to be a GUID (Discovery API format), skipping direct lookup, will search by title/year", 
                               item_with_guids.rating_key);
                    }
                    
                    // If rating_key lookup failed or wasn't attempted, try searching by title/year
                    if !found_via_rating_key {
                        debug!("Plex watchlist: Item '{}' (rating_key: {}) not found via rating_key, searching server libraries", 
                               item_with_guids.title, item_with_guids.rating_key);
                        
                        // Try to find the item in server libraries by title/year
                        trace!("Plex watchlist: Searching server libraries for '{}' (year: {:?})", 
                              item_with_guids.title, item_with_guids.year);
                        match client.get_libraries(server_url).await {
                            Ok(libraries) => {
                                trace!("Plex watchlist: Found {} libraries to search", libraries.len());
                                let mut found = false;
                                for library in libraries {
                                if library.type_ == "movie" {
                                    // Check cache first
                                    let movies = {
                                        let cache = self.library_movies_cache.read().await;
                                        if let Some(cached) = cache.get(&library.key) {
                                            debug!("Plex watchlist: Cache hit for movies in library '{}'", library.title);
                                            cached.clone()
                                        } else {
                                            // Cache miss - fetch and cache
                                            drop(cache);
                                            match client.get_movies(server_url, &library.key).await {
                                                Ok(fetched) => {
                                                    let mut cache = self.library_movies_cache.write().await;
                                                    cache.insert(library.key.clone(), fetched.clone());
                                                    debug!("Plex watchlist: Cache miss for movies in library '{}', fetched {} items", library.title, fetched.len());
                                                    fetched
                                                }
                                                Err(e) => {
                                                    debug!("Plex watchlist: Failed to get movies from library '{}': {}", library.title, e);
                                                    continue;
                                                }
                                            }
                                        }
                                    };
                                    
                                    debug!("Plex watchlist: Searching {} movies in library '{}'", movies.len(), library.title);
                                    for movie in movies {
                                        // Priority 1: Check IMDB ID match first (if available)
                                        let imdb_match = if let Some(existing_imdb) = Self::extract_imdb_id_from_guids(&item_with_guids.guids) {
                                            if let Some(movie_imdb) = Self::extract_imdb_id_from_guids(&movie.guids) {
                                                existing_imdb == movie_imdb
                                            } else {
                                                false
                                            }
                                        } else {
                                            false // No existing IMDB ID to validate against
                                        };
                                        
                                        if imdb_match {
                                            // IMDB ID match - accept immediately
                                            trace!("Plex watchlist: Found IMDB ID match '{}' (IMDB: {}) in library", 
                                                   movie.title, 
                                                   Self::extract_imdb_id_from_guids(&movie.guids).unwrap_or_default());
                                            item_with_guids.guids = movie.guids;
                                            found = true;
                                            break;
                                        }
                                        
                                        // Priority 2: Check title and year match
                                        let title_match = movie.title == item_with_guids.title;
                                        let year_match = match (item_with_guids.year, movie.year) {
                                            (Some(watchlist_year), Some(movie_year)) => watchlist_year == movie_year,
                                            (None, _) | (_, None) => true,
                                        };
                                        
                                        if title_match && year_match {
                                            // Title/year match - accept
                                            trace!("Plex watchlist: Found matching movie '{}' (year: {:?}) in library, found {} GUIDs", 
                                                   movie.title, movie.year, movie.guids.len());
                                            item_with_guids.guids = movie.guids;
                                            found = true;
                                            break;
                                        }
                                    }
                                } else if library.type_ == "show" {
                                    // Check cache first
                                    let shows = {
                                        let cache = self.library_shows_cache.read().await;
                                        if let Some(cached) = cache.get(&library.key) {
                                            debug!("Plex watchlist: Cache hit for shows in library '{}'", library.title);
                                            cached.clone()
                                        } else {
                                            // Cache miss - fetch and cache
                                            drop(cache);
                                            match client.get_shows(server_url, &library.key).await {
                                                Ok(fetched) => {
                                                    let mut cache = self.library_shows_cache.write().await;
                                                    cache.insert(library.key.clone(), fetched.clone());
                                                    debug!("Plex watchlist: Cache miss for shows in library '{}', fetched {} items", library.title, fetched.len());
                                                    fetched
                                                }
                                                Err(e) => {
                                                    debug!("Plex watchlist: Failed to get shows from library '{}': {}", library.title, e);
                                                    continue;
                                                }
                                            }
                                        }
                                    };
                                    
                                    debug!("Plex watchlist: Searching {} shows in library '{}'", shows.len(), library.title);
                                    for show in shows {
                                        // Priority 1: Check IMDB ID match first (if available)
                                        let imdb_match = if let Some(existing_imdb) = Self::extract_imdb_id_from_guids(&item_with_guids.guids) {
                                            if let Some(show_imdb) = Self::extract_imdb_id_from_guids(&show.guids) {
                                                existing_imdb == show_imdb
                                            } else {
                                                false
                                            }
                                        } else {
                                            false // No existing IMDB ID to validate against
                                        };
                                        
                                        if imdb_match {
                                            // IMDB ID match - accept immediately
                                            trace!("Plex watchlist: Found IMDB ID match '{}' (IMDB: {}) in library", 
                                                   show.title, 
                                                   Self::extract_imdb_id_from_guids(&show.guids).unwrap_or_default());
                                            item_with_guids.guids = show.guids;
                                            found = true;
                                            break;
                                        }
                                        
                                        // Priority 2: Check title and year match
                                        let title_match = show.title == item_with_guids.title;
                                        let year_match = match (item_with_guids.year, show.year) {
                                            (Some(watchlist_year), Some(show_year)) => watchlist_year == show_year,
                                            (None, _) | (_, None) => true, // If either is missing, don't filter by year
                                        };
                                        
                                        if title_match && year_match {
                                            // Title/year match - accept
                                            debug!("Plex watchlist: Found matching show '{}' (year: {:?}) in library, found {} GUIDs", 
                                                   show.title, show.year, show.guids.len());
                                            item_with_guids.guids = show.guids;
                                            found = true;
                                            break;
                                        }
                                    }
                                }
                                if found {
                                    break;
                                }
                            }
                            if !found {
                                trace!("Plex watchlist: Could not find matching item '{}' (year: {:?}) in any server library, trying search API", 
                                       item_with_guids.title, item_with_guids.year);
                                
                                // Fallback: Use Plex search API to find the item
                                match client.search_by_title(server_url, &item_with_guids.title, item_with_guids.year, &item_with_guids.type_).await {
                                    Ok(search_results) => {
                                        trace!("Plex watchlist: Search API returned {} results for '{}'", search_results.len(), item_with_guids.title);
                                        // Find the best match (exact title match, prefer year match if available)
                                        let mut best_match: Option<&MetadataItem> = None;
                                        for result in &search_results {
                                            let title_match = result.title == item_with_guids.title;
                                            if title_match {
                                                debug!("Plex watchlist: Search API found matching item '{}' with {} GUIDs", 
                                                       result.title, result.guids.len());
                                                best_match = Some(result);
                                                break;
                                            }
                                        }
                                        if let Some(match_result) = best_match {
                                            trace!("Plex watchlist: Search API found exact match '{}' with {} GUIDs", 
                                                  match_result.title, match_result.guids.len());
                                            item_with_guids.guids = match_result.guids.clone();
                                            found = true;
                                        } else if !search_results.is_empty() {
                                            // Priority 1: Check IMDB ID match first (if available)
                                            let mut imdb_match: Option<&MetadataItem> = None;
                                            if let Some(existing_imdb) = Self::extract_imdb_id_from_guids(&item_with_guids.guids) {
                                                for result in &search_results {
                                                    if let Some(result_imdb) = Self::extract_imdb_id_from_guids(&result.guids) {
                                                        if existing_imdb == result_imdb {
                                                            imdb_match = Some(result);
                                                            break; // IMDB ID match found - no further validation needed
                                                        }
                                                    }
                                                }
                                            }
                                            
                                            if let Some(match_result) = imdb_match {
                                                trace!("Plex watchlist: Search API found IMDB ID match '{}' (IMDB: {}) with {} GUIDs", 
                                                       match_result.title, 
                                                       Self::extract_imdb_id_from_guids(&match_result.guids).unwrap_or_default(),
                                                       match_result.guids.len());
                                                item_with_guids.guids = match_result.guids.clone();
                                                found = true;
                                            } else {
                                                // Priority 2: Try to find a match that validates against title and year
                                                let mut validated_match: Option<&MetadataItem> = None;
                                                
                                                for result in &search_results {
                                                    // Check title match
                                                    let title_match = result.title == item_with_guids.title;
                                                    
                                                    // Check year match if both are available
                                                    let year_match = match (item_with_guids.year, result.year) {
                                                        (Some(watchlist_year), Some(result_year)) => watchlist_year == result_year,
                                                        (None, _) | (_, None) => true, // If either is missing, don't filter by year
                                                    };
                                                    
                                                    if title_match && year_match {
                                                        validated_match = Some(result);
                                                        break;
                                                    }
                                                }
                                                
                                                if let Some(match_result) = validated_match {
                                                    trace!("Plex watchlist: Search API found title/year match '{}' (year: {:?}) with {} GUIDs", 
                                                           match_result.title, match_result.year, match_result.guids.len());
                                                    item_with_guids.guids = match_result.guids.clone();
                                                    found = true;
                                                } else {
                                                    warn!("Plex watchlist: Search API returned {} results for '{}' but none passed validation (no IMDB ID match, no title/year match)", 
                                                          search_results.len(), item_with_guids.title);
                                                    // Don't use fuzzy match - let it be resolved later via ID resolution
                                                }
                                            }
                                        } else {
                                            trace!("Plex watchlist: Search API returned no results for '{}', trying TMDB lookup", item_with_guids.title);
                                            
                                            // Final fallback: Use TMDB API to look up IMDB ID by title/year
                                            if let Some(imdb_id) = Self::lookup_imdb_id_via_tmdb(&item_with_guids.title, item_with_guids.year).await {
                                                // Create a fake GUID with the IMDB ID so it gets extracted
                                                item_with_guids.guids.push(crate::plex::api::Guid {
                                                    id: format!("imdb://{}", imdb_id),
                                                });
                                                found = true;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        debug!("Plex watchlist: Search API failed for '{}': {}, trying TMDB lookup", item_with_guids.title, e);
                                        
                                        // Fallback: Use TMDB API to look up IMDB ID by title/year
                                        if let Some(imdb_id) = Self::lookup_imdb_id_via_tmdb(&item_with_guids.title, item_with_guids.year).await {
                                            // Create a fake GUID with the IMDB ID so it gets extracted
                                            item_with_guids.guids.push(crate::plex::api::Guid {
                                                id: format!("imdb://{}", imdb_id),
                                            });
                                            found = true;
                                        }
                                    }
                                }
                            }
                            }
                            Err(e) => {
                                debug!("Plex watchlist: Failed to get libraries from server: {}, trying TMDB lookup", e);
                                
                                // Fallback: Use TMDB API to look up IMDB ID by title/year
                                if let Some(imdb_id) = Self::lookup_imdb_id_via_tmdb(&item_with_guids.title, item_with_guids.year).await {
                                    // Create a fake GUID with the IMDB ID so it gets extracted
                                    item_with_guids.guids.push(crate::plex::api::Guid {
                                        id: format!("imdb://{}", imdb_id),
                                    });
                                }
                            }
                        }
                    }
                } else {
                    debug!("Plex watchlist: Item '{}' has no IMDB ID and no server URL available for lookup, trying TMDB lookup", item_with_guids.title);
                    
                    // Fallback: Use TMDB API to look up IMDB ID by title/year
                    if let Some(imdb_id) = Self::lookup_imdb_id_via_tmdb(&item_with_guids.title, item_with_guids.year).await {
                        // Create a fake GUID with the IMDB ID so it gets extracted
                        item_with_guids.guids.push(crate::plex::api::Guid {
                            id: format!("imdb://{}", imdb_id),
                        });
                    }
                }
            }
            
            // Convert to watchlist item (always succeeds, even if no IDs - IDs will be resolved later)
            let watchlist_item = Self::api_watchlist_to_watchlist_item(&item_with_guids);
            
            // Cache the IMDB ID -> rating_key mapping if we have an IMDB ID
            if !watchlist_item.imdb_id.is_empty() {
                self.cache_imdb_to_rating_key(watchlist_item.imdb_id.clone(), item_with_guids.rating_key.clone()).await;
            } else {
                items_without_imdb += 1;
                trace!("Plex watchlist item has no IMDB ID (rating_key: '{}', title: '{}', GUIDs: {:?}) - will be resolved later", 
                       item_with_guids.rating_key, item_with_guids.title,
                       item_with_guids.guids.iter().map(|g| &g.id).collect::<Vec<_>>());
            }
            
            // Always add to watchlist, even without IMDB ID - cache should contain all data
            // IDs will be resolved later in resolve_missing_ids
            watchlist.push(watchlist_item);
        }
        
        info!("Plex watchlist collection: {} items collected, {} items without IMDB ID", watchlist.len(), items_without_imdb);
        Ok(watchlist)
    }

    async fn get_ratings(&self) -> Result<Vec<Rating>, Self::Error> {
        // Ratings require a server URL - if we can't get one, return empty (ratings are server-only)
        let server_url = match self.get_server_url().await {
            Ok(url) => url,
            Err(e) => {
                warn!("Plex ratings: No server available ({}). Ratings are stored on your Plex server, not in the cloud. Configure a server URL or ensure your server is accessible.", e);
                return Ok(Vec::new());
            }
        };
        
        let client = self.get_api_client().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        let rating_items = client.get_ratings(&server_url).await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        let mut ratings = Vec::new();
        let total_items = rating_items.len();
        let mut items_without_imdb = 0;
        
        for item in rating_items {
            if let Some(rating) = self.rating_item_to_rating(&item, &server_url).await {
                // Cache the IMDB ID -> rating_key mapping if we have an IMDB ID
                if !rating.imdb_id.is_empty() {
                    self.cache_imdb_to_rating_key(rating.imdb_id.clone(), item.rating_key.clone()).await;
                } else {
                    items_without_imdb += 1;
                    if items_without_imdb <= 5 {
                        debug!("Plex rating has no IMDB ID (rating_key: '{}')", item.rating_key);
                    }
                }
                // Always add rating, even without IMDB ID - IDs can be resolved later
                ratings.push(rating);
            }
        }
        
        info!("Plex ratings collection: {} total items, {} ratings collected, {} items without IMDB ID", 
              total_items, ratings.len(), items_without_imdb);
        
        Ok(ratings)
    }

    async fn get_reviews(&self) -> Result<Vec<Review>, Self::Error> {
        // Reviews are not yet fully implemented
        Ok(vec![])
    }

    async fn get_watch_history(&self) -> Result<Vec<WatchHistory>, Self::Error> {
        // Watch history requires a server URL - if we can't get one, return empty (history is server-only)
        let server_url = match self.get_server_url().await {
            Ok(url) => url,
            Err(e) => {
                warn!("Plex watch history: No server available ({}). Watch history is stored on your Plex server, not in the cloud. Configure a server URL or ensure your server is accessible.", e);
                return Ok(Vec::new());
            }
        };
        
        let client = self.get_api_client().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        let play_history = client.get_play_history(&server_url).await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        // Clear excluded items from previous collection
        self.clear_excluded_items().await;
        
        let mut history = Vec::new();
        let total_items = play_history.len();
        let mut items_without_imdb = 0;
        
        let mut items_filtered = 0;
        for item in play_history {
            if let Some(history_item) = self.play_history_to_watch_history(&item, &server_url).await {
                // Cache the IMDB ID -> rating_key mapping if we have an IMDB ID
                if !history_item.imdb_id.is_empty() {
                    self.cache_imdb_to_rating_key(history_item.imdb_id.clone(), item.rating_key.clone()).await;
                } else {
                    items_without_imdb += 1;
                    if items_without_imdb <= 5 {
                        debug!("Plex watch history item has no IMDB ID (rating_key: '{}')", item.rating_key);
                    }
                }
                // Always add to history, even without IMDB ID - cache should contain all data
                history.push(history_item);
            } else {
                items_filtered += 1;
                // Track excluded items (unsupported media types like "track")
                let excluded_title = item.title.clone().unwrap_or_else(|| "unknown".to_string());
                let excluded_rating_key = if item.rating_key.is_empty() { None } else { Some(item.rating_key.clone()) };
                {
                    let mut excluded = self.excluded_items.write().await;
                    excluded.push((excluded_title, excluded_rating_key, item.type_.clone()));
                }
                if items_filtered <= 5 {
                    debug!("Plex watch history: Item filtered out (type: '{}', rating_key: '{}', title: '{:?}')", 
                           item.type_, item.rating_key, item.title);
                }
            }
        }
        
        if items_filtered > 0 {
            warn!("Plex watch history: {} items were filtered out (unsupported media types like 'track')", items_filtered);
        }
        
        // Save excluded items to cache (collect phase - unsupported media types)
        let excluded_raw = self.get_excluded_items().await;
        if !excluded_raw.is_empty() {
            use media_sync_config::PathManager;
            use media_sync_models::ExcludedItem;
            use serde_json;
            use std::fs;
            
            let path_manager = PathManager::default();
            // Excluded items from collect phase go to cache/collect/plex/excluded.json
            let excluded_path = path_manager.cache_collect_dir().join("plex").join("excluded.json");
            
            let excluded: Vec<ExcludedItem> = excluded_raw
                .into_iter()
                .map(|(title, rating_key, type_)| ExcludedItem {
                    title: Some(title),
                    imdb_id: None, // Excluded items are unsupported types, so they don't have IMDB IDs
                    rating_key,
                    media_type: type_.clone(),
                    reason: format!("Unsupported media type: {}", type_),
                    source: "plex".to_string(),
                    date_added: None, // Not a watchlist item, so no date_added
                })
                .collect();
            
            // Ensure parent directory exists
            if let Some(parent) = excluded_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    warn!("Failed to create directory for Plex excluded items cache: {}", e);
                }
            }
            
            // Save excluded items to JSON file
            if let Ok(json) = serde_json::to_string_pretty(&excluded) {
                if let Err(e) = fs::write(&excluded_path, json) {
                    warn!("Failed to save Plex excluded items to cache: {}", e);
                } else {
                    info!("Saved {} excluded items for Plex to {:?} (unsupported media types)", excluded.len(), excluded_path);
                }
            } else {
                warn!("Failed to serialize Plex excluded items");
            }
        }
        
        info!("Plex watch history collection: {} total items, {} history items collected, {} items without IMDB ID, {} items excluded", 
              total_items, history.len(), items_without_imdb, items_filtered);
        
        if history.is_empty() && total_items > 0 {
            warn!("Plex watch history: WARNING - {} play history items were fetched but 0 were converted to WatchHistory. This suggests play_history_to_watch_history is filtering out all items.", total_items);
        }
        
        Ok(history)
    }

    async fn add_to_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error> {
        let server_url = self.get_server_url().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        let client = self.get_api_client().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        if items.is_empty() {
            return Ok(());
        }

        let progress_interval = if items.len() < 50 { 10 } else { 50 };
        let mut tracker = ProgressTracker::with_operation_name(
            items.len(),
            progress_interval,
            Some("Plex watchlist add".to_string()),
        );
        let mut added_count = 0;
        let mut not_found_count = 0;
        
        for (idx, item) in items.iter().enumerate() {
            let current = idx + 1;
            // Try to get rating_key from MediaIds (checks plex_rating_key first, then tries imdb, tmdb, tvdb, then discover provider)
            // For watchlist operations, we need discover provider metadata keys, not local server keys
            let rating_key = if let Some(ref media_ids) = item.ids {
                // Pass item.title, item.year, and item.media_type as fallback in case ids fields are missing or different
                // require_discover_provider_key=true because watchlist API needs discover provider metadata keys
                self.get_rating_key_from_media_ids(
                    media_ids, 
                    &server_url,
                    Some(&item.title),  // Fallback to item title
                    item.year,          // Fallback to item year
                    Some(&item.media_type),  // Fallback to item media_type
                    true  // Require discover provider key for watchlist operations
                ).await
            } else {
                // If no MediaIds, try discover provider with item title/year
                self.get_rating_key_from_discover_provider(
                    &item.title,
                    item.year,
                    &item.media_type
                ).await
            };
            
            if let Some(rating_key) = rating_key {
                // Media Provider watchlist endpoint uses ratingKey parameter
                match client.add_to_watchlist(&rating_key).await {
                    Ok(_) => {
                        trace!("Added '{}' to Plex watchlist (rating_key: {})", item.title, rating_key);
                        tracker.record_added();
                        added_count += 1;
                    }
                    Err(e) => {
                        warn!("Failed to add '{}' to Plex watchlist: {}", item.title, e);
                        tracker.record_failed_with_error("Plex: add_to_watchlist_api_error");
                    }
                }
            } else {
                warn!("Could not find Plex rating_key for '{}' (ids.title: {:?}) - item may not be in Plex library or discover provider", 
                      item.title, item.ids.as_ref().and_then(|ids| ids.title.as_ref()));
                tracker.record_failed_with_error("Plex: rating_key_not_found");
                not_found_count += 1;
            }

            tracker.log_progress(current);
        }
        
        tracker.log_summary("Plex watchlist add");
        Ok(())
    }

    async fn remove_from_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error> {
        let server_url = self.get_server_url().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        let client = self.get_api_client().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        if items.is_empty() {
            return Ok(());
        }

        let progress_interval = if items.len() < 50 { 10 } else { 50 };
        let mut tracker = ProgressTracker::with_operation_name(
            items.len(),
            progress_interval,
            Some("Plex watchlist remove".to_string()),
        );
        
        for (idx, item) in items.iter().enumerate() {
            let current = idx + 1;
            // Try to get rating_key from MediaIds (checks plex_rating_key first, then tries imdb, tmdb, tvdb, then discover provider)
            // For watchlist operations, we need discover provider metadata keys, not local server keys
            let rating_key = if let Some(ref media_ids) = item.ids {
                // Pass item.title, item.year, and item.media_type as fallback in case ids fields are missing or different
                // require_discover_provider_key=true because watchlist API needs discover provider metadata keys
                self.get_rating_key_from_media_ids(
                    media_ids, 
                    &server_url,
                    Some(&item.title),  // Fallback to item title
                    item.year,          // Fallback to item year
                    Some(&item.media_type),  // Fallback to item media_type
                    true  // Require discover provider key for watchlist operations
                ).await
            } else {
                // Fallback to imdb_id if MediaIds not available
                if !item.imdb_id.is_empty() {
                    self.get_rating_key_from_imdb_id(&item.imdb_id, &server_url).await
                } else {
                    None
                }
            };
            
            if let Some(rating_key) = rating_key {
                match client.remove_from_watchlist(&rating_key).await {
                    Ok(_) => {
                        trace!("Removed '{}' ({}) from Plex watchlist", item.title, item.imdb_id);
                        tracker.record_added();
                    }
                    Err(e) => {
                        warn!("Failed to remove '{}' from Plex watchlist: {}", item.title, e);
                        tracker.record_failed_with_error("Plex: remove_from_watchlist_api_error");
                    }
                }
            } else {
                warn!("Could not find Plex rating_key for '{}' (IMDB: {}) - item may not be in Plex library or discover provider", item.title, item.imdb_id);
                tracker.record_failed_with_error("Plex: rating_key_not_found");
            }

            tracker.log_progress(current);
        }
        
        tracker.log_summary("Plex watchlist remove");
        Ok(())
    }

    async fn set_ratings(&self, ratings: &[Rating]) -> Result<(), Self::Error> {
        let server_url = self.get_server_url().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        let client = self.get_api_client().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        if ratings.is_empty() {
            return Ok(());
        }

        let progress_interval = if ratings.len() < 50 { 10 } else { 50 };
        let mut tracker = ProgressTracker::with_operation_name(
            ratings.len(),
            progress_interval,
            Some("Plex ratings set".to_string()),
        );
        
        for (idx, rating) in ratings.iter().enumerate() {
            let current = idx + 1;
            // Try to get rating_key from MediaIds (tries imdb, tmdb, tvdb in order)
            // Rating doesn't have title/year/media_type fields, so we pass None for fallback
            // require_discover_provider_key=false because ratings can use local server keys
            let rating_key = if let Some(ref media_ids) = rating.ids {
                self.get_rating_key_from_media_ids(media_ids, &server_url, None, None, None, false).await
            } else {
                // Fallback to imdb_id if MediaIds not available
                if !rating.imdb_id.is_empty() {
                    self.get_rating_key_from_imdb_id(&rating.imdb_id, &server_url).await
                } else {
                    None
                }
            };
            
            if let Some(rating_key) = rating_key {
                // Detect if this is a Discover provider key (long hex string, 20+ chars) 
                // vs local library key (short numeric, typically < 10 chars)
                let is_discover_key = rating_key.len() >= 20 && rating_key.chars().all(|c| c.is_ascii_hexdigit());
                
                if is_discover_key {
                    // Discover provider keys don't work with local server rating endpoint (produces 500 errors)
                    // Skip silently to avoid noise, but track as skipped
                    trace!("Skipping rating for Discover provider item (rating_key={}): not supported by local server rating endpoint", rating_key);
                    tracker.record_skipped();
                } else {
                    // Convert from 1-10 scale (stored) to 0-10 scale (Plex API)
                    // Plex API accepts 0-10, but we store as 1-10 to match other sources
                    let rating_value = if rating.rating > 0 {
                        (rating.rating - 1) as f64
                    } else {
                        0.0
                    };
                    
                    match client.set_rating(&server_url, &rating_key, rating_value).await {
                        Ok(_) => {
                            trace!("Set rating {} on Plex", rating.rating);
                            tracker.record_added();
                        }
                        Err(e) => {
                            warn!("Failed to set rating: {}", e);
                            tracker.record_failed_with_error("Plex: set_rating_api_error");
                        }
                    }
                }
            } else {
                warn!("Could not find Plex rating_key for rating - item may not be in Plex library");
                tracker.record_failed_with_error("Plex: rating_key_not_found");
            }

            tracker.log_progress(current);
        }
        
        tracker.log_summary("Plex ratings set");
        Ok(())
    }

    async fn set_reviews(&self, reviews: &[Review]) -> Result<(), Self::Error> {
        let server_url = self.get_server_url().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        let client = self.get_api_client().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        info!("Plex: Starting to process {} reviews", reviews.len());
        
        let mut success_count = 0;
        let mut not_found_count = 0;
        let mut error_count = 0;
        
        for review in reviews {
            // Try to get rating_key from MediaIds (checks plex_rating_key first, then tries imdb, tmdb, tvdb, then discover provider)
            // Review has media_type field, and MediaIds may have title/year, so pass those as fallbacks
            // require_discover_provider_key=false because reviews can use local server keys
            let rating_key = if let Some(ref media_ids) = review.ids {
                // Use ids.title/year if available, otherwise None (Review doesn't have title/year fields)
                self.get_rating_key_from_media_ids(
                    media_ids, 
                    &server_url, 
                    media_ids.title.as_deref(),  // Use ids.title if available
                    media_ids.year,              // Use ids.year if available
                    Some(&review.media_type),    // Review DOES have media_type field
                    false
                ).await
            } else {
                // Fallback to imdb_id if MediaIds not available
                if !review.imdb_id.is_empty() {
                    self.get_rating_key_from_imdb_id(&review.imdb_id, &server_url).await
                } else {
                    None
                }
            };
            
            if let Some(rating_key) = rating_key {
                // Use Timeline API on local server (same as mark_watched)
                let review_text = review.content.as_str();
                match client.set_review(&server_url, &rating_key, review_text).await {
                    Ok(_) => {
                        info!("Plex: Successfully set review for '{}' (imdb_id={}) on Plex", review.imdb_id, review.imdb_id);
                        success_count += 1;
                    }
                    Err(e) => {
                        warn!("Plex: Failed to set review for '{}' (imdb_id={}) on Plex: {}", review.imdb_id, review.imdb_id, e);
                        error_count += 1;
                        // Continue processing other reviews instead of returning
                    }
                }
            } else {
                warn!("Plex: Could not find Plex rating_key for '{}' - item may not be in Plex library or discover provider", review.imdb_id);
                not_found_count += 1;
            }
        }
        
        info!("Plex: Completed processing reviews: {} total items, {} succeeded, {} failed (not found), {} failed (API error)", 
              reviews.len(), success_count, not_found_count, error_count);
        
        // Return error only if ALL reviews failed
        if success_count == 0 && !reviews.is_empty() {
            Err(crate::error::SourceError::new(format!(
                "Failed to set any reviews: {} not found, {} API errors",
                not_found_count, error_count
            )))
        } else {
            Ok(())
        }
    }

    async fn add_watch_history(&self, items: &[WatchHistory]) -> Result<(), Self::Error> {
        let server_url = self.get_server_url().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        let client = self.get_api_client().await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        if items.is_empty() {
            return Ok(());
        }

        let progress_interval = if items.len() < 50 { 10 } else { 50 };
        let mut tracker = ProgressTracker::with_operation_name(
            items.len(),
            progress_interval,
            Some("Plex watch history add".to_string()),
        );
        let mut success_count = 0;
        let mut not_found_count = 0;
        let mut error_count = 0;
        
        for (idx, item) in items.iter().enumerate() {
            let current = idx + 1;
            // Try to get the best available title: prefer item.title, then ids.title
            let title = item.title.as_deref()
                .or_else(|| item.ids.as_ref().and_then(|ids| ids.title.as_deref()));
            
            // Try to get the best available year: prefer item.year, then ids.year
            let year = item.year
                .or_else(|| item.ids.as_ref().and_then(|ids| ids.year));
            
            trace!("Plex: Processing watch history item {}/{}: imdb_id={}, title={:?}, year={:?}, media_type={:?}",
                  current, items.len(), item.imdb_id, title, year, item.media_type);
            
            // Try to get rating_key from MediaIds (checks plex_rating_key first, then tries imdb, tmdb, tvdb, then discover provider)
            // require_discover_provider_key=true because mark_watched uses discover provider API
            // which requires discover provider metadata keys, not local server keys
            let rating_key = if let Some(ref media_ids) = item.ids {
                debug!("Plex: Attempting to get rating_key from MediaIds for imdb_id={}", item.imdb_id);
                // Pass the best available title and year as fallback
                // get_rating_key_from_media_ids will use ids.title first, then fallback_title
                self.get_rating_key_from_media_ids(
                    media_ids, 
                    &server_url,
                    title,  // Best available title (from item.title or ids.title)
                    year,   // Best available year (from item.year or ids.year)
                    Some(&item.media_type),  // Fallback to item media_type
                    true  // Require discover provider key because mark_watched uses discover provider API
                ).await
            } else {
                debug!("Plex: No MediaIds available, falling back to imdb_id lookup for imdb_id={}", item.imdb_id);
                // Fallback to imdb_id if MediaIds not available
                if !item.imdb_id.is_empty() {
                    self.get_rating_key_from_imdb_id(&item.imdb_id, &server_url).await
                } else {
                    None
                }
            };
            
            if let Some(rating_key) = rating_key {
                trace!("Plex: Found rating_key '{}' for imdb_id={}, calling mark_watched", rating_key, item.imdb_id);
                // Use Timeline API scrobble endpoint (PUT /:/scrobble?identifier={identifier}&key={key})
                // This works for both local library items and discover provider items
                match client.mark_watched(&server_url, &rating_key).await {
                    Ok(_) => {
                        trace!("Plex: Successfully marked '{}' (imdb_id={}) as watched on Plex", 
                              title.unwrap_or("unknown"), item.imdb_id);
                        tracker.record_added();
                        success_count += 1;
                    }
                    Err(e) => {
                        // Check if this is a local library item (short numeric rating_key)
                        // Local library items should warn, Discover items should trace
                        let is_local_library = rating_key.len() < 20 && rating_key.parse::<u32>().is_ok();
                        if is_local_library {
                            warn!("Plex: Failed to mark '{}' (imdb_id={}) as watched on Plex (local library item): {}", 
                                  title.unwrap_or("unknown"), item.imdb_id, e);
                            tracker.record_failed_with_error("Plex: mark_watched_api_error");
                        } else {
                            trace!("Plex: Failed to mark '{}' (imdb_id={}) as watched on Plex (discover item, expected to fail): {}", 
                                  title.unwrap_or("unknown"), item.imdb_id, e);
                            tracker.record_failed_with_error("Plex: mark_watched_discover_failed");
                        }
                        error_count += 1;
                        // Continue processing other items instead of returning
                    }
                }
            } else {
                trace!("Plex: Could not find rating_key for '{}' (imdb_id={}, title={:?}, year={:?}) - item may not be in Plex library or discover provider (expected for items from other sources)", 
                      item.imdb_id, item.imdb_id, title, year);
                tracker.record_failed_with_error("Plex: rating_key_not_found");
                not_found_count += 1;
            }

            tracker.log_progress(current);
        }
        
        tracker.log_summary("Plex watch history add");
        
        // Return error only if ALL items failed
        if success_count == 0 && items.len() > 0 {
            Err(crate::error::SourceError::new(format!(
                "Failed to mark any items as watched: {} not found, {} API errors",
                not_found_count, error_count
            )))
        } else {
            Ok(())
        }
    }
}

impl RatingNormalization for PlexClient {
    fn normalize_rating(&self, rating: f64, target_scale: u8) -> u8 {
        // Plex uses 0.0-10.0 for ratings, convert to target scale
        if target_scale == 10 {
            rating.round() as u8
        } else {
            (rating * (target_scale as f64 / 10.0)).round() as u8
        }
    }
    
    fn denormalize_rating(&self, rating: u8, source_scale: u8) -> f64 {
        // Convert from source scale back to 0.0-10.0
        if source_scale == 10 {
            rating as f64
        } else {
            rating as f64 * 10.0 / source_scale as f64
        }
    }
    
    fn native_rating_scale(&self) -> u8 {
        10 // Plex uses 0.0-10.0 scale
    }
}

impl StatusMapping for PlexClient {
    fn requires_status_mapping(&self) -> bool {
        true
    }
}

impl IdExtraction for PlexClient {
    fn extract_ids(&self, imdb_id: Option<&str>, native_ids: Option<&serde_json::Value>) -> Option<MediaIds> {
        let mut media_ids = MediaIds::default();
        
        // Extract IMDB ID if provided
        if let Some(imdb) = imdb_id.filter(|id| !id.is_empty()) {
            media_ids.imdb_id = Some(imdb.to_string());
        }
        
        // Extract from Plex GUIDs JSON structure
        if let Some(ids_json) = native_ids {
            // Try to parse as array of GUID objects
            if let Some(guid_array) = ids_json.as_array() {
                let mut guids = Vec::new();
                for guid_val in guid_array {
                    if let Some(guid_obj) = guid_val.as_object() {
                        if let Some(id_val) = guid_obj.get("id").and_then(|v| v.as_str()) {
                            guids.push(crate::plex::api::Guid { id: id_val.to_string() });
                        }
                    } else if let Some(id_str) = guid_val.as_str() {
                        guids.push(crate::plex::api::Guid { id: id_str.to_string() });
                    }
                }
                let extracted = Self::extract_ids_from_guids(&guids);
                media_ids.merge(&extracted);
            }
        }
        
        if !media_ids.is_empty() {
            Some(media_ids)
        } else {
            None
        }
    }
    
    fn native_id_type(&self) -> &str {
        "plex_guid"
    }
}

#[async_trait]
impl IdLookupProvider for PlexClient {
    async fn lookup_ids(
        &self,
        title: &str,
        year: Option<u32>,
        media_type: &MediaType,
    ) -> Result<Option<MediaIds>, Box<dyn std::error::Error + Send + Sync>> {
        let server_url = self.get_server_url().await
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("{}", e))) as Box<dyn std::error::Error + Send + Sync>)?;
        
        let client = self.get_api_client().await
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("{}", e))) as Box<dyn std::error::Error + Send + Sync>)?;
        
        let search_type = match media_type {
            MediaType::Movie => "movie",
            MediaType::Show => "show",
            MediaType::Episode { .. } => return Ok(None), // Episodes not supported
        };
        
        // First, try local server search
        let mut found_ids: Option<MediaIds> = None;
        match client.search_by_title(&server_url, title, year, search_type).await {
            Ok(search_results) => {
                // Find the best match (exact title match)
                for result in &search_results {
                    if result.title == title {
                        // Extract IDs from GUIDs
                        let mut ids = Self::extract_ids_from_guids(&result.guids);
                        // Include rating_key from local server
                        ids.plex_rating_key = Some(result.rating_key.clone());
                        if !ids.is_empty() || !result.rating_key.is_empty() {
                            found_ids = Some(ids);
                            break;
                        }
                    }
                }
                
                // If no exact match, use first result
                if found_ids.is_none() {
                    if let Some(first_result) = search_results.first() {
                        let mut ids = Self::extract_ids_from_guids(&first_result.guids);
                        // Include rating_key from local server
                        ids.plex_rating_key = Some(first_result.rating_key.clone());
                        if !ids.is_empty() || !first_result.rating_key.is_empty() {
                            found_ids = Some(ids);
                        }
                    }
                }
            }
            Err(e) => {
                debug!("Plex local server search failed for '{}': {}", title, e);
            }
        }
        
        // If local server search found results, return them
        if found_ids.is_some() {
            return Ok(found_ids);
        }
        
        // Local server search failed or found no results - try discover provider
        match client.search_discover_provider(title, year, search_type).await {
            Ok(search_results) => {
                // Find the best match (exact title match, prefer year match if available)
                for result in &search_results {
                    if result.title == title {
                        // Extract IDs from GUIDs
                        let mut ids = Self::extract_ids_from_guids(&result.guids);
                        // Include rating_key from discover provider
                        ids.plex_rating_key = Some(result.rating_key.clone());
                        // Add metadata for future lookups
                        ids.title = Some(title.to_string());
                        ids.year = year;
                        ids.media_type = Some(media_type.clone());
                        return Ok(Some(ids));
                    }
                }
                
                // If no exact match, use first result
                if let Some(first_result) = search_results.first() {
                    let mut ids = Self::extract_ids_from_guids(&first_result.guids);
                    // Include rating_key from discover provider
                    ids.plex_rating_key = Some(first_result.rating_key.clone());
                    // Add metadata for future lookups
                    ids.title = Some(title.to_string());
                    ids.year = year;
                    ids.media_type = Some(media_type.clone());
                    return Ok(Some(ids));
                }
            }
            Err(e) => {
                debug!("Plex discover provider search failed for '{}': {}", title, e);
            }
        }
        
        Ok(None)
    }
    
    fn lookup_priority(&self) -> u8 {
        50 // Medium priority
    }
    
    fn lookup_provider_name(&self) -> &str {
        "plex"
    }
    
    fn is_lookup_available(&self) -> bool {
        self.authenticated
    }
}

impl CapabilityRegistry for PlexClient {
    fn as_incremental_sync(&mut self) -> Option<&mut dyn IncrementalSync> {
        None
    }
    
    fn as_rating_normalization(&self) -> Option<&dyn RatingNormalization> {
        Some(self)
    }
    
    fn as_status_mapping(&self) -> Option<&dyn StatusMapping> {
        Some(self)
    }
    
    fn as_id_extraction(&self) -> Option<&dyn IdExtraction> {
        Some(self)
    }
    
    fn as_id_lookup_provider(&self) -> Option<&dyn IdLookupProvider> {
        Some(self)
    }
}
