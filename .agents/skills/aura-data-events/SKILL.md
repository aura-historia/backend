---
name: aura-data-events
description: "Audit aura-historia/backend PostgreSQL integrity, migrations, concurrency, CDC/SQS workers, replay receipts, projection fences and deletion/retention. Invoke manually for changed flows or an end-to-end failure review."
disable-model-invocation: true
---

# Data integrity and event reliability

Read the [shared policy](../aura-maintenance/references/policy.md),
[scope and report procedure](../aura-maintenance/references/runbook.md), current root
and applicable nested AGENTS.md, and the [document map](../aura-maintenance/references/repo-map.md).
Default `mode=audit execution=static scope=changed`; inherit explicit coordinator
parameters, or resolve scope using the runbook when invoked alone.
Read current storage, event-flow, worker-runbook, listing, consent and migration owners.

If a required local reference is missing, report `blocked`; do not invent a replacement.

## Trace committed intent to durable completion

Select a changed flow when a base exists, plus a risk-selected flow inside the declared focus. Draw a textual table of source,
authority, transaction, emitted intent, transport, consumer, external effect, completion
proof, retry identity, fence, terminal/retryable outcomes and recovery owner. Confirm all
links in code and infra; an event type or consumer function alone proves no wiring.

Review these failure windows with explicit expected behavior:
- before commit; committed but not published; partial or uncertain router fanout;
- duplicate/reordered delivery; unsupported/malformed event; missing authoritative source;
- claim acquired; lease expired while external I/O continues; effect accepted but response
  lost; finalization conflict; retry after deletion; exhausted deadline mid-batch;
- stale/equal projection revision, delete tombstone followed by old upsert, rebuild while
  live writers run, successful receipt surviving owner deletion and retention cleanup.

Check DB uniqueness/FK/check constraints, fallible codecs, null vs corruption semantics,
transaction-bound factories, affected-row checks, compare-and-swap/lock order, deletion
races, migrations on empty AND representative prior schemas, expand/contract compatibility,
CDC selection and row-image assumptions. Never edit an already-applied migration or run
migrations against a shared database as cleanup.

For SQS/Kinesis inspect batch response identifiers, missing IDs, complete setup failure,
Standard vs FIFO processing, unfinished FIFO suffix retention, timeout headroom, checkpoints,
poison-message custody, redrive and bounded drains. Transport message IDs are not business
idempotency keys. Trace caller completion predicates, not only shared transport helpers.
For provider execution inspect local claims/fencing and acceptance/finalization ambiguity.

For projections inspect authoritative rereads, source markers, query exclusion of tombstones,
version monotonicity, resurrection prevention and rebuild/reconciliation contracts. Don't
turn caches/search into the authority or convert unknown corruption into absence.
For cleanup check bounded work, logical expiry on use, unfinished progress, replay windows,
independent ordering/withdrawal/deletion fences and retained historical facts.

## Evidence and allowed changes

Create deterministic crash-window, duplicate/reordered and concurrent DB regression plans.
Test actual constraints in isolated Postgres where needed; mocks do not establish atomicity.
Report migration/wiring/deployed-state unknowns explicitly. Almost all behavioral fixes in
this lane need design approval; periodic repair may add a non-invasive regression only
when authorized and must not change durable semantics. Recommend operator observations,
restore drills or live replay separately—never perform them by default.

Before returning, perform the read-only [verification pass](../aura-verify/SKILL.md).
Use the [report template](../aura-maintenance/references/report-template.md). Report
severity separately from confidence, preserve prior unresolved findings, and state
`partial` or `blocked` when the declared coverage or required checks are incomplete.
