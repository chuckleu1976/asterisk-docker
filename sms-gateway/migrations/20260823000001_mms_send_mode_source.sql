-- Distinguish a per-SIM user choice from the legacy column default so
-- config.toml controls MMS send mode until the MMS page saves an override.
ALTER TABLE sim_cards ADD COLUMN mms_send_mode_source TEXT;