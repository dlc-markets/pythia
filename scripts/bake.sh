#!/usr/bin/env bash

set -e

export LOCAL=true

docker buildx bake -f docker-bake.hcl
