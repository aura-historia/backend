-- Active PartyLocations always provide exact geography. Keep earlier migrations
-- immutable and adapt completed creation receipts to the reduced command contract.
ALTER TABLE party_locations DROP COLUMN disclosure;

UPDATE party_location_create_receipts
SET command = command #- '{content,disclosure}',
    result = result #- '{content,disclosure}';

-- Recompute existing derived geography under the new policy in bounded pages.
-- Locations that previously withheld coordinates now contribute their exact point.
SELECT enqueue_listing_geography('SOURCE', listing_source_id::text) FROM listing_sources;
