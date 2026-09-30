#!/bin/sh
# Build the images this stack runs, from the Dockerfiles in this repository.
#
# asterisk/Dockerfile still downloads phcodercat/asterisk-vowifi and
# phcodercat/pcscd during the build. Those are source stages only. The image
# that runs is compiled here, including the local ami_usim, entrypoint, and
# strongSwan tree. Do not docker-pull the Hub image and start that instead.
#
# pcscd/Dockerfile adds this repo's entrypoint on top of phcodercat/pcscd.
# Compose runs that result as ghcr.io/chuckleu1976/pcscd-sysmocom:latest.

set -eu
cd "$(CDPATH= cd -- "$(dirname "$0")" && pwd)"

export SIM_MODE="${SIM_MODE:-local}"

if ! docker info >/dev/null 2>&1; then
    echo "docker is not usable from this shell." >&2
    echo "Install Docker, add this user to the docker group, and log in again." >&2
    exit 1
fi

echo "Building pcscd and asterisk from the local Dockerfiles..."
echo "The Asterisk compile usually takes a long time."
docker compose build pcscd asterisk
echo "Images built."
