-- When the live schema audit last audited the ListingSource's live product schemas (#3118;
-- docs/crawler/extraction-and-review.md, live schema audit). NULL until its first audit, which
-- comes first: the audit takes the sources it never audited, then the longest ago.
ALTER TABLE listing_sources
    ADD COLUMN schema_audited_at TIMESTAMPTZ;
