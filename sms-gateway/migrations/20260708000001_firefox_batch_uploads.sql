-- Batch upload records for the 火狐狸 platform
CREATE TABLE IF NOT EXISTS firefox_batch_uploads (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_id      TEXT NOT NULL,
    country_id    TEXT NOT NULL,
    phone_numbers TEXT NOT NULL, -- comma-separated list
    created_at    TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_firefox_batch_uploads_country_id
    ON firefox_batch_uploads (country_id);

CREATE INDEX IF NOT EXISTS idx_firefox_batch_uploads_created_at
    ON firefox_batch_uploads (created_at DESC);
