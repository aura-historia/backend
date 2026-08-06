CREATE INDEX IF NOT EXISTS search_filter_matches_filter_created_product_idx
    ON search_filter_matches (user_search_filter_id, created ASC, product_id ASC);
