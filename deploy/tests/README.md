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

## Secured stock OpenSearch rehearsal

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
