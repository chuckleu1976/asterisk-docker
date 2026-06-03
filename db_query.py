#!/usr/bin/env python3
"""
db_query.py — Query sim_inventory.db from the command line.

Usage:
  python3 db_query.py               # show recent SMS (default 20)
  python3 db_query.py sms           # show SMS
  python3 db_query.py sms -n 50     # show last 50 SMS
  python3 db_query.py sms --otp     # show only messages with OTP
  python3 db_query.py sms --instance asterisk1
  python3 db_query.py sms --sender +18165537217
  python3 db_query.py sms --since "2026-06-01"
  python3 db_query.py sims          # show SIM inventory
  python3 db_query.py watch         # live-tail new SMS (Ctrl+C to stop)
"""

import argparse
import sqlite3
import sys
import time
from datetime import datetime

DB_PATH = "/home/ht/docker/asterisk-docker/sim_inventory.db"

# ── ANSI colours ────────────────────────────────────────────────────────────
RESET  = "\033[0m"
BOLD   = "\033[1m"
CYAN   = "\033[96m"
GREEN  = "\033[92m"
YELLOW = "\033[93m"
DIM    = "\033[2m"


def connect():
    db = sqlite3.connect(DB_PATH)
    db.row_factory = sqlite3.Row
    return db


def print_sms_row(row):
    ts      = row["received_at"]
    inst    = row["instance"] or ""
    sender  = row["sender"] or ""
    body    = row["body"] or ""
    otp     = row["otp"] or ""
    rid     = row["id"]

    otp_tag = f"  {YELLOW}[OTP: {otp}]{RESET}" if otp else ""
    inst_color = CYAN if "1" in inst else GREEN
    print(f"{DIM}#{rid:>4}{RESET}  {DIM}{ts}{RESET}  "
          f"{inst_color}{inst:<10}{RESET}  "
          f"{BOLD}{sender:<20}{RESET}  "
          f"{body}{otp_tag}")


def cmd_sms(args):
    db = connect()

    where = []
    params = []

    if args.instance:
        where.append("instance = ?")
        params.append(args.instance)
    if args.sender:
        where.append("sender LIKE ?")
        params.append(f"%{args.sender}%")
    if args.otp:
        where.append("otp != '' AND otp IS NOT NULL")
    if args.since:
        where.append("received_at >= ?")
        params.append(args.since)
    if args.no_reports:
        where.append("body != '' AND body IS NOT NULL")

    clause = ("WHERE " + " AND ".join(where)) if where else ""
    params.append(args.n)

    query = f"""
        SELECT id, instance, sender, body, otp, received_at
        FROM sms
        {clause}
        ORDER BY received_at DESC
        LIMIT ?
    """

    rows = db.execute(query, params).fetchall()
    if not rows:
        print("No messages found.")
        return

    print(f"\n{'#':>5}  {'received_at':<19}  {'instance':<10}  {'sender':<20}  body")
    print("─" * 90)
    for row in reversed(rows):
        print_sms_row(row)
    print(f"\n{len(rows)} message(s)")


def cmd_sims(args):
    db = connect()
    rows = db.execute(
        "SELECT reader, iccid, imsi, msisdn, mcc, mnc, updated_at FROM sims ORDER BY reader"
    ).fetchall()

    if not rows:
        print("No SIMs in inventory.")
        return

    print(f"\n{'P':<3}  {'MSISDN':<16}  {'IMSI':<16}  {'MCC':<4}  {'MNC':<4}  {'ICCID':<22}  updated_at")
    print("─" * 95)
    for r in rows:
        print(f"P{r['reader']:<2}  {r['msisdn'] or '':<16}  {r['imsi'] or '':<16}  "
              f"{r['mcc'] or '':<4}  {r['mnc'] or '':<4}  {r['iccid'] or '':<22}  {r['updated_at'] or ''}")


def cmd_watch(args):
    db = connect()
    last_id = db.execute("SELECT COALESCE(MAX(id), 0) FROM sms").fetchone()[0]
    print(f"Watching for new SMS (last id={last_id})... Ctrl+C to stop\n")

    try:
        while True:
            rows = db.execute(
                "SELECT id, instance, sender, body, otp, received_at "
                "FROM sms WHERE id > ? ORDER BY id",
                (last_id,)
            ).fetchall()
            for row in rows:
                print_sms_row(row)
                last_id = row["id"]
            time.sleep(2)
    except KeyboardInterrupt:
        print("\nStopped.")


def main():
    parser = argparse.ArgumentParser(description="Query sim_inventory.db")
    sub = parser.add_subparsers(dest="cmd")

    # sms
    p_sms = sub.add_parser("sms", help="Show SMS messages")
    p_sms.add_argument("-n", type=int, default=20, metavar="N", help="Number of rows (default 20)")
    p_sms.add_argument("--instance", help="Filter by instance (asterisk1 / asterisk2)")
    p_sms.add_argument("--sender", help="Filter by sender (partial match)")
    p_sms.add_argument("--otp", action="store_true", help="Only messages with OTP")
    p_sms.add_argument("--since", metavar="DATE", help="Only messages since DATE (e.g. 2026-06-01)")
    p_sms.add_argument("--no-reports", action="store_true", help="Hide empty RP-ACK report rows")

    # sims
    sub.add_parser("sims", help="Show SIM inventory")

    # watch
    sub.add_parser("watch", help="Live-tail new incoming SMS")

    args = parser.parse_args()

    if args.cmd == "sims":
        cmd_sims(args)
    elif args.cmd == "watch":
        cmd_watch(args)
    else:
        # default: sms
        if args.cmd is None:
            args.cmd = "sms"
            args.n = 20
            args.instance = None
            args.sender = None
            args.otp = False
            args.since = None
            args.no_reports = False
        cmd_sms(args)


if __name__ == "__main__":
    main()
