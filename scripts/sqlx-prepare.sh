#!/usr/bin/env bash

DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5432/pythia"

docker compose down --volumes postgres
docker compose up -d --wait postgres

docker compose exec postgres psql -U postgres -c "CREATE DATABASE pythia;"

cargo sqlx migrate run --source packages/pythia/migrations --database-url $DATABASE_URL
cargo sqlx prepare --workspace --database-url $DATABASE_URL
