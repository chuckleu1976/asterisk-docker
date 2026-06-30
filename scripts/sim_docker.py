import json
import re
import subprocess
import time
from pathlib import Path

from .sim_config_gen import CONFIG_DIR

SCRIPT_DIR = Path(__file__).resolve().parent.parent

# Embedded APDU reader script run inside the asterisk container via
# docker compose exec python3 -c "...".
READER_SCRIPT = r"""
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
    _apdu(conn, f'00A40400{len(aid)//2:02X}{aid}')

def select_mf(conn):
    _apdu(conn, '00A40004023F00')

def read_iccid(conn):
    if not _select_path(conn, 0x3F00, 0x2FE2):
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

def read_msisdn(conn):
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
    if (ton & 0x70) == 0x10:
        return '+' + number
    return number

def _extract_smsc_from_record(data):
    i = 0
    while i < len(data) - 2:
        al = data[i]
        if 1 <= al <= 20 and i + 1 + al <= len(data):
            ton = data[i + 1]
            num_digits = al - 1
            if num_digits >= 2 and i + 2 + num_digits <= len(data):
                dh = toHexString(data[i+2:i+2+num_digits]).replace(' ', '')
                number = _swap(dh).rstrip('fF')
                if len(number) >= 5:
                    if (ton & 0x70) == 0x10:
                        return '+' + number
                    return number
        i += 1
    return None

def _select_path(conn, *fids):
    for fid in fids:
        data, sw1, sw2 = _apdu(conn, f'00A4000402{fid:04X}')
        if sw1 == 0x61:
            data, sw1, sw2 = _apdu(conn, f'00C00000{sw2:02X}')
        if sw1 != 0x90:
            return False
    return True

def _try_read_smsc(conn, df):
    if not _select_path(conn, 0x3F00, df, 0x6F42):
        return None
    for rec in (2, 1):
        data, sw1, sw2 = _apdu(conn, f'00B201{rec:02X}28')
        if sw1 == 0x6C:
            data, sw1, sw2 = _apdu(conn, f'00B201{rec:02X}{sw2:02X}')
        if sw1 == 0x90 and data:
            result = _extract_smsc_from_record(data)
            if result:
                return result
    data, sw1, sw2 = _apdu(conn, '00B0000028')
    if sw1 == 0x6C:
        data, sw1, sw2 = _apdu(conn, f'00B00000{sw2:02X}')
    if sw1 == 0x90 and data:
        return _extract_smsc_from_record(data)
    return None

def read_sms_center(conn):
    result = _try_read_smsc(conn, 0x7F10)
    if result:
        return result
    return _try_read_smsc(conn, 0x7F20)

try:
    conn = connect(reader_index)
    aid = select_aid(conn)

    # Set initial context: prefer USIM if available
    if aid:
        reselect_aid(conn, aid)

    iccid = read_iccid(conn)
    if iccid is None:
        select_mf(conn)
        iccid = read_iccid(conn)

    # If we have a USIM AID and are currently in MF, re-select USIM
    if aid:
        reselect_aid(conn, aid)

    imsi = read_imsi(conn)
    if imsi is None:
        select_mf(conn)
        imsi = read_imsi(conn)
        if imsi and aid:
            reselect_aid(conn, aid)

    mnc_len = read_mnc_length(conn)
    msisdn = read_msisdn(conn)
    smsc = read_sms_center(conn)
    if smsc is None:
        select_mf(conn)
        smsc = read_sms_center(conn)
        if smsc and aid:
            reselect_aid(conn, aid)

    # If ICCID is still unknown, derive from IMSI
    if not iccid and imsi:
        iccid = "imsi:" + imsi

    mcc = imsi[:3] if imsi else None
    mnc = imsi[3:3 + mnc_len] if imsi else None

    print(json.dumps({
        'iccid': iccid,
        'imsi': imsi,
        'mcc': mcc,
        'mnc': mnc,
        'msisdn': msisdn or '',
        'sms_center': smsc or '',
    }))
except Exception as e:
    print(json.dumps({'error': str(e)}), file=sys.stderr)
    sys.exit(1)
"""

READ_CONTAINER = "asterisk"


def docker_compose(*args, timeout=None):
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
    print("Starting pcscd + asterisk-1 (SIM probe)...")
    r = docker_compose("up", "-d", "pcscd", READ_CONTAINER)
    if r.returncode != 0:
        print(f"  Error: {r.stderr.strip()}", file=sys.stderr)
    else:
        print("  OK")
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
        return
    print(f"  Unregistering {svc} from IMS...", end="", flush=True)
    r = docker_compose("exec", "-T", svc, "asterisk", "-rx",
                       "pjsip send unregister volte_ims", timeout=10)
    print(" OK" if r.returncode == 0 else " (skipped)")
    time.sleep(3)
    print(f"  Stopping {svc} (no SIM)...", end="", flush=True)
    r = docker_compose("stop", svc)
    print(" OK" if r.returncode == 0 else f" Error: {r.stderr.strip()}")


def _patch_instance(svc):
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


def get_read_container():
    r = docker_compose("ps", "--format", "{{.Service}}\t{{.State}}", timeout=10)
    if r.returncode == 0:
        for line in r.stdout.splitlines():
            parts = line.strip().split('\t')
            if len(parts) == 2:
                svc, state = parts
                if svc.startswith('asterisk') and state == 'running':
                    return svc
    print(f"  [probe] No asterisk container running, starting {READ_CONTAINER} as probe...",
          flush=True)
    up = docker_compose("up", "-d", READ_CONTAINER, timeout=60)
    if up.returncode != 0:
        print(f"  [probe] Failed to start {READ_CONTAINER}: {up.stderr.strip()}")
        return READ_CONTAINER
    time.sleep(5)
    print(f"  [probe] {READ_CONTAINER} started", flush=True)
    return READ_CONTAINER


def enumerate_readers_in_container():
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


def read_sim(reader_index, retries=1):
    last_err = None
    for attempt in range(retries + 1):
        container = get_read_container()
        try:
            r = docker_compose("exec", "-T", container,
                               "python3", "-c", READER_SCRIPT, str(reader_index),
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
    raise last_err


# ─── Config file updaters ─────────────────────────────────────────────────────

def update_ami_usim(path, sim):
    txt = path.read_text()
    txt = re.sub(r'^(reader\s*=\s*imsi:).*$',
                 rf'\g<1>{sim["imsi"]}', txt, flags=re.MULTILINE)
    path.write_text(txt)
    print(f"    ami_usim.ini  reader=imsi:{sim['imsi']}")


def update_epdg(path, sim, hostname=None):
    mcc, mnc, imsi = sim['mcc'], sim['mnc'], sim['imsi']
    if not mcc or not mnc or not imsi:
        print("    [warn] Missing MCC/MNC/IMSI — epdg.conf not updated")
        return
    txt = path.read_text()
    txt = re.sub(
        r'(remote_addrs\s*=\s*)epdg\.epc\.mnc\w+\.mcc\w+\.pub\.3gppnetwork\.org',
        rf'\g<1>epdg.epc.mnc{mnc}.mcc{mcc}.pub.3gppnetwork.org', txt)
    txt = re.sub(
        r'(id\s*=\s*)0\w+@nai\.epc\.mnc\w+\.mcc\w+\.3gppnetwork\.org',
        rf'\g<1>0{imsi}@nai.epc.mnc{mnc}.mcc{mcc}.3gppnetwork.org', txt)
    if hostname:
        txt = re.sub(r'(local_addrs\s*=\s*)\S+', rf'\g<1>{hostname}', txt)
    path.write_text(txt)
    print(f"    epdg.conf     ePDG=mnc{mnc}.mcc{mcc}, NAI=0{imsi}")


def update_pjsip(path, sim):
    mcc, mnc, imsi = sim['mcc'], sim['mnc'], sim['imsi']
    if not mcc or not mnc or not imsi:
        print("    [warn] Missing MCC/MNC/IMSI — pjsip.conf not updated")
        return
    msisdn = sim['msisdn']
    sms_center = sim.get('sms_center', '')
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
        r'(server_uri\s*=\s*sip:)ims\.mnc\w+\.mcc\w+\.3gppnetwork\.org',
        rf'\g<1>{domain}', txt)
    txt = re.sub(
        r'(client_uri\s*=\s*sip:)\w+@ims\.mnc\w+\.mcc\w+\.3gppnetwork\.org',
        rf'\g<1>{imsi}@{domain}', txt)
    txt = re.sub(
        r'^(from_domain\s*=\s*)ims\.mnc\w+\.mcc\w+\.3gppnetwork\.org',
        rf'\g<1>{domain}', txt, flags=re.MULTILINE)
    if sms_center:
        txt = re.sub(
            r'(smsc_uri\s*=\s*sip:)[^@]+@ims\.mnc\w+\.mcc\w+\.3gppnetwork\.org',
            rf'\g<1>{sms_center}@{domain}', txt)
    else:
        txt = re.sub(
            r'(smsc_uri\s*=\s*sip:[^@]+@)ims\.mnc\w+\.mcc\w+\.3gppnetwork\.org',
            rf'\g<1>{domain}', txt)
    txt = re.sub(
        r'^(username\s*=\s*)\S+@ims\.mnc\w+\.mcc\w+\.3gppnetwork\.org',
        rf'\g<1>{imsi}@{domain}', txt, flags=re.MULTILINE)
    txt = re.sub(
        r'(contact\s*=\s*sip:)\S+@ims\.mnc\w+\.mcc\w+\.3gppnetwork\.org',
        rf'\g<1>{imsi}@{domain}', txt)
    txt = re.sub(r'\[ims\.mnc\w+\.mcc\w+\.3gppnetwork\.org\]',
                 f'[{domain}]', txt)
    txt = re.sub(r'\[smsoip\.ims\.mnc\w+\.mcc\w+\.3gppnetwork\.org\]',
                 f'[smsoip.{domain}]', txt)
    path.write_text(txt)
    print(f"    pjsip.conf    IMSI={imsi}, MSISDN={msisdn or '(unchanged)'}, domain={domain}")


def update_instance(instance, sim):
    d = CONFIG_DIR / str(instance)
    if not d.exists():
        print(f"  Error: config/{instance}/ not found", file=sys.stderr)
        return False
    from .sim_config_gen import EXAMPLE_DIR
    hostname = None
    try:
        import tomllib
        devices_toml = SCRIPT_DIR / "devices.toml"
        with open(devices_toml, 'rb') as f:
            data = tomllib.load(f)
        for entry in data.get('readers', []):
            for key in entry:
                if key.startswith('P') and int(key[1:]) == instance - 1:
                    hostname = entry[key]
                    break
    except Exception:
        pass
    print(f"  Instance {instance} — writing SIM data:")
    update_ami_usim(d / "ami_usim.ini", sim)
    update_epdg(d / "epdg.conf", sim, hostname=hostname)
    update_pjsip(d / "asterisk" / "pjsip.conf", sim)
    return True
