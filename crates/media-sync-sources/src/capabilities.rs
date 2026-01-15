/// Capability traits for media sources
/// 
/// These traits allow sources to declare their capabilities without
/// requiring string-based matching in the core pipeline.

use async_trait::async_trait;
use media_sync_models::{MediaIds, MediaType};

/// Registry pattern for accessing capabilities without unsafe downcasting
/// 
/// Sources implement this trait to provide safe access to their capabilities
/// through trait object references.
pub trait CapabilityRegistry: Send + Sync {
    /// Get a mutable reference to IncrementalSync capability if supported
    fn as_incremental_sync(&mut self) -> Option<&mut dyn IncrementalSync>;
    
    /// Get a reference to RatingNormalization capability if supported
    fn as_rating_normalization(&self) -> Option<&dyn RatingNormalization>;
    
    /// Get a reference to StatusMapping capability if supported
    fn as_status_mapping(&self) -> Option<&dyn StatusMapping>;
    
    /// Check if this source supports the IncrementalSync capability
    fn supports_incremental_sync(&self) -> bool {
        // Default implementation: try to get a reference (immutable check)
        // This is a bit of a hack, but works for detection
        false // Will be overridden by implementations that actually support it
    }
    
    /// Check if this source supports the RatingNormalization capability
    fn supports_rating_normalization(&self) -> bool {
        self.as_rating_normalization().is_some()
    }
    
    /// Check if this source supports the StatusMapping capability
    fn supports_status_mapping(&self) -> bool {
        self.as_status_mapping().is_some()
    }
    
    /// Get a reference to IdExtraction capability if supported
    fn as_id_extraction(&self) -> Option<&dyn IdExtraction>;
    
    /// Get a reference to IdLookupProvider capability if supported
    fn as_id_lookup_provider(&self) -> Option<&dyn IdLookupProvider>;
    
    /// Check if this source supports ID extraction
    fn supports_id_extraction(&self) -> bool {
        self.as_id_extraction().is_some()
    }
    
    /// Check if this source supports ID lookup
    fn supports_id_lookup(&self) -> bool {
        self.as_id_lookup_provider().is_some()
    }
}

/// Trait for sources that support native incremental sync
/// 
/// Sources implementing this trait can efficiently fetch only changed data
/// since the last sync, rather than requiring full data fetches.
pub trait IncrementalSync: Send + Sync {
    /// Set whether to force a full sync (ignore incremental sync)
    fn set_force_full_sync(&mut self, force: bool);
    
    /// Check if the source supports native incremental sync
    /// 
    /// Sources like Simkl have native incremental sync support via their
    /// activities API, so they don't need timestamp-based filtering.
    fn supports_native_incremental_sync(&self) -> bool {
        false
    }
}

/// Trait for sources that support status mapping
/// 
/// Some sources (like Trakt) need to map between their status values
/// and normalized status values used in the pipeline.
pub trait StatusMapping: Send + Sync {
    /// Check if the source requires status mapping
    fn requires_status_mapping(&self) -> bool {
        false
    }
}

/// Trait for rating normalization
/// 
/// Replaces the Trakt-specific `normalize_to_trakt` and `normalize_from_trakt`
/// methods with a more generic normalization interface.
pub trait RatingNormalization: Send + Sync {
    /// Normalize a rating from the source's format to a target format
    /// 
    /// # Arguments
    /// * `rating` - The rating in the source's native format (as f64)
    /// * `target_scale` - The target scale (e.g., 10 for 1-10 scale)
    /// 
    /// # Returns
    /// The normalized rating as a u8 in the target scale
    fn normalize_rating(&self, rating: f64, target_scale: u8) -> u8;
    
    /// Denormalize a rating from a target format to the source's format
    /// 
    /// # Arguments
    /// * `rating` - The rating in the target format (as u8)
    /// * `target_scale` - The source scale (e.g., 10 for 1-10 scale)
    /// 
    /// # Returns
    /// The denormalized rating as f64 in the source's native format
    fn denormalize_rating(&self, rating: u8, source_scale: u8) -> f64;
    
    /// Get the source's native rating scale
    /// 
    /// # Returns
    /// The maximum value of the source's rating scale (e.g., 10 for 1-10, 5 for 1-5)
    fn native_rating_scale(&self) -> u8;
}

/// Trait for sources that can extract IDs from their native format
/// 
/// Sources implementing this trait can extract normalized MediaIds from their
/// native API responses (e.g., TraktIds, SimklIds, Plex GUIDs).
pub trait IdExtraction: Send + Sync {
    /// Extract all available IDs from a source's native ID structure
    /// 
    /// This method is called when processing items from the source's API responses.
    /// The source should extract all available IDs (imdb, trakt, tmdb, etc.) and
    /// return them as a normalized MediaIds struct.
    /// 
    /// # Arguments
    /// * `imdb_id` - The IMDB ID if available (may be empty)
    /// * `native_ids` - A JSON value containing the source's native ID structure
    /// 
    /// # Returns
    /// A MediaIds struct with all available IDs, or None if no IDs could be extracted
    fn extract_ids(&self, imdb_id: Option<&str>, native_ids: Option<&serde_json::Value>) -> Option<MediaIds>;
    
    /// Get the source's native ID format name
    /// 
    /// Returns a string identifier for the ID type this source primarily uses
    /// (e.g., "trakt", "simkl", "plex_guid")
    fn native_id_type(&self) -> &str;
}

/// Trait for sources that can look up IDs by title/metadata
/// 
/// Sources implementing this trait can participate in external ID lookups.
/// This allows sources to use their own APIs (e.g., Trakt search, Simkl search)
/// or external services to find IDs for items that are missing them.
#[async_trait]
pub trait IdLookupProvider: Send + Sync {
    /// Look up IDs for a media item using title and metadata
    /// 
    /// # Arguments
    /// * `title` - The title of the media item
    /// * `year` - Optional year of release
    /// * `media_type` - The type of media (Movie, Show, Episode)
    /// 
    /// # Returns
    /// * `Ok(Some(MediaIds))` - IDs were found
    /// * `Ok(None)` - No IDs found (not an error, just no match)
    /// * `Err(_)` - An error occurred during lookup
    async fn lookup_ids(
        &self,
        title: &str,
        year: Option<u32>,
        media_type: &MediaType,
    ) -> Result<Option<MediaIds>, Box<dyn std::error::Error + Send + Sync>>;
    
    /// Get the priority/confidence of this lookup provider
    /// 
    /// Higher priority providers are queried first. This allows sources
    /// with better APIs (e.g., authenticated Trakt) to be preferred over
    /// generic services.
    /// 
    /// Default is 0 (lowest priority)
    fn lookup_priority(&self) -> u8 {
        0
    }
    
    /// Get the name of this lookup provider (for logging/debugging)
    fn lookup_provider_name(&self) -> &str;
    
    /// Check if this provider is available/ready for lookups
    /// 
    /// Some providers might require authentication or specific configuration.
    /// Returns false if the provider cannot perform lookups at this time.
    fn is_lookup_available(&self) -> bool {
        true
    }
    
    /// Look up title, year, and IDs for a media item using IMDB ID (reverse lookup)
    /// 
    /// This is the inverse of `lookup_ids` - instead of searching by title,
    /// this searches by IMDB ID to get the title, year, and other IDs.
    /// 
    /// # Arguments
    /// * `imdb_id` - The IMDB ID (e.g., "tt1234567")
    /// * `media_type` - The type of media (Movie, Show, Episode)
    /// 
    /// # Returns
    /// * `Ok(Some((title, year, MediaIds)))` - Title, year, and IDs were found
    /// * `Ok(None)` - No match found (not an error, just no match)
    /// * `Err(_)` - An error occurred during lookup
    /// 
    /// Default implementation returns None (not supported)
    async fn lookup_by_imdb_id(
        &self,
        _imdb_id: &str,
        _media_type: &MediaType,
    ) -> Result<Option<(String, Option<u32>, MediaIds)>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(None)
    }
}

