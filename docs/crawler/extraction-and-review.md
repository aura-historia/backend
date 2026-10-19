# Crawler extraction and review

## Extraction contract

The scraper fetches active and sold product URLs, detects verified removed-page templates, and first evaluates cached schemas. It applies every cached schema to the same parsed page, validates each candidate independently, and ranks usable candidates by completeness before normalization. A bad candidate rejects that schema only; an external or system failure aborts the attempt and does not trigger generation.

Raw extraction remains source evidence. The crawler keeps raw strings, ordered image candidate groups, duplicates, relative values, rejected candidates, and configured raw attributes. It stores a validated, canonical image projection separately for generic raw values. Extracted values must never invent auction identity, lot data, or attribution.

A matching page hash and effective schema-set fingerprint can skip extraction. Otherwise the crawler hashes its generic raw-input envelope. Equal input refreshes crawler scrape metadata without creating a new operational capture.

Availability normalization is synchronous and deterministic in `product-listing-normalization`. Only a resolved `SoldOut` changes the crawler disposition. Quantity evidence resolves only as a whole label; negated, future or contradicted quantities stay unresolved.

Generic raw values keep every supplied price string. A price that preparation cannot resolve for lack of currency still reaches the worker, which preserves the canonical field and reports it. Candidate checks during extraction are the crawler's own quick checks: rejected candidates and schema repair stay crawler-side and never enter the business [normalization replay corpus](../normalization-replay.md). Description language may inherit title language only when the title itself supplied the detected language.

Shipping interpretation runs in downstream raw normalization using the same pure crate. It
combines item-specific `rawShipment`/`rawShipmentNote` text with description context;
contradictory or unsupported delivery clauses veto the observation. Duration ranges
remain complete text with their units and dispatch/delivery context.
Selectors must exclude shared footers and site policies. Explicit phrases cover all 14
catalog languages; phrase tables, language fixtures and input bounds are executable in
[`shipping.rs`](../../src/product-listing-normalization/src/shipping.rs). Interpretation
uses no LLM, interpretation agent or model fallback, and adds no schema-generation calls
or native description ingestion.

Successful page fetch time travels as the crawler capture's `source_occurred_at`, outside
the content hash. Schema evaluation, normalization, capture retries and queue delivery keep
that acquisition time. Historical captures without it remain readable and cannot establish
shipping observations. Matching page/input hashes retain the existing capture deduplication:
they do not advance shipping observation time. A new immutable raw revision can observe
unchanged shipping assertions when other captured item content changed. See the
[shipping contract](../shipping.md#deterministic-crawler-item-assertions) for application rules.

## Schema lifecycle

Fresh generation begins only after cached candidates are exhausted. It creates a new schema from the current page; it never edits a cached schema. A generated schema persists only after it applies and normalizes successfully.

Initial generation is product-schema-only: its response contract offers no page classification, every seed sample is treated as a product page, and a sample the model considers non-product is excluded from schema coverage and reported as a risk rather than classified. Single-page classification may produce a product schema, a verified removal, or a verified non-product result. A removal needs selector-bound evidence and results in raw deletion capture. A stored removed-page template is removal evidence only while no cached product schema still applies to the page with a title other than the not-found notice and, when the crawler URL already carries a captured source listing id, the same identity; a related-item card on a removed page is therefore not product evidence. When product evidence exists it wins, the page is scraped normally (a candidate that then fails deterministic preparation reaches fresh generation, not withdrawal) and the conflict is logged as an over-broad template to fix. A removed-page regex that matches any text is rejected both when proposed and when applied. A non-product result changes that URL only; one page never rewrites a domain URL pattern.

Every generated product schema must pass deterministic rule validation (selector syntax and attribute names) before it can be reviewed or persisted. An invalid response is returned to the model with a fixed correction code plus the offending field and value during bounded response correction, and the retry log carries the same detail without the prompt or HTML; if it stays invalid, generation ends with a schema-generation validation failure. The review gate rejects invalid candidates before review persistence, so review artifacts and approved schemas only ever contain valid rules, and model output is never reported as a database failure.

Generated selectors are grounded in page structure. Optional fields remain absent rather than guessed. A configured optional rule (identity, description, price, price estimates, raw attributes) that matches nothing on a page leaves that field empty with a per-field diagnostic; it never fails the page, so one page without a description does not reject the schema or trigger fresh generation. Title and state fail closed on a miss, and images fail closed unless the image container exists and is empty. The schema matrix and the review gate report optional-field misses separately from page failures: they stay visible to the reviewer and do not block deterministic approval. State selectors represent availability or a cart action, not price text. Raw attribute selectors are review/display inputs and participate in the raw-input hash; adding a new attribute requires schema regeneration for existing cached schemas.

## Review rail

The review rail records generated URL-pattern and product-schema artifacts as auditable state. Artifacts may be pending, approved, rejected, marked for repair, or superseded. When review gating is enabled, pending artifacts do not become active crawler configuration.

A review records its ListingSource, and URL-pattern reviews also record their domain. Candidate changes increment the review version and invalidate any cached schema matrix, so a stale live inspection cannot overwrite current review state. Review identities and matrix references use the TypeID policy in [object IDs](../object-ids.md).

A review-approved `NO_PATTERN` is a completed URL classification. It suppresses new pattern inference until an explicit reset returns the domain to `UNKNOWN`.

## LLM limits

LLM use is limited to URL-pattern inference, product-schema generation, fresh page classification, and schema evaluation. Provider selection belongs to executable wiring; crawler services rely on the generic LLM capability.

Crawler LLM calls are governed by bounded concurrency, minimum request-start spacing, bounded provider retries, and a per-ListingSource budget. A request reservation does not hold the scheduling mutex while it waits, and retry sleeps release the request permit. This keeps expensive recovery explicit and prevents unlimited generation.
