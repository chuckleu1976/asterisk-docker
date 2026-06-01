#!/usr/bin/env python3
"""
Save incoming SMS and extract OTP codes.
Called by Asterisk extensions.conf on incoming SIP MESSAGE:
  TrySystem(/usr/local/bin/sms_save.py '${MESSAGE(from)}' '${SMSBODY}' &)
  where SMSBODY is BASE64_ENCODE(${MESSAGE(body)})

Appends raw message to /logs/messages.txt
Appends extracted OTP (if found) to /logs/otp_sms.txt
Saves all SMS to /logs/sms.db (SQLite)
"""
import sys
import re
import base64
import os
import sqlite3
from datetime import datetime

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
# Priority order: contextual patterns first, then standalone digits
OTP_PATTERNS = [
    # "code is 123456" / "code: 123456" / "code 123456"
    r'(?:verification\s+)?code[:\s]+(\d{4,8})',
    # "OTP: 123456" / "OTP is 123456"
    r'\bOTP[:\s]+(\d{4,8})',
    # "PIN: 123456"
    r'\bPIN[:\s]+(\d{4,8})',
    # "passcode: 123456"
    r'passcode[:\s]+(\d{4,8})',
    # "password: 123456"
    r'password[:\s]+(\d{4,8})',
    # "use 123456" / "enter 123456"
    r'(?:use|enter|is)\s+(\d{4,8})\b',
    # Standalone 4-8 digit number (last resort)
    r'\b(\d{4,8})\b',
]

otp = None
for pattern in OTP_PATTERNS:
    m = re.search(pattern, body, re.IGNORECASE)
    if m:
        otp = m.group(1)
        break

if otp:
    entry = (
        f"[{timestamp}] From: {sender}\n"
        f"SMS: {body}\n"
        f"OTP: {otp}\n"
        f"{'=' * 50}\n\n"
    )
    with open("/logs/otp_sms.txt", "a") as f:
        f.write(entry)
    print(f"OTP extracted: {otp}")
else:
    print("No OTP found in SMS body.")

# --- Save to SQLite ---
try:
    db = sqlite3.connect("/logs/sms.db")
    db.execute('''CREATE TABLE IF NOT EXISTS sms (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        sender      TEXT,
        body        TEXT,
        otp         TEXT,
        received_at TEXT NOT NULL
    )''')
    db.execute(
        'INSERT INTO sms (sender, body, otp, received_at) VALUES (?, ?, ?, ?)',
        (sender, body, otp, timestamp)
    )
    db.commit()
    db.close()
    print(f"Saved to sms.db: from={sender} otp={otp}")
except Exception as e:
    print(f"SQLite error: {e}", file=sys.stderr)
