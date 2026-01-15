use crate::traits::MediaSource;
use crate::capabilities::{RatingNormalization, CapabilityRegistry, StatusMapping, IncrementalSync, IdExtraction, IdLookupProvider};
use crate::trakt::api;
use crate::trakt::auth;
use anyhow::Result;
use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem, MediaIds, MediaType};
use reqwest::Client;
use std::sync::Arc;
use tracing::info;
use async_trait::async_trait;

#[derive(Clone)]
pub struct TraktClient {
    client: Arc<Client>,
    access_token: Option<String>,
    client_id: String,
    client_secret: String,
    encoded_username: Option<String>,
}

impl TraktClient {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client: Arc::new(auth::create_trakt_client()), // Use client with browser-like headers
            access_token: None,
            client_id,
            client_secret,
            encoded_username: None,
        }
    }

    pub async fn authenticate(&mut self) -> Result<()> {
        use crate::trakt::auth::authenticate as trakt_authenticate;
        use media_sync_config::CredentialStore;
        use media_sync_config::PathManager;
        use chrono::{Duration, Utc};

        let path_manager = PathManager::default();
        let mut cred_store = CredentialStore::new(path_manager.credentials_file());
        cred_store.load()?;

        // Check if we have a valid access token first
        if let Some(saved_token) = cred_store.get_trakt_access_token() {
            // Check if token is still valid (not expired or expiring soon)
            if let Some(expires_at) = cred_store.get_trakt_token_expires() {
                // Token is valid if it expires more than 5 minutes from now
                if expires_at > Utc::now() + Duration::minutes(5) {
                    self.access_token = Some(saved_token.clone());
                    
                    // Try to get encoded username with existing token
                    // If this fails, we'll need to re-authenticate
                    match api::get_encoded_username(&self.client, saved_token, &self.client_id).await {
                        Ok(encoded_username) => {
                            self.encoded_username = Some(encoded_username);
                            info!("Using saved Trakt access token (expires at {})", expires_at);
                            return Ok(());
                        }
                        Err(_) => {
                            // Token might be invalid, fall through to refresh
                            info!("Saved Trakt token appears invalid, attempting refresh");
                        }
                    }
                } else {
                    // Token is expired or expiring soon, need to refresh
                    info!("Trakt access token expired or expiring soon (expires at {}), refreshing", expires_at);
                }
            } else {
                // No expiration info, try to use the token
                match api::get_encoded_username(&self.client, saved_token, &self.client_id).await {
                    Ok(encoded_username) => {
                        self.access_token = Some(saved_token.clone());
                        self.encoded_username = Some(encoded_username);
                        info!("Using saved Trakt access token (no expiration info)");
                        return Ok(());
                    }
                    Err(_) => {
                        // Token might be invalid, fall through to refresh
                        info!("Saved Trakt token appears invalid, attempting refresh");
                    }
                }
            }
        }

        // No valid token, need to authenticate (refresh or new authorization)
        let refresh_token = cred_store.get_trakt_refresh_token().map(|s| s.as_str());

        let token_info = trakt_authenticate(&self.client_id, &self.client_secret, refresh_token).await?;

        self.access_token = Some(token_info.access_token.clone());

        // Get encoded username
        let encoded_username = api::get_encoded_username(&self.client, &token_info.access_token, &self.client_id).await?;
        self.encoded_username = Some(encoded_username);

        // Save tokens
        cred_store.set_trakt_access_token(token_info.access_token);
        cred_store.set_trakt_refresh_token(token_info.refresh_token);
        cred_store.set_trakt_token_expires(token_info.expires_at);
        cred_store.save()?;

        info!("Authenticated to Trakt");
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.access_token.is_some() && self.encoded_username.is_some()
    }

    fn access_token(&self) -> Result<&str> {
        self.access_token.as_deref().ok_or_else(|| anyhow::anyhow!("Not authenticated"))
    }

    fn encoded_username(&self) -> Result<&str> {
        self.encoded_username.as_deref().ok_or_else(|| anyhow::anyhow!("Username not available"))
    }

    /// Trakt ratings are already in the target format (1-10 integer)
    pub fn normalize_to_trakt(&self, trakt_rating: u8) -> u8 {
        trakt_rating
    }

    /// Convert Trakt rating (1-10 integer) to Trakt format
    pub fn normalize_from_trakt(&self, trakt_rating: u8) -> u8 {
        trakt_rating
    }
}

#[async_trait::async_trait]
impl MediaSource for TraktClient {
    type Error = crate::error::SourceError;

    fn source_name(&self) -> &str {
        "trakt"
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
        let encoded_username = self.encoded_username().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::get_watchlist(&self.client, access_token, encoded_username, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn get_ratings(&self) -> Result<Vec<Rating>, Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        let encoded_username = self.encoded_username().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::get_ratings(&self.client, access_token, encoded_username, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn get_reviews(&self) -> Result<Vec<Review>, Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        let encoded_username = self.encoded_username().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::get_comments(&self.client, access_token, encoded_username, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn get_watch_history(&self) -> Result<Vec<WatchHistory>, Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        let encoded_username = self.encoded_username().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::get_watch_history(&self.client, access_token, encoded_username, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn add_to_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::add_to_watchlist(&self.client, access_token, items, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn remove_from_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::remove_from_watchlist(&self.client, access_token, items, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn set_ratings(&self, ratings: &[Rating]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::set_ratings(&self.client, access_token, ratings, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn set_reviews(&self, reviews: &[Review]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::add_comments(&self.client, access_token, reviews, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

    async fn add_watch_history(&self, items: &[WatchHistory]) -> Result<(), Self::Error> {
        let access_token = self.access_token().map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
        api::add_watch_history(&self.client, access_token, items, &self.client_id)
            .await
            .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
    }

}

impl RatingNormalization for TraktClient {
    fn normalize_rating(&self, rating: f64, target_scale: u8) -> u8 {
        // Trakt uses 1-10 scale, same as target
        rating.round() as u8
    }
    
    fn denormalize_rating(&self, rating: u8, source_scale: u8) -> f64 {
        // Trakt uses 1-10 scale
        rating as f64
    }
    
    fn native_rating_scale(&self) -> u8 {
        10
    }
}

impl IdExtraction for TraktClient {
    fn extract_ids(&self, imdb_id: Option<&str>, native_ids: Option<&serde_json::Value>) -> Option<MediaIds> {
        let mut media_ids = MediaIds::default();
        
        // Extract IMDB ID if provided
        if let Some(imdb) = imdb_id.filter(|id| !id.is_empty()) {
            media_ids.imdb_id = Some(imdb.to_string());
        }
        
        // Extract from TraktIds JSON structure
        if let Some(ids_json) = native_ids {
            // Try to deserialize as TraktIds
            if let Ok(trakt_ids_map) = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(ids_json.clone()) {
                let mut trakt_imdb = trakt_ids_map.get("imdb").and_then(|v| v.as_str()).map(|s| s.to_string());
                let trakt_trakt = trakt_ids_map.get("trakt").and_then(|v| v.as_u64());
                let trakt_tmdb = trakt_ids_map.get("tmdb").and_then(|v| v.as_u64()).map(|v| v as u32);
                let trakt_tvdb = trakt_ids_map.get("tvdb").and_then(|v| v.as_u64()).map(|v| v as u32);
                let trakt_slug = trakt_ids_map.get("slug").and_then(|v| v.as_str()).map(|s| s.to_string());
                
                if media_ids.imdb_id.is_none() {
                    media_ids.imdb_id = trakt_imdb.map(|s| s.replace('/', ""));
                }
                media_ids.trakt_id = trakt_trakt;
                media_ids.tmdb_id = trakt_tmdb;
                media_ids.tvdb_id = trakt_tvdb;
                media_ids.slug = trakt_slug;
            }
        }
        
        if !media_ids.is_empty() {
            Some(media_ids)
        } else {
            None
        }
    }
    
    fn native_id_type(&self) -> &str {
        "trakt"
    }
}

#[async_trait]
impl IdLookupProvider for TraktClient {
    async fn lookup_ids(
        &self,
        title: &str,
        year: Option<u32>,
        media_type: &MediaType,
    ) -> Result<Option<MediaIds>, Box<dyn std::error::Error + Send + Sync>> {
        let access_token = self.access_token()
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("{}", e))) as Box<dyn std::error::Error + Send + Sync>)?;
        
        api::search_by_title(&self.client, access_token, &self.client_id, title, year, media_type)
            .await
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("{}", e))) as Box<dyn std::error::Error + Send + Sync>)
    }
    
    async fn lookup_by_imdb_id(
        &self,
        imdb_id: &str,
        media_type: &MediaType,
    ) -> Result<Option<(String, Option<u32>, MediaIds)>, Box<dyn std::error::Error + Send + Sync>> {
        let access_token = self.access_token()
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("{}", e))) as Box<dyn std::error::Error + Send + Sync>)?;
        
        api::search_by_imdb_id(&self.client, access_token, &self.client_id, imdb_id, media_type)
            .await
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("{}", e))) as Box<dyn std::error::Error + Send + Sync>)
    }
    
    fn lookup_priority(&self) -> u8 {
        80 // High priority - authenticated API
    }
    
    fn lookup_provider_name(&self) -> &str {
        "trakt"
    }
    
    fn is_lookup_available(&self) -> bool {
        self.is_authenticated()
    }
}

impl CapabilityRegistry for TraktClient {
    fn as_incremental_sync(&mut self) -> Option<&mut dyn IncrementalSync> {
        None
    }
    
    fn as_rating_normalization(&self) -> Option<&dyn RatingNormalization> {
        Some(self)
    }
    
    fn as_status_mapping(&self) -> Option<&dyn StatusMapping> {
        None
    }
    
    fn as_id_extraction(&self) -> Option<&dyn IdExtraction> {
        Some(self)
    }
    
    fn as_id_lookup_provider(&self) -> Option<&dyn IdLookupProvider> {
        Some(self)
    }
}

