#!/bin/sh

# Try the full source build first (requires fedora:latest base image).
# Fall back to the local patch build if base images are not available offline.
(
  cd asterisk
  ${DOCKER-docker} build --tag=phcodercat/asterisk-vowifi .
) || (
  echo "Full source build failed; falling back to local patch build..."
  cd asterisk
  ${DOCKER-docker} build -f Dockerfile.local --tag=phcodercat/asterisk-vowifi .
)

(cd pcscd; ${DOCKER-docker} build --tag=phcodercat/pcscd .)
