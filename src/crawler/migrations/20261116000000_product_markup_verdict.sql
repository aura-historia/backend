-- Whether the product page a URL's latest successful scrape fetched (a capture, or a scrape
-- that found the page unchanged) described exactly one schema.org Product in its structured
-- data; NULL while no such scrape recorded it, and again after a captured removal. The scraper
-- reads the ListingSource's verdicts as evidence before it classifies a failing page by its
-- markup instead of opening a product-schema review (docs/crawler/extraction-and-review.md,
-- product markup pre-check). A nullable column without a default adds without a table rewrite;
-- existing rows record their verdict on their next scrape.
ALTER TABLE listing_source_urls
    ADD COLUMN last_scraped_describes_one_product BOOLEAN;
