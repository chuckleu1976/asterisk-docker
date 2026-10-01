-- MMS receive support, phase 1: WAP-push notification detection + storage.
-- The EC20/EC25 modules used in this project have no native MMS receive/auto-fetch
-- AT commands, so incoming MMS notifications (a binary SMS carrying a WSP Push PDU
-- whose body is a WAP-209 encoded m-notification-ind) are detected and decoded
-- entirely in software (see src/decode.rs + src/mms_wap.rs) and recorded here.
-- Fetching the actual MMS content (phase 2) will update these rows in place.

CREATE TABLE IF NOT EXISTS mms_inbox (
    id TEXT PRIMARY KEY,
    sim_id TEXT NOT NULL,
    sender TEXT NOT NULL,
    transaction_id TEXT NOT NULL,
    content_location TEXT,
    message_size INTEGER,
    message_class TEXT,
    expiry_at DATETIME,
    -- notified | fetching | fetched | failed | expired
    status TEXT NOT NULL DEFAULT 'notified',
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at DATETIME,
    -- Raw WSP push bytes, retained so the (best-effort) header decoder can be
    -- fixed/replayed later without needing the original SMS, which is deleted
    -- from the modem right after each read cycle.
    notification_raw BLOB NOT NULL,
    created_at DATETIME NOT NULL,
    updated_at DATETIME NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_mms_inbox_sim_txn ON mms_inbox(sim_id, transaction_id);
CREATE INDEX IF NOT EXISTS idx_mms_inbox_status ON mms_inbox(status, created_at);
