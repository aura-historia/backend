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
    -- A crawl that stops classifying at a pending URL-pattern review is neither a success nor a
    -- failure, and a domain whose deterministic failures keep repeating stops retrying until an
    -- operator acts. Both are recorded as their own crawl outcome.
    CONSTRAINT listing_source_domains_last_crawl_outcome_check
        CHECK (last_crawl_outcome IN (
            'COMPLETED',
            'PAGE_BUDGET_REACHED',
            'HARD_TIMEOUT_TRUNCATED',
            'FAILED',
            'AWAITING_URL_PATTERN_REVIEW',
            'NEEDS_OPERATOR'
        )),
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
    -- The pending product-schema review this page waits on and how many consecutive scrapes
    -- waited on it, so its retry delay grows while that review stays pending. Meaningful only
    -- while `last_error_kind` is `PendingSchemaReview`.
    schema_review_waited_on   UUID,
    schema_review_wait_streak INT NOT NULL DEFAULT 0,
    CHECK (char_length(url) > 0),
    CHECK (url_class IN ('product', 'category', 'imprint', 'info', 'other')),
    CHECK (crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')),
    CHECK (last_captured_raw_input_sha256 IS NULL OR octet_length(last_captured_raw_input_sha256) = 32),
    CONSTRAINT listing_source_urls_raw_source_record_key_nonempty
        CHECK (char_length(raw_source_record_key) > 0),
    CONSTRAINT listing_source_urls_last_source_listing_id_nonempty
        CHECK (last_source_listing_id IS NULL OR char_length(last_source_listing_id) > 0),
    CONSTRAINT listing_source_urls_schema_review_wait_shape_check CHECK (
        (schema_review_waited_on IS NULL AND schema_review_wait_streak = 0)
        OR (schema_review_waited_on IS NOT NULL AND schema_review_wait_streak > 0)
    ),
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
    -- A needs-repair verdict keeps the review pending and records when and why repair was
    -- requested. No verdict writes the legacy terminal `NEEDS_REPAIR` status any more.
    repair_requested   TIMESTAMPTZ,
    repair_note        TEXT,
    -- The stored removed-page templates a product-schema candidate started from, so approval
    -- keeps a template another writer stored since then instead of dropping it.
    removed_page_schemas_base JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- A URL-pattern review belongs to one domain of its own ListingSource.
    CONSTRAINT crawler_reviews_listing_source_domain_fkey
        FOREIGN KEY (listing_source_id, domain_id)
        REFERENCES listing_source_domains (listing_source_id, domain_id)
        ON DELETE CASCADE,
    CONSTRAINT crawler_reviews_artifact_domain_shape_check CHECK (
        (artifact_type = 'URL_PATTERN' AND domain_id IS NOT NULL)
        OR (artifact_type = 'PRODUCT_SCHEMA' AND domain_id IS NULL)
    ),
    CONSTRAINT crawler_reviews_repair_request_shape_check CHECK (
        (repair_requested IS NULL AND repair_note IS NULL)
        OR (repair_requested IS NOT NULL AND repair_note IS NOT NULL AND btrim(repair_note) <> '')
    ),
    CONSTRAINT crawler_reviews_removed_page_schemas_base_shape_check
        CHECK (jsonb_typeof(removed_page_schemas_base) = 'array')
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
        role IN (
            'PRIMARY',
            'SEED',
            'TRIGGERING_REPAIR_PAGE',
            'TRIGGERING_GENERATION_PAGE',
            'REVIEWER_ADDED',
            'WAITING_PAGE'
        )
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
        NEW.validation_summary = NEW.validation_summary - 'schema_matrix' - 'schema_matrix_summary';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER crawler_reviews_candidate_version
    BEFORE UPDATE OF candidate_payload ON crawler_reviews
    FOR EACH ROW
    EXECUTE FUNCTION increment_crawler_review_candidate_version();

-- The append-only trail of who changed each review and how. Every accepted review-console
-- mutation appends one row in its own transaction, and a refused mutation none. An event is
-- never updated, and deleted only with its review.
CREATE TABLE crawler_review_events (
    event_id          UUID        PRIMARY KEY,
    review_id         UUID        NOT NULL REFERENCES crawler_reviews(review_id) ON DELETE CASCADE,
    actor             TEXT        NOT NULL CHECK (
        actor IN ('HUMAN', 'AGENT_AUTHOR', 'AGENT_REVIEWER', 'SYSTEM')
    ),
    action            TEXT        NOT NULL CHECK (
        action IN (
            'CANDIDATE_EDITED',
            'SCHEMA_FIELD_EDITED',
            'PAGE_ADDED',
            'PAGE_REMOVED',
            'REPAIR_REQUESTED',
            'APPROVED',
            'REJECTED',
            'RESTORED'
        )
    ),
    -- The review's candidate version after the mutation.
    candidate_version BIGINT      NOT NULL,
    notes             TEXT,
    details           JSONB       NOT NULL CHECK (jsonb_typeof(details) = 'object'),
    -- The time of the write, not of the transaction's start: a review's mutations append under
    -- its row lock, so this orders a review's events as they committed.
    created           TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

CREATE INDEX idx_crawler_review_events_review
    ON crawler_review_events (review_id, created);

-- The review agents' approvals and rejections of the last hour, which their hourly cap counts.
CREATE INDEX idx_crawler_review_events_agent_decisions
    ON crawler_review_events (created)
    WHERE actor IN ('AGENT_AUTHOR', 'AGENT_REVIEWER')
      AND action IN ('APPROVED', 'REJECTED');

-- Refuses every update and every delete except the cascade of the review's own deletion, which
-- runs from the foreign key's trigger and so one trigger level deeper than a direct statement.
CREATE FUNCTION refuse_crawler_review_event_change()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' AND pg_trigger_depth() > 1 THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'crawler_review_events is append-only';
END;
$$;

CREATE TRIGGER crawler_review_events_append_only
    BEFORE UPDATE OR DELETE ON crawler_review_events
    FOR EACH ROW
    EXECUTE FUNCTION refuse_crawler_review_event_change();
