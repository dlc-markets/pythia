#!/usr/bin/env bash

set -e

if [[ -z "${REGISTRY}" ]]; then
  echo "REGISTRY is not set"
  exit 1
fi

if [[ -z "${NAME}" ]]; then
  echo "NAME is not set"
  exit 1
fi

BAKE_META_PATH=/tmp/bake-meta.json
DIGESTS_FOLDER=/tmp/digests

export SLUG=${REGISTRY}/${NAME}

ls -la $DIGESTS_FOLDER

TAGS=$(jq --arg registry "$REGISTRY" -cr '.target."docker-metadata-action".tags | map(select(startswith($registry)) | "-t " + .) | join(" ")' $BAKE_META_PATH)
DIGESTS=$(find $DIGESTS_FOLDER  -maxdepth 1 -type f -print0 | xargs -0 -I {} bash -c 'printf "%s@sha256:%s " $SLUG $(basename {})')

echo "docker buildx imagetools create $TAGS $DIGESTS"

docker buildx imagetools create $TAGS $DIGESTS
