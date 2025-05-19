#!/usr/bin/env bash

DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5433/pythia_test"

# Check if postgres container is already running
if ! docker ps | grep -q postgres-test; then
  echo "Starting postgres container..."
  docker compose -f compose.dev.yml down --volumes postgres-test
  docker compose -f compose.dev.yml up -d --wait postgres-test
else
  echo "Postgres container is already running."
fi

# Check if the database exists, create it if it doesn't
if ! docker compose -f compose.dev.yml exec -T postgres-test psql -U postgres -lqt | cut -d \| -f 1 | grep -qw pythia_test; then
  echo "Creating pythia database..."
  docker compose -f compose.dev.yml exec -T postgres-test psql -U postgres -c "CREATE DATABASE pythia_test;"
else
  echo "Database 'pythia_test' already exists."
fi

echo "Running migrations..."
cargo sqlx migrate run --source packages/pythia/migrations --database-url $DATABASE_URL

echo "Preparing SQLx..."
cargo sqlx prepare --workspace --database-url $DATABASE_URL -- --all-targets
