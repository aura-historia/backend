-- Every ListingSource that exists before this migration is a dealer, so the backfill is exact.
-- The default exists only for the backfill: writers must always state the type.
ALTER TABLE listing_sources
    ADD COLUMN listing_source_type text NOT NULL DEFAULT 'DEALER',
    ADD CONSTRAINT listing_sources_listing_source_type_check
        CHECK (listing_source_type IN ('DEALER', 'AUCTION_HOUSE', 'MARKETPLACE', 'AUCTION_PLATFORM'));

ALTER TABLE listing_sources
    ALTER COLUMN listing_source_type DROP DEFAULT;

-- Proposed ListingSources now state their type; every earlier proposal is a dealer as well.
UPDATE partnership_applications
SET proposal = jsonb_set(proposal, '{listing_source,listing_source_type}', '"DEALER"')
WHERE proposal->>'type' = 'PROPOSED_LISTING_SOURCE'
  AND NOT proposal->'listing_source' ? 'listing_source_type';

-- The stored calculation predates every non-dealer ListingSource, so it splits exactly and
-- the public overview stays available until the next scheduled refresh.
ALTER TABLE public_catalogue_overview
    ADD COLUMN dealer_sources_indexed bigint,
    ADD COLUMN auction_house_sources_indexed bigint,
    ADD COLUMN marketplace_sources_indexed bigint,
    ADD COLUMN auction_platform_sources_indexed bigint;

UPDATE public_catalogue_overview
SET dealer_sources_indexed = sources_indexed,
    auction_house_sources_indexed = 0,
    marketplace_sources_indexed = 0,
    auction_platform_sources_indexed = 0;

ALTER TABLE public_catalogue_overview
    ALTER COLUMN dealer_sources_indexed SET NOT NULL,
    ALTER COLUMN auction_house_sources_indexed SET NOT NULL,
    ALTER COLUMN marketplace_sources_indexed SET NOT NULL,
    ALTER COLUMN auction_platform_sources_indexed SET NOT NULL,
    ADD CONSTRAINT public_catalogue_overview_sources_indexed_by_type_check CHECK (
        dealer_sources_indexed >= 0
        AND auction_house_sources_indexed >= 0
        AND marketplace_sources_indexed >= 0
        AND auction_platform_sources_indexed >= 0
        AND dealer_sources_indexed
            + auction_house_sources_indexed
            + marketplace_sources_indexed
            + auction_platform_sources_indexed = sources_indexed
    );
