-- Evaluation provenance is independent of the listing's sale/asking-price basis.
ALTER TABLE search_filter_matches
    ADD COLUMN shipping_fx_rate_id uuid REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    ADD COLUMN shipping_fx_captured_at timestamptz,
    ADD COLUMN shipping_target_currency text,
    ADD COLUMN shipping_maximum_minor bigint,
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
             'TWD','HUF','RON','MXN','THB'
         ))
    );
