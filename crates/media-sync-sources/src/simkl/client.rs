use crate::traits::MediaSource;
use crate::capabilities::{IncrementalSync, RatingNormalization, CapabilityRegistry, StatusMapping, IdExtraction, IdLookupProvider};
use crate::simkl::api;
use crate::simkl::auth;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem, MediaIds, MediaType};
use media_sync_config::StatusMapping as StatusMappingConfig;
use reqwest::Client;
use std::sync::Arc;
use std::collections::HashMap;
use tracing::info;
use async_trait::async_trait;

#[derive(Clone)]
pub struct SimklClient {
    client: Arc<Client>,
    access_token: Option<String>,
    client_id: String,
    client_secret: String,
    force_full_sync: bool,
    status_mapping: StatusMappingConfig,
}

impl SimklClient {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client: Arc::new(auth::create_simkl_client()),
            access_token: None,
            client_id,
            client_secret,
            force_full_sync: false,
            status_mapping: StatusMappingConfig {
                to_normalized: HashMap::new(),
                from_normalized: HashMap::new(),
            },
        }
    }

    pub fn with_status_mapping(mut self, status_mapping: StatusMappingConfig) -> Self {
        self.status_mapping = status_mapping;
        self
    }

    pub fn set_force_full_sync(&mut self, force: bool) {
        self.force_full_sync = force;
    }

    pub async fn authenticate(&mut self) -> Result<()> {
        use crate::simkl::auth::authenticate as simkl_authenticate;
        use media_sync_config::CredentialStore;
        use media_sync_config::PathManager;

        let path_manager = PathManager::default();
        let mut cred_store = CredentialStore::new(path_manager.credentials_file());
        cred_store.load()?;

        // Check if we have a valid access token
        // According to Simkl API docs, access tokens never expire, so if we have a token, use it
        if let Some(saved_token) = cred_store.get_simkl_access_token() {
            if !saved_token.is_empty() {
                // If expires_at exists and is in the future, use it
                // If expires_at doesn't exist or is None, treat as never expiring (per API docs)
                // If expires_at exists but is in the past, still use the token (API says tokens never expire)
                if let Some(expires_at) = cred_store.get_simkl_token_expires() {
                    if expires_at > Utc::now() + Duration::minutes(5) {
                        // Token has expiration set and is still valid
                        self.access_token = Some(saved_token.clone());
                        info!("Using saved Simkl access token (expires at {})", expires_at);
                        return Ok(());
                    } else {
                        // Token has expiration set but it's in the past
                        // Per API docs, tokens never expire, so use it anyway
                        // This handles tokens saved with incorrect expiration times from previous code
                        self.access_token = Some(saved_token.clone());
                        info!("Using saved Simkl access token (expiration was set but tokens never expire per API)");
                        return Ok(());
                    }
                } else {
                    // No expiration set - treat as never expiring (per Simkl API docs)
                    self.access_token = Some(saved_token.clone());
                    info!("Using saved Simkl access token (never expires)");
                    return Ok(());
                }
            }
        }

        // No valid token, need to authenticate
        let refresh_token = cred_store.get_simkl_refresh_token().map(|s| s.as_str());

        let token_info = simkl_authenticate(&self.client_id, &self.client_secret, refresh_token).await?;

        self.access_token = Some(token_info.access_token.clone());

        // Save tokens
        cred_store.set_simkl_access_token(token_info.access_token);
        cred_store.set_simkl_refresh_token(token_info.refresh_token);
        cred_store.set_simkl_token_expires(token_info.expires_at);
        cred_store.save()?;

        info!("Authenticated to Simkl");
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.access_token.is_some()
    }

    fn access_token(&self) -> Result<&str> {
        self.access_token.as_deref().ok_or_else(|| anyhow::anyhow!("Not authenticated"))
    }

    /// Simkl ratings are already in the target format (1-10 integer), same as Trakt
    pub fn normalize_to_trakt(&self, trakt_rating: u8) -> u8 {
        trakt_rating
    }

    /// Convert Trakt rating (1-10 integer) to Simkl format (same scale)
    pub fn normalize_from_trakt(&self, trakt_rating: u8) -> u8 {
        trakt_rating
    }

    fn get_credential_store(&self) -> Result<(media_sync_config::CredentialStore, media_sync_config::PathManager)> {
        let path_manager = media_sync_config::PathManager::default();
        let mut cred_store = media_sync_config::CredentialStore::new(path_manager.credentials_file());
        cred_store.load()?;
        Ok((cred_store, path_manager))
    }
    
    async fn check_activities_changed(
        &self,
        data_type: &str, // "watchlist", "ratings", "watch_history"
        force_full_sync: bool,
    ) -> Result<Option<DateTime<Utc>>> {
        if force_full_sync {
            return Ok(None); // Fetch all
        }

        let (mut cred_store, _) = self.get_credential_store()?;
        
        // Fetch current activities
        let current_activities = api::get_activities(
            &self.client,
            &self.access_token()?,
            &self.client_id,
        ).await?;
        
        // Load saved activities
        let saved_activities_json = cred_store.get_simkl_last_activities().unwrap_or_default();
        
        if saved_activities_json.is_empty() {
            // First sync - save activities and return None (fetch all)
            cred_store.set_simkl_last_activities(serde_json::to_string(&current_activities)?);
            cred_store.save()?;
            return Ok(None);
        }
        
        let saved_activities: api::SimklActivities = serde_json::from_str(&saved_activities_json)?;
        
        // Check if specific data type changed
        let changed = match data_type {
            "watchlist" => {
                // Check tv_shows.all, anime.all, movies.all
                let tv_changed = current_activities.tv_shows.as_ref()
                    .and_then(|s| s.all.as_ref())
                    .map(|current| {
                        saved_activities.tv_shows.as_ref()
                            .and_then(|s| s.all.as_ref())
                            .map(|saved| current != saved)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                
                let anime_changed = current_activities.anime.as_ref()
                    .and_then(|s| s.all.as_ref())
                    .map(|current| {
                        saved_activities.anime.as_ref()
                            .and_then(|s| s.all.as_ref())
                            .map(|saved| current != saved)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                
                let movies_changed = current_activities.movies.as_ref()
                    .and_then(|s| s.all.as_ref())
                    .map(|current| {
                        saved_activities.movies.as_ref()
                            .and_then(|s| s.all.as_ref())
                            .map(|saved| current != saved)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                
                tv_changed || anime_changed || movies_changed
            },
            "ratings" => {
                // Check rated_at timestamps
                let tv_changed = current_activities.tv_shows.as_ref()
                    .and_then(|s| s.rated_at.as_ref())
                    .map(|current| {
                        saved_activities.tv_shows.as_ref()
                            .and_then(|s| s.rated_at.as_ref())
                            .map(|saved| current != saved)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                
                let anime_changed = current_activities.anime.as_ref()
                    .and_then(|s| s.rated_at.as_ref())
                    .map(|current| {
                        saved_activities.anime.as_ref()
                            .and_then(|s| s.rated_at.as_ref())
                            .map(|saved| current != saved)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                
                let movies_changed = current_activities.movies.as_ref()
                    .and_then(|s| s.rated_at.as_ref())
                    .map(|current| {
                        saved_activities.movies.as_ref()
                            .and_then(|s| s.rated_at.as_ref())
                            .map(|saved| current != saved)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                
                tv_changed || anime_changed || movies_changed
            },
            "watch_history" => {
                // Check playback timestamps
                let tv_changed = current_activities.tv_shows.as_ref()
                    .and_then(|s| s.playback.as_ref())
                    .map(|current| {
                        saved_activities.tv_shows.as_ref()
                            .and_then(|s| s.playback.as_ref())
                            .map(|saved| current != saved)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                
                let anime_changed = current_activities.anime.as_ref()
                    .and_then(|s| s.playback.as_ref())
                    .map(|current| {
                        saved_activities.anime.as_ref()
                            .and_then(|s| s.playback.as_ref())
                            .map(|saved| current != saved)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                
                let movies_changed = current_activities.movies.as_ref()
                    .and_then(|s| s.playback.as_ref())
                    .map(|current| {
                        saved_activities.movies.as_ref()
                            .and_then(|s| s.playback.as_ref())
                            .map(|saved| current != saved)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                
                tv_changed || anime_changed || movies_changed
            },
            _ => false,
        };
        
        if changed {
            // Use saved "all" timestamp as date_from
            let date_from = saved_activities.all
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            
            // Save new activities
            cred_store.set_simkl_last_activities(serde_json::to_string(&current_activities)?);
            cred_store.save()?;
            
            Ok(date_from)
        } else {
            // No changes, return special marker to indicate empty result
            // Using epoch as marker - caller should check for this and return empty
            Ok(Some(DateTime::from_timestamp(0, 0).unwrap().with_timezone(&Utc)))
        }
    }
}

#[async_trait::async_trait]
impl MediaSource for SimklClient {
    type Error = crate::error::SourceError;

    fn source_name(&self) -> &str {
        "simkl"
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
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        // Check activities to determine if we need incremental sync
        let date_from = match self.check_activities_changed("watchlist", self.force_full_sync).await {
            Ok(Some(date)) => {
                // Check if it's the special "no changes" marker (epoch)
                if date.timestamp() == 0 {
                    // No changes, return empty
                    return Ok(Vec::new());
                }
                Some(date)
            },
            Ok(None) => None, // First sync or force full sync
            Err(e) => {
                // If activities check fails, fall back to full sync
                tracing::warn!("Failed to check Simkl activities, falling back to full sync: {}", e);
                None
            },
        };
        
        api::get_watchlist(&self.client, access_token, &self.client_id, date_from, &self.status_mapping.to_normalized)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn get_ratings(&self) -> Result<Vec<Rating>, Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        // Check activities to determine if we need incremental sync
        let date_from = match self.check_activities_changed("ratings", self.force_full_sync).await {
            Ok(Some(date)) => {
                // Check if it's the special "no changes" marker (epoch)
                if date.timestamp() == 0 {
                    // No changes, return empty
                    return Ok(Vec::new());
                }
                Some(date)
            },
            Ok(None) => None, // First sync or force full sync
            Err(e) => {
                // If activities check fails, fall back to full sync
                tracing::warn!("Failed to check Simkl activities, falling back to full sync: {}", e);
                None
            },
        };
        
        api::get_ratings(&self.client, access_token, &self.client_id, date_from)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn get_reviews(&self) -> Result<Vec<Review>, Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::get_reviews(&self.client, access_token, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn get_watch_history(&self) -> Result<Vec<WatchHistory>, Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        
        // Check activities to determine if we need incremental sync
        let date_from = match self.check_activities_changed("watch_history", self.force_full_sync).await {
            Ok(Some(date)) => {
                // Check if it's the special "no changes" marker (epoch)
                if date.timestamp() == 0 {
                    // No changes, return empty
                    return Ok(Vec::new());
                }
                Some(date)
            },
            Ok(None) => None, // First sync or force full sync
            Err(e) => {
                // If activities check fails, fall back to full sync
                tracing::warn!("Failed to check Simkl activities, falling back to full sync: {}", e);
                None
            },
        };
        
        api::get_watch_history(&self.client, access_token, &self.client_id, date_from)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn add_to_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::add_to_watchlist(&self.client, access_token, &self.client_id, items, &self.status_mapping.from_normalized)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn remove_from_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::remove_from_watchlist(&self.client, access_token, &self.client_id, items)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn set_ratings(&self, ratings: &[Rating]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::set_ratings(&self.client, access_token, &self.client_id, ratings)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn set_reviews(&self, reviews: &[Review]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::set_reviews(&self.client, access_token, &self.client_id, reviews)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn add_watch_history(&self, items: &[WatchHistory]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::add_watch_history(&self.client, access_token, &self.client_id, items)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

}

impl IncrementalSync for SimklClient {
    fn set_force_full_sync(&mut self, force: bool) {
        self.force_full_sync = force;
    }
    
    fn supports_native_incremental_sync(&self) -> bool {
        true
    }
}

impl RatingNormalization for SimklClient {
    fn normalize_rating(&self, rating: f64, target_scale: u8) -> u8 {
        // Simkl uses 1-10 scale, same as Trakt
        // For now, we assume target_scale is 10 (Trakt scale)
        rating.round() as u8
    }
    
    fn denormalize_rating(&self, rating: u8, source_scale: u8) -> f64 {
        // Simkl uses 1-10 scale, same as Trakt
        rating as f64
    }
    
    fn native_rating_scale(&self) -> u8 {
        10
    }
}

impl CapabilityRegistry for SimklClient {
    fn as_incremental_sync(&mut self) -> Option<&mut dyn IncrementalSync> {
        Some(self)
    }
    
    fn as_rating_normalization(&self) -> Option<&dyn RatingNormalization> {
        Some(self)
    }
    
    fn as_status_mapping(&self) -> Option<&dyn StatusMapping> {
        None
    }
    
    fn supports_incremental_sync(&self) -> bool {
        true
    }
    
    fn as_id_extraction(&self) -> Option<&dyn IdExtraction> {
        Some(self)
    }
    
    fn as_id_lookup_provider(&self) -> Option<&dyn IdLookupProvider> {
        Some(self)
    }
}

impl IdExtraction for SimklClient {
    fn extract_ids(&self, imdb_id: Option<&str>, native_ids: Option<&serde_json::Value>) -> Option<MediaIds> {
        let mut media_ids = MediaIds::default();
        
        // Extract IMDB ID if provided
        if let Some(imdb) = imdb_id.filter(|id| !id.is_empty()) {
            media_ids.imdb_id = Some(imdb.to_string());
        }
        
        // Extract from SimklIds JSON structure
        if let Some(ids_json) = native_ids {
            if let Ok(ids_map) = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(ids_json.clone()) {
                if media_ids.imdb_id.is_none() {
                    if let Some(imdb_val) = ids_map.get("imdb").and_then(|v| v.as_str()) {
                        media_ids.imdb_id = Some(imdb_val.to_string());
                    }
                }
                if let Some(simkl_val) = ids_map.get("simkl").and_then(|v| v.as_u64()) {
                    media_ids.simkl_id = Some(simkl_val);
                }
            }
        }
        
        if !media_ids.is_empty() {
            Some(media_ids)
        } else {
            None
        }
    }
    
    fn native_id_type(&self) -> &str {
        "simkl"
    }
}

#[async_trait]
impl IdLookupProvider for SimklClient {
    async fn lookup_ids(
        &self,
        title: &str,
        year: Option<u32>,
        media_type: &MediaType,
    ) -> Result<Option<MediaIds>, Box<dyn std::error::Error + Send + Sync>> {
        let access_token = self.access_token()
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("{}", e))) as Box<dyn std::error::Error + Send + Sync>)?;
        
        // Note: Simkl doesn't have a public search API, so this will return None
        // Do not use TMDB fallback as per plan requirements
        api::search_by_title(&self.client, access_token, &self.client_id, title, year, media_type)
            .await
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("{}", e))) as Box<dyn std::error::Error + Send + Sync>)
    }
    
    fn lookup_priority(&self) -> u8 {
        70 // Medium-high priority
    }
    
    fn lookup_provider_name(&self) -> &str {
        "simkl"
    }
    
    fn is_lookup_available(&self) -> bool {
        self.is_authenticated()
    }
}

