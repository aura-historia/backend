-- AGENTS_UNRESOLVED (#2990): the label the review agents park a ListingSource with when even their
-- final round could not finish its review, and that they retry on their own once it expires
-- (docs/crawler/README.md, ListingSource scope). Nothing existing changes: every label persisted
-- before stays valid.
ALTER TABLE listing_sources
    DROP CONSTRAINT listing_sources_source_label_check,
    ADD CONSTRAINT listing_sources_source_label_check
        CHECK (source_label IS NULL OR source_label IN (
            'NOT_AN_ESHOP', 'NOT_AN_ANTIQUE_SHOP',
            'URL_PATTERN_APPROACH_NOT_APPLICABLE', 'PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE',
            'ROBOTS_DISALLOWED', 'SHOP_CLOSED', 'BOT_PROTECTION', 'JAVASCRIPT_RENDERED',
            'LOGIN_REQUIRED', 'AGENTS_UNRESOLVED', 'OTHER', 'APPROACH_NOT_APPLICABLE'));

ALTER TABLE listing_source_label_events
    DROP CONSTRAINT listing_source_label_events_source_label_check,
    ADD CONSTRAINT listing_source_label_events_source_label_check
        CHECK (source_label IS NULL OR source_label IN (
            'NOT_AN_ESHOP', 'NOT_AN_ANTIQUE_SHOP',
            'URL_PATTERN_APPROACH_NOT_APPLICABLE', 'PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE',
            'ROBOTS_DISALLOWED', 'SHOP_CLOSED', 'BOT_PROTECTION', 'JAVASCRIPT_RENDERED',
            'LOGIN_REQUIRED', 'AGENTS_UNRESOLVED', 'OTHER', 'APPROACH_NOT_APPLICABLE'));
