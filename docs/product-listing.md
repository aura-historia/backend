# ProductListing domain contract

## Scope

Aura owns `ProductListing`: a source's offer, not an intrinsic/catalog `Product` identity. Provider vocabulary stays at adapter boundaries.

This document owns listing semantics. [OpenAPI](swagger.yaml) owns REST shapes and statuses; [event flow](events/flow.md) owns delivery contracts; the [worker runbook](durable-worker-runbook.md) owns recovery.

## Aggregate and invariants

- A listing has an immutable `ProductListingId` and source key `(ListingSourceId, SourceListingId)`.
- `SourceListingId` is opaque: trim outer Unicode whitespace, reject blank/NUL-containing values, preserve case, punctuation and internal whitespace, and limit the canonical value to 512 UTF-8 bytes. Only the trimmed value is persisted and emitted.
- `ProductListingSlugId` is an immutable, globally unique Aura-owned public locator (`productListingTitleSlugId`), not a source-composite key or TypeID. Creation selects it through a collision-aware service flow.
- Availability, catalog lifecycle and sale observation are independent dimensions. Persisted state must satisfy the same invariants as new state; invalid state fails closed.

## Availability and orderability

Availability is an optional, reliable current source assertion. It has no `UNKNOWN` or default value.

| Canonical code | Meaning |
| --- | --- |
| `AVAILABLE` | Available without finer stock detail. |
| `IN_STOCK` | Ordinary current stock is available. |
| `LIMITED_AVAILABILITY` | Explicitly limited quantity/capacity. |
| `BACK_ORDER` | Orders accepted for later fulfillment. |
| `MADE_TO_ORDER` | Prepared or produced after order. |
| `PRE_ORDER` | Orders accepted before ordinary release. |
| `PRE_SALE` | Explicit pre-sale, distinct from pre-order. |
| `UNAVAILABLE` | Unavailable without a precise reason. |
| `RESERVED` | Temporarily held for another buyer. |
| `OUT_OF_STOCK` | No current stock; it may return. |
| `SOLD_OUT` | Source says sold or permanently exhausted. |

Orderability is derived, never independently mutable or persisted:

| Availability codes | Orderability |
| --- | --- |
| `AVAILABLE`, `IN_STOCK`, `LIMITED_AVAILABILITY` | `ORDERABLE_NOW` |
| `BACK_ORDER`, `MADE_TO_ORDER`, `PRE_ORDER`, `PRE_SALE` | `ORDERABLE_CONDITIONALLY` |
| `UNAVAILABLE`, `RESERVED`, `OUT_OF_STOCK`, `SOLD_OUT` | `NOT_ORDERABLE` |

Query exact values OR together; orderability expands to these values. Supplying both intersects them. `include_unspecified` optionally ORs in absence; contradictory filters match no concrete values. Search omits absent availability rather than indexing a sentinel.

## Lifecycle and absence semantics

- New listings are `ACTIVE`. Ordinary listing-data mutation requires an active listing.
- `WITHDRAWN` means the source no longer offers/publishes the listing. Withdrawal clears availability but retains sale observation, history, watch state and quota occupancy. It is reversible, not physical purge or retention cleanup.
- Only explicit restore/upsert intent restores a withdrawn listing. Restore starts without an availability assertion; upsert then applies supplied facts.
- Absent availability on an active listing means no sufficiently reliable current assertion. It does not mean unavailable, sold, unchanged or extraction failure.
- Absent asking price means no current assertion. `MONETARY` is an explicit numeric price; `ON_REQUEST` is an explicit request to ask the seller/source. Only monetary prices support conversion and numeric filtering; estimates remain monetary-only.

Absence is state, not a write instruction. Patches distinguish `Unchanged`, `Clear` and `Set`; clearing an asserted price or availability is an ordinary semantic change. `LISTED`, `UNKNOWN`, `REMOVED` and `SOLD` are not listing availability codes.

## Sale observation

`ListingSaleObservation` is complete or absent: `observed_at` records when Aura first observed explicit sold evidence, and `fx_rate_id` pins the snapshot used to value the last advertised source price. It is not proof of a completed transaction or its amount.

- Availability writes never create, overwrite or clear an observation. `SOLD_OUT` without an observation is valid.
- Recording uses a dedicated authorized transaction and the latest persisted FX snapshot at or before `observed_at`.
- An equal observation is a no-op; a different observation conflicts. Retraction is a dedicated correction, not an implicit overwrite.
- An observation may survive withdrawal or relisting. Use its FX only while currently sold out or for deliberately historical/withdrawn presentation; an active relisted listing uses current FX.

## Behaviors and events

Creation emits `PRODUCT_LISTING_DISCOVERED`. A later committed semantic revision emits at most one non-empty `PRODUCT_LISTING_CHANGED`; no-ops emit nothing. The service supplies event identity/time and commits state and event atomically. Rehydration emits no events.

- Discovery records immutable source identity, initial title/description, pricing, availability, URL, image count and Auction/lot facts. It omits the public slug, image URLs, lifecycle and sale observation. Initial discovery cannot include a lifecycle transition or sale observation.
- Changes keep price, each estimate, availability, URL, image replacement, Auction/lot facts, lifecycle and sale observation separate.
- Ordinary changes coalesce first `previous` and final `current`; net-zero changes disappear. Image identity/order changes remain meaningful even with equal counts; durable counts use `u64`.
- Withdrawal records the lifecycle transition and previous availability. Sale observations transition only absent → present or present → absent, never directly between different observations.

Enrichment is separate from aggregate changes and public domain history. Persisted event decoding is strict; delivery and projection rules belong to [event flow](events/flow.md).

## Application, API, and search contracts

Title and description are optional creation-only inputs. Upsert of an existing listing preserves them. Availability, main price and estimates use tri-state patches: omitted preserves, `null` clears, a value sets. Absent response availability is explicit `null`.

Title normalization is idempotent: truncation ellipses remain stable, and removing
sentence-final periods also removes exposed outer whitespace. Shortened ellipses
from prior rehydration remain readable. PostgreSQL title decoders also accept
bounded, otherwise-canonical titles with trailing whitespace emitted by the old
constructor, presenting the canonical value without rewriting immutable history.
Leading whitespace, case/punctuation changes and overlength persisted titles still
fail validation. This compatibility does not regenerate public IDs or slugs.

Listing-owned Auction/lot facts may exist without an Auction ID; all-empty facts normalize to absence. A supplied Auction ID must resolve to an existing Auction for the same ListingSource. Lot labels are opaque; catalogue positions are positive and one-based. Timing facts require exact instants, not guessed date-only or timezone-ambiguous values. Partner listing writes do not create or mutate Auctions; raw normalization preserves stored Auction/lot facts.

Public discovery and detail expose active listings only. Withdrawn detail is not found, rather than gone; withdrawal removes the search projection and restore rebuilds it. Auction filters match resolved membership, not standalone lot facts. Exact REST patch exceptions, routes and filter limits remain in [OpenAPI](swagger.yaml).

## Asynchronous ingestion submission

**Typed intent → confirmed FIFO admission → atomic command application and receipt → downstream work.** Admission is never business completion.

| Intent | Execution meaning |
| --- | --- |
| `CREATE` | Existing create semantics. |
| `UPDATE` | Change an existing listing only. |
| `UPSERT` | Create, update or explicitly restore. |
| `WITHDRAW` | Retain the listing and its history. |
| `CAPTURE_RAW` | Store source evidence; separate normalization may later change canonical state. |

Admission awaits publication; it does not write canonical state or fall back to detached work. Partner intake requires a user, with `ProductListingsWrite` for delegated credentials; internal intake accepts only trusted service/system callers without a partnership prerequisite. Intake validates input, actor/scope and envelope, not current database eligibility. Execution checks current source grants, referenced entities, lifecycle and business conflicts. Provider authentication remains at its boundary; credentials never enter messages.

Each item retains its unique, in-range original zero-based index and input count, including repeated listing keys. Invalid items need not block valid siblings. An evaluated report accounts for every input as confirmed acceptance or an original-index failure, not listing IDs or a completion/polling result.

| Outcome | Meaning |
| --- | --- |
| `Accepted` | Matching publisher receipt confirms queue custody only. |
| `Rejected` | Admission rejected, with a safe reason and retryability. |
| `Unconfirmed` | Possibly queued; never count as accepted. |
| `NotAttempted` | Unsent because of a deadline or blocked FIFO predecessor. |

Missing or non-unique publisher outcomes are unconfirmed, not success. Within each logical FIFO group, a successor waits for confirmed predecessor admission. Unsent deterministic rejection reserves no position; a sent failed/uncertain predecessor blocks successors. Independent groups may advance. Uncertainty and deadline exhaustion are retryable; blocked successors inherit predecessor retryability.

### Retry identity

- REST accepts one `Idempotency-Key` of 1–128 visible ASCII bytes excluding comma. Without it, intake generates and returns an effective key; a key lost with the first response cannot be recovered.
- `submissionId` scopes actor, source and effective key; `commandId` additionally scopes operation and original index. Request IDs and current time do not affect them. An authorized delegated caller shares the same user's identity scope.
- Retry a partial/lost response with the **unchanged original ordered batch**, actor/source, effective key, operations, indices and count. Never compact, reorder or change content under that key.
- Stable IDs do not reproduce a stored response or guarantee exactly-once execution. There is no submission registry or batch rollback; commands sharing a submission remain independent.

FIFO orders per-group shared-queue admission, not upstream chronology, cross-group execution, synchronous writes or raw normalization. Wire validation and delivery mechanics belong to [event flow](events/flow.md); recovery belongs to the [worker runbook](durable-worker-runbook.md).

### Downstream command execution

Execution verifies semantic integrity and serializes by `commandId`. Canonical state/event or raw capture and an `APPLIED` receipt commit together. Matching committed receipts replay without effects; conflicting reuse fails closed. Rejection or rollback creates no success receipt. After an ambiguous commit, retry checks the receipt; acknowledge only confirmed application or matching replay.

Receipts survive listing/source deletion and have no TTL. They deduplicate successful command application, not admission, rejection or an entire batch. Raw capture completion still does not mean canonical normalization or projection completion. [Storage](storage.md) owns persistence guardrails; [event flow](events/flow.md) owns worker acknowledgment.

## Content assessment and image visibility

Images are URL-only source facts. Listing-level assessment is asynchronous enrichment; missing or stale assessment means unassessed, not an invented policy value.

Without the stored `show_unassessed_or_sensitive_content` preference, image URLs are visible only under a current `ALLOWED` assessment. Opted-in users may see allowed, sensitive (`REQUIRES_CONSENT(NAZI_GERMANY)`) and unassessed images. Redaction preserves image order/cardinality and emits hidden URLs as `null`.

Assessment does not block ingestion, change aggregate revision/history or become search authority. Non-content changes do not invalidate text assessment; a new text source must invalidate the old result.

## Source anti-corruption rules

Boundary uncertainty never becomes invented domain truth. Reliable assertions set availability; reliable absence clears it; ambiguous or failed extraction preserves it. Only reliable removal evidence withdraws a listing, never timeout, blocking or parsing failure. Missing/untracked inventory is not zero; zero stock is not sold evidence. Explicit sold availability still does not implicitly create a sale observation.

Provider mappings and ordering/recovery details belong to [event flow](events/flow.md); crawler-specific contracts remain in the [crawler docs](crawler/README.md).

## Persistence contract

PostgreSQL is authoritative; OpenSearch is rebuildable. Raw evidence is separate from canonical state and history. Raw capture alone cannot mutate the listing; ordered normalization is responsible for canonical changes.

Canonical enum codes and persisted invariants are decoded exactly, without defaults or case-normalizing corruption. Storage JSON identity exceptions live in [object IDs](object-ids.md#storage-only-json); schema and concurrency guardrails belong to [storage](storage.md).

## Public history

History exposes only committed `PRODUCT_LISTING_DISCOVERED` and `PRODUCT_LISTING_CHANGED` domain entries, ordered by occurrence time then event ID. One changed entry represents one committed revision with a deterministically ordered `changes` list.

History excludes raw evidence, operational receipts, enrichment, storage/core payload wrappers and source image URLs. Public object identities use TypeIDs even where persisted event JSON uses UUID text.
