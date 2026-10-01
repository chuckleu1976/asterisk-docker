-- Barcode scan buffer: stores ICCID/MSISDN pairs scanned from the barcode scanner
-- before they are imported into the phone-number management system.
CREATE TABLE IF NOT EXISTS barcode_scans (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    iccid      TEXT NOT NULL UNIQUE,
    msisdn     TEXT NOT NULL,
    imported   BOOLEAN NOT NULL DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_barcode_scans_imported
    ON barcode_scans (imported);
CREATE INDEX IF NOT EXISTS idx_barcode_scans_iccid
    ON barcode_scans (iccid);
