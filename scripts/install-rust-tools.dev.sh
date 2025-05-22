#!/usr/bin/env bash

set -e

echo "Installing Rust development tools..."

# Check if cargo-make is installed
if ! command -v cargo-make &> /dev/null; then
    echo "Installing cargo-make..."
    cargo install --force cargo-make
else
    echo "cargo-make is already installed"
fi

# Check if sqlx-cli is installed
if ! command -v sqlx &> /dev/null; then
    echo "Installing sqlx-cli..."
    cargo install sqlx-cli --no-default-features --features native-tls,postgres
else
    echo "sqlx-cli is already installed"
fi

echo "Rust development tools installation complete!"
