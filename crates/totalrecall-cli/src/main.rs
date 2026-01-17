use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use color_eyre::eyre::Context;
use commands::{clear, config, daemon as start, sync};

mod commands;
mod logging;
mod output;

#[derive(Parser)]
#[command(name = "totalrecall")]
#[command(about = "TotalRecall - Remember everything you've watched everywhere")]
#[command(version)]
struct Cli {
    /// Enable verbose output (use multiple times for more verbosity: -v, -vv, -vvv)
    #[arg(short, long, action = ArgAction::Count, global = true)]
    verbose: u8,

    /// Suppress all output except errors
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Output format
    #[arg(long, global = true, default_value = "human", value_enum)]
    output: output::OutputFormat,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync data between sources
    Sync {
        /// Sync watchlist items
        #[arg(long, action = ArgAction::SetTrue)]
        watchlist: bool,

        /// Sync ratings
        #[arg(long, action = ArgAction::SetTrue)]
        ratings: bool,

        /// Sync reviews/comments
        #[arg(long, action = ArgAction::SetTrue)]
        reviews: bool,

        /// Sync watch history
        #[arg(long, action = ArgAction::SetTrue)]
        watch_history: bool,

        /// Force a full sync, ignoring saved timestamps
        #[arg(long, action = ArgAction::SetTrue)]
        force_full_sync: bool,

        /// Dry-run mode: preview what would be synced without making changes.
        /// Writes JSON files per-source with prepared data after distribution strategy.
        /// Defaults to all sources if no list provided: --dry-run=plex,imdb
        #[arg(long, value_name = "SOURCES", num_args = 0..=1, default_missing_value = "all")]
        dry_run: Option<String>,

        /// Sync all enabled data types (conflicts with individual flags)
        #[arg(long, action = ArgAction::SetTrue, conflicts_with_all = ["watchlist", "ratings", "reviews", "watch_history"])]
        all: bool,

        /// Use cached source data instead of fetching fresh data (for testing Resolve/Distribute pipeline).
        /// Defaults to all configured sources. Can specify comma-separated list: --use-cache=imdb,trakt,simkl
        #[arg(long, value_name = "SOURCES", num_args = 0..=1, default_missing_value = "all")]
        use_cache: Option<String>,
    },
    /// Start the daemon with internal scheduler
    Start {
        /// Cron schedule expression (e.g., '0 */6 * * *' for every 6 hours)
        #[arg(long, value_name = "SCHEDULE")]
        schedule: Option<String>,

        /// Skip initial sync on startup
        #[arg(long, action = ArgAction::SetTrue)]
        no_startup_sync: bool,

        /// Run in foreground (don't daemonize)
        #[arg(long, action = ArgAction::SetTrue)]
        foreground: bool,
    },
    /// Stop the running daemon
    Stop,
    /// Configure credentials and settings
    Config {
        #[command(subcommand)]
        cmd: Option<ConfigCommands>,
    },
    /// Clear cached data
    Clear {
        /// Clear all cache and credentials
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "credentials")]
        all: bool,

        /// Clear application cache
        #[arg(long, action = ArgAction::SetTrue)]
        cache: bool,

        /// Clear stored credentials
        #[arg(long, action = ArgAction::SetTrue)]
        credentials: bool,

        /// Clear sync timestamps (forces full sync on next run)
        #[arg(long, action = ArgAction::SetTrue)]
        timestamps: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Show current configuration (masks sensitive data)
    Show {
        /// Show full configuration including masked secrets
        #[arg(long, action = ArgAction::SetTrue)]
        full: bool,
    },

    /// Configure Trakt (OAuth flow)
    Trakt {
        /// Trakt Client ID (if not provided, will prompt)
        #[arg(long)]
        client_id: Option<String>,

        /// Trakt Client Secret (if not provided, will prompt)
        #[arg(long)]
        client_secret: Option<String>,
    },

    /// Configure IMDB credentials
    Imdb {
        /// IMDB Username (if not provided, will prompt)
        #[arg(long)]
        username: Option<String>,
    },

    /// Configure Simkl (OAuth flow)
    Simkl {
        /// Simkl Client ID (if not provided, will prompt)
        #[arg(long)]
        client_id: Option<String>,

        /// Simkl Client Secret (if not provided, will prompt)
        #[arg(long)]
        client_secret: Option<String>,
    },

    /// Configure Plex (token-based authentication)
    Plex {
        /// Plex API Token (if not provided, will prompt)
        #[arg(long)]
        token: Option<String>,

        /// Plex Server URL (optional, for direct server access)
        #[arg(long)]
        server_url: Option<String>,
    },

    /// Interactive configuration wizard
    Interactive,

    /// Configure sync options
    Sync {
        /// Enable watchlist syncing
        #[arg(long)]
        enable_watchlist: Option<bool>,

        /// Enable ratings syncing
        #[arg(long)]
        enable_ratings: Option<bool>,

        /// Enable reviews syncing
        #[arg(long)]
        enable_reviews: Option<bool>,

        /// Enable watch history syncing
        #[arg(long)]
        enable_watch_history: Option<bool>,
    },
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    
    let cli = Cli::parse();
    
    // Create output handler
    let output = output::Output::new(cli.output, cli.quiet);

    // Determine if we need file logging (daemon mode, not foreground)
    let log_file = match &cli.command {
        Commands::Start { foreground: false, .. } => {
            let path_manager = media_sync_config::PathManager::default();
            Some(path_manager.daemon_log_file())
        }
        _ => None,
    };

    // Initialize logging (with file if daemon mode, otherwise stderr)
    logging::init_logging_with_file(cli.verbose, cli.quiet, log_file)
        .map_err(|e| color_eyre::eyre::eyre!("{}", e))?;

    match cli.command {
        Commands::Sync {
            watchlist,
            ratings,
            reviews,
            watch_history,
            dry_run,
            all,
            use_cache,
            force_full_sync,
        } => {
            sync::run_sync(watchlist, ratings, reviews, watch_history, dry_run, all, use_cache, force_full_sync, &output).await
        }
        Commands::Start {
            schedule,
            no_startup_sync,
            foreground,
        } => {
            start::run_start(schedule, no_startup_sync, foreground, &output).await
        }
        Commands::Stop => {
            start::run_stop(&output).await
        }
        Commands::Config { cmd } => {
            let cmd = cmd.unwrap_or(ConfigCommands::Interactive);
            config::run_config(cmd, &output).await
        },
        Commands::Clear { all, cache, credentials, timestamps } => clear::run_clear(all, cache, credentials, timestamps, &output).await,
    }
}

