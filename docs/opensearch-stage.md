# Stage OpenSearch (issue #1782)

## Ownership and state

The backend operator owns the **independent** Hetzner host, persistent Docker volume, certificates and credentials. At the 2026-09-25 inspection (`de161a2b68bb0f82fe8e691475ad5b791c2e66f6`), `opensearch/stage/compose.yaml` pinned a single OpenSearch **3.8.0** node by digest (16 GiB container / 6 GiB heap). Authenticated HTTPS is loopback-only at `127.0.0.1:19200`; Caddy exposes restricted application routes on 9443, not cluster transport or admin APIs. The volume-preserving upgrade from an empty 3.1.0 installation left both application indices green. A single host is an accepted stage outage domain, not a production availability design.

**Index contract:** `product-listings` and `user_search_filters` have one primary, zero replicas; `hybrid-search-pipeline` is installed. Mappings include five built-in language analyzers, 768-dimensional Faiss HNSW fp16 vectors and percolator fields. PostgreSQL owns business state and embeddings; OpenSearch is rebuildable ([architecture §12](arch.md#12-cdc-and-projection-architecture)). Both indices use permanent, content-free, externally versioned `projectionDeleted: true` tombstones, not physical deletes or TTLs; all search and percolation readers exclude them. ProductListing writes use `product_listings.projection_version`; withdrawals tombstone at that version and restores need a newer one. Filter writes use `search_filters.version`; deletes tombstone at `version + 1`. Older/equal writes cannot replace newer state. See [ProductListing](events/flow.md#canonical-productlisting-opensearch-projection), [filters](events/flow.md#canonical-search-filter-opensearch-projection) and [fenced rebuild](durable-worker-runbook.md#projection-fences-and-rebuild) for full source/version and deletion-history rules. Do not configure unbenchmarked compression or another search service.

**Observed stage status (2026-09-25):** DNS A `148.251.91.20`; `https://opensearch.stage.aura-historia.com:9443` has trusted TLS expiring **2026-12-24 15:12 UTC**. Local HTTPS/auth checks passed; this is not AWS connectivity evidence. AWS uses `dev`, not `stage`: the endpoint and four scoped credential pairs are in `eu-central-1` SSM. No active dev CloudFormation stack or Lambda was found. The owner waived a standalone private-subnet/NAT connection test, **not** workload smoke. Capacity testing and search-dependent activation remain open.

## Reproducible host setup

Require Docker Engine/Compose, `jq`, >=16 GiB spare RAM, persistent disk and `vm.max_map_count >= 262144` (host: 16438223). Restrict Docker socket/group access as root-equivalent. From repo root create mode-0600 `opensearch/stage/.env` with a strong random `OPENSEARCH_INITIAL_ADMIN_PASSWORD` (upper/lowercase, digits, punctuation). Git-ignored `.env`, `.admin-curl`, `.credentials/` and `certs/` need access-controlled backups, never Git/tickets/logs. Docker operators can inspect container environment variables; keep admin and AWS runtime credentials separate.

```sh
docker compose --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml up -d opensearch
```

On a **fresh volume**, the bundled demo security config is loopback-only. Before starting `gateway`, replace the admin password, rotate reserved `kibanaserver` and delete unused demo users. Reserved accounts require the image's `securityadmin.sh`: back up current `internalusers`, change only those hashes and reapply only `-t internalusers`, never the entire demo config or runtime users. Sync `.env`, restart and verify the old password fails (completed on this host). The node's self-signed HTTPS and gateway `tls_insecure_skip_verify` are **internal Docker-bridge only**; public clients must verify trusted TLS.

Use mode-0600 `opensearch/stage/.admin-curl` with Basic credentials and `insecure` **only for loopback**; never pass passwords on the command line. On a new install, PUT `opensearch/stage/roles/*.json` to `/_plugins/_security/api/roles/aura_stage_<role>` and privately create distinct random-password `aura_stage_<role>` users, each with `opendistro_security_roles: ["aura_stage_<role>"]`. Roles: `reader`, `product_projector`, `filter_projector`, `percolator` (role filenames use hyphens). Reader searches both indices; projectors index only their respective documents; percolator searches filters and creates/closes PITs. None has admin, index-management or unrelated-index access. Rotate user credentials and SSM references as a controlled operation, not a Lambda release.

After checking that the indices/pipeline do **not** exist, apply the assets once, from the repo root:

```sh
curl --config opensearch/stage/.admin-curl --fail-with-body -H 'Content-Type: application/json' -X PUT --data-binary @opensearch/stage/hybrid-search-pipeline.json https://127.0.0.1:19200/_search/pipeline/hybrid-search-pipeline
jq '.settings += {"number_of_shards":1,"number_of_replicas":0}' opensearch/mappings/product_listings.json | curl --config opensearch/stage/.admin-curl --fail-with-body -H 'Content-Type: application/json' -X PUT --data-binary @- https://127.0.0.1:19200/product-listings
jq '.settings += {"number_of_shards":1,"number_of_replicas":0}' opensearch/mappings/user_search_filters.json | curl --config opensearch/stage/.admin-curl --fail-with-body -H 'Content-Type: application/json' -X PUT --data-binary @- https://127.0.0.1:19200/user_search_filters
```

Never recreate populated indices, reset aliases, delete tombstones or reapply old security backups during deployment. Mapping changes/rebuilds need a controlled #1803 plan. `docker compose up` preserves `stage_data`; **never use `down -v`** on populated storage. Engine upgrades require a planned outage, verified PostgreSQL reconstruction inputs/backup, pinned-image and adapter review, explicit Compose change/restart, and post-restart version/green/security checks. Never downgrade a migrated volume.

## Release and schema-change contract

Host setup, mappings, security and upgrades are **manual operator operations**, never GitHub deployment steps. There is no OpenSearch migration runner or public admin route; runtime roles must not acquire admin rights. `Migrate (CD)` changes only PostgreSQL; `Initialize (CD)` handles FX, and Deploy does not apply OpenSearch assets.

Follow the [first-time stage sequence](../infra/README.md#first-time-stage): foundation → Migrate Postgres → host setup if needed → Initialize FX → same-ref Deploy `scope=all`. Create assets from the selected ref only on a fresh install; otherwise check compatibility. Before `all`, record owner approval and index/mapping/pipeline, role, TLS, endpoint, network and [native-consumer handoff](durable-worker-runbook.md#activation-and-legacy-handoff) evidence. Hold if a required check is unavailable: compute creation activates mappings, not a readiness probe or workflow marker.

For later engine/mapping changes, compare release assets with actual state and approve a resource-specific plan, sanitized pre/post checks and recovery. Apply compatible readers and additive mappings before writers. Incompatible changes require [fenced new-generation rebuild and alias cutover](durable-worker-runbook.md#projection-fences-and-rebuild) using PostgreSQL and retained deletion facts; no automated tool is implied. Follow the [release procedure](../infra/README.md#routine-and-schema-dependent-releases) for migration-source changes (auto stops at foundation; forward Migrate then same-ref `all`). That guard does **not** detect OpenSearch-only changes; cancel blocked auto runs before manual gating to avoid stage-concurrency deadlock. Rollback or rerunning Migrate cannot reverse index/security changes. Prod needs separate host approval and evidence.

## Public TLS and AWS gate

Caddy serves `https://opensearch.stage.aura-historia.com:9443`, separate from `internal-agent.aura-historia.com` on 80/443/8080. It routes only application search/projection/percolation, never admin APIs. Private, Git-ignored `opensearch/stage/certs/` holds Certbot state (including `live/` symlinks to `archive/`); mount the whole directory read-only, not just `live/`. `OPENSEARCH_CERT_DIR` overrides the bind source. Caddy does not perform ACME challenges or bind 80/443.

To issue on a **new** host, first ensure the A record points at that host and port 80 is available and reachable. Then, from repo root:

```sh
docker compose --profile cert --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml run --rm --service-ports certbot certonly --standalone --preferred-challenges http --non-interactive --agree-tos --register-unsafely-without-email --config-dir /etc/letsencrypt --work-dir /etc/letsencrypt/work --logs-dir /etc/letsencrypt/logs -d opensearch.stage.aura-historia.com
```

The pinned Certbot 5.8.0 image publishes port 80 **only during the command**. Issuance and `certonly --dry-run` passed locally. No notification email is registered; monitor expiry. Coordinate a port-80 window with its owner or use DNS-01.

Before **2026-11-24**, schedule the next operator renewal window, then (from repo root) use:

```sh
docker compose --profile cert --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml run --rm --service-ports certbot
docker compose --profile public --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml restart gateway
```

`renew` may delay startup (439 seconds observed); allow for it and check exit status. A 120-second renewal dry run timed out and **has not passed**. After a timeout, stop any orphaned `certbot` container to release port 80; the current certificate is unaffected. Verify new expiry and trusted HTTPS after renewal. If port 80 is unavailable, use approved DNS-01; never stop `internal-agent` without coordination. Gateway restart picks up renewed symlinks; no app release is needed.

```sh
docker compose --profile public --env-file opensearch/stage/.env -f opensearch/stage/compose.yaml up -d gateway
curl --max-time 5 https://opensearch.stage.aura-historia.com:9443/product-listings/_search
```

Unauthenticated requests must return 401; trusted-TLS reader search 200, wrong credentials 401, and reader writes/admin routes 403. Local checks also confirmed hostname/chain verification; never use `-k` on the public endpoint. Stage does not require a **host-side inbound source-IP** allowlist while TLS/auth hold; dev AWS still requires the outbound destination-CIDR rule. Never publish 9300 or the loopback admin port. The host operator owns firewall and certificate-expiry monitoring.

AWS **dev**, not `stage`, reads `/opensearch/dev/endpoint-url` = `https://opensearch.stage.aura-historia.com:9443` and four distinct SSM pairs at `/opensearch/dev/{reader,product-projector,filter-projector,percolator}/{username,password}`. Keep `/opensearch/stage` unchanged; do not recreate retired shared `/opensearch/dev/{username,password}`. On 2026-09-25 in `eu-central-1`, role pairs were at version 1 and the endpoint at version 2. CDK uses `{{resolve:ssm:...}}` (not `ssm-secure`), so passwords are `String` parameters: restrict IAM/operator reads; Secrets Manager would require a CDK contract change. Never put values in Git, CLI arguments, issues or logs. Hold first `Deploy scope=all` until search checks and capacity validation; for existing compute, fence affected mappings under separate approval if needed. No crawler work is included.

### Dev private-subnet egress and release gate (#1850)

`infra/src/constructs/network.ts` allows `ApplicationSecurityGroup` outbound TCP 443 as before and **dev-only** TCP 9443 to `148.251.91.20/32` via NAT (`infra/src/config.ts`); prod and other groups are unchanged. This synthesized rule is **not deployed-connection evidence**. Never broaden 9443 to `0.0.0.0/0` or use raw IP/disabled hostname or chain verification: AWS clients use the DNS-name HTTPS URL. The private #1843 periodic matcher uses this same group, not a new rule.

The 2026-09-25 DNS A result `148.251.91.20` does **not** prove host ownership, stable DNS or AWS reachability. Before deployment, get host/network owner CIDR approval and verify current A records and gateway routing; if they differ, update `infra/src/config.ts` and this runbook. Without an approved stable destination, hold network deployment and search-dependent rollout. Coordinate DNS/CIDR/TLS on host rotation. The owner waived only the **standalone** private-subnet/NAT connection test; **host NAT EIP allowlisting is not mandatory**. This is neither AWS workload acceptance nor a prod precedent.

For #1802/#1805 record deployed TCP 9443 rule/CIDR, effective endpoint and role references, and sanitized private-workload `/api/v1/ready` (PostgreSQL plus permitted zero-result `product-listings` search), reader, projector and percolator smoke as applicable. Require approved pre-activation access: if unavailable, **hold `scope=all`** rather than activate mappings as a probe. After deployment verify TLS hostname and chain; host-local curl, synth and SSM inspection are not AWS workload smoke. Record failed, omitted and waived checks as such. Hold activation/cutover on missing rule or failed probe; leave affected mappings off, repair DNS/SG/NAT/TLS/auth and retest. See [first-time release](../infra/README.md#first-time-stage) and [consumer handoff](durable-worker-runbook.md#activation-and-legacy-handoff) for general rollout rules.

## Monitoring, recovery and evidence gates

Monitor loopback `/_cluster/health`, `/_nodes/stats/jvm,fs`, index status, Docker restarts, disk/inodes, JVM/native RSS, gateway HTTPS 401/200, certificate expiry and p95 search/percolation latency. Alert on outages, repeated restarts, disk >80%, sustained heap >75%, certificate <30 days and sustained timeouts/latency. Reconstruct from PostgreSQL (#1803), preserving [deletion fences](durable-worker-runbook.md#projection-fences-and-rebuild); consider off-host snapshots only with tested restore. Keep recovery disk headroom and measure rebuild time. The 16 GiB / 6 GiB limits are not capacity or recovery guarantees: representative counts and latency must be measured in #1786 before broad activation. Host operation and backup have ongoing cost.

**Evidence (2026-09-25, local only):** 3.8.0 upgrade preserved green indices (2 primaries, 0 unassigned); Docker RSS 6.766 GiB / 16 GiB. Reader/projector/percolator permission checks, empty-index hybrid RRF, 768-dimensional KNN and filter percolation passed; invalid PIT ID returns 500 and cannot test close permission. `test-api` passed 7 tests; combined adapter run timed out after 68 `product-listing-opensearch` passes and 9/10 observed `search-filter-opensearch` passes. `npm --prefix infra test`, `npm --prefix infra run synth:all` and `cargo depgraph-check check` passed. Public hostname/chain verified from the host; AWS dev SSM names/types/versions inspected without values. Remaining adapter test, populated-data latency, AWS workload smoke and dev Lambda/consumer activation are **unverified**; the standalone NAT check is **waived**, not passed.
