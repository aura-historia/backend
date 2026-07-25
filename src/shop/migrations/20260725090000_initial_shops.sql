CREATE TABLE IF NOT EXISTS shops (
    shop_id uuid PRIMARY KEY,
    shop_slug_id text NOT NULL UNIQUE,
    name text NOT NULL,
    shop_type text NOT NULL,
    partner_status text NOT NULL,
    shop_domains text[] NOT NULL DEFAULT '{}',
    shopify_domain text UNIQUE,
    shopify_currency text,
    shopify_language text,
    woocommerce_webhook_secret text,
    woocommerce_currency text,
    woocommerce_language text,
    url text,
    view_url text,
    image text,
    structured_address_addressline text,
    structured_address_addressline_extra text,
    structured_address_locality text,
    structured_address_region text,
    structured_address_postal_code text,
    structured_address_country text,
    geo_address_lat double precision,
    geo_address_lon double precision,
    phone text,
    email text,
    affiliate_configuration jsonb,
    version bigint NOT NULL DEFAULT 1,
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT shops_geo_lat_range CHECK (geo_address_lat IS NULL OR geo_address_lat BETWEEN -90 AND 90),
    CONSTRAINT shops_geo_lon_range CHECK (geo_address_lon IS NULL OR geo_address_lon BETWEEN -180 AND 180),
    CONSTRAINT shops_affiliate_configuration_object CHECK (
        affiliate_configuration IS NULL OR jsonb_typeof(affiliate_configuration) = 'object'
    )
);

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.tables
        WHERE table_schema = 'public' AND table_name = 'user_partner_shops'
    ) AND NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'user_partner_shops_shop_id_fkey'
    ) THEN
        ALTER TABLE user_partner_shops
            ADD CONSTRAINT user_partner_shops_shop_id_fkey
            FOREIGN KEY (shop_id) REFERENCES shops(shop_id) ON DELETE CASCADE;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS shops_shop_domains_gin_idx ON shops USING gin (shop_domains);
CREATE INDEX IF NOT EXISTS shops_partner_status_updated_idx ON shops (partner_status, updated DESC);
