# Native image smoke (R2)

For simultaneous13-process ordinary Compose startup and same-version restart (R3), see [`../compose/README.md`](../compose/README.md). R2 below remains the separate per-image/signal regression. For the actual host-command A→B/fault-candidate rehearsal (R4), see [`../bin/README.md`](../bin/README.md).

Runs the **actual four application image entrypoints** against fresh PostgreSQL16/pg_ttl_index3.0.0 and OpenSearch3.1.0. Worker image runs `product-listing-opensearch` and `product-listing-normalization`. Python stdlib provides non-forwarding ADC/JWKS/SQS doubles. No live accounts, customer data, inference or email.

This is **idle image startup/shutdown evidence**, not complete Compose, real SQS/Sequin custody, active-work drain, provider authentication/TLS, host reboot or A→B cutover. These are outside R2’s evidence; see the R3/R4 results above and their remaining gates.

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
