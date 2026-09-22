# Crawler extraction and review

## Extraction contract

The scraper fetches active and sold product URLs, detects verified removed-page templates, and first evaluates cached schemas. It applies every cached schema to the same parsed page, validates each candidate independently, and ranks usable candidates by completeness before normalization. A bad candidate rejects that schema only; an external or system failure aborts the attempt and does not trigger generation.

Raw extraction remains source evidence. The crawler keeps raw strings, ordered image candidate groups, duplicates, relative values, rejected candidates, and configured raw attributes. It stores a validated, canonical image projection separately for generic raw values. Extracted values must never invent auction identity, lot data, or attribution.

A matching page hash and effective schema-set fingerprint can skip extraction. Otherwise the crawler hashes its generic raw-input envelope. Equal input refreshes crawler scrape metadata without creating a new operational capture.

Availability normalization is synchronous and deterministic in `product-listing-normalization`. Only a resolved `SoldOut` changes the crawler disposition. Description language may inherit title language only when the title itself supplied the detected language.

## Schema lifecycle

Fresh generation begins only after cached candidates are exhausted. It creates a new schema from the current page; it never edits a cached schema. A generated schema persists only after it applies and normalizes successfully.

Initial generation is product-schema-only. Single-page classification may produce a product schema, a verified removal, or a verified non-product result. A removal needs selector-bound evidence and results in raw deletion capture. A non-product result changes that URL only; one page never rewrites a domain URL pattern.

Generated selectors are grounded in page structure. Optional fields remain absent rather than guessed. State selectors represent availability or a cart action, not price text. Raw attribute selectors are review/display inputs and participate in the raw-input hash; adding a new attribute requires schema regeneration for existing cached schemas.

## Review rail

The review rail records generated URL-pattern and product-schema artifacts as auditable state. Artifacts may be pending, approved, rejected, marked for repair, or superseded. When review gating is enabled, pending artifacts do not become active crawler configuration.

A review records its ListingSource, and URL-pattern reviews also record their domain. Candidate changes increment the review version and invalidate any cached schema matrix, so a stale live inspection cannot overwrite current review state. Review identities and matrix references use the TypeID policy in [object IDs](../object-ids.md).

A review-approved `NO_PATTERN` is a completed URL classification. It suppresses new pattern inference until an explicit reset returns the domain to `UNKNOWN`.

## LLM limits

LLM use is limited to URL-pattern inference, product-schema generation, fresh page classification, and schema evaluation. Provider selection belongs to executable wiring; crawler services rely on the generic LLM capability.

Crawler LLM calls are governed by bounded concurrency, minimum request-start spacing, bounded provider retries, and a per-ListingSource budget. A request reservation does not hold the scheduling mutex while it waits, and retry sleeps release the request permit. This keeps expensive recovery explicit and prevents unlimited generation.
