CREATE TABLE IF NOT EXISTS product_watchlist (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    product_id uuid NOT NULL REFERENCES products(product_id) ON DELETE CASCADE,
    shop_id uuid NOT NULL,
    shops_product_id text NOT NULL,
    notifications boolean NOT NULL DEFAULT true,
    state text NOT NULL,
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, product_id),
    CONSTRAINT product_watchlist_user_shop_product_unique UNIQUE (user_id, shop_id, shops_product_id)
);

CREATE INDEX IF NOT EXISTS product_watchlist_user_created_idx ON product_watchlist (user_id, created DESC);
CREATE INDEX IF NOT EXISTS product_watchlist_product_id_idx ON product_watchlist (product_id);
