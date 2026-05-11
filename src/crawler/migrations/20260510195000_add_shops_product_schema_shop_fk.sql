DELETE FROM shops_product_schema
WHERE shop_id NOT IN (
    SELECT shop_id
    FROM shops
);

ALTER TABLE shops_product_schema
DROP CONSTRAINT IF EXISTS shops_product_schema_shop_id_fkey;

ALTER TABLE shops_product_schema
ADD CONSTRAINT shops_product_schema_shop_id_fkey
FOREIGN KEY (shop_id) REFERENCES shops(shop_id) ON DELETE CASCADE;
