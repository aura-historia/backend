-- Why a product URL was retired as `DUPLICATE` (docs/crawler/scheduling-and-retries.md, captured
-- identity): the product URL it duplicates, the evidence that showed one product and when. A
-- retired URL is never scraped again, so the record lets an operator find and undo a retirement
-- with one UPDATE that sets the URL `ACTIVE` and clears the three columns. Rows retired before
-- the record existed, and rows the spider collapses into their canonical spelling, have none.
-- `duplicate_of_url` is the carrier's stored URL, or for a redirect the destination as fetched,
-- which may differ from its stored row in a trailing slash.
-- The columns are nullable without a default, so adding them rewrites nothing; the checks are
-- added NOT VALID, so they hold for every write without scanning the existing rows, which all
-- carry no record.
ALTER TABLE listing_source_urls
    ADD COLUMN duplicate_of_url TEXT,
    ADD COLUMN duplicate_evidence TEXT,
    ADD COLUMN duplicate_retired_at TIMESTAMPTZ;

ALTER TABLE listing_source_urls
    ADD CONSTRAINT listing_source_urls_duplicate_record_complete CHECK (
        (duplicate_of_url IS NULL) = (duplicate_evidence IS NULL)
        AND (duplicate_of_url IS NULL) = (duplicate_retired_at IS NULL)
    ) NOT VALID,
    ADD CONSTRAINT listing_source_urls_duplicate_record_retired CHECK (
        duplicate_of_url IS NULL OR crawler_disposition = 'DUPLICATE'
    ) NOT VALID,
    ADD CONSTRAINT listing_source_urls_duplicate_evidence_known CHECK (
        duplicate_evidence IN ('REDIRECT_DESTINATION', 'SPELLING', 'IDENTITY_TOKEN', 'TITLE')
    ) NOT VALID;
