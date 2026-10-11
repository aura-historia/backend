---
name: aura-cleanup
description: "Perform evidence-based, behavior-preserving cleanup in aura-historia/backend: dead code, duplicated logic, misleading abstractions, error hygiene and local complexity. Audit by default and avoid cosmetic churn or speculative rewrites."
disable-model-invocation: true
---

# Maintainability and conservative cleanup

Read the [shared policy](../aura-maintenance/references/policy.md),
[scope and report procedure](../aura-maintenance/references/runbook.md), current root
and applicable nested AGENTS.md, and the [document map](../aura-maintenance/references/repo-map.md).
Default `mode=audit execution=static scope=changed`; inherit explicit coordinator
parameters, or resolve scope using the runbook when invoked alone.
First enforce the repository's actual conventions; don't replace them with personal style.

If a required local reference is missing, report `blocked`; do not invent a replacement.

1. Find measurable maintenance costs: unreachable or obsolete private code, genuinely
   duplicated logic with identical semantics, misleading names/comments, needless clones
   on hot paths, oversized responsibilities, redundant conversions, unused dependencies
   and accidental complexity. Use current compiler/Clippy/TypeScript diagnostics first.
2. Before deleting, search all references including tests, cfg/feature/target combinations,
   proc macros, string/dynamic registration, public exports, generated code, CI scripts,
   serialization and external consumers. No textual references does not prove dead code.
   An unreferenced public API or persisted variant is not safe to remove automatically.
3. Before consolidating, compare validation, normalization, failure behavior, ownership,
   concurrency and lifetime/representation contracts. Two snippets that look alike may
   have different bounded-context meanings. Prefer the existing correct owner; avoid
   abstraction for its own sake and don't create a generic utility hub.
4. Evaluate modern/idiomatic alternatives against the pinned Rust/Node/dependency versions.
   Do not cargo-update, add tools/dependencies, adopt nightly or raise MSRV as cleanup.
   Keep #[source]/BoxError chains and intentional validation checks. Don't convert failure
   into default data to reduce branch count or appease a lint.
5. In repair mode choose tiny behavior-preserving changes after baseline validation;
   stay within the policy's three-finding/300-production-line budget. Avoid repository-wide
   reformatting, API renames, enum churn, public visibility changes, crate shuffles or edits
   to generated artifacts. Verify affected dependents as well as the owning crate.

For each proposal state maintenance benefit, proof of equivalent behavior, references
searched, risk, minimal patch and verification. Present subjective or uncertain improvements
as optional design suggestions; don't manufacture a cleanup quota. Recurring patterns
should become focused automated checks only after review of false positives.

Before returning, perform the read-only [verification pass](../aura-verify/SKILL.md).
Use the [report template](../aura-maintenance/references/report-template.md). Report
severity separately from confidence, preserve prior unresolved findings, and state
`partial` or `blocked` when the declared coverage or required checks are incomplete.
