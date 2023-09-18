#!/usr/bin/env bash

DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5432/pythia"

# Avoid droping a used database
docker exec -it pythia-postgres-1 psql -U postgres -c "DROP DATABASE pythia;"
docker exec -it pythia-postgres-1 psql -U postgres -c "CREATE DATABASE pythia;"

cargo sqlx migrate run --database-url $DATABASE_URL
cargo sqlx prepare --database-url $DATABASE_URL
