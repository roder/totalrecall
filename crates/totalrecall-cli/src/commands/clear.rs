use crate::output::Output;
use color_eyre::eyre::Context;
use color_eyre::Result;
use media_sync_config::{CredentialStore, PathManager};
use media_sync_core::CacheManager;
use std::fs;

pub async fn run_clear(all: bool, cache: bool, credentials: bool, timestamps: bool, output: &Output) -> Result<()> {
    let path_manager = PathManager::default();

    if all {
        // Clear everything
        clear_cache(&path_manager, output).await?;
        clear_credentials(&path_manager, output).await?;
        clear_timestamps(&path_manager, output).await?;
        output.success("All cache, credentials, and timestamps cleared");
        return Ok(());
    }

    let mut cleared_anything = false;

    if cache {
        clear_cache(&path_manager, output).await?;
        cleared_anything = true;
    }

    if credentials {
        clear_credentials(&path_manager, output).await?;
        cleared_anything = true;
    }

    if timestamps {
        clear_timestamps(&path_manager, output).await?;
        cleared_anything = true;
    }

    if !cleared_anything {
        output.warn("No clear option specified. Use --cache, --credentials, --timestamps, or --all");
        output.println("\nExample: totalrecall clear --cache");
    }

    Ok(())
}

async fn clear_cache(path_manager: &PathManager, output: &Output) -> Result<()> {
    // Clear browser user data directory if it exists
    let data_dir = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .ok_or_else(|| color_eyre::eyre::eyre!("Could not determine data directory"))?;
    
    let browser_dir = data_dir.join("totalrecall").join("browser");
    
    if browser_dir.exists() {
        fs::remove_dir_all(&browser_dir)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to remove browser cache at {}: {}", browser_dir.display(), e))?;
        output.success(&format!("Cleared browser cache: {}", browser_dir.display()));
    } else {
        output.info("No browser cache found to clear");
    }

    // Clear download directory
    let download_dir = std::env::temp_dir().join("totalrecall_exports");
    if download_dir.exists() {
        fs::remove_dir_all(&download_dir)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to remove download cache at {}: {}", download_dir.display(), e))?;
        output.success(&format!("Cleared download cache: {}", download_dir.display()));
    } else {
        output.info("No download cache found to clear");
    }

    // Clear source data cache
    if let Ok(cache_manager) = CacheManager::new(path_manager) {
        if let Err(e) = cache_manager.clear_cache() {
            output.warn(&format!("Failed to clear source data cache: {}", e));
        } else {
            output.success(&format!("Cleared source data cache: {}", path_manager.cache_dir().display()));
        }
    } else {
        output.info("No source data cache found to clear");
    }

    Ok(())
}

async fn clear_credentials(path_manager: &PathManager, output: &Output) -> Result<()> {
    let credentials_file = path_manager.credentials_file();
    
    if credentials_file.exists() {
        fs::remove_file(&credentials_file)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to remove credentials file at {}: {}", credentials_file.display(), e))?;
        output.success(&format!("Cleared credentials: {}", credentials_file.display()));
    } else {
        output.info("No credentials file found to clear");
    }

    Ok(())
}

async fn clear_timestamps(path_manager: &PathManager, output: &Output) -> Result<()> {
    let mut cred_store = CredentialStore::new(path_manager.credentials_file());
    
    if !path_manager.credentials_file().exists() {
        output.info("No credentials file found, nothing to clear");
        return Ok(());
    }
    
    cred_store.load()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to load credentials: {}", e))?;
    
    // Collect all timestamp-related keys
    let keys_to_remove: Vec<String> = cred_store
        .get_all_keys()
        .into_iter()
        .filter(|k| k.contains("_last_sync_") || k == "simkl_last_activities")
        .collect();
    
    if keys_to_remove.is_empty() {
        output.info("No sync timestamps found to clear");
        return Ok(());
    }
    
    for key in &keys_to_remove {
        cred_store.remove(key);
    }
    
    cred_store.save()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to save credentials: {}", e))?;
    output.success(&format!("Cleared {} sync timestamp(s)", keys_to_remove.len()));
    Ok(())
}

