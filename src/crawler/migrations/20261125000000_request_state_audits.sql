-- One-off (#2878): asks the live schema audit to reopen the ListingSources from the issue whose
-- `state` rule reads page chrome or the title instead of the listing's status, each with a note
-- of what the review agents should repair. No check finds this, so only such a request reopens
-- them; the audit takes them in its next passes, ahead of the others. Each source is matched by
-- the listing_source_id the issue names or by its crawl host. A database without them, such as a
-- local or test one, is unchanged.
UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = '`state` reads the shop-wide notice "欲購買任何商品請私訊我們的粉絲專頁" (div.items-single__info-bottom > p), the same on every page, so no listing gets an availability. Read `state` from a per-listing element, such as the sold-out label in the price slot, or drop the rule.'
WHERE listing_source_id = '01a1a182-b2c2-71f3-a843-b3e593e11720' OR crawl_host = 'naturalnvintage.com';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = '`state` reads the "Valider" button of a reminder form (input#reminder-submit) on every page, so no listing gets an availability. Read the availability of the page''s JSON-LD offer or another per-listing element, or drop the rule.'
WHERE listing_source_id = '01a1a2b4-2a9a-7241-91a3-dc38b2a7e35c' OR crawl_host = 'napoli-antic.fr';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = '`state` reads the site navigation "Acheter un livre ancien" (alt of input.commande) on every page. Read a per-listing element, such as the itemprop availability the fallback names, or drop the rule. Some product pages capture no title or identity, e.g. https://www.bibliographique.com/livre-nogueira-portugaliae-tabellionum-instrumenta-documentacao-5290: check their template.'
WHERE listing_source_id = '01a1a26a-1533-73fa-9f64-24fdc5df42aa' OR crawl_host = 'bibliographique.com';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = '`state` reads the first button of div.product_button, the shop-wide "如需購買請私訊我們" on every page. Read a per-listing element, such as the add-to-cart button or the JSON-LD offer availability, or drop the rule.'
WHERE listing_source_id = '01a1a1a0-d04c-717e-980d-caaa8e5da3cd' OR crawl_host = 'jandtliving.com.tw';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = '`state` falls back to the title (h2.fusion-title-heading), so sold items capture the whole title, e.g. "Vintage satijnen sjaal (rood) – [sold]". Drop that fallback: `state` may read the [sold] marker only from an element that holds it alone. Where none does, say so in the summary for a human.'
WHERE listing_source_id = '01a1991e-f720-726b-bfb2-4dbb11c3018a' OR crawl_host = 'jalabala.nl';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = '`state` reads the marketing line "Acheter en toute sécurité avec Stripe" (a.action) on every page. Read a per-listing status element, or drop the rule.'
WHERE listing_source_id = '01a19951-8874-71ca-8b01-6b88cbec6b2b' OR crawl_host = 'boutique.cimiezantiquites.fr';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = '`state` reads an unscoped button[type=submit], which takes the cookie banner''s "Cookies erlauben" before the product form''s button. Scope the selector to the product''s own form.'
WHERE listing_source_id = '01a199dd-68c8-7074-9140-a13d40562965' OR crawl_host = 'antiquariat-norbert-haas.de';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = '`state` falls back to page text such as "Holy Land" or "3 modèles au choix" (div.ProductGallery div.ProductCatchPhrase). Keep only per-listing status elements. Some product pages capture no title or identity, e.g. https://decouvertesdumonde.fr/2182439-Paire-de-statuettes-en-bois-exotique-chasseurs-guerriers-malgaches: check their template.'
WHERE listing_source_id = '01a19948-5c0e-7030-a821-266f93c610ac' OR crawl_host = 'decouvertesdumonde.fr';

UPDATE listing_sources
SET schema_audit_requested_at = NOW(),
    schema_audit_request_note = '`state` falls back to div.toevoegen, which captured a description sentence and the word "Een". Keep only the cart button or another per-listing status element.'
WHERE listing_source_id = '01a19d28-c555-72ab-b739-1d986e6e6ea2' OR crawl_host = 'curiosadevlinder.com';
