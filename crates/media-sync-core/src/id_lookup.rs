use anyhow::Result;
use media_sync_models::{MediaIds, MediaType};
use media_sync_sources::{MediaSource, SourceError};
use tracing::{debug, warn};

/// Aggregator service that queries multiple ID lookup providers
/// 
/// This service is decoupled from specific sources and coordinates
/// lookups across all available providers, merging results.
pub struct IdLookupService {
    /// Providers sorted by priority (highest first)
    /// Maps source name to priority
    providers: Vec<(String, u8)>, // (source_name, priority)
}

impl IdLookupService {
    /// Create a new lookup service from available sources
    pub fn new(sources: &[Box<dyn MediaSource<Error = SourceError>>]) -> Self {
        let mut providers: Vec<(String, u8)> = Vec::new();
        
        for source in sources {
            if let Some(provider) = source.as_id_lookup_provider() {
                if provider.is_lookup_available() {
                    providers.push((
                        provider.lookup_provider_name().to_string(),
                        provider.lookup_priority(),
                    ));
                    debug!("ID lookup service: Registered provider '{}' with priority {}", 
                           provider.lookup_provider_name(), provider.lookup_priority());
                } else {
                    debug!("ID lookup service: Provider '{}' is not available (likely not authenticated)", 
                           provider.lookup_provider_name());
                }
            }
        }
        
        // Sort by priority (highest first)
        providers.sort_by(|a, b| b.1.cmp(&a.1));
        
        if providers.is_empty() {
            warn!("ID lookup service: No lookup providers available! ID resolution by title will not work. Ensure at least one source (Plex, Trakt, or Simkl) is authenticated.");
        } else {
            debug!("ID lookup service: {} provider(s) available: {:?}", 
                   providers.len(), 
                   providers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>());
        }
        
        Self { providers }
    }
    
    /// Look up IDs using all available providers
    /// 
    /// Queries providers in priority order and merges results.
    /// Stops early if a high-confidence match is found (when imdb_id is found).
    /// 
    /// # Arguments
    /// * `sources` - All available media sources (to access providers)
    /// * `title` - Title to search for
    /// * `year` - Optional year
    /// * `media_type` - Type of media
    /// 
    /// # Returns
    /// Merged MediaIds from all providers that found matches
    pub async fn lookup_ids(
        &self,
        sources: &[Box<dyn MediaSource<Error = SourceError>>],
        title: &str,
        year: Option<u32>,
        media_type: &MediaType,
    ) -> Result<MediaIds> {
        let mut merged_ids = MediaIds::default();
        let mut errors = Vec::new();
        
        // Check if any providers are available
        if self.providers.is_empty() {
            warn!("ID lookup: No providers available for '{}' (year: {:?}, type: {:?}). Cannot perform title-based lookup. Ensure at least one source (Plex, Trakt, or Simkl) is authenticated.", 
                  title, year, media_type);
            debug!("ID lookup: Provider list is empty - no lookup providers registered");
            return Ok(merged_ids);
        }
        
        debug!("ID lookup: Attempting lookup for '{}' (year: {:?}, type: {:?}) using {} provider(s): {:?}", 
               title, year, media_type, self.providers.len(),
               self.providers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>());
        
        // Query providers in priority order
        for (provider_name, _priority) in &self.providers {
            // Find the source that provides this lookup
            if let Some(source) = sources.iter().find(|s| s.source_name() == provider_name.as_str()) {
                if let Some(provider) = source.as_id_lookup_provider() {
                    match provider.lookup_ids(title, year, media_type).await {
                        Ok(Some(ids)) => {
                            debug!("ID lookup via {} found IDs: imdb={:?}, trakt={:?}, tmdb={:?}", 
                                   provider_name, ids.imdb_id, ids.trakt_id, ids.tmdb_id);
                            merged_ids.merge(&ids);
                            
                            // Early exit if we have imdb_id (high confidence)
                            if merged_ids.imdb_id.is_some() {
                                break;
                            }
                        }
                        Ok(None) => {
                            // No matches - this is normal, don't log
                        }
                        Err(e) => {
                            warn!("ID lookup via {} failed for '{}' (year: {:?}): {}", 
                                  provider_name, title, year, e);
                            errors.push(format!("{}: {}", provider_name, e));
                        }
                    }
                } else {
                    warn!("ID lookup: Source '{}' does not provide IdLookupProvider trait", provider_name);
                }
            } else {
                warn!("ID lookup: Could not find source '{}' in sources list", provider_name);
            }
        }
        
        if !errors.is_empty() && merged_ids.is_empty() {
            return Err(anyhow::anyhow!(
                "All ID lookups failed: {}",
                errors.join("; ")
            ));
        }
        
        Ok(merged_ids)
    }
    
    /// Get list of available lookup providers
    pub fn available_providers(&self) -> Vec<&str> {
        self.providers.iter().map(|(name, _)| name.as_str()).collect()
    }
    
    /// Look up title, year, and IDs by IMDB ID (reverse lookup)
    /// 
    /// Queries providers in priority order to find title/year from an IMDB ID.
    /// This is useful when we have an IMDB ID but need the title for discover provider searches.
    /// 
    /// # Arguments
    /// * `sources` - All available media sources (to access providers)
    /// * `imdb_id` - IMDB ID to search for (e.g., "tt1234567")
    /// * `media_type` - Type of media
    /// 
    /// # Returns
    /// * `Ok(Some((title, year, MediaIds)))` - Title, year, and IDs found
    /// * `Ok(None)` - No match found
    pub async fn lookup_by_imdb_id(
        &self,
        sources: &[Box<dyn MediaSource<Error = SourceError>>],
        imdb_id: &str,
        media_type: &MediaType,
    ) -> Result<Option<(String, Option<u32>, MediaIds)>> {
        // Check if any providers are available
        if self.providers.is_empty() {
            debug!("ID reverse lookup: No providers available for imdb_id={}, type={:?}", imdb_id, media_type);
            return Ok(None);
        }
        
        debug!("ID reverse lookup: Attempting lookup for imdb_id={}, type={:?} using {} provider(s): {:?}", 
               imdb_id, media_type, self.providers.len(),
               self.providers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>());
        
        // Query providers in priority order
        for (provider_name, _priority) in &self.providers {
            // Find the source that provides this lookup
            if let Some(source) = sources.iter().find(|s| s.source_name() == provider_name.as_str()) {
                if let Some(provider) = source.as_id_lookup_provider() {
                    match provider.lookup_by_imdb_id(imdb_id, media_type).await {
                        Ok(Some((title, year, ids))) => {
                            debug!("ID reverse lookup via {} found: title='{}', year={:?}, imdb={:?}, trakt={:?}, tmdb={:?}", 
                                   provider_name, title, year, ids.imdb_id, ids.trakt_id, ids.tmdb_id);
                            return Ok(Some((title, year, ids)));
                        }
                        Ok(None) => {
                            // No matches - continue to next provider
                        }
                        Err(e) => {
                            warn!("ID reverse lookup via {} failed for imdb_id={}: {}", 
                                  provider_name, imdb_id, e);
                            // Continue to next provider
                        }
                    }
                }
            }
        }
        
        Ok(None)
    }
}

