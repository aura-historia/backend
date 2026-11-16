-- Whether the product page a URL's latest successful scrape fetched (a capture, or a scrape that
-- found the page unchanged) made exactly one product, of schema.org's `Product` or a subtype, the
-- page's own in its structured data. NULL while no such scrape recorded it, for a page that showed
-- its item sold or unavailable, outside the authoring modes that run the check, after a captured
-- removal, and after the URL's class changed. The scraper reads the ListingSource's verdicts as
-- evidence before it classifies a failing page by its markup instead of opening a product-schema
-- review (docs/crawler/extraction-and-review.md, product markup pre-check). A nullable column
-- without a default adds without a table rewrite; existing rows record their verdict on their
-- next scrape.
ALTER TABLE listing_source_urls
    ADD COLUMN last_scraped_describes_one_product BOOLEAN;

-- The evidence of one ListingSource, read before a failing page is classified by its markup: the
-- verdicts of its live product URLs, which this index holds alone, so the read never touches the
-- source's other URLs or the table. Its predicate is the evidence query's, word for word.
CREATE INDEX idx_listing_source_urls_product_markup_verdict
    ON listing_source_urls (listing_source_id, last_scraped_describes_one_product)
    WHERE url_class = 'product'
      AND crawler_disposition = 'ACTIVE'
      AND last_scraped_describes_one_product IS NOT NULL;

-- A verdict describes the page as a product URL of its class: whichever write changes the URL's
-- class (the spider, a URL-pattern approval, a review verdict, the scraper) forgets it, so a URL
-- that becomes a product URL again carries no verdict from before. A verdict written in the same
-- statement as a class change is forgotten too: the trigger overrides it.
CREATE FUNCTION forget_product_markup_verdict_on_class_change()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.url_class IS DISTINCT FROM OLD.url_class THEN
        NEW.last_scraped_describes_one_product := NULL;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER listing_source_urls_forget_product_markup_verdict
    BEFORE UPDATE OF url_class ON listing_source_urls
    FOR EACH ROW
    EXECUTE FUNCTION forget_product_markup_verdict_on_class_change();
