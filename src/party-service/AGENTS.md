# DOX

## Purpose

- Own `party-service` crate.
- Own Party use-case contracts, handlers, and outbound ports.

## Core Design

- Depends on `party-core`, shared `application` contracts, and `user-service` admin policy.
- Root modules: `ports`, `use_cases`.
- Create, update, and internal details use cases own transaction scope through `application::transaction::UnitOfWork` and transaction-scoped Party repository factories.
- Party mutations and internal details require the established admin-or-service/system policy in the service layer.
- Repository writes return storage-neutral Party state. No use case rereads after a write.
- No SQLx or transport dependency.

## Ownership

- This doc rules `src/party-service/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p party-service`
- `cargo test -p party-service --all-features`

## Child DOX Index

- None.
