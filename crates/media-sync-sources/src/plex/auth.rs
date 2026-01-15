use anyhow::Result;
use reqwest::Client;
use tracing::info;

const PLEX_TV_BASE_URL: &str = "https://plex.tv";

/// Verify that a token is valid by making an API call
pub async fn verify_token(token: &str) -> Result<bool> {
    let client = Client::new();
    let url = format!("{}/api/v2/user", PLEX_TV_BASE_URL);
    
    let response = client
        .get(&url)
        .header("X-Plex-Token", token)
        .header("Accept", "application/json")
        .send()
        .await?;
    
    Ok(response.status().is_success())
}

/// Authenticate with token (for compatibility, returns token)
/// This is now a simple pass-through since we use direct HTTP calls
pub fn authenticate_with_token(token: String) -> Result<String> {
    info!("Plex token provided for authentication");
    Ok(token)
}

