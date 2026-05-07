#!/bin/bash

# Build binaries before Docker image creation
# This ensures we use the correct local Rust toolchain

set -e

echo "🏗️  Building release binaries..."

# Build market-simulator
cargo build --release -p market-simulator
echo "✓ market-simulator built"

# Build players service
cargo build --release -p players
echo "✓ players-server built"

echo ""
echo "✅ All binaries built successfully!"
echo ""
echo "To build Docker images, run:"
echo "  docker-compose build"
echo "  docker-compose up"
