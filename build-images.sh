#!/bin/sh

(cd asterisk; ${DOCKER-docker} build --tag=phcodercat/asterisk-vowifi .)
(cd pcscd; ${DOCKER-docker} build --tag=phcodercat/pcscd .)
