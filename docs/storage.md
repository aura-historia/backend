# Storage Design and Guardrails

This guide owns durable persistence rules, not a table or port inventory. [Architecture](arch.md) owns layer boundaries; [`migrations/`](../migrations/), adapter code and tests own exact schemas/codecs. Domain and event details belong to their [specialized contracts](README.md).

## Authority and atomicity

- PostgreSQL owns business state and durable execution intent unless a bounded context explicitly documents another authority. Projections and caches are rebuildable, never substitute write models.
- Service-owned transactions commit authoritative changes, required events and local delivery intent together. No database transaction spans provider I/O or claims atomicity with another system.
- External execution must tolerate possible acceptance followed by lost finalization: fence claims/completion, preserve idempotency evidence and define recovery. Queue admission and diagnostic logs are not committed business completion.
- Keep operational receipts, secrets and raw provider evidence out of CDC, projections and public history unless a separately designed contract requires them.

## Integrity and concurrency

- Constrain identity, uniqueness, references and valid state in storage where practical. Persist declared configuration with its owning state; missing, orphaned or invalid configuration fails closed.
- Decode persisted values explicitly and fallibly. Corruption is an operation error, not absence, normalization or permission to reconstruct defaults. Canonical enum and [object-ID](object-ids.md) encodings remain stable.
- Invariant-critical reads and writes share the appropriate transaction. Racing changes need optimistic version checks or a documented lock order; conflicts are not not-found. Deletion checks must prevent concurrent creation of blocking references.
- Index actual identity, ordering and pagination needs; verify query plans and deterministic tie-breaking. Avoid indexing unbounded evidence merely for convenience.

## Retention, deletion and expiry

Geographic provider evidence has purpose-specific retention and cache-isolation rights;
single-use or private evidence MUST NOT silently become reusable dealer data. The
[geography contract](geography.md#provider-terms-and-retention-gate) owns these rules.

- Enforce logical expiry when credentials, proofs or receipts are used, independently of physical cleanup. Cleanup is bounded and recoverable; unfinished work is not successful housekeeping.
- Retention windows must cover the documented replay/recovery horizon. Cleanup or owner deletion MUST NOT erase an independently required ordering, withdrawal, deletion or successful-command fence.
- Domain withdrawal, physical deletion and personal-data erasure have different contracts. Define reference, history and external-effect consequences before deletion; retained facts must not depend on accidental cascades.
- Store hashes rather than reusable bearer secrets. Where short-lived raw exchange material is unavoidable, restrict it to that exchange and exclude it from logs, CDC and projections. Consume one-time credentials atomically, roll back failed exchanges and revoke related credentials on owner deletion.

## Users and email-marketing consent

Purpose/address consent, double opt-in, synchronization, provider webhooks, evidence and recovery are owned by [marketing consent](marketing-consent.md). Consent decisions remain authoritative locally; provider membership is separate external state. Do not duplicate that workflow contract here.

## ProductListing events and revisions

Aggregate concurrency, projection ordering and derived-content source revisions are distinct fences. The immutable semantic journal is not a generic outbox; raw capture and normalization evidence are not canonical domain events. [ProductListing](product-listing.md#persistence-contract) owns keys, revision semantics and source contracts; [event flow](events/flow.md) owns selected routes.

Provider-observation receipts enforce their logical replay window on use. Expiry/cleanup must not alter immutable raw revisions or erase independent source-order fences; delayed cleanup cannot extend deduplication guarantees.

## ProductListing ingestion command receipts

Successful execution and its receipt commit together. Matching commands replay without effects; conflicting verified semantics fail closed. Receipts are not admission or rejection history, have no TTL and survive listing/source deletion so an executed command cannot become executable again. They remain outside CDC and public history; see [ingestion submission](product-listing.md#asynchronous-ingestion-submission).

## Historical facts and valuations

- Notification snapshots survive deletion of their referenced listing; separate delivery intent commits with creation. Historical eligibility uses immutable event time and the applicable active/notification interval, not only current watch state. Quota reconciliation and watch changes serialize against authoritative lifecycle/quota state.
- PostgreSQL owns complete validated immutable FX snapshots. Valuations use persisted snapshots, not live provider reads; sale history pins its exact observation basis. Search continuation preserves its initial valuation basis. Missing/partial snapshots fail instead of inventing rates; FX capture alone does not rewrite historical matches or trigger unrelated notifications.

## Evolution and operations

Schema and CDC selection changes require compatibility, migration and activation review. A migration or CDK declaration is not proof that a route or cleanup job runs. [Event flow](events/flow.md) owns propagation contracts, the [worker runbook](durable-worker-runbook.md) owns custody/recovery, and [infrastructure](infra.md) owns deployment operations.
