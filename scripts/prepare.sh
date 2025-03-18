#!/usr/bin/env bash

set -e

if [[ -n "$RUNNING_IN_DOCKER" || -n "$CI" ]]; then
  echo "Skipping prepare"
  exit 0
fi

pnpm lefthook install
