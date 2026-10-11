---
name: aura-security
description: "Review aura-historia/backend security boundaries: authorization, OAuth/Cognito, provider webhooks, SSRF, secrets, AI inputs, dependency and CI supply chain. Use for recurring defensive audits; no live exploitation or automatic auth redesign."
disable-model-invocation: true
---

# Security and supply-chain review

Read the [shared policy](../aura-maintenance/references/policy.md),
[scope and report procedure](../aura-maintenance/references/runbook.md), current root
and applicable nested AGENTS.md, and the [document map](../aura-maintenance/references/repo-map.md).
Default `mode=audit execution=static scope=changed`; inherit explicit coordinator
parameters, or resolve scope using the runbook when invoked alone.
Map trust boundaries and attacker-controlled inputs first. Read the current front-door,
consent, identity, storage and provider contracts, not just generic security checklists.

If a required local reference is missing, report `blocked`; do not invent a replacement.

## Review lanes

**Identity and authorization.** Follow anonymous, valid Cognito, valid Aura opaque-token,
invalid optional-token, user/admin/partner and delegated-scope paths. Check operation-level
and object-level authorization, tenant/owner scoping, mass-assignment, subject binding,
revocation/expiry on use, one-time atomic consumption, PKCE/state/redirect binding where
implemented, account linking trust and race barriers. The documented lack of a Gateway
JWT authorizer is intentional; determine whether Axum/use cases enforce the contract.

**HTTP and provider intake.** Check body/header limits, exact-byte signature verification,
timestamp/replay/receipt semantics, constant-time comparisons where required, signature
verification before effects, strict provider identity and environment binding. Check
parameterized SQL, constrained dynamic identifiers, query-language injection, redirects,
CORS and approved shared-cache variation. A malformed bearer is not an anonymous request.
Never log raw signatures/tokens/body evidence to demonstrate a finding.

**Outbound network and content.** Trace actual callers of public-network-policy and
image-fetcher/crawler/provider clients. Inspect scheme/credentials restrictions, all DNS
answers, connection binding, every redirect, IPv4/IPv6 encodings and private/metadata
ranges; an IP validator alone does not establish complete SSRF protection. Check response
byte/time/decompression/image-size budgets and bounded retries. Model output and scraped
pages are untrusted data: structured parsing, canonical validation, output size/cost
budgets and no conversion of generated instructions into tool authority or shell commands.

**Secrets and operational exposure.** Check redaction, error causes vs external messages,
configuration defaults, fail-closed missing configuration, least-privilege IAM, trust
policies and cross-stage isolation from source. Separate source concerns from unverified
live configuration. Review raw evidence/PII in CDC, search, logs and test fixtures. For a
potential leaked credential record only location/type and a redacted indicator; don't
copy it into reports or validate it against the provider.

**Dependencies and automation.** Inventory Cargo.lock plus infra/mjml lockfiles, direct and
transitive packages, registry/git sources, build/install scripts, native code and images.
With explicit network permission, use installed cargo-audit and npm audit; record versions,
advisory retrieval time, exact package/version, enabled features/target and reachability.
Cargo-deny is useful only with a reviewed dependency/license/source policy. Do not invent
an organizational license allowlist. Inspect Actions permissions, full-SHA pinning,
untrusted PR data in shell/AI prompts, pull_request_target/workflow_run trust boundaries,
credential exposure, checkout persistence, artifact/cache poisoning and stale exceptions.
Do not blindly upgrade dependencies or suppress advisories.

## Validation and output

Require input -> trust check -> sink/side effect evidence. Try to refute candidates and
record prerequisites before assigning severity. Use synthetic data/local emulators only.
Prefer a regression at the vulnerable boundary over a scanner-only finding. Independent
verification is especially important for P0/P1. Report exploitability as confirmed only
when proven; unknown environmental conditions remain assumptions. Review outputs locally
before sharing. Do not automatically publish vulnerabilities to this public repository.


Optional external methodology is listed in [sources](../aura-maintenance/references/sources.md).
No external skill, plugin, service or scanner is required; do not fetch or install one.


Before returning, perform the read-only [verification pass](../aura-verify/SKILL.md).
Use the [report template](../aura-maintenance/references/report-template.md). Report
severity separately from confidence, preserve prior unresolved findings, and state
`partial` or `blocked` when the declared coverage or required checks are incomplete.
