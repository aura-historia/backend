-- Rename the existing `state` column to `last_scraped_state` for naming consistency,
-- and add all remaining tracked-field snapshot columns used for change detection.

ALTER TABLE shop_urls RENAME COLUMN state TO last_scraped_state;

ALTER TABLE shop_urls
    ADD COLUMN IF NOT EXISTS last_scraped_price              TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_price_estimate_min TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_price_estimate_max TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_url                TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_images_hash        TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_auction_start      TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_auction_end        TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_origin_year        TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_authenticity       TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_condition          TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_provenance         TEXT,
    ADD COLUMN IF NOT EXISTS last_scraped_restoration        TEXT;
