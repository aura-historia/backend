CREATE TABLE IF NOT EXISTS shops_removed_page_schema (
    shop_id UUID PRIMARY KEY REFERENCES shops(shop_id) ON DELETE CASCADE,
    removed_page_schema JSONB NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
