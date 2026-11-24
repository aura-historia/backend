-- One-off (#3118): asks the live schema audit to reopen the ListingSources from the issue whose
-- titles carry what no check reliably finds, each with a note of what the review agents should
-- repair. The audit takes them in its next passes, ahead of the others. Ronald Phillips and Philip
-- Salmon are matched by their slugs; the others by the slug their name gives or by that name, in
-- case a collision suffixed the slug. A database without them, such as a local or test one, is
-- unchanged.
UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = 'Titles dropped the dating and origin the page shows (e.g. "A GEORGE III MAHOGANY CABINET, English, circa 1775"). Keep them in the title as the heading shows them, or capture them in rawPeriod/rawOrigin; never drop them.'
WHERE listing_source_slug IN ('ronald-phillips', 'philip-salmon-company-rare-books');

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = 'Titles carry the article number, e.g. "Antiek hek 2118031", and switched between with and without it across schema saves. Read the product name from an element without the number; the number belongs in source_listing_id, not in the title.'
WHERE listing_source_slug = 'burbri-antiek' OR listing_source_name = 'Burbri Antiek';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = 'Titles carry the item number in brackets, e.g. "… (商品番号 cm-396)". Read the product name without it; the item number belongs in source_listing_id, not in the title.'
WHERE listing_source_slug = 'anteikusiyotupuhandle' OR listing_source_name = 'アンティークショップHandle';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = 'Titles start with a section heading, e.g. "Informatie over de Kuyperkaart Gemeente Bathmen" instead of "Gemeente Bathmen". Read only the map''s own name.'
WHERE listing_source_slug = 'atlas-en-kaart' OR listing_source_name = 'Atlas en Kaart';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = 'Titles run the author into the name without a separator, e.g. "James Ensor (1860-1949) Avermaete, Roger" instead of "James Ensor (1860-1949)". Read the name alone and capture the author in rawCreatorNote.'
WHERE listing_source_slug = 'archives-du-nord' OR listing_source_name = 'Archives du Nord';
