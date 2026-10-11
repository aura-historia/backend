---
name: aura-test-quality
description: "Audit whether aura-historia/backend tests can catch real defects: meaningful assertions, negative paths, integration isolation, feature coverage, property tests and targeted mutation testing. Invoke manually on touched behavior or for a deeper test review."
disable-model-invocation: true
---

# Test effectiveness, not test count

Read the [shared policy](../aura-maintenance/references/policy.md),
[scope and report procedure](../aura-maintenance/references/runbook.md), current root
and applicable nested AGENTS.md, and the [document map](../aura-maintenance/references/repo-map.md).
Default `mode=audit execution=static scope=changed`; inherit explicit coordinator
parameters, or resolve scope using the runbook when invoked alone.
Inspect the actual fixtures, CI matrix and current contracts. AI-generated code and tests
may agree with each other while disagreeing with the requirement; use an independent oracle.

If a required local reference is missing, report `blocked`; do not invent a replacement.

1. Map each selected invariant to at least one test that would fail if it were violated.
   Review assertions for observable behavior, not just `is_ok`, mock call counts or a
   snapshot freshly copied from implementation. Check for tautologies, broad error matching,
   never-executed branches, ignored/flaky tests and helpers sharing the implementation bug.
2. Prioritize negative and boundary cases: unauthorized/wrong-owner operations, invalid
   optional bearer, corrupt persisted values, duplicate/reordered jobs, transient provider
   failures, uncertain commits, zero/max amounts, time boundaries, deletion races and expired
   credentials before cleanup. Test state sequences, not only isolated happy paths.
3. Check whether mocks omit DB constraints, row mapping, real transaction semantics,
   serialization/provider signatures or actual HTTP route/auth behavior. Identify where
   isolated PostgreSQL, OpenSearch or LocalStack tests add necessary evidence. Never silently
   replace these with mocks to make the suite pass. Library tests may use infrastructure.
4. Audit test discovery: library vs integration/binary/doctest targets, affected-crate
   selection, reverse dependents, CI path filters, generated resources and supported feature
   combinations. Do not claim all-feature tests cover default/no-default variants.
5. For an explicitly requested deeper review, choose one small high-value pure component (e.g. money/value validation/revision
   comparison) for approved property or mutation testing. Use an installed reviewed tool;
   inspect its help/configuration and run in a disposable copy/worktree with explicit
   timeout/scope. Classify surviving mutants: genuine test gap, equivalent, unreachable,
   timeout, or unviable. Never count every survivor as a bug or aim for an arbitrary 100%.
   Bounded fuzzing is useful for boundary parsers/codecs; keep corpora synthetic/redacted.
6. In explicit repair mode add focused tests for confirmed behavior; don't alter production
   semantics under a test-cleanup pretext. Demonstrate tests fail under the corresponding
   broken implementation where safe, and restore experimental mutations completely.

Output an invariant-to-test matrix, significant gaps, exact commands/outcomes, unsupported
coverage and a prioritized small test plan. Coverage percentages inform investigation;
they are not proof of assertions, independence, security or correctness.

Before returning, perform the read-only [verification pass](../aura-verify/SKILL.md).
Use the [report template](../aura-maintenance/references/report-template.md). Report
severity separately from confidence, preserve prior unresolved findings, and state
`partial` or `blocked` when the declared coverage or required checks are incomplete.
