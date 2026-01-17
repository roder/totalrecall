use anyhow::Result;
use chromiumoxide::Page;
use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem};
use crate::ProgressTracker;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info, trace, warn};
use browser_debug::PageInspector;

/// Add items to IMDB watchlist
pub async fn add_to_watchlist(page: &Page, items: &[WatchlistItem]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let progress_interval = if items.len() < 50 { 10 } else { 50 };
    let mut tracker = ProgressTracker::with_operation_name(
        items.len(),
        progress_interval,
        Some("IMDB watchlist add".to_string()),
    );

    for (idx, item) in items.iter().enumerate() {
        let current = idx + 1;
        trace!(
            "Adding item {} of {} to IMDB watchlist: {} ({})",
            current,
            items.len(),
            item.title,
            item.imdb_id
        );

        let url = format!("https://www.imdb.com/title/{}/", item.imdb_id);
        page.goto(&url).await?;
        sleep(Duration::from_secs(2)).await;

        let current_url = page.url().await?.unwrap_or_default();
        let current_url_str = current_url.as_str();

        if current_url_str.contains("/reference") {
            // Reference view - use different selector
            let button_selector = ".titlereference-watch-ribbon > .wl-ribbon";
            match page.find_element(button_selector).await {
                Ok(button) => {
                    button.scroll_into_view().await?;
                    sleep(Duration::from_secs(1)).await;

                    let classes = button
                        .attribute("class")
                        .await?
                        .unwrap_or_default();
                    
                    if classes.contains("not-inWL") {
                        button.click().await?;
                        sleep(Duration::from_secs(1)).await;
                        trace!("Added {} to IMDB watchlist (reference view)", item.title);
                        tracker.record_added();
                    } else {
                        trace!("{} already in IMDB watchlist (reference view)", item.title);
                        tracker.record_already_present();
                    }
                }
                Err(e) => {
                    warn!("Failed to find watchlist button for {}: {}", item.imdb_id, e);
                }
            }
        } else {
            // Normal view
            // Wait for loader to disappear
            let loader_selector = "[data-testid=\"tm-box-wl-loader\"]";
            let mut attempts = 0;
            while attempts < 10 {
                match page.find_element(loader_selector).await {
                    Ok(_) => {
                        sleep(Duration::from_millis(500)).await;
                        attempts += 1;
                    }
                    Err(_) => break, // Loader disappeared
                }
            }

            let button_selector = "button[data-testid=\"tm-box-wl-button\"]";
            match page.find_element(button_selector).await {
                Ok(button) => {
                    button.scroll_into_view().await?;
                    sleep(Duration::from_secs(1)).await;

                    // Check if already in watchlist by looking for done icon
                    let inner_html = button
                        .inner_html()
                        .await?
                        .unwrap_or_default();

                    if !inner_html.contains("ipc-icon--done") {
                        // Not in watchlist, click to add
                        let mut retry_count = 0;
                        while retry_count < 2 {
                            button.click().await?;
                            sleep(Duration::from_secs(1)).await;

                            // Check for confirmation
                            match page
                                .find_element("button[data-testid=\"tm-box-wl-button\"] .ipc-icon--done")
                                .await
                            {
                                Ok(_) => {
                                    trace!("Added {} to IMDB watchlist", item.title);
                                    tracker.record_added();
                                    break;
                                }
                                Err(_) => {
                                    retry_count += 1;
                                    if retry_count >= 2 {
                                        warn!("Failed to add {} to IMDB watchlist after retries", item.title);
                                        tracker.record_failed();
                                    }
                                }
                            }
                        }
                    } else {
                        trace!("{} already in IMDB watchlist", item.title);
                        tracker.record_already_present();
                    }
                }
                Err(e) => {
                    warn!("Failed to find watchlist button for {}: {}", item.imdb_id, e);
                    tracker.record_failed();
                }
            }
        }

        tracker.log_progress(current);
    }

    tracker.log_summary("IMDB watchlist add");
    Ok(())
}

/// Remove items from IMDB watchlist
pub async fn remove_from_watchlist(page: &Page, items: &[WatchlistItem]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let progress_interval = if items.len() < 50 { 10 } else { 50 };
    let mut tracker = ProgressTracker::with_operation_name(
        items.len(),
        progress_interval,
        Some("IMDB watchlist remove".to_string()),
    );

    for (idx, item) in items.iter().enumerate() {
        let current = idx + 1;
        trace!(
            "Removing item {} of {} from IMDB watchlist: {} ({})",
            current,
            items.len(),
            item.title,
            item.imdb_id
        );

        let url = format!("https://www.imdb.com/title/{}/", item.imdb_id);
        page.goto(&url).await?;
        sleep(Duration::from_secs(2)).await;

        let current_url = page.url().await?.unwrap_or_default();
        let current_url_str = current_url.as_str();

        if current_url_str.contains("/reference") {
            // Reference view
            let button_selector = ".titlereference-watch-ribbon > .wl-ribbon";
            match page.find_element(button_selector).await {
                Ok(button) => {
                    button.scroll_into_view().await?;
                    sleep(Duration::from_secs(1)).await;

                    let classes = button
                        .attribute("class")
                        .await?
                        .unwrap_or_default();
                    
                    if !classes.contains("not-inWL") {
                        button.click().await?;
                        sleep(Duration::from_secs(1)).await;
                        trace!("Removed {} from IMDB watchlist (reference view)", item.title);
                        tracker.record_added();
                    } else {
                        trace!("{} not in IMDB watchlist (reference view)", item.title);
                        tracker.record_skipped();
                    }
                }
                Err(e) => {
                    warn!("Failed to find watchlist button for {}: {}", item.imdb_id, e);
                }
            }
        } else {
            // Normal view
            // Wait for loader to disappear
            let loader_selector = "[data-testid=\"tm-box-wl-loader\"]";
            let mut attempts = 0;
            while attempts < 10 {
                match page.find_element(loader_selector).await {
                    Ok(_) => {
                        sleep(Duration::from_millis(500)).await;
                        attempts += 1;
                    }
                    Err(_) => break,
                }
            }

            let button_selector = "button[data-testid=\"tm-box-wl-button\"]";
            match page.find_element(button_selector).await {
                Ok(button) => {
                    button.scroll_into_view().await?;
                    sleep(Duration::from_secs(1)).await;

                    // Check if in watchlist (not-inWL means NOT in watchlist)
                    let inner_html = button
                        .inner_html()
                        .await?
                        .unwrap_or_default();

                    if !inner_html.contains("ipc-icon--add") {
                        // In watchlist, click to remove
                        let mut retry_count = 0;
                        while retry_count < 2 {
                            button.click().await?;
                            sleep(Duration::from_secs(1)).await;

                            // Check for confirmation (add icon appears)
                            match page
                                .find_element("button[data-testid=\"tm-box-wl-button\"] .ipc-icon--add")
                                .await
                            {
                                Ok(_) => {
                                    trace!("Removed {} from IMDB watchlist", item.title);
                                    tracker.record_added();
                                    break;
                                }
                                Err(_) => {
                                    retry_count += 1;
                                    if retry_count >= 2 {
                                        warn!("Failed to remove {} from IMDB watchlist after retries", item.title);
                                        tracker.record_failed();
                                    }
                                }
                            }
                        }
                    } else {
                        trace!("{} not in IMDB watchlist", item.title);
                        tracker.record_skipped();
                    }
                }
                Err(e) => {
                    warn!("Failed to find watchlist button for {}: {}", item.imdb_id, e);
                    tracker.record_failed();
                }
            }
        }

        tracker.log_progress(current);
    }

    tracker.log_summary("IMDB watchlist remove");
    Ok(())
}

/// Set ratings on IMDB
pub async fn set_ratings(
    page: &Page,
    ratings: &[Rating],
    mut inspector: Option<&mut PageInspector>,
) -> Result<()> {
    if ratings.is_empty() {
        return Ok(());
    }

    let progress_interval = if ratings.len() < 50 { 10 } else { 50 };
    let mut tracker = ProgressTracker::with_operation_name(
        ratings.len(),
        progress_interval,
        Some("IMDB ratings set".to_string()),
    );
    
    for (idx, rating) in ratings.iter().enumerate() {
        let current = idx + 1;
        trace!(
            "Setting rating {} of {} on IMDB: {} - {}/10",
            current,
            ratings.len(),
            rating.imdb_id,
            rating.rating
        );

        let url = format!("https://www.imdb.com/title/{}/", rating.imdb_id);
        
        // Handle navigation errors gracefully
        match page.goto(&url).await {
            Ok(_) => {
                sleep(Duration::from_secs(2)).await;
                
                // Capture debug state after navigation if inspector is provided
                if let Some(ref mut insp) = inspector {
                    let _ = insp.screenshot("navigate_to_page").await;
                    let _ = insp.save_page_html("navigate_to_page").await;
                }
            }
            Err(e) => {
                warn!("Failed to navigate to {}: {}", url, e);
                tracker.record_failed();
                tracker.log_progress(current);
                continue;
            }
        }

        let current_url = match page.url().await {
            Ok(url) => url.unwrap_or_default(),
            Err(e) => {
                warn!("Failed to get current URL for {}: {}", rating.imdb_id, e);
                tracker.record_failed();
                tracker.log_progress(current);
                continue;
            }
        };
        let current_url_str = current_url.as_str();

        // Try normal view first (most common)
        let mut rating_set = false;
        
        if !current_url_str.contains("/reference") {
            // Capture debug state before clicking rating button
            if let Some(ref mut insp) = inspector {
                let _ = insp.screenshot("before_click_rating_button").await;
            }
            
            // Normal view - try multiple selector strategies
            let result = try_set_rating_normal_view(page, rating).await;
            
            // Capture debug state after attempting to set rating
            if let Some(ref mut insp) = inspector {
                let _ = insp.screenshot("after_set_rating").await;
            }
            
            match result {
                Ok(true) => {
                    rating_set = true;
                    tracker.record_added();
                }
                Ok(false) => {
                    // Try reference view as fallback
                }
                Err(e) => {
                    warn!("Error setting rating for {} (normal view): {}", rating.imdb_id, e);
                }
            }
        }
        
        // If normal view failed or we're on reference view, try reference view
        if !rating_set {
            // Capture debug state before trying reference view
            if let Some(ref mut insp) = inspector {
                let _ = insp.screenshot("before_reference_view").await;
            }
            
            match try_set_rating_reference_view(page, rating).await {
                Ok(true) => {
                    tracker.record_added();
                }
                Ok(false) => {
                    warn!("Failed to set rating for {} - all selector strategies failed", rating.imdb_id);
                    tracker.record_failed();
                }
                Err(e) => {
                    warn!("Error setting rating for {} (reference view): {}", rating.imdb_id, e);
                    tracker.record_failed();
                }
            }
        }

        tracker.log_progress(current);
    }

    tracker.log_summary("IMDB ratings set");
    Ok(())
}

/// Try to set rating using normal view selectors
async fn try_set_rating_normal_view(page: &Page, rating: &Rating) -> Result<bool> {
    // Wait for page to be fully loaded
    sleep(Duration::from_secs(2)).await;
    
    // Wait for rating bar loader to disappear
    let loader_selector = "[data-testid=\"hero-rating-bar__loading\"]";
    let mut attempts = 0;
    while attempts < 10 {
        match page.find_element(loader_selector).await {
            Ok(_) => {
                sleep(Duration::from_millis(500)).await;
                attempts += 1;
            }
            Err(_) => break,
        }
    }

    // Try primary selector - use the exact structure from IMDB
    let button_selectors = vec![
        "[data-testid=\"hero-rating-bar__user-rating\"] button.ipc-btn",
        "[data-testid=\"hero-rating-bar__user-rating\"] button",
        "button[data-testid*=\"rating\"]",
        "[data-testid=\"hero-rating-bar__user-rating\"]",
        "button[aria-label*=\"Rate\"]",
        "button[aria-label*=\"Your rating\"]",
        ".ipc-rating-prompt__rating-button",
    ];

    for button_selector in button_selectors.iter() {
        match page.find_element(*button_selector).await {
            Ok(button) => {
                // Wait for button to be fully enabled (check multiple conditions)
                let mut attempts = 0;
                while attempts < 20 {
                    let aria_disabled = button
                        .attribute("aria-disabled")
                        .await?
                        .unwrap_or_default();
                    
                    let disabled_attr = button
                        .attribute("disabled")
                        .await?
                        .is_some();
                    
                    let class_attr = button
                        .attribute("class")
                        .await?
                        .unwrap_or_default();
                    let has_not_interactable = class_attr.contains("ipc-btn--not-interactable");
                    
                    // Button is enabled when:
                    // - aria-disabled is "false" or not set
                    // - disabled attribute is not present
                    // - class does not contain "ipc-btn--not-interactable"
                    if aria_disabled != "true" && !disabled_attr && !has_not_interactable {
                        break;
                    }
                    sleep(Duration::from_millis(500)).await;
                    attempts += 1;
                }

                // Check existing rating
                let existing_rating = match page
                    .find_element("[data-testid=\"hero-rating-bar__user-rating__score\"] span")
                    .await
                {
                    Ok(score_element) => {
                        let text = score_element
                            .inner_text()
                            .await?
                            .unwrap_or_default();
                        text.trim().parse::<u8>().ok()
                    }
                    Err(_) => None,
                };

                if existing_rating != Some(rating.rating) {
                    // Scroll button into view and wait for it to be visible
                    match button.scroll_into_view().await {
                        Ok(_) => {
                            sleep(Duration::from_millis(500)).await;
                        }
                        Err(e) => {
                            debug!("Failed to scroll rating button into view: {}", e);
                        }
                    }
                    
                    // Check if button is actually visible using bounding box
                    let mut is_visible = false;
                    for _ in 0..5 {
                        if let Ok(bbox) = button.bounding_box().await {
                            if bbox.width > 0.0 && bbox.height > 0.0 {
                                is_visible = true;
                                break;
                            }
                        }
                        sleep(Duration::from_millis(200)).await;
                    }
                    
                    if !is_visible {
                        debug!("Rating button is not visible, trying JavaScript click");
                    }
                    
                    // Try clicking with retries, fallback to JavaScript
                    let mut click_success = false;
                    for retry in 0..3 {
                        match button.click().await {
                            Ok(_) => {
                                click_success = true;
                                break;
                            }
                            Err(e) => {
                                if retry < 2 {
                                    debug!("Click attempt {} failed for rating button, retrying: {}", retry + 1, e);
                                    sleep(Duration::from_millis(500)).await;
                                } else {
                                    debug!("Direct click failed, trying JavaScript click: {}", e);
                                    // Try JavaScript click as fallback - use data-testid for reliability
                                    let js_code = format!(
                                        "(() => {{ const container = document.querySelector('[data-testid=\\\"hero-rating-bar__user-rating\\\"]'); if (container) {{ const btn = container.querySelector('button.ipc-btn:not(.ipc-btn--not-interactable):not([disabled])'); if (btn) {{ btn.scrollIntoView({{behavior: 'smooth', block: 'center'}}); setTimeout(() => {{ btn.click(); }}, 100); return true; }} }} return false; }})()"
                                    );
                                    if let Ok(result) = page.evaluate(js_code.as_str()).await {
                                        if let Some(value) = result.value() {
                                            if value.as_bool().unwrap_or(false) {
                                                click_success = true;
                                                sleep(Duration::from_millis(500)).await;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    
                    if !click_success {
                        debug!("All click methods failed for rating button");
                        return Ok(false);
                    }
                    
                    sleep(Duration::from_secs(2)).await; // Wait longer for dialog to appear

                    // Try multiple rating option selectors - expanded list
                    let rating_option_selectors = vec![
                        format!("button[aria-label=\"Rate {}\"]", rating.rating),
                        format!("button[aria-label=\"Rate {} out of 10\"]", rating.rating),
                        format!("button[aria-label=\"{} out of 10\"]", rating.rating),
                        format!("[data-value=\"{}\"]", rating.rating),
                        format!("button[data-rating=\"{}\"]", rating.rating),
                        format!("button[data-value=\"{}\"]", rating.rating),
                        format!(".ipc-rating-prompt__star-button[data-value=\"{}\"]", rating.rating),
                        format!("button:has-text(\"{}\")", rating.rating),
                    ];

                    for rating_option_selector in rating_option_selectors.iter() {
                        match page.find_element(rating_option_selector.as_str()).await {
                            Ok(rating_button) => {
                                // Scroll into view and wait
                                let _ = rating_button.scroll_into_view().await;
                                sleep(Duration::from_millis(500)).await;
                                
                                // Try clicking with retries, fallback to JavaScript
                                let mut rating_click_success = false;
                                for retry in 0..3 {
                                    match rating_button.click().await {
                                        Ok(_) => {
                                            rating_click_success = true;
                                            break;
                                        }
                                        Err(e) => {
                                            if retry < 2 {
                                                debug!("Click attempt {} failed for rating option, retrying: {}", retry + 1, e);
                                                sleep(Duration::from_millis(500)).await;
                                            } else {
                                                debug!("Direct click failed for rating option, trying JavaScript: {}", e);
                                                // Try JavaScript click as fallback
                                                let js_code = format!(
                                                    "(() => {{ const el = document.querySelector('{}'); if (el) {{ el.scrollIntoView({{behavior: 'smooth', block: 'center'}}); el.click(); return true; }} return false; }})()",
                                                    rating_option_selector.replace('"', "\\\"").replace('\'', "\\'").replace('[', "\\[").replace(']', "\\]")
                                                );
                                                if let Ok(result) = page.evaluate(js_code.as_str()).await {
                                                    if let Some(value) = result.value() {
                                                        if value.as_bool().unwrap_or(false) {
                                                            rating_click_success = true;
                                                            break;
                                                        }
                                                    }
                                                }
                                                continue; // Try next selector
                                            }
                                        }
                                    }
                                }
                                
                                if !rating_click_success {
                                    continue; // Try next selector
                                }
                                
                                sleep(Duration::from_secs(1)).await;

                                // Try multiple submit button selectors - wait a bit for dialog to fully render
                                sleep(Duration::from_millis(500)).await;
                                let submit_selectors = vec![
                                    "button.ipc-rating-prompt__rate-button",
                                    "button[type=\"submit\"]",
                                    "button[aria-label*=\"Submit\"]",
                                    "button[aria-label*=\"Confirm\"]",
                                    "button[aria-label*=\"Rate\"]",
                                    ".ipc-rating-prompt__rate-button",
                                    "button.ipc-btn--primary",
                                ];

                                for submit_selector in submit_selectors.iter() {
                                    match page.find_element(*submit_selector).await {
                                        Ok(submit_button) => {
                                            // Scroll into view and wait
                                            let _ = submit_button.scroll_into_view().await;
                                            sleep(Duration::from_millis(500)).await;
                                            
                                            // Try clicking with retries, fallback to JavaScript
                                            let mut submit_click_success = false;
                                            for retry in 0..3 {
                                                match submit_button.click().await {
                                                    Ok(_) => {
                                                        submit_click_success = true;
                                                        break;
                                                    }
                                                    Err(e) => {
                                                        if retry < 2 {
                                                            debug!("Click attempt {} failed for submit button, retrying: {}", retry + 1, e);
                                                            sleep(Duration::from_millis(500)).await;
                                                        } else {
                                                            debug!("Direct click failed for submit button, trying JavaScript: {}", e);
                                                            // Try JavaScript click as fallback
                                                            let js_code = format!(
                                                                "(() => {{ const el = document.querySelector('{}'); if (el) {{ el.scrollIntoView({{behavior: 'smooth', block: 'center'}}); el.click(); return true; }} return false; }})()",
                                                                submit_selector.replace('"', "\\\"").replace('\'', "\\'").replace('[', "\\[").replace(']', "\\]")
                                                            );
                                                            if let Ok(result) = page.evaluate(js_code.as_str()).await {
                                                                if let Some(value) = result.value() {
                                                                    if value.as_bool().unwrap_or(false) {
                                                                        submit_click_success = true;
                                                                        break;
                                                                    }
                                                                }
                                                            }
                                                            continue; // Try next selector
                                                        }
                                                    }
                                                }
                                            }
                                            
                                            if submit_click_success {
                                                sleep(Duration::from_secs(1)).await;
                                                trace!("Set rating {}/10 for {} on IMDB", rating.rating, rating.imdb_id);
                                                return Ok(true);
                                            }
                                        }
                                        Err(e) => {
                                            debug!("Failed to find submit button with selector {}: {}", *submit_selector, e);
                                        }
                                    }
                                }
                                
                                // If no submit button found, rating might be set directly
                                trace!("Set rating {}/10 for {} on IMDB (no submit button found)", rating.rating, rating.imdb_id);
                                return Ok(true);
                            }
                            Err(e) => {
                                debug!("Failed to find rating option with selector {}: {}", rating_option_selector, e);
                            }
                        }
                    }
                } else {
                    trace!("Rating {}/10 already set for {} on IMDB", rating.rating, rating.imdb_id);
                    return Ok(true);
                }
            }
            Err(e) => {
                debug!("Failed to find rating button with selector {}: {}", button_selector, e);
            }
        }
    }
    
    Ok(false)
}

/// Try to set rating using reference view selectors (with fallbacks)
async fn try_set_rating_reference_view(page: &Page, rating: &Rating) -> Result<bool> {
    // Try multiple selector strategies for reference view
    let button_selectors = vec![
        ".ipl-rating-interactive__star-container", // Old selector (might still work in some cases)
        "[data-testid*=\"rating\"] button",
        "button[aria-label*=\"Rate\"]",
        ".rating-bar__base-button",
    ];

    for button_selector in button_selectors.iter() {
        match page.find_element(*button_selector).await {
            Ok(button) => {
                match button.scroll_into_view().await {
                    Ok(_) => {
                        sleep(Duration::from_secs(1)).await;
                    }
                    Err(e) => {
                        debug!("Failed to scroll rating button into view: {}", e);
                    }
                }
                
                // Try clicking with retries
                let mut click_success = false;
                for retry in 0..3 {
                    match button.click().await {
                        Ok(_) => {
                            click_success = true;
                            break;
                        }
                        Err(e) => {
                            if retry < 2 {
                                debug!("Click attempt {} failed for rating button, retrying: {}", retry + 1, e);
                                sleep(Duration::from_millis(500)).await;
                            } else {
                                debug!("Failed to click rating button after retries: {}", e);
                                continue; // Try next selector
                            }
                        }
                    }
                }
                
                if !click_success {
                    continue; // Try next selector
                }
                
                sleep(Duration::from_secs(1)).await;

                // Try multiple rating option selectors
                let rating_selectors = vec![
                    format!(".ipl-rating-selector__star-link[data-value=\"{}\"]", rating.rating), // Old selector
                    format!("button[aria-label=\"Rate {}\"]", rating.rating),
                    format!("[data-value=\"{}\"]", rating.rating),
                    format!("button[data-rating=\"{}\"]", rating.rating),
                ];

                for rating_selector in rating_selectors.iter() {
                    match page.find_element(rating_selector.as_str()).await {
                        Ok(rating_button) => {
                            // Scroll into view and wait
                            let _ = rating_button.scroll_into_view().await;
                            sleep(Duration::from_millis(500)).await;
                            
                            // Try clicking with retries
                            let mut rating_click_success = false;
                            for retry in 0..3 {
                                match rating_button.click().await {
                                    Ok(_) => {
                                        rating_click_success = true;
                                        break;
                                    }
                                    Err(e) => {
                                        if retry < 2 {
                                            debug!("Click attempt {} failed for rating option, retrying: {}", retry + 1, e);
                                            sleep(Duration::from_millis(500)).await;
                                        } else {
                                            debug!("Failed to click rating option after retries: {}", e);
                                            continue; // Try next selector
                                        }
                                    }
                                }
                            }
                            
                            if rating_click_success {
                                sleep(Duration::from_secs(1)).await;
                                trace!("Set rating {}/10 for {} on IMDB (reference view)", rating.rating, rating.imdb_id);
                                return Ok(true);
                            }
                        }
                        Err(e) => {
                            debug!("Failed to find rating option with selector {}: {}", rating_selector, e);
                        }
                    }
                }
            }
            Err(e) => {
                debug!("Failed to find rating button with selector {}: {}", *button_selector, e);
            }
        }
    }
    
    Ok(false)
}

/// Set reviews on IMDB
pub async fn set_reviews(
    page: &Page,
    reviews: &[Review],
    last_submitted: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<()> {
    // Check rate limiting (10 days)
    if let Some(last_date) = last_submitted {
        let days_since = (chrono::Utc::now() - last_date).num_days();
        if days_since < 10 {
            info!(
                "Reviews were submitted {} days ago. Skipping review submission (10 day limit).",
                days_since
            );
            return Ok(());
        }
    }

    if reviews.is_empty() {
        return Ok(());
    }

    let progress_interval = if reviews.len() < 25 { 10 } else { 25 };
    let mut tracker = ProgressTracker::with_operation_name(
        reviews.len(),
        progress_interval,
        Some("IMDB reviews set".to_string()),
    );

    for (idx, review) in reviews.iter().enumerate() {
        let current = idx + 1;
        trace!(
            "Setting review {} of {} on IMDB: {}",
            current,
            reviews.len(),
            review.imdb_id
        );

        let url = format!(
            "https://contribute.imdb.com/review/{}/add?bus=imdb",
            review.imdb_id
        );
        page.goto(&url).await?;
        sleep(Duration::from_secs(3)).await;

        // Find title and review inputs
        let title_input_selector = "#text-input__0";
        let review_input_selector = "#textarea__0";

                match page.find_element(title_input_selector).await {
            Ok(title_input) => {
                // Check if review already exists
                // Get value using attribute or JavaScript evaluation
                let title_value = title_input
                    .attribute("value")
                    .await?
                    .unwrap_or_default();

                let review_input = page.find_element(review_input_selector).await?;
                let review_value = review_input
                    .attribute("value")
                    .await?
                    .unwrap_or_default();

                if title_value.is_empty() && review_value.is_empty() {
                    // Clear existing inputs by clicking and selecting all
                    title_input.click().await?;
                    // Triple-click to select all
                    title_input.click().await?;
                    title_input.click().await?;
                    
                    review_input.click().await?;
                    review_input.click().await?;
                    review_input.click().await?;

                    // Set title to "My Review"
                    title_input.type_str("My Review").await?;
                    sleep(Duration::from_millis(500)).await;

                    // Set review content
                    review_input.type_str(&review.content).await?;
                    sleep(Duration::from_millis(500)).await;

                    // Set spoiler radio button
                    let spoiler_selector = if review.is_spoiler {
                        "#is_spoiler-1" // Yes
                    } else {
                        "#is_spoiler-0" // No
                    };

                    match page.find_element(spoiler_selector).await {
                        Ok(spoiler_button) => {
                            spoiler_button.click().await?;
                            sleep(Duration::from_millis(500)).await;
                        }
                        Err(e) => {
                            warn!("Failed to find spoiler button for {}: {}", review.imdb_id, e);
                        }
                    }

                    // Submit
                    match page.find_element("button[aria-label='Submit']").await {
                        Ok(submit_button) => {
                            submit_button.click().await?;
                            sleep(Duration::from_secs(3)).await;
                            trace!("Submitted review for {} on IMDB", review.imdb_id);
                            tracker.record_added();
                        }
                        Err(e) => {
                            warn!("Failed to find submit button for {}: {}", review.imdb_id, e);
                            tracker.record_failed();
                        }
                    }
                } else {
                    trace!("Review already exists for {} on IMDB", review.imdb_id);
                    tracker.record_already_present();
                }
            }
            Err(e) => {
                warn!("Failed to find title input for {}: {}", review.imdb_id, e);
                tracker.record_failed();
            }
        }

        tracker.log_progress(current);
    }

    tracker.log_summary("IMDB reviews set");
    Ok(())
}

/// Add watch history (check-ins) on IMDB
pub async fn add_watch_history(page: &Page, items: &[WatchHistory]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let progress_interval = if items.len() < 50 { 10 } else { 50 };
    let mut tracker = ProgressTracker::with_operation_name(
        items.len(),
        progress_interval,
        Some("IMDB check-ins add".to_string()),
    );

    for (idx, item) in items.iter().enumerate() {
        let current = idx + 1;
        trace!(
            "Adding check-in {} of {} on IMDB: {} ({})",
            current,
            items.len(),
            item.imdb_id,
            item.watched_at
        );

        let url = format!("https://www.imdb.com/title/{}/", item.imdb_id);
        page.goto(&url).await?;
        sleep(Duration::from_millis(500)).await; // Reduced from 2s - page should load faster

        let current_url = page.url().await?.unwrap_or_default();
        let current_url_str = current_url.as_str();

        // Skip reference view (not supported)
        if current_url_str.contains("/reference") {
            trace!("Skipping check-in for {} (reference view not supported)", item.imdb_id);
            tracker.record_skipped();
            tracker.log_progress(current);
            continue;
        }

        // Wait for loader to disappear (reduced attempts for speed)
        let loader_selector = "[data-testid=\"tm-box-wl-loader\"]";
        let mut attempts = 0;
        while attempts < 2 {
            match page.find_element(loader_selector).await {
                Ok(_) => {
                    sleep(Duration::from_millis(300)).await;
                    attempts += 1;
                }
                Err(_) => break,
            }
        }

        let button_selector = "button[data-testid=\"tm-box-addtolist-button\"]";
        match page.find_element(button_selector).await {
            Ok(button) => {
                button.scroll_into_view().await?;
                sleep(Duration::from_millis(300)).await; // Reduced from 1s

                button.click().await?;
                sleep(Duration::from_millis(300)).await; // Reduced from 1s

                // Wait for dropdown menu to appear before trying to find "Your check-ins" option
                // Reduced attempts for speed - dropdown usually appears quickly
                let mut checkins_element = None;
                let mut attempts = 0;
                while attempts < 3 {
                    match page.find_xpath("//div[contains(text(), 'Your check-ins')]").await {
                        Ok(element) => {
                            checkins_element = Some(element);
                            break;
                        }
                        Err(_) => {
                            attempts += 1;
                            if attempts < 3 {
                                sleep(Duration::from_millis(150)).await;
                            }
                        }
                    }
                }

                // Find "Your check-ins" option using XPath (matching Python: //div[contains(text(), 'Your check-ins')])
                match checkins_element {
                    Some(checkins_element) => {
                        // Check if already in check-ins (matching Python: 'true' not in watch_history_button.get_attribute('data-titleinlist'))
                        let data_titleinlist = checkins_element
                            .attribute("data-titleinlist")
                            .await?
                            .unwrap_or_default();

                        if data_titleinlist != "true" {
                            // Not in check-ins, click to add
                            let mut retry_count = 0;
                            while retry_count < 2 {
                                checkins_element.click().await?;
                                sleep(Duration::from_millis(400)).await; // Reduced from 1s

                                // Verify it was added (matching Python: WebDriverWait until presence_of_element_located with data-titleinlist='true')
                                match page.find_xpath("//div[contains(@class, 'ipc-promptable-base__content')]//div[@data-titleinlist='true']").await {
                                    Ok(_) => {
                                        trace!("Added check-in for {} on IMDB", item.imdb_id);
                                        tracker.record_added();
                                        break;
                                    }
                                    Err(_) => {
                                        retry_count += 1;
                                        if retry_count >= 2 {
                                            trace!("Failed to verify check-in for {} after retries", item.imdb_id);
                                            tracker.record_failed();
                                        }
                                    }
                                }
                            }
                        } else {
                            trace!("{} already in IMDB check-ins", item.imdb_id);
                            tracker.record_already_present();
                        }
                    }
                    None => {
                        trace!("Failed to find 'Your check-ins' option for {} after {} attempts: dropdown menu may not have appeared", item.imdb_id, attempts);
                        tracker.record_failed();
                    }
                }
            }
            Err(e) => {
                trace!("Failed to find add to list button for {}: {}", item.imdb_id, e);
                tracker.record_failed();
            }
        }

        tracker.log_progress(current);
    }

    tracker.log_summary("IMDB check-ins add");
    Ok(())
}
