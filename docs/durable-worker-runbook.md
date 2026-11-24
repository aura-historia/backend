# Outbox, queue and worker operations

Use this runbook for custody and recovery, not as evidence that the host is processing work.
[Event flow](events/flow.md) owns routes and completion; the [outbox](../src/delivery-outbox/README.md)
and the [job queue](../src/platform-postgres-queue/README.md) own their mechanics;
[infrastructure](infra.md) owns the host and its deploys; [marketing consent](marketing-consent.md)
owns consent-specific recovery. Record the deployed commit, operator, checks and omissions. Keep
payloads, credentials and personal content out of evidence.

A deploy applies reviewed forward migrations and restarts the worker, which finishes its in-flight
jobs first. Queued jobs and custody outlive the deploy, so the deployed dispatcher and consumers
must still decode what earlier commits captured and enqueued, and a rollback must decode what the
newer commit wrote ([schema evolution](arch.md#128-schema-evolution)). An HTTP smoke test does not
prove queued-job compatibility or outbox health; `ops inspect` after the deploy does.

## Outbox source changes

A selected source is a capture trigger, its snapshot codec in the dispatcher and its route; the
three ship together, with the trigger in a migration and the codec and route in the same commit.

- **Adding a source** captures only inserts committed after its trigger exists. Rows that existed
  before are never captured: reconcile approved preexisting state through the owning consumer's
  contract, never by rewriting business rows to manufacture captures.
- **Changing a source's snapshot** is expand-and-contract: deploy a codec that accepts both the
  old and the new shape before the trigger writes the new one, and remove the old shape only once
  no pending snapshot or custody row has it.
- **Retiring a source or a destination scope** drops the trigger first, then lets the dispatcher
  drain the outbox and the consumers drain the scope, settles its custody, and only then removes
  the codec, route or consumer. A dispatcher that no longer knows a pending snapshot's source moves
  it to custody as `unsupported_source`; a job whose scope has no consumer waits forever.
- A rollback to a commit that does not know a newer source does the same: its snapshots move to
  custody, and the redeployed newer commit takes them back with `ops redrive delivery-outbox`.

## Failure custody and controlled redrive

| Failure location | What to inspect |
| --- | --- |
| Delivery outbox | Pending count and oldest age, and `delivery_outbox_failures` with its error classes, separate from job custody. See [delivery outbox recovery](#delivery-outbox-recovery) |
| Job queue | Per scope: depth, leased count, oldest waiting age and `job_queue_failures` (custody) with reasons and last error classes; `ops inspect` prints them |
| Provider ingress | Upstream delivery history as well as downstream command custody |
| Scheduled work | The `ops` command's exit status and JSON report from the host timer; a started timer is not completed work |

The [job queue](../src/platform-postgres-queue/README.md) keeps every job in PostgreSQL until
it is acknowledged or moved to custody; nothing expires and custody is never purged. A job
whose retry budget is spent (`RETRY_BUDGET_EXHAUSTED`) or that its consumer parked (`PARKED`)
moves unchanged into `job_queue_failures`, with its attempt count and a bounded
`last_error_class`. The class is the consumer's outcome label (for example
`resolution_failed`, `projection_unavailable`, `timed_out`, `panicked`, `lease_expired` or
`released`), never error text.

The [worker](../src/aura-historia-worker/README.md) acknowledges only confirmed complete
outcomes. Dependency, active-claim and uncertain-effect failures, timeouts and panics are
retried after the scope's retry delay. A FIFO consumer parks a job that can never succeed as
delivered on its first redelivery instead of retrying it for the whole budget; later jobs of its
group then run without it, so check what they did before redriving it. On shutdown a worker
stops claiming, lets in-flight jobs finish within its grace period and releases the rest, which
other workers claim at once.

Each worker or scheduled-job boundary records a failure in one event: the retained-job event,
or for workers without one, the event that names the failed work. `stage` names the failed step,
`dependency` the system it called and `error_category` a bounded lower-case cause class such as
`pool_timed_out`, `authentication` or `http_status`; `http_status`, `sqlstate` or a channel's
`provider_code` follow when the cause carries one. `dependency` is absent when the use case
refused the request itself and `unknown` when the service did not say which system failed.
`unclassified` means the cause had no safe classification; inspect the dependency. Use these
fields to choose a repair before a redrive: `invalid_persisted_state` and `document_invalid`
point to data to repair, not an outage. They never carry error messages, provider bodies or
payloads. Expected claim, lease or version contention carries no failure diagnostic; identity
deletion, geo resolution and ingestion record it at INFO, which production's `warn` level does not write until `LOG_LEVEL` is raised.

A process whose configuration or PostgreSQL pool is invalid fails at startup and logs the failed
stage; the worker also names the scope whose composition failed. Pools connect lazily, so a login
refused after a credential change appears in the first job's own diagnostic, such as `database`
with SQLSTATE `28P01`; fix the credential file rather than redriving.

For recovery:

1. Locate **all** copies and custody boundaries; preserve original domain IDs, source
   sequence, command fingerprint and FIFO grouping/order. A custody copy does not prove no
   other copy completed.
2. Inspect PostgreSQL receipts, revisions, leases and current eligibility with bounded,
   read-only queries. Use metadata/safe IDs, not exported raw records. A lost commit or
   provider reply is not proof of rollback or non-delivery.
3. Repair the cause and test isolated replay using the real fail-closed decoder. Obtain
   separate operator approval for a small ordered redrive (`ops redrive <scope> <job-id>...`,
   or `--all`); it restores each job's identity and position in its FIFO group. Observe
   completion and downstream effects before expanding it.
4. Preserve receipts and fences. Never fabricate success, reset an applied receipt,
   skip poison input, blindly resend historic mail, or delete custody to clear a backlog. Only an
   operator runs `ops redrive`; the runtime never redrives.

## Delivery outbox recovery

The [delivery outbox](../src/delivery-outbox/README.md) holds captured source inserts until
the dispatcher has enqueued their jobs. A growing pending count or oldest age means the
dispatcher is not running or its transactions fail; capture continues, and nothing is lost
while PostgreSQL keeps the rows. Restarting the dispatcher drains them in outbox order.

A snapshot in `delivery_outbox_failures` could not be decoded, routed or encoded. Its
`error_class` names the step: `unsupported_source` and `unsupported_snapshot_version` mean a
capture trigger and the dispatcher's codec disagree, `invalid_snapshot` and
`invalid_source_row` a shape the source contract does not allow, `invalid_job` a routed job its
wire format or the queue's group and dedup keys refuse, `invalid_identity` an ID or
key that fails its type, and `unsupported_event`, `unsupported_event_version` and
`invalid_event_payload` a ProductListing event the route does not accept. Read the row's
source and IDs, not its whole snapshot, into evidence.

Repair the cause first: deploy a codec or route that accepts the reviewed shape, or repair the
source row and let its writer capture a fresh insert. Resolve custody promptly: it holds compact IDs of the source rows, including user IDs of
search-filter matches and identity deletions, and ProductListing event payloads. Then redrive
selected rows with `ops redrive delivery-outbox <outbox-id>...`; redrive
restores each under its original outbox ID, ahead of every later snapshot still waiting, and
the dispatcher treats it like a fresh capture. Custody is never deleted to clear a backlog.

## ProductListing ingestion FIFO command recovery

Admission confirms custody, not execution. [Retry identity](product-listing.md#retry-identity)
requires the unchanged original ordered batch, actor, source and effective key; correct
known-invalid items separately with a new key. Consumer success requires atomic application
and its matching `APPLIED` receipt. Those receipts survive business-row deletion.

Deterministic business rejections are acknowledged without a receipt and logged as
`Ingestion command rejected without retry` with command identity and error code. They never
reach custody: have the producer resubmit
corrected input. Invalid commands, identity conflicts and invalid persisted state are logged
as `Ingestion command cannot apply as delivered` and parked in custody on their first
redelivery. Later commands of their group then run, so a dependent update or withdraw may
have been rejected and need resubmission once the cause is repaired. Technical failures reach
custody only after the retry budget and delay only later commands of their own group.
A consumer rolled back behind a producer that already sends a newer command version parks
those commands on their next delivery; redrive them once a compatible consumer runs.

A completed old command replays as a no-op; an old **unfinished** command can still apply
against newer state or fail guards. Before redrive, inspect source-order barriers, current
partnership/source grants and raw/provider/command receipts. FIFO does not order synchronous
writes, normalization or manually reintroduced work. Pause producers or consumption only with
an explicit backlog plan; never delete jobs to stop traffic.

Keep acceptance fixtures isolated and record admissions, committed receipts and
normalization separately.

## Shopify forwarding recovery

Shopify posts `product_listings/*` webhooks to the API, which answers `200` only after
confirmed command-FIFO admission. A lost reply may already have queued the command; Shopify's
retry carries the same `X-Shopify-Webhook-Id` and therefore the same command identity. The
FIFO acknowledges application independently, and normalization/projection complete later.

The API does not start without `SHOPIFY_APP_CLIENT_SECRET`, the app's client secret. While the
app's secret rotates, set the old one as `SHOPIFY_APP_PREVIOUS_CLIENT_SECRET` until Shopify
signs only with the new one. A `401` burst means neither matches the app's secret.

On failure, inspect the app's webhook delivery metrics in the Shopify Partner Dashboard, the
API's `post_shopify_webhook` events and the ingestion scope in `ops inspect`. Shopify stops retrying a
delivery after about four hours and may remove a subscription that keeps failing, so after a
longer outage confirm the app's subscriptions still point at the HTTPS endpoint and re-import
the affected shops through the canonical asynchronous upsert and withdrawal endpoints.

## WooCommerce webhook queue recovery

Use isolated exact-byte signed probes with valid partner/source authorization. Authorized
ignored inputs produce no command/receipt; a mapped `204` means confirmed admission only.
Deterministic source-order rejections are logged instead of retried; receipt
identity conflicts and technical capture failures can still reach the ingestion scope's custody
without a provider retry.

After an uncertain HTTP reply, preserve signed bytes, source, topic, caller and delivery ID.
Without a provider delivery ID, the original command identity may not be recoverable. Inspect
the queue and database state before redrive. Fence only this producer for rollback, not shared
consumption, and do not run concurrent immediate capture of the same observations.

## Projection fences and rebuild

ProductListing withdrawals use content-free, externally versioned
`projectionDeleted: true` tombstones. Never TTL or physically delete tombstones: remote physical-delete version memory expires.

Rebuild into an isolated index generation: fence old writers, reconstruct authoritative
facts, catch up commits, verify IDs/source versions/deletion visibility, then switch the
approved reader/alias target.

`reproject-all` reconstructs the facts: it enqueues one projector job per existing listing,
withdrawn ones included, naming the listing's current event, so the projector writes current
state or the versioned tombstone. A separate generation `<g>` receives its jobs in the scope
`product-listing-opensearch-<g>`, consumed by a projector bound to the index
`product-listings-<g>`; the live index and its scope are untouched. Nothing creates that index:
before the run, create it through the loopback administration endpoint
([OpenSearch operations](opensearch-stage.md)) from
[`product_listings.json`](../opensearch/mappings/product_listings.json) with
`index.number_of_replicas` 0, as the single node's live index has, and start a worker with
`AURA_HISTORIA_WORKER_PROJECTION_GENERATIONS=<g>`. The projector role may write every
`product-listings-*` index ([role](../opensearch/roles/product-projector.json)). Readers query the
index `product-listings` by name, not an alias, so a generation cannot yet replace it; target the
live scope to rebuild search after the index was lost. Each page commits on its own and the run reports
the last listing, so an interrupted run resumes after it. Catch up by rerunning once old
writers are fenced: listings whose job is still waiting are skipped, every other listing is
enqueued again, and the projector completes the ones already current as stale; an
undiscoverable listing rewrites its equal tombstone. Withdrawn listings retain authoritative source state.
Index reset, the queue and absent history cannot restore missing authoritative facts.
[OpenSearch operations](opensearch-stage.md) own host/security and asset changes.

### Narrowed projection rule

A release that narrows which listings are
[discoverable](product-listing.md#application-api-and-search-contracts) leaves documents the
earlier rule indexed at their current version. Once no projector of the earlier release runs,
run `ops reproject-all --live`: the projector replaces each such document with an equal-version
tombstone and completes current documents as stale. Only narrowing works in place: an
equal-version upsert never replaces a tombstone, so a rollback or a later widening of the rule
needs a [new index generation](#projection-fences-and-rebuild). Search keeps serving throughout; until the
run completes, an affected listing can still appear and is valued at current FX. Verify that the
number of live documents equals the number of discoverable listings:

```sql
SELECT count(*) FROM product_listings
WHERE lifecycle = 'ACTIVE'
  AND (availability IS NULL OR availability NOT IN ('SOLD_OUT', 'OUT_OF_STOCK', 'UNAVAILABLE'));
```

```json
GET product-listings/_count
{"query": {"bool": {"must_not": [{"term": {"projectionDeleted": true}}]}}}
```

Listings changing during the comparison and outstanding projector jobs account for small
differences; compare again once the scope's queue is empty.

## Title translation recovery

[Title translation](product-listing.md#title-translation) owns the contract. A
`product-translation` job only records work, so its custody means the work was never recorded:
redrive it once PostgreSQL is healthy. Every worker consuming that scope also runs the
translation rounds with `ANTHROPIC_API_KEY`; work waits in
`product_listing_title_translation_work` and batches in
`product_listing_title_translation_batches`, neither of which is ever purged except by settlement.

- Each round logs `product.translation.round.completed` with its counts, or
  `product.translation.round.failed` with the failed stage and safe categories; the next round
  continues from the durable state. A batch that cannot be collected logs
  `product.translation.batch_failed` and is retried next round without holding back other
  batches or new work; a title whose guarded write fails logs `product.translation.write_failed`
  and spends an attempt. `LLM_BATCH` events log each batch's ID, size, outcome counts
  and token usage when it is submitted and when its results are read.
- Pending work older than a day, or batches submitted more than 24 hours ago, mean the rounds
  are not running or Anthropic is failing: check the worker logs and the batch in the Anthropic
  Console by its ID.
- A batch whose submission was not recorded within 10 minutes is closed and its work returns to
  the queue. If Anthropic accepted it anyway, it runs unread and is billed once more.
- Results stay downloadable for 29 days. A batch whose results the provider no longer serves
  fails each of its titles once, and they are submitted again.
- Parked work (`parked IS NOT NULL`) spent three attempts or was refused by the fallback model.
  It waits for a title change; a description-only change carries it parked. To retry it after
  fixing a cause, set `parked` to `NULL`, `attempts` to `0` and `mode` to `'GROUPED'` for the
  selected listings; never delete work to silence it.

## Product embedding recovery

[Embeddings](product-listing.md#embeddings) owns the contract. A `product-embedding` job only
records work, so its custody means the work was never recorded: redrive it once PostgreSQL is
healthy. Every worker consuming that scope also runs the embedding rounds with
`VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION` (`global`, `us` or `eu`, where
`gemini-embedding-2` batch prediction runs) and `VERTEX_AI_EMBEDDING_BATCH_BUCKET`; work waits in
`product_listing_embedding_work` and batches in `product_listing_embedding_batches`, neither of
which is ever purged except by settlement.

- Each round logs `product.embedding.round.completed` with its counts, or
  `product.embedding.round.failed` with the failed stage and safe categories; the next round
  continues from the durable state. Each job logs `product.embedding.batch.submitted` with its ID
  and size, and `product.embedding.batch.collected` with its size, answered requests, outcome
  counts and `turnaround_seconds` (from submission to the round that collected it). A batch that
  cannot be collected logs `product.embedding.batch_failed` and is retried next round without
  holding back other batches or new work; a vector whose guarded write fails logs
  `product.embedding.write_failed` and spends an attempt. Work whose persisted document is
  invalid logs `product.embedding.work_invalid` and is parked alone. The worker keeps these
  records at `info` whatever `LOG_LEVEL` says.
- The `…-host-embedding-backlog-age` alarm fires when work that is neither committed nor parked
  is 36 hours old: the rounds are not running, submissions fail, or jobs keep failing as a whole
  (a missing bucket permission or quota returns their work unspent, so it is resubmitted every
  round). Check the worker logs and the job in the Vertex AI console by its ID.
- A batch whose submission was not recorded within two hours is closed and its work returns to
  the queue. If Vertex AI accepted the job anyway, it runs unread and is billed once more.
- The staging bucket's lifecycle rule deletes objects after 7 days, longer than a job may queue
  and run (72 hours) plus a round. A job whose output can no longer be read returns its work
  unspent, and it is submitted again.
- `ops embedding-sweep` runs at 14:00 UTC (`aura-prod-embedding-sweep`) and logs
  `product.embedding.sweep.completed`. It fails when Vertex AI answered none of the work it swept
  or a result could not be settled; that work stays where it was for the rounds. It stops after
  45 minutes and then logs `product.embedding.sweep.out_of_time`. A manual run is safe at any
  time; it pays the synchronous price, also for work in running batches.
- Parked work (`parked IS NOT NULL`) spent three attempts or cannot be embedded. It waits for a
  content change. To retry it after fixing a cause, set `parked` to `NULL` and `attempts` to `0`
  for the selected listings; never delete work to silence it.

## Listing shipping recovery

[Shipping](shipping.md#effective-shipping-discovery-projection) owns the conservative
index subset, observation eligibility and distinction between current listing
responses and delayed indexed discovery. The worker accepts only compact work ID/generation
wake-ups, rereads canonical PostgreSQL and commits a bounded page with its checkpoint.
It never calls providers or writes OpenSearch. Stale/superseded work cannot commit a page;
active claims, malformed input and dependency failures are retried and then kept in custody.
Shipping processing relies on outbox delivery, retries and controlled redrive. There
is no scheduled shipping expiry or pending-work recovery scan.

Original shipping journals, policies, observations, dependency rows and checkpoints stay
outside the outbox. The dispatcher publishes the address-free wake-up pair only; snapshots it
cannot route stay in outbox custody.

For bounded rebuilds, enqueue selected source UUIDs in reviewed keyset pages with `SELECT enqueue_listing_shipping('SOURCE', $1)` in the
operational transaction. Preserve work generations, live leases, reverse references,
canonical revisions and deletion fences; never manufacture listing changes to drive a
rebuild. Source jobs and independent listing jobs cover overlapping creation/assignment
changes. No provider access or copied addresses are needed. Unknown schema/invalid storage
requires correction through its owning contract, not replacement with an empty policy.

An empty replacement index also needs replay of the existing complete product-projection
journal after derived state catches up: equal shipping summaries intentionally emit no
new events. Follow the isolated index-generation procedure above, including old-writer
fencing and retained tombstones, before switching readers. Verify safe identities/versions,
country-service-price nesting and deletion visibility. Runtime acknowledgment means an
accepted or externally fenced write, not search visibility.

Monitor oldest pending work, expired leases, changed/scanned page counts and shipping
and product-projector scope age, depth and custody. Repair both pipeline stages; queries remain available during catch-up. Use only identities,
revision/generation, policy and failure categories in operator evidence. Never
export terms, addresses, producer ownership keys, buyer data or provider payloads.

## Periodic matching recovery

[Event flow](events/flow.md#periodic-saved-filter-matching) owns matching semantics;
[infrastructure](infra.md#scheduled-work) owns the schedule. Before enabling
runs, verify PostgreSQL access, ProductListing search with reader credentials,
complete persisted FX and classifier configuration. Filter candidates and progress come
from PostgreSQL. Every filter with text intent retrieves with its stored embedding; one
without a valid embedding fails as invalid persisted state. The update use case repairs missing
semantic embeddings outside its transaction and rechecks current intent and tier under
lock. Preserve authored intent and do not generate embeddings during matching.

In singleton mode, concurrency eight gives up to seven request waves for 50 candidates;
image fetching adds bounded preparation time. The two-hour run deadline also bounds expanded
Free/Pro coverage: monitor selected/attempted counts, failed filters and timeout outcomes for
saturation. Candidate/scan caps are successful bounded discovery, not exhaustive coverage.

Inspect the timer's unit result and the command's completion report separately; a started
unit is not a completed run. Failed filters, cancellation or timeout are incomplete work even if
other matches committed; overlapping-run exclusion is not evidence that a run advanced
progress. Verify per-filter progress and persisted matches with bounded read-only checks.
Repair the dependency and use the next scheduled run or an approved `ops periodic-match` run;
preserve checkpoints and match identities instead of resetting progress or deleting
matches to force a replay. The replay overlap tolerates bounded projection lag, not an
unlimited outage: review uncovered windows after prolonged search/projection failures.

Shipping and geography filters use the same automatic `updated` interval as every other
filter. An older listing is reconsidered only after an effective shipping, geography or
profile/default change advances its freshness; FX changes alone never do. Candidate/scan
limits remain bounded, so inspect cap outcomes for broad shipping filters. Recover shipping projection custody before investigating missing indexed offers;
current response terms may already reflect a change. Never generate a canonical price or
availability event to force discovery, or reset saved destination snapshots after an
address/preference change. See [shipping search](shipping.md#shipping-search-and-saved-destinations).

Saved-filter writes, previews and each run's translation backfill translate semantic intent
with the listing-title translator's Anthropic key (`ANTHROPIC_API_KEY`). An outage never fails a
write or a run: affected filters match their text as written (`intentSource=ORIGINAL`) and the
next run translates them. Each run attempts at most 500 translations, never-attempted filters
first and failed ones after them; follow
the `search_filter.intent_translation_backfill.completed` counts and `SEARCH_INTENT_TRANSLATION`
outcomes. A run that resumes an unfinished window keeps its frozen filters, so translations made
before it apply from the next window. Logs carry only outcomes and categories, never intent text.
An invalid stored translation fails only that filter's matching
(`search_filter.periodic_match.invalid_intent_translation`); the owner can still edit it, the
next write clears it, and setting `intent_translation` to NULL lets the next run retranslate it.

The API's search-filter previews use the same Workers AI account. A classifier outage or rate
limit also fails previews with `503 SEARCH_FILTER_TEMPORARILY_UNAVAILABLE`, logged by the API as
`search_filter.preview.failed`; preview traffic counts against the same provider limits as runs.

Match INSERTs normally wake generation through the outbox and healthy notification workers;
[digest recovery](#saved-search-digest-recovery) owns unfinished selection
and finalization. Verify the outbox and notification scopes independently. Rerunning matching
is not a substitute for recovering retained notification jobs and must not blindly resend
historical mail.

### Saved-search batching quality gate

`PERIODIC_MATCH_BATCH_CANDIDATES` (integer 1–50) configures the periodic matcher.
Search-filter previews in the API always send single candidates; they share only the model,
threshold and Workers AI token with the matcher.
The default, 1, is the singleton fallback switch. Larger values enable budgeted shared
text requests for one filter, with one independently keyed `should_show` question per
listing. Fifty candidates can share a request only when every provider question, text,
context and payload budget permits it; actual requests may be smaller. Image-bearing
batching is disabled. Keep 1 until a human-reviewed multilingual antiques/art benchmark
establishes acceptable precision/recall, decision and score disagreement, neighbor interference,
response integrity, latency, token consumption, actual cost savings and recovery rates
under the intended acceptance threshold. Transport tests and singleton outputs do not
establish semantic quality or cost savings.

Attach the reviewed dataset revision, benchmark report, reconciled costs and accepted
quality thresholds for the configured model to the release/issue before increasing the
deployment setting. Every image-containing request remains a singleton regardless
of this setting.

Groups may carry an `intent_translation` (`base_queries` and nullable `enhanced_description`)
to classify the English pivot instead of the authored intent; compare such runs against the
same groups without it on the same day and model. Case `title` and `description` are the
evidence the classifier receives, so supply the English title and the original description.
The [multilingual pivot fixtures](../src/search-filter-service/benchmarks/clef-should-show/multilingual-pivot-fixtures.json)
cover Latin and non-Latin scripts, explicit preferences and sparse listings with agent-authored
labels.

The opt-in [benchmark runner](../src/search-filter-service/src/product_match_evaluator/batching/benchmark.rs)
first compares the old two-question schema (A) with the one-question singleton schema
(B), then compares B with shared requests and reversed neighboring candidates. All modes
use identical human-labeled cases, intents, permitted evidence and model at the inclusive
0.5 threshold. Removing a jointly evaluated question can shift `should_show` scores even
when its wording stays fixed. Review every flip, especially A-correct/B-wrong cases,
before adopting the schema change; investigate material regressions before changing
instructions or threshold. Start with the existing hard cases and add fresh human-labeled
cases; repeated prompts are repeated measurements of the same examples.
Normal tests skip provider access. Use a dedicated test account/token,
never deployed service credentials. Supply the benchmark-only credential variables
`AURA_BENCH_CLOUDFLARE_ACCOUNT_ID` and `AURA_BENCH_CLOUDFLARE_API_TOKEN` through the
test environment without printing them. Run:

```sh
AURA_BENCH_ALLOW_PROVIDER=1 \
AURA_BENCH_FIXTURES=/path/to/reviewed-fixtures.json \
AURA_BENCH_REPORT=/path/to/benchmark-report.json \
cargo test --locked -p search-filter-service --lib -- --ignored --exact product_match_evaluator::batching::benchmark::saved_search_batch_quality_benchmark
```

The fixture manifest contains `label_provenance: "human-reviewed"`, `reviewed_by`,
`dataset_revision`, `should_show_threshold: 0.5`, `batch_candidates` (1–50), optional
`classifier_model` (default `clef`, must match the deployment), the verified current
`input_usd_per_million_tokens`, and `groups`. Each group is one filter selection with
canonical ISO `language`, `base_queries`, optional `enhanced_description`, write-resolved
`evidence_policy` and 1–50 `cases`. Each case has a unique opaque `id`, nullable
`title`/`description`, a `human_match` boolean, coverage `tags` and optional ordered
`image_urls` (zero, one or up to four, within the persisted evidence policy). Use stable
reviewed image fixtures; examine per-case sent-image counts and any evidence disagreement
between modes before interpreting score changes. A response that omits reviewed images
remains unresolved for the quality comparison. Supply text in its persisted normalized
form; the runner rejects any constructor normalization or truncation
that would change reviewed evidence. Set `batch_candidates: 1` for the initial singleton
A/B comparison using the existing hard cases, with human positives and negatives. This
mode reports only the schema comparison and cannot establish the batching quality gate.
Exploratory synthetic fixtures may instead declare `label_provenance: "agent-authored"`,
`authored_by`, and per-case `expected_match` labels. Their reports remain explicitly
ineligible for the release quality gate until a human reviews the evidence and labels.
Reports use `expected_match` for the comparison label and retain its provenance.
The [self-contained real-object challenge set](../src/search-filter-service/benchmarks/real-world-v1/README.md)
stores its catalogue evidence and images locally and includes an opt-in deployed-matcher
driver. Its agent-authored labels have the same human-review requirement; controlled
image delivery and a purposive sample do not establish merchant-host reliability or
a production-wide error rate.
For `batch_candidates` above 1, the runner requires at least three languages, twenty
positives and twenty negatives, 2–50 mixed-label cases in each group, and the coverage
tags defined in its fixture validator. Review the actual evidence
for easy/hard positives/negatives, absent descriptions/images, long instructions,
contradictory/injected neighbors and both antiques and art, with ordinary/enhanced searches,
explicit must-have conflicts, preference mismatches, ambiguous positives and lexical near
misses. Include text-only, one-image and four-image evidence where present; tags alone
are not review.

Reports contain opaque IDs, human labels, `should_show` scores, threshold decisions and
every decision flip, plus precision/recall, false positives/negatives and aggregate metrics,
including actual shared provider requests, without instructions or listing text. Fixtures
must produce at least one shared request within provider budgets when benchmarking batching.
Unresolved work is counted separately
and contributes no achieved recall. Invalid
envelopes and unresolved questions expose mapping/omission failures; plausible misassociated
scores require human review. Token-priced cost is reported only when every invocation
has usage; it is not invoice reconciliation. Optionally supply `AURA_BENCH_RECONCILED_COSTS`
with a JSON file containing nonnegative `baseline_two_questions`, `singleton`, `batched`,
`batched_reversed` actual USD costs (only the first two in singleton A/B mode) and a
non-sensitive `reference` to billing evidence.
Failed-attempt charges must be included. Missing or malformed answers remain unresolved; the adapter may classify
both as invalid responses, so the report cannot always distinguish their wire causes.
Provider variability can affect schema and reverse-order disagreement, so repeat
the controlled benchmark when reviewing interference. Approval remains a human release
decision; the runner never turns batching on.

## Notification recovery limits

Delivery intent/lease state lives in PostgreSQL. Commit the claim before SES; retry
finalization with the **same lease and result**, not another send. Active claims and lost
leases are not completion. A send/finalize crash can duplicate mail after lease expiry.
Inspect safe delivery IDs, database completion and restricted provider evidence before
approved recovery. Never interpret timeout as definitely unsent or automatically resend
historic customer mail. An SES account-level or configuration refusal (sending paused,
account suspended, unverified MAIL FROM domain, missing configuration resource) is retried
with error code `SES_SENDING_UNAVAILABLE` and leaves the delivery `PENDING`; once its jobs
are in custody, fix the account or configuration, then redrive them.

External email deliveries ([event flow](events/flow.md#external-email-delivery)) follow
the same rules. Their message rows hold the recipient address and payload: triage by
delivery ID, source type and error code, and never export or log addresses or payloads.
A `delivery_state_invalid` job in custody with source type `external` points to a
corrupt or missing message; fix the persisted state through a reviewed change before
redrive. Exception: a rollback past a release that added a delivery source or template kind
leaves the older worker unable to handle those jobs, and it moves them to custody as
`delivery_state_invalid`, possibly without a source type. They need no state change: redrive
them once a worker that knows them runs again.

## Saved-search digest recovery

Normal filter completion and notification selection finalize ready digests
immediately; delivery then follows the existing asynchronous outbox path. The hourly
cleanup is the fallback for interrupted work, performing bounded
notification-selection recovery and digest finalization using PostgreSQL,
independently of outbox wake-ups. Failed selections are deferred for 15 minutes and
reordered behind untouched work so they cannot starve the queue. Monitor cleanup
run failures and backlog capacity: a healthy bounded run does not
prove that all pending groups completed. A run retains its identity on retry until
its fixed 24-hour recovery deadline. At expiry successful persisted matches can
finalize even with unfinished filters; subsequent matching keeps ordinary
progress/replay-overlap semantics. Selected late arrivals are retained for the next
digest, including when that next run discovers no fresh matches.

Inspect `saved_search_digest_status` with bounded, read-only queries for safe run/user
and digest/delivery identities, pending filters/selections, group age, full eligible
membership, distinct listings and delivery state/attempts/errors. Structured
`saved_search.digest.*` events report finalization, rendering counts and current
eligibility suppression; generator outcomes expose quota and other selection
suppression. These are log events, not provisioned metrics or alarms.
Investigate groups approaching the deadline, old unassigned members, stalled
selection, delivery scopes and their custody, and expired processing leases. Do not export snapshot
payloads, authored search instructions or private email addresses for diagnostics.

Repair failed dependencies, then use the next cleanup or matcher run or an
approved `ops backend-cleanup` run. Preserve run keys, match keys, membership,
finalized groups and delivery leases/receipts. Never reset a finalized group or clear
members to force another email. A delivery remains pending/processing/failed until
the established delivery handler confirms its outcome; outbox capture and job
publication are not provider acceptance. Reconcile ambiguous SES sends under
[notification recovery](#notification-recovery-limits) before approved replay.

## Geographic resolution recovery

[Geography](geography.md#durable-resolution-and-party-integration-2002) owns acceptance,
privacy and retention. Geocoding is [always on](geography.md#provider-terms-and-retention-gate):
there is no provider-control procedure and no database access is needed to start geocoding.
Only the automatic circuit breaker pauses calls, for five minutes after three consecutive
provider-health failures. Stopping paid calls or withdrawing retention rights requires a
reviewed release that removes the calls and purges retained evidence.

Keep private inputs, bindings, work checkpoints and evidence outside the outbox. The
administrator resolution `request` operation establishes work for preexisting eligible
PartyLocations; the outbox captures only new inserts and never backfills them. Review `AMBIGUOUS` locations and retry
`BLOCKED` or `FAILED` ones through the administrator review queue (see `docs/swagger.yaml`),
never through direct SQL.

The hourly backend cleanup purges orphaned evidence and requeues bounded runnable work
whose provider circuit is closed. Fresh wake-ups require a healthy outbox and queue. Recovery preserves
attempts, pending age and backoff; live leases keep their claims. Additional bounded cleanup
runs follow ordinary operational approval when backlog requires faster recovery or
purging. Readers suppress ineligible evidence immediately;
listing search accepts projection lag while removal propagates.

Binaries from before [address suggestions](geography.md#address-suggestions) reject
`SELECTED_PLACE_V1` inputs as invalid persisted state. During the deploy that introduces them,
an older worker briefly fails those wake-ups and the hourly recovery until the new worker is
live; both then recover on redelivery. Once picked places exist, do not roll the API or the
resolution worker back past that release: an older recovery sweep stops for every resolution
and older reads of an affected address fail. Fix forward, or first ship a reviewed release that
detaches selected-place bindings.

Likewise, once a resolution has been accepted although its submitted subdivision was not derived,
older binaries re-validate that evidence as invalid persisted state. Do not roll the API or
the resolution worker back past the release that relaxed the subdivision rule.

Inspect only safe metadata: binding identity, generation, lease, attempts, failure category,
deadline and circuit state. Monitor resolution reuse/outcomes, oldest pending age, maintenance
counts, scope age, depth and custody, and cleanup failures. Structured log fields are observations, not
provisioned metric filters; BLOCKED/FAILED work is acknowledged rather than retained, so inspect
the recorded `geo_resolution_outcome` and resolution state for it. Never include private input
or provider payloads in logs, operator evidence or replay messages.

Transport custody follows what a redelivery could still achieve. The worker acknowledges a
wake-up whose generation is settled: resolved, `BLOCKED`, `FAILED`, out of attempts, erased,
unbound or superseded. A redelivery of settled work
makes no provider call. It retains a
wake-up whose lease another claim holds or whose retry backoff is running, and a transient
provider failure with its diagnostic. Provider-gated wake-ups (provider circuit open) are
retained with reason `provider_gated`; growing custody of the scope is then the signal of a
stalled resolution backlog. Cleanup recovery republishes their PostgreSQL work regardless.

An active claim is incomplete. A paid-call/commit crash may repeat charging after lease
expiry; attempts are not refunded. Business Retry/selection requires current location
CAS and resolution generation. Unchanged completed retries reuse evidence; forced resolution
invalidates derivatives/selections and cannot bypass the circuit. Private addresses expose
ordinary replacement/read APIs; corrected provider configuration or authentication allows
bounded background recovery, while exhausted or invalid inputs require cause correction and changed input or
approved operational recovery. Cleanup requeues only private browsing blocks: business
resolutions blocked as `STORAGE_NOT_PERMITTED` stay blocked after a retention-rule or
reference-release correction until protected Retry, changed input, or approved operational
recovery that reproduces Retry's generation fence, invalidation and wake-up. Each recovery is
a paid call and blocks again when no candidate is retainable.

Redrive a small approved custody sample with original identities/generations: superseded and
terminal work completes without a provider call, completed work reuses evidence, active leases
defer and malformed jobs retain custody. Provider-gated copies need no redrive: once the circuit
closes, cleanup republishes their work and the old copies complete as stale. Confirm
completion and eligibility before expanding redrive. Never invent a new generation in a
transport payload. Runbook checks do not establish deployed readiness.

## Listing geography recovery

The worker consumes compact work ID/generation wake-ups and commits bounded pages plus
checkpoints. Continuations retain durable custody; newer invalidation fences claims and
restarts traversal. The hourly cleanup also drains a page to recover missed wake-ups. Neither
path calls a geocoder or writes OpenSearch directly.

For rebuilding, fence old projection writers and follow the existing isolated OpenSearch
generation procedure. Enqueue source UUIDs in bounded keyset pages with
`SELECT enqueue_listing_geography('SOURCE', $1)` in the operational transaction. Workers reread
current permitted authority. After draining derived state, replay the product projection
journal and validate the replacement index before activation: source enqueue alone cannot
repopulate a missing index when derived values are unchanged. Preserve checkpoints,
projection versions and deletion tombstones. Repeat source traversal if source creation
overlaps it; listing journals independently cover concurrent listings.

Monitor oldest pending work, expired claims, projection queue lag and
backlog age, failures and custody, version/fence correctness and rebuildability. Product projector
acknowledgment means OpenSearch accepted the write or external versioning fenced it as stale.
Search visibility follows the normal refresh cycle; no persisted acknowledgment claims
search visibility. Queries continue during catch-up; repair/replay stalled geography and product
projectors to propagate retirement, applicability and evidence removal.

## User identity deletion recovery

The `user-identity-deletion` scope uses the shared custody and redrive procedure. Fix failures before redrive; preserve the deleted user ID, checkpointed
provider target and claim/completion fences. An active claim remains unfinished until
its lease expires, while completed redelivery performs no provider I/O. A lost provider
response is reconciled using the saved target on retry. Never recreate the business
account to make a retained Cognito profile usable, or replace its subject from an email
match. Missing initial target evidence requires operator reconciliation under
[account lifecycle](account-lifecycle.md#operations-and-recovery). Verify provider removal
and retained completion before closing recovery; API authentication rejection alone is
not evidence that the Cognito profile was removed.
