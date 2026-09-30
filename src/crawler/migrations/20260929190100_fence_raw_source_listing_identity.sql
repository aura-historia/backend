ALTER TABLE listing_source_urls
    ADD COLUMN last_source_listing_id TEXT;

ALTER TABLE listing_source_urls
    ADD CONSTRAINT listing_source_urls_last_source_listing_id_nonempty
        CHECK (last_source_listing_id IS NULL OR char_length(last_source_listing_id) > 0);
