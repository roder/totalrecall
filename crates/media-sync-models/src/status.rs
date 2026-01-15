use serde::{Deserialize, Serialize};

/// Normalized status values used across all services during resolution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum NormalizedStatus {
    /// Want to watch (plantowatch on Simkl, watchlist on IMDB/Trakt)
    Watchlist,
    /// Currently watching (watching on Simkl, check-ins on IMDB/Trakt)
    Watching,
    /// Finished watching (completed on Simkl, watched on IMDB/Trakt)
    Completed,
    /// Stopped watching (dropped on Simkl, not supported on IMDB/Trakt)
    Dropped,
    /// On hold (hold on Simkl, not supported on IMDB/Trakt)
    Hold,
}


