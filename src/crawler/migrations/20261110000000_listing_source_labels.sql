-- Operator labels of a ListingSource (docs/crawler/README.md, ListingSource scope).
--
-- purchase_channel mirrors the business ListingSource's value for display and filtering; it
-- never affects crawling. A source that leaves the scope keeps its last value. The default is
-- the business default.
--
-- unscrapable_* is crawler-local: a human marked the shop as one the crawler cannot scrape
-- (review console). Marking sets crawl_enabled = FALSE and the scope sync never re-enables a
-- marked source, so a marked source is not admitted by the schedulers. The reason is a closed
-- code, the note optional free text (required for OTHER), the actor a review actor name.
ALTER TABLE listing_sources
    ADD COLUMN purchase_channel TEXT NOT NULL DEFAULT 'ONLINE_SHOP',
    ADD COLUMN unscrapable_reason TEXT,
    ADD COLUMN unscrapable_note TEXT,
    ADD COLUMN unscrapable_since TIMESTAMPTZ,
    ADD COLUMN unscrapable_by TEXT,
    ADD CONSTRAINT listing_sources_purchase_channel_check
        CHECK (purchase_channel IN ('ONLINE_SHOP', 'ENQUIRY_ONLY')),
    ADD CONSTRAINT listing_sources_unscrapable_reason_check
        CHECK (unscrapable_reason IS NULL OR unscrapable_reason IN (
            'BOT_PROTECTION', 'JAVASCRIPT_RENDERED', 'LOGIN_REQUIRED', 'NO_PRODUCT_PAGES', 'OTHER')),
    ADD CONSTRAINT listing_sources_unscrapable_shape_check
        CHECK ((unscrapable_reason IS NULL AND unscrapable_note IS NULL
                AND unscrapable_since IS NULL AND unscrapable_by IS NULL)
            OR (unscrapable_reason IS NOT NULL AND unscrapable_since IS NOT NULL
                AND unscrapable_by IS NOT NULL)),
    ADD CONSTRAINT listing_sources_unscrapable_note_check
        CHECK (unscrapable_note IS NULL
            OR (btrim(unscrapable_note) <> '' AND char_length(unscrapable_note) <= 500)),
    ADD CONSTRAINT listing_sources_unscrapable_other_note_check
        CHECK (unscrapable_reason IS DISTINCT FROM 'OTHER' OR unscrapable_note IS NOT NULL),
    ADD CONSTRAINT listing_sources_unscrapable_by_check
        CHECK (unscrapable_by IS NULL
            OR unscrapable_by IN ('HUMAN', 'AGENT_AUTHOR', 'AGENT_REVIEWER', 'SYSTEM')),
    ADD CONSTRAINT listing_sources_unscrapable_not_crawled_check
        CHECK (unscrapable_reason IS NULL OR NOT crawl_enabled);
