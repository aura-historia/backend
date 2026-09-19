# Lightweight deployment checks in CI (C1)

The `deployment-checks` job requires Python3, OpenSSL and the reviewed official Docker Compose CLI plugin 5.4.0 for Linux x86-64. CI verifies its executable SHA256 (`837fd1d35bf6a494f41b5b5988269a7be79de337cf1a1a6ff0e45ab51bb4e9be`) through a cleared-environment private Docker config before running the existing test modules with the actual Compose-config check enabled. Compose 2.30 introduced `env_file.format: raw`, but that minimum does not establish the unresolved `env_file` JSON contract required by the host command.

```sh
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  AURA_TEST_COMPOSE_CONFIG=1 \
  python3 deploy/tests/run_ci.py
```

`run_ci.py` discovers `test_*.py`, requires nonzero discovery and a successful result, and rejects failures, errors, unexpected successes and any skip outside the one documented case: `test_search_ca_compose.SearchCaComposeTests.test_actual_candidate_ca_bind_is_readable_and_readonly_as_uid10001`, skipped for `opt-in cached networkless helper only`. C1 does not set `AURA_TEST_CA_MOUNT=1`; it does not build images or start application services. The Compose-config test executes against the checked-in files.

## C2 — maintained secured OpenSearch pin and scoped identities

2026-09-18 implementation handoff: official downloads, version history and artifacts identify OpenSearch3.8.0, released2026-08-04. Registry metadata confirms the official multi-platform index `sha256:fafe3fc3587088674669235575aa166228c48bdb940294a8cdbbc1da75236a40`; linux/amd64 child manifest is `sha256:68a688de28fb9bb66601552650b91a52a9fd5e7eac5481dd2b225ecb66fd09b0`. Checked-in `deploy/compose/opensearch/image.ref` contains the full official reference. No local3.8.0 image was present; no pull or engine rehearsal was run.

C2 code makes `deploy/bin/opensearch initialize-fresh|verify` parse that immutable pin and require the exact server version/distribution before any write. The secured-node harness stages and verifies the same pin, uses its actual local image ID only after registry-digest/platform proof, and performs no runtime pull/build. Native node/admin Compose still share `OPENSEARCH_IMAGE`; the node remains HTTPS/native-security/no-demo/no-auto-create/no-query-capture by configuration. C2h changes only the product mapping SQ encoder parameters; analysis, filter mapping, and RRF bytes remain unchanged. Mapping hashes are recorded in the source guard.

Ordinary application Compose now gives `product-listing-opensearch.env`, `search-filter-projection.env` and `search-filter-percolator.env` after shared `worker.env`, all `format: raw`. API/api-candidate use `aura_reader`; the three workers use `aura_product_projector`, `aura_filter_projector`, `aura_percolator`; cron uses `aura_cron`. The seven other worker scopes and crawler receive no OpenSearch credentials. Host model/input checks enforce exact paths, private0600 scoped files, snapshots, drift rejection and real-stage username/nonblank-password identity checks.

Observed safe checks: `run_ci.py` passed209 deployment tests with exactly the documented cached-helper skip. Current focused source counts are host/decoder33, secured-harness48, pure Compose26; actual Compose-config identity/credential tests passed with one expected helper skip. Rust format, focused locked/all-feature Rust library tests, worker binary tests and strict Clippy passed at the reviewed C2 checkpoint. These are unit, source-guard, loopback TLS and Compose-config evidence—not selected-engine, native application startup or Rust-SDK-to-OpenSearch evidence.

The real3.8.0 secured-node and real-client rehearsal is **NOT RUN/BLOCKED** at this handoff: local Docker has no disposable daemon/socket, and the new ordinary-PR `opensearch-secured` job has not yet supplied execution evidence. Do not use the historical OpenSearch3.1.0 R2/R3 pass below as C2 evidence. SDK2.4 redirect limitation remains: direct preflight refuses redirects, but the public SDK builder has no same-origin/no-follow control; C2 uses only a trusted private nonredirecting endpoint when that rehearsal is later authorized.

# Native image smoke (R2)

For simultaneous13-process ordinary Compose startup and same-version restart (R3), see [`../compose/README.md`](../compose/README.md). R2 below remains the separate per-image/signal regression. For the actual host-command A→B/fault-candidate rehearsal (R4), see [`../bin/README.md`](../bin/README.md).

Runs the **actual four application image entrypoints** against fresh PostgreSQL16/pg_ttl_index3.0.0 and OpenSearch3.1.0. Worker image runs `product-listing-opensearch` and `product-listing-normalization`. Python stdlib provides non-forwarding ADC/JWKS/SQS doubles. No live accounts, customer data, inference or email.

This is **idle image startup/shutdown evidence**, not complete Compose, real SQS/Sequin custody, active-work drain, provider authentication/TLS, host reboot or A→B cutover. These are outside R2’s evidence; see the R3/R4 results above and their remaining gates.

## Real Sequin TLS rehearsal

`smoke-sequin-tls.py` launches stock Sequin0.14.6, PostgreSQL16, Redis and stunnel5.80 through the checked-in platform/TLS Compose files plus `sequin-tls.fixture.yml`. Synthetic SQL/WAL and a non-forwarding webhook only; no application/worker/search startup or cloud calls. Requires Python3/OpenSSL, local Docker/Compose and cached R3 images/helper plus the [built TLS sidecar](../images/README.md#sequin-postgresql-tls-sidecar).

```sh
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  python3 -m unittest discover -s deploy/tests -p test_sequin_tls.py -v
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  python3 deploy/tests/smoke-sequin-tls.py \
  --tls-image sha256:e740ea04ebd5d3d70e6ac8822d390f8a3ceea90ae3465fc930644689ebc479ee
```

2026-09-14: **integrator PASS; independent repeat PASS; 24 guard tests pass**. Real metadata migrations and TLS SQL/WAL sessions (`pg_stat_ssl`); committed synthetic event delivered. Wrong metadata CA/hostname blocks migrations. Wrong source CA/hostname and server TLS refusal reject both SQL and WAL attempts; no source authorization/delivery, metadata stays healthy. Restoring trust reconnects the **same slot and sink**, delivers the committed event, creates no backfill. A separate backend container cannot connect to either plaintext loopback listener.

Evidence uses pinned actual signatures: DBConnection2.7.0/Postgrex0.19.3 SQL failure PIDs are attributed via bounded stock release RPC reading existing connection state; SlotProducer failures require matching source/replication UUIDs. No connection options/credentials are returned. Stunnel failures must belong to the correct listener/connection and specific fault. A timeout or absent delivery alone cannot pass. Boolean-only diagnostics identify missing evidence.

Isolation: internal bridge, no published ports, tmpfs database/Redis state, synthetic credentials/certificates, protected temporary directory. Exact reviewed Compose source hashes are checked **before parsing**; rendered model/env/mount/network policy is validated before startup. Actual resource ownership/configuration is verified after startup and before cleanup. No builds/pulls or inherited cloud credentials. Test logger keeps one1MiB file with compression disabled (Docker rejects this single-file limit with compression enabled).

Work deadline540s plus60s owned cleanup; use660s external timeout. Nonzero/unknown Docker mutations retain resources and ownership journal for explicit inspection; do not blindly rerun. Cleanup removes only checked owned fixtures, never dev state or cached images. Successful integrator/reviewer runs confirmed cleanup. SIGKILL/host loss cannot promise cleanup.

Limits: this proves the [shared-loopback stunnel boundary](../compose/README.md#verified-sequin-postgresql-transport), not Sequin-native end-to-end verified TLS, full worker custody, live CA/firewall safety, expired-certificate behavior, loaded operation, namespace replacement, host reboot or production readiness. PostgreSQL plaintext HBA is deliberate **test-only** fallback detection. Do not reuse fixture configuration in dev.

## Real dev fresh-initialization rehearsal

Runs the actual nonroot `bootstrap-dev` image through `compose.bootstrap-dev.yml`, with explicit **STAGE=dev**, verified TLS and Docker DNS. Disposable synthetic PostgreSQL16/pg_ttl_index3 only; not a live dev deployment or permission grant.

```sh
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  python3 deploy/tests/smoke-bootstrap-dev.py \
  --image sha256:d5fcd8e90ce6b1e414559bcd0f8eca3dbb15796fdf35ee30d8a01054b25f7de7 \
  --source-sha 13a4b822673e0a33ed0e90ddee4f90889a097d4a
```

**Integrator PASS; independent exact-command repeat PASS; all109 deployment Python tests PASS.** Both embedded SQLx histories match checkout/baseline sources; rejected initialization and read-only verification preserve logical `pg_dump`/globals/database-inventory/TTL-config snapshots. Successful initialization changes only its selected DB. Invalid unselected URL is tolerated; this is not a claim about whether `getenv` reads it. Repeat initialization rejects `NOT_FRESH`. No Docker executable/socket in the one-shot container.

Wrong CA/hostname/password require case peer + PostgreSQL PID + specific rejection signatures; valid TLS controls bracket each. Plaintext refusal has narrower evidence: server SSL off, successful authenticated plaintext control, attributed TCP receipt without authentication, and client completion before its connection timeout. Restricted-role verification succeeds with TLS, rejects plaintext and succeeds after restoration. No raw logs or synthetic secrets printed.

Requires cached [initializer image](../images/README.md#explicit-dev-fresh-initializer), pinned PG fixture `sha256:1ef4f65fa354b5771def2872dc765c5cafb5c3f1e56ce1d394a6ad4af33279be`, local Unix Docker/Compose, OpenSSL and Git. Internal unique network, no published ports, bounded tmpfs PostgreSQL, private CLI/env/CA fixtures. Reviewed Compose bytes checked before parsing; model/runtime inputs checked before start. Test-only fixed peers are derived from the owned network for log attribution—not application topology.

Work600s + cleanup90s; external timeout780s. Any acceptance/unknown-command failure retains the fixture and ownership/snapshot records. Inspect exact owners and completed command outcomes before cleanup/rerun. Both successful runs independently left zero owned containers/networks/directories. No global prune or cached-image removal.

Limits: synthetic superuser initialization and read-only verifier, not real runtime write grants, complete DDL drift, TTL-worker health, crash/commit-loss coverage, cross-target atomicity or live authority. TTL worker is not started in this snapshot fixture. Plaintext HBA exists only to detect client fallback; never reuse it in dev. Image source label describes the baseline plus uncommitted implementation, not clean-source provenance. No new SQL, adoption, repair, incremental upgrade or backfill machinery.

## C2a secured-engine CI gate

The ordinary `opensearch-secured` PR job uses a fresh GitHub-hosted `ubuntu-24.04` runner, installs checksum-verified Compose5.4.0, builds `opensearch-test-helper` plus the test-executable-only `opensearch-tls-witness`, pulls the checked-in OpenSearch3.8.0 digest for linux/amd64, runs the literal raw-environment Compose witness, and then passes all inspected local IDs to the existing rehearsal. It has `contents: read` only and no AWS credentials, OIDC, deployment environment, published port, application service or full-stack dependency.

The witness uses `literal-env.fixture.yml` and `literal-env-witness.py`: raw `env_file` values include single/double dollars and a literal `${...}`; the nonroot, read-only, capability-free, `network_mode: none` container emits only `LITERAL_ENV_OK`. Docker `Config.Env` is compared privately with the decoded Compose model and exact baked-image merge. Failure/unknown ownership remains bounded and is not converted to a skip.

The gate's final rehearsal command is:

```sh
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  python3 deploy/tests/smoke-opensearch.py --reviewed-run \
  --helper-image "$C2_HELPER_IMAGE" \
  --witness-image "$C2_WITNESS_IMAGE" \
  --opensearch-image "$C2_OPENSEARCH_IMAGE"
```

`C2_HELPER_IMAGE` and `C2_WITNESS_IMAGE` are iidfile IDs from `.github/scripts/prepare-opensearch-test-images.sh`; `C2_OPENSEARCH_IMAGE` is the inspected local ID for the validated `deploy/compose/opensearch/image.ref`. The helper and witness options do not change engine selection. After existing grants, the harness runs five short-lived read-only/capability-free UID10001 witness containers on the fixture’s internal network: API reader, each of the three worker search scopes, and cron. Each mounts only the generated root CA read-only and receives one synthetic role’s OpenSearch credentials; retained evidence records fixed runtime/scope/role, exit status, output size, and output hash—not passwords or output.

Current C2a source checks: decoder/host tests **33 passed**; secured-harness tests **48 passed**; pure Compose tests **26 passed**; `run_ci.py` **209 passed with one documented skip**; actual Compose-config identity/credential tests **passed with one expected helper skip**. The real-container witness and selected-engine rehearsal are **NOT RUN locally** because this workstation has no Docker socket. The CI gate remains **PENDING**; no OpenSearch3.8.0 version/plugin or Rust SDK request is claimed here.

## C2b — secured OpenSearch gate corrections

2026-09-18: The preparation script keeps the tagged pinned pull input but verifies the tagless repository digest in Docker `RepoDigests`. Its local ID, Linux/amd64, helper-contract, immutable-pin, build and pull failures remain fail-closed. The actual script is regression-tested with a non-forwarding Docker stub across canonical success and metadata/input/build/pull failures; synthetic IDs in that test are not observed images.

The literal witness uses one unresolved Compose5.4.0 render for raw `env_file` declarations and one resolved render for effective values, both through the existing decoder. A new lightweight test renders the checked-in `literal-env.fixture.yml` without inspecting, creating or starting a container, and checks literal bytes, `0600` mode, mount/security boundaries and the synthetic image syntax input. Local C2b `run_ci.py` evidence is **211 passed, one documented cached-helper skip**. Observed `Integrate (CI)` run `35367462503` completed with **78 successful jobs and one failed secured OpenSearch job**; preparation failed at canonical image identity validation, so the literal container and selected-engine rehearsal did not run.

## C2c — native securityadmin diagnosis

`Probe.admin()` now keeps the bounded native output in one fixed private `0600` file (`securityadmin-{offline,admin,node,unregistered}-output.txt`), records fixed nonsecret facts in `evidence.json`, and prints one bounded `SECURITYADMIN_RESULT` JSON line. Facts include the actual native exit code, output size/hash, the existing marker booleans, terminal status markers, known configuration types, allowlisted exception labels, and a strict numeric packaged Security plugin version when the native tool reports it. The original conservative success predicate and certificate-rejection predicate are unchanged; failed/unknown admin containers remain retained and are never retried.

The 3.8.0.0 native source was inspected: required/optional configuration behavior is not changed by this slice, and no guessed YAML file was added. The reviewed C2b run `35428127852` proved preparation, the real literal witness and deployment-tooling job, but its public log did not contain the native result. Local focused validation is **57 secured-harness tests passed**; the full daemon-free deployment runner is **220 passed with one documented cached-helper skip**. C2c source changes are not pushed; the local Docker daemon is not treated as an authorized disposable runner. Therefore the resulting native rehearsal, packaged-plugin observation, root cause and full acceptance remain **NOT RUN/BLOCKED** until the existing CI lane runs the changed revision.

## C2d — read-only native stage isolation

C2d adds one preflight after node readiness and before the unchanged online native command. It uses the isolated admin-certificate client for GET-only `/_plugins/_security/whoami`, `/_nodes`, and bounded `/_cluster/health?level=cluster&timeout=5s` checks. The single `SECURITY_PREFLIGHT` line publishes only fixed booleans, node count/versions, numeric Security plugin versions, and `green|yellow|red` health plus the timeout boolean; the same facts are retained in safe evidence. The preflight does not fail only on red health, and a failed preflight stops before online SecurityAdmin.

`SECURITYADMIN_RESULT` now adds fixed upstream 3.8.0 error categories, safe simple unexpected-exception class extraction, and boolean progress markers. Unknown `ERR:` output remains `unclassified_failure`; raw output stays private `0600`, and native flags, retention, and verdict semantics are unchanged. Local C2d validation is **72 focused tests passed** and **235 lightweight tests passed with the one documented skip**. The C2c remote facts remain: run `35432020086`, job `105868211680`, online exit `255`, `ERR:` true, no recognized config-upload result, and current classification **unclassified**. Root cause is **PENDING** until a changed-revision secured run.

## C2e/C2f — maintained secured-engine native stage

C2e final remote evidence: head `0a6f5656be44f31c7106d794c1653a53524d4406`; `Integrate (CI)` run `35435402516`; synthetic merge `43c7e33132e2acdd9dc64a6bf5a8c89e8860272d`; secured job `105877083797`. The selected node was OpenSearch3.8.0 with Security3.8.0.0. GET-only preflight passed with one node, matching admin DN, `admin=true`, `node_certificate=false`, plugin present/version `3.8.0.0`, and bounded yellow health. Online native SecurityAdmin then exited255 with `unexpected_exception_class=UnsatisfiedLinkError` before `Connected as`; no progress markers were seen. The complete inventory was **78 success, 1 failure, 0 cancelled, 0 skipped, 0 queued, 0 in progress**; only the secured job failed.

C2f preserves `/tmp:rw,nosuid,nodev,noexec,size=64m,mode=1777` and adds only to `opensearch-admin` a bounded memory-backed `/securityadmin-native-tmp:rw,nosuid,nodev,exec,size=16m,mode=0700,uid=1000,gid=1000`. `java.io.tmpdir` and `jna.tmpdir` point there; heap/CPU limits and all existing isolation/retention rules remain. The exact Compose 5.4.0 model and source pin reject weaker `/tmp`, missing or unsafe native temp, wrong Java paths, writable `/operator`, and propagation to another service. C2f local evidence is **75 focused tests passed** and **238 lightweight tests passed with the one documented cached-helper skip**.

Remote C2f run `35437937144` reached the changed online native stage on the selected OpenSearch3.8.0/Security3.8.0.0 node. Preflight passed. `SECURITYADMIN_RESULT` reported exit0, connected_as_seen=true, cluster contact/identity/config population/security-index creation true, done_with_success=true, no error category or unexpected exception, and all eight expected configuration types uploaded; `UnsatisfiedLinkError` was removed. The existing rehearsal continued to `CHECK partial-init-refused-unchanged` and then safely retained on `OPERATOR_RESULT_RETAINED`. The complete Integrate inventory was **78 success, 1 failure, 0 cancelled, 0 skipped, 0 queued, 0 in progress**; only secured job `105883692593` failed, while `Test src/fxrate-postgres` job `105883752850` passed. A separate SonarCloud Code Analysis PR check failed outside that matrix. No C2f follow-on fix, Rust SDK gate, C3, C4, activation, or live deployment was started.

The final documentation-only C2f head `32d0d2f6b1e720a0afa7de6eb7029db163e0645a` was verified by `Integrate (CI)` run `35439335184`, secured job `105887321925`. It reproduced the safe native result and then `CHECK partial-init-refused-unchanged` followed by `FAIL OPERATOR_RESULT_RETAINED`.

C2f acceptance is explicit: **native SecurityAdmin loader compatibility PASS; `UnsatisfiedLinkError` removed PASS; native security upload PASS**. Fresh application OpenSearch operator acceptance remains **FAIL/PENDING**.

## C2g — safe operator phase/trace evidence

C2g changes only the secured rehearsal harness and tests. Pure `operator_diagnostic(mode, expected_success, result)` requires the exact helper-result shape, real boolean expectation, bounded output/trace, and fixed mode/outcome/phase vocabularies. It emits only fixed facts: exit-code comparison, parsed status, write risk from the operator outcome, fixed trace labels, counts, and trace validity. Allowed requests map to `target`, read labels, and create labels; unknown pairs are never printed and fail closed. `Probe.operator()` records `operator-result` and prints `OPENSEARCH_OPERATOR_RESULT` before the unchanged retained-result, expected-negative no-write, and GET-only assertions. No retry, post-failure probe, cleanup, initializer, image, Compose, TLS, role, mapping, pipeline, or workflow behavior changed.

Local C2g focused validation is **86 tests passed, no skips**; the full lightweight runner is **249 passed with one documented cached-helper skip**. Ordinary PR `Integrate (CI)` run `35440856771` tested head `01609494c164b1474dc9793d15a8cf36e97f620c` at synthetic merge `79419364f23435f8972f7c019aed002f22cae98b`. Secured job `105891266900` passed `SECURITY_PREFLIGHT` and online `SECURITYADMIN_RESULT` (exit0, connected/cluster/config/security-index progress true, all eight native types uploaded), then reached `CHECK partial-init-refused-unchanged`. The last safe `OPENSEARCH_OPERATOR_RESULT` was `mode=initialize-fresh`, `expected_success=true`, `exit_code=1`, `expected_exit_code=0`, `outcome=partial-or-unknown-do-not-retry`, `phase=create-product-listings`, `writes_may_have_occurred=true`, `trace_valid=true`, `trace_steps=[target, read-product-listings, read-user-search-filters, read-pipeline, create-product-listings]`, `last_trace_step=create-product-listings`, `trace_count=5`, `write_trace_count=1`; it was followed by `FAIL OPERATOR_RESULT_RETAINED`. No raw output or request/response body was published. Integrate inventory was **76 success, 2 failure, 0 cancelled, 1 skipped, 0 queued, 0 in progress**; failures were `Test secured OpenSearch` job `105891266900` and `Test src/aura-historia-api` job `105891329870`. The skipped Integrate non-matrix `SonarQube-Cloud Analysis` job was `105893907273`; external CodeQL passed, with no external failure in this run. Rust SDK compatibility, C3, C4, activation, and live deployment remain deferred.

## C2h — OpenSearch 3.8 Faiss SQ FP16 mapping contract

C2g isolated the first failed mutation to `PUT /product-listings`. OpenSearch k-NN3.8.0.0 requires `bits` when an SQ encoder is explicitly configured for FLOAT vectors; its no-bits integration test rejects `sq + type=fp16`, while its `bits=16,type=fp16,clip=false` test accepts the selected configuration. C2h changes only the product mapping encoder parameters to `sq + bits=16 + type=fp16 + clip=false`. The current product mapping SHA-256 is `53e0185379e8379a4edfa81d5acabde31e10e5f6c3939203f10840f0a5d0e4fe`; the fail-closed source guard pins those exact bytes.

Local C2h validation passed JSON syntax, the exact digest, `git diff --check`, **115** focused bootstrap/secured tests with no skips, and **250** lightweight deployment tests with the one documented cached-helper skip. Remote `Integrate (CI)` run `35447213189` tested head `7d574438a92cf576744c8692026aa09c67ed4e88` at synthetic merge `a07e638337d20a77a5d214d17f5e3f78b9a5d009`; secured job `105908101480` retained the passing native/security and unchanged negative-path checks, then advanced beyond `create-product-listings` and safely stopped at `create-user_search_filters` with `partial-or-unknown-do-not-retry`. No retry occurred. Thus C2h confirms the product mapping correction but not full secured-rehearsal acceptance. Rust SDK compatibility, C3, C4, activation, and live deployment remain deferred.

## Historical secured stock OpenSearch rehearsal (3.1.0 compatibility fixture)

From repo root, with Python3/OpenSSL and local Linux/amd64 Docker/Compose:

```sh
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  python3 -m unittest discover -s deploy/tests -p 'test_*.py'
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  python3 deploy/tests/smoke-opensearch.py --reviewed-run
```

**Full rehearsal PASS; independent exact-command repeat PASS; all180 deployment Python tests PASS.** Cached stock image `sha256:0dd81b2051dc9ccd9e466596aa66b7764b55184886eeccd36c1cf17bdf5ed27d` and Python helper `sha256:adbdfc3fab194e4291b7e5db1eaa1dfa997ea8a51e5a50229bec60124374deb8`; no builds/pulls/cloud credentials. Stock OpenSearch3.1.0/Security3.1.0.0 is an **unmaintained compatibility fixture, not an approved live engine**.

Tests execute checked-in native security tools/configuration and `deploy/bin/opensearch`: fresh mappings/RRF, GET-only admin verification, missing/partial/repeated/populated refusal; all five runtime roles; projector own-index writes, standard denials and exact item-level bulk denials. Content-free external-version fences survive older/equal writes beyond delete GC and after restart. Synthetic768-vector hybrid search and multipage PIT percolation succeed. Wrong CA/hostname/password/plaintext and node/unregistered admin certificates reject. Same-container restart preserves checked logical data/security/settings. No filtering of system alias entries or security snapshot sections; hashes do not attest all plugin/physical state or exclude transient writes.

Native ML health waits for **one active shard**, not yellow status alone, before baselines/restart verification. Stock may report yellow with zero active primaries during creation. All five Query Insights settings are checked disabled/exporter none, with no exported insight indices at checkpoints. Security REST management denial does not require the response to echo a username; exact stock authorization signature and unchanged snapshots are required. Original denial/health responses remain bounded/private for diagnosis, never printed.

Isolation: unique internal network/owned disposable data volume; no published ports, inherited credentials or external providers. Frozen source hashes checked before parsing; exact model/runtime/mount/ownership checks. Synthetic CA/node/admin/runtime credentials separated; admin tool UID1000, no admin key mounted to node. Work1200s + successful cleanup90s; use1380s external timeout. Failures retain exact ownership/evidence; inspect completed commands and exact resources before scoped cleanup. No blind rerun/prune. Both successful runs left no owned resources/temp directories; cached images retained.

Limits: REST/platform compatibility, **not Rust-client CA wiring**, active CDC, application startup against this secured node, expired-certificate/rotation testing, host/daemon reboot, maintained-engine or live-dev acceptance. The node restart is not a host reboot. [Operator setup](../compose/README.md#secured-opensearch-fresh-setup) remains separate and requires real inputs/authority.

## Rust search-client CA and mount checks

Actual API, worker SDK/direct preflight and cron client constructors are tested with loopback TLS, not cloud/database fixtures. Run each command with a240s outer bound:

```sh
cargo test -p platform-opensearch -p aura-historia-api -p aura-historia-worker -p aura-historia-cron --lib --all-features --locked --offline
cargo test -p aura-historia-worker --bin aura-historia-worker --all-features --locked --offline

env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  AURA_TEST_COMPOSE_CONFIG=1 AURA_TEST_CA_MOUNT=1 \
  python3 -m unittest discover -s deploy/tests -p 'test_*.py'
```

Independent validation: **761 Rust tests passed** across those suites, six ignored parent-owned/opt-in entries (owning tests invoke their required children); **184 Python tests passed, no skips** with both opt-ins. Workspace check/dependency rules/format passed. New tests require Unix/OpenSSL; no provider/paid calls. Trusted CA succeeds, wrong CA/hostname fails, supplied file replacement does not change frozen clients. Worker/cron negatives require certificate alerts, not timeout/reset. Shared SDK/direct clients also test two-CA trust overlap and direct no-redirect behavior.

The Python opt-ins run actual Compose config (including candidate inheritance) and one cached Python helper with `--network=none`, UID10001, one public test-CA read-only mount. Reads match exact bytes; writing returns EROFS. Exact-owned container/temp cleanup checked; no app services or platform state touched. Default Python invocation skips those two opt-ins. Compose HTTP fixtures still leave the CA env input unset even though the bind file exists.

This is runtime-client and mount evidence, **not newly built images, full application startup against the secured node, A→B replay, live permissions/credentials or maintained-engine acceptance**. SDK default redirects remain a documented limitation; the CA tests do not prove HTTPS on redirected hops. Historical R2–R4 image IDs below remain old artifacts.

## Build and run

Requires local Linux/amd64 Docker at `/var/run/docker.sock`, Python3, the existing PostgreSQL fixture image and the reviewed application/helper images. Build instructions: `../images/README.md`. Public registry/package downloads occur during build/pull only; the smoke never pulls.

The current reproducible fixture uses source `672bdcefdeaabc6cd9f78461ec1bf859c31dc443`. The script checks expected source identity and image configuration; source-context comparison is separate. For another source, deliberately update its fixture SHA/builds together; do not relabel old binaries. Labels alone are not provenance. Rust/Cargo/migration/search input bytes were checked unchanged against that source before execution; no signed/public release was produced.

From repository root, after the documented builds/pull:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s deploy/tests -p test_smoke_images.py -v

PYTHONDONTWRITEBYTECODE=1 python3 deploy/tests/smoke-images.py \
  --api sha256:7d0037cc85d6aba544be7f35dc2e2ac2be156c6dc697fdef38a1ae45e3451cfe \
  --worker sha256:963f0ea9bb704cf3b88dfe9791645bfc81a75bb159793bbdb4c54363bbd38284 \
  --cron sha256:8e180232aad2995de05ce972ea12d30989c9010597a29c49c1aa28d3bb45be06 \
  --crawler sha256:38d7d0067a645e6055aafc8efcdd0ac8f7a6d0119df4dbaf04a65e25f9e204d6 \
  --helper sha256:adbdfc3fab194e4291b7e5db1eaa1dfa997ea8a51e5a50229bec60124374deb8 \
  --opensearch sha256:0dd81b2051dc9ccd9e466596aa66b7764b55184886eeccd36c1cf17bdf5ed27d \
  --opensearch-digest opensearchproject/opensearch@sha256:0dd81b2051dc9ccd9e466596aa66b7764b55184886eeccd36c1cf17bdf5ed27d
```

Rebuilding may produce different image IDs despite identical source (image timestamps/attestations are not bit-reproducibility guarantees). Inspect the locally built IDs and supply those exact IDs; no mutable tag is used during smoke. PostgreSQL is pinned in the script to the existing fixture digest; building that fixture is documented under `src/test-api/postgres/`. No arbitrary existing database is accepted.

## Isolation and ownership

- Helper runs `--network=none`; all other containers share its loopback-only namespace. No published ports, host network, Docker socket mount, forwarding proxy or host cloud credentials. Script verifies the helper has only `lo`.
- Existing `bootstrap-local` runs **only in the fixture helper**, initializing separate newly created business/crawler databases using genuine SQLx histories. It never ships in any application image. No history stamping, adoption or down migration.
- Restricted fixture runtime role has SELECT/public access plus the crawler's narrow empty-source UPDATE grant; bootstrap superuser is not passed to apps. Test-only plaintext and security-disabled OpenSearch are contained in the isolated namespace, never production defaults.
- Public checked-in analysis files bind at OpenSearch's actual config path; checked-in mappings are submitted unchanged. Readback accounts only for observed3.1.0 omission of redundant object type/default enabled:true. Nested/disabled properties and all other definitions remain checked.
- Applications/helper run UID10001, read-only, no capabilities or privilege escalation, with resource limits. Long-lived fixture restart=no is **test isolation only**, not the rejected production restart policy. Production Compose will use ordinary active-service restart policy.
- Acquire random name/ownership label/cidfile/full ID before start. Cleanup touches only verified acquired full IDs; verifies absence. Uncertain create/removal retains safe manual-recovery identifiers and prevents a final PASS. Do not blindly rerun while uncertainty remains. Inspect the exact reported name/label/ID; never prefix-delete or prune.
- Commands and HTTP are bounded; global smoke deadline30min, then bounded cleanup. Docker create and critical acquisition briefly defer signals so ownership is not forgotten. SIGKILL/host loss cannot run cleanup; inspect exact fixture labels before another run. No automatic recovery promise.

## Assertions and limits

Each of five process configurations passes read-only preflight, real START/READY and exact operational identity, default budgets, nonroot PID1, resolved ELF libraries, then separate SIGTERM and SIGINT exits0. Old listeners refuse connections and runtime DB sessions disappear. Two worker scopes complete actual empty AWS-SDK polls against the double; normalizer completes a structured zero-work reconciliation turn. No application has been changed to substitute a fake handler.

Negative startup inputs: missing SQLx history, bad JWKS/queue attributes, missing ADC. Readiness must not be observed; JWKS/SQS cases additionally require fault acknowledgement, witnessed request and expected safe error category. Missing-history/ADC cases demonstrate configuration rejection, not cause-specific internal diagnosis. Preflight and failed composition must make no token/receive attempts. Tiny synthetic JWKS proves availability/parse shape, not JWT signature verification.

Probe bodies and **retained1MiB logs only** are checked for fixture credential/connection-URL canaries. Not whole-lifetime redaction evidence after log rotation. No raw request/header/provider error bodies printed. Database histories and empty source tables stay unchanged after runtime tests.

2026-09-13 integrator result: **PASS** all four images, two worker scopes, ten READY/signal launches, five preflights, startup negatives and owned cleanup. Initial runs exposed only OpenSearch's omitted default mapping representation; those failed assertions were repaired, not application mappings. Six pure helper regressions also pass. Independent review and accepted commit: `docs/deployment/implementation-status.md`.
