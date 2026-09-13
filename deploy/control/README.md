# Dormant deployment helpers

**The deployment reset supersedes the old controller design.** No workflow invokes this package for live deployment. Do not extend its intent/plan/CAS/evidence/ownership protocol or generate a replacement Compose renderer. Use ordinary checked-in Compose and a thin flock-based host command instead.

Retain useful catalog, byte/hash, CalVer and redaction helpers. `release validate-catalog` checks actual Cargo/CDK binaries/scopes and current mail/search/schema assets. The actual catalog contains four native binaries, five Lambdas and ten scopes; no nonexistent migrator blocks current releases. Missing real required components still fail. Native images/new deployment need no legacy manifest or migration-compatibility metadata.

```sh
npm --prefix deploy/control ci
npm --prefix deploy/control test
npm --prefix deploy/control run validate-catalog
```

Node26 target, committed package lock. Unsupported old CLI commands still fail nonzero; they never pretend deployment success. Old schema/type/fixture/state logic remains dormant for now, not a runnable release system. Historical fixture migrator identity remains readable only to preserve unrelated tests; installed catalog rejects extra/nonexistent components. Removing that identity from a four-image record now passes native completeness checks. Other historical metadata remains unused by the reset path, not an implementation prerequisite.

Legacy schema parsers also perform semantic checks not captured by JSON Schema. Preserve their regression checks while dormant; do not expand the framework. Deleted08a generated build outputs must not survive a clean test build.

Active decisions/status: `docs/deployment/architecture-decisions.md`, `docs/deployment/implementation-status.md`. Historical protocol details: `docs/deployment/architecture-history.md`. No live approval, provider execution or distributed-lock claim.
