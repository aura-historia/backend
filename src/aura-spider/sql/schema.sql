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
