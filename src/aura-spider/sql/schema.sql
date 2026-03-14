CREATE TABLE IF NOT EXISTS spider_shop_pattern (
    shop_url   TEXT PRIMARY KEY,
    pattern    TEXT,
    created    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);


