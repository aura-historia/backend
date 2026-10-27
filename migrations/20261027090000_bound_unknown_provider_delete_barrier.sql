-- A provider delete without a source time cannot be placed in provider order. Its capture time
-- is an upper bound for when the provider deleted the record, so a later upsert whose source
-- time is after it provably follows the delete and may restore the listing.
ALTER TABLE product_listing_raw_streams
    ADD COLUMN latest_provider_unknown_delete_captured_at timestamptz;

-- The stream's last write time is at or after the capture of its timestamp-free delete.
UPDATE product_listing_raw_streams
SET latest_provider_unknown_delete_captured_at = updated
WHERE latest_provider_source_ordering_state = 'UNKNOWN_DELETE';

ALTER TABLE product_listing_raw_streams
    ADD CONSTRAINT product_listing_raw_streams_unknown_delete_captured_at_shape_check
        CHECK (
            (latest_provider_source_ordering_state = 'UNKNOWN_DELETE')
            = (latest_provider_unknown_delete_captured_at IS NOT NULL)
        );
