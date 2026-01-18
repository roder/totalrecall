#!/bin/sh
set -e

# Get container base path from environment variable, defaulting to /app
BASE_PATH="${TOTALRECALL_BASE_PATH:-/app}"
CONFIG_FILE="${BASE_PATH}/config.toml"

# Debug: Log what we're doing
echo "[ENTRYPOINT] Starting with command: $@"
echo "[ENTRYPOINT] Config file path: $CONFIG_FILE"
echo "[ENTRYPOINT] Config file exists: $([ -f "$CONFIG_FILE" ] && echo "yes" || echo "no")"

# Verify binary exists and is executable
if [ ! -f "/usr/local/bin/totalrecall" ]; then
    echo "[ENTRYPOINT] ERROR: Binary not found at /usr/local/bin/totalrecall"
    exit 1
fi

if [ ! -x "/usr/local/bin/totalrecall" ]; then
    echo "[ENTRYPOINT] ERROR: Binary is not executable"
    exit 1
fi

# Debug: Show binary info
echo "[ENTRYPOINT] Binary exists and is executable"
/usr/local/bin/totalrecall --version 2>&1 || echo "[ENTRYPOINT] WARNING: Could not get version"

# For commands other than "start", check if config exists and run interactive setup if needed
# The "start" command handles its own config setup and won't background until config is complete
if [ "$1" != "start" ] && [ ! -f "$CONFIG_FILE" ]; then
    echo "Configuration file not found at $CONFIG_FILE"
    echo "Running interactive configuration setup..."
    /usr/local/bin/totalrecall config interactive || {
        echo ""
        echo "ERROR: Failed to set up configuration."
        echo ""
        echo "If running non-interactively, please mount a config file:"
        echo "  docker run -v /path/to/config.toml:${BASE_PATH}/config.toml totalrecall start"
        echo ""
        echo "Or run interactively to set up configuration:"
        echo "  docker run -it --rm -v /path/to/totalrecall:${BASE_PATH} totalrecall config"
        exit 1
    }
    echo ""
    echo "Configuration setup complete. Starting with command: $@"
    echo ""
fi

# Execute the original command with error capture
echo "[ENTRYPOINT] Executing: /usr/local/bin/totalrecall $@"
# Don't use exec initially - run it and capture exit code to see what's happening
/usr/local/bin/totalrecall "$@" 2>&1
EXIT_CODE=$?
if [ $EXIT_CODE -ne 0 ]; then
    echo "[ENTRYPOINT] Command exited with code: $EXIT_CODE"
    exit $EXIT_CODE
fi

