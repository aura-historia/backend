ALTER TABLE users
    DROP CONSTRAINT users_currency_check,
    ADD CONSTRAINT users_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB'));

ALTER TABLE listing_source_web_crawl_ingestion_configurations
    DROP CONSTRAINT listing_source_web_crawl_fallback_currency_check,
    ADD CONSTRAINT listing_source_web_crawl_fallback_currency_check CHECK (fallback_currency IS NULL OR fallback_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB'));

ALTER TABLE listing_source_shopify_ingestion_configurations
    DROP CONSTRAINT listing_source_shopify_currency_check,
    ADD CONSTRAINT listing_source_shopify_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB'));

ALTER TABLE listing_source_woocommerce_ingestion_configurations
    DROP CONSTRAINT listing_source_woocommerce_currency_check,
    ADD CONSTRAINT listing_source_woocommerce_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB'));

ALTER TABLE fx_rate_quotes
    DROP CONSTRAINT fx_rate_quotes_currency_check,
    ADD CONSTRAINT fx_rate_quotes_currency_check CHECK (currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB'));

ALTER TABLE product_listings
    DROP CONSTRAINT product_listings_price_currency_check,
    ADD CONSTRAINT product_listings_price_currency_check CHECK (price_currency IS NULL OR price_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB'));

ALTER TABLE product_listings
    DROP CONSTRAINT product_listings_price_estimate_min_currency_check,
    ADD CONSTRAINT product_listings_price_estimate_min_currency_check CHECK (price_estimate_min_currency IS NULL OR price_estimate_min_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB'));

ALTER TABLE product_listings
    DROP CONSTRAINT product_listings_price_estimate_max_currency_check,
    ADD CONSTRAINT product_listings_price_estimate_max_currency_check CHECK (price_estimate_max_currency IS NULL OR price_estimate_max_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB'));

ALTER TABLE search_filters
    DROP CONSTRAINT search_filters_currency_check,
    ADD CONSTRAINT search_filters_currency_check CHECK (currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB'));
