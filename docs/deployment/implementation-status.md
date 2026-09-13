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
| R3 | Accepted isolated empty-stack startup/same-version restart; all13 apps, real Sequin/Caddy, platform state retained. Live-dev readiness still blocked |
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

Source/build baseline: `672bdcefdeaabc6cd9f78461ec1bf859c31dc443`, `task/#1412-deployment`. Rust/Cargo/toolchain/migration/search inputs unchanged. New recipe/tests were uncommitted during execution; image labels identify source, not cryptographic build provenance. Accepted capability commit: `b1e743ae740b9512019d95d3832387b79a77f976` (`feat(deploy): build and smoke-test runnable native OCI images`). Final staged whitespace check and six Python tests passed; nothing pushed or deployed.

`deploy/images/Dockerfile` builds four actual nonroot linux/amd64 executables with pinned Rust1.98.0/base digests/signed Debian snapshot and locked Cargo. Fixture-only helper reuses existing `bootstrap-local`; neither bootstrap nor Python ships in application targets. `deploy/tests/README.md` records all immutable image IDs and exact reproduction command; `deploy/images/README.md` records builds and runtime inputs/budgets.

- Earlier230s cold build timed out, exported nothing; not credited. Owner approved30min retry: shared release compilation **14m56s passed**, remaining three targets reused it. Helper build **5m42s passed**.
- Integrator actual smoke **PASS**; independent reviewer repeated exact immutable-ID command: **PASS**. Genuine separate fresh SQLx histories, restricted runtime role, real OpenSearch3.1.0 mappings/file-backed analysis. ADC/JWKS/SQS are explicit non-forwarding doubles in a loopback-only Docker namespace, not cloud integration.
- Four binaries/two worker scopes: ten READY/version/nonroot/library/SIGTERM/SIGINT launches, five read-only preflights, startup negatives, empty worker polls/normalizer turn. Owned cleanup confirmed; independent before/after container listings empty. No unowned cleanup/prune.
- Six Python regressions pass. Earlier mapping-readback assertions failed because OpenSearch omits redundant object/default enabled:true fields; test-only normalization narrowly handles those omissions. Production definitions unchanged. SHA guard/nested-target exclusion and ownership/fault-witness checks repaired before acceptance.

Implementer `8d339547-8a9a-464d-844a-261d3ecc03b9`; smoke draft/investigation `e5114dd9-f895-40b3-959a-e33bae90798b` (no startup credit); smoke repair `23275748-2516-4c0f-95a0-13eb5c6697ca`. Source reviewer `78c77645-459c-4b1b-84fb-1f8ec0d7b9e0` independently38 SHA cases; smoke reviewer `97da37d2-49bd-49dd-9a4d-e7719ad30b8a` cleared repaired attempt. Final independent reviewer `88995a93-b9c4-40bf-8080-f5989ff6890f` **accepted correctness and simplicity**, reran six tests/full smoke, checked unchanged source. All GPT-6-Astra. Its nonblocking source-identity wording finding is corrected. Harness stays test-only; no replacement deployment abstraction.

Limits: idle startup only. No all-ten-scope, real provider/TLS/JWT authentication, SQS/Sequin custody, active-work drain, full Compose, A→B, reboot or live deployment acceptance. Canary checks cover probes/retained1MiB logs only. Runtime/infra/workflow code unchanged; no architecture deviation. Removing this slice does not alter existing runtimes or schemas; cached images retained, disposable fixture state removed.

## R3 — ordinary Compose, isolated acceptance

Baseline `5b8914085c0819c0d8cbdfbbe344128ff8919e04`, clean `task/#1412-deployment` before work. Accepted capability commit `588b3f3cf140d831a35e16b5b1c1493218e5751a` (`feat(deploy): run isolated full backend with ordinary Compose`). Final43 Python tests and staged whitespace check pass. Nothing pushed or deployed. Runtime/Cargo/schema/search/infra/workflow sources unchanged; reused R2 binaries retain source identity `672bdcefdeaabc6cd9f78461ec1bf859c31dc443`, not the tooling SHA.

`deploy/compose/` now has ordinary separate platform/application/edge projects and nonsecret input example/runbook. Long-lived services use unless-stopped; applications are nonroot/read-only, default stop budgets60/300/330/330s. Platform volumes remain independent. Only Caddy publishes loopback HTTPS and joins an additional edge bridge; app/platform test network is internal. No renderer, secret materializer or new deployment framework.

Actual test: `PYTHONDONTWRITEBYTECODE=1 python3 deploy/tests/smoke-compose.py` **PASS**, then independent post-repair repeat **PASS** within30min. Same three Compose files plus explicit test-only override, genuine fresh SQLx histories/TTL3/OpenSearch, three separate databases, Redis, Sequin. All13 apps ready/identity-matched; ten scopes complete source/DLQ checks and empty polls. Ten actual Sequin worker-DNS bindings, restricted-role active replication, pause_on_full and zero backfills verified. Caddy trusted local-CA/hostname HTTPS succeeds, untrusted CA rejects, safe GET matches API404. App-only stop/start exits0/removes runtime PG sessions, preserves platform/edge container IDs/start times/volume records, then all13 return ready; checked stores/histories remain empty/unchanged. Exact owned containers/volumes/networks/temp directories removed and independently checked absent. Test implementer reports one earlier exact-manifest Caddy pull. Independent review runs performed no builds or pulls.

Failures repaired before acceptance: quoted tmpfs commas (base YAML); internal-only bridge could not publish Caddy port (Caddy-only edge bridge); invalid test ProductListingId; empty Compose IPAM normalization; draft Sequin endpoints replaced with real worker endpoints before final acceptance. Independent reviewer held volume/bind isolation gates; fixed exact project-owned local-volume and per-service fixture-bind allowlists, actual mount/env checks, backend network drift checks and revalidation before mutation/cleanup. No weakened runtime/security gates. **43 Python tests pass**, including negative ownership/storage/mount/network/provider cases. Reproduction/pins and operator inputs: `deploy/compose/README.md`.

Investigation: `f3d64531-edaa-491d-8040-381d61c064b8` runtime; `55a8122e-dfac-497e-a3f6-65afe5b1ded4` platform. Provider implementer `94ea3bf5-1b95-4665-9980-bbb17187f611`; Compose test implementer `11034af4-0e2b-4ae8-9ece-505577a2e1ee`; integrator owns ordinary Compose/docs. Independent reviewer `94551bf9-97f7-491c-a977-1881984363b7` accepted **correctness and simplicity**, independently43 tests and full post-repair smoke, zero owned resources remaining. All GPT-6-Astra; no selectable Terra. Nonblocking Zoho-input count corrected.

Acceptance limits: empty startup and same-version application restart only. SQS/ADC/JWKS doubles, not real provider/auth or active custody. No queue sends/purges, active CDC handling, A→B/bad-B, engine upgrade/reboot/TTL expiry, production acceptance or whole-stack no-egress proof. Sequin0.14.6 certificate verification, real-stage fresh initialization, OpenSearch trust/security bootstrap, approved provider credentials/queues/assets and host inputs remain real-deployment gates. Existing local-only bootstrap is not relabeled for real use. No general migration/backfill machinery added. Removal affects only these Compose/test/docs files; no existing runtime/schema changes, fixture resources removed, cached images retained.

## External and test gates

No actual target host, AWS development account/region authorization, firewall/DNS authority, reviewer identities or real runtime secrets/CA supplied for execution. These block the corresponding live steps, not permission to invent them. R5–R8 live acceptance remains blocked. Existing source/template observations never prove a running deployment.

R2 images and R3 isolated empty-stack gates are cleared by actual local execution. R3 live-security/provider gates remain above; R4 must prove A→B and bad-candidate behavior, not merely repeat same-version startup. No LocalStack Pro licensing/provider authority inferred. Prior full workspace library test timed out; not rerun for Compose/test/docs-only changes. No cloud/host workflows enabled.
