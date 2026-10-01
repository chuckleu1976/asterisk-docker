-- MMS sending support (EC20/EC25 via AT+QMMSEDIT/AT+QMMSEND)
-- Messages are enqueued by the API and processed asynchronously by mms_worker.

CREATE TABLE IF NOT EXISTS mms_messages (
    id TEXT PRIMARY KEY,
    sim_id TEXT NOT NULL,
    to_number TEXT NOT NULL,
    subject TEXT,
    -- queued | sending | sent | failed | timeout
    status TEXT NOT NULL DEFAULT 'queued',
    quectel_err_code INTEGER,
    http_response_code INTEGER,
    error_message TEXT,
    created_at DATETIME NOT NULL,
    updated_at DATETIME NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_mms_messages_status ON mms_messages(status, created_at);
CREATE INDEX IF NOT EXISTS idx_mms_messages_sim_id ON mms_messages(sim_id);

CREATE TABLE IF NOT EXISTS mms_attachments (
    id TEXT PRIMARY KEY,
    mms_id TEXT NOT NULL REFERENCES mms_messages(id) ON DELETE CASCADE,
    filename TEXT NOT NULL,
    content_type TEXT,
    size_bytes INTEGER NOT NULL,
    data BLOB NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_mms_attachments_mms_id ON mms_attachments(mms_id);

-- Per-SIM MMS profile (carriers use different APN/MMSC/proxy settings)
ALTER TABLE sim_cards ADD COLUMN mms_apn TEXT;
ALTER TABLE sim_cards ADD COLUMN mms_mmsc TEXT;
ALTER TABLE sim_cards ADD COLUMN mms_proxy_host TEXT;
ALTER TABLE sim_cards ADD COLUMN mms_proxy_port INTEGER;
