# Production host

The production host runs PostgreSQL, OpenSearch, the API, MCP, the workers and the
crawler side by side, plus interactive and agent sessions. cgroup tiers keep one
workload from starving another: CPU and IO weights decide who runs first under
contention, memory caps keep the lower tiers inside a budget, and memory protection
keeps the databases' working sets resident. Nothing is pinned to CPUs; an idle tier
leaves its share to the others.

The files here are installed by [`install.sh`](install.sh) and checked by
[`stress-test.sh`](stress-test.sh). Both need root, cgroup v2 and Docker's systemd
cgroup driver. The prod Compose project, its secrets, scheduled jobs and backups are in
[`prod/`](prod/README.md).

## Tiers

| Slice | Members | CPU / IO weight | Memory |
|---|---|---|---|
| `aura_serving.slice` | PostgreSQL, OpenSearch, API, MCP, Caddy | 1000 | `MemoryLow=28G`, covering the members' reservations |
| `aura_pipeline.slice` | worker, `ops` jobs, the crawler's log shipper | 200 | per-container limits |
| `system.slice` | sshd, Docker, journald and other host services | 100 (default) | unlimited |
| `aura_batch.slice` | crawler units, crawler database, crawler review agents | 50 | `MemoryMax=6G` |
| `user.slice` | interactive sessions, builds, agent sessions | 20 | `MemoryMax=10G` |

A capped tier that reaches its limit first loses page cache, then the kernel kills
the largest process inside that tier, never one outside it. Docker places a
container that names no tier in `user.slice`, so test fixtures and other ad-hoc
containers share the sessions' cap. The 10 GiB for `user.slice` is shared by every
login, so parallel builds may need fewer Cargo jobs. Only a host-wide OOM, when the
uncapped tiers together exhaust memory, can reach the serving tier; negative
`oom_score_adj` values make PostgreSQL and then OpenSearch its last choices.

Tier names use an underscore because a dash in a slice name nests it: an
`aura-serving.slice` would sit inside `aura.slice` and compete through that parent's
default weight and protection.

IO weights take effect only under an IO controller model (BFQ or `io.cost`). This
host's NVMe RAID uses the `none` scheduler, so today CPU weights and memory limits
are what is enforced.

## Memory plan

The plan for 64 GB; tune it from observed usage.

| Service | Tier | Container setting |
|---|---|---|
| PostgreSQL (`shared_buffers=4GB`) | serving | `mem_limit: 14g`, `mem_reservation: 12g`, `oom_score_adj: -900` |
| OpenSearch (8 GiB heap) | serving | `mem_limit: 20g`, `mem_reservation: 16g`, `oom_score_adj: -500` ([compose](../opensearch/prod/compose.yaml)) |
| API, MCP, Caddy | serving | 3 GiB together |
| Worker, `ops` and the crawler's log shipper | pipeline | worker 2 GiB; each `ops` run 768 MiB (cleanup can overlap periodic matching); the log shipper 128 MiB; the occasional restore check 4 GiB ([prod](prod/README.md)) |
| Crawler stack | batch | 6 GiB slice cap; its database `mem_limit: 3g` ([compose](../src/crawler/docker-compose.yml)) |
| Interactive sessions | user | 10 GiB slice cap |
| Host and page cache | — | the rest |

`mem_reservation` becomes the container's `memory.low`, so it protects only while
the parent slice's `MemoryLow` covers it: raise both together.

## Joining a tier

- **Containers** set `cgroup_parent: <tier>.slice` and a `mem_limit`; without a
  parent they run in `user.slice`. Serving containers that hold a working set also
  set `mem_reservation`. PostgreSQL sets the most negative `oom_score_adj`, then
  OpenSearch.
- **systemd units** set `Slice=<tier>.slice`, through a drop-in when the unit file
  is owned elsewhere, as for the crawler units.
- Changing a container's parent or memory settings needs recreation; a unit's new
  `Slice=` applies at its next start.

## Install

From a checkout of a reviewed ref:

1. `sudo host/install.sh cgroups` installs and starts the tier slices, caps
   `user.slice`, adds the crawler units' drop-ins and sets Docker's default parent to
   `user.slice`; `sudo systemctl restart docker` applies the default parent.
2. Start the containers in their tiers, keeping their external volumes: OpenSearch from its
   stable checkout with `docker compose -f opensearch/prod/compose.yaml up -d`, and the
   crawler database with `CRAWLER_POSTGRES_CGROUP_PARENT=aura_batch.slice` set before
   `db-up`. Restart the crawler units after either.
3. `sudo host/stress-test.sh` fills `aura_batch.slice` and `user.slice` past their
   caps and fails if a serving container was OOM-killed or restarted.
4. `sudo host/install.sh ssh` once every SSH user logs in with a key, then
   `sudo host/install.sh firewall`.
5. The [prod project's first start](prod/README.md#first-start), which ends with the deploy
   user (`sudo host/install.sh deploy < deploy.pub`).

`sudo host/install.sh check` prints the effective CPU masks, slice properties,
container placement, SSH authentication settings, firewall rules and public
listeners without changing anything.

## Network baseline

- **SSH accepts keys only**, and root only with a key. The step refuses while an
  account with a terminal login in wtmp or a password login in sshd's journal has
  no `authorized_keys`. Keep the current session open until a new key login
  succeeds; Hetzner's rescue system is the fallback.
- **The `deploy` user** can only run the prod deploy; GitHub Actions logs in as it
  ([deploy access](prod/README.md#deploy-access)).
- **ufw denies inbound** except SSH and 443 (TCP and UDP). 443 accepts every
  source; Caddy refuses requests without CloudFront's
  [origin secret](../docs/infra.md#cloudfront-origin-lock). Docker-published ports
  bypass ufw's input rules, so a `DOCKER-USER` block applies the same rule to new
  connections that reach containers through the external interface. The step
  refuses while ufw holds an older rule it did not create, because enabling ufw
  would activate it unreviewed.
- **Only Caddy publishes a public port (443).** PostgreSQL, OpenSearch and every other
  service bind to loopback or a Compose network. The [prod project's](prod/README.md#caddy)
  Caddy obtains its certificates through the DNS challenge, so port 80 stays closed.

## Disks

To check the wear of both NVMe drives, with `nvme-cli` installed, run `sudo nvme smart-log /dev/nvme0n1` and `/dev/nvme1n1` (`percentage_used`,
`media_errors`, `critical_warning`), and `cat /proc/mdstat` for the RAID state.
