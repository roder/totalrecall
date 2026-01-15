/// Source factory pattern for creating media sources from configuration
/// 
/// This module provides a factory pattern for creating media sources,
/// centralizing source creation logic and making the system extensible.

use anyhow::Result;
use async_trait::async_trait;
use media_sync_config::{Config, CredentialStore};
use crate::{MediaSource, SourceError};

/// Factory trait for creating media sources from configuration
#[async_trait]
pub trait SourceFactory: Send + Sync {
    /// The name of the source this factory creates
    fn source_name(&self) -> &str;
    
    /// Create a source instance from configuration
    /// Returns None if the source is not enabled or not configured
    async fn create_source(
        &self,
        config: &Config,
        credentials: &CredentialStore,
    ) -> Result<Option<Box<dyn MediaSource<Error = SourceError>>>>;
    
    /// Validate that the source configuration is valid
    /// This is called before attempting to create the source
    fn validate_config(&self, config: &Config) -> Result<()>;
    
    /// Check if this source is required (must be present)
    /// Defaults to false - most sources are optional
    fn is_required(&self) -> bool {
        false
    }
}

/// Registry of source factories
pub struct SourceFactoryRegistry {
    factories: std::collections::HashMap<String, Box<dyn SourceFactory>>,
}

impl SourceFactoryRegistry {
    /// Create a new registry with all built-in factories registered
    pub fn new() -> Self {
        let mut registry = Self {
            factories: std::collections::HashMap::new(),
        };
        
        // Register built-in factories
        registry.register(Box::new(trakt::TraktSourceFactory));
        registry.register(Box::new(simkl::SimklSourceFactory));
        registry.register(Box::new(imdb::ImdbSourceFactory));
        registry.register(Box::new(plex::PlexSourceFactory));
        
        registry
    }
    
    /// Register a new factory
    pub fn register(&mut self, factory: Box<dyn SourceFactory>) {
        self.factories.insert(factory.source_name().to_string(), factory);
    }
    
    /// Create all enabled sources from configuration
    pub async fn create_all_sources(
        &self,
        config: &Config,
        credentials: &CredentialStore,
    ) -> Result<Vec<Box<dyn MediaSource<Error = SourceError>>>> {
        let mut sources = Vec::new();
        
        for factory in self.factories.values() {
            if let Some(source) = factory.create_source(config, credentials).await? {
                sources.push(source);
            }
        }
        
        Ok(sources)
    }
    
    /// Create a specific source by name
    pub async fn create_source_by_name(
        &self,
        name: &str,
        config: &Config,
        credentials: &CredentialStore,
    ) -> Result<Option<Box<dyn MediaSource<Error = SourceError>>>> {
        if let Some(factory) = self.factories.get(name) {
            factory.create_source(config, credentials).await
        } else {
            Ok(None)
        }
    }
    
    /// Validate all source configurations
    pub fn validate_all_configs(&self, config: &Config) -> Result<()> {
        for factory in self.factories.values() {
            factory.validate_config(config)?;
        }
        Ok(())
    }
    
    /// Get all registered factory names
    pub fn registered_sources(&self) -> Vec<&str> {
        self.factories.keys().map(|s| s.as_str()).collect()
    }
    
    /// Check if a source is registered
    pub fn is_registered(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }
}

impl Default for SourceFactoryRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Factory implementations for each source
mod trakt {
    use super::*;
    use crate::trakt::TraktClient;
    
    pub struct TraktSourceFactory;
    
    #[async_trait::async_trait]
    impl SourceFactory for TraktSourceFactory {
        fn source_name(&self) -> &str {
            "trakt"
        }
        
        async fn create_source(
            &self,
            config: &Config,
            _credentials: &CredentialStore,
        ) -> Result<Option<Box<dyn MediaSource<Error = SourceError>>>> {
            if let Some(trakt_config) = &config.trakt {
                if trakt_config.enabled {
                    Ok(Some(Box::new(TraktClient::new(
                        trakt_config.client_id.clone(),
                        trakt_config.client_secret.clone(),
                    ))))
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            }
        }
        
        fn validate_config(&self, config: &Config) -> Result<()> {
            if let Some(trakt_config) = &config.trakt {
                if trakt_config.enabled {
                    if trakt_config.client_id.is_empty() || trakt_config.client_id == "YOUR_CLIENT_ID" {
                        return Err(anyhow::anyhow!("Trakt is enabled but client_id is not configured"));
                    }
                    if trakt_config.client_secret.is_empty() || trakt_config.client_secret == "YOUR_CLIENT_SECRET" {
                        return Err(anyhow::anyhow!("Trakt is enabled but client_secret is not configured"));
                    }
                }
            }
            Ok(())
        }
        
        fn is_required(&self) -> bool {
            false
        }
    }
}

mod simkl {
    use super::*;
    use crate::simkl::SimklClient;
    
    pub struct SimklSourceFactory;
    
    #[async_trait::async_trait]
    impl SourceFactory for SimklSourceFactory {
        fn source_name(&self) -> &str {
            "simkl"
        }
        
        async fn create_source(
            &self,
            config: &Config,
            _credentials: &CredentialStore,
        ) -> Result<Option<Box<dyn MediaSource<Error = SourceError>>>> {
            if let Some(simkl_config) = &config.simkl {
                if simkl_config.enabled {
                    let client = SimklClient::new(
                        simkl_config.client_id.clone(),
                        simkl_config.client_secret.clone(),
                    )
                    .with_status_mapping(simkl_config.status_mapping.clone());
                    return Ok(Some(Box::new(client)));
                }
            }
            Ok(None)
        }
        
        fn validate_config(&self, config: &Config) -> Result<()> {
            if let Some(simkl_config) = &config.simkl {
                if simkl_config.enabled {
                    if simkl_config.client_id.is_empty() || simkl_config.client_id == "YOUR_CLIENT_ID" {
                        return Err(anyhow::anyhow!("Simkl is enabled but client_id is not configured"));
                    }
                    if simkl_config.client_secret.is_empty() || simkl_config.client_secret == "YOUR_CLIENT_SECRET" {
                        return Err(anyhow::anyhow!("Simkl is enabled but client_secret is not configured"));
                    }
                }
            }
            Ok(())
        }
    }
}

mod imdb {
    use super::*;
    use crate::imdb::ImdbClient;
    
    pub struct ImdbSourceFactory;
    
    #[async_trait::async_trait]
    impl SourceFactory for ImdbSourceFactory {
        fn source_name(&self) -> &str {
            "imdb"
        }
        
        async fn create_source(
            &self,
            config: &Config,
            credentials: &CredentialStore,
        ) -> Result<Option<Box<dyn MediaSource<Error = SourceError>>>> {
            if let Some(imdb_config) = &config.sources.imdb {
                if imdb_config.enabled {
                    let password = credentials.get_imdb_password()
                        .ok_or_else(|| anyhow::anyhow!("IMDB password not found in credentials. Run 'totalrecall config imdb' first"))?
                        .clone();
                    
                    let client = ImdbClient::new(imdb_config.username.clone(), password).await?;
                    return Ok(Some(Box::new(client)));
                }
            }
            Ok(None)
        }
        
        fn validate_config(&self, config: &Config) -> Result<()> {
            if let Some(imdb_config) = &config.sources.imdb {
                if imdb_config.enabled {
                    if imdb_config.username.is_empty() {
                        return Err(anyhow::anyhow!("IMDB is enabled but username is not configured"));
                    }
                }
            }
            Ok(())
        }
    }
}

mod plex {
    use super::*;
    use crate::plex::PlexClient;
    
    pub struct PlexSourceFactory;
    
    #[async_trait::async_trait]
    impl SourceFactory for PlexSourceFactory {
        fn source_name(&self) -> &str {
            "plex"
        }
        
        async fn create_source(
            &self,
            config: &Config,
            credentials: &CredentialStore,
        ) -> Result<Option<Box<dyn MediaSource<Error = SourceError>>>> {
            if let Some(plex_config) = &config.sources.plex {
                if plex_config.enabled {
                    // Get Plex token from credentials
                    let token = credentials.get_plex_token()
                        .ok_or_else(|| anyhow::anyhow!("Plex token not found in credentials. Run 'totalrecall config plex' first"))?
                        .clone();
                    
                    let server_url = if plex_config.server_url.is_empty() {
                        None
                    } else {
                        Some(plex_config.server_url.clone())
                    };
                    
                    let client = PlexClient::with_server_url(token, server_url, plex_config.status_mapping.clone());
                    return Ok(Some(Box::new(client)));
                }
            }
            Ok(None)
        }
        
        fn validate_config(&self, config: &Config) -> Result<()> {
            if let Some(plex_config) = &config.sources.plex {
                if plex_config.enabled {
                    // Server URL is optional for MyPlex (cloud-based)
                    // Token is required and checked in create_source
                }
            }
            Ok(())
        }
    }
}

