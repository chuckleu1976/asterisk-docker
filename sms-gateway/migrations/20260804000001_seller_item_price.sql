-- Seller's own settable price per item, independent from the platform's
-- per-country reference price stored in firefox_item_prices.
ALTER TABLE firefox_item_names ADD COLUMN seller_item_price REAL NOT NULL DEFAULT 0;

-- Backfill: earlier versions stored the user-set price as a country_id-less
-- row in firefox_item_prices (a hack). Migrate those values, then drop them
-- so firefox_item_prices holds only genuine platform-synced data.
INSERT OR IGNORE INTO firefox_item_names (item_id, item_name, seller_item_price)
SELECT item_id, item_name, item_uprice FROM firefox_item_prices WHERE country_id IS NULL;

UPDATE firefox_item_names
SET seller_item_price = (
    SELECT item_uprice FROM firefox_item_prices
    WHERE firefox_item_prices.item_id = firefox_item_names.item_id
      AND firefox_item_prices.country_id IS NULL
)
WHERE EXISTS (
    SELECT 1 FROM firefox_item_prices
    WHERE firefox_item_prices.item_id = firefox_item_names.item_id
      AND firefox_item_prices.country_id IS NULL
);

DELETE FROM firefox_item_prices WHERE country_id IS NULL;
