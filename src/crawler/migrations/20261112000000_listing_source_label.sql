-- A human's label of a ListingSource the crawler should not crawl: a site that is not an online
-- shop, or one the crawler's approach cannot scrape (docs/crawler/README.md, ListingSource
-- scope). Labelling switches crawling off, and the scope sync keeps a labelled source disabled
-- until a human clears the label. The label is a closed code, the note optional free text that
-- OTHER requires, the actor a review actor name.
ALTER TABLE listing_sources
    ADD COLUMN source_label TEXT,
    ADD COLUMN source_label_note TEXT,
    ADD COLUMN source_labelled_at TIMESTAMPTZ,
    ADD COLUMN source_labelled_by TEXT,
    ADD CONSTRAINT listing_sources_source_label_check
        CHECK (source_label IS NULL OR source_label IN (
            'NOT_AN_ESHOP', 'APPROACH_NOT_APPLICABLE', 'BOT_PROTECTION',
            'JAVASCRIPT_RENDERED', 'LOGIN_REQUIRED', 'OTHER')),
    ADD CONSTRAINT listing_sources_source_label_shape_check
        CHECK ((source_label IS NULL AND source_label_note IS NULL
                AND source_labelled_at IS NULL AND source_labelled_by IS NULL)
            OR (source_label IS NOT NULL AND source_labelled_at IS NOT NULL
                AND source_labelled_by IS NOT NULL)),
    ADD CONSTRAINT listing_sources_source_label_note_check
        CHECK (source_label_note IS NULL
            OR (btrim(source_label_note) <> '' AND char_length(source_label_note) <= 500)),
    ADD CONSTRAINT listing_sources_source_label_other_note_check
        CHECK (source_label IS DISTINCT FROM 'OTHER' OR source_label_note IS NOT NULL),
    ADD CONSTRAINT listing_sources_source_labelled_by_check
        CHECK (source_labelled_by IS NULL
            OR source_labelled_by IN ('HUMAN', 'AGENT_AUTHOR', 'AGENT_REVIEWER', 'SYSTEM')),
    ADD CONSTRAINT listing_sources_source_label_not_crawled_check
        CHECK (source_label IS NULL OR NOT crawl_enabled);
