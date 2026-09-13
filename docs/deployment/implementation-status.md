# Deployment reset — implementation status

**Active plan: owner's Deployment Implementation Reset Playbook. Old numbered hybrid plan superseded. Deployment readiness:false. No live mutations.**

## Checkpoint

2026-09-13: clean `task/#1412-deployment` at `cf65af2c64bd3930908b3890b64636817291fc3c`. Local branch, cached tracking ref and read-only `git ls-remote origin refs/heads/task/#1412-deployment` all match. Earlier history preserved; no reset, force-push, merge or production deployment. Historical checks/commits are in `implementation-history.md`, not fresh validation or an active roadmap.

Models: orchestrator GPT-6-Astra; delegation exposes no model selector. Requested Terra cannot be selected. Actual agent identities/results recorded below.

## Active sequence

| Milestone | State / executable acceptance |
|---|---|
| R1 | Accepted scope reduction; commit recorded below. Rejected08a removed, nonexistent migrator no longer required; no runtime changes |
| R2 | Accepted locally: four images built and actually run; independent repeat smoke passed. Idle startup only, not deployment readiness |
| R3 | Not started: ordinary platform/application Compose must actually launch full isolated backend |
| R4 | Not started: thin flock/current/previous/incomplete host command; real A→B and failed candidate, workers/jobs/state survive |
| R5 | Not started: instantiate existing NAT construct, attach actual DB Lambdas, deliver CA, prove approved dev connectivity |
| R6 | Not started: environment-approved develop workflow around proven host command |
| R7 | Not started: CalVer promotion and previous immutable release redeploy, rehearsed outside prod |
| R8 | Not started: prove native dev API before CloudFront/Caddy public-origin cutover |

Migration scope is resolved: **fresh initialization only**; no adoption, historical backfills, incremental framework or schema downgrade. Do not ask again. Dormant legacy manifest/protocol metadata is not used for current development deployments. Missing fictional migrator no longer belongs in the actual component catalog.

## R1 — scope reduction

Remove only `deploy/control/src/host/application-compose.ts`, its dedicated test and `deploy/compose/README.md`. Existing runtime, TLS/custody fixes, fresh bootstrap, generic helpers and NAT source/tests stay unchanged. No replacement renderer or owner-aware reconciler. Current catalog is four native/five Lambda/ten worker scopes; dormant parser permits the four real images without requiring historical synthetic migrator identity, while missing real applications still reject.

Archive previous status/decisions with explicit superseded headings. Active decisions and nearest AGENTS now require ordinary Compose/restart policy/flock, direct runnable acceptance and correctness+simplicity review. Fresh Node26.8.2 TypeScript build/schema generation/**594 tests**/catalog pass. Only generated schema diff: native minItems5→4. Deleted ignored prior dist output before rebuild; no stale renderer test credited. Independent reviewer `655af283-b9b6-44bb-a079-8ce2cbbbb9e3` (GPT-6-Astra) accepted correctness and simplicity, independently183 catalog/release tests plus current-source/24-output byte match and each-missing-real-image rejection matrix. Cargo/Rust/migrations/infra/workflows byte-identical to checkpoint; no runtime regression from changed call sites. Whitespace pass. Accepted commit: `83e41ab2f5fcd1c5fd31f41a1be3cede149c43da` (`revert(deploy): remove rejected renderer and activate simple deployment reset`). Final183 targeted tests pass after stale migrator-comment correction. No replacement abstraction.

## R2 — runnable images accepted locally

Source/build baseline: `672bdcefdeaabc6cd9f78461ec1bf859c31dc443`, `task/#1412-deployment`. Rust/Cargo/toolchain/migration/search inputs unchanged. New recipe/tests were uncommitted during execution; image labels identify source, not cryptographic build provenance. Capability commit: pending recording after commit.

`deploy/images/Dockerfile` builds four actual nonroot linux/amd64 executables with pinned Rust1.98.0/base digests/signed Debian snapshot and locked Cargo. Fixture-only helper reuses existing `bootstrap-local`; neither bootstrap nor Python ships in application targets. `deploy/tests/README.md` records all immutable image IDs and exact reproduction command; `deploy/images/README.md` records builds and runtime inputs/budgets.

- Earlier230s cold build timed out, exported nothing; not credited. Owner approved30min retry: shared release compilation **14m56s passed**, remaining three targets reused it. Helper build **5m42s passed**.
- Integrator actual smoke **PASS**; independent reviewer repeated exact immutable-ID command: **PASS**. Genuine separate fresh SQLx histories, restricted runtime role, real OpenSearch3.1.0 mappings/file-backed analysis. ADC/JWKS/SQS are explicit non-forwarding doubles in a loopback-only Docker namespace, not cloud integration.
- Four binaries/two worker scopes: ten READY/version/nonroot/library/SIGTERM/SIGINT launches, five read-only preflights, startup negatives, empty worker polls/normalizer turn. Owned cleanup confirmed; independent before/after container listings empty. No unowned cleanup/prune.
- Six Python regressions pass. Earlier mapping-readback assertions failed because OpenSearch omits redundant object/default enabled:true fields; test-only normalization narrowly handles those omissions. Production definitions unchanged. SHA guard/nested-target exclusion and ownership/fault-witness checks repaired before acceptance.

Implementer `8d339547-8a9a-464d-844a-261d3ecc03b9`; smoke draft/investigation `e5114dd9-f895-40b3-959a-e33bae90798b` (no startup credit); smoke repair `23275748-2516-4c0f-95a0-13eb5c6697ca`. Source reviewer `78c77645-459c-4b1b-84fb-1f8ec0d7b9e0` independently38 SHA cases; smoke reviewer `97da37d2-49bd-49dd-9a4d-e7719ad30b8a` cleared repaired attempt. Final independent reviewer `88995a93-b9c4-40bf-8080-f5989ff6890f` **accepted correctness and simplicity**, reran six tests/full smoke, checked unchanged source. All GPT-6-Astra. Its nonblocking source-identity wording finding is corrected. Harness stays test-only; no replacement deployment abstraction.

Limits: idle startup only. No all-ten-scope, real provider/TLS/JWT authentication, SQS/Sequin custody, active-work drain, full Compose, A→B, reboot or live deployment acceptance. Canary checks cover probes/retained1MiB logs only. Runtime/infra/workflow code unchanged; no architecture deviation. Removing this slice does not alter existing runtimes or schemas; cached images retained, disposable fixture state removed.

## External and test gates

No actual target host, AWS development account/region authorization, firewall/DNS authority, reviewer identities or real runtime secrets/CA supplied for execution. These block the corresponding live steps, not permission to invent them. R5–R8 live acceptance remains blocked. Existing source/template observations never prove a running deployment.

R2 build-budget/startup gate is cleared by actual local execution. Next R3 must launch the complete backend with ordinary Compose; R4 must prove real A→B and bad-candidate behavior. No LocalStack Pro licensing/provider authority inferred. Prior full workspace library test timed out; not rerun for image/test/docs-only changes. No cloud/host workflows enabled.
