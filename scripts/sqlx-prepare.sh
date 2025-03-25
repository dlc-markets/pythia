#!/usr/bin/env bash

DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5432/pythia"

# Check if postgres container is already running
if ! docker ps | grep -q postgres; then
  echo "Starting postgres container..."
  docker compose down --volumes postgres
  docker compose up -d --wait postgres
else
  echo "Postgres container is already running."
fi

# Check if the database exists, create it if it doesn't
if ! docker compose exec postgres psql -U postgres -lqt | cut -d \| -f 1 | grep -qw pythia; then
  echo "Creating pythia database..."
  docker compose exec postgres psql -U postgres -c "CREATE DATABASE pythia;"
else
  echo "Database 'pythia' already exists."
fi

echo "Running migrations..."
cargo sqlx migrate run --source packages/pythia/migrations --database-url $DATABASE_URL

echo "Preparing SQLx..."
cargo sqlx prepare --check --workspace --database-url $DATABASE_URL -- --all-targets
