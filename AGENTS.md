# Repository guidance

## Layout

- This is a Rust workspace for an AWS serverless backend.
- `src/` contains workspace crates; `infra/` contains CDK infrastructure; `migrations/` contains business PostgreSQL migrations; `docs/` contains durable architecture and public-contract documentation.
- `mjml/` and `opensearch/` contain shared email and OpenSearch assets.

## Work safely

- Read the relevant code, tests, configuration, and canonical documentation before editing.
- Keep changes focused. Do not change runtime behavior unless the task requires it.
- Treat `docs/arch.md` as the architecture source of truth. Explain and document intentional general deviations.
- Prefer existing types, patterns, and dependencies over introducing new abstractions or packages.
- Keep persisted formats and public identifiers stable. Persist enum values in `SCREAMING_SNAKE_CASE`; retain canonical standardized identifiers such as ISO language codes.

## Architecture

- Preserve dependency direction: domain code does not depend on infrastructure.
- Keep API, Lambda, worker, and other runtime code thin. Business use cases belong in service crates; domain behavior belongs in core crates.
- Service code owns use-case orchestration and transaction boundaries.
- Repositories persist aggregates; readers build read models. Do not use repositories for presentation reads.
- Keep storage rows, provider payloads, search documents, and transport DTOs within their adapter boundaries. Map them explicitly.
- PostgreSQL is authoritative business state unless a bounded context explicitly documents otherwise. OpenSearch and other projections are rebuildable read state.
- Do not introduce hidden distributed transactions, controller orchestration, or N+1 hydration.

## Security and contracts

- Fail closed on invalid persisted state and untrusted external input.
- Do not log credentials, tokens, raw provider payloads, or other sensitive content.
- Update `docs/swagger.yaml` and `docs/CHANGELOG.md` when a public API contract changes.
- Update the relevant event or storage documentation when a durable event, persistence, or operational contract changes.

## Documentation

- Document durable architectural, operational, security, and public-contract knowledge; leave code-level detail to code and tests.
- Amend the existing canonical document when one covers the contract instead of creating overlapping documentation.
- Keep documentation concise and identify stable ownership boundaries, failure behavior, and operator requirements.

## Validation

Start with focused checks and tests for changed crates. Run broader validation when the change warrants it:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo depgraph-check check
cargo test --workspace --lib --all-features
npm --prefix infra test
npm --prefix infra run synth:all
```
