use anyhow::Result;
use chromiumoxide::{Browser, Page};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

/// Generate IMDB CSV exports for watchlist, ratings, and check-ins
/// Returns when all exports are ready (or timeout)
pub async fn generate_exports(
    browser: &Browser,
    sync_watchlist: bool,
    sync_ratings: bool,
    sync_watch_history: bool,
    remove_watched_from_watchlists: bool,
    mark_rated_as_watched: bool,
) -> Result<()> {
    let page = browser.new_page("https://www.imdb.com").await?;

    // Generate watchlist export if needed
    if sync_watchlist || remove_watched_from_watchlists {
        let _export_generated = generate_watchlist_export(&page).await?;
        // Note: We don't use the return value here because generate_exports is called
        // from multiple places and we want to maintain backward compatibility.
        // The empty check is handled in get_watchlist() instead.
    }

    // Generate ratings export if needed
    if sync_ratings || mark_rated_as_watched {
        generate_ratings_export(&page).await?;
    }

    // Generate check-ins export if needed
    if sync_watch_history || remove_watched_from_watchlists || mark_rated_as_watched {
        generate_checkins_export(&page).await?;
    }

    // Wait for exports to be ready
    wait_for_exports_ready(&page).await?;

    Ok(())
}

/// Check if watchlist is empty by looking for empty state indicators
async fn is_watchlist_empty(page: &Page) -> Result<bool> {
    // First check page text for the exact empty state message
    // Based on actual IMDB HTML: <div class="sc-b9995ff0-4 fTcYPM">This list is empty.</div>
    let page_text = match page.evaluate("document.body.innerText").await {
        Ok(result) => {
            result.value()
                .and_then(|v| v.as_str().map(|s| s.to_lowercase()))
                .unwrap_or_default()
        }
        Err(_) => String::new(),
    };
    
    // Check for the exact empty state text (most reliable)
    if page_text.contains("this list is empty") {
        return Ok(true);
    }
    
    // Also check for the specific empty state element by class
    // Note: The class "sc-b9995ff0-4" may be dynamically generated, so we verify text content
    let empty_selectors = [
        ".sc-b9995ff0-4",  // The specific class for empty state
        "[data-testid='empty-watchlist']",
        ".empty-state",
        ".ipc-empty-state",
    ];
    
    for selector in &empty_selectors {
        match page.find_element(*selector).await {
            Ok(element) => {
                // Verify it contains the empty text
                if let Ok(Some(text)) = element.inner_text().await {
                    if text.to_lowercase().contains("this list is empty") 
                        || text.to_lowercase().contains("list is empty") {
                        return Ok(true);
                    }
                }
            }
            Err(_) => continue,
        }
    }
    
    // Check for other common empty state text patterns
    let empty_indicators = [
        "your watchlist is empty",
        "no titles in your watchlist",
        "add titles to your watchlist",
        "start building your watchlist",
        "nothing in your watchlist",
    ];
    
    for indicator in &empty_indicators {
        if page_text.contains(*indicator) {
            return Ok(true);
        }
    }
    
    Ok(false)
}

async fn generate_watchlist_export(page: &Page) -> Result<bool> {
    // Returns Ok(true) if export was generated, Ok(false) if list is empty
    info!("Generating IMDB watchlist export");
    page.goto("https://www.imdb.com/list/watchlist").await?;
    sleep(Duration::from_secs(2)).await;

    // Check if watchlist is empty
    if is_watchlist_empty(page).await? {
        info!("IMDB watchlist is empty, skipping export generation");
        return Ok(false);
    }

    // Try to click export button
    match click_export_button(page).await {
        Ok(_) => {
            sleep(Duration::from_secs(3)).await;
            Ok(true)
        }
        Err(e) => {
            // If button not found, assume list is empty
            warn!("Export button not found (list may be empty): {}", e);
            Ok(false)
        }
    }
}

async fn generate_ratings_export(page: &Page) -> Result<()> {
    info!("Generating IMDB ratings export");
    page.goto("https://www.imdb.com/list/ratings").await?;
    sleep(Duration::from_secs(2)).await;

    // Click export button
    match click_export_button(page).await {
        Ok(_) => {
            info!("Successfully clicked ratings export button");
            sleep(Duration::from_secs(3)).await;
        }
        Err(e) => {
            warn!("Export button not found or click failed (list may be empty): {}", e);
            // Don't return error - empty list is valid
        }
    }

    Ok(())
}

async fn generate_checkins_export(page: &Page) -> Result<()> {
    info!("Generating IMDB check-ins export");
    page.goto("https://www.imdb.com/list/checkins").await?;
    sleep(Duration::from_secs(2)).await;

    // Debug: Check if the page loaded correctly and log URL
    let current_url = page.url().await?.unwrap_or_default();
    tracing::debug!("On check-ins page: {}", current_url.as_str());

    // Click export button
    if let Err(e) = click_export_button(page).await {
        warn!("Export button not found or click failed for check-ins (list may be empty): {}", e);
        tracing::debug!("Check-ins list may be empty or export button not available");
    } else {
        tracing::debug!("Successfully clicked check-ins export button");
        sleep(Duration::from_secs(3)).await;
    }

    Ok(())
}

async fn click_export_button(page: &Page) -> Result<()> {
    // Wait for export button and click it (matching Python implementation)
    // Python: export_button = wait.until(EC.element_to_be_clickable(...))
    let selector = "div[data-testid*='hero-list-subnav-export-button'] button";
    
    // Wait for element to be present and clickable
    let mut attempts = 0;
    let element = loop {
        match page.find_element(selector).await {
            Ok(el) => break el,
            Err(_) if attempts < 20 => {
                sleep(Duration::from_millis(500)).await;
                attempts += 1;
            }
            Err(e) => return Err(anyhow::anyhow!("Export button not found: {}", e)),
        }
    };
    
    // Scroll into view (matching Python: driver.execute_script("arguments[0].scrollIntoView(true);", export_button))
    element.scroll_into_view().await?;
    
    // Wait for visibility (matching Python: wait.until(EC.visibility_of(export_button)))
    // Check if element is visible by checking its bounding box
    let mut visibility_attempts = 0;
    while visibility_attempts < 20 {
        if let Ok(bbox) = element.bounding_box().await {
            if bbox.width > 0.0 && bbox.height > 0.0 {
                break; // Element is visible
            }
        }
        sleep(Duration::from_millis(100)).await;
        visibility_attempts += 1;
    }
    
    sleep(Duration::from_secs(1)).await;
    
    // Click (matching Python: driver.execute_script("arguments[0].click();", export_button))
    element.click().await?;
    
    Ok(())
}

async fn wait_for_exports_ready(page: &Page) -> Result<()> {
    const MAX_WAIT_TIME: Duration = Duration::from_secs(1200); // 20 minutes (matching Python: max_wait_time = 1200)
    const CHECK_INTERVAL: Duration = Duration::from_secs(30); // Matching Python: time.sleep(30)
    let start = std::time::Instant::now();

    loop {
        // Load exports page (matching Python: EH.get_page_with_retries('https://www.imdb.com/exports/', driver, wait))
        page.goto("https://www.imdb.com/exports/").await?;
        sleep(Duration::from_secs(2)).await;

        // Locate all elements with the selector (matching Python: summary_items = wait.until(EC.presence_of_all_elements_located(...)))
        let items = match page.find_elements(".ipc-metadata-list-summary-item").await {
            Ok(items) => items,
            Err(_) => {
                // No items found (matching Python: except TimeoutException: print("No items found...") break)
                info!("No items found when attempting to generate IMDB exports. Assuming no IMDB watchlist, ratings or check-ins to download.");
                break;
            }
        };

        // Check if any summary item contains "in progress" (matching Python: check_in_progress function)
        let mut in_progress = false;
        for item in items {
            let text = item.inner_text().await?.unwrap_or_default();
            if text.to_lowercase().contains("in progress") {
                in_progress = true;
                break;
            }
        }

        if !in_progress {
            info!("All IMDB exports are ready");
            break;
        }

        if start.elapsed() >= MAX_WAIT_TIME {
            return Err(anyhow::anyhow!("IMDB export processing did not complete within 20 minutes"));
        }

        info!("Exports still in progress, waiting...");
        sleep(CHECK_INTERVAL).await;
    }

    Ok(())
}

