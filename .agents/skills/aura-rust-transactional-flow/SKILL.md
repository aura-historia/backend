---
name: aura-rust-transactional-flow
description: Use when adding or changing Aura Historia Rust write flows that need transactions, UnitOfWork, transaction-scoped repository or reader factories, idempotency, multiple repositories, or cross-datasource boundaries.
---

# Aura Rust Transactional Flow

Use for service-owned transactional write orchestration.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- `docs/arch.md` §6-8, §11, §15, §17, §20.4-20.5, §21-23.

## Before coding

- Identify invariant-critical PostgreSQL work.
- Identify all repositories and readers that must share one transaction.
- Identify external reads/writes that cannot join PostgreSQL transaction.
- Decide idempotency key behavior if retries can duplicate side effects.

## Hard rules

- Service-owned use-case handler defines transaction scope.
- Handler begins an abstract `UnitOfWork` transaction.
- Handler binds transaction-scoped repositories/readers through factories.
- Handler executes domain behavior and authoritative writes.
- Successful write transaction ends with explicit `commit().await`.
- Dropping or closing a transaction is rollback, not commit.
- Unit-of-work traits expose transaction lifecycle only; no entity-specific methods.
- Repositories expose clean methods without transaction argument.
- Factory binds repository to active transaction through `.in_transaction(&mut tx)`.
- Prefer chained temporary repositories so mutable transaction borrows end at semicolon.
- Several PostgreSQL repositories may share one abstract transaction when using compatible Tx type.
- Reads that influence invariant-critical writes must use the same transaction through transaction-bound reader factories.
- Do not call a pool-backed reader on another connection when its result must be consistent with active transaction.
- Ordinary presentation reads do not need a transaction unless several SQL statements need one consistent snapshot; document exceptions.
- PostgreSQL transaction cannot atomically include search, key-value store, graph store, external API, or broker without specific transaction protocol.
- Avoid holding PostgreSQL transaction open while waiting on slow external sources. Read external data first when safe, then open short transaction and revalidate.
- Idempotency handling belongs to the use-case transaction boundary.
- Do not read after write only to build command response; repository write result is source for response.

## Avoid

- Hidden distributed transaction.
- SQLx or concrete adapter imports in service handler.
- Pool-backed reads during invariant-critical transaction.
- Transaction abstraction that becomes a service locator.
- Commit hidden inside repository method.

## Tests

- Service tests cover begin/commit behavior, no commit on failure, same transaction across factories, authorization before/inside transaction as designed, idempotency, concurrency conflicts, rollback behavior, and skipping persistence when no state changed.
- PostgreSQL adapter tests cover transaction-scoped repositories and rollback with real PostgreSQL.
