-- Backfill capabilities under the same authoritative entitlement and quota rules
-- as subscription reconciliation. User-authored search intent remains intact.
-- Serialize tier changes before taking the filter schema lock, preserving the
-- normal authority-before-filter lock order throughout migration admission.
LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE;
ALTER TABLE search_filters ADD COLUMN matching_evidence_policy text;

WITH candidates AS (
    SELECT filter.user_search_filter_id, filter.user_id, filter.created,
           CASE users.tier
               WHEN 'ULTIMATE' THEN 'ENHANCED_TEXT_AND_FOUR_IMAGES'
               WHEN 'PRO' THEN 'TEXT_AND_ONE_IMAGE'
               ELSE 'TEXT_ONLY'
           END AS policy,
           CASE
               WHEN users.tier = 'ULTIMATE' THEN true
               WHEN users.tier = 'PRO' THEN filter.enhanced_search_description IS NULL
               WHEN users.tier = 'FREE' THEN
                   filter.enhanced_search_description IS NULL
                   AND coalesce(jsonb_array_length(search -> 'listing_source_id_query'), 0) = 0
                   AND coalesce(jsonb_array_length(search -> 'exclude_listing_source_id_query'), 0) = 0
                   AND coalesce(jsonb_array_length(search -> 'auction_id_query'), 0) = 0
                   AND search ->> 'created_query' IS NULL
                   AND search ->> 'updated_query' IS NULL
                   AND search ->> 'lot_bidding_opens_query' IS NULL
                   AND search ->> 'lot_scheduled_closes_query' IS NULL
               ELSE false
           END AS feature_allowed,
           CASE users.tier WHEN 'FREE' THEN 1 WHEN 'PRO' THEN 5 ELSE 9223372036854775807 END AS quota,
           filter.state
    FROM search_filters filter JOIN users USING (user_id)
), ranked AS (
    SELECT *, sum(CASE WHEN feature_allowed AND state <> 'INACTIVE_BY_USER' THEN 1 ELSE 0 END)
        OVER (PARTITION BY user_id ORDER BY created DESC, user_search_filter_id ASC) AS eligible_rank
    FROM candidates
)
UPDATE search_filters filter
SET matching_evidence_policy = ranked.policy,
    state = CASE WHEN ranked.state = 'INACTIVE_BY_USER' THEN ranked.state
                 WHEN ranked.feature_allowed AND ranked.eligible_rank <= ranked.quota THEN 'ACTIVE'
                 ELSE 'INACTIVE_BY_RESTRICTED_PLAN' END,
    version = filter.version + 1,
    updated = now()
FROM ranked WHERE filter.user_search_filter_id = ranked.user_search_filter_id;

ALTER TABLE search_filters
    ALTER COLUMN matching_evidence_policy SET NOT NULL,
    ADD CONSTRAINT search_filters_matching_evidence_policy_check CHECK (
        matching_evidence_policy IN ('TEXT_ONLY', 'TEXT_AND_ONE_IMAGE', 'ENHANCED_TEXT_AND_FOUR_IMAGES')
    );

-- Query-only intent uses lexical retrieval and needs no embedding. Enhanced
-- retrieval uses the existing write-time base-query + description embedding.
-- Abort admission if that prerequisite is missing; operators repair it on writes
-- before enabling the expanded matcher, never during a matching run.
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM search_filters WHERE state IN ('ACTIVE', 'INACTIVE_BY_RESTRICTED_PLAN')
               AND enhanced_search_description IS NOT NULL AND embedding IS NULL) THEN
        RAISE EXCEPTION 'Active enhanced filters require write-time embedding backfill before matcher admission';
    END IF;
END $$;

ALTER TABLE search_filters ADD CONSTRAINT search_filters_active_enhanced_matching_check CHECK (
    state <> 'ACTIVE' OR enhanced_search_description IS NULL OR (
        matching_evidence_policy = 'ENHANCED_TEXT_AND_FOUR_IMAGES' AND embedding IS NOT NULL
    )
);

DROP INDEX search_filters_periodic_match_eligible_idx;
CREATE INDEX search_filters_periodic_match_eligible_idx ON search_filters (user_search_filter_id)
    WHERE state = 'ACTIVE';
