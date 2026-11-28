-- The pages scraped in each clock hour by what they came to; `pages_scraped` is their sum from
-- now on. Hours recorded before this migration keep only the total, which the console shows as
-- not split. The contract is in `docs/crawler/operations.md#activity-history`.
ALTER TABLE crawler_activity_hours
    ADD COLUMN pages_new BIGINT NOT NULL DEFAULT 0 CHECK (pages_new >= 0),
    ADD COLUMN pages_changed BIGINT NOT NULL DEFAULT 0 CHECK (pages_changed >= 0),
    ADD COLUMN pages_unchanged BIGINT NOT NULL DEFAULT 0 CHECK (pages_unchanged >= 0),
    ADD COLUMN pages_removed BIGINT NOT NULL DEFAULT 0 CHECK (pages_removed >= 0),
    ADD COLUMN pages_skipped BIGINT NOT NULL DEFAULT 0 CHECK (pages_skipped >= 0);
