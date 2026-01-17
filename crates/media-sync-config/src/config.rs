use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub trakt: Option<TraktConfig>,
    #[serde(default)]
    pub simkl: Option<SimklConfig>,
    #[serde(default)]
    pub resolution: ResolutionConfig,
    pub sources: SourceConfig,
    pub sync: SyncOptions,
    #[serde(default)]
    pub scheduler: Option<SchedulerConfig>,
    #[serde(default)]
    #[cfg(feature = "browser-debug")]
    pub browser_debug: Option<DebugConfig>,
}

/// Status mapping configuration for converting between service-native and normalized statuses
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StatusMapping {
    /// Map service-native status → normalized status (for collection)
    #[serde(default)]
    pub to_normalized: HashMap<String, media_sync_models::NormalizedStatus>,
    
    /// Map normalized status → service-native status (for distribution)
    #[serde(default)]
    pub from_normalized: HashMap<media_sync_models::NormalizedStatus, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TraktConfig {
    pub enabled: bool,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "default_trakt_status_mapping")]
    pub status_mapping: StatusMapping,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SimklConfig {
    pub enabled: bool,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "default_simkl_status_mapping")]
    pub status_mapping: StatusMapping,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SourceConfig {
    #[serde(default)]
    pub imdb: Option<ImdbConfig>,
    #[serde(default)]
    pub plex: Option<PlexConfig>,
    #[serde(default)]
    pub tmdb: Option<TmdbConfig>,
    #[serde(default)]
    pub netflix: Option<NetflixConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImdbConfig {
    pub enabled: bool,
    pub username: String,
    #[serde(default = "default_imdb_status_mapping")]
    pub status_mapping: StatusMapping,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PlexConfig {
    pub enabled: bool,
    pub server_url: String,
    #[serde(default = "default_plex_status_mapping")]
    pub status_mapping: StatusMapping,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TmdbConfig {
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NetflixConfig {
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResolutionConfig {
    // Global defaults (used for ratings and watchlist)
    #[serde(default = "default_resolution_strategy")]
    pub strategy: ResolutionStrategy,
    
    #[serde(default)]
    pub source_preference: Vec<String>,  // Required ordered list: ["trakt", "imdb"] for conflict resolution (optional during deserialization, must be set via prompt)
    
    #[serde(default = "default_timestamp_tolerance_seconds")]
    pub timestamp_tolerance_seconds: i64,
    
    // Per-data-type strategies (override global defaults)
    #[serde(default)]
    pub ratings_strategy: Option<ResolutionStrategy>,
    
    #[serde(default)]
    pub watchlist_strategy: Option<ResolutionStrategy>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ResolutionStrategy {
    Newest,
    Oldest,
    Preference,
    Merge,
}

fn default_resolution_strategy() -> ResolutionStrategy {
    ResolutionStrategy::Preference
}

fn default_timestamp_tolerance_seconds() -> i64 {
    3600  // 1 hour
}

impl Default for ResolutionConfig {
    fn default() -> Self {
        Self {
            strategy: default_resolution_strategy(),
            source_preference: Vec::new(),  // Empty by default - must be set explicitly
            timestamp_tolerance_seconds: default_timestamp_tolerance_seconds(),
            ratings_strategy: None,
            watchlist_strategy: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncOptions {
    #[serde(default = "default_true")]
    pub sync_watchlist: bool,
    #[serde(default = "default_true")]
    pub sync_ratings: bool,
    #[serde(default = "default_true")]
    pub sync_reviews: bool,
    #[serde(default = "default_true")]
    pub sync_watch_history: bool,
    #[serde(default)]
    pub remove_watched_from_watchlists: bool,
    #[serde(default)]
    pub mark_rated_as_watched: bool,
    #[serde(default)]
    pub remove_watchlist_items_older_than_days: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SchedulerConfig {
    #[serde(default = "default_schedule")]
    pub schedule: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default = "default_true")]
    pub run_on_startup: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_json_logging")]
    pub json: bool,
    pub file: Option<PathBuf>,
    #[serde(default = "default_max_log_size")]
    pub max_size_mb: u64,
    #[serde(default = "default_log_retention")]
    pub retention: u32,
}

fn default_true() -> bool {
    true
}

fn default_schedule() -> String {
    "0 */6 * * *".to_string()  // Every 6 hours
}

fn default_timezone() -> String {
    std::env::var("TZ").unwrap_or_else(|_| "UTC".to_string())
}

pub fn default_scheduler_config() -> SchedulerConfig {
    SchedulerConfig {
        schedule: default_schedule(),
        timezone: default_timezone(),
        run_on_startup: default_true(),
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_json_logging() -> bool {
    use std::io::IsTerminal;
    !std::io::stdout().is_terminal()
}

fn default_max_log_size() -> u64 {
    10
}

fn default_log_retention() -> u32 {
    5
}

pub fn default_simkl_status_mapping() -> StatusMapping {
    use media_sync_models::NormalizedStatus::*;
    
    let mut to_normalized = HashMap::new();
    to_normalized.insert("plantowatch".to_string(), Watchlist);
    to_normalized.insert("watching".to_string(), Watching);
    to_normalized.insert("completed".to_string(), Completed);
    to_normalized.insert("dropped".to_string(), Dropped);
    to_normalized.insert("hold".to_string(), Hold);
    
    let mut from_normalized = HashMap::new();
    from_normalized.insert(Watchlist, "plantowatch".to_string());
    from_normalized.insert(Watching, "watching".to_string());
    from_normalized.insert(Completed, "completed".to_string());
    from_normalized.insert(Dropped, "dropped".to_string());
    from_normalized.insert(Hold, "hold".to_string());
    
    StatusMapping { to_normalized, from_normalized }
}

pub fn default_imdb_status_mapping() -> StatusMapping {
    use media_sync_models::NormalizedStatus::*;
    
    // IMDB doesn't have native status, but we infer from data type:
    // - watchlist → Watchlist
    // - check-ins → Watching
    let mut to_normalized = HashMap::new();
    to_normalized.insert("watchlist".to_string(), Watchlist);
    to_normalized.insert("checkins".to_string(), Watching);
    
    let mut from_normalized = HashMap::new();
    from_normalized.insert(Watchlist, "watchlist".to_string());
    from_normalized.insert(Watching, "checkins".to_string());
    from_normalized.insert(Completed, "checkins".to_string()); // Completed items go to check-ins
    
    StatusMapping { to_normalized, from_normalized }
}

pub fn default_trakt_status_mapping() -> StatusMapping {
    use media_sync_models::NormalizedStatus::*;
    
    // Trakt doesn't have native status, but we infer from data type:
    // - watchlist → Watchlist
    // - watch history → Watching
    let mut to_normalized = HashMap::new();
    to_normalized.insert("watchlist".to_string(), Watchlist);
    to_normalized.insert("watch_history".to_string(), Watching);
    
    let mut from_normalized = HashMap::new();
    from_normalized.insert(Watchlist, "watchlist".to_string());
    from_normalized.insert(Watching, "watch_history".to_string());
    from_normalized.insert(Completed, "watch_history".to_string()); // Completed items go to watch history
    
    StatusMapping { to_normalized, from_normalized }
}

pub fn default_plex_status_mapping() -> StatusMapping {
    use media_sync_models::NormalizedStatus::*;
    
    // Plex status mapping based on watch history inference:
    // - watchlist (unwatched) → Watchlist
    // - watchlist (partially watched) → Watching
    // - watchlist (fully watched) → Completed
    // - watched (not in watchlist) → Completed
    let mut to_normalized = HashMap::new();
    to_normalized.insert("watchlist".to_string(), Watchlist);
    to_normalized.insert("watching".to_string(), Watching);
    to_normalized.insert("completed".to_string(), Completed);
    to_normalized.insert("watched".to_string(), Completed);
    
    let mut from_normalized = HashMap::new();
    from_normalized.insert(Watchlist, "watchlist".to_string());
    from_normalized.insert(Watching, "watch_history".to_string()); // Watching status goes to watch_history
    from_normalized.insert(Completed, "watch_history".to_string()); // Completed status goes to watch_history
    // Dropped and Hold are not supported by Plex
    
    StatusMapping { to_normalized, from_normalized }
}

impl Config {
    pub fn load_from_file(path: &PathBuf) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save_to_file(&self, path: &PathBuf) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        // Validate resolution config
        if self.resolution.timestamp_tolerance_seconds < 0 {
            return Err(anyhow::anyhow!("timestamp_tolerance_seconds must be non-negative"));
        }
        
        // Validate source_preference - must be non-empty
        if self.resolution.source_preference.is_empty() {
            return Err(anyhow::anyhow!("source_preference is required and cannot be empty"));
        }
        
        let valid_sources = ["trakt", "imdb", "plex", "simkl"];
        for source in &self.resolution.source_preference {
            if !valid_sources.contains(&source.as_str()) {
                return Err(anyhow::anyhow!("Invalid source in source_preference: {}", source));
            }
            
            match source.as_str() {
                "trakt" => {
                    let trakt = self.trakt.as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Trakt is in source_preference but is not configured"))?;
                    if !trakt.enabled {
                        return Err(anyhow::anyhow!("Trakt is in source_preference but is not enabled"));
                    }
                    if trakt.client_id.is_empty() || trakt.client_id == "YOUR_CLIENT_ID" {
                        return Err(anyhow::anyhow!("Trakt is in source_preference but client_id is not configured"));
                    }
                    if trakt.client_secret.is_empty() || trakt.client_secret == "YOUR_CLIENT_SECRET" {
                        return Err(anyhow::anyhow!("Trakt is in source_preference but client_secret is not configured"));
                    }
                }
                "simkl" => {
                    let simkl = self.simkl.as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Simkl is in source_preference but is not configured"))?;
                    if !simkl.enabled {
                        return Err(anyhow::anyhow!("Simkl is in source_preference but is not enabled"));
                    }
                    if simkl.client_id.is_empty() || simkl.client_id == "YOUR_CLIENT_ID" {
                        return Err(anyhow::anyhow!("Simkl is in source_preference but client_id is not configured"));
                    }
                    if simkl.client_secret.is_empty() || simkl.client_secret == "YOUR_CLIENT_SECRET" {
                        return Err(anyhow::anyhow!("Simkl is in source_preference but client_secret is not configured"));
                    }
                }
                "imdb" => {
                    let imdb = self.sources.imdb.as_ref()
                        .ok_or_else(|| anyhow::anyhow!("IMDB is in source_preference but is not configured"))?;
                    if !imdb.enabled {
                        return Err(anyhow::anyhow!("IMDB is in source_preference but is not enabled"));
                    }
                }
                "plex" => {
                    let plex = self.sources.plex.as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Plex is in source_preference but is not configured"))?;
                    if !plex.enabled {
                        return Err(anyhow::anyhow!("Plex is in source_preference but is not enabled"));
                    }
                }
                _ => {}
            }
        }
        
        Ok(())
    }

    pub fn is_trakt_configured(&self) -> bool {
        if let Some(ref trakt) = self.trakt {
            trakt.enabled
                && !trakt.client_id.is_empty()
                && trakt.client_id != "YOUR_CLIENT_ID"
                && !trakt.client_secret.is_empty()
                && trakt.client_secret != "YOUR_CLIENT_SECRET"
        } else {
            false
        }
    }

    /// Get list of configured and enabled services
    pub fn get_configured_services(&self) -> Vec<String> {
        let mut services = Vec::new();
        
        // Check Trakt
        if let Some(ref trakt) = self.trakt {
            if trakt.enabled 
                && !trakt.client_id.is_empty() 
                && trakt.client_id != "YOUR_CLIENT_ID"
                && !trakt.client_secret.is_empty()
                && trakt.client_secret != "YOUR_CLIENT_SECRET" {
                services.push("trakt".to_string());
            }
        }
        
        // Check Simkl
        if let Some(simkl) = &self.simkl {
            if simkl.enabled 
                && !simkl.client_id.is_empty() 
                && simkl.client_id != "YOUR_CLIENT_ID"
                && !simkl.client_secret.is_empty()
                && simkl.client_secret != "YOUR_CLIENT_SECRET" {
                services.push("simkl".to_string());
            }
        }
        
        // Check IMDB
        if let Some(imdb) = &self.sources.imdb {
            if imdb.enabled && !imdb.username.is_empty() {
                services.push("imdb".to_string());
            }
        }
        
        // Check Plex
        if let Some(plex) = &self.sources.plex {
            if plex.enabled && !plex.server_url.is_empty() {
                services.push("plex".to_string());
            }
        }
        
        services
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_config_load_and_save() {
        let file = NamedTempFile::new().unwrap();
        let config = Config {
            trakt: Some(TraktConfig {
                enabled: true,
                client_id: "test_id".to_string(),
                client_secret: "test_secret".to_string(),
                status_mapping: default_trakt_status_mapping(),
            }),
            simkl: None,
            resolution: ResolutionConfig {
                source_preference: vec!["trakt".to_string()],
                ..ResolutionConfig::default()
            },
            sources: SourceConfig {
                imdb: None,
                plex: None,
                tmdb: None,
                netflix: None,
            },
            sync: SyncOptions {
                sync_watchlist: true,
                sync_ratings: true,
                sync_reviews: false,
                sync_watch_history: true,
                remove_watched_from_watchlists: false,
                mark_rated_as_watched: false,
                remove_watchlist_items_older_than_days: None,
            },
            scheduler: None,
        };

        let path = file.path().to_path_buf();
        config.save_to_file(&path).unwrap();

        let loaded = Config::load_from_file(&path).unwrap();
        assert_eq!(loaded.trakt.as_ref().unwrap().client_id, "test_id");
        assert_eq!(loaded.trakt.as_ref().unwrap().client_secret, "test_secret");
        assert_eq!(loaded.sync.sync_watchlist, true);
        assert_eq!(loaded.sync.sync_reviews, false);
    }

    #[test]
    fn test_config_validate() {
        let mut config = Config {
            trakt: Some(TraktConfig {
                enabled: true,
                client_id: "YOUR_CLIENT_ID".to_string(),
                client_secret: "YOUR_CLIENT_SECRET".to_string(),
                status_mapping: default_trakt_status_mapping(),
            }),
            simkl: None,
            resolution: ResolutionConfig {
                source_preference: vec!["trakt".to_string()],
                ..ResolutionConfig::default()
            },
            sources: SourceConfig {
                imdb: None,
                plex: None,
                tmdb: None,
                netflix: None,
            },
            sync: SyncOptions {
                sync_watchlist: true,
                sync_ratings: true,
                sync_reviews: true,
                sync_watch_history: true,
                remove_watched_from_watchlists: false,
                mark_rated_as_watched: false,
                remove_watchlist_items_older_than_days: None,
            },
            scheduler: None,
        };

        assert!(config.validate().is_err());
        assert!(!config.is_trakt_configured());

        config.trakt = Some(TraktConfig {
            enabled: true,
            client_id: "real_id".to_string(),
            client_secret: "real_secret".to_string(),
            status_mapping: StatusMapping {
                to_normalized: std::collections::HashMap::new(),
                from_normalized: std::collections::HashMap::new(),
            },
        });
        assert!(config.validate().is_ok());
        assert!(config.is_trakt_configured());
    }

    #[test]
    fn test_sync_options_defaults() {
        let options = SyncOptions {
            sync_watchlist: true,
            sync_ratings: true,
            sync_reviews: true,
            sync_watch_history: true,
            remove_watched_from_watchlists: false,
            mark_rated_as_watched: false,
            remove_watchlist_items_older_than_days: None,
        };
        assert_eq!(options.sync_watchlist, true);
        assert_eq!(options.sync_ratings, true);
        assert_eq!(options.sync_reviews, true);
        assert_eq!(options.sync_watch_history, true);
        assert_eq!(options.remove_watched_from_watchlists, false);
        assert_eq!(options.mark_rated_as_watched, false);
        assert_eq!(options.remove_watchlist_items_older_than_days, None);
    }
}

