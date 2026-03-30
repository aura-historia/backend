CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE IF NOT EXISTS shops_product_schema (
    shop_id         UUID        PRIMARY KEY,
    product_schema  JSONB       NOT NULL,
    created         TIMESTAMPTZ NOT NULL,
    updated         TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS product_state_mapping (
    raw             TEXT        PRIMARY KEY,
    normalized      TEXT        NOT NULL,
    mapping_type    TEXT        NOT NULL DEFAULT 'VALUE',
    created         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated         TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Partial index so that "WHERE mapping_type = 'REGEX'" scans only regex rows.
CREATE INDEX IF NOT EXISTS idx_product_state_mapping_regex
    ON product_state_mapping (mapping_type)
    WHERE mapping_type = 'REGEX';

-- ---------------------------------------------------------------------------
-- Seed data: exact-value mappings (matching is done case-insensitively)
-- ---------------------------------------------------------------------------

-- English
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('available',        'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('in stock',         'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('add to cart',      'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('add to basket',    'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('buy now',          'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('listed',           'LISTED',    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('reserved',         'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('on hold',          'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('sold',             'SOLD',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('sold out',         'SOLD',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('out of stock',     'SOLD',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('removed',          'REMOVED',   'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('deleted',          'REMOVED',   'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('unavailable',      'REMOVED',   'VALUE') ON CONFLICT (raw) DO NOTHING;

-- German
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('in den warenkorb', 'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('verfügbar',        'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('auf lager',        'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('gelistet',         'LISTED',    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('reserviert',       'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('verkauft',         'SOLD',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('ausverkauft',      'SOLD',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('gelöscht',         'REMOVED',   'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('entfernt',         'REMOVED',   'VALUE') ON CONFLICT (raw) DO NOTHING;

-- French
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('disponible',       'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('en stock',         'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('listé',            'LISTED',    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('liste',            'LISTED',    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('réservé',          'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('reserve',          'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('vendu',            'SOLD',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('épuisé',           'SOLD',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('supprimé',         'REMOVED',   'VALUE') ON CONFLICT (raw) DO NOTHING;

-- Spanish
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('listado',          'LISTED',    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('reservado',        'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('vendido',          'SOLD',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('eliminado',        'REMOVED',   'VALUE') ON CONFLICT (raw) DO NOTHING;

-- Italian
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('disponibile',      'AVAILABLE', 'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('inserito',         'LISTED',    'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('riservato',        'RESERVED',  'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('venduto',          'SOLD',      'VALUE') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('rimosso',          'REMOVED',   'VALUE') ON CONFLICT (raw) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Seed data: regex-pattern mappings (quantity-style strings)
-- Patterns are matched against trimmed, lower-cased input.
-- ---------------------------------------------------------------------------

-- Available — positive quantity (English)
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('[1-9][0-9]*\s+available\b',                              'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b(only\s+|just\s+)?[1-9][0-9]*\s+remaining\b',          'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b(only\s+)?[1-9][0-9]*\s+left\b',                       'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('[1-9][0-9]*\s+in\s+stock\b',                             'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\bhurry\b.*[1-9][0-9]*',                                 'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Available — positive quantity (German)
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('[1-9][0-9]*\s+vorrätig\b',                               'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+verfügbar\b',       'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+auf\s+lager\b',     'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+stück\b',           'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Available — positive quantity (French)
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('(\bplus\s+que\s+)?[1-9][0-9]*\s+en\s+stock\b',           'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('[1-9][0-9]*\s+disponibles?\b',                           'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\bil\s+(ne\s+)?reste\s+(que\s+)?[1-9][0-9]*\b',           'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Available — positive quantity (Spanish)
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('(\bsolo\s+)?[1-9][0-9]*\s+disponibles?\b',               'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\bquedan\s+[1-9][0-9]*\b',                               'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Available — positive quantity (Italian)
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('(\bsolo\s+)?[1-9][0-9]*\s+disponibili\b',                'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\brimangono\s+[1-9][0-9]*\b',                            'AVAILABLE', 'REGEX') ON CONFLICT (raw) DO NOTHING;

-- Sold — zero quantity
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+available\b',      'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+remaining\b',      'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+left\b',           'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+in\s+stock\b',     'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+verfügbar\b',      'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+auf\s+lager\b',    'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+stück\b',          'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+en\s+stock\b',     'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+disponibles?\b',   'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;
INSERT INTO product_state_mapping (raw, normalized, mapping_type) VALUES ('\b0\s+disponibili\b',    'SOLD', 'REGEX') ON CONFLICT (raw) DO NOTHING;

CREATE TABLE IF NOT EXISTS spider_shop_pattern (
    shop_id   UUID PRIMARY KEY,
    shop_domain TEXT NOT NULL,
    url_pattern TEXT,
    last_crawled TIMESTAMPTZ,
    locked_at  TIMESTAMPTZ,
    created    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS spider_link (
    shop_id    UUID NOT NULL REFERENCES spider_shop_pattern(shop_id) ON DELETE CASCADE,
    url        TEXT NOT NULL,
    url_class  TEXT NOT NULL,
    main_hash  TEXT NOT NULL,
    state      TEXT NOT NULL DEFAULT 'UNKNOWN',
    price_currency TEXT,
    price_value    INT,
    last_scraped_hash TEXT,
    last_scraped TIMESTAMPTZ,
    created    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (char_length(url) > 0),
    CHECK (char_length(main_hash) = 64),
    CHECK (url_class IN ('product', 'category', 'imprint', 'info', 'other')),
    CHECK (state IN ('LISTED', 'AVAILABLE', 'RESERVED', 'SOLD', 'REMOVED', 'UNKNOWN')),
    PRIMARY KEY (url)
);

CREATE INDEX IF NOT EXISTS spider_url_class_last_scraped_idx ON spider_link (url_class, last_scraped);
