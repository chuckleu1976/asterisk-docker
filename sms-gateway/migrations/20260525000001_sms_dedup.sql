-- Prevent duplicate SMS records (same sender, same SIM, same timestamp, same message)
-- Used with INSERT OR IGNORE so re-reading from modem after crash doesn't create duplicates
CREATE UNIQUE INDEX IF NOT EXISTS idx_sms_dedup
    ON sms (contact_id, sim_id, timestamp, message);
