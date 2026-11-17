-- Releases the product URLs stuck in an identity conflict once captured identities are kept
-- (20261117100000_pinned_source_listing_id.sql). Re-running it changes nothing more.

-- A URL whose last scrape captured normally extracted its captured identity. A URL in an identity
-- conflict extracted another one, which was never stored, so it stays unknown until its next
-- capture.
UPDATE listing_source_urls
SET last_extracted_source_listing_id = last_source_listing_id
WHERE last_source_listing_id IS NOT NULL
  AND last_extracted_source_listing_id IS NULL
  AND last_error_kind IS DISTINCT FROM 'SourceListingIdConflict';

-- URLs stuck in an identity conflict are due at once: their next scrape captures them under their
-- captured identity again.
UPDATE listing_source_urls
SET next_retry_at = NULL
WHERE last_error_kind = 'SourceListingIdConflict';
