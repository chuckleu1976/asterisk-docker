# 📱 SMS Gateway

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/214zzl995/sms-gateway)

<div align="center">
  <img src="https://raw.githubusercontent.com/214zzl995/sms-gateway/main/frontend/public/logo.svg" alt="SMS Gateway Logo" width="200">
  <h3>A modern SMS / Voice management front-end for IMS-over-LTE handsets exposed via Asterisk AMI</h3>
</div>

## 📖 Overview

SMS Gateway is an AMI client that talks to one or more Asterisk containers and
turns every container into a SIM/IMS endpoint. SMS messages, inbound calls,
answered calls, hangups, and call recordings arrive as native AMI UserEvents;
the gateway persists them to SQLite, broadcasts them over Server-Sent Events,
forwards them through filtered webhooks, and (optionally) transcribes
recordings with `whisper.cpp`.

The legacy serial / AT-command backend was removed in `0.3.4`. SIMs are now
exposed exclusively through `pjsip` + an AMI bridge driven by the
[asterisk-docker](https://github.com/top-modem/asterisk-docker) compose stack.

### Key Features

- **Multi-SIM Support**: One AMI transport per `[[devices]]` entry; each
  asterisk container in `asterisk-docker` shows up as a separate SIM.
- **Real-time Messaging**: Send and receive SMS with live updates via SSE.
- **Call Recording + Transcription**: AMI `CallEnded` UserEvents carry a
  `RecordingPath`; the gateway resolves it to the host path under
  `{recordings_base_dir}/{instance}/recordings/<file>`, stores the WAV in the
  DB, and pipes it through ffmpeg + whisper.cpp for a transcript.
- **Modern Web Interface**: Conversation-based UI with per-SIM management.
- **Powerful Webhooks**: Forward SMS to external services with contact / SIM /
  time / regex filtering.
- **TOML Configuration**: A single `config.toml` describes every AMI endpoint
  (host, port, credentials, ICCID/IMSI/MSISDN for display).

## 🛠️ Building From Source

### Prerequisites

- Rust 1.60+ and Cargo
- Node.js 16+ and npm/pnpm
- SQLite

### Step 1: Build the Frontend

```bash
# Navigate to the frontend directory
cd frontend

# Install dependencies
pnpm install

# Build the frontend
pnpm run build
```

This will generate a `dist` directory within the frontend folder.

### Step 2: Build the Backend

```bash
# Return to the project root
cd ..

# Build the backend in release mode
cargo build --release
```

The compiled binary will be available in `target/release/sms-gateway`.

## 🚀 Running the Application

```bash
# Run with default settings
./target/release/sms-gateway

# Specify a custom config file
./target/release/sms-gateway --config /path/to/config.toml

# Set custom log directory
./target/release/sms-gateway --log /path/to/logs

# Set log level
./target/release/sms-gateway --log-level debug

# Update to the latest release
./target/release/sms-gateway update

# Show version
./target/release/sms-gateway version
```

## ⚙️ Configuration

The application is configured using a TOML file. By default, it looks for the config file at:
- Debug mode: `./config.toml`
- Release mode: `/etc/sms-gateway/config.toml`

Copy `config.toml.example` to `config.toml` and modify according to your setup:

```bash
cp config.toml.example config.toml
```

### Basic Configuration

```toml
[settings]
server_host = "0.0.0.0"
server_port = 8080
username = "admin"
password = "your_secure_password"

# Host path that maps to /logs/ inside each asterisk container.
# Inbound call recordings land in {recordings_base_dir}/{instance}/recordings/<file>.wav.
recordings_base_dir = "/home/ht/docker/logs"

# One [[devices]] block per asterisk container (= per SIM).
[[devices]]
instance   = 1                          # 1..8; AMI port defaults to 5037+instance (5038 here)
ami_host   = "127.0.0.1"
ami_user   = "jolly"
ami_secret = "geheim"
iccid      = "8901240387150170144"      # used as the sim_id key in the DB / API
imsi       = "310240385017144"          # optional, for the UI
msisdn     = "+18165537405"             # optional, for the UI

[[devices]]
instance   = 2
ami_secret = "geheim"
iccid      = "8901240387150170530"
imsi       = "310240385017053"
msisdn     = "+18165537217"
```

## 🔌 Architecture

```
  IMS / VoLTE carrier
          │
     pjsip + dialplan         (asterisk-docker, one container per SIM)
          │  UserEvent: SmsReceived / CallStarted / CallAnswered / CallEnded
          ▼
     AMI socket (5037 + instance)
          │
          ▼
  sms-gateway (this repo)
     ├ AmiTransport per SIM (login + event pump)
     ├ ModemEvent channel
     ├ DB writer (sqlite: contacts, sms, calls, sims)
     ├ SSE broadcaster (web UI live updates)
     ├ Webhook forwarder (filtered)
     └ Recording pipeline
          ├ read /home/ht/docker/logs/{instance}/recordings/<file>.wav
          ├ Call::save_recording
          └ ffmpeg | whisper-cli -> Call::save_transcript
```

## 🖥️ Running as a systemd service

A reference unit file is provided in [`contrib/systemd/sms-gateway.service`](contrib/systemd/sms-gateway.service).
Adjust `User=` and the paths to match your install, then:

```bash
sudo cp contrib/systemd/sms-gateway.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now sms-gateway.service
sudo journalctl -u sms-gateway.service -f
```

The unit assumes the release binary lives at
`/home/ht/sms-gateway/target/release/sms-gateway` and that `config.toml`,
`logs/`, and `data/` are siblings underneath the working directory.

## 📝 License

This project is licensed under the MIT License - see the [LICENSE](./LICENSE) file for details.
