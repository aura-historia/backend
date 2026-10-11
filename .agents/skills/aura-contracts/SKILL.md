---
name: aura-contracts
description: "Check aura-historia/backend API, event, persistence, generated-asset and documentation drift. Use periodically to compare canonical contracts with code/tests and preserve backward compatibility and durable identifiers."
disable-model-invocation: true
---

# Contracts and documentation drift

Read the [shared policy](../aura-maintenance/references/policy.md),
[scope and report procedure](../aura-maintenance/references/runbook.md), current root
and applicable nested AGENTS.md, and the [document map](../aura-maintenance/references/repo-map.md).
Default `mode=audit execution=static scope=changed`; inherit explicit coordinator
parameters, or resolve scope using the runbook when invoked alone.
Use docs/README.md to identify the single owner of each contract; do not create duplicate
guides or update requirements to rubber-stamp code.

If a required local reference is missing, report `blocked`; do not invent a replacement.

1. Compare changed Axum routes/DTOs/errors/auth requirements with docs/swagger.yaml,
   docs/CHANGELOG.md and the Gateway route catalog / existing route-matrix tests. Verify
   methods, path normalization, required/optional/null/omitted fields, status/error shapes,
   body limits, pagination/cursor semantics, idempotency behavior, CORS and cache headers.
   Include no-content, malformed input, invalid optional bearer and not-found cases.
2. Compare domain leaf types and boundary codecs with stable object IDs, prefixed text,
   persistence representation, canonical enum spellings and explicit alias/version rules.
   Detect derives or direct field construction bypassing constructors; test serialization
   and deserialization separately. An encoder/decoder round-trip alone can share a bug.
3. Compare producer, router, worker codec, database/CDC schema and replay expectations for
   durable jobs/events. Review missing/extra fields and unsupported versions against the
   owner's chosen contract. Do not conflate version numbers belonging to separate envelopes.
4. Compare provider assumptions with the exact API version/contract in use using primary
   docs when approved. Check partial/paginated responses and unknown status handling.
   Preserve provider-specific differences; don't impose one adapter's behavior on another.
5. Inspect generated MJML/Cognito HTML and other checked-in generated outputs. Use the
   pinned existing generator only in a disposable worktree, diff outputs and confirm the
   source of truth. Never hand-edit a generated artifact instead of its owning source.
6. Report stale operational claims and orphaned links, but keep docs/arch.md and storage.md
   as durable design guides. They are not catalogs of every type/table/route/limit.
   Source/synth cannot establish current deployment, DNS, WAF, alarm or recovery state.

Output each mismatch with both locations, direction of drift, compatibility risk,
consumer impact and minimum correction/test. Code-vs-doc uncertainty requires a decision,
not automatically choosing code. Public/durable contract changes are needs_design by
policy; spelling/link fixes may qualify for bounded repair when no semantics change.

Before returning, perform the read-only [verification pass](../aura-verify/SKILL.md).
Use the [report template](../aura-maintenance/references/report-template.md). Report
severity separately from confidence, preserve prior unresolved findings, and state
`partial` or `blocked` when the declared coverage or required checks are incomplete.
