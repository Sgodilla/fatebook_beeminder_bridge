# Build stage
FROM rust:1-bookworm as builder

WORKDIR /usr/src/app

# Copy manifests first for better caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to build dependencies
RUN mkdir src && \
    echo "fn main() {println!(\"if you see this, the build broke\")}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Now copy actual source code
COPY src ./src

# Build for release
RUN cargo build --release

# --- Runtime stage ---
FROM debian:bookworm-slim

# BEST PRACTICE: Create a non-root user to run the application
RUN groupadd --system --gid 1001 sgodilla && \
    useradd --system --uid 1001 --gid sgodilla sgodilla

WORKDIR /app

# Copy the built binary and set ownership to the new user
COPY --from=builder --chown=daemo:daemo /usr/src/app/target/release/fatebook_beeminder_bridge /app/fatebook_beeminder_bridge

# Set default environment variables
ENV RUST_LOG=fatebook_beeminder_bridge=debug,reqwest=info


# BEST PRACTICE: Switch to the non-root user
USER sgodilla

# Run the binary
CMD ["/app/daemo-engine"]
