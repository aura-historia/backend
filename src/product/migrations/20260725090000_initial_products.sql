CREATE TABLE IF NOT EXISTS products (
    product_id uuid PRIMARY KEY,
    product_slug_id text NOT NULL,
    shop_slug_id text NOT NULL,
    seller_slug_id text NOT NULL,
    event_id uuid NOT NULL,
    shop_id uuid NOT NULL REFERENCES shops(shop_id),
    seller_id uuid NOT NULL REFERENCES shops(shop_id),
    shops_product_id text NOT NULL,
    shop_name text NOT NULL,
    seller_name text NOT NULL,
    shop_type text NOT NULL,
    structured_address_addressline text,
    structured_address_addressline_extra text,
    structured_address_locality text,
    structured_address_region text,
    structured_address_postal_code text,
    structured_address_country text,
    geo_address_lat double precision,
    geo_address_lon double precision,
    title_native_text text NOT NULL,
    title_native_language text NOT NULL,
    title_de text,
    title_en text,
    title_fr text,
    title_es text,
    title_it text,
    description_native_text text,
    description_native_language text,
    price_native_amount bigint,
    price_native_currency text,
    price_eur bigint,
    price_usd bigint,
    price_gbp bigint,
    price_aud bigint,
    price_cad bigint,
    price_nzd bigint,
    price_cny bigint,
    price_brl bigint,
    price_pln bigint,
    price_try bigint,
    price_jpy bigint,
    price_czk bigint,
    price_rub bigint,
    price_aed bigint,
    price_sar bigint,
    price_hkd bigint,
    price_sgd bigint,
    price_chf bigint,
    price_estimate_min_native_amount bigint,
    price_estimate_min_native_currency text,
    price_estimate_min_eur bigint,
    price_estimate_min_usd bigint,
    price_estimate_min_gbp bigint,
    price_estimate_min_aud bigint,
    price_estimate_min_cad bigint,
    price_estimate_min_nzd bigint,
    price_estimate_min_cny bigint,
    price_estimate_min_brl bigint,
    price_estimate_min_pln bigint,
    price_estimate_min_try bigint,
    price_estimate_min_jpy bigint,
    price_estimate_min_czk bigint,
    price_estimate_min_rub bigint,
    price_estimate_min_aed bigint,
    price_estimate_min_sar bigint,
    price_estimate_min_hkd bigint,
    price_estimate_min_sgd bigint,
    price_estimate_min_chf bigint,
    price_estimate_max_native_amount bigint,
    price_estimate_max_native_currency text,
    price_estimate_max_eur bigint,
    price_estimate_max_usd bigint,
    price_estimate_max_gbp bigint,
    price_estimate_max_aud bigint,
    price_estimate_max_cad bigint,
    price_estimate_max_nzd bigint,
    price_estimate_max_cny bigint,
    price_estimate_max_brl bigint,
    price_estimate_max_pln bigint,
    price_estimate_max_try bigint,
    price_estimate_max_jpy bigint,
    price_estimate_max_czk bigint,
    price_estimate_max_rub bigint,
    price_estimate_max_aed bigint,
    price_estimate_max_sar bigint,
    price_estimate_max_hkd bigint,
    price_estimate_max_sgd bigint,
    price_estimate_max_chf bigint,
    state text NOT NULL,
    lifecycle text NOT NULL,
    url text NOT NULL,
    view_url text NOT NULL,
    product_images jsonb NOT NULL DEFAULT '[]',
    embedding real[],
    auction_start timestamptz,
    auction_end timestamptz,
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT products_shop_product_unique UNIQUE (shop_id, shops_product_id),
    CONSTRAINT products_slug_unique UNIQUE (shop_slug_id, product_slug_id),
    CONSTRAINT products_geo_lat_range CHECK (geo_address_lat IS NULL OR geo_address_lat BETWEEN -90 AND 90),
    CONSTRAINT products_geo_lon_range CHECK (geo_address_lon IS NULL OR geo_address_lon BETWEEN -180 AND 180),
    CONSTRAINT products_images_array CHECK (jsonb_typeof(product_images) = 'array')
);

CREATE TABLE IF NOT EXISTS product_events (
    event_id uuid PRIMARY KEY,
    product_id uuid NOT NULL REFERENCES products(product_id) DEFERRABLE INITIALLY DEFERRED,
    shop_id uuid NOT NULL,
    shops_product_id text NOT NULL,
    event_type text NOT NULL,
    event_group text NOT NULL,
    event_type_schema_version int NOT NULL DEFAULT 1,
    payload jsonb NOT NULL,
    event_time timestamptz NOT NULL,
    created_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_events_shop_product_fkey
        FOREIGN KEY (shop_id, shops_product_id) REFERENCES products(shop_id, shops_product_id)
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT product_events_group_check CHECK (
        event_group IN ('DOMAIN', 'ENRICHMENT', 'POLICY', 'LIFECYCLE')
    ),
    CONSTRAINT product_events_payload_object CHECK (jsonb_typeof(payload) = 'object')
);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'products_event_id_fkey'
    ) THEN
        ALTER TABLE products
            ADD CONSTRAINT products_event_id_fkey
            FOREIGN KEY (event_id) REFERENCES product_events(event_id)
            DEFERRABLE INITIALLY DEFERRED;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS products_seller_id_idx ON products (seller_id);
CREATE INDEX IF NOT EXISTS products_lifecycle_updated_idx ON products (lifecycle, updated DESC);
CREATE INDEX IF NOT EXISTS product_events_product_time_idx ON product_events (product_id, event_time ASC);
CREATE INDEX IF NOT EXISTS product_events_shop_product_time_idx
    ON product_events (shop_id, shops_product_id, event_time ASC);
CREATE INDEX IF NOT EXISTS product_events_type_time_idx ON product_events (event_type, event_time ASC);
