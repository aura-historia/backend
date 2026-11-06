-- An operator's confirmation, made in the review console, that the ListingSource's removals are
-- a real bulk clearance: the scraper's mass-removal guard captures its removals until the end
-- time, then holds again (docs/crawler/scheduling-and-retries.md, mass-removal guard). The actor
-- is a review actor name; only a human confirms.
ALTER TABLE listing_sources
    ADD COLUMN mass_removal_confirmed_until TIMESTAMPTZ,
    ADD COLUMN mass_removal_confirmed_by TEXT,
    ADD CONSTRAINT listing_sources_mass_removal_confirmation_shape_check
        CHECK ((mass_removal_confirmed_until IS NULL) = (mass_removal_confirmed_by IS NULL)),
    ADD CONSTRAINT listing_sources_mass_removal_confirmed_by_check
        CHECK (mass_removal_confirmed_by IS NULL
               OR mass_removal_confirmed_by IN ('HUMAN', 'AGENT_AUTHOR', 'AGENT_REVIEWER', 'SYSTEM'));
