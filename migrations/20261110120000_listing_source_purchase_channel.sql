-- Whether visitors can buy online at the source. The default is permanent: the backfill is not
-- exact, because known enquiry-only galleries stay ONLINE_SHOP until an administrator PATCHes them.
ALTER TABLE listing_sources
    ADD COLUMN purchase_channel text NOT NULL DEFAULT 'ONLINE_SHOP',
    ADD CONSTRAINT listing_sources_purchase_channel_check
        CHECK (purchase_channel IN ('ONLINE_SHOP', 'ENQUIRY_ONLY'));
