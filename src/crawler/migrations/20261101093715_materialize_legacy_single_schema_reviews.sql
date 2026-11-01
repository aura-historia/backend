-- A pending legacy append or repair review (`append_schema_generation`,
-- `normalization_schema_repair`) holding a single schema used to have the ListingSource's live
-- schema list merged in front of it whenever it was read, edited or approved, outside its
-- candidate version. That merge is stored here once as the review's candidate: the live list,
-- followed by the review's schema unless the live list already holds it. The candidate version
-- moves and the cached schema matrix is dropped, as for any candidate change, so the review now
-- shows, edits and approves exactly what goes live
-- (docs/crawler/extraction-and-review.md, review rail). A review the merge leaves unchanged, or
-- whose ListingSource has no live list, keeps its candidate and version.
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
                   WHERE live_schema.schema = r.candidate_payload -> 'schemas' -> 0
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
    candidate_version = r.candidate_version + 1,
    validation_summary = CASE jsonb_typeof(r.validation_summary)
        WHEN 'object' THEN r.validation_summary - 'schema_matrix' - 'schema_matrix_summary'
        ELSE r.validation_summary
    END,
    updated = NOW()
FROM merged
WHERE r.review_id = merged.review_id
  AND merged.schemas <> r.candidate_payload -> 'schemas';
