#!/bin/sh
# Build the local images and start one Asterisk container per inserted SIM.
# Host packages (Docker, pcscd, python3-pyscard) must already be installed.
# See asterisk_setup_x86.txt.

set -eu
cd "$(CDPATH= cd -- "$(dirname "$0")" && pwd)"

export SIM_MODE=local

if ! docker info >/dev/null 2>&1; then
    echo "docker is not usable from this shell." >&2
    echo "After 'sudo usermod -aG docker \$USER', log out and back in." >&2
    echo "Until then: sudo setfacl -m u:\$USER:rw /var/run/docker.sock" >&2
    exit 1
fi

if command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet pcscd; then
    echo "Host pcscd is running and will hold the USB readers." >&2
    echo "Stop it before this stack:" >&2
    echo "  sudo systemctl disable --now pcscd" >&2
    exit 1
fi

python3 - << 'PY'
from start_sim_container_config import parse_devices_toml
from scripts.sim_config_gen import generate_compose
generate_compose(parse_devices_toml())
print("compose.yaml written")
PY

./build-images.sh

echo "Probing SIM readers and starting containers..."
python3 start_sim_container_config.py --setup

echo
echo "Check IMS registration (reader0 is the service named asterisk):"
echo "  docker compose exec asterisk asterisk -rx 'pjsip show registrations'"
echo "Reader1 is asterisk2, on host SIP port 5062 and AMI port 15039."
echo "Reader0 is published on host SIP port 15060 and AMI port 15038"
echo "because a host Asterisk already uses 5060 and 5038."
