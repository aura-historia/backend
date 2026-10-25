-- A product-schema review's candidate holds the ListingSource's removed-page templates as the
-- reviewer would have them, and approval replaces the stored list with it. The base is the
-- stored list the candidate started from, so approval keeps a template another writer stored
-- since then, such as a generated one, instead of dropping it.
ALTER TABLE crawler_reviews
    ADD COLUMN removed_page_schemas_base JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD CONSTRAINT crawler_reviews_removed_page_schemas_base_shape_check
        CHECK (jsonb_typeof(removed_page_schemas_base) = 'array');

-- A pending review opened while approval only appended holds just the templates it declares.
-- It starts from the stored list instead, followed by the declared templates not stored yet, so
-- approving it stores what approving it before would have stored. A stored row that does not
-- hold a list is left alone: approval fails on it either way.
UPDATE crawler_reviews AS review
SET removed_page_schemas_base = stored.removed_page_schema,
    candidate_payload = jsonb_set(
        review.candidate_payload,
        '{removed_page_schemas}',
        stored.removed_page_schema || COALESCE((
            SELECT jsonb_agg(declared.template ORDER BY declared.position)
            FROM jsonb_array_elements(
                     COALESCE(review.candidate_payload -> 'removed_page_schemas', '[]'::jsonb)
                 ) WITH ORDINALITY AS declared(template, position)
            WHERE NOT EXISTS (
                SELECT 1
                FROM jsonb_array_elements(stored.removed_page_schema) AS kept(template)
                WHERE kept.template = declared.template
            )
        ), '[]'::jsonb),
        true
    )
FROM listing_source_removed_page_schemas AS stored
WHERE stored.listing_source_id = review.listing_source_id
  AND review.artifact_type = 'PRODUCT_SCHEMA'
  AND review.status = 'PENDING_REVIEW'
  AND jsonb_typeof(review.candidate_payload) = 'object'
  AND jsonb_typeof(COALESCE(review.candidate_payload -> 'removed_page_schemas', '[]'::jsonb)) = 'array'
  AND jsonb_typeof(stored.removed_page_schema) = 'array'
  AND stored.removed_page_schema <> '[]'::jsonb;
