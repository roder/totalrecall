# Multi-stage build for minimal final image
# Stage 1: Planner - cache dependencies
# Using Rust 1.85+ for edition2024 support (required by home crate)
# Rust 1.85.0 stabilized edition2024; using latest stable for best compatibility
FROM rust:1.89-slim as planner
WORKDIR /app

# Install build dependencies that might be needed for native crates
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace and crate manifests first (for dependency caching)
COPY Cargo.toml Cargo.lock ./
COPY crates/totalrecall-cli/Cargo.toml ./crates/totalrecall-cli/
COPY crates/media-sync-core/Cargo.toml ./crates/media-sync-core/
COPY crates/media-sync-models/Cargo.toml ./crates/media-sync-models/
COPY crates/media-sync-sources/Cargo.toml ./crates/media-sync-sources/
COPY crates/media-sync-config/Cargo.toml ./crates/media-sync-config/
COPY crates/browser-debug/Cargo.toml ./crates/browser-debug/

# Create dummy source files to allow dependency compilation
RUN mkdir -p crates/totalrecall-cli/src && \
    echo 'fn main() {}' > crates/totalrecall-cli/src/main.rs && \
    mkdir -p crates/media-sync-core/src && \
    echo '' > crates/media-sync-core/src/lib.rs && \
    mkdir -p crates/media-sync-models/src && \
    echo '' > crates/media-sync-models/src/lib.rs && \
    mkdir -p crates/media-sync-sources/src && \
    echo '' > crates/media-sync-sources/src/lib.rs && \
    mkdir -p crates/media-sync-config/src && \
    echo '' > crates/media-sync-config/src/lib.rs && \
    mkdir -p crates/browser-debug/src && \
    echo '' > crates/browser-debug/src/lib.rs

# Build dependencies (this layer will be cached if Cargo.toml/Cargo.lock don't change)
# Ensure target directory exists even if build fails
RUN mkdir -p target && cargo build --release --package totalrecall-cli || true

# Stage 2: Builder - build actual binary
FROM rust:1.89-slim as builder
WORKDIR /build

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Copy cached target directory from planner
COPY --from=planner /app/target /build/target

# Copy full source code
COPY . .

# Build the actual binary (only recompiles changed source, not dependencies)
RUN cargo build --release --package totalrecall-cli

# Stage 3: Runtime - minimal runtime image with headless Chromium
# Use Debian Trixie to match rust:latest-slim's GLIBC version
FROM debian:trixie-slim

# Install Chromium and dependencies, verify installation, and set permissions in one layer
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    chromium \
    chromium-sandbox \
    procps \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean \
    && (which chromium || which chromium-browser || (echo "Chromium not found" && exit 1)) \
    && chmod +x /usr/bin/chromium /usr/bin/chromium-browser 2>/dev/null || true

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