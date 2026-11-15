-- Ultimate's active saved searches are capped at 20 (issue #2981); until now the tier had no cap.
-- Tier reconciliation only runs when a tier changes, so accounts already above the cap are
-- brought to it once here, ranked as that reconciliation ranks: the newest 20 stay ACTIVE and the
-- rest become INACTIVE_BY_RESTRICTED_PLAN with their search intact, so a later plan change
-- restores them. The literal is the cap at this release; the cap enforced at runtime is
-- UserTier::active_search_filter_quota. Re-running changes nothing.
WITH ranked AS (
    SELECT
        filter.user_search_filter_id,
        row_number() OVER (
            PARTITION BY filter.user_id
            ORDER BY filter.created DESC, filter.user_search_filter_id ASC
        ) AS active_rank
    FROM search_filters AS filter
    JOIN users AS account ON account.user_id = filter.user_id
    WHERE account.tier = 'ULTIMATE'
      AND filter.state = 'ACTIVE'
)
UPDATE search_filters AS filter
SET state = 'INACTIVE_BY_RESTRICTED_PLAN',
    version = filter.version + 1,
    updated = aura_advance_updated(filter.updated)
FROM ranked
WHERE filter.user_search_filter_id = ranked.user_search_filter_id
  AND ranked.active_rank > 20;
