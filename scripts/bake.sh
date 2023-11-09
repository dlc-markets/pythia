#!/usr/bin/env bash

docker buildx bake -f docker-bake.hcl $@

