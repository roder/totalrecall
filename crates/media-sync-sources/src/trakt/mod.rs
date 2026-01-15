pub mod client;
pub mod auth;
pub mod api;

pub use client::TraktClient;
pub use auth::authenticate as trakt_authenticate;

