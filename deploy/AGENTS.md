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
- `images/` — shared multistage Dockerfile, four nonroot runnable application targets; separate fixture-only bootstrap/Python helper.
- `tests/` — isolated real-image idle startup/signals, genuine fresh PostgreSQL/OpenSearch, non-forwarding provider doubles and ownership-safe cleanup.
- `compose/` — ordinary independent platform/application/edge projects; all13 apps tested together with empty isolated fixtures. Host config remains explicit; no live trust/bootstrap acceptance.
- R3 reproduction/gates: `compose/README.md`; `tests/smoke-compose.py` uses these files plus a test-only override. Only Caddy gets an extra edge bridge; app/platform fixtures stay internal. No real SQS custody or A→B claim.
- Host-command files join this index only as their runnable milestone lands.
