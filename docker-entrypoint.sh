#!/bin/sh
set -e

# Get container base path from environment variable, defaulting to /app
BASE_PATH="${TOTALRECALL_BASE_PATH:-/app}"
CONFIG_FILE="${BASE_PATH}/config.toml"

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
        echo "  docker run -it --rm -v /path/to/totalrecall:${BASE_PATH} totalrecall config interactive"
        exit 1
    }
    echo ""
    echo "Configuration setup complete. Starting with command: $@"
    echo ""
fi

# Execute the original command
exec /usr/local/bin/totalrecall "$@"

