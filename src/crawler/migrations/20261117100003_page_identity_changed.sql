-- When a product URL's page last showed another identity than at its previous capture under
-- unchanged schemas (docs/crawler/scheduling-and-retries.md, captured identity): the shop likely
-- put another product at the URL, which the scrape still captures under the URL's captured
-- identity. The source overview counts the URLs that did so recently for an operator to check.
ALTER TABLE listing_source_urls
    ADD COLUMN page_identity_changed_at TIMESTAMPTZ;
