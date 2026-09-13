# Single-host replacement — R4

`deploy` is a Python3 stdlib command, not a deployment framework. Linux flock + ordinary Compose/Caddy + `current`, `previous`, `incomplete`. It uses **preloaded immutable local image IDs only**, never pulls/rebuilds historical source, changes schemas, purges queues, restarts platform services or issues cloud-control-plane commands.

**Proven locally:** real image A→B, empty-worker/job stop-before-start, bad candidate isolation, incomplete/recovery guards and flock contention. **Not live-ready:** real SQS custody, active-work drain, reboot, provider trust/credentials and the R3 real-stage gates remain outstanding. No GitHub workflow is enabled.

## Reproduce isolated acceptance

Requires R3 fixture images, local Linux/amd64 Docker/Compose, Python3 and four B images below. No live target/account or credentials. From repository root:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s deploy/tests -p 'test_*.py' -v
PYTHONDONTWRITEBYTECODE=1 python3 deploy/tests/smoke-deploy.py
```

The test reuses R3's fresh PostgreSQL/OpenSearch/Redis/Sequin and non-forwarding ADC/JWKS/SQS doubles. It installs **byte-identical copies** of the six host tooling files in a protected temporary tree, rather than changing shared-checkout permissions. It invokes the real CLI. The sole test build is a clearly labelled B-derived API wrapper: real preflight, exit42 on normal startup. That fault image is not a genuine source-built release or provenance proof.

A: source `672bdcefdeaabc6cd9f78461ec1bf859c31dc443`, immutable IDs in `deploy/tests/smoke-compose.py`.

B: source `08173849aa0a5f5849a340a1030b94af2162aee8`, actually rebuilt with the accepted Dockerfile; shared release compilation15m41s, each build bounded30min:

| Component | B local image ID |
|---|---|
| API | `sha256:829b7073da83e6be4e17843e18267c822a073b19fa60ae6dac159a1e4ab16655` |
| Worker | `sha256:9a54660aeff8bf2ba00cd258e013d464bdbb1e8e7c44c11103280a6344aa2a9e` |
| Cron | `sha256:1e3a0d90c9537d9ea0e3e368752c9b543a2dd4c04059779666892a11aad6a63b` |
| Crawler | `sha256:063c0f50186e12c49ea77990f43c6c1c7518c029db3be1545e8d6cde87f9fa5b` |

Build each target using `deploy/images/Dockerfile`, `--build-arg COMMIT_SHA=08173849aa0a5f5849a340a1030b94af2162aee8`, `--platform linux/amd64`, `--load`, and its actual source context. See `deploy/images/README.md` for the four-target command pattern. Do not relabel A. A/B Rust/Cargo/schema/search inputs are unchanged: these are different built release identities, **not changed business behavior**. Local labels and image inspection are not signatures or cryptographic source provenance; rebuilds may have different image IDs.

The rehearsal verifies candidate start before switch, exact active Caddy configuration and trusted HTTPS, old-API removal afterward, and Docker-event death/removal-before-create ordering for all12 workers/jobs. All13 B processes report B. Platform/edge IDs/start times/volumes and Sequin configuration remain unchanged. Bad candidate leaves B serving and previous A intact; blind apply, mixed-state recovery and missing-confirmation recovery reject. Flock contention is tested as a real process boundary.

The test has a30min bound and exact-owner cleanup. Unknown commands or unverified mixed state retain the printed journal/projects for inspection; never blindly rerun, delete by broad prefix, or prune. Test cleanup is not rollback. Caddy alone is technically egress-capable; fixture apps/platform are internal-only. No active SQS send/receipt/delete/redelivery or paid operation is credited.

## Install and prepare — operator authority required

Do not run real-host setup until R3 trust/bootstrap/provider gates are resolved. Installation and first adoption are deliberate operator steps, not actions triggered by a builder or release file.

Install these **six reviewed files together**, preserving their relative paths, in a root-controlled directory (for example `/opt/aura-historia`). Do not replace host tooling as part of an ordinary app release:

```text
deploy/bin/deploy
deploy/catalog.json
deploy/compose/compose.application.yml
deploy/compose/compose.replace.yml
deploy/compose/compose.edge.yml
deploy/compose/Caddyfile.replace
```

Directories root-controlled, not group/world-writable; data files0644; executable0755. Tooling reads files relative to its installation root. The full R3 platform setup remains a separate installation/maintenance concern. Keep this tree outside application-controlled mounts.

Create the environment state directory root-owned0700; **never unlink or replace its `lock` file**. Keep host config/Compose env/application env/ADC/CA inputs protected as described in `deploy/compose/README.md`. Host config and release input paths must be absolute. Real stages require root execution. Test runs use only the test UID's own protected temporary tree.

Example host JSON (operator selects actual names/paths/unused port; no secrets):

```json
{
  "stage": "dev",
  "application_project": "aura-dev-application",
  "edge_project": "aura-dev-edge",
  "config_dir": "/etc/aura-historia/dev",
  "compose_env": "/etc/aura-historia/dev/compose.env",
  "state_dir": "/var/lib/aura-historia/deploy/dev",
  "https_port": 8443,
  "https_ca": "/etc/aura-historia/dev/caddy-root.crt"
}
```

`https_ca` is the public CA for the **existing** local Caddy certificate, not the PostgreSQL CA. Caddy must already be running the rendered `Caddyfile.replace`: replace its two literal tokens with the observed active slot (`api` initially) and source SHA. This template enables admin **only at container-loopback127.0.0.1:2019**, and emits safe `X-Aura-Release` metadata. Do not publish2019. R3's `admin off` config is not silently upgraded by this command; initial Caddy setup is separate maintenance. Public origin/CloudFront remains R8.

Start and verify one existing release with R3 mechanisms before `adopt`. The release JSON has exactly `source_sha` and `images`, where images has exactly `api`, `worker`, `cron`, `crawler` and full local `sha256:` IDs. Select only trusted, compatible artifacts; no mutable tag, registry URL or arbitrary command/path can be supplied through that record. Load/pull and inspect exact artifacts separately with the approved registry identity. The command verifies image IDs, source labels/env and intended nonroot entrypoints, not build signatures or business compatibility.

Example command surface after operator setup:

```sh
/opt/aura-historia/deploy/bin/deploy --config /etc/aura-historia/dev/host.json status
/opt/aura-historia/deploy/bin/deploy --config /etc/aura-historia/dev/host.json adopt --release /etc/aura-historia/dev/release-a.json --api-slot api
/opt/aura-historia/deploy/bin/deploy --config /etc/aura-historia/dev/host.json apply --release /etc/aura-historia/dev/release-b.json
```

`adopt` records verified **running containers**, not database schema adoption. It runs no migrations and changes no runtime. It refuses an existing current/previous record or unresolved incomplete operation.

## Apply behavior

1. Acquire nonblocking flock. Refuse incomplete. Verify current records against actual containers/config/source/readiness and current Caddy disk/admin/HTTPS routing.
2. Freeze fixed host inputs in memory. Run each new binary's non-consuming `--check-config` in a one-off container. Temporary worker preflight never runs a consumer daemon.
3. Start only the unused API slot. Require readiness/source identity before touching old workers/jobs.
4. Sequentially stop the exact old worker/job ID, confirm exit0/no OOM/PID0, remove it, then start/check the same named service. No scheduler candidate overlaps its predecessor. Source/DLQ URLs remain unchanged; no queue API mutations are added by the host tool.
5. Recheck API; stage nonsecret Caddy file0644, validate, atomically replace canonical file, reload through the private admin listener, compare active/disk/container config and exclusive Docker upstream identity, verify trusted HTTPS. A Caddy header alone is not accepted routing proof.
6. Recheck candidate, drain/remove old API. Verify all components converged. Write previous/current, then remove incomplete durably.

State files are0600 with same-directory replace and file/directory fsync. `current` means **last completed release**, not a claim all components still run that release while incomplete exists. Incomplete contains old/target identities, previous snapshot, selected slot and the phase about to mutate; it deliberately does not contain secrets. Status validates stored shape before output. All command/provider outputs stay captured; only fixed failure categories and operational identities are printed.

Host config, credentials, installed tooling and manual container changes **must use the same flock**. Snapshots detect ordinary input drift before mutation; they cannot fence an uncooperative host-root/Docker administrator racing after a check. Do not change config during apply. Secret rotation is a separate controlled recycle; code rollback never restores old secrets.

Default API/worker/cron/crawler stop budgets are60/300/330/330s; Docker command bounds are larger. Overall command bound2h accommodates sequential worst-case drains; tests are bounded30min. Increasing these runtime budgets needs a coordinated host/Compose change, not just an env edit. Worker/cron/crawler failure or unclean exit halts handover rather than launching an overlapping successor.

Sequin is **not paused, restarted or reconfigured**. During worker absence its unacknowledged HTTP failures retry; existing SQS work remains with its existing queues. Before real use, verify persisted `max_retry_count=null`, `load_shedding_policy=pause_on_full`, adequate WAL/storage/retention and compatible payloads. Pre-existing paused/disabled sink settings remain untouched. Idle tests do not prove active retention/redelivery.

Never run whole-project `up`, `down`, `--remove-orphans` or scale commands once this command owns the application. The inactive API slot is explicitly removed; otherwise a naive `up` could create an extra slot. Ordinary restart policy restarts only existing active containers. Actual host/daemon reboot behavior remains untested.

## Failure and explicit recovery

Any failure after the first phase record leaves incomplete. No automatic rollback, cleanup, schema downgrade or lock stealing. A command timeout/SIGTERM/disconnection does **not** prove Docker-daemon work stopped. Even `status` may return busy while the lock is held; the atomic records remain available to the operator.

Hold the same lock during manual diagnosis/convergence. Confirm prior CLI/plugin/daemon work is finished; inspect exact recorded container IDs, actual processes, active Caddy route versus disk file, and which worker/job versions are running. Do not start a replacement while an earlier start/stop outcome is unknown. Preserve compatible completed effects; do not purge queues or reset schemas.

- Failed candidate before any handover: confirm outstanding work ended, stop/remove **only the identified failed slot**, leave current services and route intact.
- Partial handover or uncertain proxy reload: explicitly converge to the marker's original or target release using retained images and confirmed stop-before-start. Inspect/validate canonical Caddy file and actual active routing. Disk replacement and reload are not a transaction; never infer one from the other.
- If convergence/outstanding-command status cannot be proven, remain blocked. This is an operator recovery procedure, not automatic first-cutover recovery.

After manual convergence, release the diagnosis lock and invoke the verifier (it acquires that same lock; do not hold a separate flock while invoking it):

```sh
/opt/aura-historia/deploy/bin/deploy --config /etc/aura-historia/dev/host.json recover --release /etc/aura-historia/dev/release-b.json --api-slot api-candidate --commands-finished
```

`--commands-finished` is the operator's explicit confirmation, **not a detector or cancellation mechanism**. Recover performs no runtime mutations; it refuses mixed containers, wrong routes, an unrelated target or missing confirmation. It records only verified original/target convergence, preserves the correct prior release even across interrupted state-file writes, retains `last-incomplete` and then clears the active marker. Do not manually delete incomplete to make apply pass.

Previous-release redeploy can select retained original image IDs through the same apply path when compatibility and current host configuration permit. No historical rebuild or schema rollback exists. End-to-end backward redeploy, loaded-traffic drain, real queue custody and reboot are still separate acceptance work.
