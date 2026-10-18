-- Entity freshness is database-owned and strictly monotonic per row. Writers compute the next
-- value atomically from the stored value after their concurrency checks, so a transaction that
-- started before a newer committed write, equal clock readings or a clock behind the stored value
-- cannot regress or repeat `updated`. Millisecond precision matches the OpenSearch `date` mapping,
-- so every advance stays distinguishable in indexed and serialized representations.
CREATE FUNCTION aura_advance_updated(previous timestamptz) RETURNS timestamptz
LANGUAGE sql VOLATILE PARALLEL SAFE AS $$
    SELECT GREATEST(
        date_trunc('milliseconds', clock_timestamp()),
        date_trunc('milliseconds', previous) + INTERVAL '1 millisecond'
    )
$$;

ALTER TABLE product_listings
    ALTER COLUMN created SET DEFAULT date_trunc('milliseconds', now()),
    ALTER COLUMN updated SET DEFAULT date_trunc('milliseconds', now());

-- Content assessment is listing-owned enrichment state; its changes advance listing freshness
-- and re-project the listing through an enrichment event.
ALTER TABLE product_listing_events DROP CONSTRAINT product_listing_events_type_group_check;
ALTER TABLE product_listing_events ADD CONSTRAINT product_listing_events_type_group_check CHECK (
    (event_group='DOMAIN' AND event_type IN ('PRODUCT_LISTING_DISCOVERED','PRODUCT_LISTING_CHANGED')) OR
    (event_group='ENRICHMENT' AND event_type IN ('ENRICHMENT_EMBEDDED','ENRICHMENT_TRANSLATED_TITLES','ENRICHMENT_GEOGRAPHY_UPDATED','ENRICHMENT_SHIPPING_UPDATED','ENRICHMENT_CONTENT_ASSESSED'))
);
