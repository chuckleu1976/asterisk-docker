#!/usr/bin/env python3
"""
start_sim_container_config.py - Bootstrap asterisk-docker from devices.toml.

Split into modules under scripts/:
  scripts/sim_db.py         – SQLite inventory
  scripts/sim_config_gen.py – config/compose.yaml generation
  scripts/sim_docker.py     – docker helpers, SIM reading, config updating

Workflow:
  1. Read devices.toml -> get reader-to-container mapping with IMEI
  2. Generate config/<N>/ directories from config/example/
  3. Generate compose.yaml
  4. Start pcscd + asterisk-1 as SIM probe
  5. Read SIM cards from each reader via APDU
  6. Start per-reader containers only where SIM is present
  7. Optionally --watch for SIM hotswap (auto start/stop on insert/remove)

Usage:
  ./start_sim --setup               # Full bootstrap
  ./start_sim --setup --watch       # Bootstrap then monitor
  ./start_sim --list                # List readers & SIM info
  ./start_sim --watch               # Monitor only
"""

import argparse
import sys
import time
from datetime import datetime
from pathlib import Path

from scripts.sim_db import DB_PATH, SCRIPT_DIR, db_clear_sim, db_clear_empty_sims, db_init, db_save_reader, db_save_sim
from scripts.sim_config_gen import (
    CONFIG_DIR, EXAMPLE_DIR, generate_config, generate_compose,
)
from scripts.sim_docker import (
    get_read_container, enumerate_readers_in_container, read_sim,
    start_probe, start_instance, stop_instance, docker_compose,
    update_instance,
)

try:
    import tomllib
except ImportError:
    try:
        import tomli as tomllib
    except ImportError:
        tomllib = None

DEVICES_TOML = SCRIPT_DIR / "devices.toml"


def parse_devices_toml():
    if tomllib is None:
        raise RuntimeError("Need Python 3.11+ or 'pip install tomli' for TOML support")
    with open(DEVICES_TOML, 'rb') as f:
        data = tomllib.load(f)
    devices = []
    for entry in data.get('readers', []):
        module_val = entry.get('MODULE', 'SAMSUNG')
        for key, hostname in entry.items():
            if key in ('IMEI', 'MODULE'):
                continue
            if key.startswith('P'):
                reader_idx = int(key[1:])
                devices.append({
                    'reader': reader_idx,
                    'hostname': hostname,
                    'imei': str(entry.get('IMEI', '')),
                    'module': module_val,
                })
    devices.sort(key=lambda d: d['reader'])
    return devices


def setup(do_watch=False, interval=5):
    print("=" * 60)
    print("  SETUP: Bootstrap from devices.toml")
    print("=" * 60)

    print("\n[1/6] Reading devices.toml...")
    devices = parse_devices_toml()
    print(f"  Found {len(devices)} reader(s):")
    for dev in devices:
        print(f"    P{dev['reader']}: {dev['hostname']} (IMEI={dev['imei']})")

    print(f"\n[2/6] Initialising database ({DB_PATH.name})...")
    db_init()
    for dev in devices:
        db_save_reader(dev['reader'], f"P{dev['reader']}", "available",
                       hostname=dev['hostname'], imei=dev['imei'],
                       module=dev.get('module', 'SAMSUNG'))
    print(f"  {len(devices)} reader(s) saved")

    print(f"\n[3/6] Generating config directories...")
    for dev in devices:
        generate_config(dev['reader'] + 1, dev)

    print(f"\n[4/6] Generating compose.yaml...")
    generate_compose(devices)

    print(f"\n[5/6] Creating log directories and starting pcscd + probe...")
    for dev in devices:
        (SCRIPT_DIR.parent / "logs" / str(dev['reader'] + 1)).mkdir(
            parents=True, exist_ok=True)
    start_probe()

    print(f"\n[6/6] Probing SIMs and starting per-card containers...")
    sims_present = []
    sims_absent = []
    for dev in devices:
        idx = dev['reader']
        print(f"\n  P{idx} ({dev['hostname']}):")
        try:
            sim = read_sim(idx)
            print(f"    SIM: ICCID={sim['iccid']}  IMSI={sim['imsi']}  "
                  f"MSISDN={sim['msisdn'] or '(none)'}  MCC/MNC={sim['mcc']}/{sim['mnc']}")
            db_save_sim(idx, sim)
            db_save_reader(idx, f"P{idx}", "available",
                           hostname=dev['hostname'], imei=dev['imei'],
                           module=dev.get('module', 'SAMSUNG'))
            sims_present.append((dev, sim))
        except Exception as e:
            print(f"    No SIM / read error: {e}")
            db_save_reader(idx, f"P{idx}", "empty",
                           hostname=dev['hostname'], imei=dev['imei'],
                           module=dev.get('module', 'SAMSUNG'))
            sims_absent.append(dev)

    for dev in sims_absent:
        stop_instance(dev['reader'] + 1)

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


def list_readers():
    print("Detecting readers...")
    reader_indices = enumerate_readers_in_container()
    if not reader_indices:
        print(f"Error: No readers detected (is 'asterisk' running?)",
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
        dev = dev_map.get(idx, {})
        hostname = dev.get('hostname', '?')
        imei = dev.get('imei', '?')
        module = dev.get('module', 'SAMSUNG')
        print(f"  P{idx} ({hostname}, IMEI={imei}):", end=" ")
        try:
            sim = read_sim(idx)
            print("SIM detected")
            print(f"      ICCID:   {sim['iccid']}")
            print(f"      IMSI:    {sim['imsi']}")
            print(f"      MSISDN:  {sim['msisdn'] or '(not stored on card)'}")
            print(f"      MCC/MNC: {sim['mcc']}/{sim['mnc']}")
            db_save_reader(idx, f"P{idx}", "available",
                           hostname=hostname, imei=imei, module=module)
            db_save_sim(idx, sim)
        except Exception as e:
            print(f"empty/error: {e}")
            db_save_reader(idx, f"P{idx}", "empty",
                           hostname=hostname, imei=imei, module=module)


def watch_loop(interval, devices=None):
    db_init()

    dev_map = {}
    if devices is None:
        try:
            devices = parse_devices_toml()
        except Exception:
            devices = []
    for dev in devices:
        dev_map[dev['reader']] = dev

    # Startup cleanup: clear sims rows for readers already marked empty/error.
    db_clear_empty_sims()

    last_iccid = {}
    last_reader_indices = set()
    read_failures = {}
    REMOVE_THRESHOLD = 3

    print(f"[watch] Polling every {interval}s. Press Ctrl+C to stop.\n")

    while True:
        time.sleep(interval)

        reader_indices = set(enumerate_readers_in_container())

        if reader_indices != last_reader_indices:
            ts = datetime.now().strftime('%H:%M:%S')
            added = reader_indices - last_reader_indices
            removed = last_reader_indices - reader_indices
            if added:
                print(f"[{ts}] Readers added:   {sorted(added)}")
                for idx in added:
                    dev = dev_map.get(idx, {})
                    db_save_reader(idx, f"P{idx}", "available",
                                   hostname=dev.get('hostname', ''),
                                   imei=dev.get('imei', ''),
                                   module=dev.get('module', 'SAMSUNG'))
                    last_iccid.setdefault(idx, None)
            if removed:
                print(f"[{ts}] Readers removed: {sorted(removed)}")
                for idx in removed:
                    dev = dev_map.get(idx, {})
                    db_clear_sim(idx)
                    db_save_reader(idx, f"P{idx}", "error",
                                   hostname=dev.get('hostname', ''),
                                   imei=dev.get('imei', ''),
                                   module=dev.get('module', 'SAMSUNG'))
                    last_iccid.pop(idx, None)
                    stop_instance(idx + 1)
            last_reader_indices = reader_indices

        for i in reader_indices:
            instance = i + 1
            dev = dev_map.get(i, {})
            last_iccid.setdefault(i, None)

            try:
                sim = read_sim(i)
                iccid = sim['iccid']
                read_failures[i] = 0
                db_save_reader(i, f"P{i}", "available",
                               hostname=dev.get('hostname', ''),
                               imei=dev.get('imei', ''),
                               module=dev.get('module', 'SAMSUNG'))
            except Exception:
                read_failures[i] = read_failures.get(i, 0) + 1
                if last_iccid[i] is not None and read_failures[i] >= REMOVE_THRESHOLD:
                    ts = datetime.now().strftime('%H:%M:%S')
                    print(f"[{ts}] P{i}: card removed")
                    db_clear_sim(i)
                    db_save_reader(i, f"P{i}", "empty",
                                   hostname=dev.get('hostname', ''),
                                   imei=dev.get('imei', ''),
                                   module=dev.get('module', 'SAMSUNG'))
                    last_iccid[i] = None
                    read_failures[i] = 0
                    stop_instance(instance)
                continue

            if iccid != last_iccid[i]:
                ts = datetime.now().strftime('%H:%M:%S')

                moved_from = None
                for j, prev in last_iccid.items():
                    if j != i and prev == iccid:
                        moved_from = j
                        break

                if moved_from is not None:
                    print(f"[{ts}] P{i}: card moved from P{moved_from} "
                          f"ICCID={iccid}  IMSI={sim['imsi']}")
                    old_dev = dev_map.get(moved_from, {})
                    db_clear_sim(moved_from)
                    stop_instance(moved_from + 1)
                    last_iccid[moved_from] = None
                    read_failures[moved_from] = 0
                    db_save_reader(moved_from, f"P{moved_from}", "empty",
                                   hostname=old_dev.get('hostname', ''),
                                   imei=old_dev.get('imei', ''),
                                   module=old_dev.get('module', 'SAMSUNG'))
                else:
                    action = "inserted" if last_iccid[i] is None else \
                             f"changed {last_iccid[i]} ->"
                    print(f"[{ts}] P{i}: card {action} ICCID={iccid}  IMSI={sim['imsi']}")

                last_iccid[i] = iccid
                db_save_sim(i, sim)
                if update_instance(instance, sim):
                    start_instance(instance)


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
    ap.add_argument('--setup', action='store_true',
                    help='Full bootstrap: devices.toml -> config -> compose -> start -> SIM read')
    ap.add_argument('--list', action='store_true',
                    help='List PC/SC readers and SIM info')
    ap.add_argument('--instance', type=int,
                    help='Config instance to update (manual mode)')
    ap.add_argument('--reader', type=int, default=None,
                    help='PC/SC reader index (default: instance-1)')
    ap.add_argument('--msisdn', type=str, default=None,
                    help='Override MSISDN if not stored on card')
    ap.add_argument('--auto', action='store_true',
                    help='Auto-assign all detected readers to instances')
    ap.add_argument('--restart', action='store_true',
                    help='Restart container after config update')
    ap.add_argument('--watch', action='store_true',
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
                from scripts.sim_docker import update_instance
                if update_instance(instance, sim) and args.restart:
                    from scripts.sim_docker import restart_instance
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
