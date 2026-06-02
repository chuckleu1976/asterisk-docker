#!/usr/bin/env python3
"""
Save incoming SMS and extract OTP codes.
Called by Asterisk extensions.conf on incoming SIP MESSAGE:
  TrySystem(/usr/local/bin/sms_save.py '${MESSAGE(from)}' '${SMSBODY}' &)
  where SMSBODY is BASE64_ENCODE(${MESSAGE(body)})

Appends raw message to /logs/messages.txt
Appends extracted OTP (if found) to /logs/otp_sms.txt
Saves all SMS to /data/sim_inventory.db (shared SQLite, mounted from host)
"""
import sys
import re
import base64
import os
import sqlite3
from datetime import datetime

DB_PATH  = "/data/sim_inventory.db"
INSTANCE = os.environ.get("HOSTNAME", "unknown")

if len(sys.argv) < 3:
    print("Usage: sms_save.py <from> <base64_body>", file=sys.stderr)
    sys.exit(1)

sender = sys.argv[1]
try:
    body = base64.b64decode(sys.argv[2]).decode("utf-8", errors="replace")
except Exception as e:
    print(f"Failed to decode body: {e}", file=sys.stderr)
    sys.exit(1)

timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")

# --- Save raw SMS ---
os.makedirs("/logs", exist_ok=True)
with open("/logs/messages.txt", "a") as f:
    f.write(
        f"[{timestamp}] From: {sender}\n"
        f"{body}\n"
        f"{'=' * 50}\n\n"
    )

# --- Extract OTP ---
OTP_PATTERNS = [
    r'(?:verification\s+)?code[:\s]+(\d{4,8})',
    r'\bOTP[:\s]+(\d{4,8})',
    r'\bPIN[:\s]+(\d{4,8})',
    r'passcode[:\s]+(\d{4,8})',
    r'password[:\s]+(\d{4,8})',
    r'(?:use|enter|is)\s+(\d{4,8})\b',
    r'\b(\d{4,8})\b',
]

otp = None
for pattern in OTP_PATTERNS:
    m = re.search(pattern, body, re.IGNORECASE)
    if m:
        otp = m.group(1)
        break

if otp:
    with open("/logs/otp_sms.txt", "a") as f:
        f.write(
            f"[{timestamp}] From: {sender}\n"
            f"SMS: {body}\n"
            f"OTP: {otp}\n"
            f"{'=' * 50}\n\n"
        )
    print(f"OTP extracted: {otp}")
else:
    print("No OTP found in SMS body.")

# --- Skip delivery reports (empty body = SMS-DELIVER-REPORT from SMS-SC) ---
if not body.strip():
    print("Empty body (delivery report) — skipping save.")
    sys.exit(0)

# --- Save to shared SQLite (WAL mode for safe concurrent writes) ---
try:
    db = sqlite3.connect(DB_PATH, timeout=10)
    db.execute("PRAGMA journal_mode=WAL")
    db.execute('''CREATE TABLE IF NOT EXISTS sms (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        instance    TEXT,
        sender      TEXT,
        body        TEXT,
        otp         TEXT,
        received_at TEXT NOT NULL
    )''')
    db.execute(
        'INSERT INTO sms (instance, sender, body, otp, received_at) VALUES (?,?,?,?,?)',
        (INSTANCE, sender, body, otp, timestamp)
    )
    db.commit()
    db.close()
    print(f"Saved to sim_inventory.db: instance={INSTANCE} from={sender} otp={otp}")
except Exception as e:
    print(f"SQLite error: {e}", file=sys.stderr)
