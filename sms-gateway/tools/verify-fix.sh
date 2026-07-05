#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo is not installed or not in PATH" >&2
  exit 1
fi

if ! command -v npm >/dev/null 2>&1; then
  echo "ERROR: npm is not installed or not in PATH" >&2
  exit 1
fi

echo "[1/4] cargo check"
cargo check -q

echo "[2/4] targeted regression tests"
cargo test -q db_direction_tests

echo "[3/4] frontend build"
(
  cd frontend
  npm run -s build
)

echo "[4/4] sqlite sanity checks (if DB exists)"
DB_PATH="$ROOT_DIR/data/data.db"
if command -v sqlite3 >/dev/null 2>&1 && [[ -f "$DB_PATH" ]]; then
  unresolved_count="$(sqlite3 "$DB_PATH" "
    SELECT COUNT(*)
    FROM sms i
    WHERE i.send = 0
      AND (i.contact_id IS NULL OR TRIM(i.contact_id) = '')
      AND TRIM(i.message) != ''
      AND NOT EXISTS (
        SELECT 1
        FROM sms s
        JOIN sim_cards recv ON recv.id = i.sim_id
        JOIN sim_cards sc_send ON sc_send.id = s.sim_id
        WHERE s.send = 1
          AND TRIM(s.message) = TRIM(i.message)
          AND REPLACE(s.contact_id, '+', '') = REPLACE(recv.phone_number, '+', '')
          AND sc_send.phone_number IS NOT NULL
          AND TRIM(sc_send.phone_number) != ''
      )
      AND i.message NOT GLOB '*[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]*';
  " )"

  if [[ "$unresolved_count" != "0" ]]; then
    echo "ERROR: Found $unresolved_count inbound rows with blank sender and no deterministic fallback." >&2
    echo "Run this query for details:" >&2
    echo "  SELECT id, sim_id, quote(contact_id), quote(message), timestamp FROM sms WHERE send = 0 AND (contact_id IS NULL OR TRIM(contact_id) = '') ORDER BY id DESC LIMIT 20;" >&2
    exit 1
  fi

  echo "sqlite sanity: OK"
else
  echo "sqlite sanity: skipped (sqlite3 or data/data.db missing)"
fi

echo "All verification steps passed."
