use anyhow::{anyhow, Result};
use chromiumoxide::Page;
use chrono::{DateTime, NaiveDate, Utc};
use media_sync_models::{media::MediaType, Review};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

/// Scrape reviews from IMDB profile page
pub async fn scrape_reviews(page: &Page) -> Result<Vec<Review>> {
    info!("Scraping IMDB reviews from profile page");

    // Navigate to profile page
    page.goto("https://www.imdb.com/profile").await?;
    sleep(Duration::from_secs(3)).await;

    // Wait until URL contains "user/"
    let mut attempts = 0;
    while attempts < 10 {
        let current_url = page.url().await?.unwrap_or_default();
        if current_url.as_str().contains("user/") {
            break;
        }
        sleep(Duration::from_millis(500)).await;
        attempts += 1;
    }

    // Get current URL and append "reviews/"
    let current_url = page.url().await?.unwrap_or_default();
    let reviews_url = format!("{}/reviews/", current_url.as_str().trim_end_matches('/'));
    
    page.goto(&reviews_url).await?;
    sleep(Duration::from_secs(3)).await;

    let mut all_reviews = Vec::new();
    let mut page_num = 1;

    loop {
        info!("Scraping reviews page {}", page_num);

        // Find all review elements
        let review_selector = "div[data-testid='review-card-parent']";
        let review_elements = page.find_elements(review_selector).await?;

        if review_elements.is_empty() {
            info!("No more reviews found, stopping pagination");
            break;
        }

        for element in review_elements {
            // Extract title
            let title = match element.find_element("div[data-testid='review-title-header'] h3 span").await {
                Ok(title_elem) => title_elem.inner_text().await.ok().flatten().unwrap_or_default(),
                Err(_) => String::new(),
            };

            // Extract year from review date
            let year = match element.find_element("li.review-date").await {
                Ok(date_elem) => {
                    date_elem.inner_text().await.ok().flatten().and_then(|text| {
                        // Try to extract year from date text (e.g., "January 1, 2024")
                        text.split_whitespace()
                            .last()
                            .and_then(|s| s.trim_end_matches(',').parse::<u32>().ok())
                    })
                }
                Err(_) => None,
            };

            // Extract IMDB_ID from link
            let imdb_id = match element.find_element("div[data-testid='review-summary'] a").await {
                Ok(link_elem) => {
                    link_elem.attribute("href").await.ok().flatten().and_then(|href| {
                        // Parse href like "/title/tt1234567/reviews/rw1234567"
                        // We need the title ID (tt...), not the review ID (rw...)
                        let parts: Vec<&str> = href.split('/').collect();
                        // parts[0] = "" (empty before first /)
                        // parts[1] = "title"
                        // parts[2] = "tt1234567" (this is what we want!)
                        // parts[3] = "reviews"
                        // parts[4] = "rw1234567" (this is the review ID, not what we want)
                        if parts.len() > 2 && parts[1] == "title" {
                            Some(parts[2].to_string()) // Title ID (tt...)
                        } else {
                            None
                        }
                    }).unwrap_or_default()
                }
                Err(_) => String::new(),
            };

            if imdb_id.is_empty() {
                warn!("Skipping review with missing IMDB_ID");
                continue;
            }

            // Extract comment
            let comment = match element.find_element("div[data-testid='review-overflow']").await {
                Ok(comment_elem) => comment_elem.inner_text().await.ok().flatten().unwrap_or_default(),
                Err(_) => String::new(),
            };

            // Extract spoiler status
            let is_spoiler = element.find_element(".review-spoiler-button").await.is_ok();

            // Get media type via Trakt API (would need to be passed in or fetched separately)
            // For now, default to Movie - this should be enhanced to actually query Trakt
            let media_type = MediaType::Movie;

            // Use current time as date_added (IMDB doesn't provide review date in this format)
            let date_added = Utc::now();

            all_reviews.push(Review {
                imdb_id,
                ids: None,
                content: comment,
                date_added,
                media_type,
                source: "imdb".to_string(),
                is_spoiler,
            });
        }

        // Check for next page button
        let next_button_selector = "div[data-testid='index-pagination-nxt']";
        match page.find_element(next_button_selector).await {
            Ok(next_button) => {
                // Check if disabled
                let aria_disabled = next_button
                    .attribute("aria-disabled")
                    .await?
                    .unwrap_or_default();
                
                if aria_disabled == "true" {
                    info!("Next page button is disabled, stopping pagination");
                    break;
                }

                // Scroll into view and click
                page.evaluate("arguments[0].scrollIntoView(true);").await?;
                sleep(Duration::from_secs(1)).await;
                
                let url_before = page.url().await?.unwrap_or_default();
                next_button.click().await?;
                sleep(Duration::from_secs(2)).await;

                // Wait for URL to change
                let mut attempts = 0;
                while attempts < 10 {
                    let url_after = page.url().await?.unwrap_or_default();
                    if url_after != url_before {
                        break;
                    }
                    sleep(Duration::from_millis(500)).await;
                    attempts += 1;
                }

                page_num += 1;
            }
            Err(_) => {
                info!("Next page button not found, stopping pagination");
                break;
            }
        }
    }

    // Filter duplicates by IMDB_ID (keep first occurrence)
    let mut seen_ids = std::collections::HashSet::new();
    all_reviews.retain(|review| {
        if seen_ids.contains(&review.imdb_id) {
            false
        } else {
            seen_ids.insert(review.imdb_id.clone());
            true
        }
    });

    // Remove items with unknown type (would need to check against Trakt)
    // For now, we'll keep all reviews

    info!("Scraped {} reviews from IMDB", all_reviews.len());
    Ok(all_reviews)
}

