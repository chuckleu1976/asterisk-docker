"""Install osmo-remsim slot maps for the remote SIM bank.

Maps bank slots that have a card onto PC/SC client ids used by
remsim/reader.conf.d. The remsim server keeps these maps in memory, so
this is applied again whenever the probe container starts.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent.parent

# Bank 1 on 192.168.31.68. Slots 0, 1 and 2 are the readers that currently
# have a card inserted (Alcor Link AK9563 00/01/02).
DEFAULT_API = "http://192.168.31.68:9997/api/backend/v1"
DEFAULT_BANK_ID = 1
DEFAULT_SLOTS = (0, 1, 2)


def _env_from_file() -> dict[str, str]:
    values: dict[str, str] = {}
    env_file = SCRIPT_DIR / ".env"
    if not env_file.exists():
        return values
    for line in env_file.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip('"').strip("'")
    return values


def _setting(name: str, file_values: dict[str, str], default: str) -> str:
    return os.environ.get(name) or file_values.get(name) or default


def apply_slotmaps() -> None:
    file_values = _env_from_file()
    if _setting("SIM_MODE", file_values, "local") != "remote":
        return

    api = _setting("REMSIM_API", file_values, DEFAULT_API).rstrip("/")
    bank_id = int(_setting("REMSIM_BANK_ID", file_values, str(DEFAULT_BANK_ID)))
    slot_text = _setting("REMSIM_SLOTS", file_values, ",".join(str(s) for s in DEFAULT_SLOTS))
    slots = [int(part) for part in slot_text.split(",") if part.strip()]

    with urllib.request.urlopen(f"{api}/slotmaps", timeout=5) as resp:
        current = json.load(resp)

    existing = {
        (
            item["bank"]["bankId"],
            item["bank"]["slotNr"],
            item["client"]["clientId"],
            item["client"]["slotNr"],
        )
        for item in current.get("slotmaps", [])
    }

    for slot in slots:
        key = (bank_id, slot, slot, 0)
        if key in existing:
            print(f"  slotmap B{bank_id}:{slot} -> C{slot}:0 already present")
            continue
        body = json.dumps({
            "bank": {"bankId": bank_id, "slotNr": slot},
            "client": {"clientId": slot, "slotNr": 0},
        }).encode()
        req = urllib.request.Request(
            f"{api}/slotmaps",
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                status = resp.status
        except urllib.error.HTTPError as exc:
            raise RuntimeError(
                f"slotmap B{bank_id}:{slot} -> C{slot}:0 failed: HTTP {exc.code}"
            ) from exc
        print(f"  slotmap B{bank_id}:{slot} -> C{slot}:0 created ({status})")
