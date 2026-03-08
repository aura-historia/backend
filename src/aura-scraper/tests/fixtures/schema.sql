CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE IF NOT EXISTS shops_product_schema (
    shop_id         UUID        PRIMARY KEY,
    product_schema  JSONB       NOT NULL,
    created         TIMESTAMPTZ NOT NULL,
    updated         TIMESTAMPTZ NOT NULL
);
