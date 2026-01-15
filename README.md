# TotalRecall

TotalRecall - Remember everything you've watched everywhere.

A Rust port of the Python IMDB-Trakt syncer, replacing Selenium with chromiumoxide for browser automation.

## Features

- **IMDB Sync**: Browser automation with chromiumoxide for CSV export and sync
- **Plex Sync**: API integration for watch history and ratings
- **Trakt-Centric**: Trakt is the central data sink, all sources sync bidirectionally
- **Internal Scheduler**: Container-based scheduling with tokio-cron-scheduler (no host cron needed)
- **Structured Logging**: JSON format for Docker containers
- **Visual Progress**: Progress bars with indicatif (disabled in non-interactive mode)

## Quick Start

### Docker Compose

1. Create `config/config.toml`:

```toml
[trakt]
client_id = "your_client_id"
client_secret = "your_client_secret"

[sources.imdb]
enabled = true
username = "your_username"

[sources.plex]
enabled = true
server_url = "http://plex.example.com:32400"

[scheduler]
schedule = "0 */6 * * *"
timezone = "UTC"
run_on_startup = true
```

2. Run with Docker Compose:

```bash
docker-compose up -d
```

## Development

```bash
cargo build
cargo run -- sync
```

## License

MIT

