CREATE TABLE IF NOT EXISTS spider_shop_pattern (
    shop_url   TEXT PRIMARY KEY,
    url_pattern TEXT,
    last_crawled TIMESTAMPTZ,
    created    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS spider_link (
    shop_url   TEXT NOT NULL,
    url        TEXT NOT NULL,
    link_class TEXT NOT NULL,
    main_hash  TEXT NOT NULL,
    last_scraped TIMESTAMPTZ,
    created    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (char_length(shop_url) > 0),
    CHECK (char_length(url) > 0),
    CHECK (char_length(main_hash) = 64),
    CHECK (link_class IN ('product', 'category', 'imprint', 'info', 'other')),
    PRIMARY KEY (shop_url, url)
);

CREATE INDEX IF NOT EXISTS spider_link_class_idx ON spider_link (link_class);
