-- Retires the product URLs of one ListingSource captured under another URL's identity that show
-- the same product (docs/crawler/scheduling-and-retries.md, captured identity): spellings of one
-- product whose first scrapes ran in one batch before the batch claimed the identities it
-- captured, so both kept capturing one listing. It applies the scraper's same-product rule
-- (src/crawler/src/scraper/same_product.rs) with the signals SQL expresses, and with no
-- signal it leaves the URLs as they are, since shops reuse one identity for different products,
-- such as a stock code after a sale. A URL is retired as DUPLICATE, with the write the scraper
-- makes for a retired duplicate, when the healthiest URL carrying the identity its page showed
-- (the captured one when none was recorded) is another URL and
--   * the two name one page apart from a trailing slash, doubled slashes, a language segment
--     before the path and query parameters that name no product (tracking, session, facet, sort,
--     view, currency, language and pager parameters, as of this migration), or
--   * the identity its page showed is a whole path segment or query value of its captured
--     identity, a link or path, or
--   * their captured titles are equal or one of them is missing.
-- A carrier is a URL whose page showed the identity it was captured under; the healthiest is the
-- one without an error, then the most recently scraped, then the earliest stored.
-- Deliberately left to the scraper's next scrape of each URL:
--   * URLs sharing an identity without any signal: distinct products under a reused code and
--     translations that differ beyond the parameters above keep capturing as before;
--   * URLs in an identity conflict, whose page showed an identity that was never stored;
--   * identities no URL carries consistently.
-- Re-running it changes nothing more.
CREATE OR REPLACE FUNCTION pg_temp.product_spelling(url TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
    WITH parts AS (
        SELECT substring(url FROM '^[^:/?#]+://[^/?#]*') AS origin,
               COALESCE(substring(url FROM '^[^:/?#]+://[^/?#]*([^?#]*)'), '') AS path,
               substring(url FROM '\?([^#]*)') AS query
    ), page AS (
        SELECT origin,
               query,
               regexp_replace(regexp_replace(path, '/{2,}', '/', 'g'), '(.)/+$', '\1') AS path
        FROM parts
    ), unprefixed AS (
        SELECT origin,
               query,
               regexp_replace(path, '^/[A-Za-z]{2,3}(?:[-_][A-Za-z0-9]{2,4})?(/.+)$', '\1') AS path
        FROM page
    )
    SELECT origin || path || COALESCE('?' || NULLIF((
        SELECT string_agg(pair, '&' ORDER BY position)
        FROM unnest(string_to_array(query, '&')) WITH ORDINALITY AS pairs(pair, position)
        WHERE lower(split_part(pair, '=', 1)) !~ (
            '^(?:utm_.*|fbclid|gclid|gbraid|wbraid|dclid|msclkid|yclid|igshid|srsltid|mc_cid'
            || '|mc_eid|_ga|_gl|oscsid|xtcsid|zenid|phpsessid|jsessionid|sessionid|variant'
            || '|filter_.*|filter\..*|filter(?:\[|%5b).*|query_type_.*|min_price|max_price'
            || '|sort|sort_by|sortby|orderby|order_by|order|product_list_order|product_list_dir'
            || '|view_mode|viewmode|product_list_mode|gridcookie|currency|currency_code|cur'
            || '|lang|language|l|lng|hl|locale|sprache|langue|lingua|idioma|taal|page|pg)$'
        )
    ), ''), '')
    FROM unprefixed
$$;

CREATE OR REPLACE FUNCTION pg_temp.is_identity_token_of(identity TEXT, captured TEXT) RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
    SELECT identity <> ''
       AND identity <> captured
       AND captured ~* '^(?:/|https?://)'
       AND (
           COALESCE(identity = ANY (string_to_array(
               substring(captured FROM '(?i)^(?:https?://[^/?#]*)?(/[^?#]*)'), '/')), FALSE)
           OR EXISTS (
               SELECT 1
               FROM unnest(string_to_array(substring(captured FROM '\?([^#]*)'), '&')) AS pair
               WHERE position('=' IN pair) > 0
                 AND substring(pair FROM position('=' IN pair) + 1) = identity
           )
       )
$$;

WITH candidates AS (
    SELECT url,
           listing_source_id,
           last_source_listing_id AS captured_identity,
           COALESCE(last_extracted_source_listing_id, last_source_listing_id) AS page_identity,
           last_extracted_source_listing_id IS NOT DISTINCT FROM last_source_listing_id
               AS carries_its_identity,
           last_captured_title AS title,
           last_error_kind,
           last_scraped,
           created
    FROM listing_source_urls
    WHERE url_class = 'product'
      AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
      AND last_source_listing_id IS NOT NULL
), carriers AS (
    SELECT DISTINCT ON (listing_source_id, captured_identity)
           url, listing_source_id, captured_identity, title
    FROM candidates
    WHERE carries_its_identity
    ORDER BY listing_source_id, captured_identity,
             (last_error_kind IS NULL) DESC, last_scraped DESC NULLS LAST, created, url
), retired AS (
    SELECT page.url
    FROM candidates AS page
    JOIN carriers AS carrier
      ON carrier.listing_source_id = page.listing_source_id
     AND carrier.captured_identity = page.page_identity
     AND carrier.url <> page.url
    WHERE pg_temp.product_spelling(page.url) = pg_temp.product_spelling(carrier.url)
       OR pg_temp.is_identity_token_of(page.page_identity, page.captured_identity)
       OR page.title IS NULL
       OR carrier.title IS NULL
       OR page.title = carrier.title
)
UPDATE listing_source_urls AS urls
SET crawler_disposition = 'DUPLICATE',
    next_retry_at = NULL,
    updated = NOW()
FROM retired
WHERE urls.url = retired.url;

DROP FUNCTION pg_temp.product_spelling(TEXT);
DROP FUNCTION pg_temp.is_identity_token_of(TEXT, TEXT);
