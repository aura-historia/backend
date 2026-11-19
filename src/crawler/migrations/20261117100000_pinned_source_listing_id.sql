-- A product URL keeps the source listing id its listing was first captured under: a later scrape of
-- the URL itself captures under it whatever the schemas' identity rule derives now
-- (docs/crawler/scheduling-and-retries.md, captured identity). `last_extracted_source_listing_id`
-- is what the selected schema derived from the page at the URL's last capture, NULL when it
-- derived none or before the column existed: duplicate spellings are found by it as well as by the
-- captured identity, and a change of it under unchanged schemas is reported as another product on
-- the page. The check is added NOT VALID, so it holds for every write without scanning the rows.
ALTER TABLE listing_source_urls
    ADD COLUMN last_extracted_source_listing_id TEXT;

ALTER TABLE listing_source_urls
    ADD CONSTRAINT listing_source_urls_last_extracted_source_listing_id_nonempty CHECK (
        last_extracted_source_listing_id IS NULL
        OR char_length(last_extracted_source_listing_id) > 0
    ) NOT VALID;
