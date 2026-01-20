use async_trait::async_trait;
use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem};
use crate::capabilities::CapabilityRegistry;

#[async_trait]
pub trait MediaSource: Send + Sync + CapabilityRegistry {
    type Error: std::error::Error + Send + Sync + 'static + std::fmt::Display;

    // Source metadata
    fn source_name(&self) -> &str;
    
    // Distribution strategy identifier
    // Sources can return a custom strategy name, or None to use default
    // This allows sources to control how data is prepared for distribution
    // The core will use this to select the appropriate strategy
    fn distribution_strategy_name(&self) -> Option<&str> {
        None
    }
    
    // Capability detection helpers (delegated to CapabilityRegistry)
    /// Check if this source supports incremental sync
    fn has_incremental_sync(&self) -> bool {
        self.supports_incremental_sync()
    }
    
    /// Check if this source supports rating normalization
    fn has_rating_normalization(&self) -> bool {
        self.supports_rating_normalization()
    }
    
    /// Check if this source supports status mapping
    fn has_status_mapping(&self) -> bool {
        self.supports_status_mapping()
    }

    // Authentication
    async fn authenticate(&mut self) -> Result<(), Self::Error>;
    fn is_authenticated(&self) -> bool;

    // Data retrieval
    async fn get_watchlist(&self) -> Result<Vec<WatchlistItem>, Self::Error>;
    async fn get_ratings(&self) -> Result<Vec<Rating>, Self::Error>;
    async fn get_reviews(&self) -> Result<Vec<Review>, Self::Error>;
    async fn get_watch_history(&self) -> Result<Vec<WatchHistory>, Self::Error>;

    // Data modification
    async fn add_to_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error>;
    async fn remove_from_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error>;
    async fn set_ratings(&self, ratings: &[Rating]) -> Result<(), Self::Error>;
    async fn set_reviews(&self, reviews: &[Review]) -> Result<(), Self::Error>;
    async fn add_watch_history(&self, items: &[WatchHistory]) -> Result<(), Self::Error>;
    
    // Cleanup/shutdown (optional - default implementation does nothing)
    // Called when sync job completes to free resources (e.g., close browser instances)
    async fn cleanup(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

