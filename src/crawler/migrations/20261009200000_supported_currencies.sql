ALTER TABLE listing_sources
    DROP CONSTRAINT listing_sources_fallback_currency_check,
    ADD CONSTRAINT listing_sources_fallback_currency_check CHECK (
        fallback_currency IS NULL OR fallback_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB')
    );
