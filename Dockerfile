# ---------- Build stage ----------
FROM rust:1-bookworm AS builder
WORKDIR /usr/src/app

# Copy manifests first (layer-cached dependency resolve)
COPY Cargo.toml Cargo.lock ./

# Copy source
COPY src ./src

# Build (use --locked if you want Cargo.lock enforced)
RUN cargo build --release

# ---------- Runtime stage ----------
FROM debian:bookworm-slim

# Non-root user
RUN groupadd --system --gid 1001 sgodilla && \
    useradd  --system --uid 1001 --gid 1001 sgodilla

# HTTPS certs + timezone info
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the built binary (adjust name if your crate/binary name differs)
COPY --from=builder --chown=1001:1001 /usr/src/app/target/release/fatebook_beeminder_bridge /app/bridge

# Default logs (can be overridden by compose)
ENV RUST_LOG=info

USER sgodilla
CMD ["/app/bridge"]
