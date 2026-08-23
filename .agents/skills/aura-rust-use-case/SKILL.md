---
name: aura-rust-use-case
description: Use when adding or changing Aura Historia Rust service use cases, commands, queries, inbound traits, service handlers, service errors, authorization policy calls, or outbound ports.
---

# Aura Rust Use Case

Use for service-crate use-case work.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- `docs/arch.md` §3, §4-7, §13-15, §17, §19-23.

## Before coding

- Identify bounded context and aggregate.
- Decide command vs query.
- Find owner `<entity>-service` crate.
- Reuse smallest capability-oriented outbound ports.
- If adding a new general rule, update `docs/arch.md`.

## Hard rules

- Use-case contract and handler live in `<entity>-service/src/use_cases/...`. One use-case per file.
- One use-case file owns command/request, result/view, use-case error, inbound trait, and focused handler.
- Commands express business intent: `Create*`, `Rename*`, `Archive*`, `Publish*`.
- Broad `Update*` only for intentional PATCH. Use service-owned `Update*Command` and `common::patch_field::PatchField`.
- Handler depends only on core types, service-owned ports, transaction abstractions, and real cross-service contracts.
- Handler MUST NOT import SQLx, `PgPool`, database clients, OpenSearch clients, adapter rows/documents/items, or concrete adapter factories.
- Controllers depend on inbound use-case traits, not handlers or adapters.
- Outbound ports name application capability, not technology.
- Final read model belongs to the use case, not a data source.
- Protected mutations use trusted `OperationContext` and reject anonymous principals.
- Do not accept actor/user identity from JSON body, query, or path as trusted auth.
- Use-case errors expose stable semantic variants; keep infrastructure causes as sources.
- Externally invoked use cases should have safe `tracing` spans with request/correlation/principal/target fields.
- Use workspace object-safe async trait convention, currently `async_trait`, for dyn inbound traits.

## Avoid

- God service with dependencies for unrelated use cases.
- Vague command names with optional bags of fields.
- Business authorization in controllers.
- Domain or service dependency on infrastructure.
- Catch-all errors when caller-relevant cause is known.
- Sensitive payloads or credentials in logs.

## Tests

- Add domain tests for new invariants.
- Add service handler orchestration tests with fakes/mocks beside the use-case implementation.
- Test authorization, error translation, transaction behavior, batching, and no-op persistence skip when relevant.
