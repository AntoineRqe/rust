# Runtime stage only - uses pre-built binary
FROM ubuntu:24.04

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the pre-compiled binary from local build
COPY ../target/release/market-simulator /app/market-simulator
COPY ../crates/config /app/crates/config

# Expose ports for web terminal, and gRPC
EXPOSE 9860 9870 9871 9885 9884 50051 50052 9891 9892

ENV RUST_LOG=info

ENTRYPOINT ["/app/market-simulator"]
CMD ["--config", "crates/config/docker.json"]
