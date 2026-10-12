---
name: aura-bug-hunt
description: "Search aura-historia/backend for reproducible correctness bugs, async/concurrency failures, boundary-value errors and false-success paths. Invoke manually on changes and callers or on a bounded context."
disable-model-invocation: true
---

# Evidence-driven bug hunt

Read the [shared policy](../aura-maintenance/references/policy.md),
[scope and report procedure](../aura-maintenance/references/runbook.md), current root
and applicable nested AGENTS.md, and the [document map](../aura-maintenance/references/repo-map.md).
Default `mode=audit execution=static scope=changed`; inherit explicit coordinator
parameters, or resolve scope using the runbook when invoked alone.
Read the owning domain/storage/API/event contracts before deciding behavior is wrong.

If a required local reference is missing, report `blocked`; do not invent a replacement.

## Method

1. Build a behavior map for the selected use cases: inputs, authority, state transitions,
   permissions, output contract, irreversible effects and failure paths. Choose high-risk
   paths rather than mechanically reading every file or flagging every panic helper.
2. State falsifiable bug hypotheses and trace input to sink. Search especially for:
   swallowed Result/error conversions; failed dependencies treated as empty/not-found;
   unauthorized optional-bearer fallback; unchecked conversions and newtype bypass;
   off-by-one pagination and unstable ordering; missing cursor/filter scope binding;
   integer overflow, currency exponent and rounding mistakes; source vs display-price
   confusion; event-time vs processing-time errors; precision loss and unit mismatches.
3. Exercise state sequences: duplicate, reorder, retry after timeout, cancel during await,
   concurrent update/delete, expired-but-not-cleaned credential, provider acceptance then
   failed local finalization, stale worker lease, and process restart. Distinguish a caller
   giving up from actual cancellation of a side effect. Look for DB transactions across
   network I/O, blocking work in async paths, unbounded task spawning and dropped errors.
4. For financial/history logic inspect persisted snapshot provenance, temporal selection,
   exact boundary inclusion, immutable source preservation and deterministic ordering.
   Read current specialized contracts for whether a missing projection is permitted;
   never generalize all missing data into either fallback or failure.
5. Reproduce locally with the smallest realistic test using public contracts or actual
   adapter boundaries. Check competing explanations in existing guards/configuration.
   Prefer parameterized boundaries and deterministic clocks/fakes; use isolated real DB
   tests for constraints/concurrency. Never probe deployed systems by default.
6. In repair mode, implement only eligible low-risk fixes after a regression fails for the
   right reason; verify it passes afterwards and related behavior remains unchanged.
   A test that cannot execute supports a candidate/unverified patch, not a verified fix.

## Deliverable

For each candidate: exact location, inputs/state/preconditions, expected vs actual,
reachability, minimal reproducer, observed baseline result, alternative explanation
checked, fix/test plan and confidence. A static proof may confirm a finding when decisive;
label it as static evidence and never invent runtime results. Keep risky auth, transaction,
billing and persisted-contract fixes for an approved design change.

Before returning, perform the read-only [verification pass](../aura-verify/SKILL.md).
Use the [report template](../aura-maintenance/references/report-template.md). Report
severity separately from confidence, preserve prior unresolved findings, and state
`partial` or `blocked` when the declared coverage or required checks are incomplete.
