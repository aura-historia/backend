# Repository discovery map

Authored against `develop@c77155fd7872461b41ccedbdd36492bf9595dbb1` on 2026-10-11.
Read current files each run. This is a discovery map, not a fixed baseline or architecture
inventory. Paths below are relative to the repository root. Discover renamed owners through
`docs/README.md`; do not silently omit a missing required owner.

| Concern | Read first |
| --- | --- |
| Workflow and validation | `AGENTS.md`, applicable nested guidance, `rust-toolchain.toml`, `Cargo.toml`, `infra/package.json`, `.github/workflows/` |
| Ownership and dependency direction | `docs/arch.md`, `depgraph-rules.toml` |
| Persistence and concurrency | `docs/storage.md`, `migrations/`, owning adapters/tests |
| Ingestion and revisions | `docs/product-listing.md`, `docs/object-ids.md` |
| CDC, acknowledgement and replay | `docs/events/flow.md`, `docs/durable-worker-runbook.md` |
| Consent, provider evidence and permissions | `docs/marketing-consent.md` |
| HTTP/auth/cache contracts | `docs/http-api-front-door.md`, `docs/swagger.yaml`, `docs/CHANGELOG.md` |
| Deployment and recovery | `docs/infra.md`, `docs/opensearch-stage.md`, `infra/`, `ci/` |
| Crawler and untrusted content | `docs/crawler/README.md`, `src/crawler/`, `src/public-network-policy/`, `src/image-fetcher/` |

Important counterexamples to generic cleanup: composition roots may wire concrete adapters;
Gateway's lack of a Cognito authorizer is intentional when Axum enforces the documented
Cognito/Aura contract; domain events are not mandatory in every aggregate. PostgreSQL
remains business authority unless an owner explicitly says otherwise. Queue admission is
not business completion, and OpenSearch is rebuildable read state. Keep `#[source]` and
`application::error::BoxError`, stable semantic identifiers/TypeID text, persisted
`SCREAMING_SNAKE_CASE` enums and canonical standardized codes. Current specialized owners
control details. Do not copy implementation inventories into `docs/arch.md`/`docs/storage.md`.
