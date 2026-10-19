-- Reconcile existing FREE filters under the same feature and quota rules as
-- subscription changes, preserving authored criteria and manual deactivation.
-- Serialize writers in the normal user-before-filter lock order.
LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE;
LOCK TABLE search_filters IN SHARE ROW EXCLUSIVE MODE;

WITH candidates AS (
    SELECT filter.user_search_filter_id, filter.user_id, filter.created,
           filter.enhanced_search_description IS NULL
           AND coalesce(jsonb_array_length(search -> 'listing_source_id_query'), 0) = 0
           AND coalesce(jsonb_array_length(search -> 'exclude_listing_source_id_query'), 0) = 0
           AND coalesce(jsonb_array_length(search -> 'auction_id_query'), 0) = 0
           AND search ->> 'created_query' IS NULL
           AND search ->> 'updated_query' IS NULL
           AND search ->> 'lot_bidding_opens_query' IS NULL
           AND search ->> 'lot_scheduled_closes_query' IS NULL
           AND search ->> 'geography' IS NULL
           AND search ->> 'shipping' IS NULL AS feature_allowed
    FROM search_filters filter JOIN users USING (user_id)
    WHERE users.tier = 'FREE'
      AND filter.state IN ('ACTIVE', 'INACTIVE_BY_RESTRICTED_PLAN')
), ranked AS (
    SELECT *, sum(CASE WHEN feature_allowed THEN 1 ELSE 0 END)
        OVER (PARTITION BY user_id ORDER BY created DESC, user_search_filter_id ASC) AS eligible_rank
    FROM candidates
), reconciled AS (
    SELECT user_search_filter_id,
           CASE WHEN feature_allowed AND eligible_rank <= 1 THEN 'ACTIVE'
                ELSE 'INACTIVE_BY_RESTRICTED_PLAN' END AS target_state
    FROM ranked
)
UPDATE search_filters filter
SET state = reconciled.target_state,
    version = filter.version + 1,
    updated = aura_advance_updated(filter.updated)
FROM reconciled
WHERE filter.user_search_filter_id = reconciled.user_search_filter_id
  AND filter.state IS DISTINCT FROM reconciled.target_state;
