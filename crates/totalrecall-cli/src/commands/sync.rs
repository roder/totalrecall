use super::sync_ui::SyncUI;
use super::config::load_config_or_prompt_source_preference;
use crate::output::Output;
use color_eyre::eyre::Context;
use color_eyre::Result;
use media_sync_config::{Config, PathManager};
use media_sync_core::SyncOrchestrator;
use media_sync_sources::{SourceFactoryRegistry, MediaSource};
use serde_json::json;

pub async fn run_sync(
    watchlist: bool,
    ratings: bool,
    reviews: bool,
    watch_history: bool,
    dry_run: Option<String>,
    all: bool,
    use_cache: Option<String>,
    force_full_sync: bool,
    output: &Output,
) -> Result<()> {
    tracing::debug!("Sync command started");

    // Load config (prompt for source_preference if missing)
    let config = load_config_or_prompt_source_preference(output)?;

    // Determine sync options from flags or config
    // If --all is specified, use config defaults
    // If any individual flags are specified, use only those flags
    // Otherwise use config defaults
    let any_flags_set = watchlist || ratings || reviews || watch_history;
    let sync_watchlist = if all || !any_flags_set { config.sync.sync_watchlist } else { watchlist };
    let sync_ratings = if all || !any_flags_set { config.sync.sync_ratings } else { ratings };
    let sync_reviews = if all || !any_flags_set { config.sync.sync_reviews } else { reviews };
    let sync_watch_history = if all || !any_flags_set { config.sync.sync_watch_history } else { watch_history };

    // Load credentials
    let path_manager = PathManager::default();
    let credentials_file = path_manager.credentials_file();
    let mut cred_store = media_sync_config::CredentialStore::new(credentials_file.clone());
    cred_store.load()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to load credentials from {}: {}", credentials_file.display(), e))?;

    // Create factory registry and validate configurations
    let factory_registry = SourceFactoryRegistry::new();
    factory_registry.validate_all_configs(&config)
        .map_err(|e| color_eyre::eyre::eyre!("Configuration validation failed: {}", e))?;

    // Create all enabled sources using factories
    let sources = factory_registry.create_all_sources(&config, &cred_store).await
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create sources: {}", e))?;

    // Parse use_cache sources
    let use_cache_sources = if let Some(cache_list) = use_cache {
        if cache_list == "all" {
            // Default to all configured sources
            sources
                .iter()
                .map(|s| s.source_name().to_lowercase())
                .collect()
        } else {
            // Parse comma-separated list
            let sources_set: std::collections::HashSet<String> = cache_list
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .collect();
            
            // Validate that all specified sources are configured
            let configured_sources: std::collections::HashSet<String> = sources
                .iter()
                .map(|s| s.source_name().to_lowercase())
                .collect();
            
            for source in &sources_set {
                if !configured_sources.contains(source) {
                    return Err(color_eyre::eyre::eyre!(
                        "Source '{}' specified in --use-cache is not configured/enabled",
                        source
                    ));
                }
            }
            
            sources_set
        }
    } else {
        // No use-cache specified
        std::collections::HashSet::new()
    };

    // Parse dry_run sources
    let dry_run_sources = if let Some(dry_run_list) = dry_run {
        if dry_run_list == "all" {
            // Default to all configured sources
            sources
                .iter()
                .map(|s| s.source_name().to_lowercase())
                .collect()
        } else {
            // Parse comma-separated list
            let sources_set: std::collections::HashSet<String> = dry_run_list
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .collect();
            
            // Validate that all specified sources are configured
            let configured_sources: std::collections::HashSet<String> = sources
                .iter()
                .map(|s| s.source_name().to_lowercase())
                .collect();
            
            for source in &sources_set {
                if !configured_sources.contains(source) {
                    return Err(color_eyre::eyre::eyre!(
                        "Source '{}' specified in --dry-run is not configured/enabled",
                        source
                    ));
                }
            }
            
            sources_set
        }
    } else {
        // No dry-run
        std::collections::HashSet::new()
    };

    let sync_options = media_sync_core::SyncOptions {
        sync_watchlist,
        sync_ratings,
        sync_reviews,
        sync_watch_history,
        force_full_sync,
    };
    
    let dry_run_sources_clone = dry_run_sources.clone();
    let mut orchestrator = SyncOrchestrator::new(
        sources,
        config.resolution,
    )
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create sync orchestrator: {}", e))?
        .with_sync_options(sync_options)
        .with_config_sync_options(config.sync)
        .with_use_cache(use_cache_sources)
        .with_dry_run(dry_run_sources);
    let _ui = SyncUI::new();

    let result = orchestrator.sync().await
        .map_err(|e| color_eyre::eyre::eyre!("Sync operation failed: {}", e))?;

    // Output results based on format
    match output.format() {
        crate::output::OutputFormat::Human => {
            if !dry_run_sources_clone.is_empty() {
                let path_manager = PathManager::default();
                let distribute_dir = path_manager.cache_distribute_dir();
                output.info(&format!(
                    "Dry-run mode: JSON files written to {}",
                    distribute_dir.display()
                ));
                output.info(&format!(
                    "Dry-run sources: {}",
                    dry_run_sources_clone.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
            output.success(&format!("Sync completed: {} items synced in {:?}", result.items_synced, result.duration));
        }
        crate::output::OutputFormat::Json | crate::output::OutputFormat::JsonPretty => {
            let json_result = json!({
                "success": true,
                "items_synced": result.items_synced,
                "duration_seconds": result.duration.as_secs_f64(),
                "duration": format!("{:?}", result.duration),
            });
            output.json(&json_result);
        }
    }

    Ok(())
}
