use super::prompts;
use crate::output::Output;
use color_eyre::Result;
use comfy_table::{Cell, Table};
use media_sync_config::{Config, CredentialStore, PathManager, SyncOptions, TraktConfig, SimklConfig, PlexConfig, default_plex_status_mapping, default_simkl_status_mapping};
use media_sync_sources::{trakt_authenticate, simkl_authenticate};
use owo_colors::OwoColorize;
use serde_json::json;
use std::io::{self, Write};

pub async fn run_config(cmd: crate::ConfigCommands, output: &Output) -> Result<()> {
    match cmd {
        crate::ConfigCommands::Show { full } => show_config(full, output).await,
        crate::ConfigCommands::Trakt { client_id, client_secret } => configure_trakt(client_id, client_secret, output).await,
        crate::ConfigCommands::Simkl { client_id, client_secret } => configure_simkl(client_id, client_secret, output).await,
        crate::ConfigCommands::Imdb { username } => configure_imdb(username, output).await,
        crate::ConfigCommands::Plex { token, server_url } => configure_plex(token, server_url, output).await,
        crate::ConfigCommands::Sync { enable_watchlist, enable_ratings, enable_reviews, enable_watch_history } => {
            configure_sync(enable_watchlist, enable_ratings, enable_reviews, enable_watch_history, output).await
        }
    }
}

async fn show_config(full: bool, output: &Output) -> Result<()> {
    let path_manager = PathManager::default();
    let config_file = path_manager.config_file();

    if !config_file.exists() {
        output.warn(&format!("Configuration file not found at: {}", config_file.display()));
        output.info("Configuration will be created automatically when you run 'totalrecall config trakt' or 'totalrecall config imdb'.");
        return Ok(());
    }

    let config = Config::load_from_file(&config_file)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to load config from {}: {}", config_file.display(), e))?;

    match output.format() {
        crate::output::OutputFormat::Human => {
            if output.is_quiet() {
                return Ok(());
            }

            // Header
            println!("\n{}", "╔════════════════════════════════════════════════════════════╗".bright_white());
            println!("{}", "║".bright_white());
            println!("{} {}", "║".bright_white(), "Configuration".bright_cyan().bold());
            println!("{}", "╚════════════════════════════════════════════════════════════╝".bright_white());
            println!();

            // Config file location
            let mut info_table = Table::new();
            info_table.set_header(vec![
                Cell::new("Config File").add_attribute(comfy_table::Attribute::Bold),
                Cell::new(config_file.display().to_string())
            ]);
            info_table.load_preset(comfy_table::presets::UTF8_FULL);
            info_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
            println!("{}", info_table);
            println!();

            // Trakt Configuration
            if let Some(trakt) = &config.trakt {
                let mut trakt_table = Table::new();
                trakt_table.set_header(vec![
                    Cell::new("Trakt Configuration").fg(comfy_table::Color::Cyan).add_attribute(comfy_table::Attribute::Bold)
                ]);
                trakt_table.add_row(vec![
                    Cell::new("Enabled"),
                    Cell::new(if trakt.enabled { "✓".green().to_string() } else { "✗".red().to_string() })
                ]);
                let client_id_display = if full { trakt.client_id.clone() } else { mask_string(&trakt.client_id) };
                let client_secret_display = if full { trakt.client_secret.clone() } else { mask_string(&trakt.client_secret) };
                trakt_table.add_row(vec![
                    Cell::new("Client ID"),
                    Cell::new(client_id_display)
                ]);
                trakt_table.add_row(vec![
                    Cell::new("Client Secret"),
                    Cell::new(client_secret_display)
                ]);
                trakt_table.load_preset(comfy_table::presets::UTF8_FULL);
                trakt_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
                println!("{}", trakt_table);
                println!();
            } else {
                println!("{}", "Trakt: Not configured".bright_black());
                println!();
            }

            // Simkl Configuration
            if let Some(simkl) = &config.simkl {
                let mut simkl_table = Table::new();
                simkl_table.set_header(vec![
                    Cell::new("Simkl Configuration").fg(comfy_table::Color::Cyan).add_attribute(comfy_table::Attribute::Bold)
                ]);
                let client_id_display = if full { simkl.client_id.clone() } else { mask_string(&simkl.client_id) };
                let client_secret_display = if full { simkl.client_secret.clone() } else { mask_string(&simkl.client_secret) };
                simkl_table.add_row(vec![
                    Cell::new("Enabled"),
                    Cell::new(if simkl.enabled { "✓".green().to_string() } else { "✗".red().to_string() })
                ]);
                simkl_table.add_row(vec![
                    Cell::new("Client ID"),
                    Cell::new(client_id_display)
                ]);
                simkl_table.add_row(vec![
                    Cell::new("Client Secret"),
                    Cell::new(client_secret_display)
                ]);
                simkl_table.load_preset(comfy_table::presets::UTF8_FULL);
                simkl_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
                println!("{}", simkl_table);
                println!();
            }

            // IMDB Configuration
            if let Some(imdb) = &config.sources.imdb {
                let mut imdb_table = Table::new();
                imdb_table.set_header(vec![
                    Cell::new("IMDB Configuration").fg(comfy_table::Color::Cyan).add_attribute(comfy_table::Attribute::Bold)
                ]);
                let username_display = if full { imdb.username.clone() } else { mask_string(&imdb.username) };
                imdb_table.add_row(vec![
                    Cell::new("Enabled"),
                    Cell::new(if imdb.enabled { "✓".green().to_string() } else { "✗".red().to_string() })
                ]);
                imdb_table.add_row(vec![
                    Cell::new("Username"),
                    Cell::new(username_display)
                ]);
                imdb_table.load_preset(comfy_table::presets::UTF8_FULL);
                imdb_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
                println!("{}", imdb_table);
                println!();
            }

            // Plex Configuration
            if let Some(plex) = &config.sources.plex {
                let mut plex_table = Table::new();
                plex_table.set_header(vec![
                    Cell::new("Plex Configuration").fg(comfy_table::Color::Cyan).add_attribute(comfy_table::Attribute::Bold)
                ]);
                plex_table.add_row(vec![
                    Cell::new("Enabled"),
                    Cell::new(if plex.enabled { "✓".green().to_string() } else { "✗".red().to_string() })
                ]);
                plex_table.add_row(vec![
                    Cell::new("Server URL"),
                    Cell::new(&plex.server_url)
                ]);
                plex_table.load_preset(comfy_table::presets::UTF8_FULL);
                plex_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
                println!("{}", plex_table);
                println!();
            }

            // Resolution Configuration
            let mut resolution_table = Table::new();
            resolution_table.set_header(vec![
                Cell::new("Resolution Configuration").fg(comfy_table::Color::Cyan).add_attribute(comfy_table::Attribute::Bold)
            ]);
            resolution_table.add_row(vec![
                Cell::new("Strategy"),
                Cell::new(format!("{:?}", config.resolution.strategy))
            ]);
            resolution_table.add_row(vec![
                Cell::new("Source Preference"),
                Cell::new(format!("{:?}", config.resolution.source_preference))
            ]);
            resolution_table.add_row(vec![
                Cell::new("Timestamp Tolerance"),
                Cell::new(format!("{} seconds", config.resolution.timestamp_tolerance_seconds))
            ]);
            if let Some(ratings_strategy) = &config.resolution.ratings_strategy {
                resolution_table.add_row(vec![
                    Cell::new("Ratings Strategy"),
                    Cell::new(format!("{:?}", ratings_strategy))
                ]);
            }
            if let Some(watchlist_strategy) = &config.resolution.watchlist_strategy {
                resolution_table.add_row(vec![
                    Cell::new("Watchlist Strategy"),
                    Cell::new(format!("{:?}", watchlist_strategy))
                ]);
            }
            resolution_table.load_preset(comfy_table::presets::UTF8_FULL);
            resolution_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
            println!("{}", resolution_table);
            println!();

            // Sync Options
            let mut sync_table = Table::new();
            sync_table.set_header(vec![
                Cell::new("Sync Options").fg(comfy_table::Color::Cyan).add_attribute(comfy_table::Attribute::Bold)
            ]);
            sync_table.add_row(vec![
                Cell::new("Sync Watchlist"),
                Cell::new(if config.sync.sync_watchlist { "✓".green().to_string() } else { "✗".red().to_string() })
            ]);
            sync_table.add_row(vec![
                Cell::new("Sync Ratings"),
                Cell::new(if config.sync.sync_ratings { "✓".green().to_string() } else { "✗".red().to_string() })
            ]);
            sync_table.add_row(vec![
                Cell::new("Sync Reviews"),
                Cell::new(if config.sync.sync_reviews { "✓".green().to_string() } else { "✗".red().to_string() })
            ]);
            sync_table.add_row(vec![
                Cell::new("Sync Watch History"),
                Cell::new(if config.sync.sync_watch_history { "✓".green().to_string() } else { "✗".red().to_string() })
            ]);
            sync_table.add_row(vec![
                Cell::new("Remove Watched from Watchlists"),
                Cell::new(if config.sync.remove_watched_from_watchlists { "✓".green().to_string() } else { "✗".red().to_string() })
            ]);
            sync_table.add_row(vec![
                Cell::new("Mark Rated as Watched"),
                Cell::new(if config.sync.mark_rated_as_watched { "✓".green().to_string() } else { "✗".red().to_string() })
            ]);
            if let Some(days) = config.sync.remove_watchlist_items_older_than_days {
                sync_table.add_row(vec![
                    Cell::new("Remove Watchlist Items Older Than"),
                    Cell::new(format!("{} days", days))
                ]);
            }
            sync_table.load_preset(comfy_table::presets::UTF8_FULL);
            sync_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
            println!("{}", sync_table);
            println!();

            // Scheduler Configuration
            if let Some(scheduler) = &config.scheduler {
                let mut scheduler_table = Table::new();
                scheduler_table.set_header(vec![
                    Cell::new("Scheduler Configuration").fg(comfy_table::Color::Cyan).add_attribute(comfy_table::Attribute::Bold)
                ]);
                scheduler_table.add_row(vec![
                    Cell::new("Schedule"),
                    Cell::new(&scheduler.schedule)
                ]);
                scheduler_table.add_row(vec![
                    Cell::new("Timezone"),
                    Cell::new(&scheduler.timezone)
                ]);
                scheduler_table.add_row(vec![
                    Cell::new("Run on Startup"),
                    Cell::new(if scheduler.run_on_startup { "✓".green().to_string() } else { "✗".red().to_string() })
                ]);
                scheduler_table.load_preset(comfy_table::presets::UTF8_FULL);
                scheduler_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
                println!("{}", scheduler_table);
                println!();
            }
        }
        crate::output::OutputFormat::Json | crate::output::OutputFormat::JsonPretty => {
            let json_config = json!({
                "config_file": config_file.display().to_string(),
                "trakt": if let Some(trakt) = &config.trakt {
                    json!({
                        "enabled": trakt.enabled,
                        "client_id": if full { trakt.client_id.clone() } else { mask_string(&trakt.client_id) },
                        "client_secret": if full { trakt.client_secret.clone() } else { mask_string(&trakt.client_secret) },
                    })
                } else {
                    json!(null)
                },
                "simkl": if let Some(simkl) = &config.simkl {
                    json!({
                        "enabled": simkl.enabled,
                        "client_id": if full { simkl.client_id.clone() } else { mask_string(&simkl.client_id) },
                        "client_secret": if full { simkl.client_secret.clone() } else { mask_string(&simkl.client_secret) },
                    })
                } else {
                    json!(null)
                },
                "imdb": if let Some(imdb) = &config.sources.imdb {
                    json!({
                        "enabled": imdb.enabled,
                        "username": if full { imdb.username.clone() } else { mask_string(&imdb.username) },
                    })
                } else {
                    json!(null)
                },
                "plex": if let Some(plex) = &config.sources.plex {
                    json!({
                        "enabled": plex.enabled,
                        "server_url": plex.server_url,
                    })
                } else {
                    json!(null)
                },
                "resolution": {
                    "strategy": format!("{:?}", config.resolution.strategy),
                    "source_preference": config.resolution.source_preference,
                    "timestamp_tolerance_seconds": config.resolution.timestamp_tolerance_seconds,
                    "ratings_strategy": config.resolution.ratings_strategy.as_ref().map(|s| format!("{:?}", s)),
                    "watchlist_strategy": config.resolution.watchlist_strategy.as_ref().map(|s| format!("{:?}", s)),
                },
                "sync": {
                    "sync_watchlist": config.sync.sync_watchlist,
                    "sync_ratings": config.sync.sync_ratings,
                    "sync_reviews": config.sync.sync_reviews,
                    "sync_watch_history": config.sync.sync_watch_history,
                    "remove_watched_from_watchlists": config.sync.remove_watched_from_watchlists,
                    "mark_rated_as_watched": config.sync.mark_rated_as_watched,
                    "remove_watchlist_items_older_than_days": config.sync.remove_watchlist_items_older_than_days,
                },
                "scheduler": if let Some(scheduler) = &config.scheduler {
                    json!({
                        "schedule": scheduler.schedule,
                        "timezone": scheduler.timezone,
                        "run_on_startup": scheduler.run_on_startup,
                    })
                } else {
                    json!(null)
                }
            });
            output.json(&json_config);
        }
    }

    Ok(())
}

async fn configure_trakt(client_id_arg: Option<String>, client_secret_arg: Option<String>, output: &Output) -> Result<()> {
    let path_manager = PathManager::default();
    path_manager.ensure_directories()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create configuration directories: {}", e))?;

    let config_file = path_manager.config_file();
    let mut config = if config_file.exists() {
        Config::load_from_file(&config_file)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to load config from {}: {}", config_file.display(), e))?
    } else {
        output.info("Configuration file not found. Creating default configuration...");
        let default_config = Config {
            trakt: Some(TraktConfig {
                enabled: false,
                client_id: String::new(),
                client_secret: String::new(),
                status_mapping: media_sync_config::default_trakt_status_mapping(),
            }),
            simkl: None,
            resolution: media_sync_config::ResolutionConfig {
                source_preference: Vec::new(), // Must be set via interactive prompt
                ..media_sync_config::ResolutionConfig::default()
            },
            sources: media_sync_config::SourceConfig {
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
            scheduler: Some(media_sync_config::default_scheduler_config()),
        };
        default_config
    };

    print_section_header("Trakt API Setup", output);
    output.println("");
    output.println("Follow the instructions to setup your Trakt API application:");
    print_instruction_list(&[
        "Login to Trakt and navigate to your API apps page: https://trakt.tv/oauth/applications",
        "Create a new API application named 'TotalRecall'",
        "Use 'urn:ietf:wg:oauth:2.0:oob' as the Redirect URI",
    ], output);
    output.println("");

    if config.trakt.is_none() {
        config.trakt = Some(TraktConfig {
            enabled: true,
            client_id: String::new(),
            client_secret: String::new(),
            // Explicitly write default status mappings for user visibility
            status_mapping: media_sync_config::default_trakt_status_mapping(),
        });
    }
    let trakt_config = config.trakt.as_mut().unwrap();
    let client_id = if let Some(id) = client_id_arg {
        id
    } else if !trakt_config.client_id.is_empty()
        && trakt_config.client_id != "YOUR_CLIENT_ID"
    {
        loop {
            let input = prompts::prompt_string(
                "Trakt Client ID",
                Some(&trakt_config.client_id),
            )?;
            match validate_client_id(&input) {
                Ok(()) => break input,
                Err(e) => {
                    output.error(&format!("Validation error: {}", e));
                    output.info("You can find your Client ID at: https://trakt.tv/oauth/applications");
                    continue;
                }
            }
        }
    } else {
        loop {
            let input = prompts::prompt_string("Trakt Client ID", None)?;
            match validate_client_id(&input) {
                Ok(()) => break input,
                Err(e) => {
                    output.error(&format!("Validation error: {}", e));
                    output.info("You can find your Client ID at: https://trakt.tv/oauth/applications");
                    continue;
                }
            }
        }
    };

    // Get client secret
    let client_secret = if let Some(secret) = client_secret_arg {
        secret
    } else {
        loop {
            let mut password_prompt = dialoguer::Password::new()
                .with_prompt("Trakt Client Secret");
            
            // Only add confirmation if there's no existing secret (new setup)
            if trakt_config.client_secret.is_empty() || trakt_config.client_secret == "YOUR_CLIENT_SECRET" {
                password_prompt = password_prompt.with_confirmation("Confirm Trakt Client Secret", "Passwords do not match");
            }
            
            let input = password_prompt
                .interact()
                .map_err(|e| color_eyre::eyre::eyre!("Failed to read password: {}", e))?;
            match validate_password_strength(&input) {
                Ok(()) => break input,
                Err(e) => {
                    output.error(&format!("Validation error: {}", e));
                    output.info("The Client Secret will be hidden as you type for security.");
                    continue;
                }
            }
        }
    };

    if client_id.is_empty() || client_secret.is_empty() {
        return Err(color_eyre::eyre::eyre!("Client ID and Client Secret are required"));
    }

    // Update config with default status mappings (explicitly written for user visibility)
    trakt_config.enabled = true;
    trakt_config.client_id = client_id.clone();
    trakt_config.client_secret = client_secret.clone();
    // Only update status_mapping if it's empty (first time setup)
    if trakt_config.status_mapping.to_normalized.is_empty() && trakt_config.status_mapping.from_normalized.is_empty() {
        trakt_config.status_mapping = media_sync_config::default_trakt_status_mapping();
    }
    config.save_to_file(&config_file)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save config to {}: {}", config_file.display(), e))?;

    // Perform OAuth flow
    let credentials_file = path_manager.credentials_file();
    let mut cred_store = CredentialStore::new(credentials_file.clone());
    cred_store.load()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to load credentials from {}: {}", credentials_file.display(), e))?;

    let refresh_token = cred_store.get_trakt_refresh_token().map(|s| s.as_str());

    // Show OAuth progress
    output.println("");
    
    let token_info = if let Some(refresh_token) = refresh_token {
        // Try to refresh token first - but don't use spinner since it might fail and prompt
        // We'll try refresh directly, and if it fails, we'll handle the prompt cleanly
        print_oauth_progress("Attempting to refresh Trakt token...", output);
        
        // Try refresh without spinner - if it fails, we'll get an error and can handle it
        match trakt_authenticate(&client_id, &client_secret, Some(refresh_token)).await {
            Ok(token_info) => {
                // Refresh succeeded!
                token_info
            }
            Err(_) => {
                // Refresh failed, need new authorization - this will prompt for input
                output.println("");
                print_oauth_progress("Token refresh failed. Starting new authorization...", output);
                // Ensure output is flushed before prompting
                use std::io::Write;
                std::io::stdout().flush().ok();
                // Call authenticate again without refresh token - this will prompt
                trakt_authenticate(&client_id, &client_secret, None).await
                    .map_err(|e| color_eyre::eyre::eyre!("Trakt OAuth authentication failed: {}", e))?
            }
        }
    } else {
        // No refresh token, will need to prompt - don't use spinner
        print_oauth_progress("Starting Trakt OAuth authentication...", output);
        trakt_authenticate(&client_id, &client_secret, None).await
            .map_err(|e| color_eyre::eyre::eyre!("Trakt OAuth authentication failed: {}", e))?
    };
    print_oauth_progress("Authentication successful! Saving credentials...", output);

    // Save tokens
    cred_store.set_trakt_access_token(token_info.access_token);
    cred_store.set_trakt_refresh_token(token_info.refresh_token);
    cred_store.set_trakt_token_expires(token_info.expires_at);
    cred_store.save()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save credentials to {}: {}", credentials_file.display(), e))?;

    output.println("");
    output.success("Trakt authentication successful!");
    output.println(&format!("  Access token expires at: {}", token_info.expires_at.bright_green()));

    Ok(())
}

async fn configure_simkl(client_id_arg: Option<String>, client_secret_arg: Option<String>, output: &Output) -> Result<()> {
    let path_manager = PathManager::default();
    path_manager.ensure_directories()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create configuration directories: {}", e))?;

    let config_file = path_manager.config_file();
    let mut config = if config_file.exists() {
        Config::load_from_file(&config_file)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to load config from {}: {}", config_file.display(), e))?
    } else {
        output.info("Configuration file not found. Creating default configuration...");
        let default_config = Config {
            trakt: Some(TraktConfig {
                enabled: false,
                client_id: String::new(),
                client_secret: String::new(),
                status_mapping: media_sync_config::StatusMapping {
                    to_normalized: std::collections::HashMap::new(),
                    from_normalized: std::collections::HashMap::new(),
                },
            }),
            simkl: None,
            resolution: media_sync_config::ResolutionConfig {
                source_preference: Vec::new(), // Must be set via interactive prompt
                ..media_sync_config::ResolutionConfig::default()
            },
            sources: media_sync_config::SourceConfig {
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
            scheduler: Some(media_sync_config::default_scheduler_config()),
        };
        default_config
    };

    print_section_header("Simkl API Setup", output);
    output.println("");
    output.println("Follow the instructions to setup your Simkl API application:");
    print_instruction_list(&[
        "Login to Simkl and navigate to your API apps page: https://simkl.com/settings/developer/new/",
        "Create a new API application named 'TotalRecall'",
        "No redirect URI is needed - we use device PIN authentication",
    ], output);
    output.println("");
    output.println("During authentication, you'll be shown a PIN code.");
    output.println("Visit the provided URL and enter the PIN to complete authorization.");
    output.println("");

    let client_id = if let Some(id) = client_id_arg {
        id
    } else if let Some(ref simkl_config) = config.simkl {
        if !simkl_config.client_id.is_empty() && simkl_config.client_id != "YOUR_CLIENT_ID" {
            loop {
                let input = prompts::prompt_string(
                    "Simkl Client ID",
                    Some(&simkl_config.client_id),
                )?;
                match validate_client_id(&input) {
                    Ok(()) => break input,
                    Err(e) => {
                        output.error(&format!("Validation error: {}", e));
                        output.info("You can find your Client ID at: https://simkl.com/oauth/applications");
                        continue;
                    }
                }
            }
        } else {
            loop {
                let input = prompts::prompt_string("Simkl Client ID", None)?;
                match validate_client_id(&input) {
                    Ok(()) => break input,
                    Err(e) => {
                        output.error(&format!("Validation error: {}", e));
                        output.info("You can find your Client ID at: https://simkl.com/oauth/applications");
                        continue;
                    }
                }
            }
        }
    } else {
        loop {
            let input = prompts::prompt_string("Simkl Client ID", None)?;
            match validate_client_id(&input) {
                Ok(()) => break input,
                Err(e) => {
                    output.error(&format!("Validation error: {}", e));
                    output.info("You can find your Client ID at: https://simkl.com/oauth/applications");
                    continue;
                }
            }
        }
    };

    // Get client secret
    let client_secret = if let Some(secret) = client_secret_arg {
        secret
    } else {
        let existing_secret = config.simkl.as_ref()
            .map(|s| s.client_secret.as_str())
            .filter(|s| !s.is_empty() && *s != "YOUR_CLIENT_SECRET");
        loop {
            let mut password_prompt = dialoguer::Password::new()
                .with_prompt("Simkl Client Secret");
            
            // Only add confirmation if there's no existing secret (new setup)
            if existing_secret.is_none() {
                password_prompt = password_prompt.with_confirmation("Confirm Simkl Client Secret", "Passwords do not match");
            }
            
            let input = password_prompt
                .interact()
                .map_err(|e| color_eyre::eyre::eyre!("Failed to read password: {}", e))?;
            match validate_password_strength(&input) {
                Ok(()) => break input,
                Err(e) => {
                    output.error(&format!("Validation error: {}", e));
                    output.info("The Client Secret will be hidden as you type for security.");
                    continue;
                }
            }
        }
    };

    if client_id.is_empty() || client_secret.is_empty() {
        return Err(color_eyre::eyre::eyre!("Client ID and Client Secret are required"));
    }

    // Update config with default status mappings (explicitly written for user visibility)
    config.simkl = Some(SimklConfig {
        enabled: true,
        client_id: client_id.clone(),
        client_secret: client_secret.clone(),
        status_mapping: default_simkl_status_mapping(),
    });
    config.save_to_file(&config_file)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save config to {}: {}", config_file.display(), e))?;

    // Perform OAuth flow
    let credentials_file = path_manager.credentials_file();
    let mut cred_store = CredentialStore::new(credentials_file.clone());
    cred_store.load()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to load credentials from {}: {}", credentials_file.display(), e))?;

    let refresh_token = cred_store.get_simkl_refresh_token().map(|s| s.as_str());

    // Show OAuth progress
    output.println("");
    print_oauth_progress("Starting Simkl OAuth authentication...", output);
    
    // The simkl_authenticate function handles device code flow with automatic polling
    // It will display the PIN and verification URL, then poll until authorized
    let token_info = simkl_authenticate(&client_id, &client_secret, refresh_token)
        .await
        .map_err(|e| {
            color_eyre::eyre::eyre!("Simkl OAuth authentication failed: {}", e)
        })?;

    print_oauth_progress("Authentication successful! Saving credentials...", output);

    // Save tokens
    cred_store.set_simkl_access_token(token_info.access_token);
    cred_store.set_simkl_refresh_token(token_info.refresh_token);
    cred_store.set_simkl_token_expires(token_info.expires_at);
    cred_store.save()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save credentials to {}: {}", credentials_file.display(), e))?;

    output.println("");
    output.success("Simkl authentication successful!");
    output.println(&format!("  Access token expires at: {}", token_info.expires_at.bright_green()));

    Ok(())
}

async fn configure_imdb(username_arg: Option<String>, output: &Output) -> Result<()> {
    let path_manager = PathManager::default();
    path_manager.ensure_directories()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create configuration directories: {}", e))?;

    let config_file = path_manager.config_file();
    let mut config = if config_file.exists() {
        Config::load_from_file(&config_file)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to load config from {}: {}", config_file.display(), e))?
    } else {
        output.info("Configuration file not found. Creating default configuration...");
        let default_config = Config {
            trakt: Some(TraktConfig {
                enabled: false,
                client_id: String::new(),
                client_secret: String::new(),
                status_mapping: media_sync_config::default_trakt_status_mapping(),
            }),
            simkl: None,
            resolution: media_sync_config::ResolutionConfig {
                source_preference: Vec::new(), // Must be set via interactive prompt
                ..media_sync_config::ResolutionConfig::default()
            },
            sources: media_sync_config::SourceConfig {
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
            scheduler: Some(media_sync_config::default_scheduler_config()),
        };
        default_config
    };

    print_section_header("IMDB Credentials Setup", output);
    output.println("");

    let username = if let Some(user) = username_arg {
        user
    } else {
        let existing_username = config
            .sources
            .imdb
            .as_ref()
            .map(|imdb| imdb.username.as_str());
        loop {
            let input = prompts::prompt_string(
                "IMDB Username (email or phone number)",
                existing_username,
            )?;
            match validate_email_or_phone(&input) {
                Ok(()) => break input,
                Err(e) => {
                    output.error(&format!("Validation error: {}", e));
                    output.info("Enter your IMDB account email address or phone number");
                    continue;
                }
            }
        }
    };

    if username.is_empty() {
        return Err(color_eyre::eyre::eyre!("Username is required"));
    }

    // Get password
    let password = loop {
        let input = dialoguer::Password::new()
            .with_prompt("IMDB Password")
            .with_confirmation("Confirm IMDB Password", "Passwords do not match")
            .interact()
            .map_err(|e| color_eyre::eyre::eyre!("Failed to read password: {}", e))?;
        match validate_password_strength(&input) {
            Ok(()) => break input,
            Err(e) => {
                output.error(&format!("Validation error: {}", e));
                output.info("Your password will be securely stored and hidden as you type.");
                continue;
            }
        }
    };

    if password.is_empty() {
        return Err(color_eyre::eyre::eyre!("Password is required"));
    }

    // Ask if IMDB should be enabled
    let enabled = prompts::prompt_yes_no("Enable IMDB sync?", Some(true))?;

    // Update config with default status mappings (explicitly written for user visibility)
    config.sources.imdb = Some(media_sync_config::ImdbConfig {
        enabled,
        username: username.clone(),
        status_mapping: media_sync_config::default_imdb_status_mapping(),
    });
    config.save_to_file(&config_file)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save config to {}: {}", config_file.display(), e))?;

    // Save password to credentials
    let credentials_file = path_manager.credentials_file();
    let mut cred_store = CredentialStore::new(credentials_file.clone());
    cred_store.load()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to load credentials from {}: {}", credentials_file.display(), e))?;
    cred_store.set_imdb_password(password);
    cred_store.save()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save credentials to {}: {}", credentials_file.display(), e))?;

    output.success("\nIMDB credentials saved!");
    output.println(&format!("  Username: {}", username));
    output.println(&format!("  Enabled: {}", enabled));

    Ok(())
}

async fn configure_plex(token_arg: Option<String>, server_url_arg: Option<String>, output: &Output) -> Result<()> {
    let path_manager = PathManager::default();
    path_manager.ensure_directories()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create configuration directories: {}", e))?;

    let config_file = path_manager.config_file();
    let mut config = if config_file.exists() {
        Config::load_from_file(&config_file)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to load config from {}: {}", config_file.display(), e))?
    } else {
        output.info("Configuration file not found. Creating default configuration...");
        let default_config = Config {
            trakt: None,
            simkl: None,
            resolution: media_sync_config::ResolutionConfig {
                source_preference: Vec::new(),
                ..media_sync_config::ResolutionConfig::default()
            },
            sources: media_sync_config::SourceConfig {
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
            scheduler: Some(media_sync_config::default_scheduler_config()),
        };
        default_config
    };

    print_section_header("Plex API Setup", output);
    output.println("");
    output.println("Configure Plex for MyPlex cloud access:");
    print_instruction_list(&[
        "Your Plex token can be found in your Plex account settings",
        "Or by inspecting network requests in Plex Web (look for X-Plex-Token header)",
        "The token is used for MyPlex cloud-based authentication",
        "Server URL is optional and can be used for direct server access",
    ], output);
    output.println("");

    // Initialize Plex config if it doesn't exist
    if config.sources.plex.is_none() {
        config.sources.plex = Some(PlexConfig {
            enabled: true,
            server_url: String::new(),
            status_mapping: default_plex_status_mapping(),
        });
    }
    let plex_config = config.sources.plex.as_mut().unwrap();

    // Get token
    let token = if let Some(t) = token_arg {
        t
    } else {
        // Check if we have an existing token in credentials
        let credentials_file = path_manager.credentials_file();
        let mut cred_store = CredentialStore::new(credentials_file.clone());
        let existing_token = cred_store.load()
            .ok()
            .and_then(|_| cred_store.get_plex_token().cloned());
        
        loop {
            let mut password_prompt = dialoguer::Password::new()
                .with_prompt("Plex API Token");
            
            // Only add confirmation if there's no existing token (new setup)
            if existing_token.is_none() {
                password_prompt = password_prompt.with_confirmation("Confirm Plex API Token", "Tokens do not match");
            }
            
            let input = password_prompt
                .interact()
                .map_err(|e| color_eyre::eyre::eyre!("Failed to read token: {}", e))?;
            
            if input.is_empty() {
                output.error("Token cannot be empty");
                continue;
            }
            
            // Basic validation - Plex tokens are typically alphanumeric
            if input.len() < 10 {
                output.warn("Token seems too short. Please verify it's correct.");
                if !prompts::prompt_yes_no("Continue with this token?", Some(true))? {
                    continue;
                }
            }
            
            break input;
        }
    };

    if token.is_empty() {
        return Err(color_eyre::eyre::eyre!("Plex token is required"));
    }

    // Get server URL (optional)
    let existing_server_url = plex_config.server_url.clone();
    let server_url = if let Some(url) = server_url_arg {
        url
    } else {
        let existing_url = if !existing_server_url.is_empty() {
            Some(existing_server_url.as_str())
        } else {
            None
        };
        
        let input = prompts::prompt_string(
            "Plex Server URL (optional, press Enter to skip)",
            existing_url,
        )?;
        input.trim().to_string()
    };

    // Verify token (optional but good UX)
    output.println("");
    output.info("Verifying Plex token...");
    
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.set_style(
        indicatif::ProgressStyle::default_spinner()
            .template("{spinner:.blue} {msg}")
            .unwrap()
    );
    spinner.set_message("Verifying token...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    // Try to create a MyPlex client with the token to verify it works
    use media_sync_sources::plex::auth;
    match auth::verify_token(&token).await {
        Ok(true) => {
            spinner.finish_and_clear();
            output.success("Token verified successfully!");
        },
        Ok(false) => {
            spinner.finish_and_clear();
            output.warn("Token verification failed. The token may be invalid.");
            if !prompts::prompt_yes_no("Continue anyway?", Some(false))? {
                return Err(color_eyre::eyre::eyre!("Token verification failed"));
            }
        },
        Err(e) => {
            spinner.finish_and_clear();
            output.warn(&format!("Could not verify token: {}. Continuing anyway...", e));
        }
    }

    // Ask if Plex should be enabled
    let enabled = prompts::prompt_yes_no("Enable Plex sync?", Some(true))?;

    // Update config (drop mutable borrow first)
    let server_url_display = server_url.clone();
    {
        let plex_config = config.sources.plex.as_mut().unwrap();
        plex_config.enabled = enabled;
        plex_config.server_url = server_url;
    }
    config.save_to_file(&config_file)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save config to {}: {}", config_file.display(), e))?;

    // Save token to credentials
    let credentials_file = path_manager.credentials_file();
    let mut cred_store = CredentialStore::new(credentials_file.clone());
    cred_store.load()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to load credentials from {}: {}", credentials_file.display(), e))?;
    cred_store.set_plex_token(token);
    cred_store.save()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save credentials to {}: {}", credentials_file.display(), e))?;

    output.println("");
    output.success("Plex configuration saved!");
    output.println(&format!("  Enabled: {}", enabled));
    if !server_url_display.is_empty() {
        output.println(&format!("  Server URL: {}", server_url_display));
    }

    Ok(())
}

async fn configure_sync(
    enable_watchlist: Option<bool>,
    enable_ratings: Option<bool>,
    enable_reviews: Option<bool>,
    enable_watch_history: Option<bool>,
    output: &Output,
) -> Result<()> {
    let path_manager = PathManager::default();
    let config_file = path_manager.config_file();

    if !config_file.exists() {
        output.warn("Configuration file not found. It will be created automatically when you configure your first source.");
        return Ok(());
    }

    let mut config = Config::load_from_file(&config_file)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to load config from {}: {}", config_file.display(), e))?;

    print_section_header("Sync Options Configuration", output);
    output.println("");
    output.println("Configure which data should be synced between sources.");
    output.println("");

    // Sync watchlist
    config.sync.sync_watchlist = if let Some(val) = enable_watchlist {
        val
    } else {
        prompts::prompt_yes_no(
            "Do you want to sync watchlists?",
            Some(config.sync.sync_watchlist),
        )?
    };

    // Sync ratings
    config.sync.sync_ratings = if let Some(val) = enable_ratings {
        val
    } else {
        prompts::prompt_yes_no(
            "Do you want to sync ratings?",
            Some(config.sync.sync_ratings),
        )?
    };

    // Sync reviews
    output.println("\nReviews synced to IMDB will use 'My Review' as the title field.");
    config.sync.sync_reviews = if let Some(val) = enable_reviews {
        val
    } else {
        prompts::prompt_yes_no(
            "Do you want to sync reviews?",
            Some(config.sync.sync_reviews),
        )?
    };

    // Sync watch history
    output.println("\nTrakt watch history is synced using IMDB Check-ins.");
    output.println("See FAQ: https://help.imdb.com/article/imdb/track-movies-tv/check-ins-faq/GG59ELYW45FMC7J3");
    config.sync.sync_watch_history = if let Some(val) = enable_watch_history {
        val
    } else {
        prompts::prompt_yes_no(
            "Do you want to sync your watch history?",
            Some(config.sync.sync_watch_history),
        )?
    };

    // Remove watched from watchlists
    output.println("");
    output.println("Movies and Episodes are removed from watchlists after 1 play.");
    output.println("Shows are removed when at least 80% of the episodes are watched AND the series is marked as ended or cancelled.");
    config.sync.remove_watched_from_watchlists = prompts::prompt_yes_no(
        "Do you want to remove watched items from watchlists?",
        Some(config.sync.remove_watched_from_watchlists),
    )?;

    // Mark rated as watched
    config.sync.mark_rated_as_watched = prompts::prompt_yes_no(
        "Do you want to mark rated movies and episodes as watched?",
        Some(config.sync.mark_rated_as_watched),
    )?;

    // Remove old watchlist items
    output.println("\nIf choosing (y) in the following, you will be prompted to enter the number of days.");
    output.println("This setting is meant to help address the 100 item limit in Trakt watchlists for free tier users.");
    output.println("In order to prevent old items from being re-added, it is recommended to disable Trakt watchlist sync");
    output.println("in other projects when enabling this setting. Such as Reelgood and other similar apps.");
    output.println("If you use the PlexTraktSync project, it is recommended to disable watchlist sync from Plex to Trakt.");
    let remove_old = prompts::prompt_yes_no(
        "Do you want to remove watchlist items older than x number of days?",
        Some(config.sync.remove_watchlist_items_older_than_days.is_some()),
    )?;

    if remove_old {
        let default_days = config.sync.remove_watchlist_items_older_than_days.unwrap_or(90);
        output.println("\nFor reference: (30 = 1 month, 90 = 3 months, 180 = 6 months, 365 = 1 year). Any number of days is valid.");
        let days = prompts::prompt_number_with_output(
            "How many days old should the items be to be removed?",
            Some(default_days),
            Some(output),
        )?;
        config.sync.remove_watchlist_items_older_than_days = Some(days);
    } else {
        config.sync.remove_watchlist_items_older_than_days = None;
    }

    // Save config
    config.save_to_file(&config_file)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save config to {}: {}", config_file.display(), e))?;

    output.success("\nSync options saved!");

    Ok(())
}

fn mask_string(s: &str) -> String {
    if s.is_empty() || s == "YOUR_CLIENT_ID" || s == "YOUR_CLIENT_SECRET" {
        return "<not set>".to_string();
    }
    if s.len() <= 4 {
        return "*".repeat(s.len());
    }
    format!("{}***{}", &s[..2], &s[s.len() - 2..])
}

// Validation helpers

/// Validates Trakt Client ID format
fn validate_client_id(input: &str) -> Result<(), &'static str> {
    if input.is_empty() {
        return Err("Client ID cannot be empty");
    }
    if input.len() < 10 {
        return Err("Client ID seems too short. Please verify it's correct.");
    }
    Ok(())
}

/// Validates email or phone number format for IMDB username
fn validate_email_or_phone(input: &str) -> Result<(), &'static str> {
    if input.is_empty() {
        return Err("Username cannot be empty");
    }
    // Basic validation: check if it looks like an email or phone number
    let is_email = input.contains('@') && input.contains('.');
    let is_phone = input.chars().all(|c| c.is_ascii_digit() || c == '+' || c == '-' || c == ' ' || c == '(' || c == ')');
    
    if !is_email && !is_phone {
        return Err("Username should be an email address or phone number");
    }
    Ok(())
}

/// Basic password validation
fn validate_password_strength(input: &str) -> Result<(), &'static str> {
    if input.is_empty() {
        return Err("Password cannot be empty");
    }
    if input.len() < 6 {
        return Err("Password must be at least 6 characters long");
    }
    Ok(())
}

// Formatting helpers

/// Print a formatted section header
fn print_section_header(title: &str, output: &Output) {
    output.println("");
    output.println(&format!("{}", title.bold().bright_cyan()));
    output.println(&format!("{}", "─".repeat(title.len()).bright_cyan()));
}

/// Print a numbered instruction list
fn print_instruction_list(items: &[&str], output: &Output) {
    for (idx, item) in items.iter().enumerate() {
        output.println(&format!("  {}. {}", idx + 1, item));
    }
}

/// Print OAuth progress message
fn print_oauth_progress(message: &str, output: &Output) {
    output.println(&format!("{} {}", "→".bright_blue(), message.bright_white()));
}

/// Get list of configured/enabled services from config
fn get_configured_services(config: &Config) -> Vec<String> {
    let mut services = Vec::new();
    
    // Check Trakt
    if let Some(ref trakt) = config.trakt {
        if trakt.enabled && !trakt.client_id.is_empty() && trakt.client_id != "YOUR_CLIENT_ID" {
            services.push("trakt".to_string());
        }
    }
    
    if let Some(ref simkl) = config.simkl {
        if simkl.enabled && !simkl.client_id.is_empty() && simkl.client_id != "YOUR_CLIENT_ID" {
            services.push("simkl".to_string());
        }
    }
    
    if let Some(ref imdb) = config.sources.imdb {
        if imdb.enabled && !imdb.username.is_empty() {
            services.push("imdb".to_string());
        }
    }
    
    if let Some(ref plex) = config.sources.plex {
        if plex.enabled {
            services.push("plex".to_string());
        }
    }
    
    services
}

/// Prompt user to set source_preference order interactively
fn prompt_source_preference(config: &Config, output: &Output) -> Result<Vec<String>> {
    let configured_services = get_configured_services(config);
    
    if configured_services.is_empty() {
        return Err(color_eyre::eyre::eyre!("No services are configured. Please configure at least one service first."));
    }
    
    output.println("");
    print_section_header("Source Preference Configuration", output);
    output.println("");
    output.println("Source preference determines the order used for conflict resolution.");
    output.println("Position 1 = highest priority (used as tiebreaker when timestamps are within tolerance).");
    output.println("");
    output.println(&format!("Configured services: {}", configured_services.join(", ")));
    output.println("");
    
    let mut source_preference = Vec::new();
    let mut used_positions = std::collections::HashSet::new();
    
    for service in &configured_services {
        loop {
            let position = prompts::prompt_number(
                &format!("What position should {} be? (1 = highest priority)", service),
                None,
            )?;
            
            if position == 0 {
                output.error("Position must be at least 1");
                continue;
            }
            
            if used_positions.contains(&position) {
                output.error(&format!("Position {} is already assigned. Please choose a different position.", position));
                continue;
            }
            
            used_positions.insert(position);
            source_preference.push((position, service.clone()));
            break;
        }
    }
    
    // Sort by position and extract service names
    source_preference.sort_by_key(|(pos, _)| *pos);
    let result: Vec<String> = source_preference.into_iter().map(|(_, service)| service).collect();
    
    Ok(result)
}

/// Load config and prompt for source_preference if missing
pub fn load_config_or_prompt_source_preference(output: &Output) -> Result<Config> {
    let path_manager = PathManager::default();
    let config_file = path_manager.config_file();
    
    let mut config = if config_file.exists() {
        Config::load_from_file(&config_file)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to load config from {}: {}", config_file.display(), e))?
    } else {
        return Err(color_eyre::eyre::eyre!("Configuration file not found. Please run 'totalrecall config' to set up your configuration."));
    };
    
    // Check if source_preference is missing or empty
    if config.resolution.source_preference.is_empty() {
        output.warn("source_preference is not set in your configuration.");
        output.info("You need to configure the source preference order for conflict resolution.");
        
        let source_preference = prompt_source_preference(&config, output)?;
        config.resolution.source_preference = source_preference;
        
        // Save updated config
        config.save_to_file(&config_file)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to save config to {}: {}", config_file.display(), e))?;
        
        output.println("");
        output.info("Configuration updated with source preference order.");
    }
    
    Ok(config)
}

/// Run interactive configuration wizard
pub async fn run_interactive_config(output: &Output) -> Result<()> {
    let path_manager = PathManager::default();
    let config_file = path_manager.config_file();
    
    // Load existing config or create default
    let mut config = if config_file.exists() {
        let mut loaded_config = Config::load_from_file(&config_file)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to load config from {}: {}", config_file.display(), e))?;
        
        // If source_preference is missing/empty, prompt for it
        if loaded_config.resolution.source_preference.is_empty() {
            output.warn("source_preference is not set in your configuration.");
            output.info("You need to configure the source preference order for conflict resolution.");
            output.println("");
            
            let source_preference = prompt_source_preference(&loaded_config, output)?;
            loaded_config.resolution.source_preference = source_preference;
            
            // Save updated config before continuing
            loaded_config.save_to_file(&config_file)
                .map_err(|e| color_eyre::eyre::eyre!("Failed to save config to {}: {}", config_file.display(), e))?;
            
            output.println("");
            output.info("Configuration updated with source preference order.");
            output.println("");
        }
        
        loaded_config
    } else {
        output.info("Starting interactive configuration wizard...");
        output.println("");
        // Create default config
        Config {
            trakt: Some(TraktConfig {
                enabled: false,
                client_id: String::new(),
                client_secret: String::new(),
                status_mapping: media_sync_config::default_trakt_status_mapping(),
            }),
            simkl: None,
            resolution: media_sync_config::ResolutionConfig {
                source_preference: Vec::new(),
                ..media_sync_config::ResolutionConfig::default()
            },
            sources: media_sync_config::SourceConfig {
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
            scheduler: Some(media_sync_config::default_scheduler_config()),
        }
    };
    
    output.println("");
    print_section_header("Interactive Configuration Wizard", output);
    output.println("");
    output.println("This wizard will guide you through configuring all services and preferences.");
    output.println("");
    
    // Step 1: Configure services
    print_section_header("Step 1: Configure Services", output);
    output.println("");
    
    // Trakt
    if prompts::prompt_yes_no("Enable Trakt?", Some(false))? {
        if config.trakt.is_none() || !config.trakt.as_ref().map(|t| t.enabled).unwrap_or(false) {
            configure_trakt(None, None, output).await?;
            config = Config::load_from_file(&config_file)
                .map_err(|e| color_eyre::eyre::eyre!("Failed to reload config: {}", e))?;
        } else {
            output.info("Trakt is already configured.");
        }
    }
    output.println("");
    
    // Simkl
    if prompts::prompt_yes_no("Enable Simkl?", Some(false))? {
        if config.simkl.is_none() || !config.simkl.as_ref().map(|s| s.enabled).unwrap_or(false) {
            configure_simkl(None, None, output).await?;
            config = Config::load_from_file(&config_file)
                .map_err(|e| color_eyre::eyre::eyre!("Failed to reload config: {}", e))?;
        } else {
            output.info("Simkl is already configured.");
        }
    }
    output.println("");
    
    // IMDB
    if prompts::prompt_yes_no("Enable IMDB?", Some(false))? {
        if config.sources.imdb.is_none() || !config.sources.imdb.as_ref().map(|i| i.enabled).unwrap_or(false) {
            configure_imdb(None, output).await?;
            config = Config::load_from_file(&config_file)
                .map_err(|e| color_eyre::eyre::eyre!("Failed to reload config: {}", e))?;
        } else {
            output.info("IMDB is already configured.");
        }
    }
    output.println("");
    
    // Plex
    if prompts::prompt_yes_no("Enable Plex?", Some(false))? {
        if config.sources.plex.is_none() || !config.sources.plex.as_ref().map(|p| p.enabled).unwrap_or(false) {
            configure_plex(None, None, output).await?;
            config = Config::load_from_file(&config_file)
                .map_err(|e| color_eyre::eyre::eyre!("Failed to reload config: {}", e))?;
        } else {
            output.info("Plex is already configured.");
        }
    }
    output.println("");
    
    // Step 2: Configure source preference
    print_section_header("Step 2: Configure Source Preference", output);
    output.println("");
    
    let source_preference = prompt_source_preference(&config, output)?;
    config.resolution.source_preference = source_preference;
    
    // Step 3: Configure sync preferences
    print_section_header("Step 3: Configure Sync Preferences", output);
    output.println("");
    
    config.sync.sync_watchlist = prompts::prompt_yes_no("Enable watchlist syncing?", Some(config.sync.sync_watchlist))?;
    config.sync.sync_ratings = prompts::prompt_yes_no("Enable ratings syncing?", Some(config.sync.sync_ratings))?;
    config.sync.sync_reviews = prompts::prompt_yes_no("Enable reviews syncing?", Some(config.sync.sync_reviews))?;
    config.sync.sync_watch_history = prompts::prompt_yes_no("Enable watch history syncing?", Some(config.sync.sync_watch_history))?;
    
    // Remove watched from watchlists
    output.println("");
    output.println("Movies and Episodes are removed from watchlists after 1 play.");
    output.println("Shows are removed when at least 80% of the episodes are watched AND the series is marked as ended or cancelled.");
    config.sync.remove_watched_from_watchlists = prompts::prompt_yes_no(
        "Do you want to remove watched items from watchlists?",
        Some(config.sync.remove_watched_from_watchlists),
    )?;

    // Mark rated as watched
    config.sync.mark_rated_as_watched = prompts::prompt_yes_no(
        "Do you want to mark rated movies and episodes as watched?",
        Some(config.sync.mark_rated_as_watched),
    )?;

    // Remove old watchlist items
    output.println("\nIf choosing (y) in the following, you will be prompted to enter the number of days.");
    output.println("This setting is meant to help address the 100 item limit in Trakt watchlists for free tier users.");
    let remove_old = prompts::prompt_yes_no(
        "Do you want to remove watchlist items older than x number of days?",
        Some(config.sync.remove_watchlist_items_older_than_days.is_some()),
    )?;

    if remove_old {
        let default_days = config.sync.remove_watchlist_items_older_than_days.unwrap_or(90);
        output.println("\nFor reference: (30 = 1 month, 90 = 3 months, 180 = 6 months, 365 = 1 year). Any number of days is valid.");
        let days = prompts::prompt_number_with_output(
            "How many days old should the items be to be removed?",
            Some(default_days),
            Some(output),
        )?;
        config.sync.remove_watchlist_items_older_than_days = Some(days);
    } else {
        config.sync.remove_watchlist_items_older_than_days = None;
    }
    
    // Save final configuration
    config.save_to_file(&config_file)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save config to {}: {}", config_file.display(), e))?;
    
    output.println("");
    print_section_header("Configuration Complete", output);
    output.println("");
    output.info("Your configuration has been saved successfully!");
    output.println("");
    
    Ok(())
}
