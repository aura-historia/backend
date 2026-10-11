---
name: aura-operations
description: "Audit aura-historia/backend operational readiness from source: performance/cost bounds, observability, CI blind spots, deployment and migration ordering, recovery and configuration drift. No cloud changes or live probes by default."
disable-model-invocation: true
---

# Operational reliability, performance and cost

Read the [shared policy](../aura-maintenance/references/policy.md),
[scope and report procedure](../aura-maintenance/references/runbook.md), current root
and applicable nested AGENTS.md, and the [document map](../aura-maintenance/references/repo-map.md).
Default `mode=audit execution=static scope=changed`; inherit explicit coordinator
parameters, or resolve scope using the runbook when invoked alone.
Read current infrastructure, front-door, event and worker runbooks plus actual CDK/CI code.
Separate static findings from operator checks that require approved access to deployed state.

If a required local reference is missing, report `blocked`; do not invent a replacement.

## Source review

Trace deadline budgets across Lambda setup, DB acquisition/transactions and provider calls;
check cancellation ambiguity, retries with bounded backoff, concurrency caps, queue visibility
and failure custody. Inspect resource bounds for pagination, fanout, full-table scans,
N+1 hydration, large payloads, decompression, batch size and backpressure. Estimate worst-case
calls/work from actual code; do not claim latency/cost savings without measurements.

Inspect SQL ordering/index use from actual queries and representative test data. Read-only
EXPLAIN and workload testing belong in isolated data environments; EXPLAIN ANALYZE executes
queries and must not be run against production or on mutations by default. Preserve
historical valuation bases and projection/caching semantics while investigating performance.

Check trace/error redaction, correlation propagation, metric cardinality, useful failure
classification, retry/DLQ/lag signals, cleanup progress and evidence retention. Logging a
failure is not failure custody; successful invocation is not durable business completion.
Avoid adding sensitive labels, raw payloads or unbounded user IDs to metrics.

Review CI affected-crate logic and path filters for code, migrations, resources and skill
changes; feature/target blind spots; non-blocking/skipped jobs; timeout/retry behavior;
reproducible tool versions, provenance and dependency/image update policy. Don't assume
branch protection settings from repository files; inspect only with authorized connector access.

Review migration-before-consumer order, CDK app/synth checks, artifact/version promotion,
rollback compatibility, retained state, provider credentials/configuration ownership,
initialization gates and run-to-completion scheduled tasks. Source code that defines an
alarm, DLQ, schedule or CDC route does not prove it is active or staffed.

## Operator checklist — report, do not execute

List evidence needed for backup/restore drills; deletion/erasure behavior; replay/rebuild
safety; queue age/DLQ recovery; expired secrets/certificates; dependency and base-image
support; alert delivery; stage isolation; provider webhook reconciliation; and rollback
exercises. Assign unknowns a human owner and last-verified date only when actually known.
Use privacy-minimized evidence and explicitly authorized read-only live checks in a separate
workflow. Never deploy, send messages, replay queues or perform retention changes here.

Output source-backed findings, measurement plans, CI blind spots and a separate table of
unverified operational assumptions. Low-risk code cleanups follow policy; infrastructure,
IAM, routing, migration and timeout/retention semantics require reviewed changes.

Before returning, perform the read-only [verification pass](../aura-verify/SKILL.md).
Use the [report template](../aura-maintenance/references/report-template.md). Report
severity separately from confidence, preserve prior unresolved findings, and state
`partial` or `blocked` when the declared coverage or required checks are incomplete.
