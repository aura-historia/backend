-- Why a product URL was retired as `DUPLICATE` (docs/crawler/scheduling-and-retries.md, captured
-- identity): the product URL it duplicates, the evidence that showed one product and when. A
-- retired URL is never scraped again, so the record lets an operator find and undo a retirement
-- with one UPDATE that sets the URL `ACTIVE` and clears the three columns. Rows retired before
-- the record existed, and rows the spider collapses into their canonical spelling, have none.
ALTER TABLE listing_source_urls
    ADD COLUMN duplicate_of_url TEXT,
    ADD COLUMN duplicate_evidence TEXT,
    ADD COLUMN duplicate_retired_at TIMESTAMPTZ,
    ADD CONSTRAINT listing_source_urls_duplicate_record_complete CHECK (
        (duplicate_of_url IS NULL) = (duplicate_evidence IS NULL)
        AND (duplicate_of_url IS NULL) = (duplicate_retired_at IS NULL)
    ),
    ADD CONSTRAINT listing_source_urls_duplicate_record_retired CHECK (
        duplicate_of_url IS NULL OR crawler_disposition = 'DUPLICATE'
    ),
    ADD CONSTRAINT listing_source_urls_duplicate_evidence_known CHECK (
        duplicate_evidence IN ('REDIRECT_DESTINATION', 'SPELLING', 'IDENTITY_TOKEN', 'TITLE')
    );
