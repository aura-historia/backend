ALTER TABLE search_filter_matches
    DROP CONSTRAINT search_filter_matches_shipping_fx_check,
    ADD CONSTRAINT search_filter_matches_shipping_fx_check CHECK (
        (shipping_fx_rate_id IS NULL AND shipping_fx_captured_at IS NULL
         AND shipping_target_currency IS NULL AND shipping_maximum_minor IS NULL)
        OR
        (shipping_fx_rate_id IS NOT NULL AND shipping_fx_captured_at IS NOT NULL
         AND shipping_target_currency IS NOT NULL AND shipping_maximum_minor IS NOT NULL
         AND shipping_maximum_minor >= 0
         AND shipping_target_currency IN (
             'EUR','GBP','USD','AUD','CAD','NZD','CNY','BRL','PLN','TRY','JPY','CZK',
             'RUB','AED','SAR','HKD','SGD','CHF','ZAR','SEK','DKK','NOK','KRW','INR',
             'TWD','HUF','RON','MXN','THB','ARS'
         ))
    );
