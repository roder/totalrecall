use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const REDIRECT_URI: &str = "urn:ietf:wg:oauth:2.0:oob";
const TOKEN_URL: &str = "https://api.trakt.tv/oauth/token";
const AUTHORIZE_URL: &str = "https://trakt.tv/oauth/authorize";

/// Create a reqwest Client with browser-like headers to bypass Cloudflare
pub fn create_trakt_client() -> Client {
    Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .unwrap_or_else(|_| Client::new())
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

#[derive(Debug)]
pub struct TokenInfo {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn authenticate(
    client_id: &str,
    client_secret: &str,
    refresh_token: Option<&str>,
) -> Result<TokenInfo> {
    let client = create_trakt_client();

    if let Some(refresh_token) = refresh_token {
        // Try to refresh the token
        match refresh_access_token(&client, client_id, client_secret, refresh_token).await {
            Ok(token_info) => return Ok(token_info),
            Err(_) => {
                // Refresh failed, fall through to new authorization
            }
        }
    }

    // New authorization flow
    authorize_new(client_id, client_secret).await
}

async fn refresh_access_token(
    client: &Client,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenInfo> {
    let payload = serde_json::json!({
        "refresh_token": refresh_token,
        "client_id": client_id,
        "client_secret": client_secret,
        "redirect_uri": REDIRECT_URI,
        "grant_type": "refresh_token"
    });

    let response = client
        .post(TOKEN_URL)
        .json(&payload)
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .header("Origin", "https://trakt.tv")
        .header("Referer", "https://trakt.tv/")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Token refresh failed: {}", response.status()));
    }

    let token_response: TokenResponse = response.json().await?;
    let expires_at = Utc::now() + Duration::seconds(token_response.expires_in as i64 - 120);

    Ok(TokenInfo {
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
        expires_at,
    })
}

async fn authorize_new(client_id: &str, client_secret: &str) -> Result<TokenInfo> {
    // Generate authorization URL
    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}",
        AUTHORIZE_URL, client_id, REDIRECT_URI
    );

    println!("\nPlease visit the following URL to authorize this application:");
    println!("{}\n", auth_url);

    // Prompt for authorization code
    use std::io::{self, Write};
    print!("Please enter the authorization code from the URL: ");
    io::stdout().flush()?;

    let mut code = String::new();
    io::stdin().read_line(&mut code)?;
    let code = code.trim();

    if code.is_empty() {
        return Err(anyhow!("Authorization code cannot be empty"));
    }

    // Exchange code for tokens
    let client = create_trakt_client();
    let payload = serde_json::json!({
        "code": code,
        "client_id": client_id,
        "client_secret": client_secret,
        "redirect_uri": REDIRECT_URI,
        "grant_type": "authorization_code"
    });

    let response = client
        .post(TOKEN_URL)
        .json(&payload)
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .header("Origin", "https://trakt.tv")
        .header("Referer", "https://trakt.tv/")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Failed to exchange authorization code: {} - {}",
            status,
            error_text
        ));
    }

    let token_response: TokenResponse = response.json().await?;
    let expires_at = Utc::now() + Duration::seconds(token_response.expires_in as i64 - 120);

    Ok(TokenInfo {
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
        expires_at,
    })
}
