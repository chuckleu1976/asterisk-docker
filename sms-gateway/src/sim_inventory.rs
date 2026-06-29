use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::OnceLock;

static INV_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn set_path(p: PathBuf) {
    let _ = INV_PATH.set(p);
}

fn db_path() -> PathBuf {
    INV_PATH
        .get()
        .cloned()
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("sim_inventory.db")
        })
}

fn open_ro() -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(db_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(conn)
}

pub fn get_imei(reader_index: u8) -> anyhow::Result<Option<String>> {
    let conn = open_ro()?;
    let mut stmt = conn.prepare("SELECT imei FROM readers WHERE reader = ?1")?;
    let result: Option<String> = stmt
        .query_row([reader_index], |row| row.get(0))
        .ok();
    Ok(result.filter(|s| !s.is_empty()))
}

pub fn get_msisdn(reader_index: u8) -> anyhow::Result<Option<String>> {
    let conn = open_ro()?;
    let mut stmt = conn.prepare("SELECT msisdn FROM sims WHERE reader = ?1")?;
    let result: Option<String> = stmt
        .query_row([reader_index], |row| row.get(0))
        .ok();
    Ok(result.filter(|s| !s.is_empty()))
}

pub fn get_imsi(reader_index: u8) -> anyhow::Result<Option<String>> {
    let conn = open_ro()?;
    let mut stmt = conn.prepare("SELECT imsi FROM sims WHERE reader = ?1")?;
    let result: Option<String> = stmt
        .query_row([reader_index], |row| row.get(0))
        .ok();
    Ok(result.filter(|s| !s.is_empty()))
}

pub fn get_iccid(reader_index: u8) -> anyhow::Result<Option<String>> {
    let conn = open_ro()?;
    let mut stmt = conn.prepare("SELECT iccid FROM sims WHERE reader = ?1")?;
    let result: Option<String> = stmt
        .query_row([reader_index], |row| row.get(0))
        .ok();
    Ok(result.filter(|s| !s.is_empty()))
}

pub fn get_mcc_mnc(reader_index: u8) -> anyhow::Result<Option<(String, String)>> {
    let conn = open_ro()?;
    let mut stmt = conn.prepare("SELECT mcc, mnc FROM sims WHERE reader = ?1")?;
    let result: Option<(String, String)> = stmt
        .query_row([reader_index], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok();
    Ok(result.filter(|(mcc, mnc)| !mcc.is_empty() && !mnc.is_empty()))
}

const OPERATOR_LOOKUP: &[(&str, &str, &str)] = &[
    ("310", "240", "T-Mobile US"),
    ("310", "260", "T-Mobile US"),
    ("310", "410", "AT&T"),
    ("310", "150", "AT&T"),
    ("310", "170", "AT&T"),
    ("310", "004", "Verizon"),
    ("310", "012", "Verizon"),
    ("310", "480", "Verizon"),
    ("001", "01", "Test Network"),
    ("311", "480", "Verizon"),
    ("312", "530", "T-Mobile US"),
    ("310", "030", "AT&T"),
    ("310", "560", "AT&T"),
    ("310", "680", "AT&T"),
    ("310", "090", "AT&T"),
    ("310", "070", "AT&T"),
    ("310", "880", "T-Mobile US"),
    ("310", "490", "T-Mobile US"),
    ("310", "660", "T-Mobile US"),
    ("310", "200", "T-Mobile US"),
    ("310", "160", "T-Mobile US"),
    ("310", "210", "T-Mobile US"),
    ("310", "250", "T-Mobile US"),
    ("310", "270", "T-Mobile US"),
    ("310", "310", "T-Mobile US"),
    ("310", "330", "T-Mobile US"),
    ("310", "370", "T-Mobile US"),
    ("310", "400", "T-Mobile US"),
    ("310", "440", "T-Mobile US"),
    ("310", "510", "T-Mobile US"),
    ("310", "530", "T-Mobile US"),
    ("310", "580", "T-Mobile US"),
    ("310", "590", "T-Mobile US"),
    ("310", "640", "T-Mobile US"),
    ("310", "650", "T-Mobile US"),
    ("310", "800", "T-Mobile US"),
    ("310", "810", "T-Mobile US"),
    ("310", "870", "T-Mobile US"),
    ("310", "900", "T-Mobile US"),
    ("311", "220", "T-Mobile US"),
    ("311", "230", "T-Mobile US"),
    ("311", "240", "T-Mobile US"),
    ("311", "250", "T-Mobile US"),
    ("311", "260", "T-Mobile US"),
    ("311", "270", "T-Mobile US"),
    ("311", "280", "T-Mobile US"),
    ("311", "290", "T-Mobile US"),
    ("311", "300", "T-Mobile US"),
    ("311", "310", "T-Mobile US"),
    ("311", "320", "T-Mobile US"),
    ("311", "330", "T-Mobile US"),
    ("250", "01", "MTS Russia"),
    ("250", "02", "MegaFon"),
    ("250", "99", "Beeline"),
    ("262", "01", "Telekom DE"),
    ("262", "02", "Vodafone DE"),
    ("262", "03", "O2 DE"),
    ("262", "07", "O2 DE"),
    ("310", "026", "T-Mobile US"),
    ("310", "011", "T-Mobile US"),
    ("310", "120", "T-Mobile US"),
    ("310", "130", "T-Mobile US"),
    ("310", "140", "T-Mobile US"),
    ("310", "180", "T-Mobile US"),
    ("310", "190", "T-Mobile US"),
    ("310", "220", "T-Mobile US"),
    ("310", "230", "T-Mobile US"),
    ("310", "240", "T-Mobile US"),
    ("310", "260", "T-Mobile US"),
    ("310", "280", "T-Mobile US"),
    ("310", "290", "T-Mobile US"),
    ("310", "300", "T-Mobile US"),
    ("310", "310", "T-Mobile US"),
    ("310", "320", "T-Mobile US"),
    ("310", "340", "T-Mobile US"),
];

pub fn get_sms_center(reader_index: u8) -> anyhow::Result<Option<String>> {
    let conn = open_ro()?;
    let mut stmt = conn.prepare("SELECT sms_center FROM sims WHERE reader = ?1")?;
    let result: Option<String> = stmt
        .query_row([reader_index], |row| row.get(0))
        .ok();
    Ok(result.filter(|s| !s.is_empty()))
}

pub fn lookup_operator(mcc: &str, mnc: &str) -> Option<&'static str> {
    OPERATOR_LOOKUP
        .iter()
        .find(|(m, n, _)| *m == mcc && *n == mnc)
        .map(|(_, _, name)| *name)
}
