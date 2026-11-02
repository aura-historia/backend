-- The title of a URL's last product capture, carried forward when the shop later blanks the name
-- (docs/crawler/extraction-and-review.md, missing required values), and the stored removed-page
-- templates whose match captured its last removal, so retiring or narrowing one of them re-arms
-- the URL (docs/crawler/scheduling-and-retries.md, fetch retries). A captured removal clears the
-- title; any other last capture leaves no templates. Existing rows start without either: a
-- captured URL gains its title at its next capture.
ALTER TABLE listing_source_urls
    ADD COLUMN last_captured_title TEXT,
    ADD COLUMN removed_by_templates JSONB,
    ADD CONSTRAINT listing_source_urls_last_captured_title_nonblank
        CHECK (last_captured_title IS NULL OR btrim(last_captured_title) <> ''),
    ADD CONSTRAINT listing_source_urls_removed_by_templates_shape
        CHECK (removed_by_templates IS NULL
               OR (jsonb_typeof(removed_by_templates) = 'array'
                   AND jsonb_array_length(removed_by_templates) > 0));
