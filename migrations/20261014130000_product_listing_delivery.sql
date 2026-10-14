-- Delivery is part of the listing revision/transaction, never an independently mutable row.
ALTER TABLE product_listings ADD COLUMN delivery_configuration jsonb;
ALTER TABLE product_listings ADD CONSTRAINT product_listings_delivery_object
    CHECK (delivery_configuration IS NULL OR jsonb_typeof(delivery_configuration) = 'object');
CREATE INDEX product_listings_delivery_profile_idx
    ON product_listings ((delivery_configuration #>> '{selection,profile_id}'))
    WHERE delivery_configuration #>> '{selection,kind}' = 'EXPLICIT';
CREATE INDEX product_listings_delivery_source_idx ON product_listings (listing_source_id)
    WHERE delivery_configuration IS NOT NULL;
-- Profiles created through source management are source-authored. Provider bindings are explicit;
-- equal codes in another integration never establish membership. Acquisition owns future binding writes.
ALTER TABLE shipping_profiles ADD COLUMN integration_method text;
ALTER TABLE shipping_profiles ADD COLUMN integration_domain text;
ALTER TABLE shipping_profiles ADD CONSTRAINT shipping_profiles_integration_pair CHECK (
    (integration_method IS NULL AND integration_domain IS NULL) OR
    (integration_method IS NOT NULL AND integration_method IN ('SHOPIFY','WOOCOMMERCE')
        AND integration_domain IS NOT NULL AND octet_length(integration_domain) BETWEEN 1 AND 253)
);
ALTER TABLE product_listing_command_receipts DROP CONSTRAINT product_listing_command_receipts_operation_check;
ALTER TABLE product_listing_command_receipts ADD CONSTRAINT product_listing_command_receipts_operation_check
    CHECK (operation IN ('CREATE','UPDATE','UPSERT','WITHDRAW','CAPTURE_RAW','CHANGE_DELIVERY'));

-- Explicit provider identity; business presentation URLs do not identify integrations.
-- Authenticated provider acquisition/configuration owns assigning this value.
ALTER TABLE listing_source_woocommerce_ingestion_configurations
    ADD COLUMN shipping_integration_domain text
    CHECK (shipping_integration_domain IS NULL OR octet_length(shipping_integration_domain) BETWEEN 1 AND 253);
