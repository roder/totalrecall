use crate::output::Output;
use crate::commands;
use color_eyre::Result;
use media_sync_config::{Config, CredentialStore, PathManager};
use media_sync_core::SyncOrchestrator;
use media_sync_sources::SourceFactoryRegistry;
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
            // Force full sync on startup
            self.orchestrator.set_force_full_sync(true);
            self.run_sync().await?;
            // Reset to incremental sync for scheduled runs
            self.orchestrator.set_force_full_sync(false);
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

#[cfg(unix)]
fn daemonize() -> Result<()> {
    use nix::unistd::{fork, ForkResult, setsid};
    use std::fs::File;
    use std::os::unix::io::AsRawFd;
    
    // First fork
    match unsafe { fork()? } {
        ForkResult::Parent { child: _ } => {
            // Parent exits immediately
            std::process::exit(0);
        }
        ForkResult::Child => {
            // Child continues
        }
    }
    
    // Create a new session (detach from controlling terminal)
    setsid()?;
    
    // Second fork to ensure we're not a session leader
    match unsafe { fork()? } {
        ForkResult::Parent { child: _ } => {
            // Second parent exits
            std::process::exit(0);
        }
        ForkResult::Child => {
            // Final daemon process continues
        }
    }
    
    // Change to root directory to avoid keeping mount points busy
    std::env::set_current_dir("/")?;
    
    // Close and redirect standard file descriptors
    let dev_null = File::open("/dev/null")?;
    let null_fd = dev_null.as_raw_fd();
    
    unsafe {
        // Redirect stdin, stdout, stderr to /dev/null
        libc::dup2(null_fd, libc::STDIN_FILENO);
        libc::dup2(null_fd, libc::STDOUT_FILENO);
        libc::dup2(null_fd, libc::STDERR_FILENO);
    }
    
    Ok(())
}

#[cfg(not(unix))]
fn daemonize() -> Result<()> {
    // Daemonization not supported on non-Unix systems
    // On Windows, services should be used instead
    Err(color_eyre::eyre::eyre!("Daemonization is only supported on Unix-like systems"))
}

// Helper function to detect if we're running in a container
fn is_container() -> bool {
    use media_sync_config::container_base_path;
    
    // Check for Docker/Podman indicators
    std::path::Path::new("/.dockerenv").exists() ||
    container_base_path().exists() ||  // Our container uses configurable base path
    std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .map(|s| s.contains("docker") || s.contains("containerd") || s.contains("podman"))
        .unwrap_or(false)
}

pub async fn run_start(
    schedule_override: Option<String>,
    no_startup_sync: bool,
    foreground: bool,
    output: &Output,
) -> Result<()> {
    let path_manager = PathManager::default();
    let config_file = path_manager.config_file();
    
    // Check if config exists - if not, run interactive config in foreground
    let config = if !config_file.exists() {
        output.info("Configuration file not found. Running interactive configuration setup...");
        output.println("");
        
        // Run interactive config (this will run in foreground)
        commands::config::run_interactive_config(output).await?;
        
        output.println("");
        output.info("Configuration setup complete!");
        
        // Load the newly created config
        Config::load_from_file(&config_file)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to load config from {}: {}", config_file.display(), e))?
    } else {
        // Load existing config (prompt for source_preference if missing)
        commands::config::load_config_or_prompt_source_preference(output)?
    };
    
    // In containers, always run in foreground to keep the container alive
    // Only daemonize if explicitly not in a container and not in foreground mode
    let should_daemonize = !foreground && !is_container();
    
    if should_daemonize {
        output.println("");
        output.info("Starting daemon in background mode...");
        output.println("");
        
        // Daemonize if not running in foreground mode and not in container
        #[cfg(unix)]
        {
            daemonize()?;
        }
        
        let log_file = path_manager.daemon_log_file();
        info!("Daemon running in background mode. Logs are being written to: {}", log_file.display());
    } else if is_container() && !foreground {
        // In container but not foreground - inform user we're running in foreground for container compatibility
        output.info("Running in foreground mode (container detected - daemonization disabled)");
        output.println("");
    }
    
    // Now run the daemon (will run in foreground if in container or foreground flag is set)
    run_daemon_internal(config, schedule_override, no_startup_sync, foreground || is_container(), output).await
}

async fn run_daemon_internal(
    config: Config,
    schedule_override: Option<String>,
    no_startup_sync: bool,
    foreground: bool,
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
    
    // Extract scheduler config after sources are created, or use defaults
    let default_scheduler_config = media_sync_config::default_scheduler_config();
    
    let scheduler_config_from_file = config.scheduler.as_ref().unwrap_or(&default_scheduler_config);

    let schedule = schedule_override.unwrap_or_else(|| scheduler_config_from_file.schedule.clone());
    let run_on_startup = if no_startup_sync {
        false
    } else {
        scheduler_config_from_file.run_on_startup
    };

    // Use timezone from config, but allow TZ environment variable to override
    let timezone = std::env::var("TZ")
        .unwrap_or_else(|_| scheduler_config_from_file.timezone.clone());

    let scheduler_config = media_sync_config::SchedulerConfig {
        schedule,
        timezone,
        run_on_startup,
    };
    
    // Create sync options from config (same as manual sync command)
    // Default to incremental syncs (force_full_sync=false), will be set to true for startup sync if needed
    let sync_options = media_sync_core::SyncOptions {
        sync_watchlist: config.sync.sync_watchlist,
        sync_ratings: config.sync.sync_ratings,
        sync_reviews: config.sync.sync_reviews,
        sync_watch_history: config.sync.sync_watch_history,
        force_full_sync: false, // Will be set to true for startup sync, false for scheduled syncs
    };
    
    let orchestrator = SyncOrchestrator::new(
        sources,
        config.resolution.clone(),
    )
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create sync orchestrator: {}", e))?
        .with_sync_options(sync_options)
        .with_config_sync_options(config.sync.clone());

    // Create and start scheduler
    let mut scheduler = Scheduler::new(orchestrator, scheduler_config).await
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create scheduler: {}", e))?;
    scheduler.start().await
        .map_err(|e| color_eyre::eyre::eyre!("Failed to start scheduler: {}", e))?;

    Ok(())
}

pub async fn run_stop(output: &Output) -> Result<()> {
    #[cfg(unix)]
    {
        use std::process::Command;
        
        // Find the daemon process
        let output_cmd = Command::new("pgrep")
            .arg("-f")
            .arg("totalrecall start")
            .output()?;
        
        if !output_cmd.status.success() {
            output.warn("No running daemon process found.");
            return Ok(());
        }
        
        let pid_str = String::from_utf8(output_cmd.stdout)?;
        let pids: Vec<&str> = pid_str.trim().lines().collect();
        
        if pids.is_empty() {
            output.warn("No running daemon process found.");
            return Ok(());
        }
        
        // Kill all matching processes
        for pid in &pids {
            let pid_num = pid.trim().parse::<i32>()
                .map_err(|e| color_eyre::eyre::eyre!("Invalid PID {}: {}", pid, e))?;
            
            // Try SIGTERM first (graceful shutdown)
            let kill_result = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .output()?;
            
            if kill_result.status.success() {
                output.info(&format!("Sent SIGTERM to daemon process (PID: {})", pid_num));
            } else {
                // If SIGTERM fails, try SIGKILL
                let kill_result = Command::new("kill")
                    .arg("-KILL")
                    .arg(pid.to_string())
                    .output()?;
                
                if kill_result.status.success() {
                    output.info(&format!("Sent SIGKILL to daemon process (PID: {})", pid_num));
                } else {
                    output.warn(&format!("Failed to kill process (PID: {})", pid_num));
                }
            }
        }
        
        // Wait a moment and verify processes are gone
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        
        let verify_output = Command::new("pgrep")
            .arg("-f")
            .arg("totalrecall start")
            .output()?;
        
        if verify_output.status.success() {
            output.warn("Some daemon processes may still be running. You may need to kill them manually.");
        } else {
            output.info("Daemon stopped successfully.");
        }
    }
    
    #[cfg(not(unix))]
    {
        output.warn("Stop command is only supported on Unix-like systems.");
    }
    
    Ok(())
}

