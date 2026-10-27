-- A provider delete without a source time cannot be placed in provider order. Its barrier is the
-- later of the delete's capture time and every source time already known for the record, so an
-- observation with a later source time follows every delete Aura has seen and may restore it.
ALTER TABLE product_listing_raw_streams
    ADD COLUMN latest_provider_delete_barrier_at timestamptz;

-- The stream's last write time is at or after the capture of its timestamp-free delete.
UPDATE product_listing_raw_streams
SET latest_provider_delete_barrier_at = updated
WHERE latest_provider_source_ordering_state = 'UNKNOWN_DELETE';

ALTER TABLE product_listing_raw_streams
    ADD CONSTRAINT product_listing_raw_streams_delete_barrier_shape_check
        CHECK (
            (latest_provider_source_ordering_state = 'UNKNOWN_DELETE')
            = (latest_provider_delete_barrier_at IS NOT NULL)
        );
