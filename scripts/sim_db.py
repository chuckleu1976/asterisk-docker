import sqlite3
from datetime import datetime
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent.parent
DB_PATH = SCRIPT_DIR / "sim_inventory.db"


def db_init():
    con = sqlite3.connect(DB_PATH)
    con.execute("""
        CREATE TABLE IF NOT EXISTS readers (
            reader      INTEGER PRIMARY KEY,
            name        TEXT,
            hostname    TEXT,
            imei        TEXT,
            module      TEXT DEFAULT 'SAMSUNG',
            status      TEXT,
            last_seen   TEXT,
            updated_at  TEXT
        )
    """)
    # Add module column if missing (migration for existing DBs)
    try:
        con.execute("ALTER TABLE readers ADD COLUMN module TEXT DEFAULT 'SAMSUNG'")
    except sqlite3.OperationalError:
        pass  # column already exists
    con.execute("""
        CREATE TABLE IF NOT EXISTS sims (
            reader      INTEGER PRIMARY KEY,
            iccid       TEXT,
            imsi        TEXT,
            msisdn      TEXT,
            mcc         TEXT,
            mnc         TEXT,
            sms_center  TEXT,
            updated_at  TEXT
        )
    """)
    # Add sms_center column if missing (migration)
    try:
        con.execute("ALTER TABLE sims ADD COLUMN sms_center TEXT")
    except sqlite3.OperationalError:
        pass  # column already exists
    con.commit()
    con.close()


def db_save_reader(reader_index, reader_name, status,
                   hostname='', imei='', module='SAMSUNG'):
    now = datetime.now().strftime('%Y-%m-%d %H:%M:%S')
    con = sqlite3.connect(DB_PATH)
    con.execute("""
        INSERT INTO readers (reader, name, hostname, imei, module, status, last_seen, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(reader) DO UPDATE SET
            name=excluded.name, hostname=excluded.hostname, imei=excluded.imei,
            module=excluded.module, status=excluded.status, last_seen=excluded.last_seen,
            updated_at=excluded.updated_at
    """, (reader_index, reader_name, hostname, imei, module, status, now, now))
    con.commit()
    con.close()


def db_save_sim(reader_index, sim):
    now = datetime.now().strftime('%Y-%m-%d %H:%M:%S')
    con = sqlite3.connect(DB_PATH)
    con.execute("""
        INSERT INTO sims (reader, iccid, imsi, msisdn, mcc, mnc, sms_center, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(reader) DO UPDATE SET
            iccid=excluded.iccid, imsi=excluded.imsi, msisdn=excluded.msisdn,
            mcc=excluded.mcc, mnc=excluded.mnc, sms_center=excluded.sms_center,
            updated_at=excluded.updated_at
    """, (reader_index, sim.get('iccid'), sim.get('imsi'), sim.get('msisdn'),
          sim.get('mcc'), sim.get('mnc'), sim.get('sms_center'), now))
    con.commit()
    con.close()
