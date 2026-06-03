#!/bin/bash
# =============================================================
# VoWiFi Pi Production Deployment Setup Script
# Tested on: Raspberry Pi 5, Debian 13 (trixie)
# =============================================================
set -e

DEPLOY_DIR="$(cd "$(dirname "$0")" && pwd)"
GITHUB_USER="chuckleu1976"

echo "=============================="
echo " VoWiFi Pi Deployment Setup"
echo "=============================="

# 1. Install Docker if not present
if ! command -v docker &>/dev/null; then
  echo "[1/5] Installing Docker..."
  curl -fsSL https://get.docker.com | sh
  systemctl enable docker
  systemctl start docker
else
  echo "[1/5] Docker already installed: $(docker --version)"
fi

# 2. Configure Docker mirror (required if Docker Hub is blocked)
if [ ! -f /etc/docker/daemon.json ]; then
  echo "[2/5] Configuring Docker registry mirror..."
  cat > /etc/docker/daemon.json <<EOF
{
  "registry-mirrors": ["https://mirror.gcr.io"]
}
EOF
  systemctl restart docker
  sleep 3
else
  echo "[2/5] Docker daemon.json already exists, skipping."
fi

# 3. Authenticate to ghcr.io
echo "[3/5] Authenticating to ghcr.io..."
echo "  -> Enter your GitHub Personal Access Token (read:packages scope):"
read -rs GHCR_TOKEN
echo "$GHCR_TOKEN" | docker login ghcr.io -u "$GITHUB_USER" --password-stdin

# 4. Pull prebuilt images
echo "[4/5] Pulling prebuilt ARM64 images from ghcr.io..."
docker pull ghcr.io/$GITHUB_USER/pcscd-sysmocom:latest
docker pull ghcr.io/$GITHUB_USER/asterisk-vowifi-arm64:latest

# 5. Setup config and remsim files
echo "[5/5] Checking configuration..."

# Copy .env if not present
if [ ! -f "$DEPLOY_DIR/.env" ]; then
  cp "$DEPLOY_DIR/.env.example" "$DEPLOY_DIR/.env"
  echo "  -> Created .env from .env.example — please edit it!"
fi

# Check remsim files
if [ ! -f "$DEPLOY_DIR/remsim/serial/libifd_remsim_client.so" ]; then
  echo "  [WARNING] remsim/serial/libifd_remsim_client.so not found!"
  echo "  -> Copy from existing Pi or build from https://github.com/chuckleu1976/osmo-remsim"
fi

if [ ! -d "$DEPLOY_DIR/remsim/libs" ] || [ -z "$(ls -A $DEPLOY_DIR/remsim/libs)" ]; then
  echo "  [WARNING] remsim/libs/ is empty!"
  echo "  -> Copy ARM64 osmocom .so files from existing Pi: ~/asterisk-docker/remsim/libs/"
fi

# Create logs dir
mkdir -p "$DEPLOY_DIR/logs"

echo ""
echo "=============================="
echo " Setup complete!"
echo "=============================="
echo ""
echo "Next steps:"
echo "  1. Edit .env with your remsim server IP and SIM settings"
echo "  2. Copy your SIM config to config/asterisk/ (pjsip.conf, epdg.conf, etc.)"
echo "  3. Update remsim/reader.conf.d/osmo-remsim-client with correct server IP"
echo "  4. Run: docker compose up -d"
echo ""
