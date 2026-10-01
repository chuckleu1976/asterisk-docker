-- MMS receive support, phase 2: fetched content storage.
-- content_location is retrieved via AT+QHTTPxxx (see Modem::fetch_mms_content)
-- and decoded (see src/mms_retrieve.rs) into an optional subject/sender plus
-- one row per MMS part (SMIL, text, image, ...).

ALTER TABLE mms_inbox ADD COLUMN subject TEXT;
ALTER TABLE mms_inbox ADD COLUMN from_address TEXT;
ALTER TABLE mms_inbox ADD COLUMN fetched_at DATETIME;

CREATE TABLE IF NOT EXISTS mms_inbox_parts (
    id TEXT PRIMARY KEY,
    inbox_id TEXT NOT NULL REFERENCES mms_inbox(id) ON DELETE CASCADE,
    content_type TEXT,
    filename TEXT,
    size_bytes INTEGER NOT NULL,
    data BLOB NOT NULL,
    created_at DATETIME NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_mms_inbox_parts_inbox_id ON mms_inbox_parts(inbox_id);
