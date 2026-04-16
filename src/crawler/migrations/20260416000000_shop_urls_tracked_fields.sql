-- Rename the existing `state` column to `last_scraped_state` for naming consistency,
-- and add all remaining tracked-field snapshot columns used for change detection.
--
-- The DO block makes the rename idempotent: on a fresh database the initial schema
-- already uses `last_scraped_state`, so the column `state` will not exist and the
-- rename is skipped.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_name = 'shop_urls'
          AND column_name = 'state'
    ) THEN
        ALTER TABLE shop_urls RENAME COLUMN state TO last_scraped_state;
    END IF;
END $$;

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
