# Self-hosted OpenSearch

## Ownership and state

The backend operator owns the independent host, persistent volume, credentials,
certificates and firewall. The [dev](../opensearch/stage/compose.yaml) and
[production](../opensearch/prod/compose.yaml) Compose projects pin their engine and
resource limits; neither proves the running version or capacity.
A single host is an outage domain, not a high-availability design.
Verify actual host, security, assets and capacity before activation.

PostgreSQL owns business state; OpenSearch contains only rebuildable ProductListing
search state. Saved-filter configuration, matching progress and matches stay in PostgreSQL;
[periodic matching](events/flow.md#periodic-saved-filter-matching) queries product search.
Checked-in mappings and pipeline assets define the product projection. Versioned,
content-free deletion tombstones must survive delayed writes, and readers must exclude them. Never physically delete
fences or configure a TTL to clear them. Rebuild rules belong in the
[projection runbook](durable-worker-runbook.md#projection-fences-and-rebuild).

## Production host setup

Use a reviewed source ref in a dedicated, stable checkout and a fresh volume.
Keep that checkout in place for Docker's configuration and certificate bind mounts;
switch development branches in a separate checkout. The node publishes no public port:
the prod project's application containers reach it at `https://opensearch:9200` on the
shared `search` network and trust its private CA
([prod networks](../host/prod/README.md#networks)). Until the cutover, the
[prod project's Caddy](../host/prod/README.md#caddy) also serves restricted read routes at
`https://opensearch.aura-historia.com` for the AWS runtime; that public site is removed
afterwards. Administration is available
only at `https://localhost:19201` with the private operator client certificate.
Transport/node/admin keys and runtime credentials are generated independently of
dev; production does not load demo certificates or demo users.

The [production bootstrap](../opensearch/prod/bootstrap.py) requires Python 3,
OpenSSL, Docker Compose and the normal OpenSearch host prerequisites below. It
checks for at least 100 GiB free Docker storage and enough available RAM for the
configured container limit plus 8 GiB host headroom. The container runs without
swap; its heap and memory limits follow the host's memory plan.

### Production resource allocation

OpenSearch runs in the host's serving tier, `aura_serving.slice`, with no CPU
pinning or quota: CPU and IO weights rank it with PostgreSQL above the pipeline,
batch and interactive tiers, and its memory reservation is protected from their
pressure. The [host tiers](../host/README.md) own the slices, the memory plan and
their verification. Install the tiers before first bootstrap. Changing the
container's parent slice or memory settings requires recreation and a brief search
outage; preserve its existing external volumes.

### Production storage and bootstrap

The independent external Docker volumes survive Compose removal and grow on the
host filesystem as data grows. They are not a dedicated disk or a space quota.
Allocation/rebalancing stops at the configured 60/50 GiB free disk watermarks;
the 40 GiB flood stage blocks index writes. Other host processes can consume that
headroom, so monitor the host filesystem and the node together. Dedicated space
guarantees require separate storage or filesystem quotas. Never prune these
production volumes or remove deletion fences to reclaim space.

From the repository root:

```sh
python3 opensearch/prod/bootstrap.py
docker compose -f opensearch/prod/compose.yaml up -d
```

Then copy the two runtime credential pairs and the node's CA certificate from `.private/`
into the prod project's [secret files](../host/prod/README.md#secrets).

Bootstrap creates only its own labelled volumes, initializes security once, installs
the shared roles/pipeline and creates the empty product index. It refuses unknown
existing volumes and an existing product index. An interrupted first initialization
needs inspection before retrying; never delete populated storage to make it retry.
Subsequent completed invocations perform read-only verification. Ordinary restarts
use Compose, without replaying security initialization or index creation.

The ignored mode-0700 `opensearch/prod/.private/` directory owns the operator CA/key,
client certificate, initialization identity and mode-0600 runtime credentials.
Keep it in access-controlled operator backups; never commit or print its contents.
For local administration use `curl --config opensearch/prod/.private/admin-curl`
with the loopback URL, without disabling TLS verification. Check private certificate
expiry before the 825-day node/admin lifetime and rotate certificates through a
reviewed restart. The private admin key is not mounted into the running search node or
the proxy.

The verifier checks trusted local TLS, a green single-node cluster, reader search, denied
reader writes and denied projector reads. Initialization exercises one non-business
document before activation and removes it, leaving an empty product index. Neither
runtime credential grants search administration.
The application's `/v1/ready` queries OpenSearch, so a deployed API that is ready has
reached the node over the `search` network with its reader credential.

## Dev host setup

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
CLI arguments. PUT the checked-in [role files](../opensearch/roles/) to
`/_plugins/_security/api/roles/aura_stage_<role>` and privately create distinct
random-password users mapped only to their role. Role names use `reader` and
`product_projector` (filenames use hyphens). The reader searches ProductListings;
the product projector indexes only ProductListing documents. Neither receives
administration or index-management rights.
Single-document indexing also requires cluster permission `indices:data/write/bulk`
and index-level `indices:data/write/bulk*`, with document writes scoped to the
product index. The vector mapping explicitly selects 16-bit FP16 quantization.
Do not substitute the broader `write`
or `index` action groups: they also grant mapping or other document-write rights.

Only after confirming the product index/pipeline are absent, apply the fresh-install assets:

```sh
curl --config opensearch/stage/.admin-curl --fail-with-body -H 'Content-Type: application/json' -X PUT --data-binary @opensearch/hybrid-search-pipeline.json https://127.0.0.1:19200/_search/pipeline/hybrid-search-pipeline
jq '.settings += {"number_of_shards":1,"number_of_replicas":0}' opensearch/mappings/product_listings.json | curl --config opensearch/stage/.admin-curl --fail-with-body -H 'Content-Type: application/json' -X PUT --data-binary @- https://127.0.0.1:19200/product-listings
```

Compose preserves its data volume; **never use `down -v`** on populated storage.
Do not recreate populated indices, reset aliases or restore old security backups
as part of an application release.

## Dev public TLS

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
succeed with trusted TLS, and reader writes/admin routes to be denied. Verify product-projector
permissions separately with safe fixtures.

## Release and schema-change contract

Host, security, asset and engine changes are manual operator operations. The
[Deploy workflow](../host/prod/README.md#deploys), migrations and `ops` commands do not
administer OpenSearch and hold no administration credentials; runtime roles never gain
admin rights. A new host follows this guide before the prod project's
[first start](../host/prod/README.md#first-start), which needs the node running; changes to
what feeds the projection follow [outbox source changes](durable-worker-runbook.md#outbox-source-changes).

Before deploying a commit, compare its `opensearch/**` changes since the deployed commit
with the node's actual mapping, alias and pipeline; repository assets are desired state.
Apply compatible reader changes and additive mappings through the loopback administration
endpoint before deploying the writers that use them, while the old readers and projectors
still run. Record the sanitized resulting mapping/alias and verify old/new compatibility
before the deploy. Use the mode-0600 `.private/admin-curl` file; never put passwords in
commands or output. A missing fact holds the deploy until the owner supplies it.

Existing field/type/settings changes, alias cutovers and uncertain compatibility are
incompatible. They require a
[fenced new-generation rebuild](durable-worker-runbook.md#projection-fences-and-rebuild)
from PostgreSQL and retained deletion facts, which
[`ops reproject-all --generation <g>`](../src/aura-historia-ops/README.md) reconstructs, with
catch-up and a checked cutover. Engine upgrades need pinned-image/adapter review, an
outage/restart plan and post-restart version/health/security checks.

First-time bootstrap, incompatible cutovers, fencing/draining, destructive rebuilds and
engine transitions use their dedicated approved procedure. Never recreate a populated
index, discard tombstones, expose administration, grant runtime admin rights or downgrade a
migrated volume. Application rollback, a deploy of an earlier commit, and migration retries
do not undo database, queue or search effects.

## Monitoring and recovery

Monitor cluster/index health, restarts, disk/inodes, JVM/native memory, TLS/auth
failures, certificate expiry and product-search/projection latency. Configure
alerts from measured capacity and recovery headroom, not the Compose limits alone.
Validate representative populated-data load and rebuild time before broad activation.
The node is not backed up ([backups](../host/prod/README.md#backups)): reconstruct it with
`ops reproject-all` under the projection fence rules. Preserve failure custody under
[controlled recovery](durable-worker-runbook.md#failure-custody-and-controlled-redrive).

PostgreSQL is authoritative; OpenSearch is a rebuildable, eventually consistent read
projection. Product projector acknowledgment means a write was accepted or external
versioning fenced it as stale. Search visibility follows the normal refresh cycle. The
projector has document-write permissions; the reader only searches. Monitor queue/projection
lag, backlog age, failure custody, version/fence correctness and rebuildability alongside
cluster health.
