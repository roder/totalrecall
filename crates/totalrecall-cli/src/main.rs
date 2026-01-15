use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use color_eyre::eyre::Context;
use commands::{clear, config, daemon, sync};

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
    /// Sync data between sources (one-time sync)
    #[command(long_about = "Synchronize watchlists, ratings, reviews, and watch history between configured sources. If no flags are specified, syncs all enabled data types from configuration.")]
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
    /// Run as daemon with internal scheduler
    #[command(long_about = "Run TotalRecall as a background daemon that periodically syncs data according to the configured schedule. The daemon will perform an initial sync on startup unless --no-startup-sync is specified.")]
    Daemon {
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
    /// Configure credentials and settings
    #[command(long_about = "Manage configuration and credentials for TotalRecall. Use subcommands to view or modify settings for Trakt, IMDB, and sync options. Running without a subcommand starts the interactive configuration wizard.")]
    Config {
        #[command(subcommand)]
        cmd: Option<ConfigCommands>,
    },
    /// Clear cached data
    #[command(long_about = "Clear cached data or stored credentials. Use --cache to clear application cache, --credentials to clear stored credentials, --timestamps to clear sync timestamps, or --all to clear everything.")]
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
    #[command(long_about = "Display the current configuration. Sensitive data like passwords and tokens are masked. Use --full to show masked values.")]
    Show {
        /// Show full configuration including masked secrets
        #[arg(long, action = ArgAction::SetTrue)]
        full: bool,
    },

    /// Configure Trakt (OAuth flow)
    #[command(long_about = "Configure Trakt API credentials and perform OAuth authentication. You'll need to create a Trakt API application at https://trakt.tv/oauth/applications first.")]
    Trakt {
        /// Trakt Client ID (if not provided, will prompt)
        #[arg(long)]
        client_id: Option<String>,

        /// Trakt Client Secret (if not provided, will prompt)
        #[arg(long)]
        client_secret: Option<String>,
    },

    /// Configure IMDB credentials
    #[command(long_about = "Configure IMDB username and password. Credentials are stored securely in the credentials file.")]
    Imdb {
        /// IMDB Username (if not provided, will prompt)
        #[arg(long)]
        username: Option<String>,
    },

    /// Configure Simkl (OAuth flow)
    #[command(long_about = "Configure Simkl API credentials and perform OAuth authentication. You'll need to create a Simkl API application at https://simkl.com/oauth/applications first.")]
    Simkl {
        /// Simkl Client ID (if not provided, will prompt)
        #[arg(long)]
        client_id: Option<String>,

        /// Simkl Client Secret (if not provided, will prompt)
        #[arg(long)]
        client_secret: Option<String>,
    },

    /// Configure Plex (token-based authentication)
    #[command(long_about = "Configure Plex API token for MyPlex cloud access. You can find your Plex token in your account settings or by inspecting network requests in Plex Web.")]
    Plex {
        /// Plex API Token (if not provided, will prompt)
        #[arg(long)]
        token: Option<String>,

        /// Plex Server URL (optional, for direct server access)
        #[arg(long)]
        server_url: Option<String>,
    },

    /// Interactive configuration wizard
    #[command(long_about = "Run an interactive configuration wizard that guides you through setting up all services and preferences.")]
    Interactive,

    /// Configure sync options
    #[command(long_about = "Configure which data types to sync and sync behavior options like removing watched items from watchlists.")]
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
    
    // Initialize logging with verbose level
    logging::init_logging(cli.verbose, cli.quiet).map_err(|e| color_eyre::eyre::eyre!("{}", e))?;

    // Create output handler
    let output = output::Output::new(cli.output, cli.quiet);

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
        Commands::Daemon {
            schedule,
            no_startup_sync,
            foreground,
        } => {
            // Load config (prompt for source_preference if missing)
            let config = commands::config::load_config_or_prompt_source_preference(&output)?;
            daemon::run_daemon(config, schedule, no_startup_sync, foreground, &output).await
        }
        Commands::Config { cmd } => {
            let cmd = cmd.unwrap_or(ConfigCommands::Interactive);
            config::run_config(cmd, &output).await
        },
        Commands::Clear { all, cache, credentials, timestamps } => clear::run_clear(all, cache, credentials, timestamps, &output).await,
    }
}

