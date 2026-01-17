use anyhow::Result;
use std::io;
use std::io::IsTerminal;
use std::path::PathBuf;
use tracing_subscriber::{
    layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry,
};
use tracing_subscriber::fmt::{self, time::ChronoUtc};

pub fn init_logging(verbose_level: u8, quiet: bool) -> Result<()> {
    init_logging_with_file(verbose_level, quiet, None)
}

pub fn init_logging_with_file(verbose_level: u8, quiet: bool, log_file: Option<PathBuf>) -> Result<()> {
    // Determine log level from verbose count
    // 0 = info, 1 = debug (with hyper::proto::h1 suppressed), 2+ = trace (all logs)
    let filter = if quiet {
        // In quiet mode, only show errors
        EnvFilter::new("error")
    } else if verbose_level > 0 {
        // Use verbose level or RUST_LOG environment variable
        let filter_str = match verbose_level {
            1 => {
                // -v: debug level but suppress noisy hyper logs
                "debug,hyper::proto::h1=warn,hyper::client::pool=warn"
            },
            _ => {
                // -vv and above: trace level (includes everything, including hyper logs)
                "trace"
            },
        };
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(filter_str))
    } else {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info"))
    };

    let json = std::env::var("RUST_LOG_JSON")
        .map(|v| v == "true")
        .unwrap_or_else(|_| !io::stdout().is_terminal());

    let registry = Registry::default().with(filter);

    // If log file is provided, write to file; otherwise write to stderr
    if let Some(log_path) = log_file {
        // Ensure log directory exists
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open log file in append mode
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        if json {
            let json_layer = fmt::layer()
                .json()
                .with_timer(ChronoUtc::rfc_3339())
                .with_writer(file);

            registry.with(json_layer).init();
        } else {
            let fmt_layer = fmt::layer()
                .with_timer(ChronoUtc::rfc_3339())
                .with_ansi(false)  // Disable ANSI codes when writing to file
                .with_writer(file);

            registry.with(fmt_layer).init();
        }
    } else {
        if json {
            let json_layer = fmt::layer()
                .json()
                .with_timer(ChronoUtc::rfc_3339())
                .with_writer(io::stderr);

            registry.with(json_layer).init();
        } else {
            let fmt_layer = fmt::layer()
                .with_timer(ChronoUtc::rfc_3339())
                .with_writer(io::stderr);

            registry.with(fmt_layer).init();
        }
    }

    Ok(())
}

