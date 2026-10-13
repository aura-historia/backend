-- Keyset order for the cross-source admin directory, including retained withdrawals.
CREATE INDEX product_listings_admin_updated_id_idx ON product_listings (updated DESC, product_listing_id DESC);
CREATE INDEX product_listings_admin_created_id_idx ON product_listings (created DESC, product_listing_id DESC);
CREATE INDEX product_listings_admin_source_updated_id_idx ON product_listings (listing_source_id, updated DESC, product_listing_id DESC);
