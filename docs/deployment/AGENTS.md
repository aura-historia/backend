# DOX

## Purpose

- Own single-machine development deployment decisions, status and operator runbooks.
- Owner reset playbook supersedes old numbered hybrid plan; do not continue old06–14 or reconcile both plans.
- Source code proves implemented behavior. Tests prove only their stated environment. Neither proves live deployment.

## Contracts

- Read root, `docs/AGENTS.md`, then here before edits.
- No live host/cloud/GitHub mutation without separate target-specific authorization.
- Keep iterations independently reviewed and committed on a non-deploying integration branch until safe activation exists.
- Record actual SHA, checks, reviewer, external gates and mixed/uncertain outcomes. Never label scaffolding deployable.
- No secrets, business payloads, receipt handles or provider bodies in examples or records.
- Application rollback never downgrades schemas, purges queues or restores backups.
- Binding reset: still in development; fresh PostgreSQL/extensions/business/crawler initialization only. No incremental framework, adoption, historical backfill, down-migration or rollback-schema machinery. No nonexistent migrator release prerequisite. Scope resolved; do not ask again. Existing databases are not implicitly disposable.
- Every accepted change must directly exercise runnable images, Compose or deployment. No renderer/planner/foundation-only commits. Retain runtime TLS/custody fixes; provisional NAT's next functional change must attach actual DB Lambdas or remove it.
- Use ordinary Compose/restart policies, Caddy, GitHub Environments/concurrency and flock/current/previous/incomplete. Explicit failed-deploy operator recovery is acceptable; no custom owner reconciler, distributed CAS or generic secret materializer.
- Independent review checks correctness AND materially simpler existing-mechanism alternatives.

## Index

- `inventory.md` — checkout evidence and unresolved deployed state.
- `architecture-decisions.md` — deployment boundaries, dependency/file ownership.
- `implementation-status.md` — active reset R1–R8, actual checks and blockers.
- `implementation-history.md`, `architecture-history.md` — superseded checkpoint evidence only; not an active roadmap.
