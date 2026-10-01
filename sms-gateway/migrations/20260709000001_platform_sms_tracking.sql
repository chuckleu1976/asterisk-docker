-- Track which SMS have been uploaded to the 火狐狸 platform and under which item.

-- Add platform upload tracking columns to the existing sms table
ALTER TABLE sms ADD COLUMN uploaded_to_platform BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE sms ADD COLUMN platform_item_id TEXT;
ALTER TABLE sms ADD COLUMN platform_uploaded_at TIMESTAMP;
ALTER TABLE sms ADD COLUMN platform_response TEXT;

CREATE INDEX IF NOT EXISTS idx_sms_uploaded ON sms (uploaded_to_platform);
CREATE INDEX IF NOT EXISTS idx_sms_platform_item ON sms (platform_item_id, uploaded_to_platform);

-- Track platform items seen in the wait list
CREATE TABLE IF NOT EXISTS firefox_platform_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     TEXT NOT NULL,
    country_id  TEXT NOT NULL,
    phone_num   TEXT NOT NULL,
    iccid       TEXT,
    sim_id      TEXT,
    status      TEXT NOT NULL DEFAULT 'waiting', -- waiting/completed/failed/expired
    created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(item_id, country_id, phone_num)
);

CREATE INDEX IF NOT EXISTS idx_firefox_platform_items_item_id
    ON firefox_platform_items (item_id);
CREATE INDEX IF NOT EXISTS idx_firefox_platform_items_phone
    ON firefox_platform_items (phone_num, country_id);
CREATE INDEX IF NOT EXISTS idx_firefox_platform_items_status
    ON firefox_platform_items (status);
