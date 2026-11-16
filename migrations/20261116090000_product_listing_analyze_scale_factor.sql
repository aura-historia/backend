-- The admin overview estimates ProductListing counts from planner statistics. Re-analyze after 1%
-- of the table changed instead of the server default, so its breakdowns trail by about that
-- share of listings. Takes SHARE UPDATE EXCLUSIVE, which blocks neither reads nor writes.
ALTER TABLE product_listings SET (autovacuum_analyze_scale_factor = 0.01);
