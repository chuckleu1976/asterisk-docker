ALTER TABLE sms ADD COLUMN status_report_requested BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE sms ADD COLUMN submit_ref INTEGER;
ALTER TABLE sms ADD COLUMN delivery_status INTEGER;
ALTER TABLE sms ADD COLUMN delivered_at TIMESTAMP;
ALTER TABLE sms ADD COLUMN delivery_report_raw TEXT;

CREATE INDEX IF NOT EXISTS idx_sms_submit_ref ON sms (sim_id, submit_ref);
CREATE INDEX IF NOT EXISTS idx_sms_delivery_status ON sms (delivery_status, send);