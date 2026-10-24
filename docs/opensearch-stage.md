# Self-hosted OpenSearch

## Ownership and state

The backend operator owns the independent host, persistent volume, credentials,
certificates and firewall. The [dev](../opensearch/stage/compose.yaml) and
[production](../opensearch/prod/compose.yaml) Compose projects pin their engine and
resource limits; neither proves the running version or capacity.
A single host is an outage domain, not a high-availability design.
Verify actual host/DNS, security, assets, capacity and AWS connectivity before activation.

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
switch development branches in a separate checkout. Production uses
`https://opensearch.aura-historia.com` on HTTPS 443, which already matches the AWS
application egress policy. Point its DNS A record directly at the approved host,
with Cloudflare proxying disabled, and keep TCP 80/443 reachable for Caddy's
automatic public certificate issuance and renewal. Administration is available
only at `https://localhost:19201` with the private operator client certificate.
Transport/node/admin keys and runtime credentials are generated independently of
dev; production does not load demo certificates or demo users.

The [production bootstrap](../opensearch/prod/bootstrap.py) requires Python 3,
OpenSSL, Docker Compose and the normal OpenSearch host prerequisites below. It
checks for at least 100 GiB free Docker storage and enough available RAM for the
configured container limit plus 8 GiB host headroom. The default container limit
is 32 GiB, with a 16 GiB heap and no swap. Its four-CPU quota and increased CPU
weight cap and prioritize work; they do not exclusively reserve cores.

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
docker compose -f opensearch/prod/compose.yaml --profile public up -d
# Wait for public TLS issuance, then verify and publish only runtime credentials:
python3 opensearch/prod/bootstrap.py --verify-public --publish-ssm
```

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
reviewed restart; Caddy renews its public certificate independently. The private
admin key is not mounted into the running search node or public gateway.

The verifier checks trusted local/public TLS, reader search, denied reader writes,
denied projector reads and blocked public administration. Initialization exercises
one non-business document before public activation and removes it, leaving an empty
product index. Publishing writes only the endpoint and two scoped credential pairs
to `/opensearch/prod/` in the approved workload account. It does not grant a pipeline
or application search administration access. Fresh sanitized mapping/alias evidence
is saved in `.private/observation.json` for release review.

These checks prove host/public behavior. The private Lambda/Fargate readiness gate
below is still required after AWS platform creation and before application activation.

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
succeed with trusted TLS, and reader writes/admin routes to be denied. Verify product-projector
permissions separately with safe fixtures. Host-local checks do not prove AWS access.

### Dev private-subnet egress and release gate

AWS uses **`dev`**, not `stage`: `/opensearch/dev/endpoint-url` selects this service,
with distinct `reader` and `product-projector` credential pairs. Do not recreate shared
dev credentials or change unrelated stage parameters. Current CDK references require SSM `String` values, including passwords;
restrict SSM/function-configuration reads and never expose values in transcripts.
Rotate host credentials and affected deployed consumers together before revoking old ones.

[`config.ts`](../infra/src/config.ts) declares destination CIDRs and
[`network.ts`](../infra/src/constructs/network.ts) permits dev application TCP 9443
through NAT. Verify owner approval, current DNS/routing and the **deployed** rule
before rollout. Security groups do not follow DNS changes. Coordinate host rotation
with CIDR/configuration and TLS changes; never broaden to `0.0.0.0/0`, use a raw-IP
URL or disable verification. Host-side NAT-EIP allowlisting is not a stage prerequisite.

Use an approved private probe path before platform activation enables consumers; if it is
unavailable, hold activation. Record effective endpoint/role references, rule/CIDR
and sanitized private-workload readiness, product-reader and projector results,
including periodic-matcher Fargate access. Synth, SSM inspection, local curl or a
waived standalone NAT test are not workload evidence. Label failed/omitted/waived
checks explicitly; repair DNS/SG/NAT/TLS/auth and retest before dependent cutover.

## Release and schema-change contract

Host/security/assets/upgrades remain manual operations; the delivery pipeline and
scoped database operations do not administer OpenSearch. Follow the
[first-time release](infra.md#first-time-stage) and
[consumer handoff](durable-worker-runbook.md#activation-and-legacy-handoff), without
using compute creation as a readiness probe. Runtime roles must never gain admin rights.
On existing hosts, reconcile gateway routes, runtime users and role mappings against the
approved assets. Retire unused identities and indices only after ownership, in-flight work
and recovery posture are settled; removing source assets does not decommission live state.

Compare release assets with actual state. Approve compatible readers/additive mappings
before writers; incompatible changes require a
[fenced new-generation rebuild](durable-worker-runbook.md#projection-fences-and-rebuild)
from PostgreSQL and retained deletion facts, with catch-up and checked cutover.
No automated rebuild tool is implied. Engine upgrades need reconstruction/backup
verification, pinned-image/adapter review, an outage/restart plan and post-restart
version/health/security checks. Never downgrade a migrated volume.

The release compatibility report compares every tracked `opensearch/**` transition
from the target's last verified application release through the complete candidate.
Repository assets are desired state; sanitized current mapping/alias observations are
required. Missing private facts hold the routine path until the owner supplies evidence.
Existing field/type/settings changes, alias/rebuild cutovers and uncertain compatibility
require maintenance. A model verdict cannot override those restrictions.

For a compatible mapping addition the approval summary prominently displays:

**BLOCKING MANUAL OPENSEARCH PREREQUISITE — COMPLETE BEFORE APPROVING aws-<target>**

The checklist names the target host/index/alias, source-asset SHA256 values, reviewed
host-local patch, ordered commands, preconditions, expected resulting mapping digest,
verification and stop conditions. Use a mode-0600 `.admin-curl` credential file; never
put passwords in commands/output. Complete the action while old readers/projectors run,
record sanitized actual mapping/alias evidence, and verify old/new compatibility and
private workload readiness. The protected target approval attests the exact target,
bundle, report and prerequisite digest. Replacing a prerequisite or observing an
unplanned change invalidates approval and requires a fresh report.

Execution revalidates the expected **after** mapping and alias state against the approved
transition. A successful manual addition is not treated as unexplained baseline drift.
Read-only postchecks corroborate available observations; pipelines receive no search
administration credentials, SSH automation or exposed administrative endpoint. A missing
or incorrect completion attestation blocks mutation.

First-time bootstrap, incompatible cutovers, fencing/draining, destructive rebuilds and
engine transitions use their dedicated approved procedure and the common maintenance
hold. Never recreate a populated index, discard tombstones, expose administration,
grant runtime admin rights or downgrade a migrated volume. Application/alias rollback
and migration retries do not undo database, queue or search effects.

## Monitoring and recovery

Monitor cluster/index health, restarts, disk/inodes, JVM/native memory, gateway
TLS/auth failures, certificate expiry and product-search/projection latency. Configure
alerts from measured capacity and recovery headroom, not the Compose limits alone.
Validate representative populated-data load and rebuild time before broad activation.
Reconstruct under the projection fence rules; use off-host snapshots only with tested
restore. Preserve failure custody under [controlled recovery](durable-worker-runbook.md#failure-custody-and-controlled-redrive).

PostgreSQL is authoritative; OpenSearch is a rebuildable, eventually consistent read
projection. Product projector acknowledgment means a write was accepted or external
versioning fenced it as stale. Search visibility follows the normal refresh cycle. The
projector has document-write permissions; the runtime gateway exposes search and document
routes. Monitor queue/projection lag, backlog age, failures/DLQ, version/fence correctness
and rebuildability alongside cluster health.
