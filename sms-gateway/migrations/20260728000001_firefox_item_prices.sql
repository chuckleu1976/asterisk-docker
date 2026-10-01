-- Store 火狐狸 item price catalog (from user API getItem)
CREATE TABLE IF NOT EXISTS firefox_item_prices (
    item_id       TEXT NOT NULL,
    country_id    TEXT,
    item_name     TEXT NOT NULL,
    item_uprice   REAL NOT NULL DEFAULT 0,
    country_title TEXT,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (item_id, country_id)
);

CREATE INDEX IF NOT EXISTS idx_firefox_item_prices_item_id
    ON firefox_item_prices (item_id);

CREATE INDEX IF NOT EXISTS idx_firefox_item_prices_updated_at
    ON firefox_item_prices (updated_at);
