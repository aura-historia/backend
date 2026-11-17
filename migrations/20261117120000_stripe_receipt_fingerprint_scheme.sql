-- Stripe webhook receipts fingerprint what an event requests (type, subscription, customer)
-- instead of its rendered snapshot. Rows written by the retired EventBridge consumer, which
-- leaves this column out, keep the snapshot scheme: the webhook treats those receipts as
-- applied when customer and subscription match, so an event both destinations deliver during
-- the cutover still dedupes. Adding a column with a constant default rewrites no rows.
ALTER TABLE stripe_subscription_event_receipts
    ADD COLUMN fingerprint_scheme text NOT NULL DEFAULT 'EVENT_SNAPSHOT'
        CHECK (fingerprint_scheme IN ('EVENT_SNAPSHOT', 'EVENT_REQUEST'));
