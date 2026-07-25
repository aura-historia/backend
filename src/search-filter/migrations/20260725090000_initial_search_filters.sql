CREATE TABLE IF NOT EXISTS search_filters (
    user_search_filter_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    name text NOT NULL,
    notifications boolean NOT NULL DEFAULT true,
    state text NOT NULL,
    search jsonb NOT NULL,
    enhanced_search_description text,
    embedding real[],
    language text NOT NULL,
    currency text NOT NULL,
    last_hybrid_search_matched timestamptz NOT NULL DEFAULT '1970-01-01T00:00:00Z',
    version bigint NOT NULL DEFAULT 1,
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT search_filters_search_object CHECK (jsonb_typeof(search) = 'object')
);

CREATE INDEX IF NOT EXISTS search_filters_user_created_idx ON search_filters (user_id, created DESC);
CREATE INDEX IF NOT EXISTS search_filters_state_updated_idx ON search_filters (state, updated DESC);

CREATE TABLE IF NOT EXISTS search_filter_matches (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    user_search_filter_id uuid NOT NULL REFERENCES search_filters(user_search_filter_id) ON DELETE CASCADE,
    product_id uuid NOT NULL REFERENCES products(product_id) ON DELETE CASCADE,
    shop_id uuid NOT NULL,
    shops_product_id text NOT NULL,
    origin_event_id uuid NOT NULL REFERENCES product_events(event_id),
    user_search_filter_name text,
    enhanced_match_reason text,
    feedback boolean,
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_search_filter_id, product_id),
    CONSTRAINT search_filter_matches_user_filter_shop_product_unique
        UNIQUE (user_id, user_search_filter_id, shop_id, shops_product_id)
);

CREATE INDEX IF NOT EXISTS search_filter_matches_user_created_idx
    ON search_filter_matches (user_id, created DESC);
CREATE INDEX IF NOT EXISTS search_filter_matches_user_filter_created_idx
    ON search_filter_matches (user_id, user_search_filter_id, created DESC);
CREATE INDEX IF NOT EXISTS search_filter_matches_product_id_idx ON search_filter_matches (product_id);
CREATE INDEX IF NOT EXISTS search_filter_matches_origin_event_id_idx ON search_filter_matches (origin_event_id);
