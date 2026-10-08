# Stage OpenSearch

## Ownership and state

The backend operator owns the independent host, persistent volume, credentials,
certificates and firewall. [`compose.yaml`](../opensearch/stage/compose.yaml) pins
the engine and resource limits; it is not proof of the running version or capacity.
A single stage host is an outage domain, not a production availability design.
Verify actual host/DNS, security, assets, capacity and AWS connectivity before activation.

PostgreSQL owns business state; OpenSearch is rebuildable. Checked-in mappings and
pipeline assets define the projection. Versioned, content-free deletion tombstones
must survive delayed writes, and readers must exclude them. Never physically delete
fences or configure a TTL to clear them. Rebuild rules belong in the
[projection runbook](durable-worker-runbook.md#projection-fences-and-rebuild).

## Reproducible host setup

Use a reviewed source ref. Require Docker Engine/Compose, `jq`, persistent disk,
spare RAM for the configured container limit and `vm.max_map_count >= 262144`.
Docker socket/group access is root-equivalent. From the repository root, privately
create mode-0600 `opensearch/stage/.env` with a strong random
`OPENSEARCH_INITIAL_ADMIN_PASSWORD`. Keep `.env`, `.admin-curl`, `.credentials/`
and certificate keys out of Git, tickets and logs; backups must be access-controlled.
Docker operators can inspect environment values; admin credentials are not runtime credentials.

```sh
docker compose --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml up -d opensearch
```

On a **fresh volume**, demo security is reachable only via loopback. Before publishing
the gateway, replace admin/reserved `kibanaserver` passwords and remove unused demo
users. Reserved accounts require the image's `securityadmin.sh`: back up current
`internalusers`, modify only the intended hashes and apply only `-t internalusers`,
not the whole demo configuration. Preserve existing runtime users. Sync `.env`, restart
and verify old credentials fail; changing the initial-password variable alone is not rotation.

Use a mode-0600 `.admin-curl` file with Basic credentials; `insecure` is allowed **only
for loopback administration** at `https://127.0.0.1:19200`. Never pass passwords on
CLI arguments. PUT the checked-in [role files](../opensearch/stage/roles/) to
`/_plugins/_security/api/roles/aura_stage_<role>` and privately create distinct
random-password users mapped only to their role. Role names use `reader`,
`product_projector`, `filter_projector`, `percolator` (filenames use hyphens).
Reader searches; projectors index only their own documents; percolator searches
filters and manages PITs. None receives administration or index-management rights.

Only after confirming the indices/pipeline are absent, apply the fresh-install assets:

```sh
curl --config opensearch/stage/.admin-curl --fail-with-body -H 'Content-Type: application/json' -X PUT --data-binary @opensearch/stage/hybrid-search-pipeline.json https://127.0.0.1:19200/_search/pipeline/hybrid-search-pipeline
jq '.settings += {"number_of_shards":1,"number_of_replicas":0}' opensearch/mappings/product_listings.json | curl --config opensearch/stage/.admin-curl --fail-with-body -H 'Content-Type: application/json' -X PUT --data-binary @- https://127.0.0.1:19200/product-listings
jq '.settings += {"number_of_shards":1,"number_of_replicas":0}' opensearch/mappings/user_search_filters.json | curl --config opensearch/stage/.admin-curl --fail-with-body -H 'Content-Type: application/json' -X PUT --data-binary @- https://127.0.0.1:19200/user_search_filters
```

Compose preserves its data volume; **never use `down -v`** on populated storage.
Do not recreate populated indices, reset aliases or restore old security backups
as part of an application release.

## Public TLS and AWS gate

Caddy exposes restricted application routes on
`https://opensearch.stage.aura-historia.com:9443`, never security/admin APIs.
Do not publish cluster transport or the loopback admin port. Self-signed node TLS
and Caddy's skip-verify setting are confined to the internal Docker bridge;
**public clients must verify the trusted chain and hostname**, never use `-k`.

Private `opensearch/stage/certs/` stores Certbot state. Mount the whole tree read-only
into Caddy so `live/` symlinks reach `archive/`. If using `OPENSEARCH_CERT_DIR`, ensure
it matches Certbot's output location. Caddy does not own challenge ports 80/443.
For new issuance, verify DNS points to the approved host and coordinate a reachable
port-80 window with its owner; use approved DNS-01 if that cannot be arranged:

```sh
docker compose --profile cert --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml run --rm --service-ports certbot certonly --standalone --preferred-challenges http --non-interactive --agree-tos --register-unsafely-without-email --config-dir /etc/letsencrypt --work-dir /etc/letsencrypt/work --logs-dir /etc/letsencrypt/logs -d opensearch.stage.aura-historia.com
```

Monitor actual expiry and schedule renewal well before it; this setup registers no
notification email. During an approved port-80 window, renew and restart the gateway:

```sh
docker compose --profile cert --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml run --rm --service-ports certbot
docker compose --profile public --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml restart gateway
```

Allow time for renewal; verify exit status and any orphaned container after timeout
before releasing port 80. Do not stop other host services without coordination.
Verify actual certificate expiry, chain and hostname after restart. On first setup:

```sh
docker compose --profile public --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml up -d gateway
```

Require unauthenticated/wrong-credential requests to fail, a scoped reader search to
succeed with trusted TLS, and reader writes/admin routes to be denied. Verify projector
and PIT permissions separately with safe fixtures. Host-local checks do not prove AWS access.

### Dev private-subnet egress and release gate

AWS uses **`dev`**, not `stage`: `/opensearch/dev/endpoint-url` selects this service,
with distinct `reader`, `product-projector`, `filter-projector` and `percolator`
credential pairs. Do not recreate shared dev credentials or change unrelated stage
parameters. Current CDK references require SSM `String` values, including passwords;
restrict SSM/function-configuration reads and never expose values in transcripts.
Rotate host credentials and affected deployed consumers together before revoking old ones.

[`config.ts`](../infra/src/config.ts) declares destination CIDRs and
[`network.ts`](../infra/src/constructs/network.ts) permits dev application TCP 9443
through NAT. Verify owner approval, current DNS/routing and the **deployed** rule
before rollout. Security groups do not follow DNS changes. Coordinate host rotation
with CIDR/configuration and TLS changes; never broaden to `0.0.0.0/0`, use a raw-IP
URL or disable verification. Host-side NAT-EIP allowlisting is not a stage prerequisite.

Use an approved private probe path before `scope=all` activates consumers; if it is
unavailable, hold activation. Record effective endpoint/role references, rule/CIDR
and sanitized private-workload readiness, reader, projector and percolator results
as applicable, including Fargate when used. Synth, SSM inspection, local curl or a
waived standalone NAT test are not workload evidence. Label failed/omitted/waived
checks explicitly; repair DNS/SG/NAT/TLS/auth and retest before dependent cutover.

## Release and schema-change contract

Host/security/assets/upgrades are manual, separately approved operations; Deploy,
Migrate and Initialize do not manage OpenSearch. Follow the
[first-time release](infra.md#first-time-stage) and
[consumer handoff](durable-worker-runbook.md#activation-and-legacy-handoff), without
using compute creation as a readiness probe. Runtime roles must never gain admin rights.

Compare release assets with actual state. Approve compatible readers/additive mappings
before writers; incompatible changes require a
[fenced new-generation rebuild](durable-worker-runbook.md#projection-fences-and-rebuild)
from PostgreSQL and retained deletion facts, with catch-up and checked cutover.
No automated rebuild tool is implied. Engine upgrades need reconstruction/backup
verification, pinned-image/adapter review, an outage/restart plan and post-restart
version/health/security checks. Never downgrade a migrated volume.

The [migration-source guard](infra.md#routine-and-schema-dependent-releases) does not
cover OpenSearch-only changes. Application rollback or rerunning Migrate cannot undo
index/security changes; recovery and prod host approval are separate decisions.

## Monitoring and recovery

Monitor cluster/index health, restarts, disk/inodes, JVM/native memory, gateway
TLS/auth failures, certificate expiry and search/percolation latency. Configure
alerts from measured capacity and recovery headroom, not the Compose limits alone.
Validate representative populated-data load and rebuild time before broad activation.
Reconstruct under the projection fence rules; use off-host snapshots only with tested
restore. Preserve failure custody under [controlled recovery](durable-worker-runbook.md#failure-custody-and-controlled-redrive).
