# Ordinary Compose — R3

Three checked-in projects; no renderer or deployment framework:

| File / suggested project | Owns | Application release may restart? |
|---|---|---|
| `compose.platform.yml` / `aura-dev-platform` | PostgreSQL, OpenSearch, Redis, Sequin; persistent volumes | **No** |
| `compose.application.yml` / `aura-dev-application` | API, ten explicit worker scopes, cron, crawler | Yes, with R4 handover—not blind `up` against active singletons |
| `compose.edge.yml` / `aura-dev-edge` | Caddy and persistent local CA/config | No; routing reload belongs to R4 |

**Proven: isolated, empty full-backend startup and same-version application stop/start. Not yet a live-dev deployment command.** No A→B/bad-candidate, active queue custody, reboot, real provider authentication or production acceptance. Test configuration must never be copied to a real environment.

## Run the actual isolated stack

From repository root; Linux/amd64, Python3, local Docker/Compose (tested5.4.0; `env_file.format: raw` needs2.30+). Existing R2 immutable images/helper/PostgreSQL/OpenSearch must already be loaded; build commands: [`../images/README.md`](../images/README.md).

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s deploy/tests -p 'test_*.py' -v
# Only if the exact Caddy manifest is not cached; public registry download, bounded120s:
PYTHONDONTWRITEBYTECODE=1 python3 deploy/tests/smoke-compose.py --pull-caddy
# Subsequent runs perform no image downloads/builds:
PYTHONDONTWRITEBYTECODE=1 python3 deploy/tests/smoke-compose.py
```

The smoke uses these same three Compose files, plus **test-only** `deploy/tests/compose.fixture.yml`. It creates random project names, protected synthetic inputs and fresh persistent volumes; applies real SQLx histories using the existing disposable-local bootstrap helper; starts all services; verifies and removes only its owned resources. All Docker commands use `unix:///var/run/docker.sock` with isolated CLI configuration.

- App/platform network is internal. ADC/JWKS/SQS are non-forwarding doubles; no real credentials, website crawling, inference or email. SQS double permits attributes/empty receives only—not durable sends.
- Only Caddy joins an additional ordinary bridge: Docker cannot publish a host port from an internal-only network. Caddy is technically egress-capable; its pinned image gets only a fixed internal upstream/local-CA config, no provider credentials. **Do not describe the whole stack as no-egress.**
- Host HTTPS binds only a selected unused `127.0.0.1` port. TLS checks trust this run's Caddy CA explicitly and reject the normal trust store; host trust is not modified.
- Temporary files contain public synthetic fixture values. They are not production permission examples. Applications run UID/GID10001 and receive only applicable ADC mounts.
- Total test deadline30min. Unconfirmed/interrupted Docker operations retain the printed journal/config directory and project names. Inspect exact owners and command outcomes before rerun; no automatic takeover or global prune. SIGKILL cannot clean up.
- Successful test cleanup may use `down --volumes` **only for these verified disposable projects**. It is forbidden for ordinary application releases or existing business data.

### Tested images and evidence

Application source identity remains `672bdcefdeaabc6cd9f78461ec1bf859c31dc443`; Rust/Cargo/schema/search sources have not changed since R2. Do not relabel images to the R3 tooling commit. Full local image IDs live in `deploy/tests/smoke-compose.py` and [`../tests/README.md`](../tests/README.md); labels are not signed provenance.

Additional cached fixture image IDs: Sequin0.14.6 `sha256:336759c1c632ebdc87939bcea70b26b46df6593e666088e1bf29058209437d3e`; Redis7.4.2 `sha256:02419de7eddf55aa5bcf49efb74e88fa8d931b4d77c07eff8a6b2144472b6952`. Public Caddy2.11.4-alpine amd64 manifest: `caddy@sha256:98eb57d882ccd5213d1688764db10c1ca2c58a1ca3a6717a3411ad798f7a423a`. Cached fixture pins are not a platform security-maintenance certification; notably OpenSearch3.1 is no longer maintained.

Observed checks:

- All13 application containers simultaneously ready with exact source/scope identities; ten scopes witness both source/DLQ attributes and completed empty polls.
- Real Sequin health, active restricted-role replication slot, ten persisted endpoints pointing at **each worker's `:8081/cdc/sequin`**, `pause_on_full`, zero backfills.
- Caddy verified HTTPS GET matches direct API404 for an absent valid ProductListingId. This is proxy/read connectivity—not auth/CORS/webhook coverage.
- All apps stop with exit0 and no runtime DB sessions, then start again. Platform/edge container IDs, start times and volume records stay unchanged. SQLx histories and checked empty source/search stores stay unchanged.
- Idle only: no CDC send/receipt/delete/visibility custody, accepted in-flight drain, singleton A→B, engine restart, TTL expiry or reboot proof. Test stop allows340s; it does not measure each default stop-budget boundary. No whole-lifetime application-log redaction claim.

## Host-owned configuration contract

**Do not execute live setup from these examples yet.** Real-stage bootstrap/trust/provider gates below remain open. Operator supplies target authority and actual inputs. `env.example` contains nonsecret Compose selections; all image/queue blanks intentionally fail. Use immutable digests, explicit project names and one environment-specific network. Compose itself is not a digest validator; R4's host command must enforce immutable selection before mutation.

Keep `/etc/aura-historia/dev` root-controlled0700; raw env files root-owned0600, outside Git. Compose reads these files; it does not inherit shell credentials into containers. Do not print resolved Compose configuration, env files, or raw container/provider logs. `format: raw` preserves literal password characters rather than interpolating them.

| Host input | Required content / receiver |
|---|---|
| `compose.env` | Filled `env.example`, nonsecret image/queue/project settings |
| `api.env` | PostgreSQL/runtime, Cognito, Stripe, Zoho, search, Vertex/ADC and approved AWS credential inputs |
| `worker.env` | PostgreSQL, search/Vertex configuration, approved AWS SDK inputs; per-scope queue/scope comes from Compose |
| `notification-delivery.env` | Only delivery: `S3_BUCKET_NAME_TEMPLATES`, `NOTIFICATION_EMAIL_FROM`, `NOTIFICATION_EMAIL_REPLY_TO` |
| `cron.env` | PostgreSQL, search, Vertex/ADC, `AURA_HISTORIA_CRON_ENABLED_JOBS=search-filter-periodic-match` and schedule |
| `crawler.env` | `LOCAL_DB_URL`, `BUSINESS_DATABASE_URL`, shared stage/TLS, Vertex/ADC, `SPIDER_MAX_SIZE_BYTES` |
| `postgres-ca.pem` | Public trusted CA, mounted read-only at `/run/aura/postgres-ca.pem`; applications must be able to read it |
| `google-adc.json` | Approved ADC, mounted read-only at `/run/aura/google-adc.json`; only API/cron/crawler/percolator/embedding/translation receive this file |
| `postgres.env`, `postgres/` | PostgreSQL bootstrap secret inputs; `postgresql.conf`, referenced HBA and server cert/key files |
| `opensearch.yml`, `opensearch-certs/` | Operator-reviewed security-enabled OpenSearch/node HTTP+transport TLS configuration and certs; analysis files already mounted from repository |
| `redis.conf` | Private authenticated Redis configuration, AOF persistence at `/data`; secret auth values must not be printed |
| `sequin.env`, `sequin.yaml` | Metadata DB and Redis credentials, stable vault/session keys, native Sequin source/subscriptions; not generated by deploy tooling |
| `caddy/Caddyfile` | Local HTTPS route below; directory bind allows future atomic file replacement |

Root-owned0400 ADC files cannot be read by UID10001. Use an explicit readable owner/group or ACL for bind-mounted files while keeping the host parent root-controlled. Runtime PG password-file support, if used, requires exactly0400/0600 and correct readable ownership; these Compose files instead permit passwords inside protected env files. Root/Docker administrators can read container secrets. Replacing files does not refresh existing pools/credentials: recycle affected processes after a separately controlled rotation. No long-lived IAM user-key distribution or generic secret materializer is supplied.

Common application env example (add component-specific fields; blanks need actual values):

```dotenv
STAGE=dev
POSTGRES_SSL_MODE=verify-full
POSTGRES_SSL_ROOT_CERT=/run/aura/postgres-ca.pem
POSTGRES_HOST=postgres
POSTGRES_PORT=5432
POSTGRES_DATABASE=
POSTGRES_USERNAME=
POSTGRES_PASSWORD=
POSTGRES_MAX_CONNECTIONS=2
OPENSEARCH_ENDPOINT_URL=
OPENSEARCH_USERNAME=
OPENSEARCH_PASSWORD=
VERTEX_AI_PROJECT_ID=
VERTEX_AI_LOCATION=
VERTEX_AI_MODEL=
GOOGLE_APPLICATION_CREDENTIALS=/run/aura/google-adc.json
AWS_REGION=
COMMIT_SHA=
```

`POSTGRES_HOST=postgres` is only suitable when the certificate SAN contains that DNS name. Replace with private/remote DNS later; no app code change. Crawler's explicit URLs use separate runtime roles/databases but the same verified-TLS contract. Never use an implicit localhost to reach a different container. Native operational listeners remain container-loopback: API9080, cron8082, crawler9083; crawler review7878 private. Probe them in the container namespace, not through public Caddy. Worker8081 includes ingress/probes and is private-network-only.

Complete API-specific variable names are in `ApiConfig::from_getter` (`src/aura-historia-api/src/lib.rs`): four `AURA_HISTORIA_COGNITO_*` inputs; Stripe API key, checkout/portal URLs and four price IDs; six `ZOHO_*` inputs. Use current crate `AGENTS.md` for exact configuration, not guessed values. Mail objects must already exist at `{stage}/{source_sha}/mjml/{group}/{language}.html`; images do not contain them. Real S3/SES/Cognito/Vertex access is not exercised by the idle double.

All workers need their existing scoped Standard SQS source/DLQ pair. Source retention7d, DLQ14d,20s poll,5 receives, SSE and transport-deny policies; visibility60s for fast scopes,300s for percolator/embedding/translation/normalizer,360s for delivery. Application startup validates these; no queue creation/purge occurs here. Local endpoint overrides are intentionally rejected in dev/prod. Worker env is shared configuration, not a substitute for least-privilege runtime identity.

Budget at default pool settings: business32 + crawler-local16 + cron dedicated advisory session1, plus Sequin metadata/source pools, bootstrap/admin and deployment headroom. Two API slots add another pool. Compose limits sum to substantial memory (apps13GiB before platform); set an explicit host capacity/pool/concurrency budget rather than relying on overcommit. API/worker/cron/crawler stop grace is60/300/330/330s. Increasing runtime deadlines requires increasing Compose and host-command deadlines too.

Local-only Caddyfile used by the test:

```caddyfile
{
    admin off
    auto_https disable_redirects
}
https://localhost {
    tls internal
    reverse_proxy api:8080
}
```

No ACME/DNS changes, public origin, operational routes or CloudFront switch. R4 must define a safe reload/candidate mechanism; this file alone is not A→B orchestration.

## Fresh initialization and real-stage gates

No existing database is disposable by implication. Fresh-only flow: PostgreSQL/extensions → genuine current business/crawler SQLx histories → restricted runtime grants → five-table publication/slot → current OpenSearch definitions → workers → Sequin. Separate schema histories and Sequin metadata owner; runtime roles are never schema owners/superusers. Never stamp history because tables happen to exist.

The test uses existing `bootstrap-local --initialize-fresh business|crawler` **only under its supported local/loopback restriction**. It is not a live-dev migrator; do not relabel real resources `test` to bypass it. A reviewed explicit real-stage fresh initialization procedure remains required before real-host use; no adoption/backfill/down-migration framework is requested.

Additional concrete live gates:

1. Supply reviewed pinned PostgreSQL16/TTL3 image/server TLS/HBA. Base Compose publishes no PostgreSQL port. R5's public Lambda access requires separate approved exact-EIP firewall/publishing work, not broad exposure.
2. Sequin0.14.6 native `ssl:true`/metadata SSL does **not** perform certificate verification. Resolve verified source/metadata transport before enabling real-stage operation; do not call `verify_none` secure. Preserve stable slot/publication and no-backfill sink configuration; Sequin YAML reapplication on restart can change configuration.
3. Initialize OpenSearch security with its shipped tools, real cert trust/runtime permissions, both current mappings and hybrid pipeline. Do not use security-disabled test configuration or assume the PG CA config secures OpenSearch. Verify app→search TLS/auth separately.
4. TTL3 dynamic worker needs explicit `ttl_start_worker()` after a PostgreSQL restart; presence is not expiry proof. No reboot recovery claimed. Restrict TTL functions/public-schema writes; runtime expiry guards remain authoritative.
5. Approved AWS/Google credential delivery/refresh, actual queues, compiled mail assets, FX snapshot, host budgets, backups and real endpoint trust must be supplied/tested. Crawler initial source sync and normalizer reconciliation run immediately; empty fixture behavior is not a scheduling-disable mechanism.

Once these gates and target authority exist, use normal Compose with separate `--project-name`, `--env-file`, and `-f` arguments. `config --quiet` validates without printing secrets; pull/load exact artifacts separately. Never issue platform `down`, `--remove-orphans`, volume recreation, engine upgrade or global image prune during an application release. R4 will own safe replacement and incomplete-state handling.
