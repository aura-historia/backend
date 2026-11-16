-- A resolution can resolve the place a caller picked from address suggestions
-- (docs/geography.md, address suggestions). Its input records the provider's place reference
-- beside the free-text fallback, so only the policy vocabulary changes.
ALTER TABLE geo_resolutions DROP CONSTRAINT geo_resolutions_policy_version_check;
ALTER TABLE geo_resolutions ADD CONSTRAINT geo_resolutions_policy_version_check
    CHECK (policy_version IN ('PRECISION_FIRST_V1','AUTOMATIC_USER_BROWSING_V1','SELECTED_PLACE_V1'));

-- Per-user address suggestion quota (docs/geography.md, address suggestions): the current
-- minute and UTC-day windows only. Suggestion queries and results are never stored.
CREATE TABLE address_suggestion_usage (
    user_id uuid PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    minute_start timestamptz NOT NULL,
    minute_uses integer NOT NULL CHECK (minute_uses >= 0),
    day_start timestamptz NOT NULL,
    day_uses integer NOT NULL CHECK (day_uses >= 0)
);
