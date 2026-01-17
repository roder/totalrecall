# TotalRecall

TotalRecall syncs watchlists, ratings, reviews, and watch history across multiple media services so you have one coherent view and can push changes back to each service.

## Supported Media Sources

TotalRecall supports the following media sources and data types:

| Source | Watchlist | Ratings | Reviews | Watch history |
|--------|-----------|---------|---------|---------------|
| **Trakt** | Yes | Yes | Yes | Yes |
| **Simkl** | Yes | Yes | No | Yes |
| **IMDB** | Yes | Yes | Yes | Yes |
| **Plex** | Yes | Yes | No | Yes |

### Data Objects

All data is normalized across sources:
- **Ratings**: Normalized to 1-10 scale (integer)
- **Watchlist**: Items you want to watch
- **Reviews**: Comments/reviews you've written
- **Watch history**: Items you've already watched

All items are identified by `imdb_id` and additional IDs (TMDB, TVDB, etc.) for reliable matching across services.

### Configuration Files

TotalRecall uses two configuration files:
- **`config.toml`**: Structural settings, OAuth client IDs/secrets, and sync preferences
- **`credentials.toml`**: Tokens, passwords, and sync timestamps (automatically managed)

Both files are stored in the same configuration directory (see [Configuration](#configuration) for details).

## Quick Start

### Docker Compose

TotalRecall is designed to run in Docker. The container expects a specific directory structure:

```
./totalrecall/
├── config.toml          # Configuration (create via `totalrecall config`)
├── credentials.toml     # Credentials (auto-generated)
├── data/                # Cache and application data
│   └── cache/
│       ├── collect/     # Raw data from each source
│       ├── distribute/   # Prepared data for each target
│       └── id/          # ID resolution cache
└── logs/                # Application logs
    └── totalrecall.log
```

#### First-Time Setup

Example compose.yaml:

```yaml
services:
  totalrecall:
    image: ghcr.io/roder/totalrecall:latest
    container_name: totalrecall
    restart: unless-stopped
    user: "1043:65536"
    volumes:
      - /volume1/docker/totalrecall:/app
    environment:
      - TZ=America/Los_Angeles
      - RUST_LOG=info
```

1. **Create the directory structure for mounting:**
   ```bash
   mkdir -p ./totalrecall
   # Ensure the directory is writable by the user defined in the compose.yaml
   id totalrecall
   uid=1043(totalrecall) gid=100(users) groups=100(users),65536(MediaUsers)
   # Ensure correct permissions on the local filesystem
   chown -R 1043:65536 ./totalrecall
   ```

2. **Run interactive configuration:**
   ```bash
   docker-compose run --rm totalrecall config
   ```
   
   This wizard will guide you through:
   - Setting up Trakt, Simkl, IMDB, and/or Plex
   - Configuring OAuth apps (Trakt, Simkl)
   - Setting source preferences for conflict resolution
   - Choosing which data types to sync

3. **Start the daemon:**
   ```bash
   docker-compose up -d
   ```

The daemon will:
- Run an initial sync on startup (if `scheduler.run_on_startup = true`)
- Continue syncing on a schedule (default: every 6 hours)
- Log to `./totalrecall/logs/totalrecall.log` with daily rotation

#### Environment Variables

You can customize logging via environment variables in `docker-compose.yml`:

```yaml
environment:
  - RUST_LOG=info          # Log level: error, warn, info, debug, trace
  - RUST_LOG_JSON=true     # JSON format (useful for log aggregation)
  - TOTALRECALL_BASE_PATH=/app  # Override base path (default: /app)
```

#### Health Check

The container includes a health check that verifies the daemon process is running. Check status with:

```bash
docker-compose ps
```

## Configuration

TotalRecall is configured via `config.toml` and `credentials.toml`. The recommended way to configure is using the `totalrecall config` command, which handles both files automatically.

### Using `totalrecall config`

The `config` command provides an interactive wizard and individual configuration subcommands:

```bash
# Interactive wizard (runs when no subcommand is provided, recommended for first-time setup)
totalrecall config

# Show current configuration (secrets masked)
totalrecall config show

# Show full configuration including masked secrets
totalrecall config show --full

# Configure individual services
totalrecall config trakt [--client-id ID] [--client-secret SECRET]
totalrecall config simkl [--client-id ID] [--client-secret SECRET]
totalrecall config imdb [--username USERNAME]
totalrecall config plex [--token TOKEN] [--server-url URL]

# Configure sync options
totalrecall config sync \
  [--enable-watchlist] \
  [--enable-ratings] \
  [--enable-reviews] \
  [--enable-watch-history]
```

### config.toml Reference

For advanced users who prefer to edit `config.toml` directly, here's the complete structure:

#### `[trakt]` Section

```toml
[trakt]
enabled = true
client_id = "your_trakt_client_id"
client_secret = "your_trakt_client_secret"

# Optional: Custom status mapping (advanced)
[trakt.status_mapping]
# See defaults in codebase - usually not needed
```

- **`enabled`** (bool): Enable Trakt sync
- **`client_id`** (string): Trakt OAuth app client ID (required if enabled)
- **`client_secret`** (string): Trakt OAuth app client secret (required if enabled)
- **`status_mapping`** (optional): Advanced status conversion mapping (has sensible defaults)

#### `[simkl]` Section

```toml
[simkl]
enabled = true
client_id = "your_simkl_client_id"
client_secret = "your_simkl_client_secret"

# Optional: Custom status mapping (advanced)
[simkl.status_mapping]
```

Same structure as `[trakt]` - OAuth client credentials required.

#### `[sources.imdb]` Section

```toml
[sources.imdb]
enabled = true
username = "your_imdb_username"

# Optional: Custom status mapping (advanced)
[sources.imdb.status_mapping]
```

- **`enabled`** (bool): Enable IMDB sync
- **`username`** (string): Your IMDB username (required if enabled)
- **Password**: Stored in `credentials.toml` (set via `totalrecall config imdb`)

**Note**: IMDB requires browser automation (Chromium). Ensure the container has access to Chromium.

#### `[sources.plex]` Section

```toml
[sources.plex]
enabled = true
server_url = "http://your-plex-server:32400"

# Optional: Custom status mapping (advanced)
[sources.plex.status_mapping]
```

- **`enabled`** (bool): Enable Plex sync
- **`server_url`** (string): Plex Media Server URL (e.g. `http://192.168.1.100:32400`)
  - **If empty**: TotalRecall will use Plex "discover" API to automatically find your server
  - **If set**: Direct connection to the specified server
- **Token**: Stored in `credentials.toml` (set via `totalrecall config plex`)

**Note**: Ratings and watch history require a Plex server. Watchlist can work with Plex Discover (cloud) only.

#### `[resolution]` Section

```toml
[resolution]
strategy = "Preference"  # Options: Newest, Oldest, Preference, Merge
source_preference = ["trakt", "imdb", "plex", "simkl"]  # REQUIRED: Ordered priority list
timestamp_tolerance_seconds = 3600  # Default: 1 hour

# Optional: Override strategy for specific data types
ratings_strategy = "Preference"  # Optional
watchlist_strategy = "Preference"  # Optional
```

- **`strategy`** (enum): How to resolve conflicts when the same item exists in multiple sources
  - **`Newest`**: Use the most recently updated item
  - **`Oldest`**: Use the oldest item
  - **`Preference`**: Use the item from the highest-priority source in `source_preference`
  - **`Merge`**: Combine data from all sources (for ratings: average; for watchlist: union)
- **`source_preference`** (array of strings): **REQUIRED** - Ordered list of source names for conflict resolution. Each source must be enabled and configured. Example: `["trakt", "imdb", "plex", "simkl"]` means Trakt takes priority over IMDB, which takes priority over Plex, etc.
- **`timestamp_tolerance_seconds`** (int64, default 3600): When comparing timestamps, items within this window are considered "equal" for resolution purposes
- **`ratings_strategy`**, **`watchlist_strategy`** (optional): Override the global `strategy` for specific data types

#### `[sync]` Section

```toml
[sync]
sync_watchlist = true
sync_ratings = true
sync_reviews = true
sync_watch_history = true
remove_watched_from_watchlists = false
mark_rated_as_watched = false
remove_watchlist_items_older_than_days = null  # Optional: Remove items older than N days
```

- **`sync_watchlist`**, **`sync_ratings`**, **`sync_reviews`**, **`sync_watch_history`** (bool, default true): Enable/disable syncing each data type
- **`remove_watched_from_watchlists`** (bool, default false): Automatically remove items from watchlists once they appear in watch history
- **`mark_rated_as_watched`** (bool, default false): Automatically add rated items to watch history
- **`remove_watchlist_items_older_than_days`** (optional u32): Remove watchlist items older than N days (useful for cleanup)

#### `[scheduler]` Section

```toml
[scheduler]
schedule = "0 */6 * * *"  # Cron expression: every 6 hours
timezone = "UTC"  # Timezone for cron schedule
run_on_startup = true  # Run sync immediately when daemon starts
```

- **`schedule`** (string, default `"0 */6 * * *"`): Cron expression for automatic syncing
- **`timezone`** (string, default `"UTC"` or `$TZ` env var): Timezone for the cron schedule
- **`run_on_startup`** (bool, default true): Run a full sync when the daemon starts

### credentials.toml

This file is automatically managed by TotalRecall. You should not edit it manually. It contains:

- **OAuth tokens**: `trakt_access_token`, `trakt_refresh_token`, `simkl_access_token`, `simkl_refresh_token`
- **Passwords**: `imdb_password`, `plex_token`
- **Sync timestamps**: `last_sync_timestamp_<source>_<data_type>` (used for incremental sync)
- **Other**: `imdb_reviews_last_submitted` (tracks review submission to avoid duplicates)

All credentials are set/updated by `totalrecall config` commands and the sync process.

## How TotalRecall Works

TotalRecall uses a three-phase pipeline: **Collect**, **Resolve**, and **Distribute**. Each phase has its own caching strategy.

### Pipeline Overview

```
[Authenticate] → [Collect] → [Resolve] → [Distribute]
                     ↓            ↓            ↓
                  (cache)    (ID resolver)  (distribute cache / dry-run)
```

### Phase 1: Collect

The collect phase fetches raw data from all configured sources.

**Process:**
1. For each source in `resolution.source_preference`:
   - Authenticate to the source
   - For each enabled data type (watchlist, ratings, reviews, watch_history):
     - If `--use-cache` is specified for this source: Load from collect cache (skip API call)
     - Otherwise: Call the source API (`get_watchlist`, `get_ratings`, etc.) and save the result to collect cache

**Collect Cache:**
- **Location**: `data/cache/collect/{source}/{data_type}.json`
- **Examples**: 
  - `data/cache/collect/plex/ratings.json`
  - `data/cache/collect/imdb/watchlist.json`
  - `data/cache/collect/trakt/watch_history.json`
- **Purpose**: Persist the "last raw fetch" from each source. Used by `--use-cache` to test resolve/distribute without hitting APIs.

**ID Resolution:**
During collect, the `IdResolver` (backed by `data/cache/id/`) resolves missing IDs. For example, if an item has a TMDB ID but no IMDB ID, it will look up the IMDB ID and cache the mapping. This ensures reliable matching across sources.

### Phase 2: Resolve

The resolve phase merges data from all sources into a single coherent dataset.

**Process:**
1. **Normalize ratings**: Convert all ratings to 1-10 scale using each source's rating normalizer
2. **Resolve conflicts**: Use the configured `resolution.strategy` to handle duplicates:
   - **Preference**: Use data from the highest-priority source in `source_preference`
   - **Newest**: Use the most recently updated item
   - **Oldest**: Use the oldest item
   - **Merge**: Combine data (e.g., average ratings, union of watchlists)
3. **Apply post-resolution rules** (if enabled):
   - `mark_rated_as_watched`: Add rated items to watch history
   - `remove_watched_from_watchlists`: Remove watched items from watchlists
   - `remove_watchlist_items_older_than_days`: Clean up old watchlist items

**Output**: Single `ResolvedData` structure with one list per data type (watchlist, ratings, reviews, watch_history).

**ID Resolver Cache:**
- **Location**: `data/cache/id/` (e.g. `id_mappings.bin`)
- **Purpose**: Cache ID mappings (IMDB ↔ TMDB ↔ TVDB, etc.) to avoid repeated lookups
- **Saved**: After resolve phase and during distribute phase

### Phase 3: Distribute

The distribute phase prepares and sends the resolved data to each target source.

**Process:**
For each target source, a **distribution strategy** prepares the data:

1. **Filtering**:
   - **Incremental sync**: Only send items newer than `last_sync_timestamp_<source>_<data_type>` (unless `--force-full-sync`)
   - **Deduplication**: Skip items that already exist in the target
   - **Source-specific rules**: 
     - Plex: Exclude Discover-only items (they can't be rated on local servers)
     - Some sources: Split watchlist by status (e.g., "watching" → watch history, "plan to watch" → watchlist)

2. **Transformation**:
   - **Status mapping**: Convert normalized statuses to source-native statuses
   - **Rating normalization**: Convert 1-10 scale to source-native scale (if needed)
   - **ID conversion**: Use source-specific IDs (e.g., Plex `rating_key`)

3. **Distribution**:
   - Call source APIs: `add_to_watchlist`, `remove_from_watchlist`, `set_ratings`, `set_reviews`, `add_watch_history`
   - Update `last_sync_timestamp_<source>_<data_type>` after successful sync

**Distribute Cache:**
- **Location**: `data/cache/distribute/{source}/{data_type}.json`
- **Examples**:
  - `data/cache/distribute/plex/excluded.json` (items filtered out)
  - `data/cache/distribute/imdb/ratings.json` (prepared ratings for IMDB)
- **Purpose**: 
  - **Normal run**: Audit trail of what was sent to each source
  - **`--dry-run`**: Preview what would be synced without making changes

### Caching Summary

| Phase | Cache Location | Written When | Read When |
|-------|---------------|--------------|-----------|
| **Collect** | `data/cache/collect/{source}/{data_type}.json` | After API fetch (unless `--use-cache`) | `--use-cache` for that source |
| **ID Resolve** | `data/cache/id/` (e.g. `id_mappings.bin`) | After resolve and during distribute | During collect/resolve/distribute for ID lookups |
| **Distribute** | `data/cache/distribute/{source}/{data_type}.json` | During distribute (excluded items, etc.) and `--dry-run` | Not used by sync (for inspection/debugging) |
| **Other** | `data/cache/csv/{source}/` (IMDB CSV exports) | After IMDB collect | By IMDB source or external tools |

**Important**: On a normal sync (without `--use-cache`), the collect phase **overwrites** the collect cache with the latest API response. The cache is not re-read in the same sync; it's the persistence of "last raw fetch." With `--use-cache`, the collect step **skips** the API and **reads** from the collect cache instead.

## Logging and Manual Operations

### Logging

TotalRecall provides structured logging with multiple verbosity levels.

#### Log Locations

- **Daemon mode (background, non-container)**: `{log_dir}/totalrecall.log`
  - Example: `~/.config/totalrecall/logs/totalrecall.log` (native) or `./totalrecall/logs/totalrecall.log` (Docker)
  - **Daily rotation**: Older logs are named `totalrecall.2026-01-17`, etc.
- **Foreground mode / `sync` / `config` / `clear`**: Logs to stderr only (no file)

#### Log Levels

Control verbosity via `RUST_LOG` environment variable or `-v` flags:

- **Default** (no flags): `info` level
- **`-v`**: `debug` level (suppresses noisy HTTP logs)
- **`-vv` or higher**: `trace` level (includes all logs, including HTTP)
- **`-q`**: `error` level only

**Examples:**
```bash
# Debug-level logging
RUST_LOG=debug totalrecall sync --ratings

# Or use -v flag
totalrecall sync --ratings -v

# Trace-level logging (maximum detail)
RUST_LOG=trace totalrecall sync --ratings
# Or
totalrecall sync --ratings -vv
```

#### Log Format

- **Default**: Human-readable format with timestamps
- **JSON format**: Set `RUST_LOG_JSON=true` for structured JSON logs (useful for log aggregation)
  - Docker Compose sets this by default: `RUST_LOG_JSON=true`

**In Docker**, add to `docker-compose.yml`:
```yaml
environment:
  - RUST_LOG=debug  # or trace for maximum detail
  - RUST_LOG_JSON=true
```

### Manual Sync (No Daemon)

Run one-off syncs without starting the daemon:

```bash
# Sync all enabled data types (uses config.sync settings)
totalrecall sync

# Sync specific data types only
totalrecall sync --ratings
totalrecall sync --watchlist --ratings
totalrecall sync --reviews
totalrecall sync --watch_history

# Sync all (use config defaults, same as no flags)
totalrecall sync --all

# Preview what would be synced (no actual changes)
totalrecall sync --dry-run
totalrecall sync --dry-run=plex,imdb  # Specific sources only

# Use cached data instead of fetching from APIs
totalrecall sync --use-cache
totalrecall sync --use-cache=imdb,trakt,simkl  # Specific sources only

# Force full sync (ignore incremental sync timestamps)
totalrecall sync --force-full-sync
```

**Flag combinations:**
- `--dry-run`: Writes prepared data to `data/cache/distribute/{source}/` without making API calls
- `--use-cache`: Uses collect cache instead of calling source APIs (useful for testing resolve/distribute)
- `--force-full-sync`: Ignores `last_sync_timestamp_*` and sends all data (useful after clearing timestamps)

### Daemon Mode

The daemon runs scheduled syncs automatically.

#### Starting the Daemon

```bash
# Start daemon (runs in background on native, foreground in containers)
totalrecall start

# Start with custom schedule
totalrecall start --schedule "0 */6 * * *"

# Skip initial sync on startup
totalrecall start --no-startup-sync

# Run in foreground (don't daemonize)
totalrecall start --foreground
```

**Behavior:**
- **In containers**: Always runs in foreground (keeps container alive)
- **On native systems**: Daemonizes by default (runs in background, logs to file)
- **Startup sync**: Runs a full sync immediately if `scheduler.run_on_startup = true`
- **Scheduled syncs**: Uses `scheduler.schedule` cron expression (default: every 6 hours)

#### Stopping the Daemon

```bash
totalrecall stop
```

This sends `SIGTERM` (graceful shutdown) to the daemon process. If that fails, it sends `SIGKILL`. Works on Unix-like systems only.

#### Triggering Manual Sync While Daemon is Running

The daemon only runs syncs on its schedule and (optionally) at startup. To trigger a one-off sync while the daemon is running:

**In Docker:**
```bash
# Run a separate sync command
docker-compose run --rm totalrecall sync --ratings

# Or exec into the running container
docker-compose exec totalrecall totalrecall sync --ratings
```

**On native systems:**
```bash
# Run sync command (daemon continues running)
totalrecall sync --ratings
```

Both processes share the same `config.toml` and `credentials.toml`, so they use the same configuration.

## Troubleshooting

### Authentication Failures

**Symptoms**: Errors like "Failed to authenticate to trakt" or "Plex token not found"

**Solutions:**
1. Verify credentials: `totalrecall config show`
2. Re-authenticate:
   - Trakt/Simkl: `totalrecall config trakt` or `totalrecall config simkl` (OAuth flow)
   - IMDB: `totalrecall config imdb` (re-enter password)
   - Plex: `totalrecall config plex` (re-enter token)
3. Ensure `credentials.toml` exists and is readable

### Source Configuration Errors

**Symptoms**: "Source X is in source_preference but is not configured"

**Solutions:**
1. Enable and fully configure the source: `totalrecall config trakt` (or simkl/imdb/plex)
2. Or remove it from `resolution.source_preference` in `config.toml`
3. Verify with: `totalrecall config show`

### Plex Issues

**Symptoms**: "No server found" or empty ratings/watch history

**Solutions:**
1. **Server URL**: 
   - If `server_url` is set: Ensure it's reachable from the TotalRecall process (test: `curl http://your-server:32400`)
   - If `server_url` is empty: Ensure Plex Discover can find your server (check Plex.tv account)
2. **Token**: Verify Plex token has access to the server: `totalrecall config plex`
3. **Ratings/History**: These require a local Plex server (not just Plex Discover). Watchlist can work with Discover only.

### IMDB Issues

**Symptoms**: "Browser not initialized" or "Failed to create new page"

**Solutions:**
1. **Chromium**: Ensure Chromium is installed and accessible (Docker image includes it)
2. **Browser health**: Check logs for "Browser health check failed"
3. **Password**: Verify `imdb_password` is set: `totalrecall config imdb`
4. **Permissions**: Ensure the container/user can run Chromium (may need `--no-sandbox` in some environments)

### Cache Issues

**Symptoms**: `--use-cache` returns empty data or "Cache miss"

**Solutions:**
1. **Populate cache**: Run a normal sync first (without `--use-cache`) to populate collect cache
2. **Corrupt cache**: Clear and repopulate:
   ```bash
   totalrecall clear --cache
   totalrecall sync  # Repopulate
   ```
3. **Cache location**: Verify `data/cache/collect/{source}/` exists and contains JSON files

### Sync Not Running

**Symptoms**: Daemon is running but syncs aren't happening

**Solutions:**
1. **Check logs**: `tail -f ./totalrecall/logs/totalrecall.log` (or `docker-compose logs -f`)
2. **Verify schedule**: Check `scheduler.schedule` in `config.toml`
3. **Startup sync**: If `run_on_startup = false`, first sync waits for schedule
4. **Manual test**: Run `totalrecall sync` manually to see errors

### Clearing Data

Use `totalrecall clear` to reset various caches:

```bash
# Clear all caches and credentials
totalrecall clear --all

# Clear only application cache (collect, distribute, id)
totalrecall clear --cache

# Clear only credentials (forces re-authentication)
totalrecall clear --credentials

# Clear sync timestamps (forces full sync on next run)
totalrecall clear --timestamps
```

## License

MIT
