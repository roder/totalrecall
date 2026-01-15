use crate::output::Output;
use color_eyre::eyre::Context;
use color_eyre::Result;
use media_sync_config::{Config, CredentialStore, PathManager};
use media_sync_core::SyncOrchestrator;
use media_sync_sources::{SourceFactoryRegistry, MediaSource};
use tokio_cron_scheduler::JobScheduler;
use tracing::{error, info};

pub struct Scheduler {
    scheduler: JobScheduler,
    orchestrator: SyncOrchestrator,
    config: media_sync_config::SchedulerConfig,
}

impl Scheduler {
    pub async fn new(
        orchestrator: SyncOrchestrator,
        config: media_sync_config::SchedulerConfig,
    ) -> Result<Self> {
        let sched = JobScheduler::new().await?;

        Ok(Self {
            scheduler: sched,
            orchestrator,
            config,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        // Run sync immediately on startup if configured
        if self.config.run_on_startup {
            info!(
                operation = "scheduler_startup",
                "Running initial sync on startup"
            );
            self.run_sync().await?;
        }

        info!(
            operation = "scheduler_started",
            schedule = self.config.schedule,
            timezone = self.config.timezone,
            "Scheduler started successfully (using simple loop - tokio-cron-scheduler integration pending)"
        );

        // TODO: Implement proper tokio-cron-scheduler integration
        // For now, use a simple loop that runs every hour
        // The schedule parsing and proper cron execution will be added in a future iteration
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await; // Every hour as placeholder
            
            info!(operation = "scheduled_sync_start", "Starting scheduled sync");
            match self.run_sync().await {
                Ok(result) => {
                    info!(
                        operation = "scheduled_sync_complete",
                        items_synced = result.items_synced,
                        duration_ms = result.duration.as_millis(),
                        "Scheduled sync completed successfully"
                    );
                }
                Err(e) => {
                    error!(
                        operation = "scheduled_sync_error",
                        error = %e,
                        "Scheduled sync failed"
                    );
                }
            }
        }
    }

    async fn run_sync(&mut self) -> Result<media_sync_core::SyncResult> {
        self.orchestrator.sync().await
            .map_err(|e| color_eyre::eyre::eyre!("Sync operation failed in daemon: {}", e))
    }
}

pub async fn run_daemon(
    config: Config,
    schedule_override: Option<String>,
    no_startup_sync: bool,
    _foreground: bool,
    _output: &Output,
) -> Result<()> {
    // Load credentials first (before accessing config fields that might move)
    let path_manager = PathManager::default();
    let credentials_file = path_manager.credentials_file();
    let mut cred_store = CredentialStore::new(credentials_file.clone());
    cred_store.load()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to load credentials from {}: {}", credentials_file.display(), e))?;

    // Create factory registry and validate configurations
    let factory_registry = SourceFactoryRegistry::new();
    factory_registry.validate_all_configs(&config)
        .map_err(|e| color_eyre::eyre::eyre!("Configuration validation failed: {}", e))?;

    // Create all enabled sources using factories
    let sources = factory_registry.create_all_sources(&config, &cred_store).await
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create sources: {}", e))?;
    
    // Extract scheduler config after sources are created
    let scheduler_config_from_file = config
        .scheduler
        .as_ref()
        .ok_or_else(|| color_eyre::eyre::eyre!("Scheduler configuration not found in config.toml"))?;

    let schedule = schedule_override.unwrap_or_else(|| scheduler_config_from_file.schedule.clone());
    let run_on_startup = if no_startup_sync {
        false
    } else {
        scheduler_config_from_file.run_on_startup
    };

    let scheduler_config = media_sync_config::SchedulerConfig {
        schedule,
        timezone: scheduler_config_from_file.timezone.clone(),
        run_on_startup,
    };
    
    let orchestrator = SyncOrchestrator::new(
        sources,
        config.resolution.clone(),
    )
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create sync orchestrator: {}", e))?;

    // Create and start scheduler
    let mut scheduler = Scheduler::new(orchestrator, scheduler_config).await
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create scheduler: {}", e))?;
    scheduler.start().await
        .map_err(|e| color_eyre::eyre::eyre!("Failed to start scheduler: {}", e))?;

    Ok(())
}

