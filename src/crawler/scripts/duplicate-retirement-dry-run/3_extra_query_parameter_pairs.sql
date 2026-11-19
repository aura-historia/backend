-- READ-ONLY dry run for https://github.com/aura-historia/backend/issues/3067. It changes nothing:
-- it runs in a READ ONLY transaction that is rolled back, and uses no functions or temporary
-- objects, so it also runs on a read replica. See README.md in this directory.
\set ON_ERROR_STOP on
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '30min';

-- 3. "Same path, one URL with extra query parameters" (#3067 scope 3): live product URLs of one
--    ListingSource whose spelling differs from another live product URL's only by a query the
--    other lacks, grouped by host and the extra parameter names, with how often the two show
--    one identity and one title. A parameter such as ?id= can name another product, so only a
--    name whose pairs agree throughout is a candidate for spelling evidence.
WITH live AS (
    SELECT listing_source_id,
           url,
           lower(substring(url FROM '^[^:/?#]+://([^/?#:]*)')) AS host,
           substring(url FROM '^[^:/?#]+://[^/?#]*')
           || regexp_replace(
                  regexp_replace(
                      regexp_replace(COALESCE(substring(url FROM '^[^:/?#]+://[^/?#]*([^?#]*)'), ''), '/{2,}', '/', 'g'),
                      '(.)/+$', '\1'),
                  '^/[A-Za-z]{2,3}(?:[-_][A-Za-z0-9]{2,4})?(/.+)$', '\1') AS path_spelling,
           NULLIF((SELECT string_agg(pair, '&' ORDER BY position)
            FROM unnest(string_to_array(substring(url FROM '\?([^#]*)'), '&')) WITH ORDINALITY AS pairs(pair, position)
            WHERE NOT (position('=' IN pair) > 0
                       AND ((lower(split_part(pair, '=', 1)) IN ('lang','language','l','lng','hl','locale','sprache','langue','lingua','idioma','taal')
                             AND substring(pair FROM position('=' IN pair) + 1) ~ '^[A-Za-z]{2,3}(?:[-_][A-Za-z0-9]{2,4})?$')
                         OR (lower(split_part(pair, '=', 1)) IN ('page', 'pg')
                             AND substring(pair FROM position('=' IN pair) + 1) ~ '^[0-9]+$')))), '') AS query,
           last_source_listing_id AS captured_identity,
           last_extracted_source_listing_id AS page_identity,
           last_captured_title AS title
    FROM listing_source_urls
    WHERE url_class = 'product'
      AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
), pairs AS (
    SELECT with_query.host,
           (SELECT string_agg(DISTINCT lower(split_part(pair, '=', 1)), '&')
            FROM unnest(string_to_array(with_query.query, '&')) AS pair) AS extra_parameters,
           (SELECT string_agg(DISTINCT pair, '&')
            FROM unnest(string_to_array(with_query.query, '&')) AS pair) AS extra_query,
           with_query.captured_identity IS NOT DISTINCT FROM plain.captured_identity
               AND with_query.captured_identity IS NOT NULL AS same_captured_identity,
           with_query.page_identity = plain.page_identity AS same_page_identity,
           with_query.title = plain.title AS same_title,
           with_query.title IS NULL OR plain.title IS NULL AS a_title_missing
    FROM live AS with_query
    JOIN live AS plain
      ON plain.listing_source_id = with_query.listing_source_id
     AND plain.path_spelling = with_query.path_spelling
     AND plain.query IS NULL
     AND plain.url <> with_query.url
    WHERE with_query.query IS NOT NULL
)
SELECT host,
       extra_parameters,
       count(*) AS pairs,
       count(DISTINCT extra_query) AS distinct_values,
       min(extra_query) AS sample_value,
       count(*) FILTER (WHERE same_captured_identity) AS same_captured_identity,
       count(*) FILTER (WHERE same_page_identity) AS same_page_identity,
       count(*) FILTER (WHERE same_page_identity IS FALSE) AS other_page_identity,
       count(*) FILTER (WHERE same_page_identity IS NULL) AS page_identity_unknown,
       count(*) FILTER (WHERE same_title) AS same_title,
       count(*) FILTER (WHERE a_title_missing) AS a_title_missing
FROM pairs
GROUP BY host, extra_parameters
ORDER BY host LIKE '%antiquariat-neidhardt.de' DESC, pairs DESC, host, extra_parameters;

ROLLBACK;
