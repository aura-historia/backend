-- Korean, Greek, Hebrew, Thai and Hindi are source-only catalogue languages like zh, pt, pl, tr,
-- nl, cs, ja, ru and ar: they can be detected, stored and searched in the original language.

ALTER TABLE users
    DROP CONSTRAINT users_language_check,
    ADD CONSTRAINT users_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi'));

ALTER TABLE listing_source_shopify_ingestion_configurations
    DROP CONSTRAINT listing_source_shopify_language_check,
    ADD CONSTRAINT listing_source_shopify_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi'));

ALTER TABLE listing_source_woocommerce_ingestion_configurations
    DROP CONSTRAINT listing_source_woocommerce_language_check,
    ADD CONSTRAINT listing_source_woocommerce_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi'));

ALTER TABLE product_listings
    DROP CONSTRAINT product_listings_title_language_check,
    DROP CONSTRAINT product_listings_description_language_check,
    ADD CONSTRAINT product_listings_title_language_check CHECK (title_language IS NULL OR title_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi')),
    ADD CONSTRAINT product_listings_description_language_check CHECK (description_language IS NULL OR description_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi'));

ALTER TABLE product_listing_translations
    DROP CONSTRAINT product_listing_translations_language_check,
    ADD CONSTRAINT product_listing_translations_language_check CHECK (language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi'));

ALTER TABLE search_filters
    DROP CONSTRAINT search_filters_language_check,
    ADD CONSTRAINT search_filters_language_check CHECK (language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi'));
