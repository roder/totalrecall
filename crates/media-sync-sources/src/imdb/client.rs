use crate::traits::MediaSource;
use crate::capabilities::{RatingNormalization, CapabilityRegistry, StatusMapping, IncrementalSync, IdExtraction, IdLookupProvider};
use crate::imdb::{auth, export, download, parser, actions, reviews};
use anyhow::{anyhow, Result};
use chrono::Utc;
use chromiumoxide::{Browser, BrowserConfig, Page};
use chromiumoxide::fetcher::{BrowserFetcher, BrowserFetcherOptions};
use media_sync_config;
use media_sync_models::{Rating, Review, WatchHistory, WatchlistItem};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use which::which;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::FutureExt;
use browser_debug::PageInspector;

pub struct ImdbClient {
    browser: Option<Browser>,
    handler_task: Option<tokio::task::JoinHandle<()>>,
    authenticated: bool,
    username: String,
    password: String,
    download_dir: PathBuf,
    user_data_dir: PathBuf,
    session_id: String,
    downloaded_files: std::sync::Mutex<std::collections::HashMap<String, PathBuf>>,
    debug_config: browser_debug::config::DebugConfig,
}

impl ImdbClient {
    pub async fn new(username: String, password: String) -> Result<Self> {
        Self::new_with_debug(username, password, browser_debug::config::DebugConfig::from_env()).await
    }
    
    pub async fn new_with_debug(username: String, password: String, debug_config: browser_debug::config::DebugConfig) -> Result<Self> {
        // Setup directories
        let user_data_dir = Self::get_user_data_dir()?;
        let session_id = Self::generate_session_id();
        let download_dir = Self::get_download_dir(&session_id)?;
        
        // Initialize browser
        let (browser, handler_task) = Self::initialize_browser_internal(&user_data_dir, &download_dir).await?;
        
        Ok(Self {
            browser: Some(browser),
            handler_task: Some(handler_task),
            authenticated: false,
            username,
            password,
            download_dir,
            user_data_dir,
            session_id,
            downloaded_files: std::sync::Mutex::new(std::collections::HashMap::new()),
            debug_config,
        })
    }
    
    /// Internal helper to initialize the browser instance
    /// This is called both during construction and for lazy initialization
    async fn initialize_browser_internal(
        user_data_dir: &Path,
        download_dir: &Path,
    ) -> Result<(Browser, tokio::task::JoinHandle<()>)> {
        // Find system Chromium
        let mut chrome_path = Self::find_system_chromium();
        
        // If no system Chromium found, use BrowserFetcher to download it
        // Based on: https://github.com/mattsse/chromiumoxide?tab=readme-ov-file#fetcher
        if chrome_path.is_none() {
            info!("No system Chromium found, downloading via BrowserFetcher...");
            let fetcher_download_path = user_data_dir.parent()
                .ok_or_else(|| anyhow!("Could not determine parent directory"))?
                .join("chromium_downloads");
            tokio::fs::create_dir_all(&fetcher_download_path).await?;
            
            let fetcher = BrowserFetcher::new(
                BrowserFetcherOptions::builder()
                    .with_path(&fetcher_download_path)
                    .build()
                    .map_err(|e| anyhow!("Failed to create BrowserFetcherOptions: {}", e))?,
            );
            
            let info = fetcher.fetch().await
                .map_err(|e| anyhow!("Failed to fetch Chromium: {}", e))?;
            
            let mut downloaded_path = info.executable_path;
            
            // On macOS, remove quarantine attribute from downloaded Chromium
            // This is needed because macOS Gatekeeper blocks unsigned binaries
            #[cfg(target_os = "macos")]
            {
                use std::process::Command;
                let _ = Command::new("xattr")
                    .arg("-d")
                    .arg("com.apple.quarantine")
                    .arg(&downloaded_path)
                    .output();
                // Also remove quarantine from the .app bundle if it's a bundle
                if let Some(parent) = downloaded_path.parent() {
                    if parent.ends_with("MacOS") {
                        if let Some(bundle) = parent.parent().and_then(|p| p.parent()) {
                            let _ = Command::new("xattr")
                                .arg("-d")
                                .arg("com.apple.quarantine")
                                .arg(bundle)
                                .output();
                        }
                    }
                }
            }
            
            chrome_path = Some(downloaded_path);
            info!("Chromium downloaded to: {:?}", chrome_path);
        }
        
        // Build browser config with the found/downloaded Chromium
        let config = Self::build_browser_config(
            chrome_path.as_ref().map(|p| p.clone()),
            user_data_dir,
            download_dir,
        )?;
        
        // Launch browser
        // On macOS, log the config for debugging
        #[cfg(target_os = "macos")]
        {
            debug!("Launching browser on macOS with config: executable={:?}, args will be set by chromiumoxide", chrome_path);
        }
        
        let (browser, mut handler) = Browser::launch(config).await
            .map_err(|e| {
                // Provide more context on macOS
                #[cfg(target_os = "macos")]
                {
                    anyhow!("Failed to launch browser on macOS: {}. This might be due to macOS security restrictions. Try: xattr -d com.apple.quarantine /Applications/Chromium.app", e)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    anyhow!("Failed to launch browser: {}", e)
                }
            })?;
        
        // Spawn handler task with enhanced error detection
        let handler_task = tokio::spawn(async move {
            let mut error_count = 0;
            const MAX_ERRORS: usize = 10;
            
            while let Some(h) = handler.next().await {
                match h {
                    Ok(_) => {
                        // Reset error count on successful message
                        error_count = 0;
                    }
                    Err(e) => {
                        error_count += 1;
                        warn!(
                            "Browser handler error (count: {}/{}): {:?}",
                            error_count, MAX_ERRORS, e
                        );
                        
                        // If we get too many errors, the browser is likely crashed
                        if error_count >= MAX_ERRORS {
                            error!(
                                "Browser handler received {} consecutive errors. Browser process may have crashed.",
                                error_count
                            );
                            break;
                        }
                    }
                }
            }
            
            if error_count > 0 {
                error!("Browser handler task ended after {} errors. Browser may have crashed.", error_count);
            } else {
                info!("Browser handler task ended normally");
            }
        });
        
        Ok((browser, handler_task))
    }
    
    /// Ensure browser is initialized, initializing it lazily if needed
    /// This allows the browser to be re-initialized after shutdown for subsequent syncs
    async fn ensure_browser_initialized(&mut self) -> Result<()> {
        if self.browser.is_some() {
            // Browser already initialized
            return Ok(());
        }
        
        info!("Browser not initialized, initializing lazily...");
        
        // Generate new session ID and download directory for this sync
        let session_id = Self::generate_session_id();
        let download_dir = Self::get_download_dir(&session_id)?;
        
        // Initialize browser
        let (browser, handler_task) = Self::initialize_browser_internal(&self.user_data_dir, &download_dir).await?;
        
        // Update client state
        self.browser = Some(browser);
        self.handler_task = Some(handler_task);
        self.session_id = session_id;
        self.download_dir = download_dir;
        
        // Reset authentication state since we have a new browser instance
        self.authenticated = false;
        
        info!("Browser initialized successfully");
        Ok(())
    }
    
    /// Check if we're running in Docker
    fn is_docker() -> bool {
        std::path::Path::new("/.dockerenv").exists() ||
        std::fs::read_to_string("/proc/self/cgroup")
            .ok()
            .map(|s| s.contains("docker") || s.contains("containerd"))
            .unwrap_or(false)
    }
    
    /// Remove macOS quarantine attribute (Gatekeeper can block execution)
    #[cfg(target_os = "macos")]
    fn remove_macos_quarantine(path: &Path) {
        use std::process::Command;
        let _ = Command::new("xattr")
            .arg("-d")
            .arg("com.apple.quarantine")
            .arg(path)
            .output();
        // Silently fail if we don't have permission - user can run manually if needed
    }
    
    #[cfg(not(target_os = "macos"))]
    fn remove_macos_quarantine(_path: &Path) {
        // No-op on non-macOS
    }
    
    /// Find system Chromium with enhanced detection
    fn find_system_chromium() -> Option<PathBuf> {
        let is_docker = Self::is_docker();
        let is_macos = cfg!(target_os = "macos");
        
        // Docker-specific paths (highest priority in Docker)
        if is_docker {
            let docker_paths = [
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
            ];
            for path in &docker_paths {
                if std::path::Path::new(path).exists() {
                    return Some(PathBuf::from(path));
                }
            }
        }
        
        // macOS-specific paths
        if is_macos {
            // Try .app bundle paths first (chromiumoxide may prefer these)
            let app_bundle_paths = [
                "/Applications/Chromium.app",
                "/Applications/Google Chrome.app",
            ];
            for app_path in &app_bundle_paths {
                let app_bundle = std::path::Path::new(app_path);
                if app_bundle.exists() && app_bundle.is_dir() {
                    // Check if the executable exists inside the bundle
                    let executable = app_bundle.join("Contents/MacOS/Chromium");
                    if !executable.exists() {
                        // Try Google Chrome executable name
                        let executable = app_bundle.join("Contents/MacOS/Google Chrome");
                        if executable.exists() {
                            // Remove quarantine attribute on macOS (Gatekeeper can block execution)
                            Self::remove_macos_quarantine(&executable);
                            Self::remove_macos_quarantine(app_bundle);
                            return Some(executable);
                        }
                    } else {
                        // Remove quarantine attribute on macOS (Gatekeeper can block execution)
                        Self::remove_macos_quarantine(&executable);
                        Self::remove_macos_quarantine(app_bundle);
                        return Some(executable);
                    }
                }
            }
            
            // Fall back to direct executable paths
            let macos_paths = [
                "/Applications/Chromium.app/Contents/MacOS/Chromium",
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "/opt/homebrew/bin/chromium",
                "/usr/local/bin/chromium",
            ];
            for path in &macos_paths {
                let path_buf = PathBuf::from(path);
                if path_buf.exists() {
                    // Remove quarantine attribute on macOS (Gatekeeper can block execution)
                    Self::remove_macos_quarantine(&path_buf);
                    if let Some(app_bundle) = path_buf.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
                        Self::remove_macos_quarantine(&app_bundle);
                    }
                    return Some(path_buf);
                }
            }
        }
        
        // Standard system paths (Linux)
        let system_paths = [
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/local/bin/chromium",
            "/usr/local/bin/chromium-browser",
            "/opt/chromium/chromium",
        ];
        
        for path in &system_paths {
            if std::path::Path::new(path).exists() {
                return Some(PathBuf::from(path));
            }
        }
        
        // PATH lookup
        which("chromium")
            .or_else(|_| which("chromium-browser"))
            .ok()
    }
    
    /// Build browser configuration with all necessary options
    fn build_browser_config(
        chrome_path: Option<PathBuf>,
        user_data_dir: &Path,
        download_dir: &Path,
    ) -> Result<BrowserConfig> {
        let mut builder = BrowserConfig::builder();
        
        // Set executable if system Chromium found
        if let Some(path) = chrome_path {
            builder = builder.chrome_executable(path);
            info!("Configuring browser with system Chromium");
        } else {
            info!("Configuring browser with browser fetch (no system Chromium)");
            // When using fetcher, we don't set chrome_executable
            // The fetcher will download Chromium automatically during launch
        }
        
        // Detect platform
        let is_docker = Self::is_docker();
        let is_macos = cfg!(target_os = "macos");
        
        // Configure headless mode (always in Docker, configurable otherwise)
        if is_docker {
            builder = builder.arg("--headless=new");
        }
        // For local dev, headless is optional - could make this configurable
        
        // Browser arguments (security and performance)
        // Linux/Docker-specific flags
        if is_docker || !is_macos {
            builder = builder
                .arg("--no-sandbox")  // Required for Docker, but problematic on macOS
                .arg("--disable-dev-shm-usage");  // Prevents /dev/shm issues in Docker (Linux-specific)
        }
        
        // Common flags for all platforms
        builder = builder
            .arg("--disable-extensions")  // Disable extensions
            .arg("--disable-notifications")  // Disable notifications
            .arg("--disable-third-party-cookies")  // Privacy
            .arg("--log-level=3")  // Reduce logging
            .arg("--disable-features=WebAuthentication")  // Disable WebAuthn/passkey to force password authentication
            .arg(format!("--download-directory={}", download_dir.display()))  // Set default download directory
            // Memory optimization flags
            .arg("--memory-pressure-off")  // Disable memory pressure handling
            .arg("--aggressive-cache-discard")  // Aggressively discard cache
            .arg("--disk-cache-size=1")  // Minimize disk cache (1MB)
            .arg("--disable-features=site-per-process")  // Reduce process isolation overhead
            .arg("--js-flags=--optimize_for_size")  // Optimize JS for memory footprint
            // CPU optimization flags
            .arg("--disable-background-timer-throttling")  // Prevent background throttling
            .arg("--disable-renderer-backgrounding")  // Keep renderer active
            .arg("--disable-backgrounding-occluded-windows")  // Keep windows active
            .arg("--disable-plugins")  // Disable plugin system
            .arg("--disable-sync")  // Disable Chrome sync
            .arg("--disable-default-apps")  // Disable default apps
            .arg("--window-size=800,600");  // Smaller viewport = less memory for rendering
        
        // Platform-specific flags
        if is_docker {
            // Docker/headless specific
            builder = builder
                .arg("--disable-gpu")  // Not needed in headless
                .arg("--start-maximized")  // Maximize window (even in headless)
                .arg("--disable-crash-reporter")  // Disable crashpad handler (fixes crashpad database error)
                .arg("--disable-breakpad");  // Disable breakpad crash reporting
        }
        
        // macOS-specific stability flags
        if is_macos && !is_docker {
            builder = builder
                .arg("--disable-setuid-sandbox")  // macOS doesn't support setuid sandbox
                .arg("--disable-background-networking");  // Reduce background activity
            // Note: CPU optimization flags are now in common flags section above
        }
        
        // User data directory (for persistent sessions)
        builder = builder.arg(format!("--user-data-dir={}", user_data_dir.display()));
        
        // User agent (platform-specific)
        if is_docker || !is_macos {
            // Linux user agent for Docker/Linux
            builder = builder.arg("--user-agent=Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");
        } else {
            // macOS user agent
            builder = builder.arg("--user-agent=Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");
        }
        
        // Build config
        builder.build()
            .map_err(|e| anyhow!("Failed to build browser config: {}", e))
    }
    
    
    /// Get user data directory for browser state persistence
    fn get_user_data_dir() -> Result<PathBuf> {
        // If in container, use container data directory
        use media_sync_config::container_base_path;
        let container_base = container_base_path();
        if container_base.exists() {
            let user_data_dir = container_base.join("data").join("browser");
            std::fs::create_dir_all(&user_data_dir)?;
            return Ok(user_data_dir);
        }
        
        // Use dirs crate for platform-specific paths
        let base = dirs::data_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
            .ok_or_else(|| anyhow!("Could not determine user data directory"))?;
        
        let user_data_dir = base.join("totalrecall").join("browser");
        std::fs::create_dir_all(&user_data_dir)?;
        Ok(user_data_dir)
    }
    
    /// Get download directory for CSV exports - creates a unique session-specific directory
    fn get_download_dir(session_id: &str) -> Result<PathBuf> {
        // Use temp directory with unique session ID
        let download_dir = std::env::temp_dir()
            .join("totalrecall_exports")
            .join(session_id);
        std::fs::create_dir_all(&download_dir)?;
        Ok(download_dir)
    }
    
    /// Generate a unique session ID
    fn generate_session_id() -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("session_{}", timestamp)
    }
    
    /// Configure download behavior via CDP on the page that will trigger downloads
    async fn configure_downloads(page: &Page, download_dir: &Path) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::browser::{SetDownloadBehaviorParams, SetDownloadBehaviorBehavior};
        
        // Ensure download directory exists
        std::fs::create_dir_all(download_dir)?;
        
        // Configure download behavior via CDP
        let download_path = download_dir.to_string_lossy().to_string();
        let params = SetDownloadBehaviorParams {
            behavior: SetDownloadBehaviorBehavior::Allow,
            download_path: Some(download_path.clone()),
            browser_context_id: None,
            events_enabled: None,
        };
        
        // Execute on the page that will trigger downloads
        page.execute(params).await
            .map_err(|e| anyhow!("Failed to configure download behavior: {}", e))?;
        
        // Also try to set it via JavaScript as a fallback
        let js = format!(
            r#"
            (function() {{
                // Try to set download attribute behavior
                document.addEventListener('click', function(e) {{
                    if (e.target.tagName === 'A' || e.target.closest('a')) {{
                        const link = e.target.tagName === 'A' ? e.target : e.target.closest('a');
                        if (link.href && link.href.includes('.csv')) {{
                            link.setAttribute('download', '');
                        }}
                    }}
                }}, true);
            }})();
            "#
        );
        let _ = page.evaluate(js).await; // Ignore errors, this is just a fallback
        
        info!("Download directory configured: {:?}", download_dir);
        info!("Browser download behavior configured via CDP on exports page");
        Ok(())
    }
    
    /// Configure resource blocking to reduce CPU and memory usage
    /// Blocks non-essential resources like images, CSS, fonts, and media
    async fn configure_resource_blocking(page: &Page) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::SetBlockedUrLsParams;
        
        // Block non-essential resources
        let blocked_patterns: Vec<String> = vec![
            "*.jpg", "*.jpeg", "*.png", "*.gif", "*.webp",  // Images
            "*.css",  // Stylesheets (if not needed for functionality)
            "*.woff", "*.woff2", "*.ttf", "*.otf",  // Fonts
            "*.mp4", "*.webm", "*.mp3",  // Media
        ].into_iter().map(|s| s.to_string()).collect();
        
        let params = SetBlockedUrLsParams {
            urls: blocked_patterns,
        };
        
        page.execute(params).await
            .map_err(|e| anyhow!("Failed to configure resource blocking: {}", e))?;
        
        debug!("Resource blocking configured for page");
        Ok(())
    }
    
    pub async fn authenticate(&mut self) -> Result<()> {
        // Ensure browser is initialized (lazy initialization)
        self.ensure_browser_initialized().await?;
        
        let browser = self.browser.as_ref()
            .ok_or_else(|| anyhow!("Browser not initialized"))?;
        
        let username = self.username.clone();
        let password = self.password.clone();
        
        Self::with_page(browser, "about:blank", false, |page| async move {
            // Use auth module to authenticate
            auth::authenticate(page, &username, &password).await?;
            Ok(())
        }.boxed()).await?;
        
        self.authenticated = true;
        Ok(())
    }
    
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }
    
    /// Explicitly shutdown the browser instance
    /// Should be called when sync job completes to free resources
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(browser) = self.browser.take() {
            info!("Shutting down browser instance");
            
            // Wait for handler task to complete
            if let Some(handler_task) = self.handler_task.take() {
                // Give handler a moment to finish
                let _ = tokio::time::timeout(Duration::from_secs(2), handler_task).await;
            }
            
            // Browser will be closed when dropped
            drop(browser);
            info!("Browser instance shut down");
        }
        
        Ok(())
    }
    
    /// Convert IMDB rating (1-10 with 0.5 increments) to Trakt (1-10 integer)
    pub fn normalize_to_trakt(&self, imdb_rating: f64) -> u8 {
        imdb_rating.round() as u8
    }
    
    /// Convert Trakt rating (1-10 integer) to IMDB format
    pub fn normalize_from_trakt(&self, trakt_rating: u8) -> f64 {
        trakt_rating as f64
    }
    
    /// Cache CSV file to cache directory for debugging
    /// This preserves the raw CSV data even if parsing fails or returns empty results
    fn cache_csv_file(&self, source_path: &Path, cache_filename: &str) {
        use media_sync_config::PathManager;
        let path_manager = PathManager::default();
        let csv_dir = path_manager.cache_csv_dir("imdb");
        
        // Ensure CSV cache directory exists
        if let Err(e) = std::fs::create_dir_all(&csv_dir) {
            warn!("Failed to create CSV cache directory {:?}: {}", csv_dir, e);
            return;
        }
        
        // Check if source file exists before trying to copy
        if !source_path.exists() {
            warn!("CSV file does not exist at {:?}, cannot cache", source_path);
            return;
        }
        
        let cache_path = csv_dir.join(cache_filename);
        
        // Copy CSV to cache directory
        match std::fs::copy(source_path, &cache_path) {
            Ok(bytes) => {
                info!("Cached IMDB CSV ({} bytes) to {:?} for debugging", bytes, cache_path);
            }
            Err(e) => {
                warn!("Failed to cache IMDB CSV to {:?}: {}", cache_path, e);
            }
        }
    }
    
    /// Count total rows in a CSV file (excluding header)
    fn count_csv_rows<P: AsRef<Path>>(&self, path: P) -> Result<usize> {
        use std::fs::File;
        use csv::Reader;
        
        let file = File::open(path)?;
        let mut reader = Reader::from_reader(file);
        let mut count = 0;
        
        // Skip header
        reader.headers()?;
        
        // Count data rows
        for result in reader.records() {
            result?;
            count += 1;
        }
        
        Ok(count)
    }
    
    /// Check if the browser is still alive and responsive
    /// Returns an error if the browser appears to have crashed
    async fn check_browser_health(browser: &Browser) -> Result<()> {
        // Try to get the browser version as a health check
        // If the browser has crashed, this will fail
        match browser.version().await {
            Ok(_) => {
                debug!("Browser health check passed");
                Ok(())
            }
            Err(e) => {
                warn!("Browser health check failed: {}", e);
                Err(anyhow!("Browser appears to have crashed or is unresponsive: {}", e))
            }
        }
    }
    
    /// Helper to execute an operation with a page, ensuring it's always closed
    /// This prevents page leaks even when errors occur
    async fn with_page<F, R>(
        browser: &Browser,
        url: &str,
        block_resources: bool,
        operation: F,
    ) -> Result<R>
    where
        F: for<'a> FnOnce(&'a Page) -> BoxFuture<'a, Result<R>>,
    {
        let page = browser.new_page(url).await?;
        
        // Configure resource blocking if requested
        if block_resources {
            if let Err(e) = Self::configure_resource_blocking(&page).await {
                warn!("Failed to configure resource blocking: {}", e);
            }
        }
        
        // Execute the operation - page lives for the entire duration
        let result = operation(&page).await;
        
        // Always close the page, even on error
        if let Err(e) = page.close().await {
            warn!("Failed to close page: {}", e);
        }
        
        result
    }
    
    /// Helper for MediaSource trait methods that return SourceError
    /// Ensures page is always closed even when errors occur
    async fn with_page_source_error<F, R>(
        browser: &Browser,
        url: &str,
        block_resources: bool,
        operation: F,
    ) -> Result<R, crate::error::SourceError>
    where
        F: for<'a> FnOnce(&'a Page) -> BoxFuture<'a, Result<R, crate::error::SourceError>>,
    {
        let page = browser.new_page(url).await
            .map_err(|e| crate::error::SourceError::new(format!("Failed to create new page: {}", e)))?;
        
        // Configure resource blocking if requested
        if block_resources {
            if let Err(e) = Self::configure_resource_blocking(&page).await {
                warn!("Failed to configure resource blocking: {}", e);
            }
        }
        
        // Execute the operation - page lives for the entire duration
        let result = operation(&page).await;
        
        // Always close the page, even on error
        if let Err(e) = page.close().await {
            warn!("Failed to close page: {}", e);
        }
        
        result
    }
}

impl Drop for ImdbClient {
    fn drop(&mut self) {
        // Close browser gracefully
        if let Some(_browser) = self.browser.take() {
            info!("Closing browser");
            // Browser will be closed when dropped
        }
        
        // Cleanup temporary download directory on drop
        if let Err(e) = std::fs::remove_dir_all(&self.download_dir) {
            warn!("Failed to cleanup download directory {:?} on drop: {}", self.download_dir, e);
        } else {
            debug!("Cleaned up download directory on drop: {:?}", self.download_dir);
        }
        
        // Handler task will end when browser closes
        // We could wait for it, but it's not critical
    }
}

#[async_trait::async_trait]
impl MediaSource for ImdbClient {
    type Error = crate::error::SourceError;

    fn source_name(&self) -> &str {
        "imdb"
    }

    async fn authenticate(&mut self) -> Result<(), Self::Error> {
        match self.authenticate().await {
            Ok(()) => Ok(()),
            Err(e) => Err(crate::error::SourceError::new(format!("{}", e))),
        }
    }

    fn is_authenticated(&self) -> bool {
        self.is_authenticated()
    }

    async fn get_watchlist(&self) -> Result<Vec<WatchlistItem>, Self::Error> {
        let browser = self.browser.as_ref().ok_or_else(|| crate::error::SourceError::new("Browser not initialized".to_string()))?;
        
        // Check browser health before starting
        Self::check_browser_health(browser).await
            .map_err(|e| crate::error::SourceError::new(format!("Browser health check failed before watchlist download: {}", e)))?;
        
        // Check if watchlist is empty first by navigating to the watchlist page
        let is_empty = Self::with_page_source_error(browser, "https://www.imdb.com/list/watchlist", false, |check_page| async move {
            // Wait longer for page to fully load
            sleep(Duration::from_secs(3)).await;
            
            // Wait for page to be interactive - check if body has content
            let mut load_attempts = 0;
            while load_attempts < 10 {
                let body_ready = match check_page.evaluate("document.body && document.body.innerText.length > 0").await {
                    Ok(result) => {
                        result.value()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    }
                    Err(_) => false,
                };
                
                if body_ready {
                    break;
                }
                sleep(Duration::from_millis(500)).await;
                load_attempts += 1;
            }
            
            // Check for empty state
            let page_text = match check_page.evaluate("document.body.innerText").await {
                Ok(result) => {
                    result.value()
                        .and_then(|v| v.as_str().map(|s| s.to_lowercase()))
                        .unwrap_or_default()
                }
                Err(_) => String::new(),
            };
            
            // Check for the exact empty state message first (most reliable)
            // Based on actual IMDB HTML: <div class="sc-b9995ff0-4 fTcYPM">This list is empty.</div>
            let mut is_empty = page_text.contains("this list is empty");
            
            // Also check for the specific empty state element by class
            // The class may be dynamically generated, so we verify text content
            if !is_empty {
                let empty_selectors = [
                    ".sc-b9995ff0-4",  // The specific class for empty state
                    "[data-testid='empty-watchlist']",
                    ".empty-state",
                    ".ipc-empty-state",
                ];
                
                for selector in &empty_selectors {
                    match check_page.find_element(*selector).await {
                        Ok(element) => {
                            // Verify it contains the empty text
                            if let Ok(Some(text)) = element.inner_text().await {
                                if text.to_lowercase().contains("this list is empty") 
                                    || text.to_lowercase().contains("list is empty") {
                                    is_empty = true;
                                    break;
                                }
                            }
                        }
                        Err(_) => continue,
                    }
                }
            }
            
            // Check for other common empty state text patterns
            if !is_empty {
                let empty_indicators = [
                    "your watchlist is empty",
                    "no titles in your watchlist",
                    "add titles to your watchlist",
                    "start building your watchlist",
                    "nothing in your watchlist",
                ];
                
                is_empty = empty_indicators.iter().any(|indicator| page_text.contains(*indicator));
            }
            
            Ok(is_empty)
        }.boxed()).await?;
        
        if is_empty {
            info!("IMDB watchlist is empty, returning empty list without downloading CSV");
            return Ok(vec![]);
        }
        
        // Generate watchlist export (only if not empty)
        export::generate_exports(browser, true, false, false, false, false).await
            .map_err(|e| crate::error::SourceError::new(format!("Failed to generate IMDB watchlist export: {}", e)))?;
        
        // Check browser health before download
        Self::check_browser_health(browser).await
            .map_err(|e| crate::error::SourceError::new(format!("Browser health check failed before watchlist download: {}", e)))?;
        
        // Create a new page for downloading (exports page) with resource blocking
        let download_dir = self.download_dir.clone();
        let cache = self.downloaded_files.lock().unwrap().clone();
        
        let files = Self::with_page_source_error(browser, "https://www.imdb.com/exports/", true, |page| async move {
            // Configure downloads on THIS page before attempting download
            Self::configure_downloads(page, &download_dir).await
                .map_err(|e| crate::error::SourceError::new(format!("Failed to configure downloads: {}", e)))?;
            
            // Wait a moment for the page to fully load and configuration to take effect
            sleep(Duration::from_secs(2)).await;
            
            download::download_exports(page, &download_dir, true, false, false, false, false, &cache).await
                .map_err(|e| {
                    let error_msg = format!("{}", e);
                    
                    // If export is not available (likely empty), return special error
                    if error_msg.contains("Export not available") || error_msg.contains("not available") {
                        return crate::error::SourceError::new("EXPORT_NOT_AVAILABLE".to_string());
                    }
                    
                    crate::error::SourceError::new(format!("Failed to download IMDB watchlist export: {}", e))
                })
        }.boxed()).await?;
        
        // Update cache with downloaded files
        if let Some(ref path) = files.watchlist {
            self.downloaded_files.lock().unwrap().insert("watchlist".to_string(), path.clone());
        }
        
        // Handle export not available case
        if files.watchlist.is_none() {
            info!("Watchlist export not available (likely empty), returning empty list");
            return Ok(vec![]);
        }

        if let Some(path) = files.watchlist {
            // Verify file exists before caching
            if !path.exists() {
                warn!("Watchlist CSV file does not exist at {:?}", path);
                return Ok(vec![]);
            }
            
            // Cache CSV file BEFORE parsing (so we have it even if parsing fails)
            self.cache_csv_file(&path, "imdb_watchlist.csv");
            
            info!("Parsing watchlist CSV from: {:?}", path);
            
            // Count total rows in CSV for better logging
            let total_rows = match self.count_csv_rows(&path) {
                Ok(count) => {
                    info!("CSV file contains {} total rows", count);
                    Some(count)
                }
                Err(e) => {
                    warn!("Failed to count CSV rows: {}", e);
                    None
                }
            };
            
            let watchlist = match parser::parse_watchlist_csv(&path) {
                Ok(watchlist) => {
                    if let Some(row_count) = total_rows {
                        info!("Parsed {} watchlist items from {} CSV rows", watchlist.len(), row_count);
                        if watchlist.is_empty() && row_count > 0 {
                            warn!("Watchlist CSV has {} rows but parsed 0 items. Rows may have been filtered (empty IMDB IDs, unknown types, etc.). Raw CSV cached to imdb_watchlist.csv for inspection.", row_count);
                        } else if watchlist.is_empty() {
                            warn!("Watchlist CSV is empty (0 rows). Raw CSV cached to imdb_watchlist.csv for inspection.");
                        }
                    } else {
                        info!("Parsed {} watchlist items from CSV", watchlist.len());
                        if watchlist.is_empty() {
                            warn!("Watchlist CSV parsed but contains 0 items. Check CSV format and content. Raw CSV cached to imdb_watchlist.csv for inspection.");
                        }
                    }
                    watchlist
                }
                Err(e) => {
                    let error_msg = format!("Failed to parse IMDB watchlist CSV: {}. Raw CSV cached to imdb_watchlist.csv for inspection.", e);
                    warn!("{}", error_msg);
                    return Err(crate::error::SourceError::new(error_msg));
                }
            };
            
            // Clean up temporary CSV file after parsing (cached copy remains)
            if let Err(e) = std::fs::remove_file(&path) {
                warn!("Failed to remove watchlist CSV file {:?} after parsing: {}", path, e);
            } else {
                debug!("Removed temporary watchlist CSV file after parsing: {:?}", path);
            }
            
            Ok(watchlist)
        } else {
            warn!("No watchlist CSV file found after download");
            Ok(vec![])
        }
    }

    async fn get_ratings(&self) -> Result<Vec<Rating>, Self::Error> {
        let browser = self.browser.as_ref().ok_or_else(|| crate::error::SourceError::new("Browser not initialized".to_string()))?;
        
        // Check browser health before starting
        Self::check_browser_health(browser).await
            .map_err(|e| crate::error::SourceError::new(format!("Browser health check failed before ratings download: {}", e)))?;
        
        // Generate ratings export
        export::generate_exports(browser, false, true, false, false, false).await
            .map_err(|e| crate::error::SourceError::new(format!("Failed to generate IMDB ratings export: {}", e)))?;
        
        // Check browser health after export generation
        Self::check_browser_health(browser).await
            .map_err(|e| crate::error::SourceError::new(format!("Browser health check failed after export generation: {}", e)))?;
        
        // Check browser health before download
        Self::check_browser_health(browser).await
            .map_err(|e| crate::error::SourceError::new(format!("Browser health check failed before download: {}", e)))?;
        
        // Create a new page for downloading (exports page) with resource blocking
        let download_dir = self.download_dir.clone();
        let cache = self.downloaded_files.lock().unwrap().clone();
        
        let files = Self::with_page_source_error(browser, "https://www.imdb.com/exports/", true, |page| async move {
            // Configure downloads on THIS page before attempting download
            Self::configure_downloads(page, &download_dir).await
                .map_err(|e| crate::error::SourceError::new(format!("Failed to configure downloads: {}", e)))?;
            
            // Wait a moment for the page to fully load and configuration to take effect
            sleep(Duration::from_secs(2)).await;
            
            download::download_exports(page, &download_dir, false, true, false, false, false, &cache).await
                .map_err(|e| {
                    crate::error::SourceError::new(format!("Failed to download IMDB ratings export: {}", e))
                })
        }.boxed()).await?;
        
        // Update cache with downloaded files
        if let Some(ref path) = files.ratings {
            self.downloaded_files.lock().unwrap().insert("ratings".to_string(), path.clone());
        }

        if let Some(path) = files.ratings {
            // Verify file exists before caching
            if !path.exists() {
                warn!("Ratings CSV file does not exist at {:?}", path);
                return Ok(vec![]);
            }
            
            // Cache CSV file BEFORE parsing (so we have it even if parsing fails)
            self.cache_csv_file(&path, "imdb_ratings.csv");
            
            info!("Parsing ratings CSV from: {:?}", path);
            
            // Count total rows in CSV for better logging
            let total_rows = match self.count_csv_rows(&path) {
                Ok(count) => {
                    info!("CSV file contains {} total rows", count);
                    Some(count)
                }
                Err(e) => {
                    warn!("Failed to count CSV rows: {}", e);
                    None
                }
            };
            
            let ratings = match parser::parse_ratings_csv(&path) {
                Ok(ratings) => {
                    if let Some(row_count) = total_rows {
                        info!("Parsed {} ratings from {} CSV rows", ratings.len(), row_count);
                        if ratings.is_empty() && row_count > 0 {
                            warn!("Ratings CSV has {} rows but parsed 0 items. Rows may have been filtered (empty IMDB IDs, unknown types, etc.). Raw CSV cached to imdb_ratings.csv for inspection.", row_count);
                        } else if ratings.is_empty() {
                            warn!("Ratings CSV is empty (0 rows). Raw CSV cached to imdb_ratings.csv for inspection.");
                        }
                    } else {
                        info!("Parsed {} ratings from CSV", ratings.len());
                        if ratings.is_empty() {
                            warn!("Ratings CSV parsed but contains 0 items. Check CSV format and content. Raw CSV cached to imdb_ratings.csv for inspection.");
                        }
                    }
                    ratings
                }
                Err(e) => {
                    let error_msg = format!("Failed to parse IMDB ratings CSV: {}. Raw CSV cached to imdb_ratings.csv for inspection.", e);
                    warn!("{}", error_msg);
                    return Err(crate::error::SourceError::new(error_msg));
                }
            };
            
            // Clean up temporary CSV file after parsing (cached copy remains)
            if let Err(e) = std::fs::remove_file(&path) {
                warn!("Failed to remove ratings CSV file {:?} after parsing: {}", path, e);
            } else {
                debug!("Removed temporary ratings CSV file after parsing: {:?}", path);
            }
            
            Ok(ratings)
        } else {
            warn!("No ratings CSV file found after download");
            Ok(vec![])
        }
    }

    async fn get_reviews(&self) -> Result<Vec<Review>, Self::Error> {
        let browser = self.browser.as_ref().ok_or_else(|| crate::error::SourceError::new("Browser not initialized".to_string()))?;
        
        Self::with_page_source_error(browser, "about:blank", false, |page| async move {
            reviews::scrape_reviews(page).await
                .map_err(|e| crate::error::SourceError::new(format!("Failed to scrape IMDB reviews: {}", e)))
        }.boxed()).await
    }

    async fn get_watch_history(&self) -> Result<Vec<WatchHistory>, Self::Error> {
        let browser = self.browser.as_ref().ok_or_else(|| crate::error::SourceError::new("Browser not initialized".to_string()))?;
        
        // Check browser health before starting
        Self::check_browser_health(browser).await
            .map_err(|e| crate::error::SourceError::new(format!("Browser health check failed before check-ins download: {}", e)))?;
        
        // Generate check-ins export
        export::generate_exports(browser, false, false, true, false, false).await
            .map_err(|e| crate::error::SourceError::new(format!("Failed to generate IMDB check-ins export: {}", e)))?;
        
        // Check browser health after export generation
        Self::check_browser_health(browser).await
            .map_err(|e| crate::error::SourceError::new(format!("Browser health check failed after check-ins export generation: {}", e)))?;
        
        // Check browser health before download
        Self::check_browser_health(browser).await
            .map_err(|e| crate::error::SourceError::new(format!("Browser health check failed before check-ins download: {}", e)))?;
        
        // Create a new page for downloading (exports page) with resource blocking
        let download_dir = self.download_dir.clone();
        let cache = self.downloaded_files.lock().unwrap().clone();
        
        let files = Self::with_page_source_error(browser, "https://www.imdb.com/exports/", true, |page| async move {
            // Configure downloads on THIS page before attempting download
            Self::configure_downloads(page, &download_dir).await
                .map_err(|e| crate::error::SourceError::new(format!("Failed to configure downloads: {}", e)))?;
            
            // Wait a moment for the page to fully load and configuration to take effect
            sleep(Duration::from_secs(2)).await;
            
            download::download_exports(page, &download_dir, false, false, true, false, false, &cache).await
                .map_err(|e| {
                    crate::error::SourceError::new(format!("Failed to download IMDB check-ins export: {}", e))
                })
        }.boxed()).await?;
        
        // Update cache with downloaded files
        if let Some(ref path) = files.checkins {
            self.downloaded_files.lock().unwrap().insert("check-ins".to_string(), path.clone());
        }

        if let Some(path) = files.checkins {
            // Verify file exists before caching
            if !path.exists() {
                warn!("Check-ins CSV file does not exist at {:?}", path);
                return Ok(vec![]);
            }
            
            // Cache CSV file BEFORE parsing (so we have it even if parsing fails)
            self.cache_csv_file(&path, "imdb_checkins.csv");
            
            info!("Parsing check-ins CSV from: {:?}", path);
            
            // Count total rows in CSV for better logging
            let total_rows = match self.count_csv_rows(&path) {
                Ok(count) => {
                    info!("CSV file contains {} total rows", count);
                    Some(count)
                }
                Err(e) => {
                    warn!("Failed to count CSV rows: {}", e);
                    None
                }
            };
            
            let history = match parser::parse_checkins_csv(&path) {
                Ok(history) => {
                    if let Some(row_count) = total_rows {
                        info!("Parsed {} check-ins from {} CSV rows", history.len(), row_count);
                        if history.is_empty() && row_count > 0 {
                            warn!("Check-ins CSV has {} rows but parsed 0 items. Rows may have been filtered (empty IMDB IDs, missing dates, etc.). Raw CSV cached to imdb_checkins.csv for inspection.", row_count);
                        } else if history.is_empty() {
                            warn!("Check-ins CSV is empty (0 rows). Raw CSV cached to imdb_checkins.csv for inspection.");
                        }
                    } else {
                        info!("Parsed {} check-ins from CSV", history.len());
                        if history.is_empty() {
                            warn!("Check-ins CSV parsed but contains 0 items. Check CSV format and content. Raw CSV cached to imdb_checkins.csv for inspection.");
                        }
                    }
                    history
                }
                Err(e) => {
                    let error_msg = format!("Failed to parse IMDB check-ins CSV: {}. Raw CSV cached to imdb_checkins.csv for inspection.", e);
                    warn!("{}", error_msg);
                    return Err(crate::error::SourceError::new(error_msg));
                }
            };
            
            // Clean up temporary CSV file after parsing (cached copy remains)
            if let Err(e) = std::fs::remove_file(&path) {
                warn!("Failed to remove check-ins CSV file {:?} after parsing: {}", path, e);
            } else {
                debug!("Removed temporary check-ins CSV file after parsing: {:?}", path);
            }
            
            Ok(history)
        } else {
            warn!("No check-ins CSV file found after download");
            Ok(vec![])
        }
    }

    async fn add_to_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error> {
        let browser = self.browser.as_ref().ok_or_else(|| crate::error::SourceError::new("Browser not initialized".to_string()))?;
        let items = items.to_vec();
        
        Self::with_page_source_error(browser, "about:blank", false, |page| async move {
            actions::add_to_watchlist(page, &items).await
                .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
        }.boxed()).await
    }

    async fn remove_from_watchlist(&self, items: &[WatchlistItem]) -> Result<(), Self::Error> {
        let browser = self.browser.as_ref().ok_or_else(|| crate::error::SourceError::new("Browser not initialized".to_string()))?;
        let items = items.to_vec();
        
        Self::with_page_source_error(browser, "about:blank", false, |page| async move {
            actions::remove_from_watchlist(page, &items).await
                .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
        }.boxed()).await
    }

    async fn set_ratings(&self, ratings: &[Rating]) -> Result<(), Self::Error> {
        let browser = self.browser.as_ref().ok_or_else(|| crate::error::SourceError::new("Browser not initialized".to_string()))?;
        let ratings = ratings.to_vec();
        let debug_config = self.debug_config.clone();
        let debug_enabled = debug_config.is_enabled();
        
        Self::with_page_source_error(browser, "about:blank", false, |page| async move {
            // Initialize PageInspector if debug is enabled
            let mut inspector_opt = if debug_enabled {
                match PageInspector::new(page.clone(), debug_config.clone()) {
                    Ok(inspector) => {
                        info!("Browser debugging enabled, output directory: {:?}", debug_config.output_dir());
                        Some(inspector)
                    }
                    Err(e) => {
                        warn!("Failed to initialize PageInspector: {}", e);
                        None
                    }
                }
            } else {
                None
            };
            
            actions::set_ratings(page, &ratings, inspector_opt.as_mut()).await
                .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
            
            Ok(())
        }.boxed()).await
    }

    async fn set_reviews(&self, reviews: &[Review]) -> Result<(), Self::Error> {
        let browser = self.browser.as_ref().ok_or_else(|| crate::error::SourceError::new("Browser not initialized".to_string()))?;
        
        // Get last submitted date from credentials before page operation
        let path_manager = media_sync_config::PathManager::default();
        let credentials_file = path_manager.credentials_file();
        let mut cred_store = media_sync_config::CredentialStore::new(credentials_file);
        let last_submitted = cred_store.load().ok()
            .and_then(|_| cred_store.get_imdb_reviews_last_submitted());
        
        let reviews = reviews.to_vec();
        let reviews_empty = reviews.is_empty();
        
        Self::with_page_source_error(browser, "about:blank", false, |page| async move {
            actions::set_reviews(page, &reviews, last_submitted).await
                .map_err(|e| crate::error::SourceError::new(format!("{}", e)))?;
            
            // Update last submitted date if reviews were successfully submitted
            if !reviews_empty {
                cred_store.set_imdb_reviews_last_submitted(chrono::Utc::now());
                let _ = cred_store.save();
            }
            
            Ok(())
        }.boxed()).await
    }

    async fn add_watch_history(&self, items: &[WatchHistory]) -> Result<(), Self::Error> {
        let browser = self.browser.as_ref().ok_or_else(|| crate::error::SourceError::new("Browser not initialized".to_string()))?;
        let items = items.to_vec();
        
        Self::with_page_source_error(browser, "about:blank", false, |page| async move {
            actions::add_watch_history(page, &items).await
                .map_err(|e| crate::error::SourceError::new(format!("{}", e)))
        }.boxed()).await
    }
    
    async fn cleanup(&mut self) -> Result<(), Self::Error> {
        self.shutdown().await
            .map_err(|e| crate::error::SourceError::new(format!("Failed to shutdown browser: {}", e)))
    }

}

impl RatingNormalization for ImdbClient {
    fn normalize_rating(&self, rating: f64, target_scale: u8) -> u8 {
        // IMDB uses 1-10 with 0.5 increments, round to integer for target scale
        rating.round() as u8
    }
    
    fn denormalize_rating(&self, rating: u8, source_scale: u8) -> f64 {
        // IMDB uses 1-10 with 0.5 increments, but we store as integer
        rating as f64
    }
    
    fn native_rating_scale(&self) -> u8 {
        10
    }
}

impl CapabilityRegistry for ImdbClient {
    fn as_id_extraction(&self) -> Option<&dyn IdExtraction> {
        None // IMDB doesn't extract additional IDs
    }
    
    fn as_id_lookup_provider(&self) -> Option<&dyn IdLookupProvider> {
        None // IMDB doesn't provide lookup
    }
    
    fn as_incremental_sync(&mut self) -> Option<&mut dyn IncrementalSync> {
        None
    }
    
    fn as_rating_normalization(&self) -> Option<&dyn RatingNormalization> {
        Some(self)
    }
    
    fn as_status_mapping(&self) -> Option<&dyn StatusMapping> {
        None
    }
}
