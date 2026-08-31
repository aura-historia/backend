-- `20260101000000_initial_schema.sql` accidentally used a double escaped
-- regex. Drop by name so both fresh and upgraded databases receive this exact
-- invariant. Canonical storage is already enforced by the registration adapter.
DROP INDEX IF EXISTS idx_listing_source_domains_canonical_domain;

ALTER TABLE listing_source_domains
    ADD COLUMN IF NOT EXISTS crawl_root_host TEXT;

UPDATE listing_source_domains
SET crawl_root_host = listing_source_domain
WHERE crawl_root_host IS NULL;

ALTER TABLE listing_source_domains
    ALTER COLUMN crawl_root_host SET NOT NULL;

-- Canonicalize legacy ownership keys before making their storage contract
-- explicit. A collision fails this migration rather than silently transferring
-- a domain between ListingSources.
UPDATE listing_source_domains
SET listing_source_domain = regexp_replace(
    lower(rtrim(listing_source_domain, '.')),
    '^www[.]',
    ''
);

ALTER TABLE listing_source_domains
    DROP CONSTRAINT IF EXISTS listing_source_domains_canonical_lower_check,
    DROP CONSTRAINT IF EXISTS listing_source_domains_canonical_trailing_dot_check,
    DROP CONSTRAINT IF EXISTS listing_source_domains_canonical_www_check,
    ADD CONSTRAINT listing_source_domains_canonical_lower_check
        CHECK (listing_source_domain = lower(listing_source_domain)),
    ADD CONSTRAINT listing_source_domains_canonical_trailing_dot_check
        CHECK (listing_source_domain = rtrim(listing_source_domain, '.')),
    ADD CONSTRAINT listing_source_domains_canonical_www_check
        CHECK (listing_source_domain !~ '^www[.]');

-- `UNIQUE (listing_source_domain)` from the base schema is now the ownership
-- invariant; no expression index is needed.
