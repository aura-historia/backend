-- An asking price may be a range (`€ 1.000 – 1.500`): kind RANGE keeps its lower bound in
-- price_amount and its upper bound in price_max_amount, in the one price_currency. Existing
-- MONETARY and ON_REQUEST rows keep their meaning; only a range has an upper bound.

ALTER TABLE product_listings
    ADD COLUMN price_max_amount bigint;

ALTER TABLE product_listings
    DROP CONSTRAINT product_listings_price_shape_check,
    DROP CONSTRAINT product_listings_price_kind_check,
    ADD CONSTRAINT product_listings_price_shape_check CHECK (
        (price_kind IS NULL AND price_amount IS NULL AND price_max_amount IS NULL AND price_currency IS NULL)
        OR (price_kind = 'MONETARY' AND price_amount IS NOT NULL AND price_max_amount IS NULL AND price_currency IS NOT NULL)
        OR (price_kind = 'RANGE' AND price_amount IS NOT NULL AND price_max_amount IS NOT NULL AND price_currency IS NOT NULL AND price_amount < price_max_amount)
        OR (price_kind = 'ON_REQUEST' AND price_amount IS NULL AND price_max_amount IS NULL AND price_currency IS NULL)
    ),
    ADD CONSTRAINT product_listings_price_kind_check CHECK (price_kind IS NULL OR price_kind IN ('MONETARY', 'RANGE', 'ON_REQUEST'));
