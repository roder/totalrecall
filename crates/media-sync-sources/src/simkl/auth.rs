use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration as StdDuration;
use tokio::time::sleep;

const TOKEN_URL: &str = "https://api.simkl.com/oauth/token";
const PIN_URL: &str = "https://api.simkl.com/oauth/pin";

/// Create a reqwest Client with browser-like headers
pub fn create_simkl_client() -> Client {
    Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .unwrap_or_else(|_| Client::new())
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

fn default_expires_in() -> u64 {
    3600 // Default to 1 hour (3600 seconds) if not provided
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceCodeResponse {
    user_code: String,
    device_code: String,
    verification_url: String,
    expires_in: u64,
    interval: Option<u64>, // Polling interval in seconds
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
    let client = create_simkl_client();

    if let Some(refresh_token) = refresh_token {
        if !refresh_token.is_empty() {
        // Try to refresh the token
        match refresh_access_token(&client, client_id, client_secret, refresh_token).await {
            Ok(token_info) => return Ok(token_info),
            Err(_) => {
                // Refresh failed, fall through to new authorization
                }
            }
        }
    }

    // New authorization flow using device code
    authorize_with_device_code(client_id).await
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
        "grant_type": "refresh_token"
    });

    let response = client
        .post(TOKEN_URL)
        .json(&payload)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Token refresh failed: {}", response.status()));
    }

    let token_response: TokenResponse = response.json().await?;
    let expires_at = Utc::now() + Duration::seconds(token_response.expires_in as i64 - 120);

    let refresh_token = token_response.refresh_token.unwrap_or_default();
    if refresh_token.is_empty() {
        tracing::warn!("Simkl API did not return a refresh_token. Token refresh will not be possible.");
    }

    Ok(TokenInfo {
        access_token: token_response.access_token,
        refresh_token,
        expires_at,
    })
}

async fn authorize_with_device_code(client_id: &str) -> Result<TokenInfo> {
    let client = create_simkl_client();

    // Step 1: Request device code (GET /oauth/pin?client_id=...)
    let url = format!("{}?client_id={}", PIN_URL, client_id);

    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Failed to request device code: {} - {}",
            status,
            error_text
        ));
    }

    let response_text = response.text().await?;
    let device_code_response: DeviceCodeResponse = serde_json::from_str(&response_text)
        .map_err(|e| {
            tracing::error!("Failed to parse device code response: {}. Raw response: {}", e, response_text);
            anyhow!("Failed to parse device code response: {}", e)
        })?;

    // Step 2: Display PIN and verification URL to user
    eprintln!("\n╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║          Simkl Device Authorization Required                ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!("\n1. Visit this URL in your browser:");
    eprintln!("   {}\n", device_code_response.verification_url);
    eprintln!("2. Enter this PIN when prompted:");
    eprintln!("   {}\n", device_code_response.user_code);
    eprintln!("3. Waiting for authorization...");
    eprintln!("   (This window will automatically continue once you authorize)\n");

    // Step 3: Poll status endpoint (GET /oauth/pin/{USER_CODE}?client_id=...)
    // Per Simkl API docs: https://simkl.docs.apiary.io/#reference/authentication-pin/get-code-status/check-user_code
    // Responses: {"result": "KO", "message": "Authorization pending"} or {"result": "OK", "access_token": "..."}
    let poll_interval = device_code_response.interval.unwrap_or(5);
    let expires_at = Utc::now() + Duration::seconds(device_code_response.expires_in as i64);
    let mut attempts = 0;
    let max_attempts = device_code_response.expires_in / poll_interval;

    // Wait a bit before first poll to give user time to see the PIN
    eprintln!("Waiting {} seconds before first poll...", poll_interval);
    sleep(StdDuration::from_secs(poll_interval)).await;

    // Poll using GET /oauth/pin/{USER_CODE}?client_id=...
    loop {
        // Check if expired
        if Utc::now() >= expires_at {
            return Err(anyhow!("Device code expired. Please try again."));
        }

        // Poll using GET /oauth/pin/{USER_CODE}?client_id=...
        let status_url = format!("{}/{}/?client_id={}", PIN_URL, device_code_response.user_code, client_id);
        let response = client
            .get(&status_url)
            .header("Accept", "application/json")
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;
        
        if !status.is_success() {
            return Err(anyhow!(
                "Unexpected error during authorization: {} - {}",
                status,
                response_text
            ));
        }

        // Parse the response according to API spec
        #[derive(Debug, Serialize, Deserialize)]
        struct StatusResponse {
            result: String,
            message: Option<String>,
            access_token: Option<String>,
        }

        let status_response: StatusResponse = serde_json::from_str(&response_text)
            .map_err(|e| {
                tracing::error!("Failed to parse status response: {}. Raw response: {}", e, response_text);
                anyhow!("Failed to parse status response: {}", e)
            })?;

        match status_response.result.as_str() {
            "OK" => {
                // Authorization successful!
                let access_token = status_response.access_token
                    .ok_or_else(|| anyhow!("Authorization successful but no access_token in response"))?;
                // According to Simkl API docs, access tokens never expire
                // Don't set an expiration time - this allows the token to be treated as never expiring
                eprintln!("\n✓ Authorization successful!\n");
                return Ok(TokenInfo {
                    access_token,
                    refresh_token: String::new(), // Simkl PIN flow doesn't return refresh_token
                    expires_at: Utc::now() + Duration::days(365 * 100), // Set far future date to indicate "never expires"
                });
            }
            "KO" => {
                // Check the message to determine next action
                match status_response.message.as_deref() {
                    Some("Authorization pending") => {
                        // Still waiting, continue polling
                        attempts += 1;
                        if attempts % 4 == 0 {
                            eprint!(".");
                            use std::io::{self, Write};
                            io::stderr().flush().ok();
                        }
                    }
                    Some("Slow down") => {
                        sleep(StdDuration::from_secs(poll_interval * 2)).await;
                        continue;
                    }
                    Some(msg) => {
                        return Err(anyhow!("Authorization failed: {}", msg));
                    }
                    None => {
                        return Err(anyhow!("Authorization failed: Unknown error"));
                    }
                }
            }
            _ => {
                return Err(anyhow!("Unexpected result in status response: {}", status_response.result));
            }
        }

        // Wait before next poll (using the interval from the device code response)
        sleep(StdDuration::from_secs(poll_interval)).await;

        // Safety check
        if attempts >= max_attempts {
            return Err(anyhow!("Maximum polling attempts reached. Please try again."));
        }
    }
}
