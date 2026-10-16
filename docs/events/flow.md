# Event flow

This document owns event sources, routing and consumer completion semantics.
[Architecture](../arch.md#12-cdc-and-projection-architecture) owns general rules;
the [worker runbook](../durable-worker-runbook.md) owns activation and recovery.
Code and configuration define exact codecs and resource settings, not live state.

## Delivery paths

```mermaid
flowchart TD
    Writes[Service transaction] --> PG[PostgreSQL state and events]
    PG --> DMS[DMS CDC]
    DMS --> Kinesis[Kinesis]
    Kinesis --> Router[Router Lambda]
    Router --> Standard[Scoped Standard SQS queues]
    Router --> Consent[Recipient-scoped consent FIFO]
    Standard --> Workers[Worker use cases]
    Consent --> Loops[Consent use case and Loops adapter]
    Intake[Async partner and provider intake] --> Commands[ProductListing command FIFO]
    Commands --> Writes
```

Command admission and committed-change delivery are separate boundaries. An API
success may confirm a transaction or queue custody; neither implies search visibility
or external delivery. PostgreSQL remains authoritative. DMS start and router activation
require the [infrastructure procedure](../infra.md#cdc-and-scheduled-work).

## Shipping management signals

[Shipping](../shipping.md#immutable-management-change-signals) owns version 1 immutable
profile/default signals, retained deletion fences and protected history. Only the
minimal address-free signal is eligible for future invalidation delivery; terms,
evidence and creation receipts remain outside ordinary CDC. Profile management commits
signals locally and performs no synchronous fan-out. Shipping fan-out routing and
consumers are not activated by profile management.

## ProductListing write flow

- Partner writes commit canonical state and at most one semantic event atomically.
  The immutable domain/enrichment journal is the projection source, not a generic
  outbox or an event-sourced aggregate. [ProductListing](../product-listing.md) owns
  lifecycle, sale-observation and event meaning.
- Crawler and provider observations capture separate raw evidence and immutable
  revisions. Only ordered normalization turns those revisions into canonical state
  and events. It preserves listing-owned Auction/lot facts; it does not mutate Auctions.
- Raw-revision jobs are wake-ups: normalization drains the authoritative stream head
  and commits terminal progress with any canonical change. Invalid candidate data
  may be terminally rejected; dependency/configuration failures leave work pending.
  Capped or unfinished drains are not complete. There is no scheduled raw-backlog reconciler.
- Async partner commands and mapped Shopify/WooCommerce observations use a separate
  ProductListing FIFO. The consumer commits canonical state/event or raw capture
  together with its successful command receipt. [Admission, execution and retry identity](../product-listing.md#asynchronous-ingestion-submission)
  are independent of downstream normalization and projection.

## Geography dependency invalidation

Party service owns `party_location_changes`; ListingSource service owns
`listing_source_location_default_changes`. Each minimal immutable revision signal commits
with its authoritative semantic mutation. Default signals contain source/operator and
optional location identities, assignment/location revision, applicability flags, actor
and occurrence time, without address, point or evidence. Duplicate current-revision
no-ops emit nothing; clearing emits a null selection and preserves the ordering fence.
Location/default rows and receipts remain outside CDC. Exact persistence and
inheritance semantics belong to [geography](../geography.md#listingsource-location-defaults-2000).

These signals atomically establish coalesced durable listing-geography work. The
`listing_geography_wakeups` table alone adds CDC/job routing; the original journals stay
outside DMS. Consumers reread current authority under dependency equality guards and
use the listing's monotonic projection fence. Location correction, retirement,
source replacement/clear and provider rights/expiry refresh current read state without
rewriting historical listing assertions. Missing authority produces absence. See the
[effective geography contract](../geography.md#effective-listing-geography-2003) and
[recovery runbook](../durable-worker-runbook.md#listing-geography-recovery).

## CDC routing

The router consumes native Kinesis envelopes containing strict DMS records. It
validates the complete route and encoded jobs before any send, and checkpoints only
once every required publication is confirmed. Partial or uncertain fanout can duplicate
jobs. Failure stops at the earliest unconfirmed sequence; safe non-trigger operations
and informational controls produce no jobs. Invalid rows and incompatible schema
controls retain failure custody rather than being silently skipped. The DMS-generated
`create-table` control for `awsdms_apply_exceptions` in the empty target schema is
informational and creates no jobs; this exception does not accept its data records,
other schema operations or unknown tables.

The DMS adapter decodes a serialized JSONB `product_listing_events.payload` once
before shared event validation; already-object payloads follow the same validation.
Malformed JSON, non-object values, unsupported versions and invalid event fields
retain failure custody. Other row fields and raw evidence are not decoded into
jobs. This transport conversion preserves event identity and retry keys.

| Selected source | Trigger | Consumer |
| --- | --- | --- |
| `product_listing_events` | INSERT | Product projector; additional routes below |
| `product_listing_raw_revisions` | INSERT | Normalization only |
| `search_filter_matches` | INSERT | Match-notification generator |
| `notification_deliveries` | INSERT | Notification delivery |
| `marketing_email_consent_sync_intents` | INSERT | Recipient-scoped consent FIFO |
| `geo_resolution_wakeups` | INSERT | Geographic resolution worker |
| `listing_geography_wakeups` | INSERT | Effective listing geography worker |

Product event fanout:

| Event | Additional consumers |
| --- | --- |
| `PRODUCT_LISTING_DISCOVERED` | Content assessment, embedding, translation |
| `PRODUCT_LISTING_CHANGED` | Watchlist for main-price/availability changes; embedding for image changes |
| `ENRICHMENT_EMBEDDED`, `ENRICHMENT_TRANSLATED_TITLES`, `ENRICHMENT_GEOGRAPHY_UPDATED` | None beyond product projector; no enrichment loop |

Combined change dimensions route to the union of their consumers. Canonical listing
rows, saved-filter configuration, Users, watchlist entries, Auction events, proofs and
operational receipts do not implicitly trigger these routes. In particular, a User update
is not a subscription job.

Consent CDC is deliberately minimized: INSERT/UPDATE carry only `intent_id` and
`recipient_key`; DELETE carries only the primary key under default replica identity.
Only INSERT produces work; extra fields fail validation even on non-trigger operations.
Email, profile, proof, source keys, leases and provider data must not enter this CDC path.

Geographic resolution CDC selects only the immutable resolution ID/generation wake-up
table, with exact decimal-string generations. All operations reject extra columns.
Private inputs, cache fingerprints, tenant scope, review choices and provider evidence
remain outside CDC. Resolution claims/completions reread PostgreSQL; old generations
complete as superseded and active leases retain retry custody. Hourly backend cleanup
supplies recovery for missed deliveries by advancing runnable pending generations and
inserting fresh immutable wake-ups, preserving attempts and budget reservations. It
also purges ineligible evidence and commits invalidation. Cleanup makes no provider
calls; fresh recovery wake-ups require the normal CDC/SQS path to be healthy.
Accepted evidence and the content-free enrichment/invalidation journal commit together.
Dependency journal inserts atomically coalesce effective-listing-geography work.
Only its compact work ID/generation wake-ups participate in CDC; private source
evidence, addresses, resolution payloads and checkpoints do not. See [durable geography](../geography.md#durable-resolution-and-party-integration-2002).
Cached reuse emits a subject-specific RESOLVED signal for a new source binding;
source revision changes clear reviewed geography and atomically emit INVALIDATED,
including when they retain the same resolution identity. Unchanged binding replays are silent.

Jobs use schema version **2** and typed application IDs; version 1 is rejected.
ProductListing event payloads have their own version, independent of the job envelope.
Domain events containing delivery use version 2 with a required `delivery` discovery
assertion or complete previous/current change; version 1 remains strict and readable.
The CDC router validates the versioned configuration and emits existing compact event jobs.
Delivery-only changes project the listing without triggering price/availability watchlist
notifications or image embedding. Protected delivery terms never enter ordinary jobs or
public history. Ingestion `CHANGE_DELIVERY` uses envelope version 2; earlier operations
retain their exact version 1 codecs, semantic fingerprints and retry identities.
Jobs carry compact identity/revision metadata, never CDC rows or raw evidence. The
`CAPTURE_RAW` command is a separate, explicitly designed evidence boundary; provider
credentials, signatures and signed request headers still never enter it.

Exact route and wire definitions live in [CDC routing](../../src/aura-historia-cdc-routing/src/lib.rs),
[router decoding](../../src/cdc-router-lambda/src/cdc.rs) and [job codecs](../../src/aura-historia-jobs/src/).

## Consumer completion and fences

Workers invoke service use cases and acknowledge only confirmed completed outcomes.
Duplicates, authoritative stale skips and deliberately ignored inputs may complete;
missing required source, active leases, malformed jobs, dependency failure or uncertain
commit/effect do not. SQS transport IDs are not business idempotency keys.

| Consumer | Authority and completion rule |
| --- | --- |
| Product projection | Reread complete current PostgreSQL state; reject superseded event triggers; write at the projection version. |
| Content assessment | Commit against the evaluated content-source revision; do not manufacture a domain event or write search directly. |
| Embedding / translation | Evaluate outside a transaction, then recheck the corresponding source marker and commit guarded enrichment. First committed completion wins; its journal entry drives projection. |
| Match notification | Read the exact persisted match and its historical origin, not a newer match or current-event substitute; commit notification and delivery intent together. |
| Watchlist notification | Use immutable event time and current active/notification-enabled intervals; later activation cannot earn an older notification. Suppress currently withdrawn listings. |
| Notification delivery | Commit a fenced claim before sending; finalize the same claim/result and acknowledge only confirmed terminal completion. |
| Marketing consent | Claim the exact intent, revalidate permission and reconcile provider outcomes; see [consent synchronization](../marketing-consent.md#synchronization-and-provider-eligibility). |
| Geographic resolution | Reserve a bounded, database-budgeted claim; call the provider outside transactions; commit permitted evidence and its signal under the same unexpired claim/generation. Negative outcomes have reconsideration deadlines; provider/configuration failures retain failure custody. |

ProductListing projections use externally versioned, content-free
`projectionDeleted: true` tombstones. Readers exclude them; older/equal writes cannot
resurrect deleted state. Tombstones must outlive physical-delete version memory.
[Rebuild and deletion recovery](../durable-worker-runbook.md#projection-fences-and-rebuild)
require authoritative facts and fenced writers, not an index reset.

FX capture alone does not trigger matching, projection or notification.

## Provider and scheduled flows

| Intake | Boundary and acknowledgment |
| --- | --- |
| WooCommerce | Signed HTTP intake plus partner authorization → command FIFO. A mapped webhook acknowledges confirmed admission, not raw capture; authorized ignored inputs create no command/receipt. |
| Shopify | Partner EventBridge → retained provider Standard queue → Shopify Lambda → command FIFO. Forwarding and command application are separate acknowledgments and failure locations. |
| Stripe | Partner EventBridge → dedicated Lambda → User-service reconciliation. Provider reads occur between short PostgreSQL transactions; receipts, tier changes and entitlement reconciliation commit together. |
| Cognito pre-sign-up | Federated collision checks fail closed. Linking requires the explicit provider trust policy and verified persisted identity; email alone is insufficient. |
| Cognito post-confirmation | Registration commits the initial User/binding. Profile bootstrap applies only on first creation; replay does not synchronize profile or restore consent. [Signup consent](../marketing-consent.md#grants-and-double-opt-in) is a separate verified decision. |
| Loops | Exact-byte signed HTTP event → atomic preference application/receipt. See [preference webhooks](../marketing-consent.md#preference-webhooks). |
| Cleanup / FX | Scheduler invokes bounded use cases. Expiry is enforced on use; cleanup is not an authorization check. FX occurrences keep stable retry identity. |
| Periodic matching | Scheduler starts a one-shot Fargate task; see [saved-filter matching](#periodic-saved-filter-matching). |
| Log retention | AWS log-group creation invokes the retention handler; narrowly configured evidence groups are exempt. It does not grant business runtimes log-administration rights. |

The Cognito collision policy is provider-specific: Google linking requires verified
incoming and destination identities; Facebook collisions are rejected, and a profile
linked to Facebook is not a Google-link destination. The database's email uniqueness
remains the race barrier. [Identity recovery](../infra.md#manual-operations) must preserve
the existing bound account. PostConfirmation errors propagate but do not roll back
Cognito confirmation; there is no durable queue/DLQ for that registration boundary.
Unsettled PostgreSQL registration therefore needs operator recovery.

### Periodic saved-filter matching

New saved-filter matches are discovered by scheduled runs for active ordinary and
enhanced filters. PostgreSQL owns filter configuration, permitted matching evidence,
progress, matches, feedback and notification eligibility. Creates, updates, reactivation
and subscription reconciliation resolve evidence under the authoritative user-tier lock:
Free permits title/description, Pro adds up to one image, Ultimate adds enhanced intent
when supplied and up to four images. Restricted filters remain inactive with authored
intent intact. A policy change increments the filter version even when its state stays
active; execution consumes that policy without loading subscriptions. Persisted intent
incompatible with its resolved capability fails validation instead of losing instructions.

Query-only intent uses the existing lexical search. Enhanced intent uses hybrid retrieval
with its write-time base-query + description embedding; the description is never appended
as a mandatory all-words title query. Filters containing only deterministic constraints
use ordinary search without embedding or model work. Structured predicates, including
geography, availability, source/auction and pinned-FX prices, constrain retrieval regardless
of classifier scores. Current authoritative event checks fence obsolete projection evidence.

Each logical filter/window attempt scans at most 100 results, deduplicates listing identity
and skips existing matches, withdrawn listings and missing/stale sources before selecting
the first up to 50 eligible listings. Every selected semantic candidate receives its own
request; negative decisions never stop evaluation or refill slots. Bounded retries reuse
the same selected identities. This is bounded best-effort discovery: exhaustion, candidate
cap and scan cap are reported separately. Advancing a window does not prove catalogue
coverage; the unselected tail has no guaranteed later opportunity.

The evaluator includes complete base-query alternatives and permitted enhanced instructions,
keeps requirements/exclusions distinct from preferences and treats listing/image text as
untrusted evidence. Missing evidence proves neither satisfaction nor contradiction. Images
cannot establish authenticity, provenance, authorship or age without supporting evidence.
`should_show >= SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS / 10000` accepts an alert
(default 0.5, inclusive); the score is a decision signal, not a calibrated probability.
`hard_conflict` remains diagnostic-only. Text-only, one-image and four-image requests may
have different score behavior and require separate evaluation fixtures.

Image selection is primary-first, URL-deduplicated and policy-bounded. Free never fetches
images. Absent source media is normal; fetching/format/size failures degrade to the remaining
permitted evidence, including text-only, with requested/sent counts and omission reasons.
Exact prepared-image duplicates are omitted. The adapter enforces four images, per-image
and aggregate media/body limits and a conservative context bound before provider truncation.
A technical failure is never a negative semantic decision. Permanent candidate failures
are reported as terminal incomplete evaluations and permit bounded-window progress;
retryable failures leave progress unchanged even when accepted
matches commit. Only `CLASSIFIER_INVOCATION` events sum provider attempts and token costs;
`SAVED_SEARCH_MATCH_DECISION` events describe scores and decisions without duplicate costs.
Reports count selected identities separately from submitted classification requests
(including retries/preparation failures); actual provider attempts come only from the
invocation stream. Neither event logs private search text or image bytes.

A run excludes overlapping execution and uses projection lag and replay overlap. Image/model
work has bounded concurrency outside transactions; cancellation/timeouts and failed filters
remain incomplete task outcomes. The final short transaction rechecks filter version,
state/progress and current listing event before idempotent insertion. Matching pins persisted
FX at the window end and retains historical sale-observation valuations. Persisted match
INSERTs feed the existing CDC notification route independently of external notification
eligibility and monthly quotas.

The [periodic use case](../../src/search-filter-service/src/use_cases/run_periodic_search_filter_matching.rs)
owns executable matching semantics; [periodic recovery](../durable-worker-runbook.md#periodic-matching-recovery)
owns operator checks.

### Stripe subscription reconciliation

Subscription created, updated and deleted events request a fresh customer/subscription
read; their snapshots do not directly assign tiers. Stripe [does not guarantee event
ordering](https://docs.stripe.com/event-destinations/eventbridge#event-ordering), and
second-resolution event timestamps cannot establish a total order.

The Stripe event ID identifies a durable receipt. Reusing it with different canonical
content, customer or subscription identity fails closed. Receipts are retained for replay
safety. Identical completed deliveries skip provider I/O. A customer reconciliation
revision advances on every completed new event, including unchanged tiers. If another
reconciliation commits during a provider read, the older reader must retry from current
state. Provider failures, malformed selected events, unknown users and identity conflicts
remain failures for Lambda’s asynchronous retry policy. This route currently has no
configured asynchronous failure destination; exhausted retries require operator replay
from retained provider event evidence.

Current `active`, `trialing` and `past_due` subscriptions grant the highest configured
product tier across the customer's subscriptions. `past_due` retains access during
payment recovery; canceled, unpaid, incomplete, expired and paused subscriptions grant
none. Unknown statuses/products and incomplete responses fail closed. Subscription
pagination is explicit and the entire provider read has a bounded deadline. Customer
metadata may establish an unbound user's association but cannot replace another customer
or contradict an existing user's association.

Before running the updated consumer, apply the Stripe receipt migration and provide its
`STRIPE_API_KEY` configuration using the existing stage credential reference. The consumer
and billing API share that key: restricted permissions must preserve customer creation
and Checkout/billing-portal session creation, plus customer and subscription reads.
The key, product/price identifiers and event destination must use the same Stripe account
and test/live environment. No transaction stays open across Stripe I/O; a failed
application rolls back its receipt and revision together with business changes.

## Operations and validation

[Worker operations](../durable-worker-runbook.md) distinguish router archives, worker
DLQs, upstream provider failures and scheduled-task failures. Infrastructure declarations
and fixtures do not prove delivery. Observe real mapping states, lag, backlog and completed
outcomes before retiring a producer or enabling traffic.

Tests should cover strict CDC decoding, fanout, duplicate/reordered work, stale source
markers, uncertain commits, FIFO partial failure, notification finalization and deletion
fences/rebuilds. Use isolated real dependencies where needed; live probes require approval.

Effective geography appends schema-1 `ENRICHMENT_GEOGRAPHY_UPDATED` with exactly an empty
object payload only when compact derived values change. It routes to product projection,
with no watchlist/price noise or enrichment loop. Work continuations and crash recovery
are durable; see [listing geography recovery](../durable-worker-runbook.md#listing-geography-recovery).


### Shipping observation validity

Incremental shipping imports commit address-free shipping_validity_changes with their
assertion windows/ownership. OBSERVED is independent of management/terms/listing semantic
revisions; AUTHORITY_CHANGED follows existing source/configuration or grant lifecycle changes.
Subject identity is a profile, source default, listing or source; defaults and sources use the
source ID. The subject kind, ID and revision identify an immutable signal. The owning
[shipping contract](../shipping.md#incremental-typed-imports) defines application/replay and
expiry. B09 consumers must reread current state and tolerate duplicate/out-of-order signals;
no new router/projection worker is activated by this contract.
