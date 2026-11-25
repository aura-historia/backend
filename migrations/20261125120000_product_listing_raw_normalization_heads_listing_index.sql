-- Several raw streams, such as URL variants of one product page, can bind one listing. Finding
-- the other streams bound to a listing lets reapplication skip a stream that a newer one
-- superseded (docs/normalization-replay.md#production-reapplication).
CREATE INDEX product_listing_raw_normalization_heads_listing_idx
    ON product_listing_raw_normalization_heads (product_listing_id)
    WHERE product_listing_id IS NOT NULL;
