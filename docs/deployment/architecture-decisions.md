# Deployment decisions — active reset

The owner's **Deployment Implementation Reset Playbook** supersedes the old hybrid plan. Old06–14 is not an active checklist. Prior discussion/evidence is archived in `architecture-history.md` and `implementation-history.md`; do not implement it alongside this reset.

## Objective and mechanism

Deploy the complete current backend on one isolated development machine and replace A with B safely. Use four OCI images, one shared worker image for ten scopes, ordinary checked-in platform/application Compose, Caddy, GitHub Environments/concurrency and a thin host command using flock plus current/previous/incomplete files. No renderer, generalized intent/CAS/plan protocol, distributed ownership, reconciler or generic secret materializer.

Long-lived active containers use an ordinary restart policy such as unless-stopped. Remove/stop retired singleton containers and confirm old process termination before starting successors. Cron advisory-session loss is not transaction fencing. A failed deployment leaves inspectable incomplete state and blocks blind follow-up; explicit operator recovery is acceptable.

Application releases leave PostgreSQL/OpenSearch/Sequin state and volumes alone. Endpoints remain runtime-configurable; Docker service DNS is appropriate on one machine. Restrictive host-owned env/CA/ADC files suffice; do not log their contents. No live operation is authorized by code, credentials or a dev stage name.

## Scope and retained code

- Keep proven TLS, signals, preflight, custody/checkpoint and fresh-initialization fixes.
- Remove the rejected08a custom Compose renderer/code/tests/docs (`f759169db...`); no replacement abstraction.
- Keep `LambdaEgress` (`5a90c8dde...`) provisionally. Its next functional change must instantiate it and attach actual DB Lambdas with CA delivery, or remove it. No more standalone expansion.
- Keep reusable catalog, hash, CalVer and redaction helpers. Old framework/schema/state code may remain dormant; it is not the new deployment dependency.
- **Migration question resolved:** fresh PG/extensions/current business and crawler histories only. No incremental adoption, historical backfills, down migrations, schema-rollback or migration compatibility framework. No nonexistent migrator release prerequisite.
- Current scope R1–R8 and evidence live only in `implementation-status.md`. No merge, production deployment, force-push, reset or mass revert.

## Integration and review

Small reviewed commits on the existing deployment branch. At most three implementation agents, disjoint files. Integrator owns shared Cargo/locks, Compose/scripts/workflows/CDK wiring/docs. Every reviewer answers: is it correct, and could existing mechanisms meet the requirement with materially less custom code? A schema/planner/foundation-only result is not accepted.

Actual orchestrator: GPT-6-Astra. Delegation offers no model selector; record each agent's actual model, never claim requested Terra selection.

## ADR-004 — retained SQLx trust exception

Pinned SQLx0.9.0 transient ambient option reads remain confined to `platform-postgres`: skip pgpass, reject inherited client-certificate/key/CA/options, overwrite explicit fields, cache. Dev/prod require VerifyFull and supplied CA; supplied CA plus WebPKI trust is not exclusive pinning. No client-certificate support advertised. Credential/CA changes require client/pool recycling; a Lambda CA path alone does not deliver a file. Full historical rationale remains in `architecture-history.md`.

## ADR-005 — retained crawler terminal fence

Crawler's Unix terminal boundary retains its reviewed `libc::_exit` fence because ordinary Rust exit cleanup can block. Owned workloads/pools/exporter/runtime/output must finish inside absolute deadlines before success; forced exit is nonzero/unknown, not durable completion. Silent sticky panic failure cannot extend deadlines. This narrow existing runtime safety exception is not a deployment framework or permission to skip confirmed process termination. Full rationale remains in `architecture-history.md`.
