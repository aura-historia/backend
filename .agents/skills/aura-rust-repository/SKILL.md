---
name: aura-rust-repository
description: Use when adding or changing Aura Historia Rust aggregate repositories, PostgreSQL repository adapters, repository factories, rows, aggregate rehydration, storage versions, or optimistic concurrency.
---

# Aura Rust Repository

Use for aggregate persistence. Not for presentation reads.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- `docs/arch.md` §3, §5, §8, §10.2-10.3, §11, §17, §20.5, §21-23.
- Load `aura-rust-enum` too when persisted enum/text mapping changes.

## Before coding

- Confirm repository is for one aggregate's authoritative persistence.
- Confirm operational owner, normally PostgreSQL.
- Find service-owned repository port and adapter crate.
- Decide if repository must be transaction-scoped.

## Hard rules

- Repository reconstructs and persists aggregates.
- Repository port belongs to service crate and is public because adapter crates implement it.
- PostgreSQL adapter owns SQLx rows, SQL, mappings, scoped repository implementation, and concrete factory.
- Repository MUST NOT become a general read API.
- Presentation joins, search, recommendations, analytics, details, and user-state belong to readers.
- Use transaction-bound factories: `RepositoryFactory<Tx>::in_transaction(&mut tx) -> impl Repository`.
- Concrete transaction-scoped repository should stay private.
- Adapter rows, SQL params, and mapping helpers stay private or `pub(crate)`.
- Aggregate fields stay private.
- Rehydration API is adapter-facing, validates persisted state, and emits no new domain events.
- PostgreSQL rows should derive `sqlx::FromRow`.
- Row-to-aggregate mapping uses `TryFrom` when persisted state can be invalid.
- `find_by_id` returns `Option`; `get_by_id` means absence is an error.
- `insert` means new aggregate and returns persisted aggregate state.
- `update` means existing aggregate, receives loaded storage version, enforces optimistic concurrency, and returns persisted state.
- No returned update row maps to concurrency conflict when row was expected.
- SQL update increments version exactly once with `version = version + 1`.
- Do not expose concrete version values in ordinary use-case results.
- Write serialization from aggregate to SQL happens inside repository/DAO.
- Do not add public `From<&Aggregate> for Row` write conversions.
- Operational metadata such as created/updated belongs to persistence/readers unless domain invariants need it.

## Avoid

- Generic `Repository<T, Id>` or vague `save` semantics.
- Repository methods named after UI needs.
- Storage row/document escaping adapter.
- Search/key-value/cache projection acting as aggregate repository unless documented authoritative owner.
- Making private adapter details public for tests.

## Tests

- PostgreSQL repository tests should use real PostgreSQL beside implementation.
- Cover row mapping, aggregate rehydration, insert/update semantics, optimistic concurrency, rollback, cross-entity transaction use, and migration compatibility.
