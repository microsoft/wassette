# Wassette Docker Image
# This Dockerfile provides a containerized runtime for Wassette with additional security isolation

# Stage 1: Build the Wassette binary
FROM rust:1.90-bookworm AS builder

# Install ca-certificates for HTTPS support during build
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy the project files
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY crates ./crates
COPY build.rs ./

# Build the release binary
RUN cargo build --release --bin wassette

# Stage 2: Create the runtime image
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 && \
    rm -rf /var/lib/apt/lists/*

# Create a non-root user for running Wassette
RUN useradd -m -u 1000 -s /bin/bash wassette

# Create necessary directories with proper permissions
RUN mkdir -p /home/wassette/.local/share/wassette/components && \
    mkdir -p /home/wassette/.config/wassette/secrets && \
    chown -R wassette:wassette /home/wassette

# Copy the binary from the builder stage
COPY --from=builder /build/target/release/wassette /usr/local/bin/wassette

# Set up environment
ENV HOME=/home/wassette
ENV XDG_DATA_HOME=/home/wassette/.local/share
ENV XDG_CONFIG_HOME=/home/wassette/.config

# Twelve-factor app compliance: support PORT and BIND_HOST environment variables
# Default PORT is 9001; default BIND_HOST in containers is 0.0.0.0 
# (required for external connections; differs from non-containerized default of 127.0.0.1)
ENV PORT=9001
ENV BIND_HOST=0.0.0.0

# Switch to the non-root user
USER wassette
WORKDIR /home/wassette

# Expose the default HTTP port (configurable via PORT env var)
EXPOSE 9001

# Default command: start Wassette with streamable-http transport
# Override this in docker run or docker-compose for different transports
CMD ["wassette", "serve", "--streamable-http"]
