#!/usr/bin/env python3
"""
sim_config.py - Read SIM card(s) via pcscd-in-Docker, update asterisk-docker
               config files on the host, then optionally restart containers.

The APDU reader runs inside the 'asterisk' container (has python3-pyscard +
pcsc-sock volume). Config files are updated on the host (bind-mounted paths).

Usage:
  ./sim_config.py --list
  ./sim_config.py --instance 1 --reader 0 [--restart]
  ./sim_config.py --instance 2 --reader 1 [--restart]
  ./sim_config.py --auto [--restart]     # reader 0->instance 1, reader 1->instance 2
  ./sim_config.py --instance 1 --reader 0 --msisdn +18165551234 [--restart]
"""

import argparse
import json
import re
import sqlite3
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

SCRIPT_DIR = Path(__file__).parent      # asterisk-docker/
CONFIG_DIR = SCRIPT_DIR / "config"
DB_PATH    = SCRIPT_DIR / "sim_inventory.db"

# Container used for APDU reading — must be running, has pyscard + pcsc-sock
READ_CONTAINER = "asterisk"

# ─── Embedded reader script (runs inside container via docker compose exec) ───
# All readers are visible from any asterisk container (shared pcsc-sock volume).
_READER_SCRIPT = r"""
import json, sys
from smartcard.System import readers
from smartcard.util import toHexString, toBytes

reader_index = int(sys.argv[1]) if len(sys.argv) > 1 else 0

def _apdu(conn, hex_cmd):
    data, sw1, sw2 = conn.transmit(toBytes(hex_cmd))
    return data, sw1, sw2

def _swap(s):
    return ''.join(x + y for x, y in zip(s[1::2], s[0::2]))

def connect(idx):
    r = readers()
    if not r:
        raise RuntimeError("No PC/SC readers — is pcscd running?")
    if idx >= len(r):
        raise RuntimeError(f"Reader {idx} not found ({len(r)} reader(s) available)")
    conn = r[idx].createConnection()
    conn.connect()
    return conn

def select_aid(conn):
    data, sw1, sw2 = _apdu(conn, '00A40004022F0000')
    if sw1 != 0x61:
        return None
    data, sw1, sw2 = _apdu(conn, f'00C00000{sw2:02X}')
    if (sw1, sw2) != (0x90, 0x00):
        return None
    rl = data[7]
    data, sw1, sw2 = _apdu(conn, f'00B20104{rl:02X}')
    if sw1 != 0x90:
        return None
    al = data[3]
    return toHexString(data[4:4 + al]).replace(' ', '')

def reselect_aid(conn, aid):
    _apdu(conn, f'00A40404{len(aid)//2:02X}{aid}')

def read_iccid(conn):
    # SELECT EF.ICCID by absolute path from MF (P1=08, path=3F002FE2)
    data, sw1, sw2 = _apdu(conn, '00A40804043F002FE2')
    if sw1 == 0x61:
        data, sw1, sw2 = _apdu(conn, f'00C00000{sw2:02X}')
    if sw1 != 0x90:
        return None
    data, sw1, sw2 = _apdu(conn, '00B000000A')
    if sw1 != 0x90:
        return None
    return _swap(toHexString(data).replace(' ', '')).rstrip('fF').upper()

def read_imsi(conn):
    data, sw1, sw2 = _apdu(conn, '00A40004026F07')
    if sw1 == 0x61:
        data, sw1, sw2 = _apdu(conn, f'00C00000{sw2:02X}')
    if sw1 != 0x90:
        return None
    data, sw1, sw2 = _apdu(conn, '00B0000009')
    if (sw1, sw2) != (0x90, 0x00):
        return None
    raw = bytes(data).hex()
    length = int(raw[0:2], 16) * 2 - 1
    swapped = _swap(raw[2:]).rstrip('f')
    return swapped[1:1 + length]

def read_mnc_length(conn):
    # Read actual MNC digit count from EF.AD for correct 3GPP domain
    data, sw1, sw2 = _apdu(conn, '00A40004026FAD')
    if sw1 == 0x61:                              # GET RESPONSE for FCP
        data, sw1, sw2 = _apdu(conn, f'00C00000{sw2:02X}')
    if sw1 != 0x90:
        return 2
    # READ BINARY — actual file content; byte[3] = MNC length
    data, sw1, sw2 = _apdu(conn, '00B0000004')
    if sw1 != 0x90 or len(data) < 4:
        return 2
    mnc_len = data[3] & 0x0F
    return 3 if mnc_len == 3 else 2

def read_msisdn(conn):
    data, sw1, sw2 = _apdu(conn, '00A40004026F40')
    fcp = None
    if sw1 == 0x61:
        fcp, sw1, sw2 = _apdu(conn, f'00C00000{sw2:02X}')
    if sw1 != 0x90:
        return None
    rec_len = 28  # common default
    if fcp:
        i = 0
        while i < len(fcp) - 1:
            tag, tlen = fcp[i], fcp[i + 1]
            if tag == 0x62:
                i += 2; continue
            if tag == 0x82 and tlen >= 3:
                rec_len = (fcp[i+4] << 8 | fcp[i+5]) if tlen >= 5 else fcp[i+4]
                break
            i += 2 + tlen
    data, sw1, sw2 = _apdu(conn, f'00B20104{rec_len:02X}')
    if sw1 != 0x90:
        return None
    addr_len = data[-14]
    if addr_len in (0xFF, 0x00):
        return None
    ton = data[-13]
    num_digits = addr_len - 1
    digits_hex = toHexString(data[-12:-12 + num_digits]).replace(' ', '')
    number = _swap(digits_hex).rstrip('fF')
    return ('+' if (ton & 0x70) == 0x10 else '') + number

try:
    conn    = connect(reader_index)
    aid     = select_aid(conn)
    if aid:
        reselect_aid(conn, aid)

    iccid   = read_iccid(conn)
    if aid:
        reselect_aid(conn, aid)     # ICCID select leaves us in MF

    imsi    = read_imsi(conn)
    mnc_len = read_mnc_length(conn)
    if aid:
        reselect_aid(conn, aid)
    msisdn  = read_msisdn(conn)

    mcc = imsi[:3]
    mnc = imsi[3:3 + mnc_len].zfill(3)   # always 3-digit zero-padded per 3GPP
    # Normalize MSISDN: some carriers (e.g. T-Mobile US) store with TON=national
    # so the + is not added by the TON check; prepend it for E.164 VoLTE use.
    if msisdn and not msisdn.startswith('+'):
        msisdn = '+' + msisdn

    print(json.dumps({'iccid': iccid, 'imsi': imsi, 'msisdn': msisdn,
                      'mcc': mcc, 'mnc': mnc}))
except Exception as e:
    print(json.dumps({'error': str(e)}), file=sys.stderr)
    sys.exit(1)
"""


# ─── SQLite inventory ────────────────────────────────────────────────────────

def db_save(reader_index: int, sim: dict):
    """Upsert one row per reader into sim_inventory.db."""
    con = sqlite3.connect(DB_PATH)
    con.execute("""
        CREATE TABLE IF NOT EXISTS sims (
            reader      INTEGER PRIMARY KEY,
            iccid       TEXT,
            imsi        TEXT,
            msisdn      TEXT,
            mcc         TEXT,
            mnc         TEXT,
            updated_at  TEXT
        )
    """)
    con.execute("""
        INSERT INTO sims (reader, iccid, imsi, msisdn, mcc, mnc, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(reader) DO UPDATE SET
            iccid=excluded.iccid, imsi=excluded.imsi, msisdn=excluded.msisdn,
            mcc=excluded.mcc, mnc=excluded.mnc, updated_at=excluded.updated_at
    """, (
        reader_index,
        sim.get('iccid'), sim.get('imsi'), sim.get('msisdn'),
        sim.get('mcc'),   sim.get('mnc'),
        datetime.now().strftime('%Y-%m-%d %H:%M:%S'),
    ))
    con.commit()
    con.close()
    print(f"  DB: P{reader_index} -> {DB_PATH.name}")


# ─── SIM reading via docker compose exec ─────────────────────────────────────

def read_sim(reader_index: int) -> dict:
    """Run APDU reader inside the asterisk container; return parsed dict."""
    result = subprocess.run(
        # -T disables TTY; reader index passed as argument to the -c script
        ["docker", "compose", "exec", "-T", READ_CONTAINER,
         "python3", "-c", _READER_SCRIPT, str(reader_index)],
        cwd=SCRIPT_DIR,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        stderr = result.stderr.strip()
        raise RuntimeError(
            f"docker compose exec failed (is '{READ_CONTAINER}' running?)\n{stderr}"
        )
    try:
        data = json.loads(result.stdout.strip())
    except json.JSONDecodeError:
        raise RuntimeError(f"Unexpected output from container:\n{result.stdout}")
    if 'error' in data:
        raise RuntimeError(data['error'])
    return data


def list_readers():
    """List all PC/SC readers visible inside the asterisk container."""
    script = (
        "from smartcard.System import readers\n"
        "r = readers()\n"
        "print(f'{len(r)} reader(s) found')\n"
        "for i, rd in enumerate(r): print(f'  P{i}: {rd}')\n"
    )
    result = subprocess.run(
        ["docker", "compose", "exec", "-T", READ_CONTAINER, "python3", "-c", script],
        cwd=SCRIPT_DIR, capture_output=True, text=True,
    )
    if result.returncode != 0:
        print(f"Error: {result.stderr.strip()}", file=sys.stderr)
        print(f"(Is the '{READ_CONTAINER}' container running?)", file=sys.stderr)
        return

    for line in result.stdout.strip().splitlines():
        print(line)
        # Try to read SIM for each detected reader index
        if line.strip().startswith('P') and ':' in line:
            try:
                idx = int(line.strip()[1])
            except (ValueError, IndexError):
                continue
            try:
                sim = read_sim(idx)
                print(f"      ICCID:   {sim['iccid']}")
                print(f"      IMSI:    {sim['imsi']}")
                print(f"      MSISDN:  {sim['msisdn'] or '(not stored on card)'}")
                print(f"      MCC/MNC: {sim['mcc']}/{sim['mnc']}")
                db_save(idx, sim)
            except Exception as e:
                print(f"      (error reading SIM: {e})")


# ─── Config file updaters ─────────────────────────────────────────────────────

def update_ami_usim(path: Path, sim: dict):
    txt = path.read_text()
    txt = re.sub(r'^(reader\s*=\s*imsi:).*$',
                 rf'\g<1>{sim["imsi"]}', txt, flags=re.MULTILINE)
    path.write_text(txt)
    print(f"  ami_usim.ini  reader=imsi:{sim['imsi']}")


def update_epdg(path: Path, sim: dict):
    mcc, mnc, imsi = sim['mcc'], sim['mnc'], sim['imsi']
    txt = path.read_text()
    txt = re.sub(
        r'(remote_addrs\s*=\s*)epdg\.epc\.mnc\d+\.mcc\d+\.pub\.3gppnetwork\.org',
        rf'\g<1>epdg.epc.mnc{mnc}.mcc{mcc}.pub.3gppnetwork.org', txt)
    txt = re.sub(
        r'(id\s*=\s*)0\d+@nai\.epc\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>0{imsi}@nai.epc.mnc{mnc}.mcc{mcc}.3gppnetwork.org', txt)
    path.write_text(txt)
    print(f"  epdg.conf     ePDG=mnc{mnc}.mcc{mcc}, NAI=0{imsi}")


def update_pjsip(path: Path, sim: dict):
    mcc, mnc, imsi = sim['mcc'], sim['mnc'], sim['imsi']
    msisdn = sim['msisdn']
    domain = f"ims.mnc{mnc}.mcc{mcc}.3gppnetwork.org"
    txt = path.read_text()

    if msisdn:
        # callerid=undefined <+18165537217>
        txt = re.sub(r'(callerid\s*=\s*\S+\s*<)[^>]+(>)',
                     rf'\g<1>{msisdn}\2', txt)
        # from_user=+18165537217
        txt = re.sub(r'^(from_user\s*=\s*)\S+',
                     rf'\g<1>{msisdn}', txt, flags=re.MULTILINE)
    else:
        print("  [warn] MSISDN not on card — callerid/from_user unchanged")

    # server_uri=sip:ims.mncXXX.mccYYY.3gppnetwork.org
    txt = re.sub(
        r'(server_uri\s*=\s*sip:)ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{domain}', txt)
    # client_uri=sip:IMSI@ims.mncXXX.mccYYY.3gppnetwork.org
    txt = re.sub(
        r'(client_uri\s*=\s*sip:)\d+@ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{imsi}@{domain}', txt)
    # from_domain=ims.mncXXX.mccYYY.3gppnetwork.org
    txt = re.sub(
        r'^(from_domain\s*=\s*)ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{domain}', txt, flags=re.MULTILINE)
    # smsc_uri — keep SMSC number, update domain only
    txt = re.sub(
        r'(smsc_uri\s*=\s*sip:[^@]+@)ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{domain}', txt)
    # auth username=IMSI@domain
    txt = re.sub(
        r'^(username\s*=\s*)\d+@ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{imsi}@{domain}', txt, flags=re.MULTILINE)
    # aor contact=sip:IMSI@domain
    txt = re.sub(
        r'(contact\s*=\s*sip:)\d+@ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{imsi}@{domain}', txt)
    # section header [ims.mncXXX.mccYYY.3gppnetwork.org]
    txt = re.sub(r'\[ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org\]',
                 f'[{domain}]', txt)
    # section header [smsoip.ims.mncXXX.mccYYY.3gppnetwork.org]
    txt = re.sub(r'\[smsoip\.ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org\]',
                 f'[smsoip.{domain}]', txt)

    path.write_text(txt)
    print(f"  pjsip.conf    IMSI={imsi}, MSISDN={msisdn or '(unchanged)'}, domain={domain}")


# ─── Orchestration ────────────────────────────────────────────────────────────

def update_instance(instance: int, sim: dict) -> bool:
    d = CONFIG_DIR / str(instance)
    if not d.exists():
        print(f"Error: {d} not found", file=sys.stderr)
        return False
    print(f"\nInstance {instance}:")
    update_ami_usim(d / "ami_usim.ini", sim)
    update_epdg(d / "epdg.conf", sim)
    update_pjsip(d / "asterisk" / "pjsip.conf", sim)
    return True


def restart_instance(instance: int):
    svc = "asterisk" if instance == 1 else f"asterisk{instance}"
    print(f"Restarting {svc}...")
    r = subprocess.run(
        ["docker", "compose", "restart", svc],
        cwd=SCRIPT_DIR, capture_output=True, text=True,
    )
    if r.returncode == 0:
        print(f"  {svc} restarted OK")
    else:
        print(f"  Error: {r.stderr.strip()}", file=sys.stderr)


# ─── Watch loop ──────────────────────────────────────────────────────────────

def watch_loop(interval: int):
    """Poll both readers every `interval` seconds; reconfigure on ICCID change."""
    # reader index -> last seen ICCID (None = empty/error)
    last_iccid = {0: None, 1: None}
    # Seed with current state so first poll doesn't trigger spurious restarts
    print(f"[watch] Starting — polling every {interval}s. Press Ctrl+C to stop.")
    for i in range(2):
        try:
            sim = read_sim(i)
            last_iccid[i] = sim['iccid']
            print(f"[watch] P{i}: initial ICCID={sim['iccid']} IMSI={sim['imsi']}")
        except Exception:
            print(f"[watch] P{i}: no card")

    while True:
        time.sleep(interval)
        for i in range(2):
            instance = i + 1
            try:
                sim = read_sim(i)
                iccid = sim['iccid']
            except Exception:
                if last_iccid[i] is not None:
                    print(f"[watch] P{i}: card removed")
                    last_iccid[i] = None
                continue

            if iccid != last_iccid[i]:
                ts = datetime.now().strftime('%H:%M:%S')
                print(f"[{ts}] P{i}: ICCID changed "
                      f"{last_iccid[i]} -> {iccid} — reconfiguring instance {instance}")
                last_iccid[i] = iccid
                db_save(i, sim)
                if update_instance(instance, sim):
                    restart_instance(instance)


# ─── CLI ──────────────────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser(
        description="Read SIM via pcscd-in-Docker and update asterisk-docker configs",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  %(prog)s --list\n"
            "  %(prog)s --instance 1 --reader 0 --restart\n"
            "  %(prog)s --instance 2 --reader 1 --restart\n"
            "  %(prog)s --auto --restart\n"
            "  %(prog)s --instance 1 --reader 0 --msisdn +18165551234 --restart\n"
        ),
    )
    ap.add_argument('--list',     action='store_true',
                    help='List PC/SC readers and SIM info')
    ap.add_argument('--instance', type=int, choices=[1, 2],
                    help='Config instance to update')
    ap.add_argument('--reader',   type=int, default=None,
                    help='PC/SC reader index (default: instance-1, i.e. 0 for instance 1)')
    ap.add_argument('--msisdn',   type=str, default=None,
                    help='Override MSISDN if not stored on card (e.g. +18165551234)')
    ap.add_argument('--auto',     action='store_true',
                    help='Auto-assign readers to instances (reader 0->1, reader 1->2)')
    ap.add_argument('--restart',  action='store_true',
                    help='Restart Docker container after updating config')
    ap.add_argument('--watch',    action='store_true',
                    help='Monitor readers and reconfigure automatically on SIM swap')
    ap.add_argument('--interval', type=int, default=5, metavar='SEC',
                    help='Polling interval for --watch in seconds (default: 5)')
    args = ap.parse_args()

    if args.list:
        list_readers()
        return

    if args.watch:
        watch_loop(args.interval)
        return

    if args.auto:
        for i in range(2):
            instance = i + 1
            print(f"\n--- Reader P{i} -> Instance {instance} ---")
            try:
                sim = read_sim(i)
                if args.msisdn:
                    sim['msisdn'] = args.msisdn
                print(f"  IMSI={sim['imsi']}  MSISDN={sim['msisdn'] or '(none)'}")
                db_save(i, sim)
                if update_instance(instance, sim) and args.restart:
                    restart_instance(instance)
            except Exception as e:
                print(f"  Skipping instance {instance}: {e}", file=sys.stderr)
        return

    if args.instance is None:
        ap.print_help()
        sys.exit(1)

    idx = args.reader if args.reader is not None else (args.instance - 1)
    print(f"Reading SIM from P{idx} (via '{READ_CONTAINER}' container)...")
    try:
        sim = read_sim(idx)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)

    if args.msisdn:
        sim['msisdn'] = args.msisdn

    print(f"  ICCID:   {sim['iccid']}")
    print(f"  IMSI:    {sim['imsi']}")
    print(f"  MSISDN:  {sim['msisdn'] or '(not stored on card)'}")
    print(f"  MCC/MNC: {sim['mcc']}/{sim['mnc']}")
    db_save(idx, sim)

    if update_instance(args.instance, sim) and args.restart:
        restart_instance(args.instance)


if __name__ == '__main__':
    main()
