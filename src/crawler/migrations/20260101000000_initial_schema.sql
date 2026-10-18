CREATE EXTENSION IF NOT EXISTS "pgcrypto";


CREATE TABLE listing_sources (
    listing_source_id   UUID        PRIMARY KEY,
    listing_source_name TEXT        NOT NULL,
    listing_source_slug TEXT        NOT NULL,
    crawl_enabled       BOOLEAN     NOT NULL DEFAULT FALSE,
    fallback_currency   TEXT,
    llm_calls_count     BIGINT      NOT NULL DEFAULT 0,
    created             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT listing_sources_fallback_currency_check CHECK (
        fallback_currency IS NULL OR fallback_currency IN (
            'EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY',
            'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK',
            'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS'
        )
    )
);

CREATE TABLE listing_source_product_schemas (
    listing_source_id         UUID        PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    product_schema  JSONB       NOT NULL,
    created         TIMESTAMPTZ NOT NULL,
    updated         TIMESTAMPTZ NOT NULL
);

CREATE TABLE listing_source_removed_page_schemas (
    listing_source_id UUID PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    removed_page_schema JSONB NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE listing_source_domains (
    domain_id   UUID        PRIMARY KEY,
    listing_source_id     UUID        NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    listing_source_domain TEXT        NOT NULL,
    crawl_root_host       TEXT        NOT NULL,
    url_pattern TEXT,
    last_crawled TIMESTAMPTZ,
    crawl_failure_count INT NOT NULL DEFAULT 0,
    last_crawl_error_kind TEXT,
    next_crawl_at TIMESTAMPTZ,
    last_crawl_outcome TEXT,
    crawl_truncation_count INT NOT NULL DEFAULT 0,
    url_pattern_state TEXT NOT NULL DEFAULT 'UNKNOWN',
    scrape_failure_streak INT NOT NULL DEFAULT 0,
    last_scrape_error_kind TEXT,
    last_scrape_status_code INT,
    next_scrape_at TIMESTAMPTZ,
    UNIQUE (listing_source_domain),
    CHECK (listing_source_domain = lower(listing_source_domain)),
    CHECK (listing_source_domain = rtrim(listing_source_domain, '.')),
    CHECK (listing_source_domain !~ '^www[.]'),
    CHECK (crawl_root_host = lower(crawl_root_host)),
    CHECK (crawl_root_host = rtrim(crawl_root_host, '.')),
    CHECK (regexp_replace(crawl_root_host, '^www[.]', '') = listing_source_domain),
    UNIQUE (listing_source_id, domain_id),
    CONSTRAINT listing_source_domains_url_pattern_state_check
        CHECK (url_pattern_state IN ('UNKNOWN', 'MATCHED', 'NO_PATTERN')),
    CONSTRAINT listing_source_domains_url_pattern_state_shape_check CHECK (
        (url_pattern_state = 'MATCHED' AND url_pattern IS NOT NULL)
        OR (url_pattern_state IN ('UNKNOWN', 'NO_PATTERN') AND url_pattern IS NULL)
    ),
    CONSTRAINT listing_source_domains_scrape_failure_streak_check
        CHECK (scrape_failure_streak >= 0),
    CONSTRAINT listing_source_domains_last_crawl_outcome_check
        CHECK (last_crawl_outcome IN ('COMPLETED', 'PAGE_BUDGET_REACHED', 'HARD_TIMEOUT_TRUNCATED', 'FAILED')),
    CONSTRAINT listing_source_domains_crawl_truncation_count_check
        CHECK (crawl_truncation_count >= 0)
);

CREATE INDEX idx_listing_source_domains_listing_source_id ON listing_source_domains (listing_source_id);
CREATE INDEX idx_listing_source_domains_next_crawl_at ON listing_source_domains (next_crawl_at);
CREATE INDEX idx_listing_source_domains_next_scrape_at
    ON listing_source_domains (next_scrape_at)
    WHERE next_scrape_at IS NOT NULL;


CREATE TABLE listing_source_urls (
    listing_source_id           UUID        NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    domain_id         UUID        NOT NULL,
    FOREIGN KEY (listing_source_id, domain_id)
        REFERENCES listing_source_domains (listing_source_id, domain_id)
        ON DELETE CASCADE,
    url               TEXT        NOT NULL,
    url_class         TEXT        NOT NULL,
    crawler_disposition               TEXT        NOT NULL DEFAULT 'ACTIVE',
    last_scraped_hash                 TEXT,
    last_scraped_schema_fingerprint   TEXT,
    last_captured_raw_input_sha256    BYTEA,
    last_scraped                      TIMESTAMPTZ,
    failure_count         INT         NOT NULL DEFAULT 0,
    last_error_kind       TEXT,
    last_error_message    TEXT,
    last_status_code      INT,
    next_retry_at         TIMESTAMPTZ,
    created           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Stable raw-capture identity; it survives URL moves of the same listing.
    raw_source_record_key TEXT NOT NULL,
    last_source_listing_id TEXT,
    CHECK (char_length(url) > 0),
    CHECK (url_class IN ('product', 'category', 'imprint', 'info', 'other')),
    CHECK (crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')),
    CHECK (last_captured_raw_input_sha256 IS NULL OR octet_length(last_captured_raw_input_sha256) = 32),
    CONSTRAINT listing_source_urls_raw_source_record_key_nonempty
        CHECK (char_length(raw_source_record_key) > 0),
    CONSTRAINT listing_source_urls_last_source_listing_id_nonempty
        CHECK (last_source_listing_id IS NULL OR char_length(last_source_listing_id) > 0),
    PRIMARY KEY (url)
);

CREATE INDEX idx_listing_source_urls_class_last_scraped ON listing_source_urls (url_class, last_scraped);
CREATE INDEX idx_listing_source_urls_domain_id ON listing_source_urls (domain_id);
CREATE INDEX idx_listing_source_urls_next_retry_at ON listing_source_urls (next_retry_at);


CREATE TABLE crawler_reviews (
    review_id          UUID        PRIMARY KEY,
    listing_source_id            UUID        NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    domain_id          UUID,
    artifact_type      TEXT        NOT NULL CHECK (artifact_type IN ('URL_PATTERN', 'PRODUCT_SCHEMA')),
    CHECK (artifact_type <> 'URL_PATTERN' OR domain_id IS NOT NULL),
    status             TEXT        NOT NULL CHECK (status IN ('PENDING_REVIEW', 'APPROVED', 'REJECTED', 'NEEDS_REPAIR', 'SUPERSEDED')),
    reason             TEXT        NOT NULL,
    candidate_payload  JSONB       NOT NULL,
    validation_summary JSONB       NOT NULL DEFAULT '{}'::jsonb,
    candidate_version  BIGINT      NOT NULL DEFAULT 1,
    reviewer_notes     TEXT,
    created            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    reviewed           TIMESTAMPTZ,
    -- A URL-pattern review belongs to one domain of its own ListingSource.
    CONSTRAINT crawler_reviews_listing_source_domain_fkey
        FOREIGN KEY (listing_source_id, domain_id)
        REFERENCES listing_source_domains (listing_source_id, domain_id)
        ON DELETE CASCADE,
    CONSTRAINT crawler_reviews_artifact_domain_shape_check CHECK (
        (artifact_type = 'URL_PATTERN' AND domain_id IS NOT NULL)
        OR (artifact_type = 'PRODUCT_SCHEMA' AND domain_id IS NULL)
    )
);

CREATE INDEX idx_crawler_reviews_status
    ON crawler_reviews (status, artifact_type, created);

CREATE UNIQUE INDEX crawler_reviews_pending_url_pattern_per_domain
    ON crawler_reviews (domain_id)
    WHERE status = 'PENDING_REVIEW'
      AND artifact_type = 'URL_PATTERN';

CREATE UNIQUE INDEX crawler_reviews_pending_product_schema_per_source
    ON crawler_reviews (listing_source_id)
    WHERE status = 'PENDING_REVIEW'
      AND artifact_type = 'PRODUCT_SCHEMA';

CREATE TABLE crawler_review_pages (
    review_page_id UUID        PRIMARY KEY,
    review_id      UUID        NOT NULL REFERENCES crawler_reviews(review_id) ON DELETE CASCADE,
    url            TEXT        NOT NULL,
    role           TEXT        NOT NULL CHECK (
        role IN ('PRIMARY', 'SEED', 'TRIGGERING_REPAIR_PAGE', 'TRIGGERING_GENERATION_PAGE')
    ),
    html_hash      TEXT        NOT NULL,
    fetched        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_crawler_review_pages_review
    ON crawler_review_pages (review_id);

CREATE TABLE crawler_review_urls (
    review_url_id          UUID        PRIMARY KEY,
    review_id              UUID        NOT NULL REFERENCES crawler_reviews(review_id) ON DELETE CASCADE,
    url                    TEXT        NOT NULL,
    previous_class         TEXT,
    current_pattern_match  BOOLEAN,
    candidate_pattern_match BOOLEAN,
    candidate_class        TEXT,
    created                TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_crawler_review_urls_review
    ON crawler_review_urls (review_id);

CREATE FUNCTION increment_crawler_review_candidate_version()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.candidate_payload IS DISTINCT FROM OLD.candidate_payload THEN
        NEW.candidate_version = OLD.candidate_version + 1;
        NEW.validation_summary = NEW.validation_summary - 'schema_matrix';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER crawler_reviews_candidate_version
    BEFORE UPDATE OF candidate_payload ON crawler_reviews
    FOR EACH ROW
    EXECUTE FUNCTION increment_crawler_review_candidate_version();
