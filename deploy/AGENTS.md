# DOX

## Purpose

- Own runnable single-machine development deployment: images, ordinary Compose, thin host command and checks.
- Reset playbook supersedes old numbered hybrid iterations. Do not continue old06–14.

## Contracts

- Read root and relevant `docs/arch.md`; current status: `docs/deployment/implementation-status.md`.
- Every accepted change must advance and exercise the runnable path. No standalone foundation/planner/renderer/evidence work.
- Use checked-in Compose, Caddy, ordinary restart policies, GitHub Environments/concurrency and Linux flock.
- Thin host command owns current/previous/incomplete files. Failure leaves incomplete and blocks blind follow-up. No CAS/intent protocol, distributed locks, owner election/reconciler or generic secret materializer.
- Long-lived containers use unless-stopped; explicitly retire old singleton containers before replacements. Confirm process termination, never infer it from lost connectivity.
- Ordinary releases do not restart stateful services, recreate volumes, purge queues or downgrade schemas.
- Fresh initialization only. No incremental adoption, backfill engine or new migrator. Missing fictional migrator must not block current releases.
- Host-owned restrictive env/CA/ADC files, never committed or printed. Dependency endpoints stay configurable; same-host Docker DNS is allowed.
- Owner target: current development machine hosts the complete self-hosted dev stage; AWS dev remains separate, regional resources preferably eu-central-1. Target selection is not blanket mutation authority. Owner identifies local nginx/opensearch system services as obsolete deployments and permits their stop/removal; preserve their on-disk data unless deletion is explicitly scoped. Unrelated services stay untouched. AWS root access does not imply local sudo.
- Live host/cloud/firewall/DNS/GitHub/prod mutations need explicit target authority and actual inputs. No guessed values or simulated success.
- Integrator owns shared Cargo/locks, Compose/scripts/workflows/CDK wiring/docs. At most three disjoint implementers. Independent review asks correctness AND whether existing mechanisms can do materially less custom work.

## Verification

- Prioritize actual image startup, full Compose launch, A→B/bad-B cutover, queue custody, singleton handover and reboot.
- Preserve TLS, redaction, signal and data-custody regression tests.
- R2 image smoke: `deploy/tests/README.md` has exact immutable inputs, isolation and evidence limits; Python harness is test-only, not deployment orchestration.
- Dormant helper checks: `npm --prefix deploy/control test`; `npm --prefix deploy/control run validate-catalog`.
- No deployment workflow enabled before the host path is proven.

## Index

- `catalog.json` — actual four native binaries, five Lambdas, ten scopes and existing assets.
- `control/`, `schemas/`, `bootstrap/` — dormant old framework; retain useful catalog/hash/tag/redaction helpers, do not expand.
- `images/` — shared multistage Dockerfile, four nonroot runnable application targets; separate fixture-only bootstrap/Python helper; stock stunnel5.80 sidecar for Sequin PostgreSQL verification.
- `tests/` — isolated real-image idle startup/signals, genuine fresh PostgreSQL/OpenSearch, non-forwarding provider doubles and ownership-safe cleanup.
- `compose/` — ordinary independent platform/application/edge projects; all13 apps tested together with empty isolated fixtures. Host config remains explicit; no live trust/bootstrap acceptance.
- R3 reproduction/gates: `compose/README.md`; `tests/smoke-compose.py` uses these files plus a test-only override. Only Caddy gets an extra edge bridge; app/platform fixtures stay internal. No real SQS custody or A→B claim.
- `bin/deploy`, `bin/README.md` — preloaded immutable app replacement; flock/current/previous/incomplete, two API slots, private Caddy reload, confirmed singleton removal and explicit verified recovery. Isolated idle A→B/bad-candidate accepted; no live/active-custody/reboot acceptance.
- `compose/compose.replace.yml`, `compose/Caddyfile.replace` — checked-in R4 overlay and local edge template; do not run unscoped Compose up after adoption.
- Host configuration/tooling/manual operations share the same flock. Freeze inputs, never clear unknown outcomes blindly. Recover confirms already-converged original/target only and preserves previous artifact references.
- `compose/compose.sequin-tls.yml` — stock Sequin + stunnel PostgreSQL-aware verified TLS; plaintext only on shared loopback. Metadata route is overridden; every source must explicitly use loopback15433. Platform replacement recreates both namespace-sharing services; not an app-release phase.
- `tests/smoke-sequin-tls.py` — actual isolated metadata/source SQL/WAL trust rejection and same-slot recovery. Exact Compose bytes pinned before parsing, credential-free fixtures, attributed failure evidence, owned cleanup; no live/custody/reboot claim.
