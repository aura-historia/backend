# Deployment reset — implementation status

**Active plan: owner's Deployment Implementation Reset Playbook. Old numbered hybrid plan superseded. Deployment readiness:false. No live mutations.**

## Checkpoint

2026-09-13: clean `task/#1412-deployment` at `cf65af2c64bd3930908b3890b64636817291fc3c`. Local branch, cached tracking ref and read-only `git ls-remote origin refs/heads/task/#1412-deployment` all match. Earlier history preserved; no reset, force-push, merge or production deployment. Historical checks/commits are in `implementation-history.md`, not fresh validation or an active roadmap.

Models: orchestrator GPT-6-Astra; delegation exposes no model selector. Requested Terra cannot be selected. Actual agent identities/results recorded below.

## Active sequence

| Milestone | State / executable acceptance |
|---|---|
| R1 | Accepted scope reduction; commit recorded below. Rejected08a removed, nonexistent migrator no longer required; no runtime changes |
| R2 | In progress, **not accepted**: four-target Dockerfile drafted; cold shared compilation timed out230s; no final image/startup yet |
| R3 | Not started: ordinary platform/application Compose must actually launch full isolated backend |
| R4 | Not started: thin flock/current/previous/incomplete host command; real A→B and failed candidate, workers/jobs/state survive |
| R5 | Not started: instantiate existing NAT construct, attach actual DB Lambdas, deliver CA, prove approved dev connectivity |
| R6 | Not started: environment-approved develop workflow around proven host command |
| R7 | Not started: CalVer promotion and previous immutable release redeploy, rehearsed outside prod |
| R8 | Not started: prove native dev API before CloudFront/Caddy public-origin cutover |

Migration scope is resolved: **fresh initialization only**; no adoption, historical backfills, incremental framework or schema downgrade. Do not ask again. Dormant legacy manifest/protocol metadata is not used for current development deployments. Missing fictional migrator no longer belongs in the actual component catalog.

## R1 — scope reduction

Remove only `deploy/control/src/host/application-compose.ts`, its dedicated test and `deploy/compose/README.md`. Existing runtime, TLS/custody fixes, fresh bootstrap, generic helpers and NAT source/tests stay unchanged. No replacement renderer or owner-aware reconciler. Current catalog is four native/five Lambda/ten worker scopes; dormant parser permits the four real images without requiring historical synthetic migrator identity, while missing real applications still reject.

Archive previous status/decisions with explicit superseded headings. Active decisions and nearest AGENTS now require ordinary Compose/restart policy/flock, direct runnable acceptance and correctness+simplicity review. Fresh Node26.8.2 TypeScript build/schema generation/**594 tests**/catalog pass. Only generated schema diff: native minItems5→4. Deleted ignored prior dist output before rebuild; no stale renderer test credited. Independent reviewer `655af283-b9b6-44bb-a079-8ce2cbbbb9e3` (GPT-6-Astra) accepted correctness and simplicity, independently183 catalog/release tests plus current-source/24-output byte match and each-missing-real-image rejection matrix. Cargo/Rust/migrations/infra/workflows byte-identical to checkpoint; no runtime regression from changed call sites. Whitespace pass. Commit: `revert(deploy): remove rejected renderer and activate simple deployment reset` (exact SHA follows in next status update). No replacement abstraction.

## R2 — image work, unaccepted

Implementer `8d339547-8a9a-464d-844a-261d3ecc03b9` (GPT-6-Astra): ordinary shared multistage Dockerfile and four final targets, pinned Rust/base digests/native package snapshot, nonroot intended binaries and source SHA. Read-only smoke investigator `e5114dd9-f895-40b3-959a-e33bae90798b` (GPT-6-Astra) identified genuine PG/history/search plus bounded provider doubles needed; no smoke harness or successful startup claimed.

Four Docker build checks passed. Full API-target/shared compilation against local Unix Docker **timed out230s (exit124)**; BuildKit build `d31n1r9rjgehvdp07dxd2vcn8` terminal Error. No final image exported, no service containers launched, no lingering build. Longer build retry not performed. Independent source reviewer `78c77645-459c-4b1b-84fb-1f8ec0d7b9e0` held acceptance: whole-argument SHA validation and explicit nested-target exclusion repaired; actual guard independently38/38 shell cases pass. No completed link/runtime verification. Drafts remain uncommitted and are not part of accepted R1. Do not commit/credit this as an accepted image capability until all four actually run.

## External and test gates

No actual target host, AWS development account/region authorization, firewall/DNS authority, reviewer identities or real runtime secrets/CA supplied for execution. These block the corresponding live steps, not permission to invent them. R5–R8 live acceptance remains blocked. Existing source/template observations never prove a running deployment.

R2 cold compilation needs a longer approved test budget. Four-image readiness needs an isolated fixture with genuine SQLx histories and compatible OpenSearch; current cached fixtures alone do not prove it. LocalStack Pro licensing/provider access is not implicitly authorized. Image START/READY with named provider doubles would be limited smoke evidence, not real SQS/Sequin custody, complete Compose or A→B success. Prior full workspace library test timed out; not rerun yet. No cleanup/prune of unowned resources.
