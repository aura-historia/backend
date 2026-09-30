ALTER TABLE listing_source_urls
    ADD COLUMN raw_source_record_key TEXT NOT NULL,
    ADD CONSTRAINT listing_source_urls_raw_source_record_key_nonempty
        CHECK (char_length(raw_source_record_key) > 0);
