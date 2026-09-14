# Runnable native OCI images (R2)

One ordinary multi-stage Dockerfile: shared locked Cargo build, four nonroot final images. No deployment framework, runtime initializer, startup wrapper or embedded credentials.

| Target | Sole application executable / entrypoint | Default external stop budget |
|---|---|---:|
| `api` | `/usr/local/bin/aura-historia-api` | 60s |
| `worker` | `/usr/local/bin/aura-historia-worker` | 300s |
| `cron` | `/usr/local/bin/aura-historia-cron` | 330s |
| `crawler` | `/usr/local/bin/server` | 330s |

One worker image serves configured scopes. Default entrypoints use actual production composition. SIGINT/SIGTERM go directly to the executable. Configure the supervisor with these budgets or the declared larger runtime budgets; Docker's10s default is insufficient.

## Build

From repository root, with local Linux Docker/BuildKit and public Docker Hub/Debian snapshot/crates.io access. No host compilation or cloud account is required. Do not substitute a remote daemon.

These commands reproduce the tested Rust source baseline `672bdcefdeaabc6cd9f78461ec1bf859c31dc443`. For new source use its actual full SHA and a new traceability tag, not these old labels. The build rejects malformed/multiline/placeholder SHA values; it cannot prove the supplied context matches Git. Verify source before release.

```sh
test -S /var/run/docker.sock

docker --host unix:///var/run/docker.sock buildx build --builder default --load --target api --build-arg COMMIT_SHA=672bdcefdeaabc6cd9f78461ec1bf859c31dc443 -t aura-reset-r2-api:672bdcef -f deploy/images/Dockerfile .
docker --host unix:///var/run/docker.sock buildx build --builder default --load --target worker --build-arg COMMIT_SHA=672bdcefdeaabc6cd9f78461ec1bf859c31dc443 -t aura-reset-r2-worker:672bdcef -f deploy/images/Dockerfile .
docker --host unix:///var/run/docker.sock buildx build --builder default --load --target cron --build-arg COMMIT_SHA=672bdcefdeaabc6cd9f78461ec1bf859c31dc443 -t aura-reset-r2-cron:672bdcef -f deploy/images/Dockerfile .
docker --host unix:///var/run/docker.sock buildx build --builder default --load --target crawler --build-arg COMMIT_SHA=672bdcefdeaabc6cd9f78461ec1bf859c31dc443 -t aura-reset-r2-crawler:672bdcef -f deploy/images/Dockerfile .

docker --host unix:///var/run/docker.sock image inspect aura-reset-r2-api:672bdcef aura-reset-r2-worker:672bdcef aura-reset-r2-cron:672bdcef aura-reset-r2-crawler:672bdcef --format '{{.Id}} {{.Architecture}} {{json .Config.Entrypoint}} {{.Config.User}}'
```

Select exact image **digests/IDs**, not mutable tags or `latest`, for startup/deployment. Tags are human traceability only. Tested local IDs and complete startup command: [`../tests/README.md`](../tests/README.md). No registry publication/signing or release workflow is claimed. Preserve deployed/previous digests when later publishing; do not overwrite a release tag or prune retained images.

## Inputs and runtime

- Rust1.98.0 matches repository toolchain; `cargo build --release --locked`, two jobs, exactly four production bins. Base index digests verified with public `buildx imagetools inspect` on2026-09-13. Bookworm builder/runtime avoid GNU ABI mismatch; tested architecture linux/amd64 only.
- Signed Debian snapshot `20260901T000000Z` pins native package resolution. Build adds CMake/pkg-config/OpenSSL headers; runtime installs CA roots/OpenSSL3/GCC/C++/zlib, no compiler. Refresh pins deliberately for security; bit-identical output across rebuilds is not promised.
- Dockerfile-specific deny-by-default context includes workspace Cargo/Rust files, existing migrations, embedded review UI and search assets. Explicitly excludes nested target outputs; no `.git`, cloud config, env/key files or host binaries. A suffix allowlist is not a general secret-content scanner.
- Runtime UID/GID10001, exec entrypoint and source revision label. Crawler embeds build-time `COMMIT_SHA`; other binaries read baked env. Do not override identity independently of binaries.
- No stage/endpoints/TLS bypass/secret baked into application images. Supply existing runtime inputs through restrictive host-owned env files and readable CA/ADC mounts. PostgreSQL password-file modes remain0400/0600. Dev/prod still require verified TLS.
- Full configuration lives in crate rules: [`api`](../../src/aura-historia-api/AGENTS.md), [`worker`](../../src/aura-historia-worker/AGENTS.md), [`cron`](../../src/aura-historia-cron/AGENTS.md), [`crawler`](../../src/crawler/AGENTS.md).
- API business defaults0.0.0.0:8080; worker0.0.0.0:8081 needs trusted network. API operations127.0.0.1:9080, cron127.0.0.1:8082, crawler operations127.0.0.1:9083 and review listener remain inside container namespace. Do not publish private probes. No fabricated healthy image healthcheck.

## Sequin PostgreSQL TLS sidecar

`Dockerfile.sequin-postgres-tls` builds **stock stunnel5.80**, not another application executable or Sequin fork. It uses the same pinned Bookworm base/signed20260901 snapshot and checks upstream tarball SHA-256 `6d0841d48de07cbbaf4a055919065bf7bb5ebc63cc15c97a2c76caa2bf285513`. Nonroot UID10001, SIGTERM, only stunnel plus its runtime libraries/CA package; no private credentials. Entrypoint: `/usr/local/bin/stunnel /run/aura/stunnel/stunnel.conf`.

Build from repository root with a credential-free local Docker CLI configuration. Stdin provides only the Dockerfile, no repository/secret context. Example below uses the actual rehearsal baseline; for changed source use its actual SHA and a new tag.

```sh
docker --host unix:///var/run/docker.sock buildx build --builder default --load \
  --build-arg COMMIT_SHA=8ac6f424f87de45e28fdeed657159cfd28f11e81 \
  -t aura-historia-sequin-postgres-tls:probe-8ac6f424 - < deploy/images/Dockerfile.sequin-postgres-tls
```

Observed build37.1s; tested local image ID `sha256:e740ea04ebd5d3d70e6ac8822d390f8a3ceea90ae3465fc930644689ebc479ee`. Recipe was uncommitted during build; baseline label is traceability, not signed provenance. Pinned inputs do not promise bit-identical rebuilds. The image was run successfully in the [real Sequin TLS rehearsal](../tests/README.md); not published or activated on dev. It requires the [ordinary platform overlay and host CA/config](../compose/README.md#verified-sequin-postgresql-transport).

## Isolated smoke helper

Fixture-only `smoke-build`/`smoke-helper` targets reuse Cargo output to build the existing `bootstrap-local` executable and add Python3. They are not application/release components. Production images contain neither Python nor bootstrap. The helper exists only to initialize genuine fresh fixture histories and run non-forwarding provider doubles/probes.

```sh
docker --host unix:///var/run/docker.sock buildx build --builder default --load --target smoke-helper --build-arg COMMIT_SHA=672bdcefdeaabc6cd9f78461ec1bf859c31dc443 -t aura-reset-r2-helper:672bdcef -f deploy/images/Dockerfile .
docker --host unix:///var/run/docker.sock pull --platform linux/amd64 opensearchproject/opensearch@sha256:0dd81b2051dc9ccd9e466596aa66b7764b55184886eeccd36c1cf17bdf5ed27d
```

OpenSearch3.1.0 platform digest was resolved from official index `sha256:474ea3fdf25d229e103018b14c8a0d5bb858113f919bc3f0652a3c2e98f16f1e`. PostgreSQL uses the existing pinned PG16/TTL3 fixture; no new DB image or migration framework.

## Verified scope

Four builds and actual idle startup **passed**. Cold shared release compile14m56s under the owner-approved30min ceiling; other final targets reused it. Helper compile5m42s. Earlier230s attempt timed out and exported no image; not credited.

Actual smoke: real fresh PostgreSQL histories, real OpenSearch mappings/file-backed analysis, four native binaries/two worker scopes, ten READY/version/nonroot/library/signal launches, read-only preflight, startup negatives, cleanup. Controlled ADC/JWKS/SQS doubles only. [`../tests/README.md`](../tests/README.md) states exact assertions and limits.

Not verified: other architectures, real provider credentials/TLS, all ten worker scopes, active workload drain/custody, full Compose, A→B, host reboot, live deployment or production readiness. No runtime/Cargo/schema source changed to make these images pass. Keep existing safety tests; do not mistake successful image startup for those later milestones.
