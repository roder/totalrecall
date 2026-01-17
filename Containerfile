# Multi-stage build for minimal final image
# Using latest stable Rust to support newer dependencies
FROM rust:latest as builder
WORKDIR /build
COPY . .
RUN cargo build --release

# Minimal runtime image with headless Chromium
# Use Debian Trixie to match rust:latest's GLIBC version
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    chromium \
    chromium-sandbox \
    procps \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

# Verify Chromium installation and ensure it's executable
RUN which chromium || which chromium-browser || (echo "Chromium not found" && exit 1)
RUN chmod +x /usr/bin/chromium /usr/bin/chromium-browser 2>/dev/null || true

# Create non-root user for security
RUN useradd -m -u 1000 totalrecall && \
    mkdir -p /app/data /app/logs && \
    chown -R totalrecall:totalrecall /app

# Copy binary
COPY --from=builder /build/target/release/totalrecall /usr/local/bin/totalrecall

# Copy entrypoint script
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Switch to non-root user
USER totalrecall

WORKDIR /app

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
# Default command: start daemon (can be overridden in docker-compose)
CMD ["start"]

