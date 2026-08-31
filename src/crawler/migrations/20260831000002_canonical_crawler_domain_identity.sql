-- Fail closed if legacy rows already give the same bare/www identity to
-- different ListingSources. Resolve those ownership conflicts before rerunning.
CREATE UNIQUE INDEX IF NOT EXISTS idx_listing_source_domains_canonical_domain
    ON listing_source_domains (
        lower(regexp_replace(rtrim(listing_source_domain, '.'), '^www\.', ''))
    );
