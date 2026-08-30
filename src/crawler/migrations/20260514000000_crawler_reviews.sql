CREATE TABLE IF NOT EXISTS crawler_reviews (
    review_id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    listing_source_id            UUID        NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    domain_id          UUID        REFERENCES listing_source_domains(domain_id) ON DELETE SET NULL,
    artifact_type      TEXT        NOT NULL CHECK (artifact_type IN ('URL_PATTERN', 'PRODUCT_SCHEMA')),
    CHECK (artifact_type <> 'URL_PATTERN' OR domain_id IS NOT NULL),
    status             TEXT        NOT NULL CHECK (status IN ('PENDING_REVIEW', 'APPROVED', 'REJECTED', 'NEEDS_REPAIR', 'SUPERSEDED')),
    reason             TEXT        NOT NULL,
    candidate_payload  JSONB       NOT NULL,
    validation_summary JSONB       NOT NULL DEFAULT '{}'::jsonb,
    reviewer_notes     TEXT,
    created            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    reviewed           TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_crawler_reviews_status
    ON crawler_reviews (status, artifact_type, created);

DROP INDEX IF EXISTS idx_crawler_reviews_shop_pending;
DROP INDEX IF EXISTS idx_crawler_reviews_shop_pending_unique;

CREATE UNIQUE INDEX IF NOT EXISTS crawler_reviews_pending_url_pattern_per_domain
    ON crawler_reviews (domain_id)
    WHERE status = 'PENDING_REVIEW'
      AND artifact_type = 'URL_PATTERN';

CREATE UNIQUE INDEX IF NOT EXISTS crawler_reviews_pending_product_schema_per_source
    ON crawler_reviews (listing_source_id)
    WHERE status = 'PENDING_REVIEW'
      AND artifact_type = 'PRODUCT_SCHEMA';

CREATE TABLE IF NOT EXISTS crawler_review_pages (
    review_page_id UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    review_id      UUID        NOT NULL REFERENCES crawler_reviews(review_id) ON DELETE CASCADE,
    url            TEXT        NOT NULL,
    role           TEXT        NOT NULL CHECK (role IN ('PRIMARY', 'SEED', 'TRIGGERING_REPAIR_PAGE')),
    html_hash      TEXT        NOT NULL,
    fetched        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_crawler_review_pages_review
    ON crawler_review_pages (review_id);

CREATE TABLE IF NOT EXISTS crawler_review_urls (
    review_url_id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    review_id              UUID        NOT NULL REFERENCES crawler_reviews(review_id) ON DELETE CASCADE,
    url                    TEXT        NOT NULL,
    previous_class         TEXT,
    current_pattern_match  BOOLEAN,
    candidate_pattern_match BOOLEAN,
    candidate_class        TEXT,
    created                TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_crawler_review_urls_review
    ON crawler_review_urls (review_id);
