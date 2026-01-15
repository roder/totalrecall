use anyhow::Result;
use std::io;
use std::io::IsTerminal;
use tracing_subscriber::{
    layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry,
};
use tracing_subscriber::fmt::{self, time::ChronoUtc};

pub fn init_logging(verbose_level: u8, quiet: bool) -> Result<()> {
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

    Ok(())
}

