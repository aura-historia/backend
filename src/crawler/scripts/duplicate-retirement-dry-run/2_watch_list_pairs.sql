-- READ-ONLY dry run for https://github.com/aura-historia/backend/issues/3067. It changes nothing:
-- it runs in a READ ONLY transaction that is rolled back, and uses no functions or temporary
-- objects, so it also runs on a read replica. See README.md in this directory.
\set ON_ERROR_STOP on
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '30min';

-- 2. Every pair on the suspect, ambiguous and list-page hosts named in #3067, with what decides
--    it, to spot-check against the live pages before any migration retires them.
WITH live AS (
    -- Live product URLs captured under an identity, as the scraper reads them.
    SELECT listing_source_id,
           url,
           lower(substring(url FROM '^[^:/?#]+://([^/?#:]*)')) AS host,
           last_source_listing_id AS captured_identity,
           last_extracted_source_listing_id AS page_identity,
           last_captured_title AS title,
           last_error_kind,
           created
    FROM listing_source_urls
    WHERE url_class = 'product'
      AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
      AND last_source_listing_id IS NOT NULL
), carriers AS (
    -- The carrier of an identity, as identity_carrier() in src/crawler/src/scraper/candidate_service.rs
    -- picks it: the earliest live URL captured under it whose page showed it at its last capture,
    -- not in an identity conflict.
    SELECT DISTINCT ON (listing_source_id, captured_identity)
           listing_source_id, captured_identity AS identity, url, title
    FROM live
    WHERE page_identity = captured_identity
      AND last_error_kind IS DISTINCT FROM 'SourceListingIdConflict'
    ORDER BY listing_source_id, captured_identity, created, url
), pairs AS (
    -- Every other live URL whose page showed an identity a carrier carries (double captures).
    SELECT page.host,
           page.listing_source_id,
           page.url,
           carrier.url AS carrier_url,
           page.captured_identity,
           page.page_identity,
           page.title,
           carrier.title AS carrier_title
    FROM live AS page
    JOIN carriers AS carrier
      ON carrier.listing_source_id = page.listing_source_id
     AND carrier.identity = page.page_identity
     AND carrier.url <> page.url
    WHERE page.last_error_kind IS DISTINCT FROM 'SourceListingIdConflict'
), judged AS (
    -- The scraper's same-product rule (src/crawler/src/scraper/same_product.rs), checked in its
    -- order: spelling, identity token, title. NULL: no evidence, the URL stays.
    SELECT pairs.*,
           CASE
               WHEN (substring(pairs.url FROM '^[^:/?#]+://[^/?#]*')
           || regexp_replace(
                  regexp_replace(
                      regexp_replace(COALESCE(substring(pairs.url FROM '^[^:/?#]+://[^/?#]*([^?#]*)'), ''), '/{2,}', '/', 'g'),
                      '(.)/+$', '\1'),
                  '^/[A-Za-z]{2,3}(?:[-_][A-Za-z0-9]{2,4})?(/.+)$', '\1')
           || COALESCE('?' || NULLIF((SELECT string_agg(pair, '&' ORDER BY position)
            FROM unnest(string_to_array(substring(pairs.url FROM '\?([^#]*)'), '&')) WITH ORDINALITY AS pairs(pair, position)
            WHERE NOT (position('=' IN pair) > 0
                       AND ((lower(split_part(pair, '=', 1)) IN ('lang','language','l','lng','hl','locale','sprache','langue','lingua','idioma','taal')
                             AND substring(pair FROM position('=' IN pair) + 1) ~ '^[A-Za-z]{2,3}(?:[-_][A-Za-z0-9]{2,4})?$')
                         OR (lower(split_part(pair, '=', 1)) IN ('page', 'pg')
                             AND substring(pair FROM position('=' IN pair) + 1) ~ '^[0-9]+$')))), ''), '')) = (substring(pairs.carrier_url FROM '^[^:/?#]+://[^/?#]*')
           || regexp_replace(
                  regexp_replace(
                      regexp_replace(COALESCE(substring(pairs.carrier_url FROM '^[^:/?#]+://[^/?#]*([^?#]*)'), ''), '/{2,}', '/', 'g'),
                      '(.)/+$', '\1'),
                  '^/[A-Za-z]{2,3}(?:[-_][A-Za-z0-9]{2,4})?(/.+)$', '\1')
           || COALESCE('?' || NULLIF((SELECT string_agg(pair, '&' ORDER BY position)
            FROM unnest(string_to_array(substring(pairs.carrier_url FROM '\?([^#]*)'), '&')) WITH ORDINALITY AS pairs(pair, position)
            WHERE NOT (position('=' IN pair) > 0
                       AND ((lower(split_part(pair, '=', 1)) IN ('lang','language','l','lng','hl','locale','sprache','langue','lingua','idioma','taal')
                             AND substring(pair FROM position('=' IN pair) + 1) ~ '^[A-Za-z]{2,3}(?:[-_][A-Za-z0-9]{2,4})?$')
                         OR (lower(split_part(pair, '=', 1)) IN ('page', 'pg')
                             AND substring(pair FROM position('=' IN pair) + 1) ~ '^[0-9]+$')))), ''), '')) THEN 'SPELLING'
               WHEN pairs.page_identity <> ''
                AND pairs.page_identity <> pairs.captured_identity
                AND pairs.captured_identity ~* '^(?:/|https?://)'
                AND (COALESCE(pairs.page_identity = ANY (string_to_array(
                         substring(pairs.captured_identity FROM '(?i)^(?:https?://[^/?#]*)?(/[^?#]*)'), '/')), FALSE)
                     OR EXISTS (
                         SELECT 1
                         FROM unnest(string_to_array(substring(pairs.captured_identity FROM '\?([^#]*)'), '&')) AS pair
                         WHERE position('=' IN pair) > 0
                           AND substring(pair FROM position('=' IN pair) + 1) = pairs.page_identity))
                   THEN 'IDENTITY_TOKEN'
               WHEN pairs.title = pairs.carrier_title
                AND pairs.title !~ '^[\x09-\x0d\x20-\x40\x5b-\x60\x7b-\x7e]*$'
                AND NOT (COALESCE(substring(pairs.url FROM '^[^:/?#]+://[^/?#]*([^?#]*)'), '') ~* '/(?:page|seite|pagina)(?:[-_]?|/)[0-9]+(?:/|$)')
                AND NOT (COALESCE(substring(pairs.carrier_url FROM '^[^:/?#]+://[^/?#]*([^?#]*)'), '') ~* '/(?:page|seite|pagina)(?:[-_]?|/)[0-9]+(?:/|$)')
                   THEN 'TITLE'
           END AS evidence
    FROM pairs
)
SELECT watch_list, host, evidence, url, carrier_url, captured_identity, page_identity,
       title, carrier_title
FROM (SELECT judged.*, CASE
               WHEN host ~ '(^|\.)(deglantier\.com|arcadiaantiques\.co\.uk|marrisantiqueglass\.com|sycomore-ancient-art\.com|ririe93\.ocnk\.net|arsartis\.at|oldprintshop\.com|buch-antiquariat\.ch)$' THEN 'SUSPECT'
               WHEN host ~ '(libreriacompraventa|antiquariaat-artemis|sansantique)' THEN 'AMBIGUOUS'
               WHEN host ~ '(^|\.)antiquariat-neidhardt\.de$' THEN 'QUERY_SPELLING'
               WHEN host ~ '(^|\.)(gillmark\.com|r-select\.jp|koyonaku\.jp)$' THEN 'LIST_PAGE_CARRIER'
               ELSE ''
           END AS watch_list FROM judged) AS listed
WHERE watch_list <> ''
ORDER BY watch_list, host, evidence NULLS LAST, carrier_url, url;

ROLLBACK;
