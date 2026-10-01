-- Add per-SIM MMS attachment upload strategy.
-- Values:
--   modem_direct_attachment_upload
--   host_staged_attachment_upload
ALTER TABLE sim_cards ADD COLUMN mms_send_mode TEXT DEFAULT 'modem_direct_attachment_upload';
