-- Track failed 火狐狸 SMS uploads with retry logic
-- This table queues uploads that fail and retries them with exponential backoff

CREATE TABLE IF NOT EXISTS firefox_upload_retry_queue (
    id TEXT PRIMARY KEY,
    sms_id INTEGER NOT NULL,
    phone_number TEXT NOT NULL,
    country_id TEXT NOT NULL,
    message TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 5,
    next_retry_at DATETIME NOT NULL,
    last_error TEXT,
    last_response_code TEXT,
    created_at DATETIME NOT NULL,
    updated_at DATETIME NOT NULL,
    FOREIGN KEY(sms_id) REFERENCES sms(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_firefox_upload_retry_next_retry 
    ON firefox_upload_retry_queue(next_retry_at);
    
CREATE INDEX IF NOT EXISTS idx_firefox_upload_retry_sms_id 
    ON firefox_upload_retry_queue(sms_id);

CREATE INDEX IF NOT EXISTS idx_firefox_upload_retry_phone 
    ON firefox_upload_retry_queue(phone_number, country_id);
