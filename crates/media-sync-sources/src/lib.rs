pub mod traits;
pub mod capabilities;
pub mod factory;
pub mod imdb;
pub mod trakt;
pub mod plex;
pub mod simkl;
pub mod error;

pub use traits::MediaSource;
pub use capabilities::{IncrementalSync, StatusMapping, RatingNormalization, CapabilityRegistry, IdExtraction, IdLookupProvider};
pub use factory::{SourceFactory, SourceFactoryRegistry};
pub use error::SourceError;
pub use trakt::trakt_authenticate;
pub use simkl::simkl_authenticate;
