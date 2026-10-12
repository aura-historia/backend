---
name: aura-architecture
description: "Audit architecture and internal convention deviations in aura-historia/backend against current AGENTS.md, canonical docs and depgraph rules. Use for periodic boundary/ownership reviews; do not invent a new architecture."
disable-model-invocation: true
---

# Architecture and conventions

Read the [shared policy](../aura-maintenance/references/policy.md),
[scope and report procedure](../aura-maintenance/references/runbook.md), current root
and applicable nested AGENTS.md, and the [document map](../aura-maintenance/references/repo-map.md).
Default `mode=audit execution=static scope=changed`; inherit explicit coordinator
parameters, or resolve scope using the runbook when invoked alone.
Read current docs/arch.md, docs/storage.md, depgraph-rules.toml and relevant specialized
owners. Follow inherited coordinator scope and permissions, or establish your own baseline.

If a required local reference is missing, report `blocked`; do not invent a replacement.

## Method

1. Inventory affected crates, dependency edges including feature-gated/target/dev/build
   dependencies, exported types and runtime composition. Distinguish a composition root
   from an HTTP/worker handler even when they share a crate.
2. Trace one command and one query end to end in each selected context. Identify the
   owner of domain behavior, use-case orchestration, authorization, transactions, ports,
   repository reconstruction, read models, transport mapping and external effects.
3. Compare code to exact normative statements and documented exceptions. Use the actual
   depgraph checker when approved/installed, but also inspect use-sites: an allowed crate
   dependency does not justify controller orchestration or adapter-type leakage.
4. Check core/service purity, thin handlers, capability-oriented ports, repository vs
   reader separation, transaction ownership, explicit fallible mappings, typed identifiers,
   newtype validation on deserialization, stable machine encodings, narrow shared crates,
   and preservation of technical error sources including commit/rollback errors.
5. Check convention drift: two competing canonical types, duplicate validators/codecs,
   mirror enums with identity-only mappings, repeated provider clients/deadline helpers,
   hidden environmental reads, improper visibility and a new common-style dependency hub.
   Prove semantic equivalence before proposing consolidation; adapter differences can be
   deliberate. Domain events are not required for every aggregate.
6. Distinguish a code violation, outdated implementation example, intentional exception,
   or ambiguous design. Do not remove a documented rule or broaden an allowance to make
   code appear compliant. Propose canonical-doc changes only for a human-approved general
   decision, and keep implementation inventories out of design harnesses.

## Required output

For each finding include the exact rule/section, violating edge/use-site, call path,
impact, smallest compliant change, migration/compatibility risk and suitable automated
regression. Report architectural ambiguities as needs_design. Crate moves, transaction
redesign and new abstractions are not automatic maintenance repairs. In repair mode, only
mechanical local changes with unchanged semantics and appropriate verification qualify.

Before returning, perform the read-only [verification pass](../aura-verify/SKILL.md).
Use the [report template](../aura-maintenance/references/report-template.md). Report
severity separately from confidence, preserve prior unresolved findings, and state
`partial` or `blocked` when the declared coverage or required checks are incomplete.
