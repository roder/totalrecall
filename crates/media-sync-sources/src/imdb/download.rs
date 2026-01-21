use anyhow::{anyhow, Result};
use chromiumoxide::Page;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;
use tracing::{debug, info, warn};
use dirs;

/// Download IMDB CSV exports and save to directory
/// Returns paths to downloaded files
/// 
/// This function is idempotent - if a file was already downloaded in this session,
/// it will reuse the cached path instead of re-downloading.
pub async fn download_exports(
    page: &Page,
    download_dir: &Path,
    sync_watchlist: bool,
    sync_ratings: bool,
    sync_watch_history: bool,
    remove_watched_from_watchlists: bool,
    mark_rated_as_watched: bool,
    cached_files: &std::collections::HashMap<String, PathBuf>,
) -> Result<DownloadedFiles> {
    use std::fs;
    
    // Load page (matching Python: success, status_code, url, driver, wait = EH.get_page_with_retries('https://www.imdb.com/exports/', driver, wait))
    page.goto("https://www.imdb.com/exports/").await?;
    sleep(Duration::from_secs(2)).await;

    // Clear any previous csv files (matching Python: lines 161-164)
    if let Ok(entries) = fs::read_dir(download_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("csv") {
                let _ = fs::remove_file(&path); // Ignore errors
            }
        }
    }

    let mut files = DownloadedFiles {
        watchlist: None,
        ratings: None,
        checkins: None,
    };

    // Find and download watchlist CSV (with caching)
    if sync_watchlist || remove_watched_from_watchlists {
        let cache_key = "watchlist".to_string();
        if let Some(cached_path) = cached_files.get(&cache_key) {
            if cached_path.exists() {
                info!("Reusing cached watchlist CSV: {:?}", cached_path);
                files.watchlist = Some(cached_path.clone());
            } else {
                // Cached file no longer exists, re-download
                match download_csv(page, download_dir, "watchlist", cached_files).await {
                    Ok(path) => {
                        files.watchlist = Some(path);
                    }
                    Err(e) => {
                        warn!("Failed to download watchlist CSV: {}", e);
                    }
                }
            }
        } else {
            match download_csv(page, download_dir, "watchlist", cached_files).await {
                Ok(path) => {
                    files.watchlist = Some(path);
                }
                Err(e) => {
                    warn!("Failed to download watchlist CSV: {}", e);
                }
            }
        }
    }

    // Find and download ratings CSV (with caching)
    if sync_ratings || mark_rated_as_watched {
        let cache_key = "ratings".to_string();
        if let Some(cached_path) = cached_files.get(&cache_key) {
            if cached_path.exists() {
                info!("Reusing cached ratings CSV: {:?}", cached_path);
                files.ratings = Some(cached_path.clone());
            } else {
                match download_csv(page, download_dir, "ratings", cached_files).await {
                    Ok(path) => {
                        files.ratings = Some(path);
                    }
                    Err(e) => {
                        warn!("Failed to download ratings CSV: {}", e);
                    }
                }
            }
        } else {
            match download_csv(page, download_dir, "ratings", cached_files).await {
                Ok(path) => {
                    files.ratings = Some(path);
                }
                Err(e) => {
                    warn!("Failed to download ratings CSV: {}", e);
                }
            }
        }
    }

    // Find and download check-ins CSV (with caching)
    if sync_watch_history || remove_watched_from_watchlists || mark_rated_as_watched {
        let cache_key = "check-ins".to_string();
        if let Some(cached_path) = cached_files.get(&cache_key) {
            if cached_path.exists() {
                info!("Reusing cached check-ins CSV: {:?}", cached_path);
                files.checkins = Some(cached_path.clone());
            } else {
                match download_csv(page, download_dir, "check-ins", cached_files).await {
                    Ok(path) => {
                        files.checkins = Some(path);
                    }
                    Err(e) => {
                        warn!("Failed to download check-ins CSV: {}", e);
                    }
                }
            }
        } else {
            match download_csv(page, download_dir, "check-ins", cached_files).await {
                Ok(path) => {
                    files.checkins = Some(path);
                }
                Err(e) => {
                    warn!("Failed to download check-ins CSV: {}", e);
                }
            }
        }
    }

    Ok(files)
}

async fn download_csv(
    page: &Page,
    download_dir: &Path,
    file_type: &str,
    _cached_files: &std::collections::HashMap<String, PathBuf>,
) -> Result<PathBuf> {
    use std::fs;
    
    debug!("Looking for {} export button", file_type);
    
    // Find the export item matching file_type (matching Python: find_button(item_text))
    let selector = ".ipc-metadata-list-summary-item";
    let items = page.find_elements(selector).await?;
    
    debug!("Found {} export items on page", items.len());

    for (idx, item) in items.iter().enumerate() {
        let text = item.inner_text().await?.unwrap_or_default();
        debug!("Export item {} text: {}", idx, text);
        
        if text.to_lowercase().contains(file_type) {
            info!("Found matching export item for {}: {}", file_type, text);
            
            // Check if export is unavailable (e.g., "no export available", "export not available")
            let text_lower = text.to_lowercase();
            if text_lower.contains("no export available") 
                || text_lower.contains("export not available")
                || text_lower.contains("unavailable") {
                warn!("Export for {} is not available (likely empty list or export not ready). Item text: {}", file_type, text);
                return Err(anyhow!("Export not available for {}: {}", file_type, text));
            }
            
            // Check if export is still in progress
            if text_lower.contains("in progress") {
                warn!("Export for {} is still in progress. Item text: {}", file_type, text);
                return Err(anyhow!("Export for {} is still in progress. Wait for it to complete before downloading.", file_type));
            }
            
            // Find download button (matching Python: button[data-testid*='export-status-button'])
            let button_selector = "button[data-testid*='export-status-button']";
            match item.find_element(button_selector).await {
                Ok(button) => {
                    info!("Found download button for {}", file_type);
                    
                    // Scroll into view (matching Python: driver.execute_script("arguments[0].scrollIntoView(true);", csv_link))
                    button.scroll_into_view().await?;
                    
                    // Wait for visibility (matching Python: wait.until(EC.visibility_of(csv_link)))
                    let mut visibility_attempts = 0;
                    while visibility_attempts < 20 {
                        if let Ok(bbox) = button.bounding_box().await {
                            if bbox.width > 0.0 && bbox.height > 0.0 {
                                debug!("Button is visible (width: {}, height: {})", bbox.width, bbox.height);
                                break; // Element is visible
                            }
                        }
                        sleep(Duration::from_millis(100)).await;
                        visibility_attempts += 1;
                    }
                    
                    // Check files before clicking
                    let files_before: Vec<PathBuf> = std::fs::read_dir(download_dir)?
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("csv"))
                        .collect();
                    debug!("CSV files before click: {:?}", files_before);
                    
                    // Click to download (matching Python: driver.execute_script("arguments[0].click();", csv_link))
                    info!("Clicking download button for {}", file_type);
                    button.click().await?;
                    
                    // Wait for download with polling (matching Python: time.sleep(10))
                    // Poll for new file to appear - check every second for up to 20 seconds
                    let mut download_attempts = 0;
                    let max_attempts = 20; // 20 seconds total
                    let mut csv_file: Option<PathBuf> = None;
                    
                    while download_attempts < max_attempts {
                        sleep(Duration::from_secs(1)).await;
                        
                        // Periodically check if page is still responsive (every 5 seconds)
                        // This helps detect browser crashes during download
                        if download_attempts > 0 && download_attempts % 5 == 0 {
                            // Try a simple operation to check if browser/page is still alive
                            // Use evaluate to run a simple JavaScript check
                            match page.evaluate("document.readyState").await {
                                Ok(_) => {
                                    debug!("Page is still responsive (attempt {})", download_attempts + 1);
                                }
                                Err(e) => {
                                    warn!("Page became unresponsive during download polling (attempt {}): {}. Browser may have crashed.", download_attempts + 1, e);
                                    return Err(anyhow!("Browser/page became unresponsive during download: {}", e));
                                }
                            }
                        }
                        
                        // Check for new CSV files
                        let files_after: Vec<PathBuf> = std::fs::read_dir(download_dir)?
                            .filter_map(|e| e.ok())
                            .map(|e| e.path())
                            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("csv"))
                            .collect();
                        
                        debug!("CSV files after {} seconds: {:?}", download_attempts + 1, files_after);
                        
                        // Check if a new file appeared (not in files_before)
                        for file in &files_after {
                            if !files_before.contains(file) {
                                csv_file = Some(file.clone());
                                debug!("Found new CSV file: {:?}", csv_file);
                                break;
                            }
                        }
                        
                        if csv_file.is_some() {
                            break;
                        }
                        
                        download_attempts += 1;
                    }
                    
                    // If no new file appeared, try finding the latest file in multiple locations
                    let csv_file = if let Some(file) = csv_file {
                        file
                    } else {
                        warn!("No new CSV file detected after {} seconds of polling. File may not have downloaded, or download may have taken longer than expected.", max_attempts);
                        debug!("No new file detected in configured directory, searching common download locations");
                        // Try configured directory first
                        match find_latest_csv(download_dir) {
                            Ok(file) => {
                                info!("Found existing CSV file in download directory (may be from previous download): {:?}", file);
                                file
                            }
                            Err(e) => {
                                debug!("No CSV files found in configured download directory: {}", e);
                                // Try common download locations
                                let mut search_dirs = vec![];
                                
                                // User's Downloads folder
                                if let Some(home) = dirs::home_dir() {
                                    search_dirs.push(home.join("Downloads"));
                                    #[cfg(target_os = "macos")]
                                    {
                                        search_dirs.push(home.join("Downloads"));
                                    }
                                    #[cfg(target_os = "linux")]
                                    {
                                        search_dirs.push(home.join("Downloads"));
                                    }
                                }
                                
                                // Also check the user data directory's default download location
                                if let Some(data_dir) = dirs::data_dir() {
                                    search_dirs.push(data_dir.join("totalrecall").join("browser").join("Default").join("Downloads"));
                                }
                                
                                // Search all locations
                                let mut found_file: Option<PathBuf> = None;
                                let mut latest_time = std::time::UNIX_EPOCH;
                                
                                for search_dir in &search_dirs {
                                    debug!("Searching for CSV files in: {:?}", search_dir);
                                    if let Ok(file) = find_latest_csv(search_dir) {
                                        if let Ok(metadata) = file.metadata() {
                                            if let Ok(modified) = metadata.modified() {
                                                if modified > latest_time {
                                                    latest_time = modified;
                                                    found_file = Some(file);
                                                    info!("Found CSV file in: {:?}", search_dir);
                                                }
                                            }
                                        }
                                    }
                                }
                                
                                found_file.ok_or_else(|| {
                                    warn!("CSV file for {} not found after download. Checked: configured directory ({:?}) and {} common download locations", file_type, download_dir, search_dirs.len());
                                    anyhow!("No CSV file found in configured directory ({:?}) or common download locations after clicking download button. The export may not have been ready, or the download may have failed.", download_dir)
                                })?
                            }
                        }
                    };
                    debug!("Using CSV file: {:?}", csv_file);
                    
                    // Generate unique filename with timestamp to avoid conflicts
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos();
                    let base_name = match file_type {
                        "watchlist" => "watchlist",
                        "ratings" => "ratings",
                        "check-ins" => "checkins",
                        _ => return Err(anyhow!("Unknown file type: {}", file_type)),
                    };
                    let file_name = format!("{}_{}.csv", base_name, timestamp);
                    
                    let dest_path = download_dir.join(&file_name);
                    
                    // Grant permissions (matching Python: os.chmod(src_path, 0o777))
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&csv_file, fs::Permissions::from_mode(0o777))?;
                    }
                    
                    // If file is not already in the download directory, move it there
                    // Otherwise just rename it
                    if csv_file.parent() != Some(download_dir) {
                        info!("Moving CSV file from {:?} to download directory {:?}", csv_file, download_dir);
                        // Ensure download directory exists
                        fs::create_dir_all(download_dir)?;
                        // Remove existing file if it exists
                        if dest_path.exists() {
                            fs::remove_file(&dest_path)?;
                        }
                        // Move file to download directory
                        fs::rename(&csv_file, &dest_path)?;
                    } else {
                        // File is already in download directory, just rename it
                        // Remove existing file if it exists
                        if dest_path.exists() {
                            fs::remove_file(&dest_path)?;
                        }
                        fs::rename(&csv_file, &dest_path)?;
                    }
                    
                    info!("Downloaded and renamed {} to {:?}", file_type, dest_path);
                    
                    return Ok(dest_path);
                }
                Err(e) => {
                    warn!("Download button not found in export item {} for {}: {}. Item text: {}", idx, file_type, e, text);
                }
            }
        }
    }

    warn!("No export item matching '{}' found after checking {} items on exports page", file_type, items.len());
    if items.is_empty() {
        warn!("No export items found on page at all. The exports page may not have loaded correctly, or there may be no exports available.");
    } else {
        debug!("Available export items on page:");
        for (idx, item) in items.iter().enumerate() {
            if let Ok(text) = item.inner_text().await {
                debug!("  Item {}: {}", idx, text.unwrap_or_default());
            }
        }
    }
    Err(anyhow!("No export button found for {} after checking {} items", file_type, items.len()))
}

fn find_latest_csv(dir: &Path) -> Result<PathBuf> {
    // Find latest CSV file (matching Python: sorted by os.path.getmtime, reverse=True)
    // First check if directory exists
    if !dir.exists() {
        return Err(anyhow!("Download directory does not exist: {:?}", dir));
    }
    
    let mut csv_files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension()?.to_str()? == "csv" {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    if csv_files.is_empty() {
        return Err(anyhow!("No CSV files found in directory: {:?}", dir));
    }

    // Sort by modification time (most recent first)
    csv_files.sort_by(|a, b| {
        let a_time = a.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        let b_time = b.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        b_time.cmp(&a_time)
    });

    Ok(csv_files.into_iter().next().unwrap())
}

#[derive(Debug)]
pub struct DownloadedFiles {
    pub watchlist: Option<PathBuf>,
    pub ratings: Option<PathBuf>,
    pub checkins: Option<PathBuf>,
}

