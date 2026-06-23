#!/usr/bin/env python3
"""
start_sim_container_config.py - Bootstrap asterisk-docker from devices.toml.

Workflow:
  1. Read devices.toml -> get reader-to-container mapping with IMEI
  2. Generate config/<N>/ directories from config/example/ with placeholder
     "000" values + real IMEI from devices.toml
  3. Generate compose.yaml with one asterisk service per reader
  4. Start pcscd + asterisk-1 as a SIM probe (asterisk-1 has pyscard)
  5. Read SIM cards from each reader via APDU
  6. For each reader:
       - SIM present -> write real config, start that instance (force-recreate
         asterisk-1 if P0 has a card so it picks up the real config)
       - SIM absent  -> ensure that instance is NOT running (skip startup or
         stop it cleanly with IMS unregister)
  7. Optionally --watch for SIM hotswap (auto start/stop on insert/remove)

Containers without a SIM are never started, so the carrier is never asked to
authenticate a placeholder IMSI (which would always be rejected).

Usage:
  ./start_sim --setup               # Steps 1-6: full bootstrap
  ./start_sim --setup --watch       # Steps 1-6, then monitor
  ./start_sim --list                # List readers & SIM info
  ./start_sim --watch               # Monitor only (containers already running)
"""

import argparse
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import time

try:
    import tomllib                       # Python 3.11+
except ImportError:
    try:
        import tomli as tomllib          # pip install tomli for 3.10-
    except ImportError:
        tomllib = None

from datetime import datetime
from pathlib import Path

SCRIPT_DIR   = Path(__file__).parent     # asterisk-docker/
CONFIG_DIR   = SCRIPT_DIR / "config"
EXAMPLE_DIR  = CONFIG_DIR / "example"
DB_PATH      = SCRIPT_DIR / "sim_inventory.db"
DEVICES_TOML = SCRIPT_DIR / "devices.toml"
COMPOSE_FILE = SCRIPT_DIR / "compose.yaml"

# Container used for APDU reading — must be running, has pyscard + pcsc-sock
# This is resolved dynamically at runtime; see get_read_container()
READ_CONTAINER = "asterisk"

# ─── Embedded reader script (runs inside container via docker compose exec) ───
_READER_SCRIPT = r"""
import json, sys
from smartcard.System import readers
from smartcard.util import toHexString, toBytes
from smartcard.CardConnection import CardConnection

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
    data, sw1, sw2 = _apdu(conn, '00A40004026FAD')
    if sw1 == 0x61:
        data, sw1, sw2 = _apdu(conn, f'00C00000{sw2:02X}')
    if sw1 != 0x90:
        return 2
    data, sw1, sw2 = _apdu(conn, '00B0000004')
    if sw1 != 0x90 or len(data) < 4:
        return 2
    mnc_len = data[3] & 0x0F
    return 3 if mnc_len == 3 else 2

def read_msisdn(conn, aid=None):
    # Always re-select AID before reading MSISDN to guarantee USIM context
    if aid:
        reselect_aid(conn, aid)
    data, sw1, sw2 = _apdu(conn, '00A40004026F40')
    fcp = None
    if sw1 == 0x61:
        fcp, sw1, sw2 = _apdu(conn, f'00C00000{sw2:02X}')
    if sw1 != 0x90:
        return None
    rec_len = 28
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
    if len(data) < 14:
        return None
    addr_len = data[-14]
    if addr_len in (0xFF, 0x00):
        return None
    ton = data[-13]
    num_digits = addr_len - 1
    digits_hex = toHexString(data[-12:-12 + num_digits]).replace(' ', '')
    number = _swap(digits_hex).rstrip('fF')
    # Add '+' for international (TON=001, i.e. 0x91) or if number starts with country code
    if (ton & 0x70) == 0x10:
        return '+' + number
    return number

try:
    conn    = connect(reader_index)
    aid     = select_aid(conn)
    if aid:
        reselect_aid(conn, aid)

    iccid   = read_iccid(conn)
    if aid:
        reselect_aid(conn, aid)

    imsi    = read_imsi(conn)
    mnc_len = read_mnc_length(conn)
    msisdn  = read_msisdn(conn, aid)

    if not imsi:
        raise RuntimeError("Failed to read IMSI from card")
    if not iccid:
        raise RuntimeError("Failed to read ICCID from card")

    mcc = imsi[:3]
    mnc = imsi[3:3 + mnc_len].zfill(3)
    if msisdn and not msisdn.startswith('+'):
        msisdn = '+' + msisdn

    print(json.dumps({'iccid': iccid, 'imsi': imsi, 'msisdn': msisdn,
                      'mcc': mcc, 'mnc': mnc}))
except Exception as e:
    print(json.dumps({'error': str(e)}), file=sys.stderr)
    try:
        conn.reconnect(disposition=CardConnection.RESET)
        conn.disconnect()
    except Exception:
        pass
    sys.exit(1)
finally:
    # Power-cycle the card via pcscd so the next consumer (ami_usim.py inside
    # the container) gets a clean SIM state. Without this, the card is left
    # with the last SELECT (EF.MSISDN under ADF.USIM) still active, which
    # causes IMS-AKA in ami_usim.py to fail and the carrier to reject
    # registration — only fixed by physically re-inserting the SIM.
    try:
        conn.reconnect(disposition=CardConnection.RESET)
        conn.disconnect()
    except Exception:
        pass
"""


# ─── devices.toml parser ─────────────────────────────────────────────────────

def parse_devices_toml():
    """Parse devices.toml -> list of {'reader': 0, 'hostname': 'asterisk1', 'imei': '...'}."""
    if tomllib is None:
        raise RuntimeError("Need Python 3.11+ or 'pip install tomli' for TOML support")
    with open(DEVICES_TOML, 'rb') as f:
        data = tomllib.load(f)
    devices = []
    for entry in data.get('readers', []):
        for key, hostname in entry.items():
            if key == 'IMEI':
                continue
            if key.startswith('P'):
                reader_idx = int(key[1:])
                devices.append({
                    'reader':   reader_idx,
                    'hostname': hostname,
                    'imei':     str(entry.get('IMEI', '')),
                })
    devices.sort(key=lambda d: d['reader'])
    return devices


# ─── SQLite inventory ────────────────────────────────────────────────────────

def db_init():
    """Initialize database schema for readers and SIMs."""
    con = sqlite3.connect(DB_PATH)
    con.execute("""
        CREATE TABLE IF NOT EXISTS readers (
            reader      INTEGER PRIMARY KEY,
            name        TEXT,
            hostname    TEXT,
            imei        TEXT,
            status      TEXT,
            last_seen   TEXT,
            updated_at  TEXT
        )
    """)
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
    con.commit()
    con.close()


def db_save_reader(reader_index, reader_name, status,
                   hostname='', imei=''):
    """Update reader availability in database."""
    now = datetime.now().strftime('%Y-%m-%d %H:%M:%S')
    con = sqlite3.connect(DB_PATH)
    con.execute("""
        INSERT INTO readers (reader, name, hostname, imei, status, last_seen, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(reader) DO UPDATE SET
            name=excluded.name, hostname=excluded.hostname, imei=excluded.imei,
            status=excluded.status, last_seen=excluded.last_seen,
            updated_at=excluded.updated_at
    """, (reader_index, reader_name, hostname, imei, status, now, now))
    con.commit()
    con.close()


def db_save_sim(reader_index, sim):
    """Upsert one SIM record per reader."""
    now = datetime.now().strftime('%Y-%m-%d %H:%M:%S')
    con = sqlite3.connect(DB_PATH)
    con.execute("""
        INSERT INTO sims (reader, iccid, imsi, msisdn, mcc, mnc, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(reader) DO UPDATE SET
            iccid=excluded.iccid, imsi=excluded.imsi, msisdn=excluded.msisdn,
            mcc=excluded.mcc, mnc=excluded.mnc, updated_at=excluded.updated_at
    """, (reader_index, sim.get('iccid'), sim.get('imsi'), sim.get('msisdn'),
          sim.get('mcc'), sim.get('mnc'), now))
    con.commit()
    con.close()


# ─── Config generation from example template ─────────────────────────────────

def generate_config(instance, device):
    """
    Create config/<instance>/ from config/example/ with placeholder 000 values
    and the real IMEI from devices.toml.
    """
    dest = CONFIG_DIR / str(instance)
    if dest.exists():
        print(f"  config/{instance}/ already exists — skipping generation")
        return

    print(f"  Generating config/{instance}/ from example template...")
    shutil.copytree(str(EXAMPLE_DIR), str(dest))

    imei = device['imei']
    # Format IMEI for 3GPP: XXXXXXXX-XXXXXX-X
    imei_fmt = imei
    if len(imei) >= 15 and '-' not in imei:
        imei_fmt = f"{imei[:8]}-{imei[8:14]}-{imei[14:]}"

    # ── ami_usim.ini: placeholder IMSI
    (dest / "ami_usim.ini").write_text(
        "[volte_ims]\n"
        "reader=imsi:000000000000000\n"
        "host=127.0.0.1\n"
        "username=jolly\n"
        "secret=geheim\n"
    )

    hostname = device['hostname']

    # ── epdg.conf: placeholder MCC/MNC
    (dest / "epdg.conf").write_text(
        "connections {\n"
        "   ims {\n"
        f"      local_addrs  = {hostname}\n"
        "      remote_addrs = epdg.epc.mnc000.mcc000.pub.3gppnetwork.org\n"
        "      vips = ::\n"
        "\n"
        "      local {\n"
        "         auth = eap-aka\n"
        "         id = 0000000000000000@nai.epc.mnc000.mcc000.3gppnetwork.org\n"
        "      }\n"
        "      remote {\n"
        "         id = ims\n"
        "      }\n"
        "      children {\n"
        "         ims {\n"
        "            remote_ts = ::/0\n"
        "            updown = /usr/local/etc/ims.updown\n"
        "            close_action = start\n"
        "            if_id_in = 23\n"
        "            if_id_out = 23\n"
        "         }\n"
        "      }\n"
        "      version = 2\n"
        "   }\n"
        "}\n"
    )

    # ── pjsip.conf: placeholder values + real IMEI
    (dest / "asterisk" / "pjsip.conf").write_text(
        "[global]\n"
        "type=global\n"
        "allow_sending_180_after_183=yes\n"
        "\n"
        "[system]\n"
        "type=system\n"
        "timer_t1=2000\n"
        "\n"
        "; ====== SIP listener\n"
        "[transport-udp-sip]\n"
        "type=transport\n"
        "protocol=udp\n"
        "bind=0.0.0.0:5060\n"
        "local_net=172.18.0.0/16\n"
        "\n"
        "\n"
        ";===============ENDPOINT TEMPLATES\n"
        "\n"
        "[endpoint-basic-sip](!)\n"
        "type=endpoint\n"
        "allow=all\n"
        "media_encryption=no\n"
        "rtp_symmetric=yes\n"
        "force_rport=yes\n"
        "rewrite_contact=yes\n"
        "\n"
        "[auth-userpass-sip](!)\n"
        "type=auth\n"
        "auth_type=userpass\n"
        "\n"
        "[aor-normal-sip](!)\n"
        "type=aor\n"
        "max_contacts=3\n"
        "\n"
        ";===============EXTENSION 6000\n"
        "\n"
        "[6000](endpoint-basic-sip)\n"
        "auth=auth-sip\n"
        "aors=6000\n"
        "callerid=undefined <+000>\n"
        "context=from-sip\n"
        "message_context = msg-from-sip\n"
        "\n"
        "[auth-sip](auth-userpass-sip)\n"
        "password=123456\n"
        "username=6000\n"
        "\n"
        "\n"
        "[6000](aor-normal-sip)\n"
        "\n"
        ";===============VoLTE\n"
        "\n"
        "[volte_ims]\n"
        "type=transport\n"
        "protocol=tcp\n"
        "bind=[::]:5060\n"
        "bind_interface=ipsec0\n"
        "sec_port_c_min=40000\n"
        "sec_port_c_max=44999\n"
        "sec_port_s_min=50000\n"
        "sec_port_s_max=54999\n"
        "p_access_network_info=IEEE-802.11\\;i-wlan-node-id=mywifi\n"
        "sec_encryption=yes\n"
        "\n"
        "[volte_ims]\n"
        "type=registration\n"
        "transport=volte_ims\n"
        "outbound_auth=volte_ims\n"
        f"imei={imei_fmt}\n"
        "server_uri=sip:ims.mnc000.mcc000.3gppnetwork.org\n"
        "client_uri=sip:000000000000000@ims.mnc000.mcc000.3gppnetwork.org\n"
        "retry_interval=30\n"
        "fatal_retry_interval=30\n"
        "max_retries=999999999\n"
        "expiration=600000\n"
        "volte=yes\n"
        "manual_register=yes\n"
        "endpoint=volte_ims\n"
        "receive_sms=yes\n"
        "\n"
        "[volte_ims]\n"
        "type=endpoint\n"
        "user_eq_phone=on\n"
        "transport=volte_ims\n"
        "context=volte_ims\n"
        "message_context=volte_ims_msg\n"
        "disallow=all\n"
        "allow=amr\n"
        "bw_value=41\n"
        "outbound_auth=volte_ims\n"
        "aors=volte_ims\n"
        "rewrite_contact=yes\n"
        "from_domain=ims.mnc000.mcc000.3gppnetwork.org\n"
        "from_user=+000\n"
        "volte=yes\n"
        "dedicated_bearer_up=yes\n"
        "100rel=peer_supported\n"
        "moh_passthrough=yes\n"
        "direct_media=no\n"
        "smsc_uri=sip:+000@ims.mnc000.mcc000.3gppnetwork.org\n"
        "\n"
        "[volte_ims]\n"
        "type=auth\n"
        "auth_type=ims_aka\n"
        "username=000000000000000@ims.mnc000.mcc000.3gppnetwork.org\n"
        "usim_ami=yes\n"
        "\n"
        "[volte_ims]\n"
        "type=aor\n"
        "contact=sip:000000000000000@ims.mnc000.mcc000.3gppnetwork.org\n"
        "max_contacts=1\n"
        "\n"
        "[volte_ims]\n"
        "type=identify\n"
        "endpoint=volte_ims\n"
        "match=::1\n"
        "\n"
        "[ims.mnc000.mcc000.3gppnetwork.org]\n"
        "type=resolve\n"
        "ip=::1\n"
        "transport=volte_ims\n"
        "\n"
        "[smsoip.ims.mnc000.mcc000.3gppnetwork.org]\n"
        "type=resolve\n"
        "ip=::1\n"
        "transport=volte_ims\n"
    )

    # ── RTP port range: offset per instance to avoid conflicts
    rtp_start = 10000 + (instance - 1) * 10
    rtp_end   = rtp_start + 9
    (dest / "asterisk" / "rtp.conf").write_text(
        f"[general]\nrtpstart={rtp_start}\nrtpend={rtp_end}\n"
    )

    # ── telegram_token: keep placeholder if example has none
    tok = dest / "telegram_token"
    if tok.read_text().strip() == "telegram_token":
        tok.write_text("telegram_token\n")

    print(f"  config/{instance}/ created  (IMEI={imei_fmt}, RTP={rtp_start}-{rtp_end})")


# ─── compose.yaml generation ─────────────────────────────────────────────────

def generate_compose(devices):
    """Generate compose.yaml with pcscd + one asterisk service per reader."""
    lines = [
        "services:",
        "  pcscd:",
        "    build:",
        "      context: ./pcscd",
        "      dockerfile: Dockerfile",
        "    image: ghcr.io/chuckleu1976/pcscd-sysmocom:latest",
        "    restart: always",
        "    privileged: true",
        "    environment:",
        "      - LD_LIBRARY_PATH=/usr/lib64/remsim-libs",
        "      - SIM_MODE=${SIM_MODE:-local}",
        "    volumes:",
        "      - ./pcscd/entrypoint.sh:/entrypoint.sh:ro",
        "      - pcsc-sock:/run/pcscd",
        "      - ./remsim/libs:/usr/lib64/remsim-libs:ro",
        "      - ./remsim/serial:/usr/lib64/pcsc/drivers/serial:ro",
        "      - ./remsim/reader.conf.d:/etc/reader.conf.d:ro",
        "    tmpfs:",
        "      - /run",
        "    devices:",
        "      - /dev/bus/usb:/dev/bus/usb",
    ]

    for dev in devices:
        idx      = dev['reader']
        instance = idx + 1
        hostname = dev['hostname']
        svc      = "asterisk" if instance == 1 else f"asterisk{instance}"
        sip_port = 5060 + (instance - 1) * 2
        rtp_s    = 10000 + (instance - 1) * 10
        rtp_e    = rtp_s + 9
        ami_port = 5038 + (instance - 1)  # 5038..5045 on host loopback

        lines += [
            f"  {svc}:",
            f"    image: ghcr.io/chuckleu1976/asterisk-vowifi-patched:latest",
            f"    hostname: {hostname}",
            f"    build:",
            f"      context: ./asterisk",
            f"      dockerfile: Dockerfile",
            f"      network: host",
            f"    cap_add:",
            f"      - CAP_NET_ADMIN",
            f"    privileged: true",
            f"    devices:",
            f"      - /dev/net/tun",
            f"      - /dev/bus/usb:/dev/bus/usb",
            f"    ports:",
            f"      - {sip_port}:5060/udp",
            f"      - {rtp_s}-{rtp_e}:{rtp_s}-{rtp_e}/udp",
            f"      - 127.0.0.1:{ami_port}:5038/tcp",
            f"    tmpfs:",
            f"      - /run",
            f"    environment:",
            f"      - LD_LIBRARY_PATH=/opt/pcsc-libs",
            f"    volumes:",
            f"      - ./pcsc-libs:/opt/pcsc-libs:ro",
            f"      - ./config/{instance}/epdg.conf:/usr/local/etc/swanctl/conf.d/epdg.conf:Z,ro",
            f"      - ./config/{instance}/asterisk:/etc/asterisk:Z",
            f"      - ./config/{instance}/ami_usim.ini:/usr/local/etc/ami_usim.ini:Z,ro",
            f"      - ./config/{instance}/telegram_token:/usr/local/etc/telegram_token:Z,ro",
            f"      - ../logs/{instance}:/logs:Z",
            f"      - ./sim_inventory.db:/data/sim_inventory.db:Z",
            f"      - pcsc-sock:/run/pcscd",
            f"    pid: service:pcscd",
            f"    depends_on: [pcscd]",
            f"    restart: always",
        ]

    lines += ["", "volumes:", "  pcsc-sock:", ""]
    COMPOSE_FILE.write_text('\n'.join(lines))
    print(f"  compose.yaml written ({len(devices)} asterisk service(s))")


# ─── Docker helpers ───────────────────────────────────────────────────────────

def docker_compose(*args, timeout=None):
    """Run docker compose; fall back to sudo if permission denied."""
    cmd = ["docker", "compose"] + list(args)
    try:
        r = subprocess.run(cmd, cwd=SCRIPT_DIR, capture_output=True, text=True,
                           stdin=subprocess.DEVNULL, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise
    if r.returncode != 0 and "permission denied" in r.stderr.lower():
        cmd = ["sudo", "docker", "compose"] + list(args)
        try:
            r = subprocess.run(cmd, cwd=SCRIPT_DIR, capture_output=True, text=True,
                               stdin=subprocess.DEVNULL, timeout=timeout)
        except subprocess.TimeoutExpired:
            raise
    return r


def start_containers():
    """Start all services in background (used only when caller wants everything)."""
    print("Starting containers...")
    r = docker_compose("up", "-d")
    if r.returncode == 0:
        print("  All containers started")
    else:
        print(f"  stdout: {r.stdout.strip()}")
        print(f"  stderr: {r.stderr.strip()}", file=sys.stderr)
    print("  Waiting for containers to initialize...", end="", flush=True)
    time.sleep(5)
    print(" OK")


def _is_service_running(svc):
    r = docker_compose("ps", "--format", "{{.Service}}\t{{.State}}", timeout=10)
    if r.returncode != 0:
        return False
    for line in r.stdout.splitlines():
        parts = line.strip().split('\t')
        if len(parts) == 2 and parts[0] == svc and parts[1] == 'running':
            return True
    return False


def start_probe():
    """Start pcscd + asterisk-1 as a SIM probe.  asterisk-1 has python3 +
    pyscard and shares pcsc-sock with pcscd, so it can enumerate readers and
    read APDUs.  We need this BEFORE we know which readers have SIMs.
    """
    print("Starting pcscd + asterisk-1 (SIM probe)...")
    r = docker_compose("up", "-d", "pcscd", READ_CONTAINER)
    if r.returncode != 0:
        print(f"  Error: {r.stderr.strip()}", file=sys.stderr)
    else:
        print("  OK")
    # pcscd needs time to enumerate USB readers and let cards settle into a
    # readable state.  With 8 readers a short wait can race the first card
    # read; wait long enough that subsequent APDU reads are reliable.
    print("  Waiting for pcscd + readers to settle...", end="", flush=True)
    time.sleep(12)
    print(" OK")


def restart_instance(instance):
    svc = "asterisk" if instance == 1 else f"asterisk{instance}"
    print(f"  Restarting {svc}...", end="", flush=True)
    r = docker_compose("restart", svc)
    print(" OK" if r.returncode == 0 else f" Error: {r.stderr.strip()}")


def stop_instance(instance):
    svc = "asterisk" if instance == 1 else f"asterisk{instance}"
    if not _is_service_running(svc):
        # Not running — nothing to do.  Avoid the unregister+sleep when the
        # container was never started (common during --setup for empty readers).
        return
    # Try to release the IMS registration first so the carrier frees the
    # binding for this IMSI; otherwise a later container starting with the
    # same IMSI will get its REGISTER silently dropped (408 timeout).
    print(f"  Unregistering {svc} from IMS...", end="", flush=True)
    r = docker_compose("exec", "-T", svc, "asterisk", "-rx",
                       "pjsip send unregister volte_ims", timeout=10)
    print(" OK" if r.returncode == 0 else " (skipped)")
    # Give the carrier a moment to process the de-registration before SIGTERM
    # tears down the IPsec tunnel.
    time.sleep(3)
    print(f"  Stopping {svc} (no SIM)...", end="", flush=True)
    r = docker_compose("stop", svc)
    print(" OK" if r.returncode == 0 else f" Error: {r.stderr.strip()}")


def _patch_instance(svc):
    """Copy patched ami_usim.py and ims.updown into a running container.

    docker compose up recreates containers from the image, discarding any
    previously copied fixes.  Call this after every start_instance().
    """
    for src, dst in [
        ("asterisk/ami_usim.py",    "/usr/local/bin/ami_usim.py"),
        ("asterisk/ims.updown",     "/usr/local/etc/ims.updown"),
        ("asterisk/entrypoint.sh",  "/entrypoint.sh"),
    ]:
        r = docker_compose("cp", src, f"{svc}:{dst}")
        if r.returncode != 0:
            print(f"    [warn] cp {src} -> {svc}:{dst}: {r.stderr.strip()}")


def start_instance(instance, force_recreate=False):
    svc = "asterisk" if instance == 1 else f"asterisk{instance}"
    args = ["up", "-d"]
    if force_recreate:
        args.append("--force-recreate")
    args.append(svc)
    print(f"  Starting {svc}{' (force-recreate)' if force_recreate else ''}...",
          end="", flush=True)
    r = docker_compose(*args)
    if r.returncode == 0:
        print(" OK")
        _patch_instance(svc)
    else:
        print(f" Error: {r.stderr.strip()}")


# ─── Reader enumeration ──────────────────────────────────────────────────────

def get_read_container():
    """Return the name of a running asterisk container for reader access.

    If no asterisk container is running, lazily start asterisk (instance 1)
    as a probe so reader enumeration and SIM reads continue to work even
    after all SIM-using instances have been stopped (e.g. user moved the
    last SIM card to a different reader slot).
    """
    r = docker_compose("ps", "--format", "{{.Service}}\t{{.State}}", timeout=10)
    if r.returncode == 0:
        for line in r.stdout.splitlines():
            parts = line.strip().split('\t')
            if len(parts) == 2:
                svc, state = parts
                if svc.startswith('asterisk') and state == 'running':
                    return svc
    # No asterisk running — start asterisk (instance 1) as a probe so the
    # watch loop can keep enumerating readers and detect SIMs in other slots.
    print(f"  [probe] No asterisk container running, starting {READ_CONTAINER} as probe...",
          flush=True)
    up = docker_compose("up", "-d", READ_CONTAINER, timeout=60)
    if up.returncode != 0:
        print(f"  [probe] Failed to start {READ_CONTAINER}: {up.stderr.strip()}")
        return READ_CONTAINER
    # Give the container a few seconds to bring up pcsc-lite client + python
    time.sleep(5)
    print(f"  [probe] {READ_CONTAINER} started", flush=True)
    return READ_CONTAINER


def enumerate_readers_in_container():
    """Return list of reader indices visible inside a running asterisk container."""
    script = "from smartcard.System import readers\nr=readers()\nprint(len(r))\n"
    container = get_read_container()
    try:
        r = docker_compose("exec", "-T", container, "python3", "-c", script,
                           timeout=30)
    except subprocess.TimeoutExpired:
        return []
    if r.returncode != 0:
        return []
    try:
        return list(range(int(r.stdout.strip())))
    except ValueError:
        return []


# ─── SIM reading ─────────────────────────────────────────────────────────────

def read_sim(reader_index, retries=1):
    """Run APDU reader inside a running asterisk container; return dict.

    Retries once on transient failures \u2014 freshly-started pcscd can return
    intermittent errors on the first APDU exchange to a given reader.
    """
    last_err = None
    for attempt in range(retries + 1):
        container = get_read_container()
        try:
            r = docker_compose("exec", "-T", container,
                               "python3", "-c", _READER_SCRIPT, str(reader_index),
                               timeout=30)
        except subprocess.TimeoutExpired:
            last_err = RuntimeError(f"Timed out reading reader P{reader_index}")
            if attempt < retries:
                time.sleep(2)
                continue
            raise last_err
        if r.returncode != 0:
            err_text = r.stderr.strip()
            try:
                err_data = json.loads(err_text)
                last_err = RuntimeError(err_data.get('error', err_text))
            except (json.JSONDecodeError, TypeError):
                last_err = RuntimeError(err_text or f"Container exec failed (rc={r.returncode})")
            # Only retry transient APDU failures, not "no card inserted".
            if attempt < retries and "no smart card" not in str(last_err).lower():
                time.sleep(2)
                continue
            raise last_err
        try:
            data = json.loads(r.stdout.strip())
        except json.JSONDecodeError:
            raise RuntimeError(f"Unexpected output from container:\n{r.stdout}")
        if 'error' in data:
            raise RuntimeError(data['error'])
        return data
    raise last_err  # unreachable


# ─── Config file updaters ─────────────────────────────────────────────────────

def update_ami_usim(path, sim):
    txt = path.read_text()
    txt = re.sub(r'^(reader\s*=\s*imsi:).*$',
                 rf'\g<1>{sim["imsi"]}', txt, flags=re.MULTILINE)
    path.write_text(txt)
    print(f"    ami_usim.ini  reader=imsi:{sim['imsi']}")


def update_epdg(path, sim, hostname=None):
    mcc, mnc, imsi = sim['mcc'], sim['mnc'], sim['imsi']
    txt = path.read_text()
    txt = re.sub(
        r'(remote_addrs\s*=\s*)epdg\.epc\.mnc\d+\.mcc\d+\.pub\.3gppnetwork\.org',
        rf'\g<1>epdg.epc.mnc{mnc}.mcc{mcc}.pub.3gppnetwork.org', txt)
    txt = re.sub(
        r'(id\s*=\s*)0\d+@nai\.epc\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>0{imsi}@nai.epc.mnc{mnc}.mcc{mcc}.3gppnetwork.org', txt)
    if hostname:
        txt = re.sub(r'(local_addrs\s*=\s*)\S+', rf'\g<1>{hostname}', txt)
    path.write_text(txt)
    print(f"    epdg.conf     ePDG=mnc{mnc}.mcc{mcc}, NAI=0{imsi}")


def update_pjsip(path, sim):
    mcc, mnc, imsi = sim['mcc'], sim['mnc'], sim['imsi']
    msisdn = sim['msisdn']
    domain = f"ims.mnc{mnc}.mcc{mcc}.3gppnetwork.org"
    txt = path.read_text()

    if msisdn:
        txt = re.sub(r'(callerid\s*=\s*\S+\s*<)[^>]+(>)',
                     rf'\g<1>{msisdn}\2', txt)
        txt = re.sub(r'^(from_user\s*=\s*)\S+',
                     rf'\g<1>{msisdn}', txt, flags=re.MULTILINE)
    else:
        print("    [warn] MSISDN not on card — callerid/from_user unchanged")

    txt = re.sub(
        r'(server_uri\s*=\s*sip:)ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{domain}', txt)
    txt = re.sub(
        r'(client_uri\s*=\s*sip:)\d+@ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{imsi}@{domain}', txt)
    txt = re.sub(
        r'^(from_domain\s*=\s*)ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{domain}', txt, flags=re.MULTILINE)
    txt = re.sub(
        r'(smsc_uri\s*=\s*sip:[^@]+@)ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{domain}', txt)
    txt = re.sub(
        r'^(username\s*=\s*)\d+@ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{imsi}@{domain}', txt, flags=re.MULTILINE)
    txt = re.sub(
        r'(contact\s*=\s*sip:)\d+@ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org',
        rf'\g<1>{imsi}@{domain}', txt)
    txt = re.sub(r'\[ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org\]',
                 f'[{domain}]', txt)
    txt = re.sub(r'\[smsoip\.ims\.mnc\d+\.mcc\d+\.3gppnetwork\.org\]',
                 f'[smsoip.{domain}]', txt)
    path.write_text(txt)
    print(f"    pjsip.conf    IMSI={imsi}, MSISDN={msisdn or '(unchanged)'}, domain={domain}")


def update_instance(instance, sim):
    d = CONFIG_DIR / str(instance)
    if not d.exists():
        print(f"  Error: config/{instance}/ not found", file=sys.stderr)
        return False
    # Determine hostname for local_addrs
    hostname = None
    try:
        dev_map = {dev['reader']: dev for dev in parse_devices_toml()}
        dev = dev_map.get(instance - 1)
        if dev:
            hostname = dev['hostname']
    except Exception:
        pass
    print(f"  Instance {instance} — writing SIM data:")
    update_ami_usim(d / "ami_usim.ini", sim)
    update_epdg(d / "epdg.conf", sim, hostname=hostname)
    update_pjsip(d / "asterisk" / "pjsip.conf", sim)
    return True


# ─── Setup: full bootstrap ────────────────────────────────────────────────────

def setup(do_watch=False, interval=5):
    """Full bootstrap: devices.toml -> config -> compose -> start -> SIM read -> reconfigure."""
    print("=" * 60)
    print("  SETUP: Bootstrap from devices.toml")
    print("=" * 60)

    # Step 1 — parse devices.toml
    print("\n[1/6] Reading devices.toml...")
    devices = parse_devices_toml()
    print(f"  Found {len(devices)} reader(s):")
    for dev in devices:
        print(f"    P{dev['reader']}: {dev['hostname']} (IMEI={dev['imei']})")

    # Step 2 — init DB and save readers
    print(f"\n[2/6] Initialising database ({DB_PATH.name})...")
    db_init()
    for dev in devices:
        db_save_reader(dev['reader'], f"P{dev['reader']}", "available",
                       hostname=dev['hostname'], imei=dev['imei'])
    print(f"  {len(devices)} reader(s) saved")

    # Step 3 — generate config directories
    print(f"\n[3/6] Generating config directories...")
    for dev in devices:
        generate_config(dev['reader'] + 1, dev)

    # Step 4 — generate compose.yaml
    print(f"\n[4/6] Generating compose.yaml...")
    generate_compose(devices)

    # Step 5 — create log dirs and start pcscd + probe container
    print(f"\n[5/6] Creating log directories and starting pcscd + probe...")
    for dev in devices:
        (SCRIPT_DIR.parent / "logs" / str(dev['reader'] + 1)).mkdir(
            parents=True, exist_ok=True)
    start_probe()

    # Step 6 — probe each reader; only start instances whose SIM is present
    print(f"\n[6/6] Probing SIMs and starting per-card containers...")
    sims_present = []   # list of (dev, sim)
    sims_absent  = []   # list of dev
    for dev in devices:
        idx = dev['reader']
        print(f"\n  P{idx} ({dev['hostname']}):")
        try:
            sim = read_sim(idx)
            print(f"    SIM: ICCID={sim['iccid']}  IMSI={sim['imsi']}  "
                  f"MSISDN={sim['msisdn'] or '(none)'}  MCC/MNC={sim['mcc']}/{sim['mnc']}")
            db_save_sim(idx, sim)
            db_save_reader(idx, f"P{idx}", "available",
                           hostname=dev['hostname'], imei=dev['imei'])
            sims_present.append((dev, sim))
        except Exception as e:
            print(f"    No SIM / read error: {e}")
            db_save_reader(idx, f"P{idx}", "empty",
                           hostname=dev['hostname'], imei=dev['imei'])
            sims_absent.append(dev)

    # Empty readers: ensure their instance is NOT running.  This also stops
    # the probe (asterisk-1) when P0 is empty, freeing carrier registration.
    for dev in sims_absent:
        stop_instance(dev['reader'] + 1)

    # Readers with a card: write real config, then (re)create the instance.
    # asterisk-1 was started as a probe with placeholder config; if P0 has a
    # card we force-recreate it so the real IMEI/IMSI/MCC are applied.
    for dev, sim in sims_present:
        instance = dev['reader'] + 1
        if update_instance(instance, sim):
            start_instance(instance, force_recreate=(instance == 1))

    print("\n" + "=" * 60)
    print(f"  SETUP COMPLETE: {len(sims_present)} container(s) started, "
          f"{len(sims_absent)} skipped (no SIM)")
    print("=" * 60)

    if do_watch:
        print()
        watch_loop(interval, devices)


# ─── List readers ─────────────────────────────────────────────────────────────

def list_readers():
    print("Detecting readers...")
    reader_indices = enumerate_readers_in_container()
    if not reader_indices:
        print(f"Error: No readers detected (is '{READ_CONTAINER}' running?)",
              file=sys.stderr)
        return

    dev_map = {}
    try:
        for dev in parse_devices_toml():
            dev_map[dev['reader']] = dev
    except Exception:
        pass

    print(f"{len(reader_indices)} reader(s) found:\n")
    for idx in reader_indices:
        dev      = dev_map.get(idx, {})
        hostname = dev.get('hostname', '?')
        imei     = dev.get('imei', '?')
        print(f"  P{idx} ({hostname}, IMEI={imei}):", end=" ")
        try:
            sim = read_sim(idx)
            print("SIM detected")
            print(f"      ICCID:   {sim['iccid']}")
            print(f"      IMSI:    {sim['imsi']}")
            print(f"      MSISDN:  {sim['msisdn'] or '(not stored on card)'}")
            print(f"      MCC/MNC: {sim['mcc']}/{sim['mnc']}")
            db_save_reader(idx, f"P{idx}", "available",
                           hostname=hostname, imei=imei)
            db_save_sim(idx, sim)
        except Exception as e:
            print(f"empty/error: {e}")
            db_save_reader(idx, f"P{idx}", "empty",
                           hostname=hostname, imei=imei)


# ─── Watch loop ───────────────────────────────────────────────────────────────

def watch_loop(interval, devices=None):
    """
    Poll every interval seconds.  Detect reader hotplug and SIM changes.
    Auto-reconfigure and restart containers when a SIM is inserted/changed.
    """
    db_init()

    dev_map = {}
    if devices is None:
        try:
            devices = parse_devices_toml()
        except Exception:
            devices = []
    for dev in devices:
        dev_map[dev['reader']] = dev

    last_iccid          = {}
    last_reader_indices = set()
    read_failures       = {}   # consecutive failure count per reader index
    REMOVE_THRESHOLD    = 3    # require this many consecutive failures before "card removed"

    print(f"[watch] Polling every {interval}s. Press Ctrl+C to stop.\n")

    while True:
        time.sleep(interval)

        reader_indices = set(enumerate_readers_in_container())

        # ── hotplug ──────────────────────────────────────────────────────────
        if reader_indices != last_reader_indices:
            ts      = datetime.now().strftime('%H:%M:%S')
            added   = reader_indices - last_reader_indices
            removed = last_reader_indices - reader_indices
            if added:
                print(f"[{ts}] Readers added:   {sorted(added)}")
                for idx in added:
                    dev = dev_map.get(idx, {})
                    db_save_reader(idx, f"P{idx}", "available",
                                   hostname=dev.get('hostname', ''),
                                   imei=dev.get('imei', ''))
                    last_iccid.setdefault(idx, None)
            if removed:
                print(f"[{ts}] Readers removed: {sorted(removed)}")
                for idx in removed:
                    dev = dev_map.get(idx, {})
                    db_save_reader(idx, f"P{idx}", "error",
                                   hostname=dev.get('hostname', ''),
                                   imei=dev.get('imei', ''))
                    last_iccid.pop(idx, None)
            last_reader_indices = reader_indices

        # ── poll SIM cards ───────────────────────────────────────────────────
        for i in reader_indices:
            instance = i + 1
            dev      = dev_map.get(i, {})
            last_iccid.setdefault(i, None)

            try:
                sim   = read_sim(i)
                iccid = sim['iccid']
                read_failures[i] = 0
                db_save_reader(i, f"P{i}", "available",
                               hostname=dev.get('hostname', ''),
                               imei=dev.get('imei', ''))
            except Exception:
                read_failures[i] = read_failures.get(i, 0) + 1
                if last_iccid[i] is not None and read_failures[i] >= REMOVE_THRESHOLD:
                    ts = datetime.now().strftime('%H:%M:%S')
                    print(f"[{ts}] P{i}: card removed")
                    db_save_reader(i, f"P{i}", "empty",
                                   hostname=dev.get('hostname', ''),
                                   imei=dev.get('imei', ''))
                    last_iccid[i] = None
                    stop_instance(instance)
                continue

            if iccid != last_iccid[i]:
                ts = datetime.now().strftime('%H:%M:%S')

                # Detect SIM relocation: same ICCID was previously seen on a
                # different reader.  Stop the old instance FIRST so the carrier
                # releases the IMS registration before the new instance tries
                # to re-register the same IMSI (otherwise the new REGISTER
                # gets silently dropped with a 408 timeout).
                moved_from = None
                for j, prev in last_iccid.items():
                    if j != i and prev == iccid:
                        moved_from = j
                        break

                if moved_from is not None:
                    print(f"[{ts}] P{i}: card moved from P{moved_from} "
                          f"ICCID={iccid}  IMSI={sim['imsi']}")
                    old_dev = dev_map.get(moved_from, {})
                    stop_instance(moved_from + 1)
                    last_iccid[moved_from]  = None
                    read_failures[moved_from] = 0
                    db_save_reader(moved_from, f"P{moved_from}", "empty",
                                   hostname=old_dev.get('hostname', ''),
                                   imei=old_dev.get('imei', ''))
                else:
                    action = "inserted" if last_iccid[i] is None else \
                             f"changed {last_iccid[i]} ->"
                    print(f"[{ts}] P{i}: card {action} ICCID={iccid}  IMSI={sim['imsi']}")

                last_iccid[i] = iccid
                db_save_sim(i, sim)
                if update_instance(instance, sim):
                    start_instance(instance)


# ─── CLI ──────────────────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser(
        description="Bootstrap asterisk-docker from devices.toml, read SIM cards, auto-reconfigure",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  %(prog)s --setup                 # Full bootstrap from devices.toml\n"
            "  %(prog)s --setup --watch          # Bootstrap then monitor\n"
            "  %(prog)s --list                   # List readers & SIMs\n"
            "  %(prog)s --watch --interval 5     # Monitor only\n"
            "  %(prog)s --instance 1 --reader 0  # Manual single-reader update\n"
        ),
    )
    ap.add_argument('--setup',    action='store_true',
                    help='Full bootstrap: devices.toml -> config -> compose -> start -> SIM read')
    ap.add_argument('--list',     action='store_true',
                    help='List PC/SC readers and SIM info')
    ap.add_argument('--instance', type=int,
                    help='Config instance to update (manual mode)')
    ap.add_argument('--reader',   type=int, default=None,
                    help='PC/SC reader index (default: instance-1)')
    ap.add_argument('--msisdn',   type=str, default=None,
                    help='Override MSISDN if not stored on card')
    ap.add_argument('--auto',     action='store_true',
                    help='Auto-assign all detected readers to instances')
    ap.add_argument('--restart',  action='store_true',
                    help='Restart container after config update')
    ap.add_argument('--watch',    action='store_true',
                    help='Monitor readers for SIM changes; auto-reconfigure')
    ap.add_argument('--interval', type=int, default=5, metavar='SEC',
                    help='Polling interval for --watch (default: 5)')
    args = ap.parse_args()

    db_init()

    if args.setup:
        setup(do_watch=args.watch, interval=args.interval)
        return

    if args.list:
        list_readers()
        return

    if args.watch:
        watch_loop(args.interval)
        return

    if args.auto:
        for i in enumerate_readers_in_container():
            instance = i + 1
            print(f"\n--- Reader P{i} -> Instance {instance} ---")
            try:
                sim = read_sim(i)
                if args.msisdn:
                    sim['msisdn'] = args.msisdn
                print(f"  IMSI={sim['imsi']}  MSISDN={sim['msisdn'] or '(none)'}")
                db_save_sim(i, sim)
                if update_instance(instance, sim) and args.restart:
                    restart_instance(instance)
            except Exception as e:
                print(f"  Skipping: {e}", file=sys.stderr)
        return

    if args.instance is None:
        ap.print_help()
        sys.exit(1)

    idx = args.reader if args.reader is not None else (args.instance - 1)
    print(f"Reading SIM from P{idx}...")
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
    db_save_sim(idx, sim)

    if update_instance(args.instance, sim) and args.restart:
        restart_instance(args.instance)


if __name__ == '__main__':
    main()
