-- Settings key/value store for user configuration (e.g. 火狐狸 API key)
CREATE TABLE IF NOT EXISTS app_settings (
    key   TEXT PRIMARY KEY,
    value TEXT
);

-- Country code used when uploading a SIM to the 火狐狸 platform
ALTER TABLE sim_cards ADD COLUMN country_code TEXT;
