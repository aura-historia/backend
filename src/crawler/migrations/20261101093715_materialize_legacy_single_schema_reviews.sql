-- A pending legacy append or repair review (`append_schema_generation`,
-- `normalization_schema_repair`) holding a single schema used to have the ListingSource's live
-- schema list merged in front of it whenever it was read, edited or approved, outside its
-- candidate version. That merge is stored here once as the review's candidate: the live list,
-- followed by the review's schema unless the live list already holds it. The candidate version
-- trigger moves the version and drops the cached schema matrix, as for any candidate change, so
-- the review shows, edits and approves exactly what goes live
-- (docs/crawler/extraction-and-review.md, review rail). A review the merge leaves unchanged, or
-- whose ListingSource has no live list, keeps its candidate and version.

-- A rule as the crawler serializes it: no empty `additional_selectors`.
CREATE FUNCTION pg_temp.canonical_rule(rule JSONB) RETURNS JSONB
LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE
        WHEN jsonb_typeof(rule) = 'object' AND rule -> 'additional_selectors' = '[]'::jsonb
            THEN rule - 'additional_selectors'
        ELSE rule
    END
$$;

-- A schema as the crawler serializes it, so two spellings of one schema compare equal, as the
-- decoded schemas the merge compared did: no null fields, raw attributes under
-- `raw_attributes` rather than the `rawAttributes` alias and left out when empty, and canonical
-- rules.
CREATE FUNCTION pg_temp.canonical_schema(schema JSONB) RETURNS JSONB
LANGUAGE sql IMMUTABLE AS $$
    WITH stripped AS (
        SELECT jsonb_strip_nulls(schema) AS schema
    ),
    raw_attributes AS (
        SELECT COALESCE(
            (
                SELECT jsonb_object_agg(key, pg_temp.canonical_rule(value))
                FROM jsonb_each(
                    CASE WHEN jsonb_typeof(raw.rules) = 'object' THEN raw.rules ELSE '{}'::jsonb END
                )
            ),
            '{}'::jsonb
        ) AS rules
        FROM stripped,
             LATERAL (
                 SELECT COALESCE(
                     stripped.schema -> 'raw_attributes', stripped.schema -> 'rawAttributes'
                 ) AS rules
             ) AS raw
    )
    SELECT COALESCE(
               (
                   SELECT jsonb_object_agg(key, pg_temp.canonical_rule(value))
                   FROM jsonb_each(
                       CASE
                           WHEN jsonb_typeof(stripped.schema) = 'object' THEN stripped.schema
                           ELSE '{}'::jsonb
                       END
                   )
                   WHERE key NOT IN ('raw_attributes', 'rawAttributes')
               ),
               '{}'::jsonb
           )
           || CASE
                  WHEN raw_attributes.rules = '{}'::jsonb THEN '{}'::jsonb
                  ELSE jsonb_build_object('raw_attributes', raw_attributes.rules)
              END
    FROM stripped, raw_attributes
    WHERE jsonb_typeof(stripped.schema) = 'object'
    UNION ALL
    SELECT schema WHERE jsonb_typeof(schema) <> 'object'
$$;

WITH live AS (
    SELECT listing_source_id,
           CASE jsonb_typeof(product_schema)
               WHEN 'array' THEN product_schema
               ELSE jsonb_build_array(product_schema)
           END AS schemas
    FROM listing_source_product_schemas
),
merged AS (
    SELECT r.review_id,
           live.schemas || CASE
               WHEN EXISTS (
                   SELECT 1 FROM jsonb_array_elements(live.schemas) AS live_schema(schema)
                   WHERE pg_temp.canonical_schema(live_schema.schema)
                       = pg_temp.canonical_schema(r.candidate_payload -> 'schemas' -> 0)
               ) THEN '[]'::jsonb
               ELSE r.candidate_payload -> 'schemas'
           END AS schemas
    FROM crawler_reviews r
    JOIN live ON live.listing_source_id = r.listing_source_id
    WHERE r.artifact_type = 'PRODUCT_SCHEMA'
      AND r.status = 'PENDING_REVIEW'
      AND r.reason IN ('append_schema_generation', 'normalization_schema_repair')
      AND jsonb_typeof(r.candidate_payload) = 'object'
      AND jsonb_typeof(r.candidate_payload -> 'schemas') = 'array'
      AND jsonb_array_length(r.candidate_payload -> 'schemas') = 1
)
UPDATE crawler_reviews r
SET candidate_payload = jsonb_set(r.candidate_payload, '{schemas}', merged.schemas),
    updated = NOW()
FROM merged
WHERE r.review_id = merged.review_id
  AND merged.schemas <> r.candidate_payload -> 'schemas';

DROP FUNCTION pg_temp.canonical_schema(JSONB);
DROP FUNCTION pg_temp.canonical_rule(JSONB);
