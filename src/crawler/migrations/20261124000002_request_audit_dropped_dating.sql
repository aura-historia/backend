-- One-off: asks the live schema audit to reopen the two ListingSources whose titles dropped the
-- dating and origin their pages show (#3118), which no check can find. The audit takes them in its
-- next pass and hands the note to the review agents: Ronald Phillips and Philip Salmon & Company
-- Rare Books, matched by their slugs. A database without them, such as a local or test one, is
-- unchanged.
UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = 'Titles dropped the dating and origin the page shows (e.g. "A GEORGE III MAHOGANY CABINET, English, circa 1775"). Keep them in the title as the heading shows them, or capture them in rawPeriod/rawOrigin; never drop them.'
WHERE listing_source_slug IN ('ronald-phillips', 'philip-salmon-company-rare-books');
