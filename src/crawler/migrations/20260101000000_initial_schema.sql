CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE IF NOT EXISTS listing_availability_mapping (
    raw             TEXT        PRIMARY KEY,
    availability      TEXT,
    mapping_type    TEXT        NOT NULL DEFAULT 'VALUE',
    decision_kind   TEXT GENERATED ALWAYS AS (
        CASE WHEN availability IS NULL THEN 'NO_ASSERTION' ELSE 'AVAILABILITY' END
    ) STORED,
    created         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (mapping_type IN ('VALUE', 'REGEX')),
    CHECK (decision_kind IN ('AVAILABILITY', 'NO_ASSERTION')),
    CHECK (
        availability IS NULL OR availability IN (
            'AVAILABLE', 'IN_STOCK', 'LIMITED_AVAILABILITY', 'BACK_ORDER',
            'MADE_TO_ORDER', 'PRE_ORDER', 'PRE_SALE', 'UNAVAILABLE', 'RESERVED',
            'OUT_OF_STOCK', 'SOLD_OUT'
        )
    ),
    CHECK (
        (decision_kind = 'AVAILABILITY' AND availability IS NOT NULL)
        OR (decision_kind = 'NO_ASSERTION' AND availability IS NULL)
    )
);

-- Partial index so that "WHERE mapping_type = 'REGEX'" scans only regex rows.
CREATE INDEX IF NOT EXISTS idx_listing_availability_mapping_regex
    ON listing_availability_mapping (mapping_type)
    WHERE mapping_type = 'REGEX';

-- ---------------------------------------------------------------------------
-- Seed data: exact-value mappings (matching is done case-insensitively)
-- ---------------------------------------------------------------------------

-- English
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('available',        'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('in stock',         'IN_STOCK', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('add to cart',      'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('add to basket',    'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('buy now',          'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('listed',           NULL,        'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('reserved',         'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('on hold',          'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('sold',             'SOLD_OUT',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('sold out',         'SOLD_OUT',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('out of stock',     'OUT_OF_STOCK', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('unavailable',      'UNAVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;

-- German
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('in den warenkorb', 'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('verfügbar',        'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('auf lager',        'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('gelistet',         NULL,    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('reserviert',       'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('verkauft',         'SOLD_OUT',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('ausverkauft',      'SOLD_OUT',      'VALUE') ON CONFLICT (raw) DO NOTHING;

-- French
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('disponible',       'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('en stock',         'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('listé',            NULL,    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('liste',            NULL,    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('réservé',          'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('reserve',          'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('vendu',            'SOLD_OUT',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('épuisé',           'SOLD_OUT',      'VALUE') ON CONFLICT (raw) DO NOTHING;

-- Spanish
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('listado',          NULL,    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('reservado',        'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('vendido',          'SOLD_OUT',      'VALUE') ON CONFLICT (raw) DO NOTHING;

-- Italian
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('disponibile',      'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('inserito',         NULL,    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('riservato',        'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('venduto',          'SOLD_OUT',      'VALUE') ON CONFLICT (raw) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Seed data: regex-pattern mappings (quantity-style strings)
-- Patterns are matched against trimmed, lower-cased input.
-- ---------------------------------------------------------------------------

-- Available — positive quantity (English)
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('[1-9][0-9]*\s+available\b',                              'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b(only\s+|just\s+)?[1-9][0-9]*\s+remaining\b',          'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b(only\s+)?[1-9][0-9]*\s+left\b',                       'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('[1-9][0-9]*\s+in\s+stock\b',                             'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\bhurry\b.*[1-9][0-9]*',                                 'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Available — positive quantity (German)
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('[1-9][0-9]*\s+vorrätig\b',                               'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+verfügbar\b',       'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+auf\s+lager\b',     'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+stück\b',           'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Available — positive quantity (French)
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('(\bplus\s+que\s+)?[1-9][0-9]*\s+en\s+stock\b',           'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('[1-9][0-9]*\s+disponibles?\b',                           'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\bil\s+(ne\s+)?reste\s+(que\s+)?[1-9][0-9]*\b',           'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Available — positive quantity (Spanish)
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('(\bsolo\s+)?[1-9][0-9]*\s+disponibles?\b',               'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\bquedan\s+[1-9][0-9]*\b',                               'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Available — positive quantity (Italian)
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('(\bsolo\s+)?[1-9][0-9]*\s+disponibili\b',                'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\brimangono\s+[1-9][0-9]*\b',                            'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Sold — zero quantity
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+available\b',      'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+remaining\b',      'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+left\b',           'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+in\s+stock\b',     'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+verfügbar\b',      'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+auf\s+lager\b',    'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+stück\b',          'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+en\s+stock\b',     'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+disponibles?\b',   'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO listing_availability_mapping (raw, availability, mapping_type) VALUES ('\b0\s+disponibili\b',    'OUT_OF_STOCK', 'REGEX') ON CONFLICT (raw) DO NOTHING;

CREATE TABLE IF NOT EXISTS listing_sources (
    listing_source_id   UUID        PRIMARY KEY,
    listing_source_name TEXT        NOT NULL,
    listing_source_slug TEXT        NOT NULL,
    crawl_enabled       BOOLEAN     NOT NULL DEFAULT FALSE,
    llm_calls_count     BIGINT      NOT NULL DEFAULT 0,
    created             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated             TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS listing_source_product_schemas (
    listing_source_id         UUID        PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    product_schema  JSONB       NOT NULL,
    created         TIMESTAMPTZ NOT NULL,
    updated         TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS listing_source_domains (
    domain_id   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    listing_source_id     UUID        NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    listing_source_domain TEXT        NOT NULL,
    url_pattern TEXT,
    last_crawled TIMESTAMPTZ,
    crawl_failure_count INT NOT NULL DEFAULT 0,
    last_crawl_error_kind TEXT,
    next_crawl_at TIMESTAMPTZ,
    UNIQUE (listing_source_domain),
    UNIQUE (listing_source_id, domain_id)
);

CREATE INDEX IF NOT EXISTS idx_listing_source_domains_listing_source_id ON listing_source_domains (listing_source_id);

CREATE TABLE IF NOT EXISTS listing_source_urls (
    listing_source_id           UUID        NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    domain_id         UUID        NOT NULL,
    FOREIGN KEY (listing_source_id, domain_id)
        REFERENCES listing_source_domains (listing_source_id, domain_id)
        ON DELETE CASCADE,
    url               TEXT        NOT NULL,
    url_class         TEXT        NOT NULL,
    last_scraped_presence             TEXT        NOT NULL DEFAULT 'PRESENT',
    last_scraped_availability         TEXT,
    last_scraped_hash                 TEXT,
    last_scraped                      TIMESTAMPTZ,
    last_scraped_price                TEXT,
    last_scraped_price_estimate_min   TEXT,
    last_scraped_price_estimate_max   TEXT,
    last_scraped_url                  TEXT,
    last_scraped_images_hash          TEXT,
    last_scraped_auction_start        TEXT,
    last_scraped_auction_end          TEXT,
    failure_count         INT         NOT NULL DEFAULT 0,
    last_error_kind       TEXT,
    last_error_message    TEXT,
    last_status_code      INT,
    next_retry_at         TIMESTAMPTZ,
    created           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (char_length(url) > 0),
    CHECK (url_class IN ('product', 'category', 'imprint', 'info', 'other')),
    CHECK (last_scraped_presence IN ('PRESENT', 'WITHDRAWN')),
    CHECK (last_scraped_availability IS NULL OR last_scraped_availability IN ('AVAILABLE', 'IN_STOCK', 'LIMITED_AVAILABILITY', 'BACK_ORDER', 'MADE_TO_ORDER', 'PRE_ORDER', 'PRE_SALE', 'UNAVAILABLE', 'RESERVED', 'OUT_OF_STOCK', 'SOLD_OUT')),
    PRIMARY KEY (url)
);

CREATE INDEX IF NOT EXISTS idx_listing_source_urls_class_last_scraped ON listing_source_urls (url_class, last_scraped);
CREATE INDEX IF NOT EXISTS idx_listing_source_urls_domain_id ON listing_source_urls (domain_id);
CREATE INDEX IF NOT EXISTS idx_listing_source_urls_next_retry_at ON listing_source_urls (next_retry_at);
CREATE INDEX IF NOT EXISTS idx_listing_source_domains_next_crawl_at ON listing_source_domains (next_crawl_at);
