CREATE TABLE IF NOT EXISTS spider_shop_pattern (
    shop_id   UUID PRIMARY KEY,
    shop_domain TEXT NOT NULL,
    url_pattern TEXT,
    last_crawled TIMESTAMPTZ,
    locked_at  TIMESTAMPTZ,
    created    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE spider_shop_pattern ADD COLUMN IF NOT EXISTS locked_at TIMESTAMPTZ;

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

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_name = 'spider_link' AND column_name = 'link_class'
    )
    AND NOT EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_name = 'spider_link' AND column_name = 'url_class'
    ) THEN
        ALTER TABLE spider_link ADD COLUMN url_class TEXT;
        UPDATE spider_link SET url_class = link_class WHERE url_class IS NULL;
        ALTER TABLE spider_link ALTER COLUMN url_class SET NOT NULL;
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.table_constraints
        WHERE table_name = 'spider_link'
          AND constraint_type = 'CHECK'
          AND constraint_name = 'spider_link_link_class_check'
    ) THEN
        ALTER TABLE spider_link DROP CONSTRAINT spider_link_link_class_check;
    END IF;
END $$;

ALTER TABLE spider_link
    ADD CONSTRAINT spider_link_url_class_check
    CHECK (url_class IN ('product', 'category', 'imprint', 'info', 'other'));

CREATE INDEX IF NOT EXISTS spider_url_class_last_scraped_idx ON spider_link (url_class, last_scraped);

