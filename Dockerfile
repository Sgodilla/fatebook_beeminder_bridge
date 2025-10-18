# ---------- Build stage ----------
FROM rust:1-bookworm AS builder
WORKDIR /usr/src/app

# 1) Cache dependencies as much as possible
# Copy manifests first
COPY Cargo.toml Cargo.lock ./

# Create a minimal src to let cargo resolve deps (optional but safe)
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs && cargo build --release || true

# 2) Now bring in actual source and do the real build
# This invalidates the previous layer whenever your src changes
COPY src ./src
RUN cargo build --release

# ---------- Runtime stage ----------
FROM debian:bookworm-slim

# Non-root user
RUN groupadd --system --gid 1001 sgodilla && \
    useradd  --system --uid 1001 --gid 1001 sgodilla

# SSL certs for HTTPS (reqwest)
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tzdata && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the built binary; name it something short like "bridge"
# NOTE: Use the exact binary name produced by Cargo. If your package is `fatebook_beeminder_bridge`,
# then the path below is correct. Adjust if your crate name differs.
COPY --from=builder --chown=1001:1001 /usr/src/app/target/release/fatebook_beeminder_bridge /app/bridge

# Default log level; can be overridden by docker-compose or env
ENV RUST_LOG=info

USER sgodilla

# Long-running service (your code loops every 60s)
CMD ["/app/bridge"]
