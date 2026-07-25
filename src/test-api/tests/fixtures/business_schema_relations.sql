INSERT INTO users (user_id, email, tier, role, created_by, updated_by)
VALUES (
    '10000000-0000-0000-0000-000000000001',
    'user@example.com',
    'FREE',
    'USER',
    'system',
    'system'
);

INSERT INTO shops (
    shop_id,
    shop_slug_id,
    name,
    shop_type,
    partner_status,
    shop_domains,
    created_by,
    updated_by
)
VALUES (
    '20000000-0000-0000-0000-000000000001',
    'shop-one',
    'Shop One',
    'AUCTION_HOUSE',
    'PARTNERED',
    ARRAY['shop.example.com'],
    'system',
    'system'
);

INSERT INTO user_partner_shops (user_id, shop_id)
VALUES (
    '10000000-0000-0000-0000-000000000001',
    '20000000-0000-0000-0000-000000000001'
);

INSERT INTO partner_shop_applications (
    partner_shop_application_id,
    applicant_user_id,
    business_state,
    execution_state,
    payload_type,
    existing_shop_id,
    created_by,
    updated_by
)
VALUES (
    '60000000-0000-0000-0000-000000000001',
    '10000000-0000-0000-0000-000000000001',
    'ACCEPTED',
    'SUCCEEDED',
    'EXISTING_SHOP',
    '20000000-0000-0000-0000-000000000001',
    'system',
    'system'
);

BEGIN;

INSERT INTO products (
    product_id,
    product_slug_id,
    shop_slug_id,
    seller_slug_id,
    event_id,
    shop_id,
    seller_id,
    shops_product_id,
    shop_name,
    seller_name,
    shop_type,
    title_native_text,
    title_native_language,
    state,
    lifecycle,
    url,
    view_url,
    product_images,
    created_by,
    updated_by
)
VALUES (
    '30000000-0000-0000-0000-000000000001',
    'product-one',
    'shop-one',
    'shop-one',
    '40000000-0000-0000-0000-000000000001',
    '20000000-0000-0000-0000-000000000001',
    '20000000-0000-0000-0000-000000000001',
    'external-1',
    'Shop One',
    'Shop One',
    'AUCTION_HOUSE',
    'A vase',
    'en',
    'ACTIVE',
    'LISTED',
    'https://shop.example.com/products/external-1',
    'https://aura.example.com/shops/shop-one/products/product-one',
    '[{"position": 0, "url": "https://cdn.example.com/image.jpg"}]',
    'system',
    'system'
);

INSERT INTO product_events (
    event_id,
    product_id,
    shop_id,
    shops_product_id,
    event_type,
    event_group,
    payload,
    event_time,
    created_by
)
VALUES (
    '40000000-0000-0000-0000-000000000001',
    '30000000-0000-0000-0000-000000000001',
    '20000000-0000-0000-0000-000000000001',
    'external-1',
    'PRODUCT_CREATED',
    'DOMAIN',
    '{"kind": "created"}',
    now(),
    'system'
);

COMMIT;

INSERT INTO product_watchlist (
    user_id,
    product_id,
    shop_id,
    shops_product_id,
    state,
    created_by,
    updated_by
)
VALUES (
    '10000000-0000-0000-0000-000000000001',
    '30000000-0000-0000-0000-000000000001',
    '20000000-0000-0000-0000-000000000001',
    'external-1',
    'ACTIVE',
    'system',
    'system'
);

INSERT INTO search_filters (
    user_search_filter_id,
    user_id,
    name,
    state,
    search,
    language,
    currency,
    created_by,
    updated_by
)
VALUES (
    '50000000-0000-0000-0000-000000000001',
    '10000000-0000-0000-0000-000000000001',
    'Vases',
    'ACTIVE',
    '{"product_query": ["vase"]}',
    'en',
    'EUR',
    'system',
    'system'
);

INSERT INTO search_filter_matches (
    user_id,
    user_search_filter_id,
    product_id,
    shop_id,
    shops_product_id,
    origin_event_id,
    created_by,
    updated_by
)
VALUES (
    '10000000-0000-0000-0000-000000000001',
    '50000000-0000-0000-0000-000000000001',
    '30000000-0000-0000-0000-000000000001',
    '20000000-0000-0000-0000-000000000001',
    'external-1',
    '40000000-0000-0000-0000-000000000001',
    'system',
    'system'
);
